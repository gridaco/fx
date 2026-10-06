//! OpenRouter `agent.turn` over synthetic exchanges (spec/providers.md §9.2;
//! spec/capabilities.md §5; spec/protocol.md §6.2 step 3). Nothing here reaches a network: every
//! send goes to a `ReplayTransport` that asserts the request, and every refusal runs over a
//! transport that saw no request.

use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{RequestAdapter, Sent};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::openrouter::{
    self,
    agent::{OpenRouterAgent, TOOL_PICTURES},
};
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, media, setup, test_keys};
use grida_fx_providers::transport::replay::{Exchange, Expect, ExpectBody, ReplayTransport};
use grida_fx_providers::transport::{
    Body, HttpResponse, Offline, Transport, TransportError, TransportErrorKind,
};
use grida_fx_providers::wire;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const KEY: &str = "test-openrouter-key";
const URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const ASTRA_ROUTE: &str = "openai/gpt-6-astra@openrouter";
const TEXT_ROUTE: &str = "openai/gpt-5.6-sol@openrouter";

fn astra_contract() -> Value {
    json!({
        "adapter": "openrouter-tool-loop",
        "adapter_behavior": 1,
        "request_policy": {
            "provider": {"require_parameters": true, "only": ["openai"], "allow_fallbacks": false},
            "reasoning": {"effort": "high"},
            "image_detail": "high"
        }
    })
}

fn text_contract() -> Value {
    json!({"adapter": "openrouter-tool-loop", "adapter_behavior": 1})
}

fn adapter_over(transport: Arc<dyn Transport>, keys: Keys) -> OpenRouterAgent {
    OpenRouterAgent::new(openrouter::client(&setup(transport, keys)))
}

fn replay(exchanges: Vec<Exchange>) -> Arc<ReplayTransport> {
    Arc::new(ReplayTransport::new(exchanges))
}

fn expect(body: ExpectBody) -> Expect {
    Expect::post_json(URL, Value::Null)
        .credential("authorization", &format!("Bearer {KEY}"))
        .body(body)
}

fn render_tool() -> Value {
    json!({
        "name": "render",
        "description": "Render one view.",
        "parameters": {"type": "object", "properties": {"view": {"type": "string"}}}
    })
}

fn turn(route: &str, contract: Value, request: Value) -> TestCall {
    CallBuilder::new("agent.turn", route)
        .contract(contract)
        .request(request)
        .build()
}

fn simple_request() -> Value {
    json!({
        "system": "",
        "messages": [{"role": "user", "content": "Go."}],
        "tools": [render_tool()],
        "tool_choice": "required"
    })
}

fn reply(message: Value, usage: Value) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"choices": [{"message": message, "finish_reason": "tool_calls"}], "usage": usage}),
    )
}

fn sent_json(transport: &ReplayTransport, index: usize) -> Value {
    match &transport.requests()[index].body {
        Body::Json(value) => value.clone(),
        other => panic!("a JSON body was expected, got {other:?}"),
    }
}

/// Every refusal: `Refused` with this reason, and nothing was requested.
fn assert_refused(call: &TestCall, keys: Keys, reason: &str) {
    let transport = replay(Vec::new());
    assert_eq!(
        block_on(adapter_over(transport.clone(), keys).send(call)),
        Sent::Refused {
            reason: reason.into()
        }
    );
    assert!(transport.requests().is_empty());
}

// ---------------------------------------------------------------------------------------------
// A1: the reference transcript on the astra route

#[test]
fn a1_the_transcript_maps_to_the_wire() {
    let picture = media::png(1, 1, Some(255));
    let mut builder = CallBuilder::new("agent.turn", ASTRA_ROUTE).contract(astra_contract());
    let file = builder.file("image/png", &picture);
    let call = builder
        .request(json!({
            "system": "Judge the subject.",
            "messages": [
                {"role": "user", "content": "Look.", "images": [file]},
                {"role": "assistant", "content": "", "tool_calls": [
                    {"id": "c1", "name": "render", "arguments": "{\"view\":\"back\"}"}
                ]},
                {"role": "tool", "name": "render", "tool_call_id": "c1", "content": "back", "images": [file]}
            ],
            "tools": [render_tool()],
            "tool_choice": "required",
            "max_tokens": 4000
        }))
        .build();
    let url = wire::data_url("image/png", &picture);
    let exact = format!(
        concat!(
            r#"{{"model":"openai/gpt-6-astra","messages":["#,
            r#"{{"role":"system","content":"Judge the subject."}},"#,
            r#"{{"role":"user","content":[{{"type":"text","text":"Look."}},{{"type":"image_url","image_url":{{"url":"{url}","detail":"high"}}}}]}},"#,
            r#"{{"role":"assistant","content":null,"tool_calls":[{{"id":"c1","type":"function","function":{{"name":"render","arguments":"{{\"view\":\"back\"}}"}}}}]}},"#,
            r#"{{"role":"tool","tool_call_id":"c1","content":"back"}},"#,
            r#"{{"role":"user","content":[{{"type":"text","text":"Pictures the tools returned."}},{{"type":"image_url","image_url":{{"url":"{url}","detail":"high"}}}}]}}],"#,
            r#""tools":[{{"type":"function","function":{{"name":"render","description":"Render one view.","parameters":{{"type":"object","properties":{{"view":{{"type":"string"}}}},"required":["view"],"additionalProperties":false}},"strict":true}}}}],"#,
            r#""tool_choice":"required","#,
            r#""provider":{{"require_parameters":true,"only":["openai"],"allow_fallbacks":false}},"#,
            r#""reasoning":{{"effort":"high"}},"max_tokens":4000}}"#
        ),
        url = url
    );
    let transport = replay(vec![expect(ExpectBody::JsonText(exact)).reply(reply(
        json!({"role": "assistant", "content": null, "tool_calls": [
            {"id": "c2", "type": "function", "function": {"name": "submit", "arguments": "{\"verdict\":\"accept\"}"}}
        ]}),
        json!({"cost": 0.02}),
    ))]);
    let adapter = adapter_over(transport.clone(), test_keys());
    let Sent::Answered(answer) = block_on(adapter.send(&call)) else {
        panic!("an answer was expected")
    };
    transport.assert_done();
    assert_eq!(
        answer.data,
        json!({"text": "", "tool_calls": [{"id": "c2", "name": "submit", "arguments": {"verdict": "accept"}}]})
    );
    assert!(answer.files.is_empty());
    assert_eq!(answer.cost, Some(Usd(20_000)));
    assert_eq!(adapter.check(&call, &answer), Ok(()));
    let request = &transport.requests()[0];
    assert_eq!(request.timeout, Duration::from_secs(600));
    assert_eq!(
        request.max_response_bytes,
        openrouter::MAX_CHAT_RESPONSE_BYTES
    );
    let body = sent_json(&transport, 0);
    let roles: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["system", "user", "assistant", "tool", "user"]);
    assert_eq!(
        body["messages"][4]["content"][0]["text"],
        json!(TOOL_PICTURES)
    );
}

#[test]
fn arguments_text_passes_byte_for_byte_and_objects_go_compact() {
    let call = turn(
        TEXT_ROUTE,
        text_contract(),
        json!({
            "system": "s",
            "messages": [
                {"role": "user", "content": "go"},
                {"role": "assistant", "content": "thinking", "tool_calls": [
                    {"id": "a", "name": "render", "arguments": "{ \"view\" : \"back\", \"z\": 1.50 }"},
                    {"id": "b", "name": "render", "arguments": {"view": "é", "n": [1, 2]}},
                    {"id": "c", "name": "render"}
                ]},
                {"role": "tool", "tool_call_id": "a", "content": "ok"},
                {"role": "tool", "tool_call_id": "b", "content": "ok"},
                {"role": "tool", "tool_call_id": "c", "content": "ok"}
            ],
            "tools": [render_tool()],
            "tool_choice": "auto"
        }),
    );
    let transport = replay(vec![
        expect(ExpectBody::Any).reply(reply(json!({"content": "done"}), json!({}))),
    ]);
    let Sent::Answered(answer) = block_on(adapter_over(transport.clone(), test_keys()).send(&call))
    else {
        panic!("an answer was expected")
    };
    assert_eq!(answer.data, json!({"text": "done", "tool_calls": []}));
    assert_eq!(answer.cost, None);
    let body = sent_json(&transport, 0);
    let calls = &body["messages"][2]["tool_calls"];
    assert_eq!(
        calls[0]["function"]["arguments"],
        json!("{ \"view\" : \"back\", \"z\": 1.50 }")
    );
    assert_eq!(
        calls[1]["function"]["arguments"],
        json!("{\"view\":\"é\",\"n\":[1,2]}")
    );
    assert_eq!(calls[2]["function"]["arguments"], json!("{}"));
    assert_eq!(body["messages"][2]["content"], json!("thinking"));
    assert_eq!(body["tool_choice"], json!("auto"));
}

// ---------------------------------------------------------------------------------------------
// A2: the text-model route

#[test]
fn a2_the_text_model_route_sends_the_provider_defaults() {
    let picture = media::png(2, 2, Some(255));
    let mut builder = CallBuilder::new("agent.turn", TEXT_ROUTE).contract(text_contract());
    let image = builder.file("image/png", &picture);
    let odd = builder.file("application/octet-stream", b"opaque bytes");
    let call = builder
        .request(json!({
            "system": "Place the props.",
            "messages": [{"role": "user", "content": "Here.", "images": [image, odd]}],
            "tools": [render_tool()],
            "tool_choice": "something else",
            "max_tokens": null
        }))
        .build();
    let expected = json!({
        "model": "openai/gpt-5.6-sol",
        "messages": [
            {"role": "system", "content": "Place the props."},
            {"role": "user", "content": [
                {"type": "text", "text": "Here."},
                {"type": "image_url", "image_url": {"url": wire::data_url("image/png", &picture)}},
                {"type": "image_url", "image_url": {"url": wire::data_url("image/png", b"opaque bytes")}}
            ]}
        ],
        "tools": [{"type": "function", "function": {
            "name": "render", "description": "Render one view.",
            "parameters": {"type": "object", "properties": {"view": {"type": "string"}},
                "required": ["view"], "additionalProperties": false},
            "strict": true
        }}],
        "tool_choice": "required",
        "provider": {"require_parameters": true}
    });
    let transport = replay(vec![expect(ExpectBody::Json(expected)).reply(reply(
        json!({"content": "ok", "tool_calls": null}),
        json!({"cost": 0.003}),
    ))]);
    let Sent::Answered(answer) = block_on(adapter_over(transport.clone(), test_keys()).send(&call))
    else {
        panic!("an answer was expected")
    };
    transport.assert_done();
    assert_eq!(answer.cost, Some(Usd(3_000)));
    let body = sent_json(&transport, 0);
    assert!(body.get("reasoning").is_none());
    assert!(body.get("max_tokens").is_none());
}

// ---------------------------------------------------------------------------------------------
// A3: replies

#[test]
fn a3_replies_parse_and_malformed_calls_fail() {
    let good = reply(
        json!({
            "content": [{"type": "text", "text": "Looking."}],
            "tool_calls": [{"id": "call_1", "type": "function",
                "function": {"name": "render", "arguments": "{\"scale\": 0.4}"}}]
        }),
        json!({"total_tokens": 42}),
    )
    .with_header("x-request-id", "req-1");
    let malformed = [
        json!({"content": "x", "tool_calls": "nope"}),
        json!({"content": "x", "tool_calls": [{"id": "", "function": {"name": "render"}}]}),
        json!({"content": "x", "tool_calls": [{"id": "c", "function": {"name": "render", "arguments": "{"}}]}),
        json!({"content": "x", "tool_calls": [{"id": "c", "function": {"name": "render", "arguments": "[1]"}}]}),
        json!({"content": "x", "tool_calls": [{"id": "c", "function": {"name": "render", "arguments": "{\"scale\": NaN}"}}]}),
    ];
    let mut exchanges = vec![expect(ExpectBody::Any).reply(good)];
    for message in &malformed {
        exchanges
            .push(expect(ExpectBody::Any).reply(reply(message.clone(), json!({"cost": 0.001}))));
    }
    exchanges
        .push(expect(ExpectBody::Any).reply(HttpResponse::json(200, &json!({"choices": [{}]}))));
    exchanges.push(expect(ExpectBody::Any).reply(HttpResponse::json(200, &json!([1]))));
    let transport = replay(exchanges);
    let adapter = adapter_over(transport.clone(), test_keys());
    let call = turn(TEXT_ROUTE, text_contract(), simple_request());
    let Sent::Answered(answer) = block_on(adapter.send(&call)) else {
        panic!("an answer was expected")
    };
    assert_eq!(
        answer.data,
        json!({"text": "Looking.", "tool_calls": [{"id": "call_1", "name": "render", "arguments": {"scale": 0.4}}]})
    );
    assert_eq!(
        answer.data["tool_calls"][0]["arguments"],
        json!({"scale": 0.4})
    );
    assert_eq!(answer.cost, None);
    let reasons = [
        "OpenRouter tool loop returned malformed tool calls",
        "OpenRouter tool loop returned a tool call without an id or name",
        "OpenRouter tool loop returned invalid JSON tool arguments",
        "OpenRouter tool loop returned non-object tool arguments",
        "OpenRouter tool loop returned invalid JSON tool arguments",
    ];
    for expected in reasons {
        assert_eq!(
            block_on(adapter.send(&call)),
            Sent::Failed {
                reason: expected.into(),
                cost: Some(Usd(1_000)),
                retryable: true
            }
        );
    }
    assert_eq!(
        block_on(adapter.send(&call)),
        Sent::Failed {
            reason: "OpenRouter tool loop returned no message".into(),
            cost: None,
            retryable: true
        }
    );
    assert_eq!(
        block_on(adapter.send(&call)),
        Sent::Failed {
            reason: "OpenRouter tool loop returned a non-object JSON response".into(),
            cost: None,
            retryable: true
        }
    );
    transport.assert_done();
}

// ---------------------------------------------------------------------------------------------
// A4: empty assistant messages, ids, tool names

#[test]
fn a4_an_empty_assistant_message_sends_null_content_and_no_calls() {
    let call = turn(
        TEXT_ROUTE,
        text_contract(),
        json!({
            "system": "s",
            "messages": [
                {"role": "user", "content": "go"},
                {"role": "assistant", "content": "", "tool_calls": []},
                {"role": "user", "content": "Finish by calling submit."}
            ],
            "tools": [render_tool()],
            "tool_choice": "required",
            "max_tokens": 10
        }),
    );
    let transport = replay(vec![
        expect(ExpectBody::Any).reply(reply(json!({"content": "fine"}), json!({}))),
    ]);
    block_on(adapter_over(transport.clone(), test_keys()).send(&call));
    let body = sent_json(&transport, 0);
    assert_eq!(
        serde_json::to_string(&body["messages"][2]).unwrap(),
        r#"{"role":"assistant","content":null}"#
    );
    assert_eq!(body["max_tokens"], json!(10));
}

#[test]
fn a4_turns_that_cannot_be_sent_are_refused() {
    let with = |messages: Value| {
        turn(
            TEXT_ROUTE,
            text_contract(),
            json!({"system": "", "messages": messages, "tools": [render_tool()], "tool_choice": "required"}),
        )
    };
    assert_refused(
        &with(json!([{"role": "tool", "name": "render", "content": "back"}])),
        test_keys(),
        "a tool message without a tool_call_id cannot be sent",
    );
    assert_refused(
        &with(json!([{"role": "tool", "name": "render", "tool_call_id": null, "content": "back"}])),
        test_keys(),
        "a tool message without a tool_call_id cannot be sent",
    );
    assert_refused(
        &with(
            json!([{"role": "assistant", "content": "", "tool_calls": [{"id": null, "name": "render", "arguments": "{}"}]}]),
        ),
        test_keys(),
        "a tool call without an id cannot be sent",
    );
    assert_refused(
        &with(json!([{"role": "narrator", "content": "x"}])),
        test_keys(),
        "an agent transcript has no 'narrator' messages",
    );
    let tool = |tool: Value| {
        turn(
            TEXT_ROUTE,
            text_contract(),
            json!({"system": "", "messages": [], "tools": [render_tool(), tool], "tool_choice": "auto"}),
        )
    };
    assert_refused(
        &tool(json!({"name": "Render-View", "description": "d", "parameters": {}})),
        test_keys(),
        r#"tool name "Render-View" must be lower_snake_case, at most 64 characters"#,
    );
    assert_refused(
        &tool(json!({"name": "look", "description": " ", "parameters": {}})),
        test_keys(),
        "tool look must carry a description",
    );
    assert_refused(
        &tool(json!({"name": "look", "description": "d", "parameters": "object"})),
        test_keys(),
        "tool look parameters must be a JSON Schema object",
    );
}

#[test]
fn refusals_come_in_order_and_send_nothing() {
    let foreign = "openai/gpt-5.6-sol@openrouter is not a agent.turn route this adapter serves";
    // 1. The contract.
    assert_refused(
        &turn(
            TEXT_ROUTE,
            json!({"adapter": "openrouter-structured", "adapter_behavior": 1}),
            json!({}),
        ),
        Keys::none(),
        foreign,
    );
    assert_refused(
        &turn(
            TEXT_ROUTE,
            json!({"adapter": "openrouter-tool-loop", "adapter_behavior": 1, "pictures": "unchanged"}),
            json!({}),
        ),
        Keys::none(),
        &format!("{foreign}: the contract takes no member pictures"),
    );
    // 2. The capability, before the key.
    let mut request = simple_request();
    request["temperature"] = json!(0.2);
    assert_refused(
        &turn(TEXT_ROUTE, text_contract(), request),
        Keys::none(),
        "agent.turn takes no member temperature",
    );
    assert_refused(
        &turn(
            TEXT_ROUTE,
            text_contract(),
            json!({"system": "", "messages": [], "tools": []}),
        ),
        Keys::none(),
        "agent.turn needs tool_choice",
    );
    // 3. The key, before values.
    let mut request = simple_request();
    request["max_tokens"] = json!(0);
    assert_refused(
        &turn(TEXT_ROUTE, text_contract(), request.clone()),
        Keys::none(),
        "OPENROUTER_API_KEY is not set",
    );
    // 4. Values, before files.
    assert_refused(
        &turn(TEXT_ROUTE, text_contract(), request),
        test_keys(),
        "max_tokens must be at least 1",
    );
    let missing = json!({"file": "f".repeat(64)});
    assert_refused(
        &turn(
            TEXT_ROUTE,
            text_contract(),
            json!({"system": "", "tool_choice": "auto", "tools": [], "messages": [
                {"role": "user", "content": "x", "images": [missing]},
                {"role": "tool", "content": "no id"}
            ]}),
        ),
        test_keys(),
        "a tool message without a tool_call_id cannot be sent",
    );
    // 5. Files.
    assert_refused(
        &turn(
            TEXT_ROUTE,
            text_contract(),
            json!({"system": "", "tool_choice": "auto", "tools": [], "messages": [
                {"role": "user", "content": "x", "images": [missing]}
            ]}),
        ),
        test_keys(),
        "an agent picture has no bytes to send",
    );
}

// ---------------------------------------------------------------------------------------------
// HTTP and transport failures

#[test]
fn a_400_names_the_schema_problem_not_the_prompt() {
    let transport = replay(vec![expect(ExpectBody::Any).reply(HttpResponse::json(
        400,
        &json!({"error": {"message": "invalid schema for secret-prompt", "code": 400}}),
    ))]);
    let call = turn(
        TEXT_ROUTE,
        text_contract(),
        json!({"system": "", "messages": [{"role": "user", "content": "secret-prompt"}],
            "tools": [render_tool()], "tool_choice": "required"}),
    );
    let sent = block_on(adapter_over(transport.clone(), test_keys()).send(&call));
    transport.assert_done();
    assert_eq!(
        sent,
        Sent::Failed {
            reason: "OpenRouter tool loop returned HTTP 400: message=invalid schema; code=400"
                .into(),
            cost: Some(Usd::ZERO),
            retryable: false
        }
    );
}

#[test]
fn statuses_and_transport_failures_follow_the_openrouter_table() {
    let transport = replay(vec![
        expect(ExpectBody::Any).reply(HttpResponse::new(429, Vec::new())),
        expect(ExpectBody::Any).reply(HttpResponse::new(502, b"bad gateway".to_vec())),
        expect(ExpectBody::Any).fail(TransportError::not_sent(
            TransportErrorKind::Timeout,
            "connect timed out",
        )),
        expect(ExpectBody::Any).fail(TransportError::after_send(
            TransportErrorKind::Timeout,
            "the deadline passed",
        )),
    ]);
    let adapter = adapter_over(transport.clone(), test_keys());
    let call = turn(TEXT_ROUTE, text_contract(), simple_request());
    let outcomes: Vec<Sent> = (0..4).map(|_| block_on(adapter.send(&call))).collect();
    transport.assert_done();
    // `NotReceived` may carry more than its reason (a provider's requested wait).
    assert!(
        matches!(&outcomes[0], Sent::NotReceived { reason, .. } if reason == "OpenRouter tool loop was rate limited (HTTP 429)"),
        "{:?}",
        outcomes[0]
    );
    assert_eq!(
        outcomes[1],
        Sent::Failed {
            reason: "OpenRouter tool loop returned HTTP 502".into(),
            cost: None,
            retryable: true
        }
    );
    assert!(matches!(outcomes[2], Sent::NotReceived { .. }));
    assert_eq!(
        outcomes[3],
        Sent::Failed {
            reason: "OpenRouter tool loop failed: the deadline passed".into(),
            cost: None,
            retryable: true
        }
    );
    let offline = adapter_over(Arc::new(Offline), test_keys());
    assert!(matches!(
        block_on(offline.send(&call)),
        Sent::Refused { .. }
    ));
}

// ---------------------------------------------------------------------------------------------
// Credential hygiene

#[test]
fn the_key_never_reaches_a_reason_an_answer_or_a_url() {
    let transport = replay(vec![
        expect(ExpectBody::Any).reply(reply(
            json!({"content": "ok", "tool_calls": [{"id": "c", "function": {"name": "render", "arguments": "{}"}}]}),
            json!({"cost": 0.01}),
        )),
        expect(ExpectBody::Any).reply(HttpResponse::json(
            403,
            &json!({"error": {"message": format!("flagged {KEY}"), "type": KEY}}),
        )),
        expect(ExpectBody::Any).fail(TransportError::after_send(
            TransportErrorKind::Other,
            format!("reset after authorization: Bearer {KEY}"),
        )),
    ]);
    let adapter = adapter_over(transport.clone(), test_keys());
    let call = turn(TEXT_ROUTE, text_contract(), simple_request());
    let mut seen = Vec::new();
    for _ in 0..3 {
        match block_on(adapter.send(&call)) {
            Sent::Answered(answer) => seen.push(answer.data.to_string()),
            Sent::Failed { reason, .. }
            | Sent::Refused { reason }
            | Sent::NotReceived { reason, .. } => seen.push(reason),
        }
    }
    transport.assert_done();
    for text in &seen {
        assert!(!text.contains(KEY), "{text}");
    }
    for request in transport.requests() {
        assert!(!request.url.contains(KEY));
        assert!(request.headers.iter().all(|(_, v)| !v.contains(KEY)));
        assert!(!format!("{request:?}").contains(KEY));
        assert_eq!(
            request.credential.as_ref().map(|c| c.header),
            Some("authorization")
        );
    }
    // A refusal that would name the key does not.
    let refused = turn(
        TEXT_ROUTE,
        text_contract(),
        json!({"system": "", "messages": [{"role": KEY}], "tools": [], "tool_choice": "auto"}),
    );
    assert_refused(
        &refused,
        test_keys(),
        "an agent transcript has no '[redacted]' messages",
    );
}
