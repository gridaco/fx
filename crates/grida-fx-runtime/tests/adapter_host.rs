//! A project's own adapter in a live run (spec/adapters.md §9; spec/protocol.md §5.8, §6.8),
//! against a peer that plays the adapter host over an in-memory connection, and, where an exit
//! status matters, a fake host process: every outcome, handles, `fetch` and its rules, a lost
//! host, a host that cannot serve, cancellation and shutdown. Nothing leaves the process: the
//! transport is a `ReplayTransport` (every request asserted) or `Offline`.

use grida_fx_core::money::Usd;
use grida_fx_core::providers::{
    BaseUrl, Credential, Declaration, Declarations, DeclaredAdapter, LoadedAdapter,
};
use grida_fx_protocol::{CallKind, ClosureEntry};
use grida_fx_providers::adapter::{
    Adapter, Answer, AnsweredFile, CallRequest, Collected, LongJob, RequestAdapter, RequestFile,
    RouteRef, Sent, Submitted, Usage,
};
use grida_fx_providers::clock::FakeClock;
use grida_fx_providers::keys::{Environment, Keys};
use grida_fx_providers::setup::Setup;
use grida_fx_providers::transport::replay::{Expect, ExpectBody, ReplayTransport};
use grida_fx_providers::transport::{
    HttpResponse, Lane, Method, Offline, Transport, TransportError, TransportErrorKind,
};
use grida_fx_providers::{Adapters, BoxFuture};
use grida_fx_runtime::adapter_host::{self, Launch, Launched};
use grida_fx_runtime::host::connection::{Connection, Incoming};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::mpsc;

const NAME: &str = "adapters/acme.py#acme";
const KEY: &str = "acme-made-up-key-5e1f";
const BASE: &str = "https://api.acme.test/v1";
const DIGEST: &str = "ad5e9999a3966063951fa4baef73a311c378ae034778afe41d450d98876c5859";

// ---------------------------------------------------------------------------------------------
// The peer

/// The adapter host's end of an in-memory connection.
struct Peer {
    input: BufReader<DuplexStream>,
    output: DuplexStream,
    next_id: i64,
}

impl Peer {
    async fn read(&mut self) -> Value {
        let mut length = 0;
        loop {
            let mut line = String::new();
            let read = self.input.read_line(&mut line).await.unwrap();
            assert!(read > 0, "the engine closed the connection");
            if line == "\r\n" {
                break;
            }
            if let Some(n) = line.strip_prefix("Content-Length: ") {
                length = n.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        self.input.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn write(&mut self, message: Value) {
        let body = serde_json::to_vec(&message).unwrap();
        let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        frame.extend(body);
        self.output.write_all(&frame).await.unwrap();
    }

    async fn answer(&mut self, request: &Value, result: Value) {
        self.write(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}))
            .await;
    }

    async fn error(&mut self, request: &Value, code: i64, message: &str) {
        self.write(json!({"jsonrpc": "2.0", "id": request["id"],
                          "error": {"code": code, "message": message}}))
            .await;
    }

    /// Reads the next request, which must be `method`.
    async fn expect(&mut self, method: &str) -> Value {
        let message = self.read().await;
        assert_eq!(message["method"], json!(method), "{message}");
        message
    }

    /// Reads `adapter.load` and answers it as planned.
    async fn loads(&mut self, root: &Path) {
        let load = self.expect("adapter.load").await;
        assert_eq!(
            load["params"],
            json!({"path": "adapters/acme.py", "attribute": "acme"})
        );
        self.answer(&load, load_answer(root, Some(2))).await;
    }

    /// Sends `fetch` with `params` and reads the engine's answer: its result, or its error.
    async fn fetch(&mut self, params: Value) -> Result<Value, Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.write(json!({"jsonrpc": "2.0", "id": id, "method": "fetch", "params": params}))
            .await;
        let answer = self.read().await;
        assert_eq!(answer["id"], json!(id), "{answer}");
        match answer.get("error") {
            Some(error) => Err(error.clone()),
            None => Ok(answer["result"].clone()),
        }
    }
}

/// Launches a peer per host and hands it to the test.
struct Peers {
    sender: mpsc::UnboundedSender<Peer>,
    launches: Arc<AtomicUsize>,
}

impl Launch for Peers {
    fn launch(
        &self,
        incoming: Arc<dyn Incoming>,
        _: Arc<AtomicBool>,
    ) -> BoxFuture<'_, Result<Launched, String>> {
        Box::pin(async move {
            self.launches.fetch_add(1, Ordering::SeqCst);
            let (engine_reads, peer_writes) = tokio::io::duplex(1 << 20);
            let (peer_reads, engine_writes) = tokio::io::duplex(1 << 20);
            let connection = Connection::start(engine_reads, engine_writes, incoming);
            let peer = Peer {
                input: BufReader::new(peer_reads),
                output: peer_writes,
                next_id: 0,
            };
            self.sender
                .send(peer)
                .map_err(|_| "the test is gone".to_string())?;
            Ok(Launched {
                connection,
                process: None,
            })
        })
    }
}

// ---------------------------------------------------------------------------------------------
// The rig

/// A project folder holding the adapter's module.
fn project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("adapters")).unwrap();
    std::fs::write(root.join("adapters/acme.py"), "acme = adapter()\n").unwrap();
    (dir, root)
}

fn closure(root: &Path) -> Vec<ClosureEntry> {
    vec![ClosureEntry {
        label: "adapters/acme.py".into(),
        path: root.join("adapters/acme.py").to_string_lossy().into_owned(),
    }]
}

fn kinds() -> IndexMap<String, CallKind> {
    [
        ("structured.generate".to_string(), CallKind::Request),
        ("acme.render".to_string(), CallKind::Job),
    ]
    .into_iter()
    .collect()
}

fn load_answer(root: &Path, version: Option<u64>) -> Value {
    json!({
        "adapter": {"version": version, "capabilities": kinds()},
        "closure": closure(root),
    })
}

/// `acme`, with a key and `files.acme.test` to download from, and `local`, without a key; both
/// served by the project adapter as planning loaded it.
fn declarations(root: &Path) -> Declarations {
    let loaded = LoadedAdapter {
        version: Some(2),
        capabilities: kinds(),
        source: grida_fx_core::registry::closure_digests(&closure(root)).unwrap(),
        drift: None,
    };
    let adapter = DeclaredAdapter {
        path: "adapters/acme.py".into(),
        attribute: "acme".into(),
        loaded: Some(loaded),
    };
    let declared = |name: &str, credential: Option<Credential>, base: &str| Declaration {
        name: name.into(),
        label: name.into(),
        credential,
        base_url: BaseUrl {
            variable: None,
            default: base.into(),
        },
        download_hosts: vec!["files.acme.test".into()],
        family: None,
        adapter: Some(adapter.clone()),
    };
    Declarations::with(&[
        declared(
            "acme",
            Some(Credential {
                variable: "ACME_API_KEY".into(),
                header: "authorization".into(),
                scheme: "Bearer".into(),
            }),
            BASE,
        ),
        declared("local", None, "http://127.0.0.1:8080/v1"),
    ])
}

struct Rig {
    _dir: tempfile::TempDir,
    root: PathBuf,
    adapters: Adapters,
    peers: mpsc::UnboundedReceiver<Peer>,
    launches: Arc<AtomicUsize>,
}

impl Rig {
    /// The adapters a live run registers over `transport`, with the key unless `keyless`.
    fn new(transport: Arc<dyn Transport>, keyed: bool) -> Rig {
        let (dir, root) = project();
        let declarations = declarations(&root);
        let pairs: &[(&str, &str)] = if keyed { &[("ACME_API_KEY", KEY)] } else { &[] };
        let environment = Environment::declared(pairs, &declarations);
        let setup = Setup::new(
            transport,
            Keys::from_environment(&environment),
            Arc::new(FakeClock::new()),
        );
        let (sender, peers) = mpsc::unbounded_channel();
        let launches = Arc::new(AtomicUsize::new(0));
        let launch: Arc<dyn Launch> = Arc::new(Peers {
            sender,
            launches: Arc::clone(&launches),
        });
        let mut adapters = Adapters::new();
        adapter_host::register(&mut adapters, &setup, |_, _| Arc::clone(&launch));
        Rig {
            _dir: dir,
            root,
            adapters,
            peers,
            launches,
        }
    }

    fn request(&self, provider: &str) -> Arc<dyn RequestAdapter> {
        match self
            .adapters
            .serving(&route("structured.generate", provider))
        {
            Some(Adapter::Request(adapter)) => Arc::clone(adapter),
            other => panic!("no request adapter: {other:?}"),
        }
    }

    fn job(&self) -> Arc<dyn LongJob> {
        match self.adapters.serving(&route("acme.render", "acme")) {
            Some(Adapter::Job(adapter)) => Arc::clone(adapter),
            other => panic!("no job adapter: {other:?}"),
        }
    }

    async fn peer(&mut self) -> Peer {
        tokio::time::timeout(Duration::from_secs(10), self.peers.recv())
            .await
            .expect("a host was launched")
            .unwrap()
    }

    async fn shutdown(&self) -> bool {
        let mut verified = true;
        for runtime in self.adapters.runtimes() {
            verified &= runtime.shutdown().await;
        }
        verified
    }
}

fn route(capability: &str, provider: &str) -> RouteRef {
    RouteRef {
        capability: capability.into(),
        model: "model-a".into(),
        provider: provider.into(),
        contract: json!({"adapter": NAME, "adapter_behavior": 2}),
        limits: Default::default(),
    }
}

fn call(capability: &str, provider: &str) -> CallRequest {
    CallRequest {
        route: route(capability, provider),
        request: json!({"prompt": "a kite", "references": [{"file": DIGEST}]}),
        files: IndexMap::from([(
            DIGEST.to_string(),
            RequestFile {
                digest: DIGEST.into(),
                kind: "image/png".into(),
                size: 136,
                path: PathBuf::from("/store/files/ad/ad5e"),
            },
        )]),
        take: vec![1, 2],
        key: "k".repeat(64),
        attempt: 3,
    }
}

/// Sends a `structured.generate` call on `provider` on a task of its own.
fn sending(adapter: &Arc<dyn RequestAdapter>, provider: &str) -> tokio::task::JoinHandle<Sent> {
    let adapter = Arc::clone(adapter);
    let call = call("structured.generate", provider);
    tokio::spawn(async move { adapter.send(&call).await })
}

/// Sends one call whose host answers `result`; the call's outcome.
async fn sent(rig: &mut Rig, result: Value) -> Sent {
    let task = sending(&rig.request("acme"), "acme");
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let send = peer.expect("adapter.send").await;
    peer.answer(&send, result).await;
    task.await.unwrap()
}

fn unreadable(place: &str) -> String {
    format!("the adapter {NAME} answered something FX cannot read: {place}")
}

fn lost(reason: &str) -> Sent {
    Sent::Failed {
        reason: reason.into(),
        cost: None,
        retryable: false,
    }
}

// ---------------------------------------------------------------------------------------------
// Calls and outcomes

#[tokio::test]
async fn a_send_carries_the_call_and_its_answer_is_shaped_and_billed() {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let task = sending(&rig.request("acme"), "acme");
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let send = peer.expect("adapter.send").await;
    assert_eq!(
        send["params"],
        json!({
            "call_id": "c1",
            "adapter": {"path": "adapters/acme.py", "attribute": "acme"},
            "capability": "structured.generate",
            "route": {"id": "model-a@acme", "model": "model-a", "provider": "acme",
                      "contract": {"adapter": NAME, "adapter_behavior": 2}},
            "base_url": BASE,
            "request": {"prompt": "a kite", "references": [{"file": DIGEST}]},
            "files": {DIGEST: {"digest": DIGEST, "kind": "image/png", "size": 136,
                               "path": "/store/files/ad/ad5e"}},
            "take": [1, 2],
            "key": "k".repeat(64),
            "attempt": 3
        })
    );
    peer.answer(
        &send,
        json!({"outcome": "answered", "files": {}, "data": {"json": {"kite": true}},
               "cost_usd": 0.04, "usage": {"input_tokens": 12, "output_tokens": 3}}),
    )
    .await;
    let answer =
        Answer::new(json!({"json": {"kite": true}}), Some(Usd(40_000))).with_usage(Usage {
            input_tokens: Some(12),
            output_tokens: Some(3),
            ..Usage::default()
        });
    assert_eq!(task.await.unwrap(), Sent::Answered(answer));
    assert_eq!(rig.launches.load(Ordering::SeqCst), 1);
    // The identity is the adapter's name and version; dollars, the invocation's redactor.
    let adapter = rig.request("acme");
    let identity = adapter.identity().unwrap();
    assert_eq!((identity.adapter, identity.behavior), (NAME, "2"));
    assert_eq!(
        adapter.billing(),
        grida_fx_providers::adapter::Billing::Dollars
    );
    assert_eq!(
        adapter.redactor().reason(&format!("used {KEY}")),
        "used [redacted]"
    );
    // The run's cleanup ends the host: `shutdown`, then `exit`.
    let ending = tokio::spawn(async move { rig.shutdown().await });
    let shutdown = peer.expect("shutdown").await;
    peer.answer(&shutdown, Value::Null).await;
    peer.expect("exit").await;
    drop(peer);
    assert!(ending.await.unwrap());
}

#[tokio::test]
async fn each_send_outcome_maps_to_the_attempts() {
    let cases: Vec<(Value, Sent)> = vec![
        (
            json!({"outcome": "not_received", "reason": "busy", "retry_after_s": 2.5}),
            Sent::NotReceived {
                reason: "busy".into(),
                retry_after: Some(Duration::from_millis(2500)),
            },
        ),
        (
            json!({"outcome": "refused", "reason": "a blank prompt"}),
            Sent::Refused {
                reason: "a blank prompt".into(),
            },
        ),
        (
            json!({"outcome": "failed", "reason": "acme returned HTTP 500", "retryable": true}),
            Sent::Failed {
                reason: "acme returned HTTP 500".into(),
                cost: None,
                retryable: true,
            },
        ),
        (
            json!({"outcome": "failed", "reason": format!("rejected {KEY}"), "retryable": false,
                   "cost_usd": 0.5}),
            Sent::Failed {
                reason: "rejected [redacted]".into(),
                cost: Some(Usd(500_000)),
                retryable: false,
            },
        ),
        // The answer's shape is held as a stand-in's: a refusal is billed, and may be retried.
        (
            json!({"outcome": "answered", "files": {}, "data": {"kite": true}, "cost_usd": 0.01}),
            Sent::Failed {
                reason: r#"structured.generate returns its data as {"json": <value>}"#.into(),
                cost: Some(Usd(10_000)),
                retryable: true,
            },
        ),
        // What FX cannot read ends the call as a lost host would.
        (
            json!({"outcome": "accepted", "handle": {}}),
            lost(&unreadable("accepted is not one of the values it takes")),
        ),
        (
            json!({"outcome": "refused", "reason": " "}),
            lost(&unreadable("its reason is blank")),
        ),
        (
            json!({"outcome": "failed", "reason": "x", "retryable": true, "cost_usd": -1}),
            lost(&unreadable("cost_usd is not a number of at least 0")),
        ),
        (
            json!({"outcome": "not_received", "reason": "x", "retry_after_s": -1}),
            lost(&unreadable(
                "retry_after_s is not a number of seconds of at least 0",
            )),
        ),
        (
            json!({"outcome": "answered", "files": {"image": {"base64": "not base64!"}},
                   "data": null}),
            lost(&unreadable(
                "files.image: its bytes are not base64 with padding",
            )),
        ),
    ];
    for (result, outcome) in cases {
        let mut rig = Rig::new(Arc::new(Offline), true);
        assert_eq!(sent(&mut rig, result.clone()).await, outcome, "{result}");
    }
}

#[tokio::test]
async fn an_error_answer_ends_the_call_as_a_lost_host_would() {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let task = sending(&rig.request("acme"), "acme");
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let send = peer.expect("adapter.send").await;
    peer.error(&send, -32099, &format!("ValueError: no {KEY}"))
        .await;
    assert_eq!(
        task.await.unwrap(),
        lost(&format!(
            "the adapter {NAME} failed: ValueError: no [redacted]"
        ))
    );
    // The host goes on serving: no new launch.
    let task = sending(&rig.request("acme"), "acme");
    let send = peer.expect("adapter.send").await;
    assert_eq!(send["params"]["call_id"], json!("c2"));
    peer.answer(&send, json!({"outcome": "refused", "reason": "no"}))
        .await;
    assert!(matches!(task.await.unwrap(), Sent::Refused { .. }));
    assert_eq!(rig.launches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn calls_are_pending_at_once_and_answered_in_any_order() {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let adapter = rig.request("acme");
    let first = sending(&adapter, "acme");
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let one = peer.expect("adapter.send").await;
    let second = sending(&adapter, "acme");
    let two = peer.expect("adapter.send").await;
    peer.answer(&two, json!({"outcome": "refused", "reason": "second"}))
        .await;
    assert_eq!(
        second.await.unwrap(),
        Sent::Refused {
            reason: "second".into()
        }
    );
    peer.answer(&one, json!({"outcome": "refused", "reason": "first"}))
        .await;
    assert_eq!(
        first.await.unwrap(),
        Sent::Refused {
            reason: "first".into()
        }
    );
    assert_eq!(rig.launches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_submit_and_a_collect_carry_the_handle() {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let job = rig.job();
    let submitting = {
        let job = Arc::clone(&job);
        tokio::spawn(async move { job.submit(&call("acme.render", "acme")).await })
    };
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let submit = peer.expect("adapter.submit").await;
    assert_eq!(submit["params"]["capability"], json!("acme.render"));
    assert!(submit["params"].get("handle").is_none());
    peer.answer(
        &submit,
        json!({"outcome": "accepted", "handle": {"job": "j-1"}}),
    )
    .await;
    assert_eq!(
        submitting.await.unwrap(),
        Submitted::Accepted {
            handle: json!({"job": "j-1"})
        }
    );
    let collecting = {
        let job = Arc::clone(&job);
        tokio::spawn(async move {
            job.collect(&call("acme.render", "acme"), &json!({"job": "j-1"}))
                .await
        })
    };
    let collect = peer.expect("adapter.collect").await;
    assert_eq!(collect["params"]["handle"], json!({"job": "j-1"}));
    assert_eq!(collect["params"]["call_id"], json!("c2"));
    peer.answer(
        &collect,
        json!({"outcome": "answered", "files": {"frame": {"base64": "AAE=", "kind": "image/png"}},
               "data": {"frames": 1}, "cost_usd": 1.25}),
    )
    .await;
    let mut answer = Answer::new(json!({"frames": 1}), Some(Usd(1_250_000)));
    answer.files.insert(
        "frame".into(),
        AnsweredFile {
            kind: "image/png".into(),
            bytes: vec![0, 1],
        },
    );
    assert_eq!(collecting.await.unwrap(), Collected::Answered(answer));
}

/// Answers one submit with `result`.
async fn submitted(result: Value) -> Submitted {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let job = rig.job();
    let task = tokio::spawn(async move { job.submit(&call("acme.render", "acme")).await });
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let submit = peer.expect("adapter.submit").await;
    peer.answer(&submit, result).await;
    task.await.unwrap()
}

/// Answers one collect with `result`.
async fn collected(result: Value) -> Collected {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let job = rig.job();
    let task = tokio::spawn(async move {
        job.collect(&call("acme.render", "acme"), &json!({"job": "j-1"}))
            .await
    });
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let collect = peer.expect("adapter.collect").await;
    peer.answer(&collect, result).await;
    task.await.unwrap()
}

#[tokio::test]
async fn each_job_outcome_maps_and_a_handle_holds_no_key() {
    assert_eq!(
        submitted(json!({"outcome": "uncertain", "reason": "timed out after sending"})).await,
        Submitted::Uncertain {
            reason: "timed out after sending".into()
        }
    );
    assert_eq!(
        submitted(json!({"outcome": "not_received", "reason": "busy"})).await,
        Submitted::NotReceived {
            reason: "busy".into(),
            retry_after: None
        }
    );
    assert_eq!(
        submitted(
            json!({"outcome": "failed", "reason": "no", "retryable": false,
                          "cost_usd": 0})
        )
        .await,
        Submitted::Failed {
            reason: "no".into(),
            cost: Some(Usd(0)),
            retryable: false
        }
    );
    // A handle FX cannot keep: one holding a key, or one that is not an object.
    assert_eq!(
        submitted(json!({"outcome": "accepted", "handle": {"token": KEY}})).await,
        Submitted::Uncertain {
            reason: format!(
                "the adapter {NAME} accepted the job with a handle that holds a key, which FX \
                 cannot keep"
            )
        }
    );
    assert!(matches!(
        submitted(json!({"outcome": "accepted", "handle": "j-1"})).await,
        Submitted::Uncertain { reason } if reason.starts_with(&unreadable(""))
    ));
    assert_eq!(
        collected(json!({"outcome": "ended", "reason": "the job failed", "cost_usd": 0.2})).await,
        Collected::Ended {
            reason: "the job failed".into(),
            cost: Some(Usd(200_000))
        }
    );
    assert_eq!(
        collected(json!({"outcome": "unreachable", "reason": "still running"})).await,
        Collected::Unreachable {
            reason: "still running".into()
        }
    );
    // A collected answer whose shape is refused ends the job, billed at its cost.
    assert_eq!(
        collected(
            json!({"outcome": "answered", "files": {"frame": {"base64": "AAE="}},
                          "data": null, "cost_usd": 0.3})
        )
        .await,
        Collected::Ended {
            reason: "frame has no kind, and acme.render names none for it".into(),
            cost: Some(Usd(300_000))
        }
    );
    assert!(matches!(
        collected(json!({"outcome": "uncertain", "reason": "x"})).await,
        Collected::Unreachable { reason } if reason.starts_with(&unreadable(""))
    ));
}

// ---------------------------------------------------------------------------------------------
// A lost host, a host that cannot serve

#[tokio::test]
async fn a_lost_host_ends_each_pending_call_as_its_outcome_demands() {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let adapter = rig.request("acme");
    let job = rig.job();
    let task = sending(&adapter, "acme");
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    peer.expect("adapter.send").await;
    let submitting = {
        let job = Arc::clone(&job);
        tokio::spawn(async move { job.submit(&call("acme.render", "acme")).await })
    };
    peer.expect("adapter.submit").await;
    let collecting = {
        let job = Arc::clone(&job);
        tokio::spawn(async move {
            job.collect(&call("acme.render", "acme"), &json!({"job": "j-1"}))
                .await
        })
    };
    peer.expect("adapter.collect").await;
    drop(peer);
    let reason = "the adapter host exited";
    assert_eq!(task.await.unwrap(), lost(reason));
    assert_eq!(
        submitting.await.unwrap(),
        Submitted::Uncertain {
            reason: reason.into()
        }
    );
    assert_eq!(
        collecting.await.unwrap(),
        Collected::Unreachable {
            reason: reason.into()
        }
    );
    // The next call starts a new host.
    let task = sending(&adapter, "acme");
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let send = peer.expect("adapter.send").await;
    peer.answer(&send, json!({"outcome": "refused", "reason": "again"}))
        .await;
    assert!(matches!(task.await.unwrap(), Sent::Refused { .. }));
    assert_eq!(rig.launches.load(Ordering::SeqCst), 2);
}

/// Loads the adapter on `peer`, which first stops reading: whatever the engine writes next
/// never reaches it. Its output, returned, stays open.
async fn deaf(mut peer: Peer, root: &Path) -> DuplexStream {
    let load = peer.expect("adapter.load").await;
    drop(peer.input);
    let mut output = peer.output;
    let body = serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": load["id"],
                                          "result": load_answer(root, Some(2))}))
    .unwrap();
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend(body);
    output.write_all(&frame).await.unwrap();
    output
}

#[tokio::test]
async fn a_call_never_written_to_its_host_is_not_received_and_sent_again_to_a_new_one() {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let adapter = rig.request("acme");
    let job = rig.job();
    let root = rig.root.clone();
    let reason = "the adapter host exited";
    // The host's connection ends before the send is written: it never reached the provider.
    let task = sending(&adapter, "acme");
    let output = deaf(rig.peer().await, &root).await;
    assert_eq!(task.await.unwrap(), Sent::not_received(reason));
    drop(output);
    // So does a submit's, on the next host.
    let submitting = {
        let job = Arc::clone(&job);
        tokio::spawn(async move { job.submit(&call("acme.render", "acme")).await })
    };
    let output = deaf(rig.peer().await, &root).await;
    assert_eq!(submitting.await.unwrap(), Submitted::not_received(reason));
    drop(output);
    // The engine sends it again: a new host gets it.
    let task = sending(&adapter, "acme");
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let send = peer.expect("adapter.send").await;
    peer.answer(&send, json!({"outcome": "refused", "reason": "again"}))
        .await;
    assert!(matches!(task.await.unwrap(), Sent::Refused { .. }));
    assert_eq!(rig.launches.load(Ordering::SeqCst), 3);
    let ending = tokio::spawn(async move { rig.shutdown().await });
    let shutdown = peer.expect("shutdown").await;
    peer.answer(&shutdown, Value::Null).await;
    peer.expect("exit").await;
    drop(peer);
    assert!(ending.await.unwrap());
}

#[tokio::test]
async fn a_host_that_cannot_serve_refuses_every_call_of_the_run() {
    // It changed since planning: another version.
    let mut rig = Rig::new(Arc::new(Offline), true);
    let adapter = rig.request("acme");
    let task = sending(&adapter, "acme");
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    let load = peer.expect("adapter.load").await;
    peer.answer(&load, load_answer(&root, Some(3))).await;
    drop(peer);
    let changed = format!("the adapter {NAME} changed since the run was planned");
    assert_eq!(
        task.await.unwrap(),
        Sent::Refused {
            reason: changed.clone()
        }
    );
    // Refused for the run: no new host, a collect unreachable.
    assert_eq!(
        adapter.send(&call("structured.generate", "acme")).await,
        Sent::Refused {
            reason: changed.clone()
        }
    );
    assert_eq!(
        rig.job()
            .collect(&call("acme.render", "acme"), &json!({"job": "j-1"}))
            .await,
        Collected::Unreachable { reason: changed }
    );
    assert_eq!(rig.launches.load(Ordering::SeqCst), 1);

    // Its source changed.
    let mut rig = Rig::new(Arc::new(Offline), true);
    let task = sending(&rig.request("acme"), "acme");
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    let load = peer.expect("adapter.load").await;
    std::fs::write(root.join("adapters/acme.py"), "acme = adapter(version=2)\n").unwrap();
    peer.answer(&load, load_answer(&root, Some(2))).await;
    drop(peer);
    assert_eq!(
        task.await.unwrap(),
        Sent::Refused {
            reason: format!("the adapter {NAME} changed since the run was planned")
        }
    );

    // It cannot be loaded.
    let mut rig = Rig::new(Arc::new(Offline), true);
    let task = sending(&rig.request("acme"), "acme");
    let mut peer = rig.peer().await;
    let load = peer.expect("adapter.load").await;
    peer.error(
        &load,
        -32004,
        "adapters/acme.py failed to import: SyntaxError",
    )
    .await;
    drop(peer);
    assert_eq!(
        task.await.unwrap(),
        Sent::Refused {
            reason: format!(
                "the adapter host for {NAME} could not be started: adapters/acme.py failed to \
                 import: SyntaxError"
            )
        }
    );
}

#[tokio::test]
async fn a_host_that_cannot_start_refuses_every_call() {
    let (_dir, root) = project();
    let declarations = declarations(&root);
    let environment = Environment::declared(&[("ACME_API_KEY", KEY)], &declarations);
    let setup = Setup::new(
        Arc::new(Offline),
        Keys::from_environment(&environment),
        Arc::new(FakeClock::new()),
    );
    let mut adapters = Adapters::new();
    adapter_host::register(&mut adapters, &setup, |_, _| {
        Arc::new(adapter_host::NoLaunch(
            "no @grida/fx for the TypeScript node host".into(),
        ))
    });
    let Some(Adapter::Request(adapter)) = adapters.serving(&route("structured.generate", "acme"))
    else {
        panic!("a request adapter");
    };
    assert_eq!(
        adapter.send(&call("structured.generate", "acme")).await,
        Sent::Refused {
            reason: format!(
                "the adapter host for {NAME} could not be started: no @grida/fx for the \
                 TypeScript node host"
            )
        }
    );
    // A host that never started ends verified.
    for runtime in adapters.runtimes() {
        assert!(runtime.shutdown().await);
    }
}

#[tokio::test]
async fn a_send_without_its_key_is_refused_before_the_host_is_asked() {
    let rig = Rig::new(Arc::new(Offline), false);
    assert_eq!(
        rig.request("acme")
            .send(&call("structured.generate", "acme"))
            .await,
        Sent::Refused {
            reason: "ACME_API_KEY is not set".into()
        }
    );
    assert_eq!(rig.launches.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------------------------
// Cancellation

#[tokio::test]
async fn a_dropped_call_is_cancelled_and_its_fetches_and_late_answer_discarded() {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let adapter = rig.request("acme");
    let task = sending(&adapter, "acme");
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let send = peer.expect("adapter.send").await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let cancel = peer.expect("$/cancel").await;
    assert_eq!(cancel["params"], json!({"id": send["id"]}));
    // A fetch for the dropped call is answered `cancelled`.
    let refused = peer
        .fetch(json!({"call_id": "c1", "method": "GET", "url": BASE, "lane": "provider"}))
        .await
        .unwrap_err();
    assert_eq!(refused["code"], json!(-32002));
    // The late answer is discarded; the host serves the next call.
    peer.error(&send, -32002, "cancelled").await;
    let task = sending(&adapter, "acme");
    let send = peer.expect("adapter.send").await;
    peer.answer(&send, json!({"outcome": "refused", "reason": "next"}))
        .await;
    assert_eq!(
        task.await.unwrap(),
        Sent::Refused {
            reason: "next".into()
        }
    );
}

#[tokio::test]
async fn a_call_dropped_while_the_adapter_loads_leaves_its_host_ended_and_counted() {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let task = sending(&rig.request("acme"), "acme");
    let mut peer = rig.peer().await;
    peer.expect("adapter.load").await;
    // The run stops before the host answers: the host is still ended, on a task of its own.
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let runtimes = rig.adapters.runtimes().to_vec();
    let cleanup = tokio::spawn(async move {
        let mut verified = true;
        for runtime in runtimes {
            verified &= runtime.shutdown().await;
        }
        verified
    });
    let ended = tokio::time::timeout(Duration::from_secs(10), async {
        let shutdown = peer.expect("shutdown").await;
        peer.answer(&shutdown, Value::Null).await;
        peer.expect("exit").await;
    });
    ended.await.expect("the abandoned host is ended");
    drop(peer);
    assert!(cleanup.await.unwrap(), "the cleanup waits for it, verified");
}

// ---------------------------------------------------------------------------------------------
// fetch

/// Runs `fetches` (params, without `call_id`) for one pending send on `rig`'s `provider`, and
/// answers the send with `result`; the fetch answers and the send's outcome.
async fn fetching(
    rig: &mut Rig,
    provider: &str,
    fetches: Vec<Value>,
    result: Value,
) -> (Vec<Result<Value, Value>>, Sent) {
    let task = sending(&rig.request(provider), provider);
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let send = peer.expect("adapter.send").await;
    let call_id = send["params"]["call_id"].clone();
    let mut answers = Vec::new();
    for mut params in fetches {
        params["call_id"] = call_id.clone();
        answers.push(peer.fetch(params).await);
    }
    peer.answer(&send, result).await;
    (answers, task.await.unwrap())
}

fn refused_with(reason: &str) -> Result<Value, Value> {
    Ok(json!({"error": {"phase": "refused", "reason": reason}}))
}

#[tokio::test]
async fn fetch_attaches_the_credential_only_to_the_base_urls_origin() {
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::post_json(format!("{BASE}/draw"), json!({"prompt": "a kite"}))
            .credential("authorization", &format!("Bearer {KEY}"))
            .header("accept", "application/json")
            .reply(
                HttpResponse::json(200, &json!({"id": "j-1"})).with_header("x-request-id", "r1"),
            ),
        Expect::get("https://api.acme.test/v1/jobs/j-1").reply(HttpResponse::new(202, "{}")),
        Expect::new(Method::Post, format!("{BASE}/files"), Lane::Upload)
            .credential("authorization", &format!("Bearer {KEY}"))
            .body(ExpectBody::Bytes(vec![0, 1]))
            .reply(HttpResponse::new(201, "")),
    ]));
    let mut rig = Rig::new(Arc::clone(&transport) as Arc<dyn Transport>, true);
    let (answers, outcome) = fetching(
        &mut rig,
        "acme",
        vec![
            json!({"method": "POST", "url": format!("{BASE}/draw"),
                   "headers": [["accept", "application/json"]],
                   "body": {"json": {"prompt": "a kite"}}, "lane": "provider",
                   "credential": true, "timeout_s": 30, "max_response_bytes": 1024}),
            json!({"method": "GET", "url": "https://api.acme.test/v1/jobs/j-1",
                   "lane": "provider"}),
            json!({"method": "POST", "url": format!("{BASE}/files"),
                   "body": {"base64": "AAE=", "content_type": "image/png"},
                   "lane": "upload", "credential": true}),
            // Another origin, a download with a credential, the credential's own header.
            json!({"method": "GET", "url": "https://other.acme.test/v1", "lane": "provider",
                   "credential": true}),
            json!({"method": "GET", "url": "http://api.acme.test/v1", "lane": "provider",
                   "credential": true}),
            json!({"method": "GET", "url": "https://files.acme.test/a.png", "lane": "download",
                   "credential": true}),
            json!({"method": "GET", "url": format!("{BASE}/x"), "lane": "provider",
                   "headers": [["authorization", "Bearer mine"]]}),
        ],
        json!({"outcome": "refused", "reason": "done"}),
    )
    .await;
    assert_eq!(
        answers[0],
        Ok(
            json!({"status": 200, "headers": [["content-type", "application/json"],
                                             ["x-request-id", "r1"]],
                  "base64": "eyJpZCI6ImotMSJ9"})
        )
    );
    assert_eq!(
        answers[1],
        Ok(json!({"status": 202, "headers": [], "base64": "e30="}))
    );
    assert_eq!(
        answers[2],
        Ok(json!({"status": 201, "headers": [], "base64": ""}))
    );
    let elsewhere = refused_with("acme's credential goes only to the origin of its base URL");
    assert_eq!(answers[3], elsewhere);
    assert_eq!(answers[4], elsewhere);
    assert_eq!(
        answers[5],
        refused_with("a download never carries a credential")
    );
    assert_eq!(
        answers[6],
        refused_with(
            "the authorization header is acme's credential, which the engine attaches when asked"
        )
    );
    assert!(matches!(outcome, Sent::Refused { .. }));
    transport.assert_done();
    let requests = transport.requests();
    assert_eq!(requests[0].timeout, Duration::from_secs(30));
    assert_eq!(requests[0].max_response_bytes, 1024);
    assert_eq!(requests[1].credential, None);
    assert_eq!(requests[1].timeout, Duration::from_secs(300));
}

#[tokio::test]
async fn fetch_holds_downloads_to_the_declarations_hosts_and_redacts_them() {
    let signed = "https://files.acme.test/out/a.png?signature=abc123";
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::download(signed).reply(HttpResponse::new(200, vec![7u8])),
    ]));
    let mut rig = Rig::new(Arc::clone(&transport) as Arc<dyn Transport>, true);
    let (answers, outcome) = fetching(
        &mut rig,
        "acme",
        vec![
            json!({"method": "GET", "url": signed, "lane": "download"}),
            json!({"method": "GET", "url": "https://cdn.other.test/a.png", "lane": "download"}),
        ],
        json!({"outcome": "failed", "reason": format!("could not decode {signed}"),
               "retryable": true}),
    )
    .await;
    assert_eq!(
        answers[0],
        Ok(json!({"status": 200, "headers": [], "base64": "Bw=="}))
    );
    assert_eq!(
        answers[1],
        refused_with("the download is not on acme's download hosts")
    );
    // The call's reasons never show a URL it downloaded.
    assert_eq!(
        outcome,
        Sent::Failed {
            reason: "could not decode [redacted]".into(),
            cost: None,
            retryable: true
        }
    );
    transport.assert_done();
}

#[tokio::test]
async fn a_reason_built_from_the_hosts_result_never_shows_a_url_the_call_downloaded() {
    // Signed in its path, which no pattern finds: only the call's redactor knows it.
    let signed = "https://files.acme.test/s/tok-9f3e1a/out.png";
    let rig = || {
        Rig::new(
            Arc::new(ReplayTransport::new(vec![
                Expect::download(signed).reply(HttpResponse::new(200, vec![7u8])),
            ])),
            true,
        )
    };
    let download = || vec![json!({"method": "GET", "url": signed, "lane": "download"})];
    let hidden = |reason: &str| !reason.contains("tok-9f3e1a") && reason.contains("[redacted]");
    // A result FX cannot read, which quotes it.
    let (_, outcome) = fetching(
        &mut rig(),
        "acme",
        download(),
        json!({"outcome": "failed", "reason": "x", "retryable": signed}),
    )
    .await;
    let Sent::Failed {
        reason,
        cost: None,
        retryable: false,
    } = &outcome
    else {
        panic!("{outcome:?}")
    };
    assert!(hidden(reason), "{reason}");
    // An answer whose shape is refused, which names it.
    let (_, outcome) = fetching(
        &mut rig(),
        "acme",
        download(),
        json!({"outcome": "answered", "files": {(signed): {"kind": "image/png", "base64": "Bw=="}},
               "data": {}}),
    )
    .await;
    let Sent::Failed {
        reason,
        retryable: true,
        ..
    } = &outcome
    else {
        panic!("{outcome:?}")
    };
    assert!(hidden(reason), "{reason}");
    // A submit's handle FX cannot read, which quotes it.
    let mut rig = rig();
    let job = rig.job();
    let submitting = {
        let job = Arc::clone(&job);
        tokio::spawn(async move { job.submit(&call("acme.render", "acme")).await })
    };
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let submit = peer.expect("adapter.submit").await;
    let mut params = download().remove(0);
    params["call_id"] = submit["params"]["call_id"].clone();
    assert!(peer.fetch(params).await.is_ok());
    peer.answer(&submit, json!({"outcome": "accepted", "handle": signed}))
        .await;
    let outcome = submitting.await.unwrap();
    let Submitted::Uncertain { reason } = &outcome else {
        panic!("{outcome:?}")
    };
    assert!(hidden(reason), "{reason}");
}

#[tokio::test]
async fn fetch_reports_each_failures_phase() {
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::get(format!("{BASE}/a")).fail(TransportError::not_sent(
            TransportErrorKind::Connect,
            "acme.test refused the connection",
        )),
        Expect::get(format!("{BASE}/b")).fail(TransportError::after_send(
            TransportErrorKind::Other,
            "the connection was reset",
        )),
    ]));
    let mut rig = Rig::new(Arc::clone(&transport) as Arc<dyn Transport>, true);
    let (answers, _) = fetching(
        &mut rig,
        "acme",
        vec![
            json!({"method": "GET", "url": format!("{BASE}/a"), "lane": "provider"}),
            json!({"method": "GET", "url": format!("{BASE}/b"), "lane": "provider"}),
        ],
        json!({"outcome": "refused", "reason": "done"}),
    )
    .await;
    assert_eq!(
        answers,
        [
            Ok(json!({"error": {"phase": "not_sent",
                                "reason": "acme.test refused the connection"}})),
            Ok(json!({"error": {"phase": "after_send", "reason": "the connection was reset"}})),
        ]
    );
    transport.assert_done();
}

#[tokio::test]
async fn fetch_with_the_network_off_is_refused_before_it_leaves() {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let (answers, _) = fetching(
        &mut rig,
        "acme",
        vec![json!({"method": "POST", "url": format!("{BASE}/draw"),
                    "body": {"json": {}}, "lane": "provider", "credential": true})],
        json!({"outcome": "refused", "reason": "done"}),
    )
    .await;
    assert_eq!(
        answers,
        [refused_with("the network is off (GRIDA_FX_NETWORK=off)")]
    );
}

#[tokio::test]
async fn fetch_for_a_keyless_declaration_sends_no_credential() {
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::get("http://127.0.0.1:8080/v1/models").reply(HttpResponse::new(200, "")),
    ]));
    let mut rig = Rig::new(Arc::clone(&transport) as Arc<dyn Transport>, true);
    let (answers, _) = fetching(
        &mut rig,
        "local",
        vec![
            json!({"method": "GET", "url": "http://127.0.0.1:8080/v1/models",
                    "lane": "provider", "credential": true}),
        ],
        json!({"outcome": "refused", "reason": "done"}),
    )
    .await;
    assert_eq!(
        answers,
        [Ok(json!({"status": 200, "headers": [], "base64": ""}))]
    );
    assert_eq!(transport.requests()[0].credential, None);
}

#[tokio::test]
async fn fetch_params_outside_the_contract_are_refused() {
    let mut rig = Rig::new(Arc::new(Offline), true);
    let task = sending(&rig.request("acme"), "acme");
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let send = peer.expect("adapter.send").await;
    for (params, message) in [
        (
            json!({"call_id": "c9", "method": "GET", "url": BASE, "lane": "provider"}),
            "no adapter call c9 is pending",
        ),
        (
            json!({"call_id": "c1", "method": "GET", "url": BASE, "lane": "provider",
                   "body": {"json": 1}}),
            "a GET has no body",
        ),
        (
            json!({"call_id": "c1", "method": "GET", "url": BASE, "lane": "provider",
                   "timeout_s": 301}),
            "timeout_s is above 0 and at most 300",
        ),
    ] {
        let error = peer.fetch(params).await.unwrap_err();
        assert_eq!(error["code"], json!(-32602));
        assert_eq!(error["message"], json!(message));
    }
    let error = peer
        .fetch(json!({"run_id": "r1", "method": "GET", "url": BASE, "lane": "provider"}))
        .await
        .unwrap_err();
    assert_eq!(error["code"], json!(-32602));
    peer.answer(&send, json!({"outcome": "refused", "reason": "done"}))
        .await;
    task.await.unwrap();
}

// ---------------------------------------------------------------------------------------------
// A host process

#[cfg(unix)]
mod process {
    use super::*;
    use grida_fx_core::host::{HostFailure, NodeHost};
    use grida_fx_protocol::{AdapterLoadParams, ErrorCode, RpcError};
    use grida_fx_providers::keys::Secret;
    use grida_fx_runtime::adapter_host::ProcessLaunch;
    use grida_fx_runtime::host::process::HostSpec;
    use grida_fx_runtime::host::{LanguageHost, NodeHosts};

    fn have_python3() -> bool {
        let found = std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success());
        if !found {
            eprintln!("skipped: python3 is not on PATH");
        }
        found
    }

    /// A fake adapter host: it answers `initialize` and `adapter.load` as planned, answers a
    /// send with what it sees of its environment, and exits with status 3 on a send whose
    /// prompt is `exit`. On one whose prompt is `close` it closes its output, then writes the
    /// file `closed`, and exits with status 4 once its input ends.
    const HOST: &str = r#"
import json, os, sys
def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        name, _, value = line.decode("ascii").partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(length))
def write(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
while True:
    message = read()
    if message is None or message.get("method") == "exit":
        sys.exit(0)
    method, params = message.get("method"), message.get("params")
    if method == "initialize":
        result = {"protocol": params["protocol"],
                  "host": {"language": "python", "version": "3", "sdk_version": "0"}}
    elif method == "adapter.load":
        result = {"adapter": {"version": 2, "capabilities": {
                      "structured.generate": "request", "acme.render": "job"}},
                  "closure": [{"label": "adapters/acme.py",
                               "path": os.path.join(os.getcwd(), "adapters/acme.py")}]}
    elif method == "adapter.send":
        if params["request"]["prompt"] == "exit":
            sys.exit(3)
        if params["request"]["prompt"] == "close":
            os.close(1)
            open("closed", "w").close()
            while sys.stdin.buffer.read(65536):
                pass
            os._exit(4)
        result = {"outcome": "answered", "files": {}, "data": {"json": {
            "key": os.environ.get("ACME_API_KEY"), "home": os.environ.get("HOME")}}}
    else:
        result = None
    write({"jsonrpc": "2.0", "id": message["id"], "result": result})
"#;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_host_process_gets_its_own_key_and_its_exit_names_the_lost_call() {
        if !have_python3() {
            return;
        }
        let (_dir, root) = project();
        std::fs::write(root.join("host.py"), HOST).unwrap();
        let program = root.join("python-fake");
        grida_fx_providers::testing::write_program(
            &program,
            "#!/bin/sh\nexec python3 \"$(dirname \"$0\")/host.py\"\n",
        );
        let declarations = declarations(&root);
        // The key reached FX from `.env` alone: the host gets it, and nothing FX withholds.
        let setup = Setup::new(
            Arc::new(Offline),
            Keys::from_environment(&Environment::declared(
                &[("ACME_API_KEY", KEY)],
                &declarations,
            )),
            Arc::new(FakeClock::new()),
        );
        let spec = HostSpec::python(&program, "python-fake", &root, Vec::new())
            .unchecked()
            .with_environment(
                vec!["HOME".into()],
                vec![("ACME_API_KEY".into(), Secret::new(KEY))],
            );
        assert!(!format!("{spec:?}").contains(KEY), "the key never prints");
        let launch: Arc<dyn Launch> = Arc::new(ProcessLaunch::new(Ok(spec)));
        let mut adapters = Adapters::new();
        adapter_host::register(&mut adapters, &setup, |_, _| Arc::clone(&launch));
        let Some(Adapter::Request(adapter)) =
            adapters.serving(&route("structured.generate", "acme"))
        else {
            panic!("a request adapter");
        };
        let adapter = Arc::clone(adapter);
        assert_eq!(
            adapter.send(&call("structured.generate", "acme")).await,
            Sent::Answered(Answer::new(
                json!({"json": {"key": KEY, "home": null}}),
                None
            ))
        );
        let mut exit = call("structured.generate", "acme");
        exit.request = json!({"prompt": "exit"});
        assert_eq!(
            adapter.send(&exit).await,
            lost("the adapter host exited with status 3")
        );
        // The next call starts a new host.
        assert!(matches!(
            adapter.send(&call("structured.generate", "acme")).await,
            Sent::Answered(_)
        ));
        for runtime in adapters.runtimes() {
            assert!(runtime.shutdown().await, "the host ends verified");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_call_lost_with_a_host_the_next_call_ends_learns_its_exit_at_once() {
        if !have_python3() {
            return;
        }
        let (_dir, root) = project();
        std::fs::write(root.join("host.py"), HOST).unwrap();
        let program = root.join("python-fake");
        grida_fx_providers::testing::write_program(
            &program,
            "#!/bin/sh\nexec python3 \"$(dirname \"$0\")/host.py\"\n",
        );
        let declarations = declarations(&root);
        let setup = Setup::new(
            Arc::new(Offline),
            Keys::from_environment(&Environment::declared(
                &[("ACME_API_KEY", KEY)],
                &declarations,
            )),
            Arc::new(FakeClock::new()),
        );
        let spec = HostSpec::python(&program, "python-fake", &root, Vec::new()).unchecked();
        let launch: Arc<dyn Launch> = Arc::new(ProcessLaunch::new(Ok(spec)));
        let mut adapters = Adapters::new();
        adapter_host::register(&mut adapters, &setup, |_, _| Arc::clone(&launch));
        let Some(Adapter::Request(adapter)) =
            adapters.serving(&route("structured.generate", "acme"))
        else {
            panic!("a request adapter");
        };
        let adapter = Arc::clone(adapter);
        // The host closes its output but runs on: the call is lost, and waits to learn how the
        // host ended, which its watcher would see only after the grace period.
        let mut close = call("structured.generate", "acme");
        close.request = json!({"prompt": "close"});
        let lost_call = {
            let adapter = Arc::clone(&adapter);
            tokio::spawn(async move { adapter.send(&close).await })
        };
        let marked = tokio::time::timeout(Duration::from_secs(10), async {
            while !root.join("closed").exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        marked.await.expect("the host closed its output");
        tokio::time::sleep(Duration::from_millis(300)).await;
        // The next call ends that host and starts another; the lost call learns the end at once.
        let started = std::time::Instant::now();
        assert!(matches!(
            adapter.send(&call("structured.generate", "acme")).await,
            Sent::Answered(_)
        ));
        assert_eq!(
            lost_call.await.unwrap(),
            lost("the adapter host exited with status 4")
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "not after the grace period"
        );
        for runtime in adapters.runtimes() {
            assert!(runtime.shutdown().await, "the hosts end verified");
        }
    }

    /// A fake Python host of `grida` 0.6.1: `adapter.load` of `old.py` is a method it does not
    /// know, of `bad.py` an answer FX cannot read, of anything else a versioned adapter.
    const ADAPTER_HOST: &str = r#"
import json, sys
def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        name, _, value = line.decode("ascii").partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(length))
def write(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
while True:
    message = read()
    if message is None or message.get("method") == "exit":
        sys.exit(0)
    method, params = message.get("method"), message.get("params") or {}
    answer = {"jsonrpc": "2.0", "id": message["id"]}
    if method == "initialize":
        answer["result"] = {"protocol": params["protocol"],
                            "host": {"language": "python", "version": "3", "sdk_version": "0.6.1"}}
    elif method == "adapter.load" and params["path"] == "old.py":
        answer["error"] = {"code": -32601, "message": "unknown method adapter.load"}
    elif method == "adapter.load" and params["path"] == "bad.py":
        answer["result"] = {"adapter": 1}
    elif method == "adapter.load":
        answer["result"] = {"adapter": {"version": 1, "capabilities": {"image.generate": "request"}},
                            "closure": []}
    else:
        answer["result"] = None
    write(answer)
"#;

    #[test]
    fn the_planning_host_loads_an_adapter_and_names_an_sdk_that_predates_them() {
        if !have_python3() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("host.py"), ADAPTER_HOST).unwrap();
        let program = dir.path().join("python-fake");
        grida_fx_providers::testing::write_program(
            &program,
            "#!/bin/sh\nexec python3 \"$(dirname \"$0\")/host.py\"\n",
        );
        let mut hosts = NodeHosts::of(
            LanguageHost::python().with_program(program),
            LanguageHost::typescript().with_program(PathBuf::from("no-such-node-for-fx")),
        );
        hosts.open_project(dir.path(), &[]).unwrap();
        let load = |path: &str| AdapterLoadParams {
            path: path.into(),
            attribute: "acme".into(),
        };
        let loaded = hosts.load_adapter(&load("adapters/acme.py")).unwrap();
        assert_eq!(loaded.adapter.version, Some(1));
        assert_eq!(
            hosts.load_adapter(&load("old.py")).unwrap_err(),
            HostFailure::Rpc(RpcError::new(
                ErrorCode::LoadFailed,
                "the project's grida 0.6.1 cannot load adapters: upgrade grida"
            ))
        );
        let unreadable = hosts.load_adapter(&load("bad.py")).unwrap_err();
        assert!(
            matches!(&unreadable, HostFailure::Unavailable(m)
                if m.starts_with("the node host answered adapter.load with something FX cannot read: ")),
            "{unreadable:?}"
        );
        // The host goes on serving after an answer it could not read.
        assert!(hosts.load_adapter(&load("adapters/acme.py")).is_ok());
        // A module of no language never reaches a host.
        assert_eq!(
            hosts.load_adapter(&load("adapters/acme.rb")).unwrap_err(),
            HostFailure::Rpc(RpcError::new(
                ErrorCode::LoadFailed,
                "adapters/acme.rb: no node host for .rb modules yet"
            ))
        );
        hosts.shutdown();
    }
}

#[tokio::test]
async fn the_engine_checks_the_contract_and_the_request_before_the_host_is_asked() {
    let rig = Rig::new(Arc::new(Offline), true);
    let Adapter::Request(checked) =
        grida_fx_runtime::calls::checked::checked(Adapter::Request(rig.request("acme")))
    else {
        panic!("a request adapter");
    };
    let mut other = call("structured.generate", "acme");
    other.route.contract = json!({"adapter": NAME, "adapter_behavior": 3});
    assert_eq!(
        checked.send(&other).await,
        Sent::Refused {
            reason: "model-a@acme is not a structured.generate route this adapter serves".into()
        }
    );
    let Sent::Refused { reason } = checked.send(&call("structured.generate", "acme")).await else {
        panic!("the request is refused");
    };
    assert!(!reason.contains("adapter"), "{reason}");
    assert_eq!(rig.launches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_collect_without_its_key_reaches_the_host_and_its_credential_is_refused() {
    let mut rig = Rig::new(Arc::new(Offline), false);
    let job = rig.job();
    let task = tokio::spawn(async move {
        job.collect(&call("acme.render", "acme"), &json!({"job": "j-1"}))
            .await
    });
    let root = rig.root.clone();
    let mut peer = rig.peer().await;
    peer.loads(&root).await;
    let collect = peer.expect("adapter.collect").await;
    let answer = peer
        .fetch(
            json!({"call_id": "c1", "method": "GET", "url": format!("{BASE}/jobs/j-1"),
                      "lane": "provider", "credential": true}),
        )
        .await;
    assert_eq!(answer, refused_with("ACME_API_KEY is not set"));
    peer.answer(
        &collect,
        json!({"outcome": "unreachable", "reason": "no key"}),
    )
    .await;
    assert_eq!(
        task.await.unwrap(),
        Collected::Unreachable {
            reason: "no key".into()
        }
    );
}
