//! The `openai-compatible` family over synthetic exchanges (src/openai_compatible/README.md;
//! spec/providers.md §1, §2; spec/capabilities.md §4, §5). A made-up provider, `acme`, is declared
//! with the family and a key, and `local`, a server on this machine, with the family and no key;
//! their adapters are the ones a live invocation registers from the declarations. Nothing here
//! reaches a network: every send goes to a `ReplayTransport` that asserts the request, and every
//! refusal runs over a transport that saw no request.

use grida_fx_core::providers::{BaseUrl, Credential, Declaration, Declarations};
use grida_fx_providers::adapter::{Adapter, Answer, RequestAdapter, Sent, Usage};
use grida_fx_providers::keys::{Environment, Keys};
use grida_fx_providers::live;
use grida_fx_providers::openai_compatible::{self, agent, structured};
use grida_fx_providers::registry::Adapters;
use grida_fx_providers::setup::{Endpoints, Setup};
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, media, setup};
use grida_fx_providers::transport::replay::{Exchange, Expect, ExpectBody, ReplayTransport};
use grida_fx_providers::transport::{
    Body, HttpResponse, Transport, TransportError, TransportErrorKind,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const KEY: &str = "test-acme-key";
const URL: &str = "https://api.acme.test/v1/chat/completions";
const LOCAL_URL: &str = "http://127.0.0.1:8080/v1/chat/completions";
const TEXT_ROUTE: &str = "acme/text-a@acme";

/// FX's declarations, then `acme` (a key, a base URL its own variable moves) and `local` (no key,
/// on this machine), both of the family.
fn declarations() -> Declarations {
    let declare = |name: &str, credential, variable: Option<&str>, default: &str| Declaration {
        name: name.into(),
        label: name.into(),
        credential,
        base_url: BaseUrl {
            variable: variable.map(String::from),
            default: default.into(),
        },
        download_hosts: Vec::new(),
        family: Some(openai_compatible::FAMILY.into()),
    };
    let key = Credential {
        variable: "ACME_API_KEY".into(),
        header: "authorization".into(),
        scheme: "Bearer".into(),
    };
    Declarations::with(&[
        declare(
            "acme",
            Some(key),
            Some("ACME_BASE_URL"),
            "https://api.acme.test/v1",
        ),
        declare("local", None, None, "http://127.0.0.1:8080/v1"),
    ])
}

/// A setup over `transport` whose environment gives `pairs`, as a live run reads it.
fn setup_with(transport: Arc<dyn Transport>, pairs: &[(&str, &str)]) -> Setup {
    let environment = Environment::declared(pairs, &declarations());
    let keys = Keys::from_environment(&environment);
    let endpoints = Endpoints::from_environment(&environment, &keys).expect("valid base URLs");
    Setup {
        endpoints,
        ..setup(transport, keys)
    }
}

/// The adapters a live invocation registers over `transport`, with `acme`'s key.
fn registry(transport: Arc<dyn Transport>) -> Adapters {
    live::adapters(&setup_with(transport, &[("ACME_API_KEY", KEY)]))
}

/// The registered adapter serving `call`.
fn serving(adapters: &Adapters, call: &TestCall) -> Arc<dyn RequestAdapter> {
    match adapters.serving(&call.route) {
        Some(Adapter::Request(adapter)) => Arc::clone(adapter),
        other => panic!("no request adapter serves {}: {other:?}", call.route.id()),
    }
}

fn replay(exchanges: Vec<Exchange>) -> Arc<ReplayTransport> {
    Arc::new(ReplayTransport::new(exchanges))
}

fn expect(body: ExpectBody) -> Expect {
    Expect::post_json(URL, Value::Null)
        .credential("authorization", &format!("Bearer {KEY}"))
        .body(body)
}

/// A route's contract as its table completes it (spec/identity.md §7).
fn structured_contract() -> Value {
    json!({"adapter": "openai-compatible-structured", "adapter_behavior": 1})
}

fn agent_contract() -> Value {
    json!({"adapter": "openai-compatible-tool-loop", "adapter_behavior": 1})
}

fn structured_call(route: &str, contract: Value, request: Value) -> TestCall {
    CallBuilder::new("structured.generate", route)
        .contract(contract)
        .request(request)
        .build()
}

fn turn(route: &str, contract: Value, request: Value) -> TestCall {
    CallBuilder::new("agent.turn", route)
        .contract(contract)
        .request(request)
        .build()
}

fn colour_schema() -> Value {
    json!({
        "title": "colour",
        "description": "A colour",
        "type": "object",
        "properties": {"name": {"type": "string", "minLength": 1}},
        "required": ["name"],
        "additionalProperties": false
    })
}

fn colour_request() -> Value {
    json!({"prompt": "Name a colour.", "schema": colour_schema()})
}

fn render_tool() -> Value {
    json!({
        "name": "render",
        "description": "Render one view.",
        "parameters": {"type": "object", "properties": {"view": {"type": "string"}}}
    })
}

fn completion(message: Value, usage: Value) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"id": "chatcmpl-1", "choices": [{"index": 0, "message": message, "finish_reason": "stop"}], "usage": usage}),
    )
}

fn red() -> Value {
    json!({"role": "assistant", "content": "{\"name\": \"red\"}"})
}

fn answered(sent: Sent) -> Answer {
    match sent {
        Sent::Answered(answer) => answer,
        other => panic!("expected an answer, got {other:?}"),
    }
}

fn sent_json(transport: &ReplayTransport, index: usize) -> Value {
    match &transport.requests()[index].body {
        Body::Json(value) => value.clone(),
        other => panic!("a JSON body was expected, got {other:?}"),
    }
}

/// Every refusal over a registry with `pairs`: `Refused` with this reason, and nothing was
/// requested.
fn assert_refused(call: &TestCall, pairs: &[(&str, &str)], reason: &str) {
    let transport = replay(Vec::new());
    let adapters = live::adapters(&setup_with(transport.clone(), pairs));
    assert_eq!(
        block_on(serving(&adapters, call).send(call)),
        Sent::Refused {
            reason: reason.into()
        }
    );
    assert!(transport.requests().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Registration

#[test]
fn a_declared_family_s_providers_get_both_adapters_and_nothing_else() {
    let plain = live::adapters(&setup(
        Arc::new(grida_fx_providers::transport::replay::NoNetwork),
        Keys::none(),
    ));
    let adapters = registry(Arc::new(grida_fx_providers::transport::replay::NoNetwork));
    let declared: Vec<(String, String)> = adapters.served()[plain.served().len()..].to_vec();
    let pair = |capability: &str, provider: &str| (capability.to_string(), provider.to_string());
    assert_eq!(
        declared,
        [
            pair("structured.generate", "acme"),
            pair("agent.turn", "acme"),
            pair("structured.generate", "local"),
            pair("agent.turn", "local"),
        ]
    );
    for provider in ["acme", "local"] {
        for (capability, identity, contract) in [
            (
                "structured.generate",
                structured::IDENTITY,
                structured_contract(),
            ),
            ("agent.turn", agent::IDENTITY, agent_contract()),
        ] {
            let route = format!("text-a@{provider}");
            // A completed contract, and none, are served by the family's adapter.
            for contract in [contract, json!({})] {
                let call = CallBuilder::new(capability, &route)
                    .contract(contract)
                    .build();
                assert_eq!(serving(&adapters, &call).identity(), Some(identity));
            }
            // A contract naming another adapter is served by none.
            let other = CallBuilder::new(capability, &route)
                .contract(json!({"adapter": "acme-chat"}))
                .build();
            assert!(adapters.serving(&other.route).is_none());
        }
        let image = CallBuilder::new("image.generate", &format!("img-a@{provider}")).build();
        assert!(adapters.serving(&image.route).is_none());
    }
}

// ---------------------------------------------------------------------------------------------
// Structured output

#[test]
fn a_declared_provider_answers_structured_output() {
    let exact = r#"{"model":"acme/text-a","messages":[{"role":"user","content":"Name a colour."}],"response_format":{"type":"json_schema","json_schema":{"name":"colour","strict":true,"schema":{"title":"colour","description":"A colour","type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false},"description":"A colour"}},"max_tokens":16000}"#;
    let transport = replay(vec![expect(ExpectBody::JsonText(exact.into())).reply(
        completion(
            red(),
            json!({"prompt_tokens": 9, "completion_tokens": 3, "prompt_tokens_details": {"cached_tokens": 2}, "cost": 0.5}),
        ),
    )]);
    let call = structured_call(TEXT_ROUTE, structured_contract(), colour_request());
    let adapter = serving(&registry(transport.clone()), &call);
    let answer = answered(block_on(adapter.send(&call)));
    transport.assert_done();
    assert_eq!(answer.data, json!({"json": {"name": "red"}}));
    assert!(answer.files.is_empty());
    // No cost is read, a reported one included: the engine charges the whole hold.
    assert_eq!(answer.cost, None);
    assert_eq!(
        answer.usage.as_deref(),
        Some(&Usage {
            input_tokens: Some(9),
            cached_input_tokens: Some(2),
            output_tokens: Some(3),
            ..Usage::default()
        })
    );
    assert_eq!(adapter.check(&call, &answer), Ok(()));
    let request = &transport.requests()[0];
    assert_eq!(request.timeout, Duration::from_secs(1800));
    assert_eq!(
        request.max_response_bytes,
        openai_compatible::MAX_RESPONSE_BYTES
    );
    // The check holds the value to the request's original schema.
    let blank = Answer::new(json!({"json": {"name": ""}}), None);
    assert!(adapter.check(&call, &blank).is_err());
}

#[test]
fn pictures_and_context_are_sent_as_on_openrouter_without_detail() {
    let picture = media::png(2, 2, Some(255));
    let mut builder =
        CallBuilder::new("structured.generate", TEXT_ROUTE).contract(structured_contract());
    let image = builder.file("image/png", &picture);
    let notes = builder.file("text/plain", b"warm tones");
    let call = builder
        .request(
            json!({"prompt": "Name a colour.", "schema": colour_schema(), "system": "Be brief.",
                        "context": [image, notes], "max_tokens": 50}),
        )
        .build();
    let transport = replay(vec![
        expect(ExpectBody::Any).reply(completion(red(), json!({}))),
    ]);
    answered(block_on(
        serving(&registry(transport.clone()), &call).send(&call),
    ));
    let sent = sent_json(&transport, 0);
    assert_eq!(sent["max_tokens"], json!(50));
    assert_eq!(
        sent["messages"][0],
        json!({"role": "system", "content": "Be brief."})
    );
    let parts = sent["messages"][1]["content"].as_array().unwrap();
    assert_eq!(
        parts[0],
        json!({"type": "text", "text": "Name a colour.\n\n--- context 2 ---\nwarm tones"})
    );
    assert_eq!(parts[1]["type"], json!("image_url"));
    let url = parts[1]["image_url"]["url"].as_str().unwrap();
    assert!(url.starts_with("data:image/png;base64,"), "{url}");
    assert_eq!(
        parts[1]["image_url"].as_object().unwrap().len(),
        1,
        "no detail"
    );
    let members: Vec<&str> = sent
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        members,
        ["model", "messages", "response_format", "max_tokens"]
    );
}

// ---------------------------------------------------------------------------------------------
// Agent turns

#[test]
fn a_declared_provider_answers_an_agent_turn() {
    let exact = concat!(
        r#"{"model":"acme/text-a","messages":["#,
        r#"{"role":"system","content":"Judge the subject."},"#,
        r#"{"role":"user","content":"Look."},"#,
        r#"{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"render","arguments":"{\"view\":\"back\"}"}}]},"#,
        r#"{"role":"tool","tool_call_id":"c1","content":"back"}],"#,
        r#""tools":[{"type":"function","function":{"name":"render","description":"Render one view.","parameters":{"type":"object","properties":{"view":{"type":"string"}},"required":["view"],"additionalProperties":false},"strict":true}}],"#,
        r#""tool_choice":"required","max_tokens":4000}"#
    );
    let transport = replay(vec![expect(ExpectBody::JsonText(exact.into())).reply(
        completion(
            json!({"role": "assistant", "content": null, "tool_calls": [
                {"id": "c2", "type": "function", "function": {"name": "render", "arguments": "{\"view\":\"side\"}"}}
            ]}),
            json!({"prompt_tokens": 40, "completion_tokens": 7}),
        ),
    )]);
    let call = turn(
        TEXT_ROUTE,
        agent_contract(),
        json!({
            "system": "Judge the subject.",
            "messages": [
                {"role": "user", "content": "Look."},
                {"role": "assistant", "content": "", "tool_calls": [
                    {"id": "c1", "name": "render", "arguments": "{\"view\":\"back\"}"}
                ]},
                {"role": "tool", "name": "render", "tool_call_id": "c1", "content": "back"}
            ],
            "tools": [render_tool()],
            "tool_choice": "required",
            "max_tokens": 4000
        }),
    );
    let adapter = serving(&registry(transport.clone()), &call);
    let answer = answered(block_on(adapter.send(&call)));
    transport.assert_done();
    assert_eq!(
        answer.data,
        json!({"text": "", "tool_calls": [{"id": "c2", "name": "render", "arguments": {"view": "side"}}]})
    );
    assert_eq!(answer.cost, None);
    assert_eq!(answer.usage.and_then(|usage| usage.output_tokens), Some(7));
    assert_eq!(transport.requests()[0].timeout, Duration::from_secs(600));
    // Without max_tokens and with tool_choice auto, a turn sends no limit and `auto`.
    let transport = replay(vec![expect(ExpectBody::Any).reply(completion(
        json!({"role": "assistant", "content": "done"}),
        json!({}),
    ))]);
    let call = turn(
        TEXT_ROUTE,
        agent_contract(),
        json!({"system": "", "messages": [{"role": "user", "content": "Go."}],
               "tools": [render_tool()], "tool_choice": "auto"}),
    );
    let answer = answered(block_on(
        serving(&registry(transport.clone()), &call).send(&call),
    ));
    assert_eq!(answer.data, json!({"text": "done", "tool_calls": []}));
    let sent = sent_json(&transport, 0);
    let members: Vec<&str> = sent
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(members, ["model", "messages", "tools", "tool_choice"]);
    assert_eq!(sent["tool_choice"], json!("auto"));
}

// ---------------------------------------------------------------------------------------------
// The declaration: no key, a moved base URL

#[test]
fn a_provider_declared_without_a_key_is_sent_none() {
    let transport = replay(vec![
        Expect::post_json(LOCAL_URL, Value::Null)
            .body(ExpectBody::Any)
            .reply(completion(red(), json!({}))),
    ]);
    let call = structured_call("text-a@local", structured_contract(), colour_request());
    let adapter = serving(&registry(transport.clone()), &call);
    answered(block_on(adapter.send(&call)));
    transport.assert_done();
    assert_eq!(transport.requests()[0].credential, None);
}

#[test]
fn the_declared_base_url_variable_moves_the_provider() {
    let transport = replay(vec![
        Expect::post_json(
            "https://proxy.acme.test:8443/v2/chat/completions",
            Value::Null,
        )
        .credential("authorization", &format!("Bearer {KEY}"))
        .body(ExpectBody::Any)
        .reply(completion(red(), json!({}))),
    ]);
    let setup = setup_with(
        transport.clone(),
        &[
            ("ACME_API_KEY", KEY),
            ("ACME_BASE_URL", "https://proxy.acme.test:8443/v2/"),
        ],
    );
    let call = structured_call(TEXT_ROUTE, structured_contract(), colour_request());
    answered(block_on(
        serving(&live::adapters(&setup), &call).send(&call),
    ));
    transport.assert_done();
}

// ---------------------------------------------------------------------------------------------
// Refusals before sending

#[test]
fn refusals_come_before_anything_is_sent_in_order() {
    let key = [("ACME_API_KEY", KEY)];
    let not_served = "acme/text-a@acme is not a structured.generate route this adapter serves";
    // 1. The contract.
    for contract in [
        json!({"adapter": "openai-compatible-structured", "adapter_behavior": "1"}),
        json!({"adapter": "openai-compatible-structured", "adapter_behavior": 2}),
        json!([]),
    ] {
        assert_refused(
            &structured_call(TEXT_ROUTE, contract, colour_request()),
            &key,
            not_served,
        );
    }
    for member in ["request_policy", "pictures", "image_detail"] {
        let mut contract = structured_contract();
        contract[member] = json!("high");
        assert_refused(
            &structured_call(TEXT_ROUTE, contract, colour_request()),
            &key,
            &format!("{not_served}: the contract takes no member {member}"),
        );
    }
    // 2. The capability.
    assert_refused(
        &structured_call(
            TEXT_ROUTE,
            structured_contract(),
            json!({"prompt": "x", "schema": colour_schema(), "temperature": 1}),
        ),
        &key,
        "structured.generate takes no member temperature",
    );
    // 3. The key, before the values.
    assert_refused(
        &structured_call(
            TEXT_ROUTE,
            structured_contract(),
            json!({"prompt": " ", "schema": colour_schema()}),
        ),
        &[],
        "ACME_API_KEY is not set",
    );
    assert_refused(
        &turn(
            TEXT_ROUTE,
            agent_contract(),
            json!({"system": "", "messages": [], "tools": [], "tool_choice": "required"}),
        ),
        &[],
        "ACME_API_KEY is not set",
    );
    // 4. Provider options, then the request's values.
    for (options, reason) in [
        (
            json!({"acme": {"stream": true}}),
            "provider_options.acme.stream is a member FX owns",
        ),
        (
            json!({"acme": {"max_completion_tokens": 10}}),
            "provider_options.acme.max_completion_tokens is a member FX owns",
        ),
        (json!({"acme": 1}), "provider_options.acme is an object"),
    ] {
        assert_refused(
            &structured_call(
                TEXT_ROUTE,
                structured_contract(),
                json!({"prompt": " ", "schema": colour_schema(), "provider_options": options}),
            ),
            &key,
            reason,
        );
    }
    let mut contract = agent_contract();
    contract["provider_options"] = json!({"acme": {"tools": []}});
    assert_refused(
        &turn(
            TEXT_ROUTE,
            contract,
            json!({"system": "", "messages": [{"role": "user", "content": "Go."}], "tools": [],
                   "tool_choice": "required"}),
        ),
        &key,
        "contract.provider_options.acme.tools is a member FX owns",
    );
    assert_refused(
        &structured_call(
            TEXT_ROUTE,
            structured_contract(),
            json!({"prompt": " ", "schema": colour_schema()}),
        ),
        &key,
        "a structured call needs its prompt as text",
    );
}

/// Each adapter owns every member its bodies send (spec/adapters.md §8).
#[test]
fn the_members_its_bodies_send_are_owned() {
    let transport = replay(vec![
        expect(ExpectBody::Any).reply(completion(red(), json!({}))),
        expect(ExpectBody::Any).reply(completion(
            json!({"role": "assistant", "content": "done"}),
            json!({}),
        )),
    ]);
    let adapters = registry(transport.clone());
    let call = structured_call(TEXT_ROUTE, structured_contract(), colour_request());
    answered(block_on(serving(&adapters, &call).send(&call)));
    let call = turn(
        TEXT_ROUTE,
        agent_contract(),
        json!({"system": "", "messages": [{"role": "user", "content": "Go."}],
               "tools": [render_tool()], "tool_choice": "required", "max_tokens": 10}),
    );
    answered(block_on(serving(&adapters, &call).send(&call)));
    for (index, owned) in [(0, structured::OWNED), (1, agent::OWNED)] {
        for member in sent_json(&transport, index).as_object().unwrap().keys() {
            assert!(owned.contains(&member.as_str()), "{member}");
        }
    }
}

#[test]
fn provider_options_follow_the_body_s_own_members() {
    let mut contract = structured_contract();
    contract["provider_options"] = json!({"acme": {"temperature": 0.2, "user": "route-a"}});
    let call = structured_call(
        TEXT_ROUTE,
        contract,
        json!({"prompt": "Name a colour.", "schema": colour_schema(),
               "provider_options": {"acme": {"user": "step-a", "seed": 7},
                                    "openrouter": {"model": "never read"}}}),
    );
    let transport = replay(vec![
        expect(ExpectBody::Any).reply(completion(red(), json!({}))),
    ]);
    answered(block_on(
        serving(&registry(transport.clone()), &call).send(&call),
    ));
    let sent = sent_json(&transport, 0);
    let members: Vec<&str> = sent
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        members,
        [
            "model",
            "messages",
            "response_format",
            "max_tokens",
            "temperature",
            "user",
            "seed"
        ]
    );
    assert_eq!(
        (
            &sent["model"],
            &sent["temperature"],
            &sent["user"],
            &sent["seed"]
        ),
        (
            &json!("acme/text-a"),
            &json!(0.2),
            &json!("step-a"),
            &json!(7)
        )
    );
}

// ---------------------------------------------------------------------------------------------
// Statuses and answers

#[test]
fn statuses_follow_the_default_table_and_never_cost_zero() {
    let envelope = |code: &str| json!({"error": {"code": code, "message": format!("bad {KEY}")}});
    let transport = replay(vec![
        expect(ExpectBody::Any).reply(
            HttpResponse::json(429, &envelope("rate_limit_exceeded"))
                .with_header("retry-after", "3"),
        ),
        expect(ExpectBody::Any).reply(HttpResponse::new(408, Vec::new())),
        expect(ExpectBody::Any).reply(HttpResponse::json(400, &envelope("invalid_request"))),
        expect(ExpectBody::Any).reply(HttpResponse::json(402, &envelope("insufficient_quota"))),
        expect(ExpectBody::Any).reply(HttpResponse::new(503, b"busy".to_vec())),
        expect(ExpectBody::Any).reply(HttpResponse::new(302, Vec::new())),
        expect(ExpectBody::Any).fail(TransportError::not_sent(
            TransportErrorKind::Connect,
            "connection refused",
        )),
        expect(ExpectBody::Any).fail(TransportError::after_send(
            TransportErrorKind::Timeout,
            "the deadline passed",
        )),
    ]);
    let call = structured_call(TEXT_ROUTE, structured_contract(), colour_request());
    let adapter = serving(&registry(transport.clone()), &call);
    let outcomes: Vec<Sent> = (0..8).map(|_| block_on(adapter.send(&call))).collect();
    transport.assert_done();
    let label = "acme structured generation";
    assert!(
        matches!(&outcomes[0], Sent::NotReceived { reason, retry_after }
            if reason == &format!("{label} returned HTTP 429: code=rate_limit_exceeded; retry-after 3")
                && *retry_after == Some(Duration::from_secs(3))),
        "{:?}",
        outcomes[0]
    );
    assert!(
        matches!(&outcomes[1], Sent::NotReceived { .. }),
        "{:?}",
        outcomes[1]
    );
    assert_eq!(
        outcomes[2],
        Sent::Failed {
            reason: format!("{label} returned HTTP 400: code=invalid_request"),
            cost: None,
            retryable: false,
        }
    );
    assert!(matches!(
        outcomes[3],
        Sent::Failed {
            cost: None,
            retryable: false,
            ..
        }
    ));
    assert_eq!(
        outcomes[4],
        Sent::Failed {
            reason: format!("{label} returned HTTP 503"),
            cost: None,
            retryable: true,
        }
    );
    assert_eq!(
        outcomes[5],
        Sent::Failed {
            reason: format!("{label} was redirected (HTTP 302); check ACME_BASE_URL"),
            cost: None,
            retryable: false,
        }
    );
    assert!(matches!(&outcomes[6], Sent::NotReceived { .. }));
    assert!(matches!(
        outcomes[7],
        Sent::Failed {
            cost: None,
            retryable: true,
            ..
        }
    ));
    for outcome in &outcomes {
        assert!(!format!("{outcome:?}").contains(KEY), "{outcome:?}");
    }
}

#[test]
fn a_2xx_that_is_no_answer_is_a_billed_structural_failure() {
    let transport = replay(vec![
        expect(ExpectBody::Any).reply(HttpResponse::new(200, b"not json".to_vec())),
        expect(ExpectBody::Any).reply(
            HttpResponse::json(200, &json!({"choices": []})).with_header("x-request-id", "req-7"),
        ),
        expect(ExpectBody::Any).reply(completion(
            json!({"role": "assistant", "content": ""}),
            json!({"cost": 1}),
        )),
    ]);
    let call = structured_call(TEXT_ROUTE, structured_contract(), colour_request());
    let adapter = serving(&registry(transport.clone()), &call);
    let label = "acme structured generation";
    for reason in [
        format!("{label} returned invalid JSON"),
        format!("{label} returned no message (request req-7)"),
        format!("{label} returned empty content"),
    ] {
        assert_eq!(
            block_on(adapter.send(&call)),
            Sent::Failed {
                reason,
                cost: None,
                retryable: true,
            }
        );
    }
    transport.assert_done();
}
