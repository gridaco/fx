//! The retry owner: the engine is the only one (docs/wg/overview.md "Retry and billing
//! (ratified)"; spec/protocol.md §6.1 step 7; spec/store.md §5). Adapters make one send per call
//! and report what happened (`grida_fx_providers::Sent`, `Submitted`, `Collected`); this module
//! decides what follows, and reserves, records and settles every attempt.
//!
//! **Sends.** At most [`MAX_SENDS`] sends per call: every send of the request counts, whether it
//! starts a new attempt or resends under the current one. Before each send the route's slot and
//! pacing are taken ([`super::pacing::Admission::admit`]; `Pacing` in a run). Between sends
//! the owner waits [`Backoff`] (0.5 s, doubling, at most 8 s).
//!
//! **A plain call, attempt by attempt** (each attempt is its own hold, `ledger::hold_id`):
//!
//! | outcome of a send | hold | next |
//! |---|---|---|
//! | `NotReceived` | kept open | resend under the same attempt (sends left), else settle $0 → `Failed` |
//! | `Refused` | settled $0 | `Refused` (`capability_refused`), never retried |
//! | `Answered`, check passes | settled at the reported cost (whole hold when none) | `Answered` |
//! | `Answered`, check refuses | settled as billed (reported cost, else whole hold) | new attempt (sends left), else `Failed` |
//! | `Failed { retryable: true }` | settled as billed | new attempt (sends left), else `Failed` |
//! | `Failed { retryable: false }` | settled as billed | `Failed` |
//! | cancelled while a send is in flight | settled in full | `Cancelled` |
//! | cancelled before a send leaves | settled $0 | `Cancelled` |
//!
//! A reservation that does not fit ends the call with `Ceiling` (`ceiling_exceeded`); attempts
//! already made stay settled.
//!
//! **A long job** ([`submit_job`]): reserve; write the job record `submitting` before the submit
//! may leave; then
//! - `NotReceived`: resend the submit (sends left), else remove the record, settle $0, `Failed`;
//! - `Refused`: remove the record, settle $0, `Refused`;
//! - `Uncertain`: the record stays `submitting`, settle in full, `Unsettled` (`job_unsettled`);
//! - `Failed`: the record becomes `settled`, settle as billed, new attempt when retryable;
//! - `Accepted { handle }`: the record becomes `submitted` with the handle, then the job is
//!   collected under the same hold ([`collect_job`] rules, but the hold is settled: the reported
//!   cost of the answer, else in full);
//! - cancelled while the submit is in flight: the record stays `submitting`, settle in full,
//!   `Cancelled`; cancelled while collecting: the record stays `submitted`, settle in full.
//!
//! **Collecting** ([`collect_job`], a `submitted` record found before the call): no hold, no new
//! submit, nothing billed (`charged` 0). `Answered` → the caller publishes the call record and then
//! removes the job record; a check that refuses the answer settles the record and fails the call;
//! `Ended` → the record becomes `settled`, `Failed`; `Unreachable` → the record stays
//! `submitted`, `Failed`; cancelled → the record stays `submitted`, `Cancelled`.
//!
//! Every hold emits `budget_reserved` and `budget_settled` through the ledger. The owner never
//! writes a call record and never emits an event itself; the call path does both, after
//! `Answered`.
//!
//! **Details the table leaves open.**
//! - Each attempt's request carries its number (`CallRequest::attempt`, 1 to 6); a resend carries
//!   the number of the attempt it resends. A collect of an earlier run's job carries the number
//!   the caller gave it, at least 1.
//! - The wait after a billed attempt comes before the next reservation, so no hold is open while
//!   the owner waits; the wait after `NotReceived` keeps the attempt's hold open.
//! - Every way out settles every hold the call opened: a refusal, a failure, a cancellation and
//!   a store error alike. A store error settles the hold as the attempt stood: $0 when nothing
//!   left yet, as billed after a reported failure, in full once the provider took the job.
//! - The route's slot ([`super::pacing::Slot`]) is taken for each send, submit and collect and
//!   given back as soon as it returns. Collects are not sends: a job is collected once per call.
//! - While a long job's submit waits to be resent after `NotReceived`, its record stays
//!   `submitting` (a crash then asks a person, never submits twice); a cancellation during that
//!   wait removes it, since nothing is outstanding.
//! - A `settled` record keeps the job's handle when it had one, so a person can find the job.
//! - A long-job call given no job record fields ([`Attempts::job`] `None`) is an engine fault:
//!   `Store`, before anything is reserved or sent.

use super::pacing::{Admission, Slot};
use crate::engine::Cancel;
use crate::ledger::{Hold, Ledger, Refusal, Scopes};
use crate::store::records::{JobRecord, JobState};
use crate::store::{Store, StoreError};
use grida_fx_core::money::Usd;
use grida_fx_providers::{
    Answer, BoxFuture, CallRequest, Collected, LongJob, RequestAdapter, Sent, Submitted,
};
use serde_json::Value;
use std::time::Duration;

/// The most sends of one call's request (module doc).
pub const MAX_SENDS: u32 = 6;

/// The wait between sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub initial: Duration,
    pub factor: u32,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            initial: Duration::from_millis(500),
            factor: 2,
            max: Duration::from_secs(8),
        }
    }
}

impl Backoff {
    /// The wait after the `n`-th send (1-based): `initial × factor^(n-1)`, at most `max`. `n` 0
    /// is taken as 1; a product too large for a `Duration` is `max`.
    pub fn after(&self, n: u32) -> Duration {
        self.factor
            .checked_pow(n.saturating_sub(1))
            .and_then(|growth| self.initial.checked_mul(growth))
            .map_or(self.max, |wait| wait.min(self.max))
    }
}

/// Where holds go: the ledger in a run, a fake in tests.
pub trait HoldBook: Send + Sync {
    fn reserve(&self, node_id: String, amount: Usd, scopes: &Scopes) -> Result<Hold, Refusal>;
    fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd;
}

impl HoldBook for Ledger {
    fn reserve(&self, node_id: String, amount: Usd, scopes: &Scopes) -> Result<Hold, Refusal> {
        Ledger::reserve(self, node_id, amount, scopes)
    }

    fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd {
        Ledger::settle(self, hold, reported)
    }
}

/// Where job records go: the store in a run, a fake in tests.
pub trait JobBook: Send + Sync {
    fn save_job(&self, record: &JobRecord) -> Result<(), StoreError>;
    fn remove_job(&self, key: &str) -> Result<(), StoreError>;
}

impl JobBook for Store {
    fn save_job(&self, record: &JobRecord) -> Result<(), StoreError> {
        Store::save_job(self, record)
    }

    fn remove_job(&self, key: &str) -> Result<(), StoreError> {
        Store::remove_job(self, key)
    }
}

/// Everything one call's attempts share.
pub struct Attempts<'a> {
    /// The request; `attempt` is set per attempt.
    pub call: CallRequest,
    /// The route's high price for this request: what each attempt reserves.
    pub hold: Usd,
    pub scopes: &'a Scopes,
    /// Names each new hold (`ledger::hold_id`).
    pub hold_name: &'a (dyn Fn() -> String + Sync),
    pub limit: Option<u32>,
    pub rpm: Option<u32>,
    pub book: &'a dyn HoldBook,
    pub jobs: &'a dyn JobBook,
    pub pacing: &'a dyn Admission,
    pub cancel: &'a Cancel,
    pub backoff: Backoff,
    /// The job record's key fields (`state` and `handle` are set by the owner); long jobs only.
    pub job: Option<JobRecord>,
}

/// How a call ended (module doc).
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The answer, and what its attempt was charged (0 for a collected job).
    Answered {
        answer: Answer,
        charged: Usd,
        attempts: u32,
    },
    /// `capability_refused`.
    Refused(String),
    /// `ceiling_exceeded`.
    Ceiling(Refusal),
    /// `call_failed`: the last reason.
    Failed(String),
    /// `job_unsettled`.
    Unsettled(String),
    /// The run stopped.
    Cancelled,
    /// The store could not keep a job record: the run stops (spec/store.md §4).
    Store(StoreError),
}

/// Makes a plain call (module doc).
pub async fn send_plain(adapter: &dyn RequestAdapter, attempts: &Attempts<'_>) -> Outcome {
    let route_id = attempts.call.route.id();
    let mut call = attempts.call.clone();
    let mut sends = 0;
    let mut attempt = 0;
    let mut resend: Option<Hold> = None;
    loop {
        let hold = match resend.take() {
            Some(hold) => hold,
            None => match attempts.reserve() {
                Ok(hold) => {
                    attempt += 1;
                    call.attempt = attempt;
                    hold
                }
                Err(refusal) => return Outcome::Ceiling(refusal),
            },
        };
        let Some(slot) = attempts.admit(&route_id).await else {
            attempts.settle(hold, Some(Usd::ZERO));
            return Outcome::Cancelled;
        };
        sends += 1;
        let sent = race(adapter.send(&call), attempts.cancel).await;
        drop(slot);
        let reason = match sent {
            None => {
                attempts.settle(hold, None);
                return Outcome::Cancelled;
            }
            Some(Sent::NotReceived { reason }) => {
                if sends >= MAX_SENDS {
                    attempts.settle(hold, Some(Usd::ZERO));
                    return Outcome::Failed(reason);
                }
                if !attempts.wait(sends).await {
                    attempts.settle(hold, Some(Usd::ZERO));
                    return Outcome::Cancelled;
                }
                resend = Some(hold);
                continue;
            }
            Some(Sent::Refused { reason }) => {
                attempts.settle(hold, Some(Usd::ZERO));
                return Outcome::Refused(reason);
            }
            Some(Sent::Answered(answer)) => match adapter.check(&call, &answer) {
                Ok(()) => {
                    let charged = attempts.settle(hold, answer.cost);
                    return Outcome::Answered {
                        answer,
                        charged,
                        attempts: attempt,
                    };
                }
                Err(reason) => {
                    attempts.settle(hold, answer.cost);
                    reason
                }
            },
            Some(Sent::Failed {
                reason,
                cost,
                retryable,
            }) => {
                attempts.settle(hold, cost);
                if !retryable {
                    return Outcome::Failed(reason);
                }
                reason
            }
        };
        // The attempt failed as billed: a new attempt, when sends are left.
        if sends >= MAX_SENDS {
            return Outcome::Failed(reason);
        }
        if !attempts.wait(sends).await {
            return Outcome::Cancelled;
        }
    }
}

/// Submits a long job and collects it (module doc).
pub async fn submit_job(adapter: &dyn LongJob, attempts: &Attempts<'_>) -> Outcome {
    let Some(fields) = attempts.job.as_ref() else {
        return no_job_fields(&attempts.call);
    };
    let route_id = attempts.call.route.id();
    let mut call = attempts.call.clone();
    let mut sends = 0;
    let mut attempt = 0;
    let mut resend: Option<Hold> = None;
    loop {
        let resending = resend.is_some();
        let hold = match resend.take() {
            Some(hold) => hold,
            None => match attempts.reserve() {
                Ok(hold) => {
                    attempt += 1;
                    call.attempt = attempt;
                    hold
                }
                Err(refusal) => return Outcome::Ceiling(refusal),
            },
        };
        let Some(slot) = attempts.admit(&route_id).await else {
            if resending {
                // The `submitting` record of the submit that was not received.
                return attempts.unsent(fields, hold, Outcome::Cancelled);
            }
            attempts.settle(hold, Some(Usd::ZERO));
            return Outcome::Cancelled;
        };
        // spec/store.md §5: `submitting` is written before the submit may leave.
        if let Err(error) = attempts.save(fields, JobState::Submitting, None) {
            drop(slot);
            attempts.settle(hold, Some(Usd::ZERO));
            return Outcome::Store(error);
        }
        sends += 1;
        let submitted = race(adapter.submit(&call), attempts.cancel).await;
        drop(slot);
        let reason = match submitted {
            None => {
                // It may have left: the record stays `submitting`.
                attempts.settle(hold, None);
                return Outcome::Cancelled;
            }
            Some(Submitted::NotReceived { reason }) => {
                if sends >= MAX_SENDS {
                    return attempts.unsent(fields, hold, Outcome::Failed(reason));
                }
                if !attempts.wait(sends).await {
                    return attempts.unsent(fields, hold, Outcome::Cancelled);
                }
                resend = Some(hold);
                continue;
            }
            Some(Submitted::Refused { reason }) => {
                return attempts.unsent(fields, hold, Outcome::Refused(reason));
            }
            Some(Submitted::Uncertain { reason }) => {
                attempts.settle(hold, None);
                return Outcome::Unsettled(reason);
            }
            Some(Submitted::Failed {
                reason,
                cost,
                retryable,
            }) => {
                let saved = attempts.save(fields, JobState::Settled, None);
                attempts.settle(hold, cost);
                if let Err(error) = saved {
                    return Outcome::Store(error);
                }
                if !retryable {
                    return Outcome::Failed(reason);
                }
                reason
            }
            Some(Submitted::Accepted { handle }) => {
                if let Err(error) = attempts.save(fields, JobState::Submitted, Some(&handle)) {
                    attempts.settle(hold, None);
                    return Outcome::Store(error);
                }
                return collect_held(adapter, attempts, fields, &call, &handle, hold, attempt)
                    .await;
            }
        };
        // The attempt failed as billed: a new attempt, when sends are left.
        if sends >= MAX_SENDS {
            return Outcome::Failed(reason);
        }
        if !attempts.wait(sends).await {
            return Outcome::Cancelled;
        }
    }
}

/// Collects a job an earlier run submitted (module doc).
pub async fn collect_job(
    adapter: &dyn LongJob,
    attempts: &Attempts<'_>,
    handle: &Value,
) -> Outcome {
    let Some(fields) = attempts.job.as_ref() else {
        return no_job_fields(&attempts.call);
    };
    let mut call = attempts.call.clone();
    call.attempt = call.attempt.max(1);
    match collect_once(adapter, attempts, &call, handle).await {
        Collect::Cancelled => Outcome::Cancelled,
        Collect::Answered(answer) => Outcome::Answered {
            answer,
            charged: Usd::ZERO,
            attempts: 0,
        },
        Collect::Refused { reason, .. } | Collect::Ended(reason) => {
            match attempts.save(fields, JobState::Settled, Some(handle)) {
                Ok(()) => Outcome::Failed(reason),
                Err(error) => Outcome::Store(error),
            }
        }
        Collect::Unreachable(reason) => Outcome::Failed(reason),
    }
}

/// Collects the job just accepted under its attempt's `hold`, and settles the hold.
async fn collect_held(
    adapter: &dyn LongJob,
    attempts: &Attempts<'_>,
    fields: &JobRecord,
    call: &CallRequest,
    handle: &Value,
    hold: Hold,
    attempt: u32,
) -> Outcome {
    match collect_once(adapter, attempts, call, handle).await {
        Collect::Cancelled => {
            // Still running at the provider: the record stays `submitted`.
            attempts.settle(hold, None);
            Outcome::Cancelled
        }
        Collect::Answered(answer) => {
            let charged = attempts.settle(hold, answer.cost);
            Outcome::Answered {
                answer,
                charged,
                attempts: attempt,
            }
        }
        Collect::Refused { reason, cost } => {
            let saved = attempts.save(fields, JobState::Settled, Some(handle));
            attempts.settle(hold, cost);
            match saved {
                Ok(()) => Outcome::Failed(reason),
                Err(error) => Outcome::Store(error),
            }
        }
        Collect::Ended(reason) => {
            let saved = attempts.save(fields, JobState::Settled, Some(handle));
            attempts.settle(hold, None);
            match saved {
                Ok(()) => Outcome::Failed(reason),
                Err(error) => Outcome::Store(error),
            }
        }
        Collect::Unreachable(reason) => {
            // It may still be running: the record stays `submitted` for a later run.
            attempts.settle(hold, None);
            Outcome::Failed(reason)
        }
    }
}

/// What one collect came to, its answer checked.
enum Collect {
    Answered(Answer),
    /// The adapter's check refused the answer; `cost` is what the provider reported.
    Refused {
        reason: String,
        cost: Option<Usd>,
    },
    Ended(String),
    Unreachable(String),
    Cancelled,
}

/// Collects once under the route's slot, racing cancellation.
async fn collect_once(
    adapter: &dyn LongJob,
    attempts: &Attempts<'_>,
    call: &CallRequest,
    handle: &Value,
) -> Collect {
    let Some(slot) = attempts.admit(&call.route.id()).await else {
        return Collect::Cancelled;
    };
    let collected = race(adapter.collect(call, handle), attempts.cancel).await;
    drop(slot);
    match collected {
        None => Collect::Cancelled,
        Some(Collected::Answered(answer)) => match adapter.check(call, &answer) {
            Ok(()) => Collect::Answered(answer),
            Err(reason) => Collect::Refused {
                reason,
                cost: answer.cost,
            },
        },
        Some(Collected::Ended { reason }) => Collect::Ended(reason),
        Some(Collected::Unreachable { reason }) => Collect::Unreachable(reason),
    }
}

/// A provider request raced against cancellation: `None` when the run stopped first. An answer
/// that is ready is taken even when the run stopped at the same moment, since it was paid for.
async fn race<T>(request: BoxFuture<'_, T>, cancel: &Cancel) -> Option<T> {
    tokio::select! {
        biased;
        out = request => Some(out),
        () = cancel.cancelled() => None,
    }
}

/// The engine handed a long-job call no job record fields (module doc).
fn no_job_fields(call: &CallRequest) -> Outcome {
    Outcome::Store(StoreError::Io {
        what: format!("jobs/{}.json", call.key),
        reason: "the engine gave a long job no job record to keep".into(),
    })
}

impl Attempts<'_> {
    /// Reserves a new attempt's hold under a new name.
    fn reserve(&self) -> Result<Hold, Refusal> {
        self.book
            .reserve((self.hold_name)(), self.hold, self.scopes)
    }

    fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd {
        self.book.settle(hold, reported)
    }

    /// Takes the route's slot and pacing for one send; `None` when the run stopped first.
    async fn admit(&self, route_id: &str) -> Option<Slot> {
        if self.cancel.is_cancelled() {
            return None;
        }
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => None,
            slot = self.pacing.admit(route_id, self.limit, self.rpm, self.cancel) => slot,
        }
    }

    /// Waits [`Backoff::after`] the `sends`-th send; false when the run stopped first.
    async fn wait(&self, sends: u32) -> bool {
        if self.cancel.is_cancelled() {
            return false;
        }
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => false,
            () = tokio::time::sleep(self.backoff.after(sends)) => true,
        }
    }

    /// Writes the job record in `state`.
    fn save(
        &self,
        fields: &JobRecord,
        state: JobState,
        handle: Option<&Value>,
    ) -> Result<(), StoreError> {
        self.jobs.save_job(&JobRecord {
            state,
            handle: handle.cloned(),
            ..fields.clone()
        })
    }

    /// Ends a long-job attempt whose submit provably never got there: the record goes, the hold
    /// is settled at $0, and the call ends with `outcome` (or the store's error).
    fn unsent(&self, fields: &JobRecord, hold: Hold, outcome: Outcome) -> Outcome {
        let removed = self.jobs.remove_job(&fields.key);
        self.settle(hold, Some(Usd::ZERO));
        match removed {
            Ok(()) => outcome,
            Err(error) => Outcome::Store(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_half_a_second_to_eight() {
        let backoff = Backoff::default();
        let waits: Vec<Duration> = (1..=6).map(|n| backoff.after(n)).collect();
        assert_eq!(
            waits,
            [500, 1000, 2000, 4000, 8000, 8000].map(Duration::from_millis)
        );
    }

    #[test]
    fn backoff_saturates_and_takes_zero_as_the_first_send() {
        let backoff = Backoff::default();
        assert_eq!(backoff.after(0), Duration::from_millis(500));
        assert_eq!(backoff.after(64), Duration::from_secs(8));
        assert_eq!(backoff.after(u32::MAX), Duration::from_secs(8));
        let custom = Backoff {
            initial: Duration::from_millis(100),
            factor: 3,
            max: Duration::from_secs(1),
        };
        assert_eq!(custom.after(2), Duration::from_millis(300));
        assert_eq!(custom.after(3), Duration::from_millis(900));
        assert_eq!(custom.after(4), Duration::from_secs(1));
        let flat = Backoff {
            initial: Duration::ZERO,
            ..Backoff::default()
        };
        assert_eq!(flat.after(5), Duration::ZERO);
    }
}
