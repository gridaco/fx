//! The retry owner: the engine is the only one (docs/wg/overview.md "Retry and billing
//! (ratified)"; spec/protocol.md §6.1 step 7; spec/store.md §5). Adapters make one send per call
//! and report what happened (`grida_fx_providers::Sent`, `Submitted`, `Collected`); this module
//! decides what follows, and reserves, records and settles every attempt.
//!
//! **Sends.** At most [`MAX_SENDS`] sends per call: every send of the request counts, whether it
//! starts a new attempt or resends under the current one. Before each send the route's slot and
//! pacing are taken ([`super::pacing::Admission::admit`]; `Pacing` in a run), and only then is a
//! new attempt's hold reserved: a request waiting for its slot holds none of the ceiling, and a
//! refused reservation gives its slot back. Between sends the owner waits [`Backoff`] (0.5 s,
//! doubling, at most 8 s). After a `NotReceived` whose provider asked for a wait (`retry_after`,
//! a `Retry-After` header), it waits the longer of the backoff and that wait, the wait capped at
//! [`MAX_RETRY_AFTER`] (60 s) (spec/providers.md §4.3).
//!
//! **Checking an answer.** An answer is accepted only when, in this order, the adapter's `check`
//! passes, its `data` survives being written as the call record writes it and read back (its
//! canonical form, spec/identity.md §2, which is what the record replays), and the engine's own
//! check for the capability ([`Attempts::check`]; the shape of an `agent.turn` reply) passes on
//! that canonical form. The accepted answer carries the canonical `data`, so a live answer and
//! its replay are the same value. Any refusal fails the attempt as billed, before anything is
//! recorded.
//!
//! **A plain call, attempt by attempt** (each attempt is its own hold, `ledger::hold_id`):
//!
//! | outcome of a send | hold | next |
//! |---|---|---|
//! | `NotReceived` | kept open | resend under the same attempt (sends left), else settle $0 → `Failed` |
//! | `Refused` | settled $0 | `Refused` (`capability_refused`), never retried |
//! | `Answered`, checks pass | settled at the reported cost (whole hold when none) | `Answered` |
//! | `Answered`, a check refuses | settled as billed (reported cost, else whole hold) | new attempt (sends left), else `Failed` |
//! | `Failed { retryable: true }` | settled as billed | new attempt (sends left), else `Failed` |
//! | `Failed { retryable: false }` | settled as billed | `Failed` |
//! | cancelled while a send is in flight | settled in full | `Cancelled` |
//! | cancelled while a resend waits for its slot | settled $0 | `Cancelled` |
//! | cancelled while a new attempt waits for its slot | none reserved | `Cancelled` |
//!
//! A reservation that does not fit ends the call with `Ceiling` (`ceiling_exceeded`); attempts
//! already made stay settled. A reservation the run's log cannot record ends it with `Unrecorded`
//! before anything is sent, and so does a billed attempt whose settlement the log could not record
//! (`HoldBook::unrecorded`): no new attempt is made once the log has failed.
//!
//! **A long job** ([`submit_job`]): take the slot; reserve; write the job record `submitting`
//! before the submit may leave; then
//! - `NotReceived`: resend the submit (sends left), else remove the record, settle $0, `Failed`;
//! - `Refused`: remove the record, settle $0, `Refused`;
//! - `Uncertain`: the record stays `submitting` and keeps the redacted reason as its `note`
//!   (naming the provider's job id when one was returned, so a person can find the job after the
//!   run; spec/store.md §5), settle in full, `Unsettled` (`job_unsettled`);
//! - `Failed`: the record becomes `settled`, settle as billed, new attempt when retryable;
//! - `Accepted { handle }`: the record becomes `submitted` with the handle, then the job is
//!   collected under the same hold ([`collect_job`] rules, but the hold is settled: the reported
//!   cost of the answer, or the cost the adapter reports for a job that `Ended`, else in full);
//! - cancelled while the submit is in flight: the record stays `submitting`, settle in full,
//!   `Cancelled`; cancelled while collecting: the record stays `submitted`, settle in full.
//!
//! **Collecting** ([`collect_job`], a `submitted` record found before the call): no hold, no new
//! submit, nothing billed (`charged` 0). `Answered` → the caller publishes the call record and then
//! removes the job record; a check that refuses the answer settles the record and fails the call;
//! `Ended` → the record becomes `settled`, `Failed`, whatever cost it reports; `Unreachable` → the
//! record stays `submitted`, `Failed`; cancelled → the record stays `submitted`, `Cancelled`.
//!
//! **Reasons.** Every reason an outcome carries (`Refused`, `Failed`, `Unsettled`) passes through
//! the adapter's redactor last (`RequestAdapter::redactor`, `LongJob::redactor`; spec/providers.md
//! §8): a live invocation's adapters redact every key of the invocation. That covers the adapter's
//! own sentences, its check's, and the engine's checks of the answer, which may quote what the
//! provider sent back.
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
use crate::ledger::{Hold, Ledger, NotReserved, Refusal, Scopes};
use crate::store::records::{JobRecord, JobState};
use crate::store::{Store, StoreError};
use grida_fx_core::money::Usd;
use grida_fx_providers::redact::Redactor;
use grida_fx_providers::{
    Answer, BoxFuture, CallRequest, Collected, LongJob, RequestAdapter, Sent, Submitted,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// The most sends of one call's request (module doc).
pub const MAX_SENDS: u32 = 6;

/// The longest wait a provider's `retry_after` adds before a resend (module doc).
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

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

    /// The wait after the `n`-th send when the provider asked for `retry_after`: the longer of
    /// [`Backoff::after`] and `retry_after` capped at [`MAX_RETRY_AFTER`] (module doc).
    pub fn pause(&self, n: u32, retry_after: Option<Duration>) -> Duration {
        let asked = retry_after.map_or(Duration::ZERO, |wait| wait.min(MAX_RETRY_AFTER));
        self.after(n).max(asked)
    }
}

/// Where holds go: the ledger in a run, a fake in tests.
pub trait HoldBook: Send + Sync {
    /// Opens a hold once it is recorded (`crate::ledger` module doc).
    fn reserve(&self, node_id: String, amount: Usd, scopes: &Scopes) -> Result<Hold, NotReserved>;
    /// Settles a hold: what was charged, whether or not the log could record it.
    fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd;
    /// Why the book can no longer record what is spent (a settlement or a reservation its log
    /// could not write), if it cannot.
    fn unrecorded(&self) -> Option<String> {
        None
    }
}

impl HoldBook for Ledger {
    fn reserve(&self, node_id: String, amount: Usd, scopes: &Scopes) -> Result<Hold, NotReserved> {
        Ledger::reserve_recorded(self, node_id, amount, scopes)
    }

    fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd {
        Ledger::settle(self, hold, reported)
    }

    fn unrecorded(&self) -> Option<String> {
        Ledger::fault(self)
    }
}

/// An engine-side check of an answer's canonical form ([`Attempts::check`]): a refusal is a
/// sentence, and the attempt is failed as billed.
pub type AnswerCheck<'a> = dyn Fn(&Answer) -> Result<(), String> + Sync + 'a;

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
    /// Invocation admission: every send/resend/collection shares cancellation acceptance.
    pub control: Option<&'a Arc<crate::run_control::RunControl>>,
    pub backoff: Backoff,
    /// The job record's key fields (`state` and `handle` are set by the owner); long jobs only.
    pub job: Option<JobRecord>,
    /// The engine's own check of an answer, after the adapter's (module doc, "Checking an
    /// answer"); `None` checks nothing more.
    pub check: Option<&'a AnswerCheck<'a>>,
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
    /// The run's log could not record a reservation or a settlement: the run stops (module doc).
    Unrecorded(String),
}

/// Makes a plain call (module doc).
pub async fn send_plain(adapter: &dyn RequestAdapter, attempts: &Attempts<'_>) -> Outcome {
    redacted(plain(adapter, attempts).await, &adapter.redactor())
}

async fn plain(adapter: &dyn RequestAdapter, attempts: &Attempts<'_>) -> Outcome {
    let route_id = attempts.call.route.id();
    let mut call = attempts.call.clone();
    let mut sends = 0;
    let mut attempt = 0;
    let mut resend: Option<Hold> = None;
    loop {
        // The slot first, then the hold (module doc, "Sends").
        let Some((slot, _admission)) = attempts.admit(&route_id).await else {
            if let Some(hold) = resend.take() {
                attempts.settle(hold, Some(Usd::ZERO));
            }
            return Outcome::Cancelled;
        };
        let hold = match resend.take() {
            Some(hold) => hold,
            None => match attempts.reserve() {
                Ok(hold) => {
                    attempt += 1;
                    call.attempt = attempt;
                    hold
                }
                Err(not) => return not_reserved(not),
            },
        };
        sends += 1;
        let sent = race(adapter.send(&call), attempts.cancel).await;
        drop(slot);
        let reason = match sent {
            None => {
                attempts.settle(hold, None);
                return Outcome::Cancelled;
            }
            Some(Sent::NotReceived {
                reason,
                retry_after,
            }) => {
                if sends >= MAX_SENDS {
                    attempts.settle(hold, Some(Usd::ZERO));
                    return Outcome::Failed(reason);
                }
                if !attempts.wait(sends, retry_after).await {
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
            Some(Sent::Answered(answer)) => {
                let cost = answer.cost;
                match attempts.accept(&call, answer, |call, answer| adapter.check(call, answer)) {
                    Ok(answer) => {
                        let charged = attempts.settle(hold, cost);
                        return Outcome::Answered {
                            answer,
                            charged,
                            attempts: attempt,
                        };
                    }
                    Err(reason) => {
                        attempts.settle(hold, cost);
                        reason
                    }
                }
            }
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
        if let Some(outcome) = attempts.next(sends, reason).await {
            return outcome;
        }
    }
}

/// Submits a long job and collects it (module doc).
pub async fn submit_job(adapter: &dyn LongJob, attempts: &Attempts<'_>) -> Outcome {
    redacted(submit(adapter, attempts).await, &adapter.redactor())
}

async fn submit(adapter: &dyn LongJob, attempts: &Attempts<'_>) -> Outcome {
    let Some(fields) = attempts.job.as_ref() else {
        return no_job_fields(&attempts.call);
    };
    let route_id = attempts.call.route.id();
    let mut call = attempts.call.clone();
    let mut sends = 0;
    let mut attempt = 0;
    let mut resend: Option<Hold> = None;
    loop {
        // The slot first, then the hold (module doc, "Sends").
        let Some((slot, _admission)) = attempts.admit(&route_id).await else {
            if let Some(hold) = resend.take() {
                // The `submitting` record of the submit that was not received.
                return attempts.unsent(fields, hold, Outcome::Cancelled);
            }
            return Outcome::Cancelled;
        };
        let hold = match resend.take() {
            Some(hold) => hold,
            None => match attempts.reserve() {
                Ok(hold) => {
                    attempt += 1;
                    call.attempt = attempt;
                    hold
                }
                Err(not) => return not_reserved(not),
            },
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
            Some(Submitted::NotReceived {
                reason,
                retry_after,
            }) => {
                if sends >= MAX_SENDS {
                    return attempts.unsent(fields, hold, Outcome::Failed(reason));
                }
                if !attempts.wait(sends, retry_after).await {
                    return attempts.unsent(fields, hold, Outcome::Cancelled);
                }
                resend = Some(hold);
                continue;
            }
            Some(Submitted::Refused { reason }) => {
                return attempts.unsent(fields, hold, Outcome::Refused(reason));
            }
            Some(Submitted::Uncertain { reason }) => {
                // The note is written redacted, before the hold is settled (spec/store.md §5).
                let reason = adapter.redactor().reason(&reason);
                let saved = attempts.note(fields, &reason);
                attempts.settle(hold, None);
                if let Err(error) = saved {
                    return Outcome::Store(error);
                }
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
        if let Some(outcome) = attempts.next(sends, reason).await {
            return outcome;
        }
    }
}

/// Collects a job an earlier run submitted (module doc).
pub async fn collect_job(
    adapter: &dyn LongJob,
    attempts: &Attempts<'_>,
    handle: &Value,
) -> Outcome {
    redacted(
        collect(adapter, attempts, handle).await,
        &adapter.redactor(),
    )
}

async fn collect(adapter: &dyn LongJob, attempts: &Attempts<'_>, handle: &Value) -> Outcome {
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
        Collect::Refused { reason, .. } | Collect::Ended { reason, .. } => {
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
        Collect::Ended { reason, cost } => {
            // The cost the adapter reports for the ended job, else the whole hold.
            let saved = attempts.save(fields, JobState::Settled, Some(handle));
            attempts.settle(hold, cost);
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
    /// The job ended without a result; `cost` is what the adapter reported for it.
    Ended {
        reason: String,
        cost: Option<Usd>,
    },
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
    let Some((slot, _admission)) = attempts.admit(&call.route.id()).await else {
        return Collect::Cancelled;
    };
    let collected = race(adapter.collect(call, handle), attempts.cancel).await;
    drop(slot);
    match collected {
        None => Collect::Cancelled,
        Some(Collected::Answered(answer)) => {
            let cost = answer.cost;
            match attempts.accept(call, answer, |call, answer| adapter.check(call, answer)) {
                Ok(answer) => Collect::Answered(answer),
                Err(reason) => Collect::Refused { reason, cost },
            }
        }
        Some(Collected::Ended { reason, cost }) => Collect::Ended { reason, cost },
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

/// `outcome` with its reason passed through `redactor` (module doc, "Reasons"): a sentence from
/// the adapter, its check, or the engine's checks of the answer, which may quote what the provider
/// answered.
fn redacted(outcome: Outcome, redactor: &Redactor) -> Outcome {
    match outcome {
        Outcome::Refused(reason) => Outcome::Refused(redactor.reason(&reason)),
        Outcome::Failed(reason) => Outcome::Failed(redactor.reason(&reason)),
        Outcome::Unsettled(reason) => Outcome::Unsettled(redactor.reason(&reason)),
        other => other,
    }
}

/// A reservation that opened no hold: `Ceiling` when refused, `Unrecorded` when the log failed.
fn not_reserved(not: NotReserved) -> Outcome {
    match not {
        NotReserved::Refused(refusal) => Outcome::Ceiling(refusal),
        NotReserved::Unrecorded(reason) => Outcome::Unrecorded(reason),
    }
}

/// `data` as the call record writes it and reads it back (module doc, "Checking an answer").
pub fn canonical_data(data: &Value) -> Result<Value, String> {
    grida_fx_core::value::parse_json(&grida_fx_core::value::canon(data))
        .map_err(|refused| format!("the answer's data cannot be recorded: {}", refused.message))
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
    fn reserve(&self) -> Result<Hold, NotReserved> {
        self.book
            .reserve((self.hold_name)(), self.hold, self.scopes)
    }

    /// The checks of an answer (module doc, "Checking an answer"): the answer with its canonical
    /// `data`, or why it is refused.
    fn accept(
        &self,
        call: &CallRequest,
        mut answer: Answer,
        adapter_check: impl Fn(&CallRequest, &Answer) -> Result<(), String>,
    ) -> Result<Answer, String> {
        adapter_check(call, &answer)?;
        answer.data = canonical_data(&answer.data)?;
        if let Some(check) = self.check {
            check(&answer)?;
        }
        Ok(answer)
    }

    /// After an attempt failed as billed: `None` to make a new one, or how the call ends (out of
    /// sends, a log that can no longer record, or the run stopped during the wait).
    async fn next(&self, sends: u32, reason: String) -> Option<Outcome> {
        if let Some(unrecorded) = self.book.unrecorded() {
            return Some(Outcome::Unrecorded(unrecorded));
        }
        if sends >= MAX_SENDS {
            return Some(Outcome::Failed(reason));
        }
        if !self.wait(sends, None).await {
            return Some(Outcome::Cancelled);
        }
        None
    }

    fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd {
        self.book.settle(hold, reported)
    }

    /// Takes the route's slot and pacing for one send; `None` when the run stopped first.
    async fn admit(&self, route_id: &str) -> Option<(Slot, Option<crate::run_control::Admission>)> {
        if self.cancel.is_cancelled() {
            return None;
        }
        let slot = tokio::select! {
            biased;
            () = self.cancel.cancelled() => None,
            slot = self.pacing.admit(route_id, self.limit, self.rpm, self.cancel) => slot,
        }?;
        let admission = match self.control {
            Some(control) => Some(control.admit()?),
            None => None,
        };
        Some((slot, admission))
    }

    /// Waits [`Backoff::pause`] after the `sends`-th send; false when the run stopped first.
    async fn wait(&self, sends: u32, retry_after: Option<Duration>) -> bool {
        if self.cancel.is_cancelled() {
            return false;
        }
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => false,
            () = tokio::time::sleep(self.backoff.pause(sends, retry_after)) => true,
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
            note: None,
            ..fields.clone()
        })
    }

    /// Keeps the job record `submitting` with `note`, the redacted reason its submit's outcome is
    /// unknown (spec/store.md §5).
    fn note(&self, fields: &JobRecord, note: &str) -> Result<(), StoreError> {
        self.jobs.save_job(&JobRecord {
            state: JobState::Submitting,
            handle: None,
            note: Some(note.to_string()),
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

    #[test]
    fn a_provider_wait_lengthens_the_backoff_up_to_a_minute() {
        let backoff = Backoff::default();
        assert_eq!(backoff.pause(1, None), Duration::from_millis(500));
        assert_eq!(
            backoff.pause(1, Some(Duration::from_millis(100))),
            Duration::from_millis(500),
            "a shorter wait keeps the backoff"
        );
        assert_eq!(
            backoff.pause(2, Some(Duration::from_secs(20))),
            Duration::from_secs(20)
        );
        assert_eq!(
            backoff.pause(5, Some(Duration::from_secs(3600))),
            MAX_RETRY_AFTER
        );
        assert_eq!(
            backoff.pause(5, Some(Duration::ZERO)),
            Duration::from_secs(8)
        );
    }

    // --- Reasons are redacted with the adapter's redactor (module doc, "Reasons") ------------

    use crate::store::records::RouteEntry;
    use grida_fx_providers::{KeyName, Keys, RouteRef};
    use serde_json::json;

    /// A fake provider key, shaped as a media-type subtype the way a real key can be.
    const KEY: &str = "test-key-not-real-el-0123456789abcdef";

    /// Answers what it is given, echoing the key: in a check's sentence (the content type it
    /// quotes) and in the sentence the engine's own check refuses with. Its redactor knows the
    /// key, as a live invocation's adapters do.
    struct Echo {
        sent: Sent,
        submitted: Submitted,
    }

    impl Echo {
        fn redactor_of_the_invocation() -> Redactor {
            Redactor::new(Keys::from_pairs(&[(KeyName::ElevenLabs, KEY)]).secrets())
        }
    }

    impl RequestAdapter for Echo {
        fn send<'a>(&'a self, _call: &'a CallRequest) -> BoxFuture<'a, Sent> {
            let sent = self.sent.clone();
            Box::pin(async move { sent })
        }

        fn check(&self, _call: &CallRequest, answer: &Answer) -> Result<(), String> {
            match answer.files.get("audio") {
                Some(audio) if audio.kind != "audio/mpeg" => {
                    Err(format!("requested mp3 but received {}", audio.kind))
                }
                _ => Ok(()),
            }
        }

        fn redactor(&self) -> Redactor {
            Echo::redactor_of_the_invocation()
        }
    }

    impl LongJob for Echo {
        fn submit<'a>(&'a self, _call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
            let submitted = self.submitted.clone();
            Box::pin(async move { submitted })
        }

        fn collect<'a>(
            &'a self,
            _call: &'a CallRequest,
            _handle: &'a Value,
        ) -> BoxFuture<'a, Collected> {
            Box::pin(async move {
                Collected::Ended {
                    reason: format!("the job failed: {KEY}"),
                    cost: None,
                }
            })
        }

        fn redactor(&self) -> Redactor {
            Echo::redactor_of_the_invocation()
        }
    }

    struct Book;

    impl HoldBook for Book {
        fn reserve(
            &self,
            node_id: String,
            amount: Usd,
            _scopes: &Scopes,
        ) -> Result<Hold, NotReserved> {
            Ok(Hold {
                node_id,
                amount,
                scopes: Vec::new(),
            })
        }

        fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd {
            reported.unwrap_or(hold.amount)
        }
    }

    impl JobBook for Book {
        fn save_job(&self, _record: &JobRecord) -> Result<(), StoreError> {
            Ok(())
        }

        fn remove_job(&self, _key: &str) -> Result<(), StoreError> {
            Ok(())
        }
    }

    impl Admission for Book {
        fn admit<'a>(
            &'a self,
            _route_id: &'a str,
            _limit: Option<u32>,
            _rpm: Option<u32>,
            _cancel: &'a Cancel,
        ) -> BoxFuture<'a, Option<Slot>> {
            Box::pin(async { Some(Slot::free()) })
        }
    }

    fn call() -> CallRequest {
        CallRequest {
            route: RouteRef {
                capability: "sound.generate".into(),
                model: "sfx-a".into(),
                provider: "acme".into(),
                contract: json!({}),
            },
            request: json!({"prompt": "a door"}),
            files: indexmap::IndexMap::new(),
            take: vec![1],
            key: "a".repeat(64),
            attempt: 1,
        }
    }

    fn job() -> JobRecord {
        JobRecord {
            key: "a".repeat(64),
            capability: "sound.generate".into(),
            route: RouteEntry {
                id: "sfx-a@acme".into(),
                fingerprint: "0".repeat(64),
            },
            request: json!({}),
            take: vec![1],
            state: JobState::Submitting,
            handle: None,
            note: None,
        }
    }

    /// What one call's attempts borrow.
    struct Rig {
        cancel: Cancel,
        scopes: Scopes,
        hold_name: Box<dyn Fn() -> String + Sync>,
    }

    impl Rig {
        fn new() -> Rig {
            let names = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            Rig {
                cancel: Cancel::new(),
                scopes: Vec::new(),
                hold_name: Box::new(move || {
                    let n = names.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    format!("call/inv.{n}")
                }),
            }
        }

        /// The attempts of one call; `check` is the engine's own check.
        fn attempts<'a>(
            &'a self,
            job: Option<JobRecord>,
            check: Option<&'a AnswerCheck<'a>>,
        ) -> Attempts<'a> {
            Attempts {
                control: None,
                call: call(),
                hold: Usd(40_000),
                scopes: &self.scopes,
                hold_name: &*self.hold_name,
                limit: None,
                rpm: None,
                book: &Book,
                jobs: &Book,
                pacing: &Book,
                cancel: &self.cancel,
                backoff: Backoff {
                    initial: Duration::ZERO,
                    ..Backoff::default()
                },
                job,
                check,
            }
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(future)
    }

    fn mp3_declared_as(kind: &str) -> Sent {
        Sent::Answered(Answer::new(Value::Null, None).with_file("audio", kind, b"ID3".to_vec()))
    }

    fn adapter(sent: Sent) -> Echo {
        Echo {
            sent,
            submitted: Submitted::Refused {
                reason: String::new(),
            },
        }
    }

    #[test]
    fn a_check_that_quotes_a_key_is_redacted() {
        let echo = adapter(mp3_declared_as(&format!("audio/{KEY}")));
        let rig = Rig::new();
        let outcome = block_on(send_plain(&echo, &rig.attempts(None, None)));
        assert_eq!(
            outcome,
            Outcome::Failed("requested mp3 but received audio/[redacted]".into())
        );
    }

    #[test]
    fn the_engine_check_and_the_adapter_reasons_are_redacted() {
        let echo = adapter(mp3_declared_as("audio/mpeg"));
        let engine: &AnswerCheck<'_> = &|_answer| Err(format!("the reply names {KEY}"));
        let rig = Rig::new();
        let outcome = block_on(send_plain(&echo, &rig.attempts(None, Some(engine))));
        assert_eq!(
            outcome,
            Outcome::Failed("the reply names [redacted]".into())
        );
        for (sent, expected) in [
            (
                Sent::Refused {
                    reason: format!("refused {KEY}"),
                },
                Outcome::Refused("refused [redacted]".into()),
            ),
            (
                Sent::Failed {
                    reason: format!("failed {KEY}"),
                    cost: None,
                    retryable: false,
                },
                Outcome::Failed("failed [redacted]".into()),
            ),
            (
                Sent::NotReceived {
                    reason: format!("not received {KEY}"),
                    retry_after: None,
                },
                Outcome::Failed("not received [redacted]".into()),
            ),
        ] {
            let echo = adapter(sent);
            let outcome = block_on(send_plain(&echo, &rig.attempts(None, None)));
            assert_eq!(outcome, expected);
        }
    }

    #[test]
    fn long_job_reasons_are_redacted() {
        let uncertain = Echo {
            sent: Sent::not_received(""),
            submitted: Submitted::Uncertain {
                reason: format!("may have taken it {KEY}"),
            },
        };
        let rig = Rig::new();
        let outcome = block_on(submit_job(&uncertain, &rig.attempts(Some(job()), None)));
        assert_eq!(
            outcome,
            Outcome::Unsettled("may have taken it [redacted]".into())
        );
        let accepted = Echo {
            sent: Sent::not_received(""),
            submitted: Submitted::Accepted { handle: json!({}) },
        };
        let outcome = block_on(submit_job(&accepted, &rig.attempts(Some(job()), None)));
        assert_eq!(
            outcome,
            Outcome::Failed("the job failed: [redacted]".into())
        );
        let outcome = block_on(collect_job(
            &accepted,
            &rig.attempts(Some(job()), None),
            &json!({}),
        ));
        assert_eq!(
            outcome,
            Outcome::Failed("the job failed: [redacted]".into())
        );
    }
}
