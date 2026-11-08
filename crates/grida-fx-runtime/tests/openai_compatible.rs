//! Declared OpenAI-compatible providers through the real call path, ledger and event log.
//! Every HTTP exchange is replayed, the provider needs no key, and time is paused: these tests
//! exercise receipt classification and the engine's retry accounting without network access.

use grida_fx_core::money::Usd;
use grida_fx_core::providers::{BaseUrl, Declaration, Declarations};
use grida_fx_core::routes::{Route, RouteTable};
use grida_fx_protocol::ErrorCode;
use grida_fx_providers::keys::{Environment, Keys};
use grida_fx_providers::live;
use grida_fx_providers::testing::setup;
use grida_fx_providers::transport::HttpResponse;
use grida_fx_providers::transport::replay::{Expect, ExpectBody, ReplayTransport};
use grida_fx_runtime::calls::pacing::Pacing;
use grida_fx_runtime::calls::{CallAnswer, CallCounter, CallError, CallSite, call};
use grida_fx_runtime::engine::{Cancel, Engine, RunFiles, Services};
use grida_fx_runtime::events::{EventLog, read_events};
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::ledger::Ledger;
use grida_fx_runtime::run_control::RunControl;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

const CAP: Usd = Usd(50_000);
const CAPABILITIES: [&str; 2] = ["structured.generate", "agent.turn"];

struct Played {
    outcome: Result<CallAnswer, CallError>,
    events: Vec<Value>,
    charged: Usd,
    sends: usize,
}

fn request(capability: &str) -> Value {
    match capability {
        "structured.generate" => json!({
            "prompt": "Name a colour.",
            "schema": {"type": "object", "properties": {"name": {"type": "string"}},
                       "required": ["name"], "additionalProperties": false}
        }),
        "agent.turn" => json!({
            "system": "Be brief.",
            "messages": [{"role": "user", "content": "Name a colour."}],
            "tools": [],
            "tool_choice": "auto"
        }),
        _ => unreachable!("only the family's two capabilities are exercised"),
    }
}

fn success(capability: &str) -> HttpResponse {
    let content = match capability {
        "structured.generate" => "{\"name\":\"red\"}",
        "agent.turn" => "red",
        _ => unreachable!("only the family's two capabilities are exercised"),
    };
    HttpResponse::json(
        200,
        &json!({"choices": [{"message": {"role": "assistant", "content": content}}]}),
    )
}

fn success_with_cost(capability: &str, cost: Value) -> HttpResponse {
    let mut body: Value = serde_json::from_slice(&success(capability).body).unwrap();
    body["usage"] = json!({"cost": cost});
    HttpResponse::json(200, &body)
}

fn route(capability: &str, settings: Value) -> Route {
    let adapter = match capability {
        "structured.generate" => "openai-compatible-structured",
        "agent.turn" => "openai-compatible-tool-loop",
        _ => unreachable!("only the family's two capabilities are exercised"),
    };
    let mut contract = json!({"adapter": adapter, "adapter_behavior": 1});
    contract
        .as_object_mut()
        .unwrap()
        .extend(settings.as_object().unwrap().clone());
    let table = RouteTable::from_document(
        &json!({"fx": "routes/v1", "routes": [{
            "capability": capability, "route": "text-a@acme", "price": {"cap_usd": 0.05},
            "contract": contract
        }]}),
        "routes.yaml",
    )
    .unwrap();
    table.resolve(capability, "text-a@acme").unwrap().clone()
}

/// One call on a keyless declared provider. The real adapters, retry owner, budget ledger,
/// store and recorded events all participate; only the transport and clock are synthetic.
async fn play(capability: &str, ceiling: Usd, responses: Vec<HttpResponse>) -> Played {
    play_with_contract(capability, ceiling, responses, json!({})).await
}

async fn play_with_contract(
    capability: &str,
    ceiling: Usd,
    responses: Vec<HttpResponse>,
    settings: Value,
) -> Played {
    let dir = tempfile::tempdir().unwrap();
    let transport = Arc::new(ReplayTransport::new(
        responses
            .into_iter()
            .map(|response| {
                Expect::post_json("https://api.acme.test/v1/chat/completions", Value::Null)
                    .body(ExpectBody::Any)
                    .reply(response)
            })
            .collect(),
    ));
    let declarations = Declarations::with(&[Declaration {
        name: "acme".into(),
        label: "acme".into(),
        credential: None,
        base_url: BaseUrl {
            variable: None,
            default: "https://api.acme.test/v1".into(),
        },
        download_hosts: Vec::new(),
        family: Some("openai-compatible".into()),
        adapter: None,
    }]);
    let environment = Environment::declared(&[], &declarations);
    let adapters = live::adapters(&setup(
        transport.clone(),
        Keys::from_environment(&environment),
    ));
    let engine = Arc::new(Engine::new(
        tokio::runtime::Handle::current(),
        HostSpec::python(
            PathBuf::from("python3"),
            "python3",
            dir.path().to_path_buf(),
            Vec::new(),
        )
        .unchecked(),
        &dir.path().join("cache"),
        1,
        adapters,
        true,
    ));
    let log_path = dir.path().join("events.jsonl");
    let events = Arc::new(EventLog::open(&log_path, "inv", "plan").unwrap());
    let ledger = Arc::new(Ledger::new(Some(ceiling), Some(Arc::clone(&events))));
    let cancel = Cancel::new();
    let services = Services {
        control: RunControl::new(Arc::clone(&events), cancel.clone()),
        engine: Arc::clone(&engine),
        ledger: Some(Arc::clone(&ledger)),
        events: Some(events),
        pacing: Arc::new(Pacing::new()),
        cancel,
        invocation_id: "inv".into(),
        runs: AtomicU64::new(0),
        holds: AtomicU64::new(0),
    };
    let site = CallSite {
        instance_id: "choose#1".into(),
        path: "choose".into(),
        step: "choose".into(),
        type_name: "choose".into(),
        takes: vec![1],
        calls: IndexMap::from([(capability.into(), 1)]),
        routes: IndexMap::from([(capability.into(), route(capability, settings))]),
        limits: IndexMap::new(),
        scopes: Vec::new(),
        cancel: Cancel::new(),
    };
    let counter = CallCounter::new();
    let outcome = call(
        &services,
        &site,
        &counter,
        &RunFiles::new(),
        capability,
        request(capability),
    )
    .await;
    transport.assert_done();
    assert_eq!(ledger.held(), Usd::ZERO, "every hold was settled");
    assert_eq!(counter.cost(), Some(ledger.charged()));
    if let Ok(answer) = &outcome {
        let record = engine.store.load_call(&answer.key).unwrap();
        assert_eq!(record.data, answer.data);
        assert_eq!(record.cost_usd, answer.cost);
    }
    Played {
        outcome,
        events: read_events(&log_path).unwrap(),
        charged: ledger.charged(),
        sends: transport.requests().len(),
    }
}

/// Every paid attempt opens and settles its own recorded hold, in order, at the whole cap.
fn assert_paid_attempts(played: &Played, count: usize) {
    assert_attempts(played, &vec![(CAP, false); count]);
}

/// Each reservation is still the cap; each settlement records its actual charge and whether
/// the provider reported it. Every attempt has one distinct hold, even a reported-free one.
fn assert_attempts(played: &Played, settlements: &[(Usd, bool)]) {
    let budget: Vec<_> = played
        .events
        .iter()
        .filter(|event| event["event"] == "budget_reserved" || event["event"] == "budget_settled")
        .collect();
    assert_eq!(budget.len(), settlements.len() * 2);
    for (index, pair) in budget.as_chunks::<2>().0.iter().enumerate() {
        let hold = json!(format!("choose#1/inv.{}", index + 1));
        assert_eq!(pair[0]["event"], "budget_reserved");
        assert_eq!(pair[0]["node_id"], hold);
        assert_eq!(pair[0]["amount_usd"], json!(0.05));
        assert_eq!(pair[1]["event"], "budget_settled");
        assert_eq!(pair[1]["node_id"], hold);
        assert_eq!(pair[1]["charged_usd"], settlements[index].0.to_value());
        assert_eq!(pair[1]["reported"], settlements[index].1);
    }
    assert_eq!(
        played.charged,
        settlements.iter().map(|(cost, _)| *cost).sum()
    );
}

#[tokio::test(start_paused = true)]
async fn a_408_then_success_is_two_recorded_paid_attempts() {
    for capability in CAPABILITIES {
        let played = play(
            capability,
            Usd(100_000),
            vec![HttpResponse::new(408, Vec::new()), success(capability)],
        )
        .await;
        let answer = played.outcome.as_ref().unwrap();
        assert!(!answer.cached);
        assert_eq!(answer.charged, Usd(100_000));
        assert_eq!(played.sends, 2);
        assert_paid_attempts(&played, 2);
    }
}

#[tokio::test(start_paused = true)]
async fn a_408_spends_the_only_hold_and_refuses_a_retry_before_sending() {
    for capability in CAPABILITIES {
        let played = play(capability, CAP, vec![HttpResponse::new(408, Vec::new())]).await;
        let Err(CallError::Rpc(error)) = &played.outcome else {
            panic!("the retry should exceed the ceiling: {:?}", played.outcome);
        };
        assert_eq!(error.code, ErrorCode::CeilingExceeded.code());
        let data = error.data.as_ref().unwrap();
        assert_eq!(data["needed_usd"], json!(0.05));
        assert_eq!(data["remaining_usd"], json!(0));
        assert_eq!(played.sends, 1);
        assert_paid_attempts(&played, 1);
        let refused: Vec<_> = played
            .events
            .iter()
            .filter(|event| event["event"] == "budget_refused")
            .collect();
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0]["node_id"], "choose#1/inv.2");
    }
}

#[tokio::test(start_paused = true)]
async fn six_408s_end_the_call_after_six_paid_attempts() {
    for capability in CAPABILITIES {
        let played = play(
            capability,
            Usd(1_000_000),
            (0..6).map(|_| HttpResponse::new(408, Vec::new())).collect(),
        )
        .await;
        let Err(CallError::Rpc(error)) = &played.outcome else {
            panic!("the retry limit should end the call: {:?}", played.outcome);
        };
        assert_eq!(error.code, ErrorCode::CallFailed.code());
        assert!(error.message.contains("HTTP 408"));
        assert_eq!(played.sends, 6);
        assert_paid_attempts(&played, 6);
    }
}

#[tokio::test(start_paused = true)]
async fn a_429_then_success_keeps_one_recorded_paid_attempt() {
    for capability in CAPABILITIES {
        let played = play(
            capability,
            CAP,
            vec![HttpResponse::new(429, Vec::new()), success(capability)],
        )
        .await;
        let answer = played.outcome.as_ref().unwrap();
        assert_eq!(answer.charged, CAP);
        assert_eq!(played.sends, 2);
        assert_paid_attempts(&played, 1);
    }
}

#[tokio::test(start_paused = true)]
async fn opted_in_cost_is_recorded_and_settled_with_fractional_micros_rounded_up() {
    for capability in CAPABILITIES {
        for (reported, charged) in [
            (json!(0.012345), Usd(12_345)),
            (json!(0.0000001), Usd(1)),
            (json!(0.00012345), Usd(124)),
        ] {
            let played = play_with_contract(
                capability,
                CAP,
                vec![success_with_cost(capability, reported)],
                json!({"billing": {"cost": "usage.cost"}}),
            )
            .await;
            let answer = played.outcome.as_ref().unwrap();
            assert_eq!(answer.cost, Some(charged));
            assert_eq!(answer.charged, charged);
            assert_eq!(played.sends, 1);
            assert_attempts(&played, &[(charged, true)]);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_documented_unbilled_400_settles_at_zero_and_is_not_retried() {
    for capability in CAPABILITIES {
        let played = play_with_contract(
            capability,
            CAP,
            vec![HttpResponse::json(
                400,
                &json!({"error": {"code": "invalid_request"}}),
            )],
            json!({"billing": {"cost": "usage.cost", "unbilled_statuses": [400]}}),
        )
        .await;
        let Err(CallError::Rpc(error)) = &played.outcome else {
            panic!("a deterministic refusal must fail: {:?}", played.outcome);
        };
        assert_eq!(error.code, ErrorCode::CallFailed.code());
        assert!(error.message.contains("HTTP 400"));
        assert_eq!(played.sends, 1);
        assert_attempts(&played, &[(Usd::ZERO, true)]);
    }
}

#[tokio::test(start_paused = true)]
async fn a_charged_429_then_success_is_two_separately_settled_attempts() {
    for capability in CAPABILITIES {
        let played = play_with_contract(
            capability,
            Usd(100_000),
            vec![
                HttpResponse::json(429, &json!({"usage": {"cost": 0.01}})),
                success_with_cost(capability, json!(0.0000001)),
            ],
            json!({"billing": {"cost": "usage.cost"}}),
        )
        .await;
        let answer = played.outcome.as_ref().unwrap();
        assert_eq!(answer.cost, Some(Usd(1)));
        assert_eq!(answer.charged, Usd(10_001));
        assert_eq!(played.sends, 2);
        assert_attempts(&played, &[(Usd(10_000), true), (Usd(1), true)]);
    }
}

#[tokio::test(start_paused = true)]
async fn a_charged_429_needs_a_new_hold_that_must_fit_the_remaining_ceiling() {
    for capability in CAPABILITIES {
        let played = play_with_contract(
            capability,
            CAP,
            vec![HttpResponse::json(
                429,
                &json!({"usage": {"cost": 0.0000001}}),
            )],
            json!({"billing": {"cost": "usage.cost"}}),
        )
        .await;
        let Err(CallError::Rpc(error)) = &played.outcome else {
            panic!("the retry should exceed the ceiling: {:?}", played.outcome);
        };
        assert_eq!(error.code, ErrorCode::CeilingExceeded.code());
        let data = error.data.as_ref().unwrap();
        assert_eq!(data["needed_usd"], json!(0.05));
        assert_eq!(data["remaining_usd"], json!(0.049999));
        assert_eq!(played.sends, 1);
        assert_attempts(&played, &[(Usd(1), true)]);
    }
}

#[test]
fn explicit_wire_and_billing_settings_change_identity_without_changing_behavior() {
    for capability in CAPABILITIES {
        let original = route(capability, json!({}));
        assert_eq!(original.contract["adapter_behavior"], 1);
        for settings in [
            json!({"billing": {"cost": "usage.cost"}}),
            json!({"billing": {"unbilled_statuses": [400]}}),
            json!({"image_detail": "high"}),
            json!({"token_limit_field": "max_completion_tokens"}),
        ] {
            let opted_in = route(capability, settings);
            assert_eq!(opted_in.contract["adapter_behavior"], 1);
            assert_ne!(opted_in.fingerprint(), original.fingerprint());
        }
    }
    let original = route("structured.generate", json!({}));
    let unchanged_pictures = route("structured.generate", json!({"pictures": "unchanged"}));
    assert_ne!(unchanged_pictures.fingerprint(), original.fingerprint());
}
