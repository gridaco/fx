//! The retry owner (`calls::retry`): every row of its module doc's tables, driven by the scripted
//! `FakeAdapter` through fake hold and job books and a fake admission, on paused time.

use grida_fx_core::money::Usd;
use grida_fx_providers::fake::{FakeAdapter, FakeCall, refuse_bad};
use grida_fx_providers::{
    Answer, BoxFuture, CallRequest, Collected, LongJob, RequestAdapter, RouteRef, Sent, Submitted,
};
use grida_fx_runtime::calls::pacing::{Admission, Slot};
use grida_fx_runtime::calls::retry::{
    Attempts, Backoff, HoldBook, JobBook, MAX_SENDS, Outcome, collect_job, send_plain, submit_job,
};
use grida_fx_runtime::engine::Cancel;
use grida_fx_runtime::ledger::{Hold, NotReserved, Refusal, Scopes};
use grida_fx_runtime::store::StoreError;
use grida_fx_runtime::store::records::{JobRecord, JobState, RouteEntry};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::time::{Duration, Instant};

const INSTANCE: &str = "gen";
const HOLD: Usd = Usd(40_000);
const COST: Usd = Usd(20_000);

fn key() -> String {
    "a".repeat(64)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------------
// Fakes

/// A hold book that records every reservation and settlement and refuses on demand.
#[derive(Default)]
struct Book {
    reserved: Mutex<Vec<(String, Usd, Vec<String>)>>,
    settled: Mutex<Vec<(String, Option<Usd>)>>,
    refused: Mutex<Vec<String>>,
    /// Refuse the n-th reservation asked for (1-based).
    refuse_at: Option<usize>,
    /// Fail to record the n-th reservation asked for (1-based).
    unrecorded_at: Option<usize>,
    /// Fail to record the n-th settlement (1-based); the book then says so.
    unsettled_at: Option<usize>,
    unrecorded: Mutex<Option<String>>,
}

impl HoldBook for Book {
    fn reserve(&self, node_id: String, amount: Usd, scopes: &Scopes) -> Result<Hold, NotReserved> {
        let asked = lock(&self.reserved).len() + lock(&self.refused).len() + 1;
        if Some(asked) == self.unrecorded_at {
            lock(&self.refused).push(node_id.clone());
            return Err(NotReserved::Unrecorded(format!(
                "the run's events.jsonl could not record budget_reserved of {node_id}: disk full"
            )));
        }
        if Some(asked) == self.refuse_at {
            lock(&self.refused).push(node_id.clone());
            return Err(NotReserved::Refused(Refusal {
                needed: amount,
                remaining: Usd(10_000),
                message: format!(
                    "run ceiling reached: {node_id} needs up to $0.0400 and $0.0100 is left"
                ),
            }));
        }
        let owners: Vec<String> = scopes.iter().map(|(owner, _)| owner.clone()).collect();
        lock(&self.reserved).push((node_id.clone(), amount, owners.clone()));
        Ok(Hold {
            node_id,
            amount,
            scopes: owners,
        })
    }

    fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd {
        let mut settled = lock(&self.settled);
        settled.push((hold.node_id.clone(), reported));
        if Some(settled.len()) == self.unsettled_at {
            lock(&self.unrecorded).get_or_insert_with(|| {
                format!(
                    "the run's events.jsonl could not record budget_settled of {}: disk full",
                    hold.node_id
                )
            });
        }
        reported.unwrap_or(hold.amount)
    }

    fn unrecorded(&self) -> Option<String> {
        lock(&self.unrecorded).clone()
    }
}

impl Book {
    /// The names of the holds reserved, in order.
    fn holds(&self) -> Vec<String> {
        lock(&self.reserved)
            .iter()
            .map(|(name, _, _)| name.clone())
            .collect()
    }

    /// `(name, reported)` of every settlement, in order.
    fn settlements(&self) -> Vec<(String, Option<Usd>)> {
        lock(&self.settled).clone()
    }

    /// Every hold reserved was settled exactly once.
    fn assert_all_settled(&self) {
        let mut reserved = self.holds();
        let mut settled: Vec<String> = self.settlements().into_iter().map(|(n, _)| n).collect();
        reserved.sort();
        settled.sort();
        assert_eq!(reserved, settled, "every hold is settled exactly once");
    }
}

/// What happened to the job record.
#[derive(Debug, Clone, PartialEq)]
enum JobOp {
    Saved(JobState, Option<Value>),
    /// Saved `submitting` with this note (spec/store.md §5).
    Noted(String),
    Removed,
}

/// An in-memory job book recording every saved state, failing on demand.
#[derive(Default)]
struct Jobs {
    ops: Mutex<Vec<JobOp>>,
    /// Fail the n-th operation (1-based).
    fail_at: Option<usize>,
}

impl Jobs {
    fn ops(&self) -> Vec<JobOp> {
        lock(&self.ops).clone()
    }

    fn fails(&self, what: &str) -> Result<(), StoreError> {
        let n = lock(&self.ops).len() + 1;
        if Some(n) == self.fail_at {
            return Err(StoreError::Io {
                what: what.to_string(),
                reason: "disk full".into(),
            });
        }
        Ok(())
    }
}

impl JobBook for Jobs {
    fn save_job(&self, record: &JobRecord) -> Result<(), StoreError> {
        assert_eq!(record.key, key());
        assert_eq!(record.capability, "video.generate");
        self.fails(&format!("jobs/{}.json", record.key))?;
        let op = match &record.note {
            Some(note) => {
                assert_eq!(
                    (record.state, &record.handle),
                    (JobState::Submitting, &None)
                );
                JobOp::Noted(note.clone())
            }
            None => JobOp::Saved(record.state, record.handle.clone()),
        };
        lock(&self.ops).push(op);
        Ok(())
    }

    fn remove_job(&self, key: &str) -> Result<(), StoreError> {
        self.fails(&format!("jobs/{key}.json"))?;
        lock(&self.ops).push(JobOp::Removed);
        Ok(())
    }
}

/// One admit: the route id, its limit and rpm, and when it happened.
type Admit = (String, Option<u32>, Option<u32>, Instant);

/// An admission that hands out free slots, counting admits and noting when each happened.
#[derive(Default)]
struct Gate {
    admits: Mutex<Vec<Admit>>,
    /// Stop the run at the n-th admit (1-based), as `Pacing` does when cancelled while waiting.
    stop_at: Option<usize>,
}

impl Admission for Gate {
    fn admit<'a>(
        &'a self,
        route_id: &'a str,
        limit: Option<u32>,
        rpm: Option<u32>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Option<Slot>> {
        Box::pin(async move {
            let n = {
                let mut admits = lock(&self.admits);
                admits.push((route_id.to_string(), limit, rpm, Instant::now()));
                admits.len()
            };
            if Some(n) == self.stop_at {
                cancel.cancel();
                return None;
            }
            Some(Slot::free())
        })
    }
}

impl Gate {
    fn count(&self) -> usize {
        lock(&self.admits).len()
    }

    /// The waits between consecutive admits (sends take no time here).
    fn waits(&self) -> Vec<Duration> {
        let admits = lock(&self.admits);
        admits.windows(2).map(|w| w[1].3 - w[0].3).collect()
    }
}

/// Wraps a fake adapter and never answers one kind of request, until the run stops.
struct Hang {
    inner: FakeAdapter,
    on: Op,
    hung: AtomicU32,
}

#[derive(PartialEq)]
enum Op {
    Send,
    Submit,
    Collect,
}

impl Hang {
    fn new(inner: FakeAdapter, on: Op) -> Hang {
        Hang {
            inner,
            on,
            hung: AtomicU32::new(0),
        }
    }
}

impl RequestAdapter for Hang {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        if self.on == Op::Send {
            self.hung.fetch_add(1, Ordering::SeqCst);
            return Box::pin(std::future::pending());
        }
        RequestAdapter::send(&self.inner, call)
    }
}

impl LongJob for Hang {
    fn submit<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
        if self.on == Op::Submit {
            self.hung.fetch_add(1, Ordering::SeqCst);
            return Box::pin(std::future::pending());
        }
        LongJob::submit(&self.inner, call)
    }

    fn collect<'a>(&'a self, call: &'a CallRequest, handle: &'a Value) -> BoxFuture<'a, Collected> {
        if self.on == Op::Collect {
            self.hung.fetch_add(1, Ordering::SeqCst);
            return Box::pin(std::future::pending());
        }
        LongJob::collect(&self.inner, call, handle)
    }
}

/// Everything one call's attempts borrow.
struct Rig {
    book: Book,
    jobs: Jobs,
    gate: Gate,
    cancel: Cancel,
    scopes: Scopes,
    namer: Box<dyn Fn() -> String + Sync>,
}

impl Rig {
    fn new() -> Rig {
        let n = Arc::new(AtomicU64::new(0));
        Rig {
            book: Book::default(),
            jobs: Jobs::default(),
            gate: Gate::default(),
            cancel: Cancel::new(),
            scopes: vec![("scene['a']".into(), Usd(1_000_000))],
            namer: Box::new(move || {
                let n = n.fetch_add(1, Ordering::SeqCst) + 1;
                format!("{INSTANCE}/inv.{n}")
            }),
        }
    }

    fn attempts(&self, capability: &str, job: Option<JobRecord>) -> Attempts<'_> {
        Attempts {
            call: request(capability),
            hold: HOLD,
            scopes: &self.scopes,
            hold_name: &*self.namer,
            limit: Some(2),
            rpm: Some(30),
            book: &self.book,
            jobs: &self.jobs,
            pacing: &self.gate,
            cancel: &self.cancel,
            backoff: Backoff::default(),
            job,
            check: None,
        }
    }

    fn plain(&self) -> Attempts<'_> {
        self.attempts("image.generate", None)
    }

    fn long(&self) -> Attempts<'_> {
        self.attempts("video.generate", Some(job_fields()))
    }

    /// Stops the run after `after` of paused time.
    fn stop_after(&self, after: Duration) {
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            cancel.cancel();
        });
    }
}

fn request(capability: &str) -> CallRequest {
    CallRequest {
        route: RouteRef {
            capability: capability.into(),
            model: "img-a".into(),
            provider: "acme".into(),
            contract: json!({}),
        },
        request: json!({"prompt": "a lantern by the river"}),
        files: IndexMap::new(),
        take: vec![1],
        key: key(),
        attempt: 0,
    }
}

fn job_fields() -> JobRecord {
    JobRecord {
        key: key(),
        capability: "video.generate".into(),
        route: RouteEntry {
            id: "img-a@acme".into(),
            fingerprint: "b".repeat(64),
        },
        request: json!({"prompt": "a lantern by the river"}),
        take: vec![1],
        state: JobState::Settled,
        handle: None,
        note: None,
    }
}

fn answer(cost: Option<Usd>) -> Answer {
    Answer::new(json!({"n": 1}), cost).with_file("image", "image/png", vec![1, 2, 3])
}

fn bad(cost: Option<Usd>) -> Answer {
    Answer::new(json!({"bad": true}), cost)
}

fn not_received(reason: &str) -> Sent {
    Sent::not_received(reason)
}

fn failed(reason: &str, cost: Option<Usd>, retryable: bool) -> Sent {
    Sent::Failed {
        reason: reason.into(),
        cost,
        retryable,
    }
}

fn handle() -> Value {
    json!({"request_id": "R1"})
}

fn hold_name(n: u32) -> String {
    format!("{INSTANCE}/inv.{n}")
}

/// The attempt number of every request the fake was given, in order.
fn attempt_numbers(fake: &FakeAdapter) -> Vec<u32> {
    fake.log()
        .iter()
        .map(|call| call.request().attempt)
        .collect()
}

fn secs(waits: &[f64]) -> Vec<Duration> {
    waits.iter().map(|s| Duration::from_secs_f64(*s)).collect()
}

// ---------------------------------------------------------------------------------------------
// A plain call

#[tokio::test(start_paused = true)]
async fn an_answer_that_passes_its_check_is_settled_at_its_reported_cost() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![Sent::Answered(answer(Some(COST)))]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(
        outcome,
        Outcome::Answered {
            answer: answer(Some(COST)),
            charged: COST,
            attempts: 1,
        }
    );
    assert_eq!(
        *lock(&rig.book.reserved),
        vec![(hold_name(1), HOLD, vec!["scene['a']".to_string()])]
    );
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), Some(COST))]);
    let mut sent = request("image.generate");
    sent.attempt = 1;
    assert_eq!(fake.log(), vec![FakeCall::Send(sent)]);
    let admits = lock(&rig.gate.admits);
    assert_eq!(admits.len(), 1);
    assert_eq!(
        (admits[0].0.as_str(), admits[0].1, admits[0].2),
        ("img-a@acme", Some(2), Some(30))
    );
    assert!(
        rig.jobs.ops().is_empty(),
        "a plain call keeps no job record"
    );
}

#[tokio::test(start_paused = true)]
async fn an_answer_without_a_cost_charges_the_whole_hold() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![Sent::Answered(answer(None))]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(
        outcome,
        Outcome::Answered {
            answer: answer(None),
            charged: HOLD,
            attempts: 1,
        }
    );
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
}

#[tokio::test(start_paused = true)]
async fn two_not_received_then_answered_resend_under_one_hold() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![
        not_received("connection reset"),
        not_received("connection refused"),
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert!(matches!(
        outcome,
        Outcome::Answered {
            charged: COST,
            attempts: 1,
            ..
        }
    ));
    assert_eq!(rig.book.holds(), vec![hold_name(1)]);
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), Some(COST))]);
    assert_eq!(
        attempt_numbers(&fake),
        vec![1, 1, 1],
        "three sends, one attempt"
    );
    assert_eq!(rig.gate.count(), 3, "every send is admitted");
    assert_eq!(rig.gate.waits(), secs(&[0.5, 1.0]));
}

/// `NotReceived` with the wait the provider asked for.
fn rate_limited(secs: f64) -> Sent {
    Sent::NotReceived {
        reason: "rate limited (HTTP 429)".into(),
        retry_after: Some(Duration::from_secs_f64(secs)),
    }
}

#[tokio::test(start_paused = true)]
async fn a_provider_wait_lengthens_a_resend_up_to_a_minute() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![
        rate_limited(20.0),
        rate_limited(0.1),
        rate_limited(3600.0),
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert!(matches!(
        outcome,
        Outcome::Answered {
            charged: COST,
            attempts: 1,
            ..
        }
    ));
    assert_eq!(
        rig.book.holds(),
        vec![hold_name(1)],
        "one attempt, one hold"
    );
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), Some(COST))]);
    // 20 s as asked; then the 1 s backoff beats 0.1 s; then 3600 s capped at 60 s.
    assert_eq!(rig.gate.waits(), secs(&[20.0, 1.0, 60.0]));
}

#[tokio::test(start_paused = true)]
async fn a_provider_wait_applies_to_its_own_resend_only() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![
        rate_limited(20.0),
        failed("HTTP 500", None, true),
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert!(matches!(outcome, Outcome::Answered { attempts: 2, .. }));
    assert_eq!(rig.gate.waits(), secs(&[20.0, 1.0]));
    assert_eq!(attempt_numbers(&fake), vec![1, 1, 2]);
}

#[tokio::test(start_paused = true)]
async fn a_submit_waits_as_long_as_the_provider_asked() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![
            Submitted::NotReceived {
                reason: "rate limited (HTTP 429)".into(),
                retry_after: Some(Duration::from_secs(3)),
            },
            accepted(),
        ],
        vec![Collected::Answered(answer(Some(COST)))],
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert!(matches!(outcome, Outcome::Answered { attempts: 1, .. }));
    assert_eq!(rig.book.holds(), vec![hold_name(1)]);
    assert_eq!(rig.gate.waits(), secs(&[3.0, 0.0]));
}

#[tokio::test(start_paused = true)]
async fn cancelled_during_a_provider_wait_settles_zero() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![rate_limited(30.0), Sent::Answered(answer(Some(COST)))]);
    rig.stop_after(Duration::from_secs(10));
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
    assert_eq!(fake.log().len(), 1, "nothing is resent");
}

#[tokio::test(start_paused = true)]
async fn five_retryable_failures_then_answered_make_six_settled_attempts() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![
        failed("http 500", Some(Usd(1_000)), true),
        failed("http 502", None, true),
        failed("http 503", Some(Usd(2_000)), true),
        failed("timeout after the request left", None, true),
        failed("http 500", Some(Usd::ZERO), true),
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert!(matches!(
        outcome,
        Outcome::Answered {
            charged: COST,
            attempts: 6,
            ..
        }
    ));
    assert_eq!(rig.book.holds(), (1..=6).map(hold_name).collect::<Vec<_>>());
    assert_eq!(
        rig.book.settlements(),
        vec![
            (hold_name(1), Some(Usd(1_000))),
            (hold_name(2), None),
            (hold_name(3), Some(Usd(2_000))),
            (hold_name(4), None),
            (hold_name(5), Some(Usd::ZERO)),
            (hold_name(6), Some(COST)),
        ]
    );
    assert_eq!(attempt_numbers(&fake), vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(rig.gate.waits(), secs(&[0.5, 1.0, 2.0, 4.0, 8.0]));
}

#[tokio::test(start_paused = true)]
async fn six_failures_fail_the_call_after_exactly_six_sends() {
    let rig = Rig::new();
    let script: Vec<Sent> = (1..=7)
        .map(|n| failed(&format!("http 500 #{n}"), None, true))
        .collect();
    let fake = FakeAdapter::plain(script);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Failed("http 500 #6".into()));
    assert_eq!(fake.log().len(), MAX_SENDS as usize);
    assert_eq!(
        fake.left(),
        (1, 0, 0),
        "the seventh outcome is never asked for"
    );
    assert_eq!(rig.book.holds().len(), 6);
    assert_eq!(rig.book.settlements().len(), 6);
    rig.book.assert_all_settled();
    assert_eq!(rig.gate.waits(), secs(&[0.5, 1.0, 2.0, 4.0, 8.0]));
}

#[tokio::test(start_paused = true)]
async fn a_check_refusal_fails_its_attempt_as_billed() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![
        Sent::Answered(bad(None)),
        Sent::Answered(bad(Some(Usd(5_000)))),
        Sent::Answered(answer(Some(COST))),
    ])
    .with_check(refuse_bad());
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert!(matches!(
        outcome,
        Outcome::Answered {
            charged: COST,
            attempts: 3,
            ..
        }
    ));
    assert_eq!(
        rig.book.settlements(),
        vec![
            (hold_name(1), None),
            (hold_name(2), Some(Usd(5_000))),
            (hold_name(3), Some(COST)),
        ],
        "a refused answer is billed: its reported cost, else the whole hold"
    );
    assert_eq!(attempt_numbers(&fake), vec![1, 2, 3]);
}

#[tokio::test(start_paused = true)]
async fn answers_refused_six_times_fail_with_the_check_reason() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain((0..6).map(|_| Sent::Answered(bad(None))).collect())
        .with_check(refuse_bad());
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Failed("the answer is bad".into()));
    assert_eq!(rig.book.settlements().len(), 6);
    rig.book.assert_all_settled();
}

#[tokio::test(start_paused = true)]
async fn a_non_retryable_failure_is_settled_once_and_ends_the_call() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![
        failed("content policy", Some(Usd(3_000)), false),
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Failed("content policy".into()));
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd(3_000)))]
    );
    assert_eq!(fake.log().len(), 1);
    assert_eq!(rig.gate.count(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_refusal_settles_zero_and_is_never_retried() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![
        Sent::Refused {
            reason: "no key for acme".into(),
        },
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Refused("no key for acme".into()));
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
    assert_eq!(fake.log().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_ceiling_refusal_on_attempt_three_keeps_earlier_attempts_settled() {
    let mut rig = Rig::new();
    rig.book.refuse_at = Some(3);
    let fake = FakeAdapter::plain(vec![
        failed("http 500", Some(Usd(1_000)), true),
        failed("http 500", None, true),
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    let Outcome::Ceiling(refusal) = outcome else {
        panic!("expected a ceiling refusal, got {outcome:?}");
    };
    assert_eq!(refusal.needed, HOLD);
    assert!(
        refusal
            .message
            .starts_with("run ceiling reached: gen/inv.3 ")
    );
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd(1_000))), (hold_name(2), None)]
    );
    assert_eq!(*lock(&rig.book.refused), vec![hold_name(3)]);
    assert_eq!(
        fake.log().len(),
        2,
        "nothing is sent for the refused attempt"
    );
    rig.book.assert_all_settled();
}

#[tokio::test(start_paused = true)]
async fn a_ceiling_refusal_on_the_first_attempt_sends_nothing() {
    let mut rig = Rig::new();
    rig.book.refuse_at = Some(1);
    let fake = FakeAdapter::plain(vec![Sent::Answered(answer(Some(COST)))]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert!(matches!(outcome, Outcome::Ceiling(_)));
    assert!(fake.log().is_empty());
    // The slot was taken before the hold, and given back with the refusal.
    assert_eq!(rig.gate.count(), 1);
    assert!(rig.book.settlements().is_empty());
}

#[tokio::test(start_paused = true)]
async fn six_not_received_settle_one_hold_at_zero() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(
        (1..=7)
            .map(|n| not_received(&format!("reset #{n}")))
            .collect(),
    );
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Failed("reset #6".into()));
    assert_eq!(rig.book.holds(), vec![hold_name(1)]);
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
    assert_eq!(attempt_numbers(&fake), vec![1; 6]);
    assert_eq!(rig.gate.waits(), secs(&[0.5, 1.0, 2.0, 4.0, 8.0]));
}

#[tokio::test(start_paused = true)]
async fn resends_count_toward_the_six_sends() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![
        not_received("reset"),
        not_received("reset"),
        failed("http 500 #1", None, true),
        failed("http 500 #2", None, true),
        failed("http 500 #3", None, true),
        failed("http 500 #4", None, true),
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Failed("http 500 #4".into()));
    assert_eq!(attempt_numbers(&fake), vec![1, 1, 1, 2, 3, 4]);
    assert_eq!(rig.book.holds().len(), 4);
    rig.book.assert_all_settled();
}

#[tokio::test(start_paused = true)]
async fn cancelled_while_a_send_is_in_flight_settles_the_whole_hold() {
    let rig = Rig::new();
    let hang = Hang::new(FakeAdapter::default(), Op::Send);
    rig.stop_after(Duration::from_secs(1));
    let outcome = send_plain(&hang, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(hang.hung.load(Ordering::SeqCst), 1);
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
}

#[tokio::test(start_paused = true)]
async fn cancelled_before_a_send_leaves_settles_zero() {
    let mut rig = Rig::new();
    rig.gate.stop_at = Some(2);
    let fake = FakeAdapter::plain(vec![
        not_received("reset"),
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(fake.log().len(), 1, "the resend never left");
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
}

#[tokio::test(start_paused = true)]
async fn a_stopped_run_sends_nothing() {
    let rig = Rig::new();
    rig.cancel.cancel();
    let fake = FakeAdapter::plain(vec![Sent::Answered(answer(Some(COST)))]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert!(fake.log().is_empty());
    assert_eq!(rig.gate.count(), 0);
    // Nothing waited for a slot, so nothing was reserved.
    assert!(rig.book.holds().is_empty());
    assert!(rig.book.settlements().is_empty());
}

#[tokio::test(start_paused = true)]
async fn cancelled_while_waiting_to_resend_settles_zero() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![
        not_received("reset"),
        Sent::Answered(answer(Some(COST))),
    ]);
    rig.stop_after(Duration::from_millis(250));
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(fake.log().len(), 1);
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
}

#[tokio::test(start_paused = true)]
async fn cancelled_while_waiting_for_a_new_attempt_reserves_nothing_more() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![
        failed("http 500", Some(Usd(1_000)), true),
        Sent::Answered(answer(Some(COST))),
    ]);
    rig.stop_after(Duration::from_millis(250));
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(rig.book.holds(), vec![hold_name(1)]);
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd(1_000)))]
    );
}

#[test]
fn the_owner_s_futures_can_move_between_threads() {
    fn is_send<T: Send>(_: &T) {}
    let rig = Rig::new();
    let fake = FakeAdapter::plain(Vec::new());
    let attempts = rig.long();
    let handle = handle();
    is_send(&send_plain(&fake, &attempts));
    is_send(&submit_job(&fake, &attempts));
    is_send(&collect_job(&fake, &attempts, &handle));
}

#[tokio::test(start_paused = true)]
async fn a_new_attempt_waits_for_its_slot_before_it_holds_anything() {
    let mut rig = Rig::new();
    rig.gate.stop_at = Some(1);
    let fake = FakeAdapter::plain(vec![Sent::Answered(answer(Some(COST)))]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(rig.gate.count(), 1);
    assert!(
        rig.book.holds().is_empty(),
        "no hold while waiting for the slot"
    );
    assert!(fake.log().is_empty());
}

#[tokio::test(start_paused = true)]
async fn answers_are_checked_in_their_canonical_form_inside_the_owner() {
    let rig = Rig::new();
    let fake = FakeAdapter::plain(vec![
        Sent::Answered(Answer::new(
            json!({"z": 1, "a": 2.0, "bad": true}),
            Some(Usd(1_000)),
        )),
        Sent::Answered(Answer::new(json!({"z": 1, "a": 2.0}), Some(COST))),
    ]);
    let seen = Mutex::new(Vec::new());
    let check = |answer: &Answer| -> Result<(), String> {
        lock(&seen).push(serde_json::to_string(&answer.data).unwrap());
        match answer.data.get("bad") {
            Some(_) => Err("the reply is malformed".into()),
            None => Ok(()),
        }
    };
    let mut attempts = rig.plain();
    attempts.check = Some(&check);
    let outcome = send_plain(&fake, &attempts).await;
    let Outcome::Answered {
        answer, attempts, ..
    } = outcome
    else {
        panic!("expected an answer, got {outcome:?}");
    };
    assert_eq!(attempts, 2);
    // The check saw the canonical form, and the answer carries it.
    assert_eq!(
        *lock(&seen),
        vec![r#"{"a":2,"bad":true,"z":1}"#, r#"{"a":2,"z":1}"#]
    );
    assert_eq!(
        serde_json::to_string(&answer.data).unwrap(),
        r#"{"a":2,"z":1}"#
    );
    // The refused attempt was billed and settled; the next one was a new hold.
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd(1_000))), (hold_name(2), Some(COST))]
    );
    rig.book.assert_all_settled();
}

#[tokio::test(start_paused = true)]
async fn an_answer_that_cannot_be_recorded_fails_its_attempt() {
    let rig = Rig::new();
    let mut deep = json!(1);
    for _ in 0..600 {
        deep = json!([deep]);
    }
    let fake = FakeAdapter::plain(vec![
        Sent::Answered(Answer::new(deep, Some(Usd(1_000)))),
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    assert!(matches!(outcome, Outcome::Answered { attempts: 2, .. }));
    assert_eq!(rig.book.holds().len(), 2);
    rig.book.assert_all_settled();
}

#[tokio::test(start_paused = true)]
async fn an_unrecorded_reservation_sends_nothing() {
    let mut rig = Rig::new();
    rig.book.unrecorded_at = Some(2);
    let fake = FakeAdapter::plain(vec![
        failed("http 500", Some(Usd(1_000)), true),
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    let Outcome::Unrecorded(reason) = outcome else {
        panic!("expected an unrecorded hold, got {outcome:?}");
    };
    assert!(reason.contains("budget_reserved of gen/inv.2"), "{reason}");
    assert_eq!(fake.log().len(), 1, "the second attempt never left");
    rig.book.assert_all_settled();
}

#[tokio::test(start_paused = true)]
async fn an_unrecorded_settlement_makes_no_new_attempt() {
    let mut rig = Rig::new();
    rig.book.unsettled_at = Some(1);
    let fake = FakeAdapter::plain(vec![
        failed("http 500", Some(Usd(1_000)), true),
        Sent::Answered(answer(Some(COST))),
    ]);
    let outcome = send_plain(&fake, &rig.plain()).await;
    let Outcome::Unrecorded(reason) = outcome else {
        panic!("expected an unrecorded settlement, got {outcome:?}");
    };
    assert!(reason.contains("budget_settled of gen/inv.1"), "{reason}");
    assert_eq!(fake.log().len(), 1);
    assert_eq!(rig.book.holds(), vec![hold_name(1)]);
    // A long job likewise.
    let mut rig = Rig::new();
    rig.book.unsettled_at = Some(1);
    let fake = FakeAdapter::long_job(
        vec![
            Submitted::Failed {
                reason: "http 500".into(),
                cost: None,
                retryable: true,
            },
            accepted(),
        ],
        Vec::new(),
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert!(matches!(outcome, Outcome::Unrecorded(_)), "{outcome:?}");
    assert_eq!(fake.log().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// A long job

fn accepted() -> Submitted {
    Submitted::Accepted { handle: handle() }
}

fn submitting() -> JobOp {
    JobOp::Saved(JobState::Submitting, None)
}

fn submitted() -> JobOp {
    JobOp::Saved(JobState::Submitted, Some(handle()))
}

fn settled(handle: Option<Value>) -> JobOp {
    JobOp::Saved(JobState::Settled, handle)
}

#[tokio::test(start_paused = true)]
async fn an_accepted_job_is_collected_under_its_hold() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![accepted()],
        vec![Collected::Answered(answer(Some(COST)))],
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(
        outcome,
        Outcome::Answered {
            answer: answer(Some(COST)),
            charged: COST,
            attempts: 1,
        }
    );
    assert_eq!(
        rig.jobs.ops(),
        vec![submitting(), submitted()],
        "the call path, not the owner, removes the record after the call record"
    );
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), Some(COST))]);
    let mut sent = request("video.generate");
    sent.attempt = 1;
    assert_eq!(
        fake.log(),
        vec![
            FakeCall::Submit(sent.clone()),
            FakeCall::Collect(sent, handle())
        ]
    );
    assert_eq!(
        rig.gate.count(),
        2,
        "the submit and the collect each take the slot"
    );
}

#[tokio::test(start_paused = true)]
async fn a_collected_answer_without_a_cost_charges_the_whole_hold() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(vec![accepted()], vec![Collected::Answered(answer(None))]);
    let outcome = submit_job(&fake, &rig.long()).await;
    assert!(matches!(
        outcome,
        Outcome::Answered {
            charged: HOLD,
            attempts: 1,
            ..
        }
    ));
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
}

#[tokio::test(start_paused = true)]
async fn a_submit_not_received_is_resent_under_the_same_hold() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![
            Submitted::NotReceived {
                reason: "reset".into(),
                retry_after: None,
            },
            accepted(),
        ],
        vec![Collected::Answered(answer(Some(COST)))],
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert!(matches!(outcome, Outcome::Answered { attempts: 1, .. }));
    assert_eq!(
        rig.jobs.ops(),
        vec![submitting(), submitting(), submitted()]
    );
    assert_eq!(rig.book.holds(), vec![hold_name(1)]);
    assert_eq!(attempt_numbers(&fake), vec![1, 1, 1]);
    assert_eq!(rig.gate.waits(), secs(&[0.5, 0.0]));
}

#[tokio::test(start_paused = true)]
async fn six_submits_not_received_remove_the_record_and_settle_zero() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        (1..=7)
            .map(|n| Submitted::NotReceived {
                reason: format!("reset #{n}"),
                retry_after: None,
            })
            .collect(),
        Vec::new(),
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(outcome, Outcome::Failed("reset #6".into()));
    let mut expected = vec![submitting(); 6];
    expected.push(JobOp::Removed);
    assert_eq!(rig.jobs.ops(), expected);
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
    assert_eq!(fake.left(), (0, 1, 0));
}

#[tokio::test(start_paused = true)]
async fn a_refused_submit_removes_the_record_and_settles_zero() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![Submitted::Refused {
            reason: "no key for acme".into(),
        }],
        Vec::new(),
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(outcome, Outcome::Refused("no key for acme".into()));
    assert_eq!(rig.jobs.ops(), vec![submitting(), JobOp::Removed]);
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
}

#[tokio::test(start_paused = true)]
async fn an_uncertain_submit_keeps_submitting_and_settles_in_full() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![
            Submitted::Uncertain {
                reason: "timed out after the request left".into(),
            },
            accepted(),
        ],
        Vec::new(),
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(
        outcome,
        Outcome::Unsettled("timed out after the request left".into())
    );
    // The record stays `submitting`, now with the reason as its note (spec/store.md §5).
    assert_eq!(
        rig.jobs.ops(),
        vec![
            submitting(),
            JobOp::Noted("timed out after the request left".into())
        ]
    );
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
    assert_eq!(fake.log().len(), 1, "never submitted again");
}

#[tokio::test(start_paused = true)]
async fn an_uncertain_submit_s_note_is_redacted_and_names_the_provider_s_job() {
    let rig = Rig::new();
    // A fake key in an OpenAI-like shape, which every redactor masks.
    let fake = FakeAdapter::long_job(
        vec![Submitted::Uncertain {
            reason: "fal took the video job but returned no handle to collect it by \
                     (request req-7); key sk-test-not-a-real-key-0000"
                .into(),
        }],
        Vec::new(),
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    let note = "fal took the video job but returned no handle to collect it by (request req-7); \
                key [redacted]";
    assert_eq!(outcome, Outcome::Unsettled(note.into()));
    assert_eq!(
        rig.jobs.ops(),
        vec![submitting(), JobOp::Noted(note.into())]
    );
}

#[tokio::test(start_paused = true)]
async fn a_note_that_cannot_be_written_stops_the_run_and_settles_in_full() {
    let mut rig = Rig::new();
    rig.jobs.fail_at = Some(2);
    let fake = FakeAdapter::long_job(
        vec![Submitted::Uncertain {
            reason: "timed out after the request left".into(),
        }],
        Vec::new(),
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert!(matches!(outcome, Outcome::Store(_)), "{outcome:?}");
    // The record written before the submit stays `submitting`, without a note.
    assert_eq!(rig.jobs.ops(), vec![submitting()]);
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
    assert_eq!(fake.log().len(), 1, "never submitted again");
}

#[tokio::test(start_paused = true)]
async fn a_failed_submit_is_settled_and_a_new_attempt_submits_anew() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![
            Submitted::Failed {
                reason: "provider error".into(),
                cost: Some(Usd(1_000)),
                retryable: true,
            },
            accepted(),
        ],
        vec![Collected::Answered(answer(Some(COST)))],
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert!(matches!(
        outcome,
        Outcome::Answered {
            charged: COST,
            attempts: 2,
            ..
        }
    ));
    assert_eq!(
        rig.jobs.ops(),
        vec![submitting(), settled(None), submitting(), submitted()]
    );
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd(1_000))), (hold_name(2), Some(COST))]
    );
    assert_eq!(attempt_numbers(&fake), vec![1, 2, 2]);
    assert_eq!(rig.gate.waits(), secs(&[0.5, 0.0]));
}

#[tokio::test(start_paused = true)]
async fn a_non_retryable_submit_failure_settles_the_record_and_ends() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![
            Submitted::Failed {
                reason: "content policy".into(),
                cost: None,
                retryable: false,
            },
            accepted(),
        ],
        Vec::new(),
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(outcome, Outcome::Failed("content policy".into()));
    assert_eq!(rig.jobs.ops(), vec![submitting(), settled(None)]);
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
}

#[tokio::test(start_paused = true)]
async fn six_failed_submits_fail_after_six_sends() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        (1..=7)
            .map(|n| Submitted::Failed {
                reason: format!("provider error #{n}"),
                cost: None,
                retryable: true,
            })
            .collect(),
        Vec::new(),
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(outcome, Outcome::Failed("provider error #6".into()));
    let expected: Vec<JobOp> = (0..6).flat_map(|_| [submitting(), settled(None)]).collect();
    assert_eq!(rig.jobs.ops(), expected);
    assert_eq!(rig.book.settlements().len(), 6);
    rig.book.assert_all_settled();
}

#[tokio::test(start_paused = true)]
async fn a_collected_answer_its_check_refuses_settles_the_record_as_billed() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![accepted(), accepted()],
        vec![
            Collected::Answered(bad(Some(Usd(7_000)))),
            Collected::Answered(answer(Some(COST))),
        ],
    )
    .with_check(refuse_bad());
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(outcome, Outcome::Failed("the answer is bad".into()));
    assert_eq!(
        rig.jobs.ops(),
        vec![submitting(), submitted(), settled(Some(handle()))]
    );
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd(7_000)))]
    );
    assert_eq!(
        fake.log().len(),
        2,
        "a long job is never submitted again in the call"
    );
}

#[tokio::test(start_paused = true)]
async fn a_job_that_ended_without_a_result_is_settled_in_full() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![accepted()],
        vec![Collected::Ended {
            reason: "the job failed".into(),
        }],
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(outcome, Outcome::Failed("the job failed".into()));
    assert_eq!(
        rig.jobs.ops(),
        vec![submitting(), submitted(), settled(Some(handle()))]
    );
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
}

#[tokio::test(start_paused = true)]
async fn an_unreachable_job_stays_submitted_and_is_settled_in_full() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![accepted()],
        vec![Collected::Unreachable {
            reason: "collect timed out".into(),
        }],
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(outcome, Outcome::Failed("collect timed out".into()));
    assert_eq!(rig.jobs.ops(), vec![submitting(), submitted()]);
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
}

#[tokio::test(start_paused = true)]
async fn cancelled_while_submitting_keeps_submitting_and_settles_in_full() {
    let rig = Rig::new();
    let hang = Hang::new(FakeAdapter::default(), Op::Submit);
    rig.stop_after(Duration::from_secs(1));
    let outcome = submit_job(&hang, &rig.long()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(rig.jobs.ops(), vec![submitting()]);
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
}

#[tokio::test(start_paused = true)]
async fn cancelled_while_collecting_keeps_submitted_and_settles_in_full() {
    let rig = Rig::new();
    let hang = Hang::new(
        FakeAdapter::long_job(vec![accepted()], Vec::new()),
        Op::Collect,
    );
    rig.stop_after(Duration::from_secs(1));
    let outcome = submit_job(&hang, &rig.long()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(hang.hung.load(Ordering::SeqCst), 1);
    assert_eq!(rig.jobs.ops(), vec![submitting(), submitted()]);
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
}

#[tokio::test(start_paused = true)]
async fn cancelled_before_collecting_keeps_submitted_and_settles_in_full() {
    let mut rig = Rig::new();
    rig.gate.stop_at = Some(2);
    let fake = FakeAdapter::long_job(
        vec![accepted()],
        vec![Collected::Answered(answer(Some(COST)))],
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(rig.jobs.ops(), vec![submitting(), submitted()]);
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
    assert_eq!(fake.log().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn cancelled_while_waiting_to_resubmit_removes_the_record() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![
            Submitted::NotReceived {
                reason: "reset".into(),
                retry_after: None,
            },
            accepted(),
        ],
        Vec::new(),
    );
    rig.stop_after(Duration::from_millis(250));
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(rig.jobs.ops(), vec![submitting(), JobOp::Removed]);
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
}

#[tokio::test(start_paused = true)]
async fn cancelled_before_a_resubmit_leaves_removes_the_record() {
    let mut rig = Rig::new();
    rig.gate.stop_at = Some(2);
    let fake = FakeAdapter::long_job(
        vec![
            Submitted::NotReceived {
                reason: "reset".into(),
                retry_after: None,
            },
            accepted(),
        ],
        Vec::new(),
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(rig.jobs.ops(), vec![submitting(), JobOp::Removed]);
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
    assert_eq!(fake.log().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_ceiling_refusal_writes_no_job_record() {
    let mut rig = Rig::new();
    rig.book.refuse_at = Some(1);
    let fake = FakeAdapter::long_job(vec![accepted()], Vec::new());
    let outcome = submit_job(&fake, &rig.long()).await;
    assert!(matches!(outcome, Outcome::Ceiling(_)));
    assert!(rig.jobs.ops().is_empty());
    assert!(fake.log().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_record_that_cannot_be_written_stops_before_the_submit_leaves() {
    let mut rig = Rig::new();
    rig.jobs.fail_at = Some(1);
    let fake = FakeAdapter::long_job(vec![accepted()], Vec::new());
    let outcome = submit_job(&fake, &rig.long()).await;
    assert_eq!(
        outcome,
        Outcome::Store(StoreError::Io {
            what: format!("jobs/{}.json", key()),
            reason: "disk full".into(),
        })
    );
    assert!(fake.log().is_empty(), "nothing leaves without its record");
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
}

#[tokio::test(start_paused = true)]
async fn a_submitted_record_that_cannot_be_written_settles_in_full() {
    let mut rig = Rig::new();
    rig.jobs.fail_at = Some(2);
    let fake = FakeAdapter::long_job(
        vec![accepted()],
        vec![Collected::Answered(answer(Some(COST)))],
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert!(matches!(outcome, Outcome::Store(_)));
    assert_eq!(
        rig.jobs.ops(),
        vec![submitting()],
        "the record stays submitting"
    );
    assert_eq!(rig.book.settlements(), vec![(hold_name(1), None)]);
    assert_eq!(fake.log().len(), 1, "nothing is collected");
}

#[tokio::test(start_paused = true)]
async fn a_record_that_cannot_be_removed_still_settles_zero() {
    let mut rig = Rig::new();
    rig.jobs.fail_at = Some(2);
    let fake = FakeAdapter::long_job(
        vec![Submitted::Refused {
            reason: "no key".into(),
        }],
        Vec::new(),
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert!(matches!(outcome, Outcome::Store(_)));
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd::ZERO))]
    );
}

#[tokio::test(start_paused = true)]
async fn a_settled_record_that_cannot_be_written_stops_after_settling() {
    let mut rig = Rig::new();
    rig.jobs.fail_at = Some(2);
    let fake = FakeAdapter::long_job(
        vec![
            Submitted::Failed {
                reason: "provider error".into(),
                cost: Some(Usd(1_000)),
                retryable: true,
            },
            accepted(),
        ],
        Vec::new(),
    );
    let outcome = submit_job(&fake, &rig.long()).await;
    assert!(matches!(outcome, Outcome::Store(_)));
    assert_eq!(
        rig.book.settlements(),
        vec![(hold_name(1), Some(Usd(1_000)))]
    );
    assert_eq!(fake.log().len(), 1, "no new attempt after a store error");
}

#[tokio::test(start_paused = true)]
async fn a_long_job_without_record_fields_is_an_engine_fault() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(vec![accepted()], Vec::new());
    let attempts = rig.attempts("video.generate", None);
    assert!(matches!(
        submit_job(&fake, &attempts).await,
        Outcome::Store(StoreError::Io { .. })
    ));
    assert!(matches!(
        collect_job(&fake, &attempts, &handle()).await,
        Outcome::Store(StoreError::Io { .. })
    ));
    assert!(rig.book.holds().is_empty());
    assert!(fake.log().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Collecting a job an earlier run submitted

#[tokio::test(start_paused = true)]
async fn collecting_reserves_nothing_and_charges_nothing() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(Vec::new(), vec![Collected::Answered(answer(Some(COST)))]);
    let outcome = collect_job(&fake, &rig.long(), &handle()).await;
    assert_eq!(
        outcome,
        Outcome::Answered {
            answer: answer(Some(COST)),
            charged: Usd::ZERO,
            attempts: 0,
        }
    );
    assert!(rig.book.holds().is_empty());
    assert!(rig.book.settlements().is_empty());
    assert!(
        rig.jobs.ops().is_empty(),
        "the call path removes the record"
    );
    let mut sent = request("video.generate");
    sent.attempt = 1;
    assert_eq!(fake.log(), vec![FakeCall::Collect(sent, handle())]);
    assert_eq!(rig.gate.count(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_collected_answer_its_check_refuses_settles_the_record() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(Vec::new(), vec![Collected::Answered(bad(None))])
        .with_check(refuse_bad());
    let outcome = collect_job(&fake, &rig.long(), &handle()).await;
    assert_eq!(outcome, Outcome::Failed("the answer is bad".into()));
    assert_eq!(rig.jobs.ops(), vec![settled(Some(handle()))]);
    assert!(rig.book.holds().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_collected_job_that_ended_is_settled() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        Vec::new(),
        vec![Collected::Ended {
            reason: "the job expired".into(),
        }],
    );
    let outcome = collect_job(&fake, &rig.long(), &handle()).await;
    assert_eq!(outcome, Outcome::Failed("the job expired".into()));
    assert_eq!(rig.jobs.ops(), vec![settled(Some(handle()))]);
    assert!(rig.book.holds().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_unreachable_collected_job_is_left_submitted() {
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        Vec::new(),
        vec![Collected::Unreachable {
            reason: "collect timed out".into(),
        }],
    );
    let outcome = collect_job(&fake, &rig.long(), &handle()).await;
    assert_eq!(outcome, Outcome::Failed("collect timed out".into()));
    assert!(rig.jobs.ops().is_empty());
    assert!(rig.book.holds().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_collect_cancelled_in_flight_leaves_the_record() {
    let rig = Rig::new();
    let hang = Hang::new(FakeAdapter::default(), Op::Collect);
    rig.stop_after(Duration::from_secs(1));
    let outcome = collect_job(&hang, &rig.long(), &handle()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert!(rig.jobs.ops().is_empty());
    assert!(rig.book.holds().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_collect_of_a_stopped_run_never_leaves() {
    let rig = Rig::new();
    rig.cancel.cancel();
    let fake = FakeAdapter::long_job(Vec::new(), vec![Collected::Answered(answer(Some(COST)))]);
    let outcome = collect_job(&fake, &rig.long(), &handle()).await;
    assert_eq!(outcome, Outcome::Cancelled);
    assert!(fake.log().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_settled_record_that_cannot_be_written_after_collecting_stops_the_run() {
    let mut rig = Rig::new();
    rig.jobs.fail_at = Some(1);
    let fake = FakeAdapter::long_job(
        Vec::new(),
        vec![Collected::Ended {
            reason: "the job expired".into(),
        }],
    );
    let outcome = collect_job(&fake, &rig.long(), &handle()).await;
    assert!(matches!(outcome, Outcome::Store(_)));
}

// ---------------------------------------------------------------------------------------------
// Against the real ledger and store

#[tokio::test(start_paused = true)]
async fn every_attempt_is_charged_by_the_ledger() {
    use grida_fx_runtime::ledger::Ledger;
    let rig = Rig::new();
    let ledger = Ledger::new(Some(Usd(1_000_000)), None);
    let fake = FakeAdapter::plain(vec![
        failed("http 500", Some(Usd(10_000)), true),
        failed("http 500", None, true),
        Sent::Answered(answer(Some(COST))),
    ]);
    let mut attempts = rig.plain();
    attempts.book = &ledger;
    let outcome = send_plain(&fake, &attempts).await;
    assert!(matches!(outcome, Outcome::Answered { attempts: 3, .. }));
    assert_eq!(ledger.charged(), Usd(10_000) + HOLD + COST);
    assert_eq!(ledger.held(), Usd::ZERO);
}

#[tokio::test(start_paused = true)]
async fn the_store_keeps_the_job_states() {
    use grida_fx_runtime::store::Store;
    let dir = tempfile::tempdir().expect("a temporary folder");
    let store = Store::open(dir.path());
    let rig = Rig::new();
    let fake = FakeAdapter::long_job(
        vec![accepted()],
        vec![Collected::Ended {
            reason: "the job failed".into(),
        }],
    );
    let mut attempts = rig.long();
    attempts.jobs = &store;
    let outcome = submit_job(&fake, &attempts).await;
    assert_eq!(outcome, Outcome::Failed("the job failed".into()));
    let record = store
        .load_job(&key())
        .expect("a readable job record")
        .expect("a job record");
    assert_eq!(record.state, JobState::Settled);
    assert_eq!(record.handle, Some(handle()));
}
