//! Billing across calls (docs/wg/overview.md "Retry and billing (ratified)"; spec/protocol.md
//! §6.1, §6.2): one flight per call key, keys that ended for good, waiting for calls left in
//! flight, holds taken only after the route's slot, reservations and settlements the run's log
//! cannot record, the engine's own check of `agent.turn` replies, and answers that are the same
//! live and on replay. Everything goes through the real call path, store and ledger with
//! scripted adapters; nothing calls a provider.

use grida_fx_core::money::Usd;
use grida_fx_core::routes::{PriceUnit, Route, RoutePrice};
use grida_fx_core::value::file_digest;
use grida_fx_protocol::{AgentRunParams, AgentTool, ErrorCode, RpcError, ToolInvokeResult};
use grida_fx_providers::fake::FakeAdapter;
use grida_fx_providers::{
    Adapter, Adapters, Answer, BoxFuture, CallRequest, Collected, LongJob, RequestAdapter, Sent,
    Submitted,
};
use grida_fx_runtime::agent::{AgentBody, TurnCaller, run_agent};
use grida_fx_runtime::calls::pacing::Pacing;
use grida_fx_runtime::calls::{CallAnswer, CallCounter, CallError, CallSite, call};
use grida_fx_runtime::engine::{Cancel, Engine, RunFiles, Services};
use grida_fx_runtime::events::{EventLog, read_events};
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::ledger::{Ledger, NotReserved};
use grida_fx_runtime::store::records::call_key;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const HOLD: i64 = 40_000;

fn route(capability: &str, model: &str, high: i64) -> Route {
    Route {
        capability: capability.into(),
        model: model.into(),
        provider: "acme".into(),
        price: RoutePrice {
            low: Usd(high / 4),
            high: Usd(high),
            unit: PriceUnit::Call,
            max_units: None,
            by: None,
            tiers: IndexMap::new(),
        },
        features: Default::default(),
        concurrency: None,
        requests_per_minute: None,
        contract: json!({}),
    }
}

fn site(instance: &str, capability: &str, model: &str) -> CallSite {
    CallSite {
        instance_id: instance.into(),
        path: instance.split('#').next().unwrap_or(instance).into(),
        step: instance.split('#').next().unwrap_or(instance).into(),
        type_name: "t".into(),
        takes: vec![1],
        calls: IndexMap::from([(capability.to_string(), 10)]),
        routes: IndexMap::from([(capability.to_string(), route(capability, model, HOLD))]),
        limits: IndexMap::new(),
        scopes: Vec::new(),
        cancel: Cancel::new(),
    }
}

/// One invocation over a store in `root`, with an event log, a ledger and the adapters given.
struct Rig {
    root: PathBuf,
    services: Arc<Services>,
    ledger: Arc<Ledger>,
}

fn rig_in(root: &Path, adapters: Adapters, live: bool, ceiling: Option<Usd>) -> Rig {
    let engine = Arc::new(Engine::new(
        tokio::runtime::Handle::current(),
        HostSpec {
            python: PathBuf::from("python3"),
            label: "python3".into(),
            project_root: root.to_path_buf(),
            sources: Vec::new(),
        },
        &root.join(".fx/cache"),
        1,
        adapters,
        live,
    ));
    let log = Arc::new(EventLog::open(&root.join("events.jsonl"), "inv", &"0".repeat(64)).unwrap());
    let ledger = Arc::new(Ledger::new(ceiling, Some(Arc::clone(&log))));
    let services = Arc::new(Services {
        engine,
        ledger: Some(Arc::clone(&ledger)),
        events: Some(log),
        pacing: Arc::new(Pacing::new()),
        cancel: Cancel::new(),
        invocation_id: "inv".into(),
        runs: AtomicU64::new(0),
        holds: AtomicU64::new(0),
    });
    Rig {
        root: root.to_path_buf(),
        services,
        ledger,
    }
}

fn adapters(capability: &str, adapter: Adapter) -> Adapters {
    let mut adapters = Adapters::new();
    adapters.register(capability, "acme", adapter);
    adapters
}

impl Rig {
    fn events(&self, name: &str) -> Vec<Value> {
        read_events(&self.root.join("events.jsonl"))
            .unwrap()
            .into_iter()
            .filter(|event| event["event"] == name)
            .collect()
    }

    /// Every hold reserved was settled once, and the settlements add up to the ledger.
    fn assert_settled(&self) {
        let mut open: IndexMap<String, ()> = IndexMap::new();
        let mut sum = Usd::ZERO;
        for event in read_events(&self.root.join("events.jsonl")).unwrap() {
            let id = event["node_id"].as_str().unwrap_or_default().to_string();
            match event["event"].as_str() {
                Some("budget_reserved") => assert!(open.insert(id, ()).is_none()),
                Some("budget_settled") => {
                    assert!(open.shift_remove(&id).is_some(), "{id} settled twice");
                    sum = sum + Usd::from_value(&event["charged_usd"]).unwrap();
                }
                _ => {}
            }
        }
        assert!(open.is_empty(), "holds left open: {open:?}");
        assert_eq!(sum, self.ledger.charged());
        assert_eq!(self.ledger.held(), Usd::ZERO);
    }

    async fn call(&self, site: &CallSite, capability: &str, request: Value) -> Called {
        let counter = CallCounter::new();
        let files = RunFiles::new();
        let result = call(&self.services, site, &counter, &files, capability, request).await;
        Called { result, files }
    }
}

struct Called {
    result: Result<CallAnswer, CallError>,
    files: RunFiles,
}

impl Called {
    fn answer(&self) -> &CallAnswer {
        self.result.as_ref().expect("an answer")
    }

    fn rpc(&self) -> RpcError {
        match &self.result {
            Err(CallError::Rpc(error)) => error.clone(),
            other => panic!("expected an RPC error, got {other:?}"),
        }
    }
}

fn picture(n: u8) -> Answer {
    Answer::new(json!({"seed": n}), Some(Usd(10_000))).with_file("image", "image/png", vec![n])
}

/// Answers every send after `after`, counting the sends.
struct Slow {
    after: Duration,
    sends: AtomicU32,
}

impl Slow {
    fn new(after: Duration) -> Arc<Slow> {
        Arc::new(Slow {
            after,
            sends: AtomicU32::new(0),
        })
    }

    fn sends(&self) -> u32 {
        self.sends.load(Ordering::SeqCst)
    }
}

impl RequestAdapter for Slow {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        self.sends.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            tokio::time::sleep(self.after).await;
            // One picture per prompt.
            let seed = call.request["prompt"].as_str().map_or(0, |p| p.len() as u8);
            Sent::Answered(picture(seed))
        })
    }
}

// ---------------------------------------------------------------------------------------------
// One flight per key

#[tokio::test(start_paused = true)]
async fn a_key_in_flight_is_sent_once_and_its_followers_cost_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let slow = Slow::new(Duration::from_secs(3));
    let rig = rig_in(
        dir.path(),
        adapters("image.generate", Adapter::Request(slow.clone())),
        true,
        None,
    );
    let request = json!({"prompt": "a lantern"});
    // Two instances with one request, and a rerun of the first while its call is still out.
    let (draw_1, draw_2) = (
        site("draw#1", "image.generate", "img-a"),
        site("draw#2", "image.generate", "img-a"),
    );
    let (first, second, rerun) = tokio::join!(
        rig.call(&draw_1, "image.generate", request.clone()),
        rig.call(&draw_2, "image.generate", request.clone()),
        rig.call(&draw_1, "image.generate", request.clone()),
    );
    assert_eq!(slow.sends(), 1);
    let led = first.answer();
    assert!(!led.cached);
    assert_eq!(led.charged, Usd(10_000));
    for followed in [&second, &rerun] {
        let answer = followed.answer();
        assert!(answer.cached);
        assert_eq!(answer.cost, Some(Usd::ZERO));
        assert_eq!(answer.charged, Usd::ZERO);
        assert_eq!(answer.key, led.key);
        assert_eq!(answer.data, led.data);
        assert_eq!(answer.files, led.files);
        // The follower's run was handed the answer's file.
        let digest = &answer.files["image"].digest;
        assert!(followed.files.get(digest).is_some());
    }
    let calls = rig.events("call");
    let cached: Vec<bool> = calls.iter().map(|e| e["cached"] == true).collect();
    assert_eq!(cached.iter().filter(|c| !**c).count(), 1, "{calls:?}");
    assert_eq!(cached.iter().filter(|c| **c).count(), 2, "{calls:?}");
    assert_eq!(rig.ledger.charged(), Usd(10_000));
    rig.assert_settled();
    // Later calls of the key are ordinary cache hits.
    let again = rig
        .call(
            &site("draw#3", "image.generate", "img-a"),
            "image.generate",
            request,
        )
        .await;
    assert!(again.answer().cached);
    assert_eq!(slow.sends(), 1);
}

/// A long-job adapter that counts submits; each job answers after `collect_after`.
struct Jobs {
    submits: AtomicU32,
    collect_after: Duration,
    handles: Mutex<Vec<Value>>,
}

impl LongJob for Jobs {
    fn submit<'a>(&'a self, _call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
        let n = self.submits.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Submitted::Accepted {
                handle: json!({"job": n}),
            }
        })
    }

    fn collect<'a>(
        &'a self,
        _call: &'a CallRequest,
        handle: &'a Value,
    ) -> BoxFuture<'a, Collected> {
        self.handles.lock().unwrap().push(handle.clone());
        Box::pin(async move {
            tokio::time::sleep(self.collect_after).await;
            Collected::Answered(Answer::new(json!({}), Some(Usd(20_000))).with_file(
                "video",
                "video/mp4",
                vec![9],
            ))
        })
    }
}

#[tokio::test(start_paused = true)]
async fn a_long_job_in_flight_is_submitted_once_and_keeps_its_handle() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = Arc::new(Jobs {
        submits: AtomicU32::new(0),
        collect_after: Duration::from_secs(5),
        handles: Mutex::new(Vec::new()),
    });
    let rig = rig_in(
        dir.path(),
        adapters("video.generate", Adapter::Job(jobs.clone())),
        true,
        None,
    );
    let request = json!({"prompt": "same"});
    let (clip_1, clip_2) = (
        site("clip#1", "video.generate", "vid-a"),
        site("clip#2", "video.generate", "vid-a"),
    );
    let (a, b) = tokio::join!(
        rig.call(&clip_1, "video.generate", request.clone()),
        rig.call(&clip_2, "video.generate", request.clone()),
    );
    assert_eq!(jobs.submits.load(Ordering::SeqCst), 1);
    assert_eq!(*jobs.handles.lock().unwrap(), vec![json!({"job": 1})]);
    assert!(!a.answer().cached);
    assert!(b.answer().cached);
    let key = &a.answer().key;
    let store = &rig.services.engine.store;
    assert!(store.load_call(key).is_some());
    assert_eq!(store.load_job(key).unwrap(), None);
    assert_eq!(rig.ledger.charged(), Usd(20_000));
    rig.assert_settled();
}

// ---------------------------------------------------------------------------------------------
// Keys that ended for good

#[tokio::test(start_paused = true)]
async fn a_key_that_failed_is_never_sent_again_in_the_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let failing = (0..12)
        .map(|_| Sent::Failed {
            reason: "500".into(),
            cost: Some(Usd(1_000)),
            retryable: true,
        })
        .collect();
    let fake = FakeAdapter::plain(failing);
    let rig = rig_in(
        dir.path(),
        adapters("image.generate", fake.as_request()),
        true,
        None,
    );
    let request = json!({"prompt": "p"});
    let draw = site("draw#1", "image.generate", "img-a");
    let first = rig.call(&draw, "image.generate", request.clone()).await;
    assert_eq!(first.rpc().code, ErrorCode::CallFailed.code());
    assert_eq!(fake.log().len(), 6);
    // A rerun of the body (a new counter), and another instance with the same request: both are
    // answered with the same error, and nothing more is sent or reserved.
    let rerun = rig.call(&draw, "image.generate", request.clone()).await;
    let other = rig
        .call(
            &site("draw#2", "image.generate", "img-a"),
            "image.generate",
            request.clone(),
        )
        .await;
    assert_eq!(rerun.rpc(), first.rpc());
    assert_eq!(other.rpc(), first.rpc());
    assert_eq!(fake.log().len(), 6);
    assert_eq!(rig.events("budget_reserved").len(), 6);
    assert_eq!(rig.ledger.charged(), Usd(6_000));
    rig.assert_settled();
    // Another request is its own key.
    let fresh = rig
        .call(&draw, "image.generate", json!({"prompt": "q"}))
        .await;
    assert_eq!(fresh.rpc().code, ErrorCode::CallFailed.code());
    assert_eq!(fake.log().len(), 12);
}

#[tokio::test(start_paused = true)]
async fn a_refused_key_is_not_sent_again_and_a_ceiling_ends_it_for_its_instance() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(vec![Sent::Refused {
        reason: "no key".into(),
    }]);
    let rig = rig_in(
        dir.path(),
        adapters("image.generate", fake.as_request()),
        true,
        Some(Usd(30_000)),
    );
    // A hold of $0.04 never fits under $0.03: refused before anything is sent.
    let draw = site("draw#1", "image.generate", "img-a");
    let first = rig
        .call(&draw, "image.generate", json!({"prompt": "p"}))
        .await;
    assert_eq!(first.rpc().code, ErrorCode::CeilingExceeded.code());
    let rerun = rig
        .call(&draw, "image.generate", json!({"prompt": "p"}))
        .await;
    assert_eq!(rerun.rpc(), first.rpc());
    assert_eq!(rig.events("budget_refused").len(), 1);
    // Another instance asks the ledger itself (its own step budgets may differ).
    let other = rig
        .call(
            &site("draw#2", "image.generate", "img-a"),
            "image.generate",
            json!({"prompt": "p"}),
        )
        .await;
    assert_eq!(other.rpc().code, ErrorCode::CeilingExceeded.code());
    assert_eq!(rig.events("budget_refused").len(), 2);
    assert!(fake.log().is_empty());

    // capability_refused: settled at $0 once, then answered without a send.
    let mut cheap = site("cheap#1", "image.generate", "img-a");
    cheap.routes.insert(
        "image.generate".into(),
        route("image.generate", "img-a", 1_000),
    );
    let refused = rig
        .call(&cheap, "image.generate", json!({"prompt": "r"}))
        .await;
    assert_eq!(refused.rpc().code, ErrorCode::CapabilityRefused.code());
    let again = rig
        .call(&cheap, "image.generate", json!({"prompt": "r"}))
        .await;
    assert_eq!(again.rpc(), refused.rpc());
    assert_eq!(fake.log().len(), 1);
    rig.assert_settled();
}

// ---------------------------------------------------------------------------------------------
// Waiting for calls in flight

#[tokio::test(start_paused = true)]
async fn an_invocation_waits_for_a_call_its_caller_left() {
    let dir = tempfile::tempdir().unwrap();
    let slow = Slow::new(Duration::from_secs(9));
    let rig = rig_in(
        dir.path(),
        adapters("image.generate", Adapter::Request(slow.clone())),
        true,
        None,
    );
    let services = Arc::clone(&rig.services);
    assert_eq!(services.calls_running(), 0);
    // A caller that stops waiting (a step that ran past its timeout): the call goes on.
    let running = services.track_call();
    let left = {
        let services = Arc::clone(&services);
        tokio::spawn(async move {
            let _running = running;
            let site = site("draw#1", "image.generate", "img-a");
            call(
                &services,
                &site,
                &CallCounter::new(),
                &RunFiles::new(),
                "image.generate",
                json!({"prompt": "p"}),
            )
            .await
        })
    };
    drop(left);
    assert_eq!(
        services.calls_running(),
        1,
        "counted before its task first ran"
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(rig.ledger.held(), Usd(HOLD), "the call's hold is open");
    services.calls_settled().await;
    assert_eq!(services.calls_running(), 0);
    assert_eq!(slow.sends(), 1);
    assert_eq!(rig.ledger.charged(), Usd(10_000));
    rig.assert_settled();
    assert_eq!(rig.events("call").len(), 1);
    // Nothing running: the wait ends at once.
    tokio::time::timeout(Duration::from_millis(1), services.calls_settled())
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_stopped_invocation_settles_its_calls_in_flight_in_full() {
    let dir = tempfile::tempdir().unwrap();
    let slow = Slow::new(Duration::from_secs(60));
    let rig = rig_in(
        dir.path(),
        adapters("image.generate", Adapter::Request(slow.clone())),
        true,
        None,
    );
    let services = Arc::clone(&rig.services);
    let task = {
        let services = Arc::clone(&services);
        tokio::spawn(async move {
            call(
                &services,
                &site("draw#1", "image.generate", "img-a"),
                &CallCounter::new(),
                &RunFiles::new(),
                "image.generate",
                json!({"prompt": "p"}),
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    services.cancel.cancel();
    services.calls_settled().await;
    assert_eq!(task.await.unwrap(), Err(CallError::Cancelled));
    assert_eq!(rig.ledger.charged(), Usd(HOLD));
    rig.assert_settled();
}

// ---------------------------------------------------------------------------------------------
// The slot before the hold

#[tokio::test(start_paused = true)]
async fn calls_waiting_for_a_slot_hold_none_of_the_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let slow = Slow::new(Duration::from_millis(100));
    let rig = rig_in(
        dir.path(),
        adapters("image.generate", Adapter::Request(slow.clone())),
        true,
        Some(Usd(100_000)),
    );
    let mut calls = tokio::task::JoinSet::new();
    for n in 0..5 {
        let services = Arc::clone(&rig.services);
        calls.spawn(async move {
            let mut site = site(&format!("draw#{n}"), "image.generate", "img-a");
            site.limits.insert("image.generate".into(), Some(1));
            call(
                &services,
                &site,
                &CallCounter::new(),
                &RunFiles::new(),
                "image.generate",
                json!({"prompt": "p".repeat(n + 1)}),
            )
            .await
        });
    }
    while let Some(done) = calls.join_next().await {
        assert!(done.unwrap().is_ok());
    }
    // One hold of $0.04 open at a time, five answers at $0.01: all fit under $0.10.
    assert_eq!(slow.sends(), 5);
    assert_eq!(rig.ledger.charged(), Usd(50_000));
    assert!(rig.events("budget_refused").is_empty());
    rig.assert_settled();
}

// ---------------------------------------------------------------------------------------------
// What the log cannot record

/// A pipe as the run's events.jsonl: writes succeed while its reader is open and fail (EPIPE)
/// once it is closed.
#[cfg(unix)]
struct Pipe {
    path: PathBuf,
    reader: Arc<Mutex<Option<std::fs::File>>>,
}

#[cfg(unix)]
impl Pipe {
    /// Makes the pipe and opens a log on it (blocking until both ends are open).
    fn open(root: &Path) -> (Pipe, Arc<EventLog>) {
        let path = root.join("events.jsonl");
        let made = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(made.success());
        let opener = {
            let path = path.clone();
            std::thread::spawn(move || std::fs::File::open(path).unwrap())
        };
        let log = Arc::new(EventLog::open(&path, "inv", &"0".repeat(64)).unwrap());
        let reader = opener.join().unwrap();
        (
            Pipe {
                path,
                reader: Arc::new(Mutex::new(Some(reader))),
            },
            log,
        )
    }

    fn close(&self) {
        self.reader.lock().unwrap().take();
    }
}

#[cfg(unix)]
fn rig_with_log(root: &Path, adapters: Adapters, log: Arc<EventLog>) -> Rig {
    let engine = Arc::new(Engine::new(
        tokio::runtime::Handle::current(),
        HostSpec {
            python: PathBuf::from("python3"),
            label: "python3".into(),
            project_root: root.to_path_buf(),
            sources: Vec::new(),
        },
        &root.join(".fx/cache"),
        1,
        adapters,
        true,
    ));
    let ledger = Arc::new(Ledger::new(Some(Usd(1_000_000)), Some(Arc::clone(&log))));
    let services = Arc::new(Services {
        engine,
        ledger: Some(Arc::clone(&ledger)),
        events: Some(log),
        pacing: Arc::new(Pacing::new()),
        cancel: Cancel::new(),
        invocation_id: "inv".into(),
        runs: AtomicU64::new(0),
        holds: AtomicU64::new(0),
    });
    Rig {
        root: root.to_path_buf(),
        services,
        ledger,
    }
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_reservation_the_log_cannot_record_sends_nothing_and_stops_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let (pipe, log) = Pipe::open(dir.path());
    pipe.close();
    let fake = FakeAdapter::plain(vec![Sent::Answered(picture(1))]);
    let rig = rig_with_log(
        dir.path(),
        adapters("image.generate", fake.as_request()),
        log,
    );
    let called = rig
        .call(
            &site("draw#1", "image.generate", "img-a"),
            "image.generate",
            json!({"prompt": "p"}),
        )
        .await;
    let Err(CallError::Fault(fault)) = &called.result else {
        panic!("expected a fault, got {:?}", called.result)
    };
    assert!(
        fault.contains("could not record budget_reserved of draw#1/inv.1"),
        "{fault}"
    );
    assert!(
        fake.log().is_empty(),
        "nothing is sent for an unrecorded hold"
    );
    assert_eq!(rig.ledger.held(), Usd::ZERO);
    assert_eq!(rig.ledger.charged(), Usd::ZERO);
    assert!(rig.ledger.fault().is_some());
    // The ledger stays refusing: no later attempt goes out either.
    assert!(matches!(
        rig.ledger
            .reserve_recorded("x#1/inv.9".into(), Usd(1), &Vec::new()),
        Err(NotReserved::Unrecorded(_))
    ));
    let _ = std::fs::remove_file(&pipe.path);
}

/// Answers after closing the pipe's reader, so the settlement cannot be recorded.
#[cfg(unix)]
struct ClosesTheLog {
    pipe: Arc<Mutex<Option<std::fs::File>>>,
    sends: AtomicU32,
}

#[cfg(unix)]
impl RequestAdapter for ClosesTheLog {
    fn send<'a>(&'a self, _call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        self.sends.fetch_add(1, Ordering::SeqCst);
        self.pipe.lock().unwrap().take();
        Box::pin(async {
            Sent::Failed {
                reason: "500".into(),
                cost: Some(Usd(5_000)),
                retryable: true,
            }
        })
    }
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_settlement_the_log_cannot_record_ends_the_call_as_a_fault() {
    let dir = tempfile::tempdir().unwrap();
    let (pipe, log) = Pipe::open(dir.path());
    let adapter = Arc::new(ClosesTheLog {
        pipe: Arc::clone(&pipe.reader),
        sends: AtomicU32::new(0),
    });
    let rig = rig_with_log(
        dir.path(),
        adapters("image.generate", Adapter::Request(adapter.clone())),
        log,
    );
    let called = rig
        .call(
            &site("draw#1", "image.generate", "img-a"),
            "image.generate",
            json!({"prompt": "p"}),
        )
        .await;
    let Err(CallError::Fault(fault)) = &called.result else {
        panic!("expected a fault, got {:?}", called.result)
    };
    assert!(
        fault.contains("could not record budget_settled of draw#1/inv.1"),
        "{fault}"
    );
    // The billed attempt is charged, and no new attempt follows it.
    assert_eq!(adapter.sends.load(Ordering::SeqCst), 1);
    assert_eq!(rig.ledger.charged(), Usd(5_000));
    assert_eq!(rig.ledger.held(), Usd::ZERO);
    let _ = std::fs::remove_file(&pipe.path);
}

// ---------------------------------------------------------------------------------------------
// agent.turn replies and canonical answers

fn turn_site(instance: &str) -> CallSite {
    let mut site = site(instance, "agent.turn", "llm-a");
    site.routes
        .insert("agent.turn".into(), route("agent.turn", "llm-a", 4_000));
    site
}

#[tokio::test(start_paused = true)]
async fn a_malformed_turn_reply_fails_its_attempt_and_is_never_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(vec![
        Sent::Answered(Answer::new(
            json!({"text": "hi", "tool_calls": "oops"}),
            Some(Usd(2_000)),
        )),
        Sent::Answered(Answer::new(
            json!({"text": "done", "tool_calls": []}),
            Some(Usd(2_000)),
        )),
    ]);
    let rig = rig_in(
        dir.path(),
        adapters("agent.turn", fake.as_request()),
        true,
        None,
    );
    let request = json!({"system": "s", "messages": [], "tools": [], "tool_choice": "auto"});
    let called = rig.call(&turn_site("think#1"), "agent.turn", request).await;
    let answer = called.answer();
    assert_eq!(answer.data, json!({"text": "done", "tool_calls": []}));
    // Both attempts were billed, and the second is a new reserved one.
    assert_eq!(fake.log().len(), 2);
    assert_eq!(answer.charged, Usd(4_000));
    assert_eq!(rig.events("budget_reserved").len(), 2);
    let record = rig.services.engine.store.load_call(&answer.key).unwrap();
    assert_eq!(record.data, json!({"text": "done", "tool_calls": []}));
    rig.assert_settled();

    // Six malformed replies: call_failed, nothing recorded, and a rerun sends nothing more.
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(
        (0..6)
            .map(|_| Sent::Answered(Answer::new(json!({"text": 5}), Some(Usd(1_000)))))
            .collect(),
    );
    let rig = rig_in(
        dir.path(),
        adapters("agent.turn", fake.as_request()),
        true,
        None,
    );
    let request = json!({"system": "s", "messages": [], "tools": [], "tool_choice": "auto"});
    let failed = rig
        .call(&turn_site("think#1"), "agent.turn", request.clone())
        .await;
    let error = failed.rpc();
    assert_eq!(error.code, ErrorCode::CallFailed.code());
    assert_eq!(
        error.message,
        "agent.turn on llm-a@acme failed: the reply is not {\"text\", \"tool_calls\"}: text is \
         not a string"
    );
    let key = error.data.as_ref().unwrap()["key"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(rig.services.engine.store.load_call(&key).is_none());
    assert_eq!(fake.log().len(), 6);
    let rerun = rig.call(&turn_site("think#1"), "agent.turn", request).await;
    assert_eq!(rerun.rpc(), error);
    assert_eq!(fake.log().len(), 6);
    rig.assert_settled();
}

#[tokio::test(start_paused = true)]
async fn a_live_answer_is_the_value_its_replay_gives() {
    let dir = tempfile::tempdir().unwrap();
    let answer = Answer::new(
        json!({"z": 1, "degrees": 90.0, "nested": {"b": 2.50, "a": [1.0e2]}}),
        Some(Usd(10_000)),
    )
    .with_file("zeta", "image/png", vec![2])
    .with_file("alpha", "image/png", vec![1]);
    let fake = FakeAdapter::plain(vec![Sent::Answered(answer)]);
    let request = json!({"prompt": "p"});
    let live = {
        let rig = rig_in(
            dir.path(),
            adapters("image.generate", fake.as_request()),
            true,
            None,
        );
        rig.call(
            &site("draw#1", "image.generate", "img-a"),
            "image.generate",
            request.clone(),
        )
        .await
        .result
        .unwrap()
    };
    // A later invocation, offline and with no adapter, replays the record.
    std::fs::remove_file(dir.path().join("events.jsonl")).unwrap();
    let rig = rig_in(dir.path(), Adapters::new(), false, None);
    let replayed = rig
        .call(
            &site("draw#1", "image.generate", "img-a"),
            "image.generate",
            request,
        )
        .await
        .result
        .unwrap();
    assert!(replayed.cached);
    assert_eq!(live.data, replayed.data);
    assert_eq!(
        serde_json::to_string(&live.data).unwrap(),
        serde_json::to_string(&replayed.data).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&live.data).unwrap(),
        r#"{"degrees":90,"nested":{"a":[100],"b":2.5},"z":1}"#
    );
    let names = |answer: &CallAnswer| answer.files.keys().cloned().collect::<Vec<_>>();
    assert_eq!(names(&live), ["alpha", "zeta"]);
    assert_eq!(names(&live), names(&replayed));
    assert_eq!(live.files, replayed.files);
}

// ---------------------------------------------------------------------------------------------
// The agent loop over the call path

struct Turns {
    services: Arc<Services>,
    site: CallSite,
    counter: CallCounter,
    files: RunFiles,
}

impl TurnCaller for Turns {
    fn turn(&self, request: Value) -> BoxFuture<'_, Result<CallAnswer, CallError>> {
        Box::pin(async move {
            call(
                &self.services,
                &self.site,
                &self.counter,
                &self.files,
                "agent.turn",
                request,
            )
            .await
        })
    }
}

/// A body whose tools answer with the arguments they were given, as canonical text.
#[derive(Default)]
struct Echo {
    seen: Mutex<Vec<String>>,
}

impl AgentBody for Echo {
    fn invoke<'a>(
        &'a self,
        _agent_id: &'a str,
        _call_id: Option<&'a str>,
        _name: &'a str,
        arguments: &'a IndexMap<String, Value>,
    ) -> BoxFuture<'a, Result<ToolInvokeResult, RpcError>> {
        let shown = serde_json::to_string(arguments).unwrap();
        self.seen.lock().unwrap().push(shown.clone());
        Box::pin(async move {
            Ok(ToolInvokeResult::Content {
                content: Value::from(format!("did {shown}")),
                images: None,
            })
        })
    }

    fn check<'a>(
        &'a self,
        _agent_id: &'a str,
        _value: &'a Value,
    ) -> BoxFuture<'a, Result<Option<String>, RpcError>> {
        Box::pin(async { Ok(None) })
    }
}

fn agent_params(tools: &[&str]) -> AgentRunParams {
    AgentRunParams {
        run_id: "r1".into(),
        agent_id: "a1".into(),
        system: "Be brief.".into(),
        instructions: "do it".into(),
        images: None,
        tools: tools
            .iter()
            .map(|name| AgentTool {
                name: name.to_string(),
                description: "A tool.".into(),
                parameters: IndexMap::from([("type".to_string(), json!("object"))]),
            })
            .collect(),
        max_steps: 4,
        submit: None,
        check: false,
        recent_images: None,
        max_tokens: None,
    }
}

#[tokio::test(start_paused = true)]
async fn an_agent_replays_offline_with_the_arguments_it_saw_live() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(vec![
        Sent::Answered(Answer::new(
            json!({"text": "", "tool_calls": [
                {"id": "c1", "name": "rotate", "arguments": {"degrees": 90.0, "axis": "z"}}
            ]}),
            Some(Usd(1_000)),
        )),
        Sent::Answered(Answer::new(json!({"text": "done"}), Some(Usd(1_000)))),
    ]);
    let params = agent_params(&["rotate"]);
    let (live, live_seen) = {
        let rig = rig_in(
            dir.path(),
            adapters("agent.turn", fake.as_request()),
            true,
            None,
        );
        let turns = Turns {
            services: Arc::clone(&rig.services),
            site: turn_site("think#1"),
            counter: CallCounter::new(),
            files: RunFiles::new(),
        };
        let body = Echo::default();
        let result = run_agent(&params, &turns, &body).await.unwrap();
        (result, body.seen.into_inner().unwrap())
    };
    assert_eq!(fake.log().len(), 2);
    std::fs::remove_file(dir.path().join("events.jsonl")).unwrap();
    let rig = rig_in(dir.path(), Adapters::new(), false, None);
    let turns = Turns {
        services: Arc::clone(&rig.services),
        site: turn_site("think#1"),
        counter: CallCounter::new(),
        files: RunFiles::new(),
    };
    let body = Echo::default();
    let replayed = run_agent(&params, &turns, &body).await.unwrap();
    assert_eq!(*body.seen.lock().unwrap(), live_seen);
    assert_eq!(live_seen, [r#"{"axis":"z","degrees":90}"#]);
    assert_eq!(replayed.transcript, live.transcript);
    assert_eq!(replayed.turns, 2);
    assert_eq!(replayed.cost_usd, 0.0);
}

#[tokio::test(start_paused = true)]
async fn arguments_the_model_wrote_never_read_as_fx_values() {
    let dir = tempfile::tempdir().unwrap();
    let digest = file_digest(b"a file the run was never handed");
    let fake = FakeAdapter::plain(vec![
        Sent::Answered(Answer::new(
            json!({"text": "", "tool_calls": [
                {"id": "c1", "name": "count", "arguments": {"collection": [1, 2]}},
                {"id": "c2", "name": "fetch", "arguments": {"file": digest}},
                {"id": "c3", "name": "fetch", "arguments": {"x": {"missing": true}}}
            ]}),
            Some(Usd(1_000)),
        )),
        Sent::Answered(Answer::new(json!({"text": "done"}), Some(Usd(1_000)))),
    ]);
    let rig = rig_in(
        dir.path(),
        adapters("agent.turn", fake.as_request()),
        true,
        None,
    );
    let turns = Turns {
        services: Arc::clone(&rig.services),
        site: turn_site("think#1"),
        counter: CallCounter::new(),
        files: RunFiles::new(),
    };
    let result = run_agent(&agent_params(&["count", "fetch"]), &turns, &Echo::default())
        .await
        .unwrap();
    assert_eq!(result.turns, 2);
    // The second turn carries the calls with their arguments as canonical JSON text.
    let log = fake.log();
    let second = &log[1].request().request;
    assert_eq!(
        second["messages"][1]["tool_calls"],
        json!([
            {"id": "c1", "name": "count", "arguments": "{\"collection\":[1,2]}"},
            {"id": "c2", "name": "fetch", "arguments": format!("{{\"file\":\"{digest}\"}}")},
            {"id": "c3", "name": "fetch", "arguments": "{\"x\":{\"missing\":true}}"}
        ])
    );
    assert!(log[1].request().files.is_empty(), "no file is attached");
    assert_eq!(
        serde_json::to_value(&result.transcript[1]).unwrap()["tool_calls"][0]["arguments"],
        json!("{\"collection\":[1,2]}")
    );
    // The canonical request is keyed as sent.
    let key = call_key(
        "agent.turn",
        &route("agent.turn", "llm-a", 4_000).fingerprint(),
        second,
        &[1],
    );
    assert!(rig.services.engine.store.load_call(&key).is_some());
}
