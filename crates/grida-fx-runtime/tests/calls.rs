//! The call path (spec/protocol.md §6.1), the paid built-ins' requests and a run's host requests
//! (spec/protocol.md §6).
//!
//! Most tests drive the call path, `request_of` and the run handler alone; the end-to-end ones
//! go through the real store, ledger, retry owner, staging and event log.

use grida_fx_core::builtins::builtin;
use grida_fx_core::money::Usd;
use grida_fx_core::routes::{PriceUnit, Route, RoutePrice};
use grida_fx_core::spec::{CallBound, NodeSpec, Port};
use grida_fx_core::val::{FileContent, FileValue, Val};
use grida_fx_core::value::{digest, file_digest};
use grida_fx_protocol::{ErrorCode, RetryMode, RpcError};
use grida_fx_providers::fake::{FakeAdapter, FakeCall};
use grida_fx_providers::{Adapters, Answer, AnsweredFile, Collected, Sent, Submitted};
use grida_fx_runtime::calls::pacing::Pacing;
use grida_fx_runtime::calls::{CallCounter, CallError, CallSite, call};
use grida_fx_runtime::engine::{Cancel, Engine, RunFiles, Services};
use grida_fx_runtime::events::{EventLog, read_events};
use grida_fx_runtime::executor::capability_node::{request_of, run as run_node};
use grida_fx_runtime::executor::requests::RunHandler;
use grida_fx_runtime::executor::{InstanceJob, JobBody};
use grida_fx_runtime::host::connection::{Connection, NoIncoming};
use grida_fx_runtime::host::pool::RunRequests;
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::ledger::Ledger;
use grida_fx_runtime::store::Store;
use grida_fx_runtime::store::records::{
    CallRecord, FileEntry, JobRecord, JobState, RouteEntry, call_key,
};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// The cache-replay case's route fingerprint and call key (conformance/cache-replay/README.md).
const IMG_A_FINGERPRINT: &str = "4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4";
const LANTERN_KEY: &str = "3aa41bf6e466138e920882b87c9b7ef9fc22dc245a2861c121f80adaaafd066d";

fn route(capability: &str, model: &str, provider: &str, high_micros: i64) -> Route {
    Route {
        capability: capability.into(),
        model: model.into(),
        provider: provider.into(),
        price: RoutePrice {
            low: Usd(high_micros / 4),
            high: Usd(high_micros),
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

fn img_a() -> Route {
    route("image.generate", "img-a", "acme", 40_000)
}

fn site(routes: Vec<Route>, calls: &[(&str, u32)]) -> CallSite {
    CallSite {
        instance_id: "draw#1".into(),
        step: "draw".into(),
        type_name: "image.generate".into(),
        takes: vec![1],
        calls: calls.iter().map(|(c, n)| (c.to_string(), *n)).collect(),
        routes: routes
            .into_iter()
            .map(|r| (r.capability.clone(), r))
            .collect(),
        limits: IndexMap::new(),
        scopes: Vec::new(),
    }
}

fn engine(root: &Path, adapters: Adapters, live: bool) -> Arc<Engine> {
    Arc::new(Engine::new(
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
    ))
}

/// A live run's services without an event log.
fn live(engine: Arc<Engine>, ceiling: Option<Usd>) -> Arc<Services> {
    live_with(engine, Arc::new(Ledger::new(ceiling, None)), None)
}

fn live_with(
    engine: Arc<Engine>,
    ledger: Arc<Ledger>,
    events: Option<Arc<EventLog>>,
) -> Arc<Services> {
    Arc::new(Services {
        engine,
        ledger: Some(ledger),
        events,
        pacing: Arc::new(Pacing::new()),
        cancel: Cancel::new(),
        invocation_id: "inv".into(),
        runs: AtomicU64::new(0),
        holds: AtomicU64::new(0),
    })
}

fn rpc(result: Result<impl std::fmt::Debug, CallError>) -> RpcError {
    match result {
        Err(CallError::Rpc(error)) => error,
        other => panic!("expected an RPC error, got {other:?}"),
    }
}

fn file(bytes: &[u8], kind: &str, name: &str) -> FileValue {
    FileValue {
        digest: file_digest(bytes),
        kind: kind.into(),
        name: name.into(),
        size: bytes.len() as u64,
        key: None,
        content: None,
        location: None,
    }
}

/// A job of a built-in, as `InstanceJob::from_instance` would make it.
fn builtin_job(name: &str, with: Vec<(&str, Val)>, routes: Vec<Route>) -> InstanceJob {
    let builtin = builtin(name, 1).expect("a built-in");
    let capability = builtin.spec.capability.clone().expect("a paid built-in");
    InstanceJob {
        id: "draw#1".into(),
        path: "draw".into(),
        step: "draw".into(),
        key: None,
        takes: vec![1],
        uses: builtin.uses.clone(),
        type_identity: builtin.identity(),
        spec: Arc::new(builtin.spec.clone()),
        body: JobBody::Capability {
            capability: capability.clone(),
        },
        with: with.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        identity: None,
        read: BTreeMap::new(),
        calls: IndexMap::from([(capability, 1)]),
        routes: routes
            .into_iter()
            .map(|r| (r.capability.clone(), r))
            .collect(),
        limits: IndexMap::new(),
        scopes: Vec::new(),
        resources: IndexMap::new(),
        tools: IndexMap::new(),
        timeout_s: None,
        retry: RetryMode::Service,
        picked: None,
    }
}

fn lantern_job() -> InstanceJob {
    builtin_job(
        "image.generate",
        vec![
            ("prompt", Val::Str("a lantern".into())),
            ("background", Val::Str("auto".into())),
            ("vars", Val::Object(IndexMap::new())),
        ],
        vec![img_a()],
    )
}

// ---------------------------------------------------------------------------------------------
// The canonical request of a paid built-in.

#[test]
fn the_canonical_request_of_cache_replay() {
    let job = lantern_job();
    let request = request_of(&job).unwrap();
    assert_eq!(
        request,
        json!({"prompt": "a lantern", "background": "auto"})
    );
    assert_eq!(img_a().fingerprint(), IMG_A_FINGERPRINT);
    let key = digest(&json!({
        "kind": "fx-call-v1",
        "capability": "image.generate",
        "route": IMG_A_FINGERPRINT,
        "request": request,
        "take": [1]
    }));
    assert_eq!(key, LANTERN_KEY);
}

#[test]
fn a_text_file_param_is_its_content() {
    // spec/identity.md §14 "A call with a text file param".
    let bytes = b"\xEF\xBB\xBFHello.\r\n";
    let mut text = file(bytes, "text/plain", "line.txt");
    text.content = Some(FileContent::Text("Hello.\r\n".into()));
    let voice = route("speech.generate", "voice-a", "acme", 10_000);
    let job = builtin_job(
        "speech.generate",
        vec![
            ("text", Val::File(Box::new(text))),
            ("voice", Val::Str("narrator-a".into())),
        ],
        vec![voice.clone()],
    );
    let request = request_of(&job).unwrap();
    assert_eq!(
        request,
        json!({"text": "Hello.\r\n", "voice": "narrator-a"})
    );
    let key = digest(&json!({
        "kind": "fx-call-v1",
        "capability": "speech.generate",
        "route": voice.fingerprint(),
        "request": request,
        "take": [1]
    }));
    assert_eq!(
        key,
        "a789cf0e3723acd1caa7e89f4049e93fb445916b48c52676fdbdc26981c638d8"
    );
}

#[test]
fn inputs_are_files_on_top_of_the_params() {
    let image = file(b"img", "image/png", "a.png");
    let reference = file(b"ref", "image/png", "r.png");
    let job = builtin_job(
        "image.edit",
        vec![
            ("image", Val::File(Box::new(image.clone()))),
            ("mask", Val::Missing),
            (
                "references",
                Val::List(vec![Val::File(Box::new(reference.clone()))]),
            ),
            ("prompt", Val::Str("Make it blue.".into())),
            ("background", Val::Str("auto".into())),
            ("vars", Val::Object(IndexMap::new())),
        ],
        vec![route("image.edit", "img-a", "acme", 50_000)],
    );
    let request = request_of(&job).unwrap();
    assert_eq!(
        request,
        json!({
            "prompt": "Make it blue.",
            "background": "auto",
            "image": {"file": image.digest},
            "references": [{"file": reference.digest}]
        })
    );
    // Params first, then inputs.
    let names: Vec<_> = request.as_object().unwrap().keys().cloned().collect();
    assert_eq!(names, ["prompt", "background", "image", "references"]);
}

#[test]
fn a_keyed_input_is_an_object_by_key() {
    let front = file(b"front", "image/png", "front.png");
    let mut views = IndexMap::new();
    views.insert("front".to_string(), Val::File(Box::new(front.clone())));
    let job = builtin_job(
        "mesh.generate",
        vec![
            ("views", Val::Object(views)),
            ("quad", Val::Bool(false)),
            ("texture", Val::Bool(true)),
            ("pbr", Val::Bool(false)),
        ],
        vec![route("mesh.generate", "mesh-a", "acme", 300_000)],
    );
    assert_eq!(
        request_of(&job).unwrap(),
        json!({
            "quad": false, "texture": true, "pbr": false,
            "views": {"front": {"file": front.digest}}
        })
    );
}

#[test]
fn a_value_no_call_can_send_is_refused() {
    let job = builtin_job(
        "image.generate",
        vec![
            ("prompt", Val::Failed("brief#1".into())),
            ("background", Val::Str("auto".into())),
        ],
        vec![img_a()],
    );
    assert_eq!(
        request_of(&job).unwrap_err(),
        "prompt holds a failed result, which a paid call cannot send"
    );
}

// ---------------------------------------------------------------------------------------------
// The call path's refusals, before anything is stored or sent.

#[tokio::test]
async fn an_undeclared_capability_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let services = Services::planning(engine(dir.path(), Adapters::new(), false));
    let site = site(vec![img_a()], &[("image.generate", 1)]);
    let error = rpc(call(
        &services,
        &site,
        &CallCounter::new(),
        &RunFiles::new(),
        "image.edit",
        json!({"prompt": "p"}),
    )
    .await);
    assert_eq!(error.code, ErrorCode::CapabilityUndeclared.code());
    assert_eq!(
        error.message,
        "image.generate calls image.edit without declaring it"
    );
    assert_eq!(error.data, Some(json!({"capability": "image.edit"})));
}

#[tokio::test]
async fn a_call_that_is_not_live_still_counts_toward_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    let services = Services::planning(engine(dir.path(), Adapters::new(), true));
    let site = site(vec![img_a()], &[("image.generate", 1)]);
    let counter = CallCounter::new();
    let files = RunFiles::new();
    let request = json!({"prompt": "a lantern", "background": "auto"});
    let error = rpc(call(
        &services,
        &site,
        &counter,
        &files,
        "image.generate",
        request.clone(),
    )
    .await);
    assert_eq!(error.code, ErrorCode::NotLive.code());
    assert_eq!(
        error.message,
        "image.generate on img-a@acme is a paid call; run with --live"
    );
    let data = error.data.unwrap();
    assert_eq!(data["capability"], json!("image.generate"));
    assert_eq!(data["route"], json!("img-a@acme"));
    assert!(data.get("key").is_some());
    let error = rpc(call(
        &services,
        &site,
        &counter,
        &files,
        "image.generate",
        request,
    )
    .await);
    assert_eq!(error.code, ErrorCode::OverBound.code());
    assert_eq!(
        error.message,
        "image.generate declared at most 1 image.generate calls"
    );
    assert_eq!(counter.cost(), None);
}

#[tokio::test]
async fn a_capability_without_a_route_is_no_route() {
    let dir = tempfile::tempdir().unwrap();
    let services = Services::planning(engine(dir.path(), Adapters::new(), false));
    let site = site(vec![], &[("image.generate", 2)]);
    let error = rpc(call(
        &services,
        &site,
        &CallCounter::new(),
        &RunFiles::new(),
        "image.generate",
        json!({}),
    )
    .await);
    assert_eq!(error.code, ErrorCode::NoRoute.code());
    assert_eq!(error.message, "no route serves image.generate for draw");
}

#[tokio::test]
async fn requests_hold_only_ordinary_values_and_handed_files() {
    let dir = tempfile::tempdir().unwrap();
    let services = Services::planning(engine(dir.path(), Adapters::new(), false));
    let site = site(vec![img_a()], &[("image.generate", 10)]);
    let counter = CallCounter::new();
    let files = RunFiles::new();
    let handed = file(b"x", "image/png", "x.png");
    files.insert(&handed);

    let error = rpc(call(
        &services,
        &site,
        &counter,
        &files,
        "image.generate",
        json!({"prompt": {"missing": true}}),
    )
    .await);
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(
        error.message,
        "request.prompt: an object holding only `missing` in this shape is reserved for FX's own values"
    );
    assert_eq!(
        error.data,
        Some(json!({"capability": "image.generate", "route": "img-a@acme"}))
    );

    let stranger = file_digest(b"never handed");
    let error = rpc(call(
        &services,
        &site,
        &counter,
        &files,
        "image.generate",
        json!({"references": [{"file": handed.digest}, {"file": stranger}]}),
    )
    .await);
    assert_eq!(error.code, ErrorCode::UnknownFile.code());
    assert_eq!(
        error.message,
        format!("request.references[1] names the file {stranger}, which this run was not handed")
    );

    let error = rpc(call(
        &services,
        &site,
        &counter,
        &files,
        "image.generate",
        json!([1]),
    )
    .await);
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(error.message, "a capability request is a JSON object");

    // A handed file passes the request checks; planning then refuses the paid call.
    let error = rpc(call(
        &services,
        &site,
        &counter,
        &files,
        "image.generate",
        json!({"references": [{"file": handed.digest}], "ordinary": {"file": "a.png"}}),
    )
    .await);
    assert_eq!(error.code, ErrorCode::NotLive.code());
}

#[tokio::test]
async fn no_adapter_is_checked_after_the_live_check() {
    let dir = tempfile::tempdir().unwrap();
    let services = live(
        engine(dir.path(), Adapters::new(), true),
        Some(Usd(1_000_000)),
    );
    let site = site(vec![img_a()], &[("image.generate", 1)]);
    let error = rpc(call(
        &services,
        &site,
        &CallCounter::new(),
        &RunFiles::new(),
        "image.generate",
        json!({"prompt": "a lantern"}),
    )
    .await);
    assert_eq!(error.code, ErrorCode::NoRoute.code());
    assert_eq!(
        error.message,
        "no adapter serves image.generate on img-a@acme"
    );
}

#[tokio::test]
async fn an_engine_that_is_not_live_makes_no_paid_call() {
    let dir = tempfile::tempdir().unwrap();
    let services = live(engine(dir.path(), Adapters::new(), false), None);
    let site = site(vec![img_a()], &[("image.generate", 1)]);
    let error = rpc(call(
        &services,
        &site,
        &CallCounter::new(),
        &RunFiles::new(),
        "image.generate",
        json!({"prompt": "a lantern"}),
    )
    .await);
    assert_eq!(error.code, ErrorCode::NotLive.code());
}

// ---------------------------------------------------------------------------------------------
// A run's host requests.

/// A project type that calls `structured.generate` once and reads `prompts/a.md`.
fn caption_job(resource: &Path) -> InstanceJob {
    let spec = NodeSpec {
        name: "caption".into(),
        description: None,
        inputs: IndexMap::from([("image".to_string(), Port::parse("image").unwrap())]),
        params: IndexMap::from([
            ("question".to_string(), json!({"type": "string"})),
            ("n".to_string(), json!({"type": "integer"})),
        ]),
        outputs: IndexMap::from([("caption".to_string(), Port::parse("text").unwrap())]),
        judge: false,
        capability: None,
        calls: IndexMap::from([("structured.generate".to_string(), CallBound::Count(1))]),
        resources: vec!["prompts/a.md".into()],
        tools: Vec::new(),
        view: None,
        version: Some(1),
        retry: RetryMode::Service,
    };
    InstanceJob {
        id: "caption#1".into(),
        path: "caption".into(),
        step: "caption".into(),
        key: None,
        takes: vec![1],
        uses: "./nodes/caption.py#caption".into(),
        type_identity: "nodes/caption.py#caption@1".into(),
        spec: Arc::new(spec),
        body: JobBody::Project {
            path: "nodes/caption.py".into(),
            attribute: "caption".into(),
        },
        with: IndexMap::from([
            (
                "image".to_string(),
                Val::File(Box::new(file(b"img", "image/png", "a.png"))),
            ),
            ("question".to_string(), Val::Str("What is it?".into())),
            ("n".to_string(), Val::Number(3.0)),
        ]),
        identity: None,
        read: BTreeMap::new(),
        calls: IndexMap::from([("structured.generate".to_string(), 1)]),
        routes: IndexMap::from([(
            "structured.generate".to_string(),
            route("structured.generate", "llm-a", "acme", 2_000),
        )]),
        limits: IndexMap::new(),
        scopes: Vec::new(),
        resources: IndexMap::from([("prompts/a.md".to_string(), resource.to_path_buf())]),
        tools: IndexMap::new(),
        timeout_s: None,
        retry: RetryMode::Service,
        picked: None,
    }
}

struct Run {
    _dir: tempfile::TempDir,
    handler: Arc<RunHandler>,
    files: Arc<RunFiles>,
    work: PathBuf,
}

fn run_with(services: impl FnOnce(Arc<Engine>) -> Arc<Services>) -> Run {
    run_for_prompt(None, services)
}

fn run_for(job: InstanceJob, services: impl FnOnce(Arc<Engine>) -> Arc<Services>) -> Run {
    run_for_prompt(Some(job), services)
}

/// A run of `job` (else the caption job, whose `prompts/a.md` is written here).
fn run_for_prompt(
    job: Option<InstanceJob>,
    services: impl FnOnce(Arc<Engine>) -> Arc<Services>,
) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let prompts = dir.path().join("prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    std::fs::write(
        prompts.join("a.md"),
        "\u{FEFF}<!-- a note -->\n${{ question }} (${{ n }}) about ${{ topic }}\r\n",
    )
    .unwrap();
    let work = dir.path().join(".fx/cache/work/inv-1");
    std::fs::create_dir_all(&work).unwrap();
    let services = services(engine(dir.path(), Adapters::new(), false));
    let files = Arc::new(RunFiles::new());
    let connection = Connection::start(tokio::io::empty(), tokio::io::sink(), Arc::new(NoIncoming));
    let handler = Arc::new(RunHandler::new(
        services,
        Arc::new(job.unwrap_or_else(|| caption_job(&prompts.join("a.md")))),
        "inv-1".into(),
        work.clone(),
        Arc::clone(&files),
        connection,
        Cancel::new(),
    ));
    Run {
        _dir: dir,
        handler,
        files,
        work,
    }
}

fn planning_run() -> Run {
    run_with(|engine| Arc::new(Services::planning(engine)))
}

async fn ask(handler: &Arc<RunHandler>, method: &str, params: Value) -> Result<Value, RpcError> {
    Arc::clone(handler)
        .request(method.to_string(), params)
        .await
}

#[tokio::test]
async fn facts_are_recorded_in_order_and_cost_usd_is_the_engines() {
    let run = planning_run();
    let h = &run.handler;
    assert_eq!(
        ask(
            h,
            "fact",
            json!({"run_id": "inv-1", "name": "score", "value": 0.5})
        )
        .await,
        Ok(json!({}))
    );
    assert_eq!(
        ask(
            h,
            "fact",
            json!({"run_id": "inv-1", "name": "words", "value": [1, "a"]})
        )
        .await,
        Ok(json!({}))
    );
    assert_eq!(
        ask(
            h,
            "fact",
            json!({"run_id": "inv-1", "name": "score", "value": 0.75})
        )
        .await,
        Ok(json!({}))
    );
    let error = ask(
        h,
        "fact",
        json!({"run_id": "inv-1", "name": "cost_usd", "value": 1}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(
        error.message,
        "cost_usd is the engine's own fact; name yours otherwise"
    );
    let error = ask(
        h,
        "fact",
        json!({"run_id": "inv-1", "name": "x", "value": {"a": {"failed": "b#1"}}}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(
        error.message,
        "fact x.a: an object holding only `failed` in this shape is reserved for FX's own values"
    );
    let error = ask(h, "fact", json!({"run_id": "inv-1", "value": 1}))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    let facts = h.facts();
    assert_eq!(
        facts.into_iter().collect::<Vec<_>>(),
        vec![
            ("score".to_string(), json!(0.75)),
            ("words".to_string(), json!([1, "a"])),
        ]
    );
}

#[tokio::test]
async fn marks_are_checked_and_kept() {
    let run = planning_run();
    let h = &run.handler;
    let good = json!({"shape": "box", "box": [0.1, 0.1, 0.5, 0.6], "label": "cat", "score": 0.9});
    assert_eq!(
        ask(h, "annotate", json!({"run_id": "inv-1", "mark": good})).await,
        Ok(json!({}))
    );
    assert_eq!(
        ask(
            h,
            "annotate",
            json!({"run_id": "inv-1", "mark": {"label": "whole"}})
        )
        .await,
        Ok(json!({}))
    );
    for (mark, message) in [
        (
            json!({"shape": "point"}),
            Some("a point mark needs at: [x, y]"),
        ),
        (
            json!({"shape": "points", "points": [[0, 0]]}),
            Some("a points mark needs points: at least 2 [x, y] pairs"),
        ),
        (
            json!({"shape": "point", "at": [0.5, 2]}),
            Some("a mark's at lies in fractions of the image from 0 to 1, not 2"),
        ),
        (json!({"shape": "box", "box": [0, 0, 1]}), None),
        (json!({"shape": "circle"}), None),
        (json!({"label": 3}), None),
        (
            json!({"label": "x", "extra": {"collection": []}}),
            Some(
                "mark.extra: an object holding only `collection` in this shape is reserved for FX's own values",
            ),
        ),
    ] {
        let error = ask(h, "annotate", json!({"run_id": "inv-1", "mark": mark}))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams.code(), "{mark}");
        if let Some(message) = message {
            assert_eq!(error.message, message, "{mark}");
        }
    }
    let marks = h.marks();
    assert_eq!(marks.len(), 2);
    assert_eq!(serde_json::to_value(&marks[0]).unwrap(), good);
}

#[tokio::test]
async fn requests_name_this_run_and_end_with_it() {
    let run = planning_run();
    let h = &run.handler;
    let error = ask(
        h,
        "fact",
        json!({"run_id": "other", "name": "a", "value": 1}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(error.message, "no run other is pending on this host");
    let error = ask(h, "describe", json!({"run_id": "inv-1"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::MethodNotFound.code());
    // A notification of this run is shown; of another, ignored. Neither panics.
    h.notify(
        "progress".into(),
        json!({"run_id": "inv-1", "text": "half", "fraction": 0.5}),
    );
    h.notify("progress".into(), json!({"run_id": "other", "text": "x"}));
    h.close();
    let error = ask(
        h,
        "fact",
        json!({"run_id": "inv-1", "name": "a", "value": 1}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled.code());
    assert!(h.facts().is_empty());
}

#[tokio::test]
async fn a_cancelled_run_answers_cancelled() {
    let run = planning_run();
    run.handler.cancel.cancel();
    let error = ask(
        &run.handler,
        "prompt.render",
        json!({"run_id": "inv-1", "path": "prompts/a.md", "variables": {}}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled.code());
}

#[tokio::test]
async fn a_run_its_lease_stopped_answers_cancelled() {
    // What the lease does at a step's deadline, before it sends `$/cancel`.
    let run = planning_run();
    run.handler.stop();
    assert!(run.handler.cancel.is_cancelled());
    let error = ask(
        &run.handler,
        "fact",
        json!({"run_id": "inv-1", "name": "late", "value": 1}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled.code());
    assert!(run.handler.facts().is_empty());
}

#[tokio::test]
async fn prompts_render_over_the_params_with_variables_on_top() {
    let run = planning_run();
    let h = &run.handler;
    let rendered = ask(
        h,
        "prompt.render",
        json!({"run_id": "inv-1", "path": "prompts/a.md", "variables": {"topic": "cats", "n": 4}}),
    )
    .await
    .unwrap();
    assert_eq!(rendered, json!({"text": "What is it? (4) about cats\r\n"}));

    let error = ask(
        h,
        "prompt.render",
        json!({"run_id": "inv-1", "path": "prompts/a.md", "variables": {}}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::ExpressionError.code());
    assert_eq!(
        error.message,
        "the prompt names 'topic', which nobody gave it"
    );

    let error = ask(
        h,
        "prompt.render",
        json!({"run_id": "inv-1", "path": "./prompts/a.md", "variables": {}}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::UndeclaredResource.code());
    assert_eq!(
        error.message,
        "./prompts/a.md is not one of caption's declared resources"
    );

    // A file variable renders as its content; one the run was not handed is refused.
    let mut brief = file(b"a lantern", "text/plain", "brief.txt");
    brief.content = Some(FileContent::Text("a lantern".into()));
    run.files.insert(&brief);
    let rendered = ask(
        h,
        "prompt.render",
        json!({"run_id": "inv-1", "path": "prompts/a.md", "variables": {"topic": {"file": brief.digest}}}),
    )
    .await
    .unwrap();
    assert_eq!(
        rendered,
        json!({"text": "What is it? (3) about a lantern\r\n"})
    );
    let stranger = file_digest(b"nobody");
    let error = ask(
        h,
        "prompt.render",
        json!({"run_id": "inv-1", "path": "prompts/a.md", "variables": {"topic": [{"file": stranger}]}}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::UnknownFile.code());
    assert_eq!(
        error.message,
        format!("variables.topic[0] names the file {stranger}, which this run was not handed")
    );
    let error = ask(
        h,
        "prompt.render",
        json!({"run_id": "inv-1", "path": "prompts/a.md", "variables": {"topic": {"pending": 1}}}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
}

#[tokio::test]
async fn file_puts_are_refused_before_anything_is_stored() {
    let run = planning_run();
    let h = &run.handler;
    std::fs::write(run.work.join("a.png"), b"png").unwrap();
    let error = ask(
        h,
        "file.put",
        json!({"run_id": "inv-1", "work_path": "../escape.png"}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::OutsideWorkDir.code());
    assert_eq!(
        error.message,
        "../escape.png names nothing inside the work dir"
    );
    let error = ask(
        h,
        "file.put",
        json!({"run_id": "inv-1", "work_path": "b.png"}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::OutsideWorkDir.code());
    let error = ask(h, "file.put", json!({"run_id": "inv-1", "base64": "AAE"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(error.message, "base64 is not RFC 4648 base64 with padding");
    let error = ask(
        h,
        "file.put",
        json!({"run_id": "inv-1", "json": {"a": {"missing": true}}}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(
        error.message,
        "json.a: an object holding only `missing` in this shape is reserved for FX's own values"
    );
    // Exactly one source, members it takes, and text where text belongs: each said as a
    // sentence, never as the reader's own text.
    for (params, message) in [
        (
            json!({"run_id": "inv-1", "base64": "AAE=", "json": 1}),
            "file.put takes exactly one of work_path, base64, json",
        ),
        (
            json!({"run_id": "inv-1", "kind": "json"}),
            "file.put takes exactly one of work_path, base64, json",
        ),
        (
            json!({"run_id": "inv-1", "base64": "AAE=", "colour": "red"}),
            "file.put: colour is not one of its fields",
        ),
        (
            json!({"run_id": "inv-1", "work_path": 3}),
            "file.put: work_path is text",
        ),
        (
            json!({"run_id": "inv-1", "base64": "AAE=", "kind": 3}),
            "file.put: it holds the number 3 where text belongs",
        ),
    ] {
        let error = ask(h, "file.put", params.clone()).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams.code(), "{params}");
        assert_eq!(error.message, message, "{params}");
    }
    let error = ask(
        h,
        "file.put",
        json!({"run_id": "inv-1", "base64": "AAE=", "kind": ""}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
}

#[tokio::test]
async fn a_capability_request_goes_through_the_call_path() {
    let run = planning_run();
    let error = ask(
        &run.handler,
        "capability",
        json!({"run_id": "inv-1", "capability": "structured.generate", "request": {"prompt": "p"}}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotLive.code());
    assert_eq!(
        error.message,
        "structured.generate on llm-a@acme is a paid call; run with --live"
    );
    let error = ask(
        &run.handler,
        "capability",
        json!({"run_id": "inv-1", "capability": "structured.generate", "request": {"prompt": "p"}}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::OverBound.code());
    assert_eq!(
        error.message,
        "caption declared at most 1 structured.generate calls"
    );
}

#[tokio::test]
async fn agent_pictures_must_be_files_the_run_was_handed() {
    let run = planning_run();
    let stranger = file_digest(b"nobody");
    let error = ask(
        &run.handler,
        "agent.run",
        json!({
            "run_id": "inv-1", "agent_id": "a1", "system": "s", "instructions": "i",
            "images": [{"file": stranger}], "tools": [], "max_steps": 1, "submit": null, "check": false
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::UnknownFile.code());
    assert_eq!(
        error.message,
        format!("images[0] names the file {stranger}, which this run was not handed")
    );
    // The project type declares no agent.turn: the first turn is refused by the call path.
    let error = ask(
        &run.handler,
        "agent.run",
        json!({
            "run_id": "inv-1", "agent_id": "a1", "system": "s", "instructions": "i",
            "tools": [], "max_steps": 1, "submit": null, "check": false
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::CapabilityUndeclared.code());
    assert_eq!(
        error.message,
        "caption calls agent.turn without declaring it"
    );
    // The loop had started: the error carries the transcript so far, for the body to read.
    assert_eq!(
        error.data.as_ref().unwrap()["transcript"],
        json!([{"role": "user", "content": "i"}])
    );
    assert_eq!(
        error.data.as_ref().unwrap()["capability"],
        json!("agent.turn")
    );
    // Refused before the first turn: no transcript.
    let error = ask(
        &run.handler,
        "agent.run",
        json!({
            "run_id": "inv-1", "agent_id": "a1", "system": "s", "instructions": "i",
            "tools": [], "max_steps": 0, "submit": null, "check": false
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(error.data, None);
}

// ---------------------------------------------------------------------------------------------
// End to end, with the store, the ledger, the retry owner and the FakeAdapter.

fn lantern_png() -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../conformance/cache-replay/in/.fx/cache/files/f3/f3945de0c1182a1b279816f51ef2e79938d04957c7bfde94cd4bf2eb4c2170b4",
        ),
    )
    .unwrap()
}

/// Seeds the store with the cache-replay case's call record.
fn seed_lantern(store: &Store) {
    let bytes = lantern_png();
    let stored = store.put_bytes(&bytes).unwrap();
    store
        .save_call(&CallRecord {
            key: LANTERN_KEY.into(),
            capability: "image.generate".into(),
            route: RouteEntry {
                id: "img-a@acme".into(),
                fingerprint: IMG_A_FINGERPRINT.into(),
            },
            request: json!({"background": "auto", "prompt": "a lantern"}),
            take: vec![1],
            files: IndexMap::from([(
                "image".to_string(),
                FileEntry {
                    digest: stored.digest,
                    kind: "image/png".into(),
                    name: "image".into(),
                    size: stored.size,
                    key: None,
                },
            )]),
            data: Value::Null,
            cost_usd: Some(Usd(20_000)),
        })
        .unwrap();
}

fn answered(bytes: &[u8], cost: Option<Usd>) -> Answer {
    Answer {
        files: IndexMap::from([(
            "image".to_string(),
            AnsweredFile {
                kind: "image/png".into(),
                bytes: bytes.to_vec(),
            },
        )]),
        data: json!({"revised_prompt": "a lantern"}),
        cost,
    }
}

fn lantern_request() -> Value {
    json!({"prompt": "a lantern", "background": "auto"})
}

#[test]
fn the_cache_replay_key_is_the_call_key() {
    assert_eq!(
        call_key(
            "image.generate",
            IMG_A_FINGERPRINT,
            &lantern_request(),
            &[1]
        ),
        LANTERN_KEY
    );
}

#[tokio::test]
async fn a_recorded_call_replays_offline_without_adapters() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path(), Adapters::new(), false);
    seed_lantern(&engine.store);
    // A leftover job record of an answered call is removed by the hit.
    engine
        .store
        .save_job(&JobRecord {
            key: LANTERN_KEY.into(),
            capability: "image.generate".into(),
            route: RouteEntry {
                id: "img-a@acme".into(),
                fingerprint: IMG_A_FINGERPRINT.into(),
            },
            request: lantern_request(),
            take: vec![1],
            state: JobState::Submitting,
            handle: None,
            note: None,
        })
        .unwrap();
    let services = Services::planning(Arc::clone(&engine));
    let counter = CallCounter::new();
    let files = RunFiles::new();
    let site = site(vec![img_a()], &[("image.generate", 1)]);
    let answer = call(
        &services,
        &site,
        &counter,
        &files,
        "image.generate",
        lantern_request(),
    )
    .await
    .unwrap();
    assert_eq!(answer.key, LANTERN_KEY);
    assert!(answer.cached);
    assert_eq!(answer.cost, Some(Usd::ZERO));
    assert_eq!(answer.charged, Usd::ZERO);
    let image = &answer.files["image"];
    assert_eq!(image.digest, file_digest(&lantern_png()));
    assert!(image.location.is_some());
    assert!(files.get(&image.digest).is_some());
    assert_eq!(counter.cost(), None);
    assert_eq!(engine.store.load_job(LANTERN_KEY).unwrap(), None);
}

#[tokio::test]
async fn the_lantern_node_replays_its_output_under_its_own_name() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path(), Adapters::new(), false);
    seed_lantern(&engine.store);
    let services = Services::planning(engine);
    let produced = run_node(
        &services,
        &lantern_job(),
        &CallCounter::new(),
        &RunFiles::new(),
    )
    .await
    .unwrap();
    let Val::File(image) = &produced.outputs["image"] else {
        panic!("{:?}", produced.outputs)
    };
    assert_eq!(image.name, "draw/image");
    assert_eq!(image.kind, "image/png");
    assert_eq!(image.digest, file_digest(&lantern_png()));
    assert!(produced.facts.is_empty());
}

#[tokio::test]
async fn a_live_miss_is_sent_stored_recorded_and_then_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(vec![Sent::Answered(answered(
        b"new png",
        Some(Usd(20_000)),
    ))]);
    let mut adapters = Adapters::new();
    adapters.register("image.generate", "acme", fake.as_request());
    let engine = engine(dir.path(), adapters, true);
    let log_path = dir.path().join("events.jsonl");
    let events = Arc::new(EventLog::open(&log_path, "inv", "plan").unwrap());
    let ledger = Arc::new(Ledger::new(Some(Usd(1_000_000)), Some(Arc::clone(&events))));
    let services = live_with(Arc::clone(&engine), Arc::clone(&ledger), Some(events));
    let site = site(vec![img_a()], &[("image.generate", 2)]);
    let counter = CallCounter::new();
    let files = RunFiles::new();

    let answer = call(
        &services,
        &site,
        &counter,
        &files,
        "image.generate",
        lantern_request(),
    )
    .await
    .unwrap();
    assert_eq!(answer.key, LANTERN_KEY);
    assert!(!answer.cached);
    assert_eq!(answer.cost, Some(Usd(20_000)));
    assert_eq!(answer.charged, Usd(20_000));
    assert_eq!(answer.data, json!({"revised_prompt": "a lantern"}));
    assert_eq!(answer.files["image"].digest, file_digest(b"new png"));
    assert_eq!(counter.cost(), Some(Usd(20_000)));
    assert_eq!(ledger.charged(), Usd(20_000));
    let record = engine.store.load_call(LANTERN_KEY).unwrap();
    assert_eq!(record.request, lantern_request());
    assert_eq!(record.take, vec![1]);
    assert_eq!(record.cost_usd, Some(Usd(20_000)));
    assert_eq!(record.computed_key(), LANTERN_KEY);
    match &fake.log()[..] {
        [FakeCall::Send(request)] => {
            assert_eq!(request.key, LANTERN_KEY);
            assert_eq!(request.route.id(), "img-a@acme");
            assert_eq!(request.take, vec![1]);
        }
        other => panic!("{other:?}"),
    }

    // The same call again is answered by the record: no send, nothing billed.
    let again = call(
        &services,
        &site,
        &counter,
        &files,
        "image.generate",
        lantern_request(),
    )
    .await
    .unwrap();
    assert!(again.cached);
    assert_eq!(again.cost, Some(Usd::ZERO));
    assert_eq!(fake.log().len(), 1);
    assert_eq!(counter.cost(), Some(Usd(20_000)));

    let events = read_events(&log_path).unwrap();
    let names: Vec<_> = events
        .iter()
        .map(|e| e["event"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["budget_reserved", "budget_settled", "call", "call"]);
    assert_eq!(events[2]["cached"], json!(false));
    assert_eq!(events[2]["cost_usd"], json!(0.02));
    assert_eq!(events[2]["id"], json!("draw#1"));
    assert_eq!(events[2]["route"], json!("img-a@acme"));
    assert_eq!(events[2]["call"], json!(LANTERN_KEY));
    assert_eq!(events[3]["cached"], json!(true));
    assert_eq!(events[3]["cost_usd"], json!(0));
}

#[tokio::test]
async fn an_unreported_cost_charges_the_hold_and_reports_null() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(vec![Sent::Answered(answered(b"png", None))]);
    let mut adapters = Adapters::new();
    adapters.register("image.generate", "acme", fake.as_request());
    let services = live(engine(dir.path(), adapters, true), Some(Usd(1_000_000)));
    let counter = CallCounter::new();
    let answer = call(
        &services,
        &site(vec![img_a()], &[("image.generate", 1)]),
        &counter,
        &RunFiles::new(),
        "image.generate",
        lantern_request(),
    )
    .await
    .unwrap();
    assert_eq!(answer.cost, None);
    assert_eq!(answer.charged, Usd(40_000));
    assert_eq!(counter.cost(), Some(Usd(40_000)));
}

#[tokio::test]
async fn a_billed_failure_before_the_answer_is_part_of_the_charge() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(vec![
        Sent::Failed {
            reason: "the provider timed out".into(),
            cost: Some(Usd(10_000)),
            retryable: true,
        },
        Sent::Answered(answered(b"png", Some(Usd(20_000)))),
    ]);
    let mut adapters = Adapters::new();
    adapters.register("image.generate", "acme", fake.as_request());
    let services = live(engine(dir.path(), adapters, true), Some(Usd(1_000_000)));
    let counter = CallCounter::new();
    let answer = call(
        &services,
        &site(vec![img_a()], &[("image.generate", 1)]),
        &counter,
        &RunFiles::new(),
        "image.generate",
        lantern_request(),
    )
    .await
    .unwrap();
    assert_eq!(answer.cost, Some(Usd(20_000)));
    assert_eq!(answer.charged, Usd(30_000));
    assert_eq!(counter.cost(), Some(Usd(30_000)));
}

#[tokio::test]
async fn refusals_and_failures_of_the_retry_owner_map_to_their_codes() {
    let cases = [
        (
            Sent::Refused {
                reason: "no key for acme".into(),
            },
            ErrorCode::CapabilityRefused,
            "image.generate on img-a@acme was refused: no key for acme",
            Some(Usd::ZERO),
        ),
        (
            Sent::Failed {
                reason: "the picture was empty".into(),
                cost: Some(Usd(5_000)),
                retryable: false,
            },
            ErrorCode::CallFailed,
            "image.generate on img-a@acme failed: the picture was empty",
            Some(Usd(5_000)),
        ),
    ];
    for (sent, code, message, cost) in cases {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeAdapter::plain(vec![sent]);
        let mut adapters = Adapters::new();
        adapters.register("image.generate", "acme", fake.as_request());
        let services = live(engine(dir.path(), adapters, true), Some(Usd(1_000_000)));
        let counter = CallCounter::new();
        let error = rpc(call(
            &services,
            &site(vec![img_a()], &[("image.generate", 1)]),
            &counter,
            &RunFiles::new(),
            "image.generate",
            lantern_request(),
        )
        .await);
        assert_eq!(error.code, code.code());
        assert_eq!(error.message, message);
        assert_eq!(
            error.data,
            Some(
                json!({"capability": "image.generate", "route": "img-a@acme", "key": LANTERN_KEY})
            )
        );
        assert_eq!(counter.cost(), cost);
        assert!(services.engine.store.load_call(LANTERN_KEY).is_none());
    }
}

#[tokio::test]
async fn a_hold_past_the_ceiling_is_ceiling_exceeded() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(vec![Sent::Answered(answered(b"png", Some(Usd(20_000))))]);
    let mut adapters = Adapters::new();
    adapters.register("image.generate", "acme", fake.as_request());
    let services = live(engine(dir.path(), adapters, true), Some(Usd(10_000)));
    let error = rpc(call(
        &services,
        &site(vec![img_a()], &[("image.generate", 1)]),
        &CallCounter::new(),
        &RunFiles::new(),
        "image.generate",
        lantern_request(),
    )
    .await);
    assert_eq!(error.code, ErrorCode::CeilingExceeded.code());
    let data = error.data.unwrap();
    assert_eq!(data["needed_usd"], json!(0.04));
    assert_eq!(data["remaining_usd"], json!(0.01));
    assert_eq!(data["key"], json!(LANTERN_KEY));
    assert!(fake.log().is_empty());
}

fn job_record(state: JobState, handle: Option<Value>) -> JobRecord {
    JobRecord {
        key: LANTERN_KEY.into(),
        capability: "image.generate".into(),
        route: RouteEntry {
            id: "img-a@acme".into(),
            fingerprint: IMG_A_FINGERPRINT.into(),
        },
        request: lantern_request(),
        take: vec![1],
        state,
        handle,
        note: None,
    }
}

#[tokio::test]
async fn an_uncertain_submit_s_reason_is_kept_for_the_next_run() {
    let reason = "fal took the video job but returned no handle to collect it by (request req-7)";
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::long_job(
        vec![Submitted::Uncertain {
            reason: reason.into(),
        }],
        Vec::new(),
    );
    let mut adapters = Adapters::new();
    adapters.register("image.generate", "acme", fake.as_job());
    let engine_ = engine(dir.path(), adapters, true);
    let site = site(vec![img_a()], &[("image.generate", 5)]);
    let files = RunFiles::new();
    let call_once = |services: Arc<Services>| {
        let site = &site;
        let files = &files;
        async move {
            rpc(call(
                &services,
                site,
                &CallCounter::new(),
                files,
                "image.generate",
                lantern_request(),
            )
            .await)
        }
    };
    let message = format!(
        "image.generate on img-a@acme (take 1) was being submitted when a run stopped, and \
         nobody can say whether the provider took it. Check the provider's dashboard, then run \
         grida-fx jobs --forget {LANTERN_KEY} to submit it again"
    );

    // The run that submitted it: the adapter's reason, and the record keeps it as its note.
    let error = call_once(live(Arc::clone(&engine_), Some(Usd(1_000_000)))).await;
    assert_eq!(error.code, ErrorCode::JobUnsettled.code());
    assert_eq!(error.message, message);
    assert_eq!(error.data.as_ref().unwrap()["reason"], json!(reason));
    let record = engine_.store.load_job(LANTERN_KEY).unwrap().unwrap();
    assert_eq!(record.state, JobState::Submitting);
    assert_eq!(record.handle, None);
    assert_eq!(record.note.as_deref(), Some(reason));
    assert_eq!(fake.log().len(), 1);

    // A later run stops for a person, with the same reason, and submits nothing.
    let error = call_once(live(Arc::clone(&engine_), Some(Usd(1_000_000)))).await;
    assert_eq!(error.code, ErrorCode::JobUnsettled.code());
    assert_eq!(error.message, message);
    let data = error.data.unwrap();
    assert_eq!(data["reason"], json!(reason));
    assert_eq!(data["key"], json!(LANTERN_KEY));
    assert_eq!(fake.log().len(), 1, "never submitted again");

    // A record written before a submit that never returned has no note, and no reason.
    engine_
        .store
        .save_job(&job_record(JobState::Submitting, None))
        .unwrap();
    let error = call_once(live(Arc::clone(&engine_), Some(Usd(1_000_000)))).await;
    assert_eq!(error.code, ErrorCode::JobUnsettled.code());
    assert!(error.data.unwrap().get("reason").is_none());
}

#[tokio::test]
async fn job_records_decide_what_a_call_may_do() {
    // `submitting`: nobody knows whether the provider took it.
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::long_job(Vec::new(), Vec::new());
    let mut adapters = Adapters::new();
    adapters.register("image.generate", "acme", fake.as_job());
    let engine_ = engine(dir.path(), adapters, true);
    engine_
        .store
        .save_job(&job_record(JobState::Submitting, None))
        .unwrap();
    let services = live(Arc::clone(&engine_), Some(Usd(1_000_000)));
    let site = site(vec![img_a()], &[("image.generate", 5)]);
    let counter = CallCounter::new();
    let files = RunFiles::new();
    let error = rpc(call(
        &services,
        &site,
        &counter,
        &files,
        "image.generate",
        lantern_request(),
    )
    .await);
    assert_eq!(error.code, ErrorCode::JobUnsettled.code());
    assert_eq!(
        error.message,
        format!(
            "image.generate on img-a@acme (take 1) was being submitted when a run stopped, and \
             nobody can say whether the provider took it. Check the provider's dashboard, then \
             run grida-fx jobs --forget {LANTERN_KEY} to submit it again"
        )
    );
    assert!(fake.log().is_empty());

    // `submitted` with a long-job adapter: collected by its handle, billing nothing new.
    let handle = json!({"request_id": "R1"});
    let fake = FakeAdapter::long_job(
        Vec::new(),
        vec![Collected::Answered(answered(b"video", Some(Usd(300_000))))],
    );
    let mut adapters = Adapters::new();
    adapters.register("image.generate", "acme", fake.as_job());
    let dir = tempfile::tempdir().unwrap();
    let engine_ = engine(dir.path(), adapters, true);
    engine_
        .store
        .save_job(&job_record(JobState::Submitted, Some(handle.clone())))
        .unwrap();
    let ledger = Arc::new(Ledger::new(Some(Usd(1_000_000)), None));
    let services = live_with(Arc::clone(&engine_), Arc::clone(&ledger), None);
    let counter = CallCounter::new();
    let answer = call(
        &services,
        &site,
        &counter,
        &files,
        "image.generate",
        lantern_request(),
    )
    .await
    .unwrap();
    assert!(!answer.cached);
    assert_eq!(answer.cost, Some(Usd::ZERO));
    assert_eq!(answer.charged, Usd::ZERO);
    assert_eq!(counter.cost(), Some(Usd::ZERO));
    assert_eq!(ledger.charged(), Usd::ZERO);
    assert!(matches!(&fake.log()[..], [FakeCall::Collect(_, h)] if *h == handle));
    assert_eq!(engine_.store.load_job(LANTERN_KEY).unwrap(), None);
    assert!(engine_.store.load_call(LANTERN_KEY).is_some());

    // `submitted` meeting a plain adapter cannot be collected.
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(Vec::new());
    let mut adapters = Adapters::new();
    adapters.register("image.generate", "acme", fake.as_request());
    let engine_ = engine(dir.path(), adapters, true);
    engine_
        .store
        .save_job(&job_record(JobState::Submitted, Some(handle)))
        .unwrap();
    let services = live(engine_, Some(Usd(1_000_000)));
    let error = rpc(call(
        &services,
        &site,
        &CallCounter::new(),
        &files,
        "image.generate",
        lantern_request(),
    )
    .await);
    assert_eq!(error.code, ErrorCode::JobUnsettled.code());
    assert!(fake.log().is_empty());

    // `settled`: submitted anew.
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::long_job(
        vec![Submitted::Accepted {
            handle: json!({"request_id": "R2"}),
        }],
        vec![Collected::Answered(answered(b"again", Some(Usd(30_000))))],
    );
    let mut adapters = Adapters::new();
    adapters.register("image.generate", "acme", fake.as_job());
    let engine_ = engine(dir.path(), adapters, true);
    engine_
        .store
        .save_job(&job_record(JobState::Settled, None))
        .unwrap();
    let services = live(Arc::clone(&engine_), Some(Usd(1_000_000)));
    let answer = call(
        &services,
        &site,
        &CallCounter::new(),
        &files,
        "image.generate",
        lantern_request(),
    )
    .await
    .unwrap();
    assert_eq!(answer.cost, Some(Usd(30_000)));
    assert!(matches!(
        &fake.log()[..],
        [FakeCall::Submit(_), FakeCall::Collect(..)]
    ));
    assert_eq!(engine_.store.load_job(LANTERN_KEY).unwrap(), None);
}

#[tokio::test]
async fn an_unreadable_job_record_stops_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(Vec::new());
    let mut adapters = Adapters::new();
    adapters.register("image.generate", "acme", fake.as_request());
    let engine = engine(dir.path(), adapters, true);
    let jobs = dir.path().join(".fx/cache/jobs");
    std::fs::create_dir_all(&jobs).unwrap();
    std::fs::write(jobs.join(format!("{LANTERN_KEY}.json")), b"{not json").unwrap();
    let services = live(engine, Some(Usd(1_000_000)));
    let result = call(
        &services,
        &site(vec![img_a()], &[("image.generate", 1)]),
        &CallCounter::new(),
        &RunFiles::new(),
        "image.generate",
        lantern_request(),
    )
    .await;
    match result {
        Err(CallError::Store(message)) => {
            assert!(
                message.starts_with(&format!(
                    "the job record jobs/{LANTERN_KEY}.json is unreadable"
                )),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_judge_built_in_reports_its_verdict_and_facts() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(vec![Sent::Answered(Answer {
        files: IndexMap::new(),
        data: json!({"verdict": "accept", "facts": {"score": 0.9, "cost_usd": 3}, "json": {"ok": true}}),
        cost: Some(Usd(2_000)),
    })]);
    let mut adapters = Adapters::new();
    adapters.register("structured.review", "acme", fake.as_request());
    let services = live(engine(dir.path(), adapters, true), Some(Usd(1_000_000)));
    let mut subject = file(b"{}", "json", "subject.json");
    subject.content = Some(FileContent::Json(json!({})));
    let location = dir.path().join("subject.json");
    std::fs::write(&location, b"{}").unwrap();
    subject.location = Some(location);
    let job = builtin_job(
        "structured.review",
        vec![("subject", Val::File(Box::new(subject)))],
        vec![route("structured.review", "llm-a", "acme", 4_000)],
    );
    let produced = run_node(&services, &job, &CallCounter::new(), &RunFiles::new())
        .await
        .unwrap();
    assert!(produced.outputs.is_empty());
    assert_eq!(
        produced.facts.into_iter().collect::<Vec<_>>(),
        vec![
            ("verdict".to_string(), json!("accept")),
            ("score".to_string(), json!(0.9)),
        ]
    );
}

#[tokio::test]
async fn a_file_put_answers_the_stored_files_ref() {
    let run = planning_run();
    std::fs::write(run.work.join("out.txt"), b"hello\n").unwrap();
    let reference = ask(
        &run.handler,
        "file.put",
        json!({"run_id": "inv-1", "work_path": "out.txt"}),
    )
    .await
    .unwrap();
    assert_eq!(reference["digest"], json!(file_digest(b"hello\n")));
    assert_eq!(reference["kind"], json!("text/plain"));
    assert_eq!(reference["name"], json!("out.txt"));
    assert!(run.files.get(&file_digest(b"hello\n")).is_some());
    let reference = ask(
        &run.handler,
        "file.put",
        json!({"run_id": "inv-1", "json": {"b": 1, "a": [true]}}),
    )
    .await
    .unwrap();
    let written = "{\n \"a\": [\n  true\n ],\n \"b\": 1\n}";
    assert_eq!(reference["digest"], json!(file_digest(written.as_bytes())));
    assert_eq!(reference["kind"], json!("json"));
    let reference = ask(
        &run.handler,
        "file.put",
        json!({"run_id": "inv-1", "base64": "AAE=", "kind": "image/png", "name": "pic.png"}),
    )
    .await
    .unwrap();
    assert_eq!(reference["digest"], json!(file_digest(&[0, 1])));
    assert_eq!(reference["kind"], json!("image/png"));
    assert_eq!(reference["name"], json!("pic.png"));
}

#[tokio::test]
async fn only_the_engines_own_faults_stop_the_run() {
    let run = planning_run();
    // A resource that cannot be read fails the request; the run goes on.
    let resource = run.handler.job.resources["prompts/a.md"].clone();
    std::fs::remove_file(&resource).unwrap();
    let error = ask(
        &run.handler,
        "prompt.render",
        json!({"run_id": "inv-1", "path": "prompts/a.md", "variables": {}}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Internal.code());
    assert_eq!(error.message, "cannot read prompts/a.md: no such file");
    assert_eq!(run.handler.fault(), None);
    // A store that cannot be written is the engine's fault: the first one is kept.
    let store = run.work.parent().unwrap().parent().unwrap().to_path_buf();
    std::fs::write(store.join("files"), b"in the way").unwrap();
    for _ in 0..2 {
        let error = ask(
            &run.handler,
            "file.put",
            json!({"run_id": "inv-1", "base64": "AAE="}),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal.code());
    }
    let fault = run.handler.fault().unwrap();
    let digest = file_digest(&[0, 1]);
    assert!(
        fault.starts_with(&format!("the store's files/{}/{digest}: ", &digest[..2])),
        "{fault}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_work_file_the_engine_cannot_read_is_the_bodys_not_the_stores() {
    use std::os::unix::fs::PermissionsExt;
    let run = planning_run();
    let secret = run.work.join("secret.txt");
    std::fs::write(&secret, b"hidden").unwrap();
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&secret).is_ok() {
        eprintln!("skipped: this user reads files whatever their mode");
        return;
    }
    let error = ask(
        &run.handler,
        "file.put",
        json!({"run_id": "inv-1", "work_path": "secret.txt"}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(error.message, "cannot read secret.txt: permission denied");
    // The request failed; the run has no fault, so it goes on.
    assert_eq!(run.handler.fault(), None);
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
}

#[tokio::test]
async fn facts_and_marks_the_record_cannot_hold_are_refused() {
    let run = planning_run();
    let h = &run.handler;
    let nested = |depth: usize| {
        let mut value = json!(1);
        for _ in 0..depth {
            value = json!([value]);
        }
        value
    };
    let depth = grida_fx_runtime::events::VALUE_DEPTH;
    assert_eq!(
        ask(
            h,
            "fact",
            json!({"run_id": "inv-1", "name": "deep", "value": nested(depth)})
        )
        .await,
        Ok(json!({}))
    );
    let error = ask(
        h,
        "fact",
        json!({"run_id": "inv-1", "name": "deeper", "value": nested(depth + 1)}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(
        error.message,
        format!(
            "fact deeper is nested deeper than {depth} levels, which the run's record cannot hold"
        )
    );
    let error = ask(
        h,
        "annotate",
        json!({"run_id": "inv-1", "mark": {"label": "x", "extra": nested(depth)}}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(
        error.message,
        format!("mark is nested deeper than {depth} levels, which the run's record cannot hold")
    );
    assert_eq!(h.facts().keys().collect::<Vec<_>>(), ["deep"]);
    assert!(h.marks().is_empty());
    // Params FX cannot read are told as a sentence.
    let error = ask(h, "fact", json!({"run_id": "inv-1", "name": "n"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert_eq!(error.message, "fact: it has no value");
}

#[tokio::test]
async fn a_capability_hit_answers_the_host_with_file_refs() {
    let run = run_for(lantern_job(), |engine| {
        seed_lantern(&engine.store);
        Arc::new(Services::planning(engine))
    });
    let result = ask(
        &run.handler,
        "capability",
        json!({"run_id": "inv-1", "capability": "image.generate", "request": lantern_request()}),
    )
    .await
    .unwrap();
    assert_eq!(result["key"], json!(LANTERN_KEY));
    assert_eq!(result["cached"], json!(true));
    assert_eq!(result["cost_usd"], json!(0.0));
    assert_eq!(result["data"], Value::Null);
    let image = &result["files"]["image"];
    assert_eq!(image["digest"], json!(file_digest(&lantern_png())));
    assert_eq!(image["kind"], json!("image/png"));
    assert_eq!(image["facts"]["width"], json!(1));
    assert!(Path::new(image["path"].as_str().unwrap()).is_absolute());
    assert!(run.files.get(&file_digest(&lantern_png())).is_some());
    assert_eq!(run.handler.counter.cost(), None);
}
