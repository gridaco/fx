//! The retry owner: the engine is the only one (docs/wg/overview.md "Retry and billing
//! (ratified)"; spec/protocol.md §6.1 step 7; spec/store.md §5). Adapters make one send per call
//! and report what happened (`grida_fx_providers::Sent`, `Submitted`, `Collected`); this module
//! decides what follows, and reserves, records and settles every attempt.
//!
//! **Preparation.** On a cache miss, any adapter credential preparation completes cancellably
//! before admission, a hold, or a `submitting` record. Existing jobs are prepared before polling.
//!
//! **Sends.** At most [`MAX_SENDS`] sends per call: every send of the request counts, whether it
//! starts a new attempt or resends under the current one. Before each send the route's slot and
//! pacing are taken ([`super::pacing::Admission::admit`]; `Pacing` in a run), and only then is a
//! new attempt's hold reserved: a request waiting for its slot holds none of the ceiling, and a
//! refused reservation gives its slot back. Temporarily occupied budget also gives its slot
//! back before waiting cancellably for a settlement. Unpriced attempts reserve the remaining
//! applicable authorization instead of a route price. Between sends the owner waits [`Backoff`] (0.5 s,
//! doubling, at most 8 s). After a `NotReceived` whose provider asked for a wait (`retry_after`,
//! a `Retry-After` header), it waits the longer of the backoff and that wait, the wait capped at
//! [`MAX_RETRY_AFTER`] (60 s) (spec/adapters.md §3.3).
//!
//! **Checking an answer.** An answer is accepted only when, in this order, the adapter's `check`
//! passes (for an adapter FX ships or a project adapter, followed by the capability's every-route
//! check: [`super::checked`]), its `data` survives being written as the call record writes it and
//! read back (its canonical form, spec/identity.md §2, which is what the record replays), and the
//! engine's own check for the capability ([`Attempts::check`]; the shape of an `agent.turn` reply)
//! passes on that canonical form. The accepted answer carries the canonical `data`, so a live
//! answer and its replay are the same value. Any refusal fails the attempt as billed, before
//! anything is recorded.
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
//!   cost of the answer or collection outcome, else in full);
//! - cancelled while the submit is in flight: the record stays `submitting`, settle in full,
//!   `Cancelled`; cancelled while collecting: the record stays `submitted`, settle in full.
//!
//! **Collecting** ([`collect_job`], a retained task found before the call): no hold, no submit,
//! nothing billed. An answer or proof that the provider finished leaves the job `recoverable`
//! until the caller publishes the answer and records the call. A known generation failure
//! becomes `settled` without a handle. Conclusive loss during recovery of a successful or
//! legacy task leaves it `unavailable`. An inconclusive collection or cancellation leaves
//! the previous state in place.
//!
//! **Reasons.** Every reason an outcome carries (`Refused`, `Failed`, `Unsettled`) passes through
//! the adapter's redactor last (`RequestAdapter::redactor`, `LongJob::redactor`; spec/adapters.md
//! §7): a live invocation's adapters redact every key of the invocation. That covers the adapter's
//! own sentences, its check's, the engine's refusals before a send ([`super::checked`]), and the
//! engine's checks of the answer, which may quote what the provider sent back.
//!
//! Every hold emits `budget_reserved` and `budget_settled` through the ledger. The owner also
//! reports failed sends and collections to the call path's event log before trying again.
//! The call path writes the successful call record after `Answered`.
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
//! - A retained job keeps its original booked charge when known.
//! - A long-job call given no job record fields ([`Attempts::job`] `None`) is an engine fault:
//!   `Store`, before anything is reserved or sent.

use super::pacing::{Admission, Slot};
use crate::engine::Cancel;
use crate::events::{CallAttemptFailure, CallAttemptOutcome};
use crate::ledger::{Hold, Ledger, NotReserved, Refusal, Scopes};
use crate::store::records::{JobRecord, JobState};
use crate::store::{Store, StoreError};
use grida_fx_core::money::Usd;
use grida_fx_protocol::{AdapterDiagnostic, DiagnosticPhase, DiagnosticSource};
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
    /// [`HoldBook::reserve`] for the call whose key is `call`; a book that records holds names
    /// the key with the hold.
    fn reserve_call(
        &self,
        node_id: String,
        amount: Usd,
        scopes: &Scopes,
        call: &str,
    ) -> Result<Hold, NotReserved> {
        let _ = call;
        self.reserve(node_id, amount, scopes)
    }
    /// Opens the remaining authorization for an unpriced attempt.
    fn reserve_unknown_call(
        &self,
        node_id: String,
        scopes: &Scopes,
        call: &str,
    ) -> Result<Hold, NotReserved> {
        let _ = (node_id, scopes, call);
        Err(NotReserved::Unrecorded(
            "this budget book cannot authorize unpriced work".into(),
        ))
    }
    /// Budget changes, subscribed before attempting a reservation to avoid missed wakeups.
    fn changes(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        None
    }
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

    fn reserve_call(
        &self,
        node_id: String,
        amount: Usd,
        scopes: &Scopes,
        call: &str,
    ) -> Result<Hold, NotReserved> {
        Ledger::reserve_recorded_for(self, node_id, amount, scopes, Some(call.to_string()))
    }

    fn reserve_unknown_call(
        &self,
        node_id: String,
        scopes: &Scopes,
        call: &str,
    ) -> Result<Hold, NotReserved> {
        self.reserve_unknown_recorded_for(node_id, scopes, Some(call.to_string()))
    }

    fn changes(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        Some(Ledger::changes(self))
    }

    fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd {
        Ledger::settle(self, hold, reported)
    }

    fn unrecorded(&self) -> Option<String> {
        Ledger::fault(self)
    }
}

/// What the call path records about a send's wait for its route's slot (spec/store.md §8,
/// "Waiting for a route"): told only when a send cannot take its slot at once.
pub trait Waits: Send + Sync {
    /// The send waits for its slot (`call_waiting`).
    fn waiting(&self);
    /// The waiting send took its slot (`call_admitted`). Not told for a wait the run's
    /// cancellation ended.
    fn admitted(&self);
}

/// Persists failed exchanges while the retry owner still knows which attempt made them.
pub trait FailureLog: Send + Sync {
    fn failed(&self, failure: CallAttemptFailure) -> Result<(), String>;
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
    /// No numeric reservation is supplied by the route; use remaining authorization.
    pub unknown: bool,
    pub scopes: &'a Scopes,
    /// Names each new hold (`ledger::hold_id`).
    pub hold_name: &'a (dyn Fn() -> String + Sync),
    pub limit: Option<u32>,
    pub rpm: Option<u32>,
    pub book: &'a dyn HoldBook,
    pub jobs: &'a dyn JobBook,
    pub pacing: &'a dyn Admission,
    /// Told about each send's wait for its slot; `None` records nothing.
    pub waits: Option<&'a dyn Waits>,
    pub failures: Option<&'a dyn FailureLog>,
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
    /// Supplementary diagnostic evidence; execution semantics are those of the inner outcome.
    Diagnosed(Box<Outcome>, Box<grida_fx_protocol::AdapterDiagnostic>),
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
    /// The run's log could not record a reservation, settlement or failure: the run stops.
    Unrecorded(String),
}

struct FailureContext<'a> {
    attempts: &'a Attempts<'a>,
    call: &'a CallRequest,
    redactor: &'a Redactor,
    diagnostic: Option<&'a AdapterDiagnostic>,
    send: Option<u32>,
}

impl FailureContext<'_> {
    fn record(
        &self,
        outcome: CallAttemptOutcome,
        reason: &str,
        charged_usd: Option<Usd>,
        retryable: bool,
    ) -> Result<(), String> {
        let Some(log) = self.attempts.failures else {
            return Ok(());
        };
        if let Some(reason) = self.attempts.book.unrecorded() {
            return Err(reason);
        }
        let diagnostic = self
            .diagnostic
            .cloned()
            .map(|value| grida_fx_providers::diagnostic::sanitize(value, self.call, self.redactor));
        log.failed(CallAttemptFailure {
            attempt: self.call.attempt,
            send: self.send,
            outcome,
            reason: self.summarize(reason),
            diagnostic,
            charged_usd,
            retryable,
        })
    }

    fn summarize(&self, reason: &str) -> String {
        // Apply the message disclosure checks to an unstructured reason copied into evidence.
        let mut summary = AdapterDiagnostic::new(DiagnosticSource::Engine, DiagnosticPhase::Send);
        summary.message = Some(reason.to_string());
        let summary = grida_fx_providers::diagnostic::sanitize(summary, self.call, self.redactor);
        summary.message.expect("sanitized reason")
    }
}

/// Makes a plain call (module doc).
pub async fn send_plain(adapter: &dyn RequestAdapter, attempts: &Attempts<'_>) -> Outcome {
    let mut diagnostic = None;
    let outcome = plain(adapter, attempts, &mut diagnostic).await;
    reported(outcome, diagnostic, attempts, &adapter.redactor())
}

async fn plain(
    adapter: &dyn RequestAdapter,
    attempts: &Attempts<'_>,
    diagnostic: &mut Option<grida_fx_protocol::AdapterDiagnostic>,
) -> Outcome {
    if let Some(outcome) =
        preparation_failure(adapter.prepare(&attempts.call), attempts.cancel).await
    {
        return outcome;
    }
    let route_id = attempts.call.route.id();
    let mut call = attempts.call.clone();
    let mut sends = 0;
    let mut attempt = 0;
    let mut resend: Option<Hold> = None;
    loop {
        let was_resend = resend.is_some();
        let (slot, _admission, hold) = match attempts.start(&route_id, resend.take()).await {
            Ok(started) => started,
            Err(outcome) => return *outcome,
        };
        if !was_resend {
            attempt += 1;
            call.attempt = attempt;
        }
        sends += 1;
        let sent = race(adapter.send(&call), attempts.cancel)
            .await
            .map(|outcome| {
                let (outcome, detail) = outcome.into_report();
                *diagnostic = detail;
                outcome
            });
        drop(slot);
        let redactor = adapter.redactor();
        let failure = FailureContext {
            attempts,
            call: &call,
            redactor: &redactor,
            diagnostic: diagnostic.as_ref(),
            send: Some(sends),
        };
        let (reason, charged, retryable) = match sent {
            None => {
                attempts.settle(hold, None);
                return Outcome::Cancelled;
            }
            Some(Sent::Diagnosed(..)) => unreachable!("diagnostic separated above"),
            Some(Sent::NotReceived {
                reason,
                retry_after,
            }) => {
                if sends >= MAX_SENDS {
                    let charged = attempts.settle(hold, Some(Usd::ZERO));
                    if let Err(error) = failure.record(
                        CallAttemptOutcome::NotReceived,
                        &reason,
                        Some(charged),
                        false,
                    ) {
                        return Outcome::Unrecorded(error);
                    }
                    return Outcome::Failed(reason);
                }
                if let Err(error) =
                    failure.record(CallAttemptOutcome::NotReceived, &reason, None, true)
                {
                    attempts.settle(hold, Some(Usd::ZERO));
                    return Outcome::Unrecorded(error);
                }
                if !attempts.wait(sends, retry_after).await {
                    attempts.settle(hold, Some(Usd::ZERO));
                    return Outcome::Cancelled;
                }
                resend = Some(hold);
                continue;
            }
            Some(Sent::Refused { reason }) => {
                let charged = attempts.settle(hold, Some(Usd::ZERO));
                if let Err(error) =
                    failure.record(CallAttemptOutcome::Refused, &reason, Some(charged), false)
                {
                    return Outcome::Unrecorded(error);
                }
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
                    Err(reason) => (reason, attempts.settle(hold, cost), true),
                }
            }
            Some(Sent::Failed {
                reason,
                cost,
                retryable,
            }) => (reason, attempts.settle(hold, cost), retryable),
        };
        if let Err(error) = failure.record(
            CallAttemptOutcome::Failed,
            &reason,
            Some(charged),
            retryable && sends < MAX_SENDS,
        ) {
            return Outcome::Unrecorded(error);
        }
        if !retryable {
            return Outcome::Failed(reason);
        }
        // The attempt failed as billed: a new attempt, when sends are left.
        if let Some(outcome) = attempts.next(sends, reason).await {
            return outcome;
        }
    }
}

/// Submits a long job and collects it (module doc).
pub async fn submit_job(adapter: &dyn LongJob, attempts: &Attempts<'_>) -> Outcome {
    let mut diagnostic = None;
    let outcome = submit(adapter, attempts, &mut diagnostic).await;
    reported(outcome, diagnostic, attempts, &adapter.redactor())
}

async fn submit(
    adapter: &dyn LongJob,
    attempts: &Attempts<'_>,
    diagnostic: &mut Option<grida_fx_protocol::AdapterDiagnostic>,
) -> Outcome {
    let Some(fields) = attempts.job.as_ref() else {
        return no_job_fields(&attempts.call);
    };
    if let Some(outcome) =
        preparation_failure(adapter.prepare(&attempts.call), attempts.cancel).await
    {
        return outcome;
    }
    let route_id = attempts.call.route.id();
    let mut call = attempts.call.clone();
    let mut sends = 0;
    let mut attempt = 0;
    let mut resend: Option<Hold> = None;
    loop {
        let was_resend = resend.is_some();
        let (slot, _admission, hold) = match attempts.start(&route_id, resend.take()).await {
            Ok(started) => started,
            Err(outcome) => {
                if matches!(*outcome, Outcome::Cancelled) && was_resend {
                    // The earlier submit provably was not received.
                    if let Err(error) = attempts.jobs.remove_job(&fields.key) {
                        return Outcome::Store(error);
                    }
                }
                return *outcome;
            }
        };
        if !was_resend {
            attempt += 1;
            call.attempt = attempt;
        }
        // spec/store.md §5: `submitting` is written before the submit may leave.
        if let Err(error) = attempts.save(fields, JobState::Submitting, None) {
            drop(slot);
            attempts.settle(hold, Some(Usd::ZERO));
            return Outcome::Store(error);
        }
        sends += 1;
        let submitted = race(adapter.submit(&call), attempts.cancel)
            .await
            .map(|outcome| {
                let (outcome, detail) = outcome.into_report();
                *diagnostic = detail.map(|d| {
                    grida_fx_providers::diagnostic::sanitize(d, &call, &adapter.redactor())
                });
                outcome
            });
        drop(slot);
        let redactor = adapter.redactor();
        let failure = FailureContext {
            attempts,
            call: &call,
            redactor: &redactor,
            diagnostic: diagnostic.as_ref(),
            send: Some(sends),
        };
        let (reason, charged, retryable) = match submitted {
            None => {
                // It may have left: the record stays `submitting`.
                attempts.settle(hold, None);
                return Outcome::Cancelled;
            }
            Some(Submitted::Diagnosed(..)) => unreachable!("diagnostic separated above"),
            Some(Submitted::NotReceived {
                reason,
                retry_after,
            }) => {
                if sends >= MAX_SENDS {
                    let outcome = attempts.unsent(fields, hold, Outcome::Failed(reason.clone()));
                    if let Err(error) = failure.record(
                        CallAttemptOutcome::NotReceived,
                        &reason,
                        Some(Usd::ZERO),
                        false,
                    ) {
                        return Outcome::Unrecorded(error);
                    }
                    return outcome;
                }
                if let Err(error) =
                    failure.record(CallAttemptOutcome::NotReceived, &reason, None, true)
                {
                    return attempts.unsent(fields, hold, Outcome::Unrecorded(error));
                }
                if !attempts.wait(sends, retry_after).await {
                    return attempts.unsent(fields, hold, Outcome::Cancelled);
                }
                resend = Some(hold);
                continue;
            }
            Some(Submitted::Refused { reason }) => {
                let outcome = attempts.unsent(fields, hold, Outcome::Refused(reason.clone()));
                if let Err(error) =
                    failure.record(CallAttemptOutcome::Refused, &reason, Some(Usd::ZERO), false)
                {
                    return Outcome::Unrecorded(error);
                }
                return outcome;
            }
            Some(Submitted::Uncertain { reason }) => {
                // The note is written redacted, before the hold is settled (spec/store.md §5).
                let summary = failure.summarize(&reason);
                let saved = attempts.note(fields, &summary, diagnostic.as_ref());
                let charged = attempts.settle(hold, None);
                if let Err(error) = saved {
                    return Outcome::Store(error);
                }
                if let Err(error) =
                    failure.record(CallAttemptOutcome::Unsettled, &reason, Some(charged), false)
                {
                    return Outcome::Unrecorded(error);
                }
                return Outcome::Unsettled(summary);
            }
            Some(Submitted::Failed {
                reason,
                cost,
                retryable,
            }) => {
                let saved = attempts.save(fields, JobState::Settled, None);
                let charged = attempts.settle(hold, cost);
                if let Err(error) = saved {
                    return Outcome::Store(error);
                }
                (reason, charged, retryable)
            }
            Some(Submitted::Accepted { handle }) => {
                if let Err(error) = attempts.save(fields, JobState::Submitted, Some(&handle)) {
                    attempts.settle(hold, None);
                    return Outcome::Store(error);
                }
                return collect_held(
                    adapter,
                    attempts,
                    fields,
                    &call,
                    &handle,
                    (hold, sends),
                    diagnostic,
                )
                .await;
            }
        };
        if let Err(error) = failure.record(
            CallAttemptOutcome::Failed,
            &reason,
            Some(charged),
            retryable && sends < MAX_SENDS,
        ) {
            return Outcome::Unrecorded(error);
        }
        if !retryable {
            return Outcome::Failed(reason);
        }
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
    let mut diagnostic = None;
    let outcome = collect(adapter, attempts, handle, &mut diagnostic).await;
    reported(outcome, diagnostic, attempts, &adapter.redactor())
}

async fn collect(
    adapter: &dyn LongJob,
    attempts: &Attempts<'_>,
    handle: &Value,
    diagnostic: &mut Option<grida_fx_protocol::AdapterDiagnostic>,
) -> Outcome {
    let Some(fields) = attempts.job.as_ref() else {
        return no_job_fields(&attempts.call);
    };
    if let Some(outcome) =
        preparation_failure(adapter.prepare_collect(&attempts.call), attempts.cancel).await
    {
        return match outcome {
            // The provider may still be running this job. Keep its submitted record.
            Outcome::Refused(reason) => Outcome::Failed(reason),
            other => other,
        };
    }
    let mut call = attempts.call.clone();
    call.attempt = call.attempt.max(1);
    let collected = collect_once(adapter, attempts, &call, handle, diagnostic).await;
    let failure = FailureContext {
        attempts,
        call: &call,
        redactor: &adapter.redactor(),
        diagnostic: diagnostic.as_ref(),
        send: None,
    };
    finish_collection(fields, handle, collected, &failure, None)
}

/// Collects the job just accepted under its attempt's `hold`, and settles the hold.
async fn collect_held(
    adapter: &dyn LongJob,
    attempts: &Attempts<'_>,
    fields: &JobRecord,
    call: &CallRequest,
    handle: &Value,
    held: (Hold, u32),
    diagnostic: &mut Option<grida_fx_protocol::AdapterDiagnostic>,
) -> Outcome {
    let (hold, send) = held;
    let collected = collect_once(adapter, attempts, call, handle, diagnostic).await;
    let failure = FailureContext {
        attempts,
        call,
        redactor: &adapter.redactor(),
        diagnostic: diagnostic.as_ref(),
        send: Some(send),
    };
    finish_collection(fields, handle, collected, &failure, Some(hold))
}

/// Record the submission's charge once and preserve its result until the call is published.
fn finish_collection(
    fields: &JobRecord,
    handle: &Value,
    collected: Collect,
    failure: &FailureContext<'_>,
    hold: Option<Hold>,
) -> Outcome {
    let cost = match &collected {
        Collect::Answered(answer) => answer.cost,
        Collect::Incomplete { cost, .. }
        | Collect::Ended { cost, .. }
        | Collect::Undetermined { cost, .. }
        | Collect::Unavailable { cost, .. } => *cost,
        Collect::Cancelled | Collect::Unreachable(_) => None,
    };
    let charged = hold.map(|hold| failure.attempts.settle(hold, cost));
    let state = match &collected {
        Collect::Answered(_) | Collect::Incomplete { .. } => JobState::Recoverable,
        Collect::Unavailable { .. } => JobState::Unavailable,
        Collect::Undetermined { .. } if fields.state == JobState::Recoverable => {
            JobState::Unavailable
        }
        Collect::Undetermined { .. } => JobState::Settled,
        Collect::Ended { .. }
            if charged.is_none()
                && matches!(fields.state, JobState::Recoverable | JobState::Settled) =>
        {
            JobState::Unavailable
        }
        Collect::Ended { .. } => JobState::Settled,
        Collect::Cancelled | Collect::Unreachable(_) if charged.is_some() => JobState::Submitted,
        Collect::Cancelled | Collect::Unreachable(_) => fields.state,
    };
    if charged.is_some() || !matches!(collected, Collect::Cancelled | Collect::Unreachable(_)) {
        let keep_handle =
            state != JobState::Settled || matches!(collected, Collect::Undetermined { .. });
        let note = match &collected {
            Collect::Incomplete { reason, .. }
            | Collect::Unavailable { reason, .. }
            | Collect::Undetermined { reason, .. }
            | Collect::Ended { reason, .. }
                if matches!(state, JobState::Recoverable | JobState::Unavailable) =>
            {
                Some(failure.summarize(reason))
            }
            _ => None,
        };
        if let Err(error) = failure.attempts.jobs.save_job(&JobRecord {
            state,
            handle: keep_handle.then(|| handle.clone()),
            charged_usd: if keep_handle {
                charged.or(fields.charged_usd)
            } else {
                None
            },
            note,
            diagnostic: None,
            ..fields.clone()
        }) {
            return Outcome::Store(error);
        }
    }
    let (outcome, reason) = match collected {
        Collect::Cancelled => return Outcome::Cancelled,
        Collect::Answered(answer) => {
            return Outcome::Answered {
                answer,
                charged: charged.unwrap_or(Usd::ZERO),
                attempts: if charged.is_some() {
                    failure.call.attempt
                } else {
                    0
                },
            };
        }
        Collect::Incomplete { reason, .. }
        | Collect::Ended { reason, .. }
        | Collect::Undetermined { reason, .. }
        | Collect::Unavailable { reason, .. } => (CallAttemptOutcome::Failed, reason),
        Collect::Unreachable(reason) => (CallAttemptOutcome::Unreachable, reason),
    };
    match failure.record(outcome, &reason, charged, false) {
        Ok(()) => Outcome::Failed(reason),
        Err(error) => Outcome::Unrecorded(error),
    }
}

/// What one collect came to, its answer checked.
enum Collect {
    Answered(Answer),
    /// The provider finished and its result could not be collected or accepted.
    Incomplete {
        reason: String,
        cost: Option<Usd>,
    },
    /// The job ended without a result; `cost` is what the adapter reported for it.
    Ended {
        reason: String,
        cost: Option<Usd>,
    },
    Undetermined {
        reason: String,
        cost: Option<Usd>,
    },
    Unavailable {
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
    diagnostic: &mut Option<grida_fx_protocol::AdapterDiagnostic>,
) -> Collect {
    let Some((slot, _admission)) = attempts.admit(&call.route.id()).await else {
        return Collect::Cancelled;
    };
    let collected = race(adapter.collect(call, handle), attempts.cancel)
        .await
        .map(|outcome| {
            let (outcome, detail) = outcome.into_report();
            *diagnostic = detail;
            outcome
        });
    drop(slot);
    match collected {
        None => Collect::Cancelled,
        Some(Collected::Diagnosed(..)) => unreachable!("diagnostic separated above"),
        Some(Collected::Answered(answer)) => {
            let cost = answer.cost;
            match attempts.accept(call, answer, |call, answer| adapter.check(call, answer)) {
                Ok(answer) => Collect::Answered(answer),
                Err(reason) => Collect::Incomplete { reason, cost },
            }
        }
        Some(Collected::Ended { reason, cost }) => Collect::Ended { reason, cost },
        Some(Collected::Undetermined { reason, cost }) => Collect::Undetermined { reason, cost },
        Some(Collected::Incomplete { reason, cost }) => Collect::Incomplete { reason, cost },
        Some(Collected::Unavailable { reason, cost }) => Collect::Unavailable { reason, cost },
        Some(Collected::Unreachable { reason }) => Collect::Unreachable(reason),
    }
}

/// Preparation does no provider I/O, so cancellation wins without entering an attempt.
async fn preparation_failure(
    preparation: Option<BoxFuture<'_, Result<(), String>>>,
    cancel: &Cancel,
) -> Option<Outcome> {
    let preparation = preparation?;
    tokio::select! {
        biased;
        () = cancel.cancelled() => Some(Outcome::Cancelled),
        result = preparation => result.err().map(Outcome::Refused),
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

/// Keep evidence only on a failure, after sanitizing it independently of its outcome.
fn reported(
    outcome: Outcome,
    diagnostic: Option<grida_fx_protocol::AdapterDiagnostic>,
    attempts: &Attempts<'_>,
    redactor: &Redactor,
) -> Outcome {
    let outcome = redacted(outcome, redactor);
    match (outcome, diagnostic) {
        (
            outcome @ (Outcome::Refused(_) | Outcome::Failed(_) | Outcome::Unsettled(_)),
            Some(diagnostic),
        ) => Outcome::Diagnosed(
            Box::new(outcome),
            Box::new(grida_fx_providers::diagnostic::sanitize(
                diagnostic,
                &attempts.call,
                redactor,
            )),
        ),
        (outcome, _) => outcome,
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
        NotReserved::Waiting => Outcome::Unrecorded("budget waiting was not handled".into()),
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
        if self.unknown {
            self.book
                .reserve_unknown_call((self.hold_name)(), self.scopes, &self.call.key)
        } else {
            self.book
                .reserve_call((self.hold_name)(), self.hold, self.scopes, &self.call.key)
        }
    }

    /// Never keep a provider slot or an invocation admission while waiting for another hold.
    async fn start(
        &self,
        route_id: &str,
        mut resend: Option<Hold>,
    ) -> Result<(Slot, Option<crate::run_control::Admission>, Hold), Box<Outcome>> {
        let mut changes = self.book.changes();
        loop {
            let Some((slot, admission)) = self.admit(route_id).await else {
                if let Some(hold) = resend.take() {
                    self.settle(hold, Some(Usd::ZERO));
                }
                return Err(Box::new(Outcome::Cancelled));
            };
            let reserved = resend.take().map_or_else(|| self.reserve(), Ok);
            match reserved {
                Ok(hold) => return Ok((slot, admission, hold)),
                Err(NotReserved::Waiting) => {
                    drop(admission);
                    drop(slot);
                    let Some(changes) = changes.as_mut() else {
                        return Err(Box::new(Outcome::Unrecorded(
                            "budget book cannot wait for reservations".into(),
                        )));
                    };
                    tokio::select! {
                        biased;
                        () = self.cancel.cancelled() => return Err(Box::new(Outcome::Cancelled)),
                        changed = changes.changed() => if changed.is_err() {
                            return Err(Box::new(Outcome::Unrecorded("budget notifications ended".into())));
                        },
                    }
                }
                Err(not) => return Err(Box::new(not_reserved(not))),
            }
        }
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
        let waited = std::sync::atomic::AtomicBool::new(false);
        let waiting = || {
            waited.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(waits) = self.waits {
                waits.waiting();
            }
        };
        let slot = tokio::select! {
            biased;
            () = self.cancel.cancelled() => None,
            slot = self.pacing.admit(route_id, self.limit, self.rpm, self.cancel, &waiting) => slot,
        }?;
        if waited.load(std::sync::atomic::Ordering::Relaxed)
            && let Some(waits) = self.waits
        {
            waits.admitted();
        }
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
            charged_usd: None,
            note: None,
            diagnostic: None,
            ..fields.clone()
        })
    }

    /// Keeps the job record `submitting` with `note`, the redacted reason its submit's outcome is
    /// unknown (spec/store.md §5).
    fn note(
        &self,
        fields: &JobRecord,
        note: &str,
        diagnostic: Option<&grida_fx_protocol::AdapterDiagnostic>,
    ) -> Result<(), StoreError> {
        self.jobs.save_job(&JobRecord {
            state: JobState::Submitting,
            handle: None,
            charged_usd: None,
            note: Some(note.to_string()),
            diagnostic: diagnostic.cloned(),
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
            _waiting: &'a (dyn Fn() + Sync),
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
                limits: Default::default(),
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
            fresh_id: None,
            state: JobState::Submitting,
            handle: None,
            charged_usd: None,
            note: None,
            diagnostic: None,
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
                unknown: false,
                scopes: &self.scopes,
                hold_name: &*self.hold_name,
                limit: None,
                rpm: None,
                book: &Book,
                jobs: &Book,
                pacing: &Book,
                waits: None,
                failures: None,
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

    #[test]
    fn cancellation_during_preparation_leaves_no_hold_or_job() {
        struct Preparing(tokio::sync::Notify);

        impl Preparing {
            fn waiting(&self) -> Option<BoxFuture<'_, Result<(), String>>> {
                Some(Box::pin(async {
                    self.0.notify_one();
                    std::future::pending().await
                }))
            }
        }

        impl RequestAdapter for Preparing {
            fn prepare<'a>(
                &'a self,
                _: &'a CallRequest,
            ) -> Option<BoxFuture<'a, Result<(), String>>> {
                self.waiting()
            }

            fn send<'a>(&'a self, _: &'a CallRequest) -> BoxFuture<'a, Sent> {
                panic!("send reached during preparation");
            }
        }

        impl LongJob for Preparing {
            fn prepare<'a>(
                &'a self,
                _: &'a CallRequest,
            ) -> Option<BoxFuture<'a, Result<(), String>>> {
                self.waiting()
            }

            fn submit<'a>(&'a self, _: &'a CallRequest) -> BoxFuture<'a, Submitted> {
                panic!("submit reached during preparation");
            }

            fn collect<'a>(&'a self, _: &'a CallRequest, _: &'a Value) -> BoxFuture<'a, Collected> {
                panic!("collect reached during preparation");
            }
        }

        struct NoAttempt;

        impl JobBook for NoAttempt {
            fn save_job(&self, _: &JobRecord) -> Result<(), StoreError> {
                panic!("job saved during preparation");
            }

            fn remove_job(&self, _: &str) -> Result<(), StoreError> {
                panic!("job removed during preparation");
            }
        }

        impl Admission for NoAttempt {
            fn admit<'a>(
                &'a self,
                _: &'a str,
                _: Option<u32>,
                _: Option<u32>,
                _: &'a Cancel,
                _: &'a (dyn Fn() + Sync),
            ) -> BoxFuture<'a, Option<Slot>> {
                panic!("admission reached during preparation");
            }
        }

        for submit in [false, true] {
            let rig = Rig::new();
            let adapter = Preparing(tokio::sync::Notify::new());
            let mut attempts = rig.attempts(submit.then(job), None);
            attempts.hold_name = &|| panic!("reservation reached during preparation");
            attempts.jobs = &NoAttempt;
            attempts.pacing = &NoAttempt;
            let outcome = block_on(async {
                let send = async {
                    if submit {
                        submit_job(&adapter, &attempts).await
                    } else {
                        send_plain(&adapter, &attempts).await
                    }
                };
                let cancel = async {
                    adapter.0.notified().await;
                    rig.cancel.cancel();
                };
                tokio::time::timeout(Duration::from_secs(2), async {
                    tokio::join!(send, cancel).0
                })
                .await
                .expect("credential preparation should respond to cancellation")
            });
            assert_eq!(outcome, Outcome::Cancelled);
        }
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
