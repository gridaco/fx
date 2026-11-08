//! Opt-in billing over the actual family adapters and an asserted, provider-free transport.

use grida_fx_core::money::Usd;
use grida_fx_core::providers::{BaseUrl, Credential, Declaration, Declarations};
use grida_fx_providers::adapter::{Adapter, Sent};
use grida_fx_providers::keys::{Environment, Keys};
use grida_fx_providers::live;
use grida_fx_providers::openai_compatible::{self, agent, structured};
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, setup};
use grida_fx_providers::transport::replay::{Expect, ExpectBody, ReplayTransport};
use grida_fx_providers::transport::{Body, HttpResponse};
use serde_json::{Value, json};
use std::sync::Arc;

const KEY: &str = "test-acme-key";
const URL: &str = "https://api.acme.test/v1/chat/completions";
const ROUTE: &str = "text-a@acme";
const CAPABILITIES: [&str; 2] = [structured::CAPABILITY, agent::CAPABILITY];

fn declarations() -> Declarations {
    Declarations::with(&[Declaration {
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
    }])
}

fn call(capability: &str, billing: Option<Value>) -> TestCall {
    let adapter = match capability {
        structured::CAPABILITY => structured::CONTRACT_ADAPTER,
        agent::CAPABILITY => agent::CONTRACT_ADAPTER,
        _ => unreachable!(),
    };
    let mut contract = json!({"adapter": adapter, "adapter_behavior": 1});
    if let Some(billing) = billing {
        contract["billing"] = billing;
    }
    let request = if capability == structured::CAPABILITY {
        json!({
            "prompt": "Name a colour.",
            "schema": {"type": "object", "properties": {"name": {"type": "string"}},
                       "required": ["name"], "additionalProperties": false}
        })
    } else {
        json!({
            "system": "Review one view.",
            "messages": [{"role": "user", "content": "Go."}],
            "tools": [{"name": "render", "description": "Render a view.",
                       "parameters": {"type": "object", "properties": {}}}],
            "tool_choice": "auto"
        })
    };
    CallBuilder::new(capability, ROUTE)
        .contract(contract)
        .request(request)
        .build()
}

fn completion(capability: &str, usage: Value) -> HttpResponse {
    let content = if capability == structured::CAPABILITY {
        "{\"name\": \"red\"}"
    } else {
        "done"
    };
    HttpResponse::json(
        200,
        &json!({
            "choices": [{"message": {"role": "assistant", "content": content}}],
            "usage": usage
        }),
    )
}

/// Send through the registry a live run uses, while asserting the only permitted exchange.
fn exchange(capability: &str, billing: Option<Value>, response: HttpResponse) -> (Sent, Value) {
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::post_json(URL, Value::Null)
            .credential("authorization", &format!("Bearer {KEY}"))
            .body(ExpectBody::Any)
            .reply(response),
    ]));
    let environment = Environment::declared(&[("ACME_API_KEY", KEY)], &declarations());
    let adapters = live::adapters(&setup(
        transport.clone(),
        Keys::from_environment(&environment),
    ));
    let call = call(capability, billing);
    let Some(Adapter::Request(adapter)) = adapters.serving(&call.route) else {
        panic!("no family adapter serves {capability}");
    };
    let outcome = block_on(adapter.send(&call));
    transport.assert_done();
    assert!(!format!("{outcome:?}").contains(KEY));
    let Body::Json(body) = &transport.requests()[0].body else {
        panic!("expected JSON");
    };
    (outcome, body.clone())
}

#[test]
fn opted_in_answers_keep_cost_and_usage_without_changing_the_request() {
    for capability in CAPABILITIES {
        let response = completion(
            capability,
            json!({"cost": 0.00012345, "prompt_tokens": 9, "completion_tokens": 3}),
        );
        let (default, default_body) = exchange(capability, None, response.clone());
        let Sent::Answered(default) = default else {
            panic!("default call did not answer");
        };
        assert_eq!(default.cost, None);
        let (opted_in, body) = exchange(
            capability,
            Some(json!({"cost": "usage.cost", "unbilled_statuses": [400, 422]})),
            response,
        );
        let Sent::Answered(answer) = opted_in else {
            panic!("opted-in call did not answer");
        };
        assert_eq!(answer.cost, Some(Usd(124)));
        assert_eq!(answer.data, default.data);
        assert_eq!(answer.usage, default.usage);
        assert_eq!(answer.usage.unwrap().output_tokens, Some(3));
        assert_eq!(body, default_body);
    }
}

#[test]
fn answers_accept_only_finite_nonnegative_numeric_costs() {
    for capability in CAPABILITIES {
        for (cost, expected) in [
            (json!(0), Some(Usd::ZERO)),
            (json!(0.210835), Some(Usd(210_835))),
            (json!(0.0000001), Some(Usd(1))),
            (json!(null), None),
            (json!("0.2"), None),
            (json!(false), None),
            (json!(-1), None),
            (json!(1e30), None),
        ] {
            let response = completion(capability, json!({"cost": cost}));
            let (outcome, _) = exchange(capability, Some(json!({"cost": "usage.cost"})), response);
            let Sent::Answered(answer) = outcome else {
                panic!("call did not answer");
            };
            assert_eq!(answer.cost, expected, "{capability}: {cost}");
        }
    }
}

#[test]
fn structural_failures_preserve_a_cost_read_before_the_answer_failed() {
    for capability in CAPABILITIES {
        for body in [
            json!({"usage": {"cost": 0.25}, "choices": []}),
            json!({"usage": {"cost": 0.25}, "choices": [{"message": {"content": "", "tool_calls": false}}]}),
        ] {
            let (outcome, _) = exchange(
                capability,
                Some(json!({"cost": "usage.cost"})),
                HttpResponse::json(200, &body),
            );
            assert!(matches!(
                outcome,
                Sent::Failed {
                    cost: Some(Usd(250_000)),
                    retryable: true,
                    ..
                }
            ));
        }
        let (outcome, _) = exchange(
            capability,
            Some(json!({"cost": "usage.cost"})),
            HttpResponse::new(200, b"not JSON".to_vec()),
        );
        assert!(matches!(
            outcome,
            Sent::Failed {
                cost: None,
                retryable: true,
                ..
            }
        ));
    }
}

#[test]
fn only_documented_statuses_without_a_readable_charge_are_unbilled() {
    let billing = json!({"cost": "usage.cost", "unbilled_statuses": [400, 422]});
    for capability in CAPABILITIES {
        for (status, body, expected) in [
            (400, json!({}), Some(Usd::ZERO)),
            (422, json!({"usage": {"cost": null}}), Some(Usd::ZERO)),
            (401, json!({}), None),
            (400, json!({"usage": {"cost": 0.2}}), Some(Usd(200_000))),
            (400, json!({"usage": {"cost": 0}}), Some(Usd::ZERO)),
            (400, json!({"usage": {"cost": "0"}}), None),
            (422, json!({"usage": {"cost": -1}}), None),
            (400, json!([]), None),
        ] {
            let (outcome, _) = exchange(
                capability,
                Some(billing.clone()),
                HttpResponse::json(status, &body),
            );
            assert!(
                matches!(
                    outcome,
                    Sent::Failed { cost, retryable: false, .. } if cost == expected
                ),
                "{capability}, HTTP {status}: {body}"
            );
        }
        for (bytes, expected) in [(Vec::new(), Some(Usd::ZERO)), (b"bad JSON".to_vec(), None)] {
            let (outcome, _) = exchange(
                capability,
                Some(billing.clone()),
                HttpResponse::new(400, bytes),
            );
            assert!(matches!(
                outcome,
                Sent::Failed { cost, retryable: false, .. } if cost == expected
            ));
        }
    }
}

#[test]
fn refusals_without_a_cost_reader_keep_the_explicit_status_policy() {
    for capability in CAPABILITIES {
        let (outcome, _) = exchange(
            capability,
            Some(json!({"unbilled_statuses": [422]})),
            HttpResponse::json(422, &json!({"usage": {"cost": 0.5}})),
        );
        assert!(matches!(
            outcome,
            Sent::Failed {
                cost: Some(Usd::ZERO),
                retryable: false,
                ..
            }
        ));
    }
}

#[test]
fn every_billed_status_keeps_a_reported_cost_and_redacts_its_reason() {
    for capability in CAPABILITIES {
        for (status, retryable) in [
            (302, false),
            (400, false),
            (408, true),
            (429, true),
            (503, true),
        ] {
            let (outcome, _) = exchange(
                capability,
                Some(json!({"cost": "usage.cost"})),
                HttpResponse::json(
                    status,
                    &json!({
                        "usage": {"cost": 0.21},
                        "error": {"message": KEY, "code": "invalid_request"}
                    }),
                )
                .with_header("retry-after", "30")
                .with_header("x-request-id", "req-1"),
            );
            assert!(
                matches!(
                    outcome,
                    Sent::Failed { cost: Some(Usd(210_000)), retryable: actual, .. }
                        if actual == retryable
                ),
                "{capability}, HTTP {status}"
            );
        }
    }
}

#[test]
fn rate_limits_with_an_invalid_report_require_a_new_billed_attempt() {
    for capability in CAPABILITIES {
        for response in [
            HttpResponse::json(429, &json!({"usage": {"cost": "0"}})),
            HttpResponse::json(429, &json!({"usage": {"cost": -1}})),
            HttpResponse::new(429, b"not JSON".to_vec()),
        ] {
            let (outcome, _) = exchange(capability, Some(json!({"cost": "usage.cost"})), response);
            assert!(matches!(
                outcome,
                Sent::Failed {
                    cost: None,
                    retryable: true,
                    ..
                }
            ));
        }
        let (outcome, _) = exchange(
            capability,
            Some(json!({"cost": "usage.cost"})),
            HttpResponse::json(429, &json!({"usage": {"cost": 0}})),
        );
        assert!(matches!(
            outcome,
            Sent::Failed {
                cost: Some(Usd::ZERO),
                retryable: true,
                ..
            }
        ));
        for response in [
            HttpResponse::new(429, Vec::new()),
            HttpResponse::json(429, &json!({})),
            HttpResponse::json(429, &json!({"usage": {"cost": null}})),
        ] {
            let (outcome, _) = exchange(capability, Some(json!({"cost": "usage.cost"})), response);
            assert!(matches!(outcome, Sent::NotReceived { .. }));
        }
    }
}

#[test]
fn duplicate_response_fields_cannot_replace_a_possible_charge_with_an_unbilled_outcome() {
    let billing = json!({"cost": "usage.cost", "unbilled_statuses": [400]});
    for capability in CAPABILITIES {
        let content = if capability == structured::CAPABILITY {
            "{\"name\":\"red\"}"
        } else {
            "done"
        };
        let content = serde_json::to_string(content).unwrap();
        for fields in [
            r#""usage":{"cost":0.2,"cost":null}"#,
            r#""usage":{"cost":0.2},"usage":{"cost":null}"#,
            r#""usage":{"cost":0.2,"\u0063ost":null}"#,
        ] {
            let body = format!(
                "{{\"choices\":[{{\"message\":{{\"role\":\"assistant\",\"content\":{content}}}}}],{fields}}}"
            );
            for status in [200, 400, 429] {
                let (outcome, _) = exchange(
                    capability,
                    Some(billing.clone()),
                    HttpResponse::new(status, body.as_bytes().to_vec()),
                );
                match (status, outcome) {
                    (200, Sent::Answered(answer)) => assert_eq!(answer.cost, None),
                    (
                        400,
                        Sent::Failed {
                            cost: None,
                            retryable: false,
                            ..
                        },
                    )
                    | (
                        429,
                        Sent::Failed {
                            cost: None,
                            retryable: true,
                            ..
                        },
                    ) => {}
                    (_, outcome) => panic!("{capability}, HTTP {status}: {outcome:?}"),
                }
            }
        }
    }
}

#[test]
fn a_charge_read_before_the_answer_parser_refuses_deep_metadata_is_preserved() {
    let levels = 160;
    let body = format!(
        "{{\"usage\":{{\"cost\":2.5}},\"meta\":{}0{}}}",
        "[".repeat(levels),
        "]".repeat(levels)
    );
    for capability in CAPABILITIES {
        let (outcome, _) = exchange(
            capability,
            Some(json!({"cost": "usage.cost"})),
            HttpResponse::new(200, body.as_bytes().to_vec()),
        );
        assert!(matches!(
            outcome,
            Sent::Failed {
                cost: Some(Usd(2_500_000)),
                retryable: true,
                ..
            }
        ));
    }
}

#[test]
fn malformed_billing_policies_are_refused_before_the_key_or_transport() {
    for capability in CAPABILITIES {
        for billing in [
            json!(false),
            json!([]),
            json!({"unknown": true}),
            json!({"cost": "cost"}),
            json!({"cost": false}),
            json!({"unbilled_statuses": 400}),
            json!({"unbilled_statuses": [400, 400]}),
            json!({"unbilled_statuses": [399]}),
            json!({"unbilled_statuses": [500]}),
            json!({"unbilled_statuses": [408]}),
            json!({"unbilled_statuses": [429]}),
            json!({"unbilled_statuses": [400.5]}),
            json!({"unbilled_statuses": ["400"]}),
            json!({"unbilled_statuses": [true]}),
        ] {
            let transport = Arc::new(ReplayTransport::new(Vec::new()));
            let environment = Environment::declared(&[], &declarations());
            let adapters = live::adapters(&setup(
                transport.clone(),
                Keys::from_environment(&environment),
            ));
            let call = call(capability, Some(billing.clone()));
            let Some(Adapter::Request(adapter)) = adapters.serving(&call.route) else {
                panic!("no family adapter serves {capability}");
            };
            let outcome = block_on(adapter.send(&call));
            let Sent::Refused { reason } = outcome else {
                panic!("expected a refusal for {billing}");
            };
            assert!(
                reason.starts_with(&format!(
                    "{ROUTE} is not a {capability} route this adapter serves: billing"
                )),
                "{reason}"
            );
            assert!(!reason.contains("ACME_API_KEY is not set"));
            assert!(transport.requests().is_empty());
        }
    }
}
