//! Explicit OpenAI-compatible wire choices over synthetic exchanges. No provider is contacted.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use grida_fx_core::providers::{BaseUrl, Credential, Declaration, Declarations};
use grida_fx_providers::adapter::{Adapter, Answer, RequestAdapter, Sent};
use grida_fx_providers::keys::{Environment, Keys};
use grida_fx_providers::openai_compatible::{self, agent, structured};
use grida_fx_providers::registry::Adapters;
use grida_fx_providers::setup::{Endpoints, Setup};
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, media, setup};
use grida_fx_providers::transport::replay::{Expect, ExpectBody, ReplayTransport};
use grida_fx_providers::transport::{Body, HttpResponse};
use serde_json::{Value, json};
use std::sync::Arc;

const KEY: &str = "test-acme-key";
const ROUTE: &str = "text-a@acme";
const URL: &str = "https://api.acme.test/v1/chat/completions";
const CAPABILITIES: [&str; 2] = [structured::CAPABILITY, agent::CAPABILITY];

fn registry(transport: Arc<ReplayTransport>, keyed: bool) -> Adapters {
    let declarations = Declarations::with(&[Declaration {
        name: "acme".into(),
        label: "acme".into(),
        credential: Some(Credential {
            variable: "ACME_API_KEY".into(),
            header: "authorization".into(),
            scheme: "Bearer".into(),
        }),
        base_url: BaseUrl {
            variable: None,
            default: "https://api.acme.test/v1".into(),
        },
        download_hosts: Vec::new(),
        family: Some(openai_compatible::FAMILY.into()),
        adapter: None,
    }]);
    let pairs: &[(&str, &str)] = if keyed { &[("ACME_API_KEY", KEY)] } else { &[] };
    let environment = Environment::declared(pairs, &declarations);
    let keys = Keys::from_environment(&environment);
    let endpoints = Endpoints::from_environment(&environment, &keys).unwrap();
    grida_fx_providers::live::adapters(&Setup {
        endpoints,
        ..setup(transport, keys)
    })
}

fn serving(adapters: &Adapters, call: &TestCall) -> Arc<dyn RequestAdapter> {
    match adapters.serving(&call.route) {
        Some(Adapter::Request(adapter)) => Arc::clone(adapter),
        other => panic!("expected the family's request adapter: {other:?}"),
    }
}

fn base_contract(capability: &str) -> Value {
    let adapter = if capability == structured::CAPABILITY {
        structured::CONTRACT_ADAPTER
    } else {
        agent::CONTRACT_ADAPTER
    };
    json!({"adapter": adapter, "adapter_behavior": 1})
}

fn base_request(capability: &str) -> Value {
    if capability == structured::CAPABILITY {
        json!({
            "prompt": "Choose.",
            "schema": {"type": "object", "properties": {"ok": {"type": "boolean"}},
                "required": ["ok"], "additionalProperties": false}
        })
    } else {
        json!({
            "system": "Judge the choice.",
            "messages": [{"role": "user", "content": "Choose."}],
            "tools": [{"name": "choose", "description": "Choose an answer.",
                "parameters": {"type": "object", "properties": {}}}],
            "tool_choice": "auto"
        })
    }
}

fn make_call(capability: &str, contract: Value, request: Value) -> TestCall {
    CallBuilder::new(capability, ROUTE)
        .contract(contract)
        .request(request)
        .build()
}

fn exchange() -> grida_fx_providers::transport::replay::Exchange {
    Expect::post_json(URL, Value::Null)
        .credential("authorization", &format!("Bearer {KEY}"))
        .body(ExpectBody::Any)
        .reply(HttpResponse::json(
            200,
            &json!({"choices": [{"message": {"role": "assistant", "content": "{\"ok\":true}"}}]}),
        ))
}

fn answered(sent: Sent) -> Answer {
    match sent {
        Sent::Answered(answer) => answer,
        other => panic!("expected an answer: {other:?}"),
    }
}

fn sent_body(transport: &ReplayTransport, index: usize) -> Value {
    match &transport.requests()[index].body {
        Body::Json(body) => body.clone(),
        other => panic!("expected a JSON body: {other:?}"),
    }
}

fn send(call: &TestCall) -> Arc<ReplayTransport> {
    let transport = Arc::new(ReplayTransport::new(vec![exchange()]));
    answered(block_on(
        serving(&registry(transport.clone(), true), call).send(call),
    ));
    transport.assert_done();
    transport
}

#[test]
fn the_selected_token_field_carries_the_same_limit_for_both_capabilities() {
    for capability in CAPABILITIES {
        let mut contract = base_contract(capability);
        contract["token_limit_field"] = json!("max_completion_tokens");
        let mut request = base_request(capability);
        request["max_tokens"] = json!(37);
        let call = make_call(capability, contract, request);
        let transport = send(&call);
        let body = sent_body(&transport, 0);
        assert_eq!(body["max_completion_tokens"], json!(37));
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("token_limit_field").is_none());
        if capability == structured::CAPABILITY {
            assert_eq!(
                body["response_format"]["json_schema"]["strict"],
                json!(true)
            );
            assert_eq!(transport.requests()[0].timeout, structured::DEADLINE);
        } else {
            assert_eq!(body["tools"][0]["function"]["strict"], json!(true));
            assert_eq!(transport.requests()[0].timeout, agent::DEADLINE);
        }
        assert_eq!(
            transport.requests()[0].max_response_bytes,
            openai_compatible::MAX_RESPONSE_BYTES
        );
    }
}

#[test]
fn explicit_default_field_keeps_the_original_bytes_and_agent_limits_stay_optional() {
    for capability in CAPABILITIES {
        let transport = Arc::new(ReplayTransport::new(vec![
            exchange(),
            exchange(),
            exchange(),
            exchange(),
        ]));
        let adapters = registry(transport.clone(), true);
        for field in [
            None,
            Some("max_tokens"),
            Some("max_completion_tokens"),
            Some("null"),
        ] {
            let mut contract = base_contract(capability);
            if let Some(field) = field {
                contract["token_limit_field"] = if field == "null" {
                    Value::Null
                } else {
                    json!(field)
                };
                if field == "null" {
                    contract["image_detail"] = Value::Null;
                    if capability == structured::CAPABILITY {
                        contract["pictures"] = Value::Null;
                    }
                }
            }
            let call = make_call(capability, contract, base_request(capability));
            answered(block_on(serving(&adapters, &call).send(&call)));
        }
        transport.assert_done();
        assert_eq!(
            serde_json::to_vec(&sent_body(&transport, 0)).unwrap(),
            serde_json::to_vec(&sent_body(&transport, 1)).unwrap()
        );
        assert_eq!(sent_body(&transport, 0), sent_body(&transport, 3));
        let selected = sent_body(&transport, 2);
        if capability == structured::CAPABILITY {
            assert_eq!(selected["max_completion_tokens"], json!(16_000));
            assert!(selected.get("max_tokens").is_none());
        } else {
            assert!(selected.get("max_tokens").is_none());
            assert!(selected.get("max_completion_tokens").is_none());
        }
    }
}

#[test]
fn structured_context_can_keep_original_jpeg_and_alpha_bytes_with_each_detail() {
    let jpeg = media::jpeg(1700, 2);
    let alpha = media::png(2, 2, Some(7));
    for detail in ["auto", "low", "high", "original"] {
        let mut contract = base_contract(structured::CAPABILITY);
        contract["pictures"] = json!("unchanged");
        contract["image_detail"] = json!(detail);
        let mut builder = CallBuilder::new(structured::CAPABILITY, ROUTE).contract(contract);
        let first = builder.file("image/jpeg", &jpeg);
        let second = builder.file("image/png", &alpha);
        let notes = builder.file("text/plain", b"keep these colours");
        let mut request = base_request(structured::CAPABILITY);
        request["system"] = json!("Judge the original pictures.");
        request["context"] = json!([first, second, notes]);
        let call = builder.request(request).build();
        let transport = send(&call);
        let body = sent_body(&transport, 0);
        assert_eq!(
            body["messages"][0],
            json!({"role": "system", "content": "Judge the original pictures."})
        );
        let parts = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(
            parts[0],
            json!({"type": "text", "text": "Choose.\n\n--- context 3 ---\nkeep these colours"})
        );
        for (index, kind, bytes) in [(1, "image/jpeg", &jpeg), (2, "image/png", &alpha)] {
            assert_eq!(
                parts[index],
                json!({"type": "image_url", "image_url": {
                    "url": format!("data:{kind};base64,{}", STANDARD.encode(bytes)),
                    "detail": detail
                }})
            );
        }
        assert!(body.get("pictures").is_none());
        assert!(body.get("image_detail").is_none());
        let adapter = serving(&registry(transport, true), &call);
        assert!(
            adapter
                .check(&call, &Answer::new(json!({"json": {"ok": "yes"}}), None))
                .is_err(),
            "the original schema still judges the answer"
        );
    }
}

#[test]
fn detail_alone_keeps_structured_reduction_and_applies_to_agent_and_tool_pictures() {
    let jpeg = media::jpeg(1700, 2);
    let mut contract = base_contract(structured::CAPABILITY);
    contract["image_detail"] = json!("high");
    let mut builder = CallBuilder::new(structured::CAPABILITY, ROUTE).contract(contract);
    let picture = builder.file("image/jpeg", &jpeg);
    let mut request = base_request(structured::CAPABILITY);
    request["context"] = json!([picture]);
    let call = builder.request(request).build();
    let transport = send(&call);
    let body = sent_body(&transport, 0);
    let picture = &body["messages"][0]["content"][1]["image_url"];
    assert_eq!(picture["detail"], json!("high"));
    let bytes = STANDARD
        .decode(
            picture["url"]
                .as_str()
                .unwrap()
                .strip_prefix("data:image/png;base64,")
                .unwrap(),
        )
        .unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!(decoded.width(), 1600);

    for detail in ["auto", "low", "high", "original"] {
        let mut contract = base_contract(agent::CAPABILITY);
        contract["image_detail"] = json!(detail);
        let mut builder = CallBuilder::new(agent::CAPABILITY, ROUTE).contract(contract);
        let picture = builder.file("image/jpeg", &jpeg);
        let mut request = base_request(agent::CAPABILITY);
        request["messages"] = json!([
            {"role": "user", "content": "Choose.", "images": [picture]},
            {"role": "assistant", "tool_calls": [{"id": "c1", "name": "choose", "arguments": "{}"}]},
            {"role": "tool", "name": "choose", "tool_call_id": "c1", "content": "Look.", "images": [picture]}
        ]);
        let call = builder.request(request).build();
        let transport = send(&call);
        let body = sent_body(&transport, 0);
        for message in [1, 4] {
            assert_eq!(
                body["messages"][message]["content"][1]["image_url"],
                json!({"url": format!("data:image/jpeg;base64,{}", STANDARD.encode(&jpeg)), "detail": detail})
            );
        }
    }
}

#[test]
fn malformed_wire_choices_are_contract_refusals_before_keys_or_requests() {
    for capability in CAPABILITIES {
        let mut invalid = Vec::new();
        for value in [
            json!(false),
            json!(3),
            json!({}),
            json!([]),
            json!("medium"),
        ] {
            invalid.push((
                "image_detail",
                value,
                "image_detail is one of auto, low, high, original",
            ));
        }
        for value in [
            json!(false),
            json!(3),
            json!({}),
            json!([]),
            json!("tokens"),
        ] {
            invalid.push((
                "token_limit_field",
                value,
                "token_limit_field is one of max_tokens, max_completion_tokens",
            ));
        }
        for value in [
            json!(false),
            json!(3),
            json!({}),
            json!([]),
            json!("original"),
        ] {
            let reason = if capability == structured::CAPABILITY {
                "pictures is \"unchanged\" or absent"
            } else {
                "the contract takes no member pictures"
            };
            invalid.push(("pictures", value, reason));
        }
        if capability == agent::CAPABILITY {
            invalid.push((
                "pictures",
                json!("unchanged"),
                "the contract takes no member pictures",
            ));
        }
        for (member, value, why) in invalid {
            let mut contract = base_contract(capability);
            contract[member] = value;
            let call = make_call(capability, contract, json!({"unexpected": true}));
            let transport = Arc::new(ReplayTransport::new(Vec::new()));
            let adapter = serving(&registry(transport.clone(), false), &call);
            let reason = format!("{ROUTE} is not a {capability} route this adapter serves: {why}");
            assert_eq!(adapter.check_contract(&call), Err(reason.clone()));
            assert_eq!(block_on(adapter.send(&call)), Sent::Refused { reason });
            assert!(transport.requests().is_empty());
        }
    }
}

#[test]
fn provider_options_cannot_override_either_token_field_and_reasoning_stays_provider_owned() {
    for capability in CAPABILITIES {
        for selected in ["max_tokens", "max_completion_tokens"] {
            for member in ["max_tokens", "max_completion_tokens", "messages"] {
                for in_contract in [false, true] {
                    let mut contract = base_contract(capability);
                    contract["token_limit_field"] = json!(selected);
                    let mut request = base_request(capability);
                    let options = json!({"acme": {member: 12}});
                    let where_ = if in_contract {
                        contract["provider_options"] = options;
                        "contract.provider_options"
                    } else {
                        request["provider_options"] = options;
                        "provider_options"
                    };
                    let call = make_call(capability, contract, request);
                    let transport = Arc::new(ReplayTransport::new(Vec::new()));
                    let adapter = serving(&registry(transport.clone(), true), &call);
                    assert_eq!(
                        block_on(adapter.send(&call)),
                        Sent::Refused {
                            reason: format!("{where_}.acme.{member} is a member FX owns")
                        }
                    );
                    assert!(transport.requests().is_empty());
                }
            }
        }
        let mut contract = base_contract(capability);
        contract["provider_options"] =
            json!({"acme": {"reasoning_effort": "low", "temperature": 0.2}});
        let mut request = base_request(capability);
        request["provider_options"] = json!({"acme": {"reasoning_effort": "high"}});
        let transport = send(&make_call(capability, contract, request));
        let body = sent_body(&transport, 0);
        assert_eq!(body["reasoning_effort"], json!("high"));
        assert_eq!(body["temperature"], json!(0.2));
    }
}
