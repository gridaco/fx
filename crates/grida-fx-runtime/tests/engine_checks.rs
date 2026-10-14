//! The engine's checks around an adapter that names its contract, as FX's adapters do
//! (spec/adapters.md §3.4, §4; `calls::checked`): through the call path, with the real store,
//! ledger, retry owner and event log, and the `FakeAdapter` given an identity.

use grida_fx_core::money::Usd;
use grida_fx_core::routes::{PriceUnit, Route, RoutePrice};
use grida_fx_protocol::{ErrorCode, RpcError};
use grida_fx_providers::adapter::Identity;
use grida_fx_providers::fake::{FakeAdapter, FakeCall};
use grida_fx_providers::testing::media;
use grida_fx_providers::{Adapters, Answer, Collected, Sent};
use grida_fx_runtime::calls::pacing::Pacing;
use grida_fx_runtime::calls::{CallAnswer, CallCounter, CallError, CallSite, call};
use grida_fx_runtime::engine::{Cancel, Engine, RunFiles, Services};
use grida_fx_runtime::events::{EventLog, read_events};
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::ledger::Ledger;
use grida_fx_runtime::store::records::{JobRecord, JobState, RouteEntry, call_key};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// The contract the fake adapter serves: `acme-image`, behaviour the string `"1"`.
const ACME: Identity = Identity {
    adapter: "acme-image",
    behavior: r#""1""#,
};

const FOREIGN: &str = "img-a@acme is not a image.generate route this adapter serves";

fn img_a(contract: Value) -> Route {
    Route {
        capability: "image.generate".into(),
        model: "img-a".into(),
        provider: "acme".into(),
        price: RoutePrice {
            low: Usd(10_000),
            high: Usd(40_000),
            unit: PriceUnit::Call,
            max_units: None,
            by: None,
            tiers: IndexMap::new(),
        },
        features: Default::default(),
        concurrency: None,
        requests_per_minute: None,
        max_input_pictures: None,
        contract,
    }
}

fn site(route: Route) -> CallSite {
    CallSite {
        instance_id: "draw#1".into(),
        path: "draw".into(),
        step: "draw".into(),
        type_name: "image.generate".into(),
        takes: vec![1],
        calls: IndexMap::from([("image.generate".to_string(), 1)]),
        routes: IndexMap::from([("image.generate".to_string(), route)]),
        limits: IndexMap::new(),
        scopes: Vec::new(),
        cancel: Cancel::new(),
    }
}

/// A live run over `adapter`, with an event log at `root/events.jsonl` and a $1 ceiling.
fn run(root: &Path, adapter: grida_fx_providers::Adapter) -> Arc<Services> {
    let mut adapters = Adapters::new();
    adapters.register("image.generate", "acme", adapter);
    let engine = Arc::new(Engine::new(
        tokio::runtime::Handle::current(),
        HostSpec::python(
            PathBuf::from("python3"),
            "python3",
            root.to_path_buf(),
            Vec::new(),
        )
        .unchecked(),
        &root.join(".fx/cache"),
        1,
        adapters,
        true,
    ));
    let events = Arc::new(EventLog::open(&root.join("events.jsonl"), "inv", "plan").unwrap());
    let ledger = Arc::new(Ledger::new(Some(Usd(1_000_000)), Some(Arc::clone(&events))));
    let cancel = Cancel::new();
    Arc::new(Services {
        control: grida_fx_runtime::run_control::RunControl::new(
            Arc::clone(&events),
            cancel.clone(),
        ),
        engine,
        ledger: Some(ledger),
        events: Some(events),
        pacing: Arc::new(Pacing::new()),
        cancel,
        invocation_id: "inv".into(),
        runs: AtomicU64::new(0),
        holds: AtomicU64::new(0),
    })
}

async fn draw(
    services: &Services,
    route: Route,
    request: Value,
) -> (Result<CallAnswer, CallError>, CallCounter) {
    let counter = CallCounter::new();
    let answered = call(
        services,
        &site(route),
        &counter,
        &RunFiles::new(),
        "image.generate",
        request,
    )
    .await;
    (answered, counter)
}

fn rpc(result: Result<CallAnswer, CallError>) -> RpcError {
    match result {
        Err(CallError::Rpc(error)) => error,
        other => panic!("expected an RPC error, got {other:?}"),
    }
}

fn lantern() -> Value {
    json!({"prompt": "a lantern", "background": "auto"})
}

fn picture(bytes: Vec<u8>, cost: Usd) -> Sent {
    Sent::Answered(Answer::new(Value::Null, Some(cost)).with_file("image", "image/png", bytes))
}

/// The events of the run at `root`, by name, with each settlement's charge.
fn budget(root: &Path) -> Vec<(String, Value)> {
    read_events(&root.join("events.jsonl"))
        .unwrap()
        .iter()
        .map(|event| {
            let name = event["event"].as_str().unwrap().to_string();
            (
                name,
                event.get("charged_usd").cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_contract_the_adapter_does_not_serve_is_refused_inside_the_attempt_at_nothing() {
    for job in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeAdapter::default().with_identity(ACME);
        let adapter = if job {
            fake.as_job()
        } else {
            fake.as_request()
        };
        let services = run(dir.path(), adapter);
        let route = img_a(json!({"adapter": "acme-other"}));
        let (answered, counter) = draw(&services, route, lantern()).await;
        let error = rpc(answered);
        assert_eq!(error.code, ErrorCode::CapabilityRefused.code());
        assert_eq!(
            error.message,
            format!("image.generate on img-a@acme was refused: {FOREIGN}")
        );
        assert!(fake.log().is_empty(), "the adapter never saw the call");
        // Refused where the adapter's own refusal would be: after the hold, settled at $0.
        assert_eq!(
            budget(dir.path()),
            [
                ("budget_reserved".to_string(), Value::Null),
                ("budget_settled".to_string(), json!(0)),
            ]
        );
        assert_eq!(counter.cost(), Some(Usd::ZERO));
        // A long job's `submitting` record went with the refusal.
        let key = error.data.unwrap()["key"].as_str().unwrap().to_string();
        assert!(services.engine.store.load_job(&key).unwrap().is_none());
    }
}

#[tokio::test]
async fn the_adapters_own_contract_judgement_comes_between_the_engines_two_checks() {
    let pictures = Arc::new(|call: &grida_fx_providers::CallRequest| {
        match call.route.contract.get("pictures") {
            Some(pictures) if *pictures != "unchanged" => Err(format!(
                "{} is not a {} route this adapter serves: pictures is \"unchanged\" or absent",
                call.route.id(),
                call.route.capability
            )),
            _ => Ok(()),
        }
    });
    let cases = [
        (
            json!({"adapter": "acme-other", "pictures": "small"}),
            json!({}),
            FOREIGN.to_string(),
        ),
        (
            json!({"adapter_behavior": 1}),
            json!({}),
            FOREIGN.to_string(),
        ),
        (
            json!({"pictures": "small"}),
            json!({}),
            format!("{FOREIGN}: pictures is \"unchanged\" or absent"),
        ),
        (
            json!({"adapter_behavior": "1", "pictures": "unchanged"}),
            json!({}),
            "image.generate needs prompt".to_string(),
        ),
        (
            json!({}),
            json!({"prompt": "a lantern", "quality": "max"}),
            "image.generate takes no member quality".to_string(),
        ),
    ];
    for (contract, request, reason) in cases {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeAdapter::default()
            .with_identity(ACME)
            .with_contract_check(pictures.clone());
        let services = run(dir.path(), fake.as_request());
        let (answered, _) = draw(&services, img_a(contract.clone()), request).await;
        let error = rpc(answered);
        assert_eq!(
            error.code,
            ErrorCode::CapabilityRefused.code(),
            "{contract}"
        );
        assert_eq!(
            error.message,
            format!("image.generate on img-a@acme was refused: {reason}"),
            "{contract}"
        );
        assert!(fake.log().is_empty());
    }
    // A contract and a request that pass reach the adapter.
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(vec![picture(media::png(2, 2, None), Usd(20_000))])
        .with_identity(ACME)
        .with_contract_check(pictures);
    let services = run(dir.path(), fake.as_request());
    let route = img_a(json!({"adapter": "acme-image", "adapter_behavior": "1"}));
    let (answered, _) = draw(&services, route, lantern()).await;
    assert_eq!(answered.unwrap().charged, Usd(20_000));
    assert!(matches!(&fake.log()[..], [FakeCall::Send(_)]));
}

#[tokio::test(start_paused = true)]
async fn an_answer_the_capabilitys_every_route_check_refuses_is_billed() {
    let not_png = || picture(b"not a png".to_vec(), Usd(10_000));
    let png = || picture(media::png(2, 2, None), Usd(20_000));

    // The adapter's own check passes it; the engine's every-route check does not.
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(vec![not_png(), png()]).with_identity(ACME);
    let services = run(dir.path(), fake.as_request());
    let (answered, counter) = draw(&services, img_a(json!({})), lantern()).await;
    let answer = answered.unwrap();
    assert_eq!(answer.charged, Usd(30_000), "the refused answer was billed");
    assert_eq!(counter.cost(), Some(Usd(30_000)));
    assert_eq!(fake.log().len(), 2);

    // Every attempt refused: the check's sentence.
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain((0..6).map(|_| not_png()).collect()).with_identity(ACME);
    let services = run(dir.path(), fake.as_request());
    let (answered, counter) = draw(&services, img_a(json!({})), lantern()).await;
    let error = rpc(answered);
    assert_eq!(error.code, ErrorCode::CallFailed.code());
    assert_eq!(
        error.message,
        "image.generate on img-a@acme failed: the answer is not an image, not image/png"
    );
    assert_eq!(counter.cost(), Some(Usd(60_000)));

    // An adapter that names no contract gets no check of the engine's around it.
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeAdapter::plain(vec![not_png()]);
    let services = run(dir.path(), fake.as_request());
    let (answered, _) = draw(
        &services,
        img_a(json!({"adapter": "acme-other"})),
        lantern(),
    )
    .await;
    assert_eq!(answered.unwrap().charged, Usd(10_000));
}

#[tokio::test]
async fn a_submitted_job_is_collected_without_the_engines_refusals() {
    // The job's key fixes its route's contract: collecting it checks nothing first.
    let dir = tempfile::tempdir().unwrap();
    let handle = json!({"request_id": "R1"});
    let fake = FakeAdapter::long_job(
        Vec::new(),
        vec![Collected::Answered(
            Answer::new(Value::Null, Some(Usd(30_000))).with_file(
                "image",
                "image/png",
                media::png(2, 2, None),
            ),
        )],
    )
    .with_identity(ACME);
    let services = run(dir.path(), fake.as_job());
    let route = img_a(json!({"adapter": "acme-other"}));
    let fingerprint = route.fingerprint();
    let key = call_key("image.generate", &fingerprint, &lantern(), &[1]);
    services
        .engine
        .store
        .save_job(&JobRecord {
            key: key.clone(),
            capability: "image.generate".into(),
            route: RouteEntry {
                id: "img-a@acme".into(),
                fingerprint,
            },
            request: lantern(),
            take: vec![1],
            state: JobState::Submitted,
            handle: Some(handle.clone()),
            note: None,
        })
        .unwrap();
    let (answered, _) = draw(&services, route, lantern()).await;
    let answer = answered.unwrap();
    assert_eq!(answer.key, key);
    assert_eq!(answer.charged, Usd::ZERO);
    match &fake.log()[..] {
        [FakeCall::Collect(_, collected)] => assert_eq!(collected, &handle),
        other => panic!("{other:?}"),
    }
}
