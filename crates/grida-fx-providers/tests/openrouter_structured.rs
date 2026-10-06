//! OpenRouter `structured.generate` over synthetic exchanges (spec/providers.md §9.2;
//! spec/capabilities.md §4). Nothing here reaches a network: every send goes to a
//! `ReplayTransport` that asserts the request, and every refusal runs over a transport that saw
//! no request. Pictures are built in code (`testing::media::png`).

use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{Answer, RequestAdapter, Sent};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::openrouter::{self, structured::OpenRouterStructured};
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
const TEXT_ROUTE: &str = "openai/gpt-5.6-sol@openrouter";
const ASTRA_ROUTE: &str = "openai/gpt-6-astra@openrouter";

fn text_contract() -> Value {
    json!({"adapter": "openrouter-structured", "adapter_behavior": 1})
}

fn astra_contract() -> Value {
    json!({
        "adapter": "openrouter-structured",
        "adapter_behavior": 1,
        "request_policy": {
            "provider": {"require_parameters": true, "only": ["openai"], "allow_fallbacks": false},
            "reasoning": {"effort": "high"},
            "image_detail": "high"
        },
        "pictures": "unchanged"
    })
}

fn adapter_over(transport: Arc<dyn Transport>, keys: Keys) -> OpenRouterStructured {
    OpenRouterStructured::new(openrouter::client(&setup(transport, keys)))
}

fn replay(exchanges: Vec<Exchange>) -> Arc<ReplayTransport> {
    Arc::new(ReplayTransport::new(exchanges))
}

fn expect(body: ExpectBody) -> Expect {
    Expect::post_json(URL, Value::Null)
        .credential("authorization", &format!("Bearer {KEY}"))
        .body(body)
}

fn call(route: &str, contract: Value, request: Value) -> TestCall {
    CallBuilder::new("structured.generate", route)
        .contract(contract)
        .request(request)
        .build()
}

fn completion(message: Value, usage: Value) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"id": "gen-1", "choices": [{"message": message, "finish_reason": "stop"}], "usage": usage}),
    )
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

/// Sends once.
fn send(adapter: &OpenRouterStructured, call: &TestCall) -> Sent {
    block_on(adapter.send(call))
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

/// Every refusal: `Refused` with this reason, and nothing was requested.
fn assert_refused(call: &TestCall, keys: Keys, reason: &str) {
    let transport = replay(Vec::new());
    let sent = send(&adapter_over(transport.clone(), keys), call);
    assert_eq!(
        sent,
        Sent::Refused {
            reason: reason.into()
        }
    );
    assert!(transport.requests().is_empty());
}

// ---------------------------------------------------------------------------------------------
// S1: the text model, no pictures

#[test]
fn s1_the_text_model_sends_a_plain_question() {
    let schema = colour_schema();
    let exact = r#"{"model":"openai/gpt-5.6-sol","messages":[{"role":"user","content":"Name a colour."}],"response_format":{"type":"json_schema","json_schema":{"name":"colour","strict":true,"schema":{"title":"colour","description":"A colour","type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false},"description":"A colour"}},"provider":{"require_parameters":true},"max_tokens":16000}"#.to_string();
    let transport = replay(vec![expect(ExpectBody::JsonText(exact)).reply(completion(
        json!({"role": "assistant", "content": "{\"name\": \"red\"}"}),
        json!({"prompt_tokens": 9, "cost": 0.00012345}),
    ))]);
    let adapter = adapter_over(transport.clone(), test_keys());
    let call = call(
        TEXT_ROUTE,
        text_contract(),
        json!({"prompt": "Name a colour.", "schema": schema, "system": "  ", "context": null, "matte": null, "max_tokens": null}),
    );
    let answer = answered(send(&adapter, &call));
    transport.assert_done();
    assert_eq!(answer.data, json!({"json": {"name": "red"}}));
    assert!(answer.files.is_empty());
    assert_eq!(answer.cost, Some(Usd(124)));
    assert_eq!(adapter.check(&call, &answer), Ok(()));
    let request = &transport.requests()[0];
    assert_eq!(request.timeout, Duration::from_secs(1800));
    assert_eq!(
        request.max_response_bytes,
        openrouter::MAX_CHAT_RESPONSE_BYTES
    );
}

// ---------------------------------------------------------------------------------------------
// S2 and S3: one 2400x3000 RGBA picture, unchanged on astra, reduced on the text model

fn tall_picture() -> Vec<u8> {
    media::png(2400, 3000, Some(128))
}

#[test]
fn s2_astra_sees_the_picture_unchanged_with_its_policy() {
    let picture = tall_picture();
    let mut builder =
        CallBuilder::new("structured.generate", ASTRA_ROUTE).contract(astra_contract());
    let file = builder.file("image/png", &picture);
    let call = builder
        .request(json!({"prompt": "Where is the face?", "schema": colour_schema(), "context": [file], "matte": "#ffffff", "max_tokens": 1234}))
        .build();
    let expected = json!({
        "model": "openai/gpt-6-astra",
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": "Where is the face?"},
            {"type": "image_url", "image_url": {"url": wire::data_url("image/png", &picture), "detail": "high"}}
        ]}],
        "response_format": {"type": "json_schema", "json_schema": {
            "name": "colour", "strict": true,
            "schema": {"title": "colour", "description": "A colour", "type": "object",
                "properties": {"name": {"type": "string"}}, "required": ["name"], "additionalProperties": false},
            "description": "A colour"
        }},
        "provider": {"require_parameters": true, "only": ["openai"], "allow_fallbacks": false},
        "reasoning": {"effort": "high"},
        "max_tokens": 1234
    });
    let transport = replay(vec![expect(ExpectBody::Json(expected)).reply(completion(
        json!({"content": "{\"name\": \"teal\"}"}),
        json!({"cost": 0.07}),
    ))]);
    let adapter = adapter_over(transport.clone(), test_keys());
    let answer = answered(send(&adapter, &call));
    transport.assert_done();
    assert_eq!(answer.cost, Some(Usd(70_000)));
    let request = &transport.requests()[0];
    assert_eq!(request.timeout, Duration::from_secs(900));
    // The provider object keeps its member order.
    let body = sent_json(&transport, 0);
    assert_eq!(
        serde_json::to_string(&body["provider"]).unwrap(),
        r#"{"require_parameters":true,"only":["openai"],"allow_fallbacks":false}"#
    );
    assert!(body.get("image_detail").is_none());
}

#[test]
fn s3_the_text_model_sees_a_reduced_flattened_picture() {
    let mut builder = CallBuilder::new("structured.generate", TEXT_ROUTE).contract(text_contract());
    let file = builder.file("image/png", &tall_picture());
    let call = builder
        .request(
            json!({"prompt": "Where is the face?", "schema": colour_schema(), "context": [file]}),
        )
        .build();
    let transport = replay(vec![expect(ExpectBody::Any).reply(completion(
        json!({"content": "{\"name\": \"teal\"}"}),
        json!({"cost": 0.02}),
    ))]);
    let adapter = adapter_over(transport.clone(), test_keys());
    answered(send(&adapter, &call));
    transport.assert_done();
    let body = sent_json(&transport, 0);
    assert_eq!(body["provider"], json!({"require_parameters": true}));
    assert!(body.get("reasoning").is_none());
    let parts = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(
        parts[0],
        json!({"type": "text", "text": "Where is the face?"})
    );
    let image_url = parts[1]["image_url"].as_object().unwrap();
    assert!(image_url.get("detail").is_none());
    let (media_type, payload) = wire::parse_data_url(image_url["url"].as_str().unwrap()).unwrap();
    assert_eq!(media_type, "image/png");
    let bytes = wire::strict_base64(&payload).unwrap();
    let facts = grida_fx_core::facts::image_facts(&bytes, "image/png")
        .unwrap()
        .unwrap();
    assert_eq!((facts.width, facts.height), (1280, 1600));
    assert!(!facts.has_alpha, "the reduced picture is RGB");
    // RGB (10, 20, 30) at alpha 128 over #ffffff.
    let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
    for (x, y) in [(0, 0), (640, 800), (1279, 1599)] {
        let pixel = decoded.get_pixel(x, y).0;
        for (got, want) in pixel.iter().zip([132u8, 137, 142]) {
            assert!(got.abs_diff(want) <= 1, "{pixel:?} at {x},{y}");
        }
    }
}

#[test]
fn a_reduced_picture_takes_the_requests_matte() {
    let mut builder = CallBuilder::new("structured.generate", TEXT_ROUTE).contract(text_contract());
    let file = builder.file("image/png", &media::png(4, 4, Some(0)));
    let call = builder
        .request(
            json!({"prompt": "p", "schema": colour_schema(), "context": [file], "matte": "#00F"}),
        )
        .build();
    let transport = replay(vec![expect(ExpectBody::Any).reply(completion(
        json!({"content": "{\"name\": \"blue\"}"}),
        json!({}),
    ))]);
    let answer = answered(send(&adapter_over(transport.clone(), test_keys()), &call));
    assert_eq!(answer.cost, None);
    let body = sent_json(&transport, 0);
    let url = body["messages"][0]["content"][1]["image_url"]["url"]
        .as_str()
        .unwrap();
    let bytes = wire::strict_base64(&wire::parse_data_url(url).unwrap().1).unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
    assert_eq!(decoded.dimensions(), (4, 4));
    assert_eq!(decoded.get_pixel(1, 1).0, [0, 0, 255]);
}

// ---------------------------------------------------------------------------------------------
// S4: a schema file with $defs, a $ref with siblings, defaults and assertions

fn report_schema() -> Value {
    json!({
        "title": "Count report",
        "type": "object",
        "properties": {
            "count": {"type": "integer", "default": 0},
            "detail": {"$ref": "#/$defs/detail", "description": "More."}
        },
        "$defs": {
            "detail": {
                "type": "object",
                "properties": {
                    "label": {"type": "string", "default": "count", "minLength": 1, "pattern": "^[a-z]+$"}
                }
            }
        }
    })
}

#[test]
fn s4_a_schema_file_is_inlined_and_made_strict() {
    let mut builder = CallBuilder::new("structured.generate", TEXT_ROUTE).contract(text_contract());
    let schema = builder.file(
        "application/json",
        serde_json::to_string_pretty(&report_schema())
            .unwrap()
            .as_bytes(),
    );
    let call = builder
        .request(json!({"prompt": "Count them.", "schema": schema}))
        .build();
    let sent_schema = json!({
        "title": "Count report",
        "type": "object",
        "properties": {
            "count": {"type": "integer"},
            "detail": {
                "type": "object",
                "properties": {"label": {"type": "string"}},
                "description": "More.",
                "required": ["label"],
                "additionalProperties": false
            }
        },
        "required": ["count", "detail"],
        "additionalProperties": false
    });
    let expected = json!({
        "model": "openai/gpt-5.6-sol",
        "messages": [{"role": "user", "content": "Count them."}],
        "response_format": {"type": "json_schema", "json_schema": {
            "name": "Count_report", "strict": true, "schema": sent_schema
        }},
        "provider": {"require_parameters": true},
        "max_tokens": 16000
    });
    let transport = replay(vec![expect(ExpectBody::Json(expected)).reply(completion(
        json!({"content": "{\"count\": 3, \"detail\": {\"label\": \"apples\"}}"}),
        json!({"cost": 0.01}),
    ))]);
    let adapter = adapter_over(transport.clone(), test_keys());
    let answer = answered(send(&adapter, &call));
    transport.assert_done();
    assert_eq!(adapter.check(&call, &answer), Ok(()));
    // The original schema still asserts what the sent one dropped.
    let short = Answer::new(json!({"json": {"count": 3, "detail": {"label": ""}}}), None);
    assert_eq!(
        adapter.check(&call, &short).unwrap_err(),
        r#"detail/label: "" is shorter than 1 character"#
    );
}

#[test]
fn s4_unknown_and_cyclic_references_are_refused() {
    let mut unknown = report_schema();
    unknown["properties"]["detail"]["$ref"] = json!("#/$defs/missing");
    assert_refused(
        &call(
            TEXT_ROUTE,
            text_contract(),
            json!({"prompt": "p", "schema": unknown}),
        ),
        test_keys(),
        "unknown local schema reference: #/$defs/missing",
    );
    let cyclic = json!({
        "type": "object",
        "properties": {"root": {"$ref": "#/$defs/node"}},
        "$defs": {"node": {"type": "object", "properties": {"children": {"type": "array", "items": {"$ref": "#/$defs/node"}}}}}
    });
    let mut builder = CallBuilder::new("structured.generate", TEXT_ROUTE).contract(text_contract());
    let file = builder.file("application/json", cyclic.to_string().as_bytes());
    assert_refused(
        &builder
            .request(json!({"prompt": "p", "schema": file}))
            .build(),
        test_keys(),
        "cyclic local schema reference: #/$defs/node",
    );
}

// ---------------------------------------------------------------------------------------------
// S5: text context

#[test]
fn s5_text_context_follows_the_prompt() {
    let picture = media::png(2, 2, Some(255));
    let mut builder =
        CallBuilder::new("structured.generate", ASTRA_ROUTE).contract(astra_contract());
    let image = builder.file("image/png", &picture);
    let notes = builder.file("text/plain", "notes about the\nsubject".as_bytes());
    let data = builder.file("application/json", br#"{"k": 1}"#);
    let call = builder
        .request(json!({"prompt": "Describe.", "system": "Be exact.", "schema": colour_schema(), "context": [image, notes, data]}))
        .build();
    let transport = replay(vec![expect(ExpectBody::Any).reply(completion(
        json!({"content": "{\"name\": \"red\"}"}),
        json!({"cost": 0.0}),
    ))]);
    let answer = answered(send(&adapter_over(transport.clone(), test_keys()), &call));
    assert_eq!(answer.cost, Some(Usd::ZERO));
    let body = sent_json(&transport, 0);
    assert_eq!(
        body["messages"],
        json!([
            {"role": "system", "content": "Be exact."},
            {"role": "user", "content": [
                {"type": "text", "text": "Describe.\n\n--- context 2 ---\nnotes about the\nsubject\n\n--- context 3 ---\n{\"k\": 1}"},
                {"type": "image_url", "image_url": {"url": wire::data_url("image/png", &picture), "detail": "high"}}
            ]}
        ])
    );
}

// ---------------------------------------------------------------------------------------------
// S6: answers

#[test]
fn s6_answers_parse_unwrap_or_fail() {
    let message = |m: Value| completion(m, json!({"cost": 0.25}));
    let replies = vec![
        // 1. A parsed object wins over content.
        message(json!({"parsed": {"name": "parsed"}, "content": "{\"name\": \"content\"}"})),
        // 2. String content.
        message(json!({"content": " {\"name\": \"string\", \"name\": \"last\"} "})),
        // 3. Text parts, joined.
        message(
            json!({"content": [{"type": "text", "text": "{\"name\":"}, {"type": "text", "text": " \"parts\"}"}]}),
        ),
        // 4. Empty content.
        message(json!({"content": "  "})),
        // 5. NaN.
        message(json!({"content": "{\"name\": NaN}"})),
        // 6. A completionState wrapper.
        message(
            json!({"content": "{\"completionState\": \"complete\", \"entries\": [[\"name\", {\"completionState\": \"complete\", \"value\": \"wrapped\"}]]}"}),
        ),
        // 7. No message.
        HttpResponse::json(200, &json!({"choices": [], "usage": {"cost": 0.25}})),
        // 8. Not JSON at all.
        HttpResponse::new(200, b"<html>busy</html>".to_vec()).with_header("x-request-id", "req-8"),
        // 9. A message that is not an object; a parsed value that is not an object.
        message(json!("text")),
        message(json!({"parsed": ["x"], "content": "{\"name\": \"fallback\"}"})),
    ];
    let transport = replay(
        replies
            .into_iter()
            .map(|reply| expect(ExpectBody::Any).reply(reply))
            .collect(),
    );
    let adapter = adapter_over(transport.clone(), test_keys());
    let call = call(
        TEXT_ROUTE,
        text_contract(),
        json!({"prompt": "p", "schema": colour_schema()}),
    );
    let mut outcomes: Vec<Sent> = (0..10).map(|_| send(&adapter, &call)).collect();
    transport.assert_done();
    let data = |sent: &Sent| match sent {
        Sent::Answered(answer) => answer.data.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(data(&outcomes[0]), json!({"json": {"name": "parsed"}}));
    assert_eq!(data(&outcomes[1]), json!({"json": {"name": "last"}}));
    assert_eq!(data(&outcomes[2]), json!({"json": {"name": "parts"}}));
    assert_eq!(data(&outcomes[5]), json!({"json": {"name": "wrapped"}}));
    assert_eq!(data(&outcomes[9]), json!({"json": {"name": "fallback"}}));
    let failed = |sent: Sent| match sent {
        Sent::Failed {
            reason,
            cost,
            retryable: true,
        } => (reason, cost),
        other => panic!("{other:?}"),
    };
    let quarter = Some(Usd(250_000));
    assert_eq!(
        failed(outcomes.remove(8)),
        (
            "OpenRouter structured generation returned no message".into(),
            quarter
        )
    );
    assert_eq!(
        failed(outcomes.remove(7)),
        (
            "OpenRouter structured generation returned invalid JSON (request req-8)".into(),
            None
        )
    );
    assert_eq!(
        failed(outcomes.remove(6)),
        (
            "OpenRouter structured generation returned no message".into(),
            quarter
        )
    );
    assert_eq!(
        failed(outcomes.remove(4)),
        (
            "OpenRouter structured generation returned invalid JSON content".into(),
            quarter
        )
    );
    assert_eq!(
        failed(outcomes.remove(3)),
        (
            "OpenRouter structured generation returned empty content".into(),
            quarter
        )
    );
}

// ---------------------------------------------------------------------------------------------
// S7: check

#[test]
fn s7_the_check_refuses_with_the_smallest_location() {
    let schema = json!({
        "type": "object",
        "properties": {
            "count": {"type": "integer"},
            "tags": {"type": "array", "items": {"type": "string"}},
            "mail": {"type": "string", "format": "email"}
        },
        "required": ["count"]
    });
    let adapter = adapter_over(replay(Vec::new()), test_keys());
    let call = call(
        TEXT_ROUTE,
        text_contract(),
        json!({"prompt": "p", "schema": schema}),
    );
    let check = |value: Value| adapter.check(&call, &Answer::new(json!({ "json": value }), None));
    assert_eq!(
        check(json!({"tags": ["a", 3]})),
        Err(r#"the answer: "count" is a required property"#.into())
    );
    assert_eq!(
        check(json!({"count": "x", "tags": [1]})),
        Err(r#"count: "x" is not of type "integer""#.into())
    );
    assert_eq!(
        check(json!({"count": 1, "tags": ["a", "b", 3]})),
        Err(r#"tags/2: 3 is not of type "string""#.into())
    );
    // format is not asserted.
    assert_eq!(check(json!({"count": 1, "mail": "not an address"})), Ok(()));
    assert_eq!(
        adapter.check(&call, &Answer::new(json!({"other": 1}), None)),
        Err("the answer holds no json value".into())
    );
    // A refusal is cut to 500 characters.
    let long = "a b ".repeat(200);
    let refusal = check(json!({"count": long})).unwrap_err();
    assert_eq!(refusal.chars().count(), 500);
    assert!(refusal.starts_with("count: \"a b a b"));
}

// ---------------------------------------------------------------------------------------------
// S8: HTTP failures

#[test]
fn s8_an_upstream_error_keeps_only_its_safe_detail() {
    let prompt = "private-prompt-value";
    let raw = json!({"error": {
        "message": format!("Invalid schema for response_format 'x': In context=(), 'required' is required to be supplied and to be an array including every key in properties. required must include every key {KEY} {prompt}"),
        "type": "invalid_request_error", "param": "response_format", "code": "invalid_json_schema"
    }})
    .to_string();
    let transport = replay(vec![expect(ExpectBody::Any).reply(HttpResponse::json(
        400,
        &json!({"error": {"message": format!("Provider returned error {prompt}"), "code": 400,
            "metadata": {"raw": raw, "provider_name": "OpenAI"}}}),
    ))]);
    let adapter = adapter_over(transport.clone(), test_keys());
    let call = call(
        TEXT_ROUTE,
        text_contract(),
        json!({"prompt": prompt, "schema": colour_schema()}),
    );
    let sent = send(&adapter, &call);
    transport.assert_done();
    let Sent::Failed {
        reason,
        cost,
        retryable,
    } = sent
    else {
        panic!("{sent:?}")
    };
    assert_eq!(
        reason,
        "OpenRouter structured generation returned HTTP 400: message=invalid schema, required must include every key, response_format; type=invalid_request_error; code=invalid_json_schema; param=response_format"
    );
    assert_eq!((cost, retryable), (Some(Usd::ZERO), false));
    assert!(!reason.contains(KEY) && !reason.contains(prompt));
}

#[test]
fn statuses_and_transport_failures_follow_the_openrouter_table() {
    let transport = replay(vec![
        expect(ExpectBody::Any).reply(
            HttpResponse::json(429, &json!({"error": {"code": 429}}))
                .with_header("retry-after", "7"),
        ),
        expect(ExpectBody::Any).reply(HttpResponse::json(503, &json!({"error": {"code": 503}}))),
        expect(ExpectBody::Any).reply(HttpResponse::new(500, b"oops".to_vec())),
        expect(ExpectBody::Any).reply(HttpResponse::new(402, b"no credits".to_vec())),
        expect(ExpectBody::Any).reply(HttpResponse::new(302, Vec::new())),
        expect(ExpectBody::Any).fail(TransportError::not_sent(
            TransportErrorKind::Connect,
            "connection refused",
        )),
        expect(ExpectBody::Any).fail(TransportError::after_send(
            TransportErrorKind::Other,
            "connection reset",
        )),
        expect(ExpectBody::Any).fail(TransportError::too_large(16 * 1024 * 1024)),
    ]);
    let adapter = adapter_over(transport.clone(), test_keys());
    let call = call(
        TEXT_ROUTE,
        text_contract(),
        json!({"prompt": "p", "schema": colour_schema()}),
    );
    let outcomes: Vec<Sent> = (0..8).map(|_| send(&adapter, &call)).collect();
    transport.assert_done();
    // `NotReceived` may carry more than its reason (a provider's requested wait).
    assert!(
        matches!(&outcomes[0], Sent::NotReceived { reason, .. } if reason == "OpenRouter structured generation was rate limited (HTTP 429); retry-after 7"),
        "{:?}",
        outcomes[0]
    );
    assert!(matches!(outcomes[1], Sent::NotReceived { .. }));
    assert!(matches!(
        outcomes[2],
        Sent::Failed {
            cost: None,
            retryable: true,
            ..
        }
    ));
    assert!(matches!(
        outcomes[3],
        Sent::Failed {
            cost: None,
            retryable: false,
            ..
        }
    ));
    assert!(matches!(
        outcomes[4],
        Sent::Failed {
            retryable: false,
            ..
        }
    ));
    assert!(matches!(outcomes[5], Sent::NotReceived { .. }));
    for outcome in &outcomes[6..] {
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
fn a_transport_that_refuses_is_a_refusal() {
    let adapter = adapter_over(Arc::new(Offline), test_keys());
    let call = call(
        TEXT_ROUTE,
        text_contract(),
        json!({"prompt": "p", "schema": colour_schema()}),
    );
    assert_eq!(
        send(&adapter, &call),
        Sent::Refused {
            reason:
                "OpenRouter structured generation was not sent: the network is off (GRIDA_FX_NETWORK=off)"
                    .into()
        }
    );
}

// ---------------------------------------------------------------------------------------------
// Refusals before sending, in the order of spec/providers.md §5

#[test]
fn refusals_come_in_order_and_send_nothing() {
    let foreign =
        "openai/gpt-5.6-sol@openrouter is not a structured.generate route this adapter serves";
    // 1. The contract, before anything else.
    assert_refused(
        &call(
            TEXT_ROUTE,
            json!({"adapter": "openrouter-tool-loop", "adapter_behavior": 1}),
            json!({"prompt": "", "bogus": 1}),
        ),
        Keys::none(),
        foreign,
    );
    assert_refused(
        &call(
            TEXT_ROUTE,
            json!({"adapter": "openrouter-structured", "adapter_behavior": 1, "pictures": "small"}),
            json!({}),
        ),
        test_keys(),
        &format!("{foreign}: pictures is \"unchanged\" or absent"),
    );
    assert_refused(
        &call(
            TEXT_ROUTE,
            json!({"request_policy": {"provider": {"require_parameters": false}}}),
            json!({}),
        ),
        test_keys(),
        &format!("{foreign}: request_policy.provider.require_parameters must be true"),
    );
    // 2. The capability, before the key.
    assert_refused(
        &call(
            TEXT_ROUTE,
            text_contract(),
            json!({"prompt": "p", "schema": {}, "temperature": 0}),
        ),
        Keys::none(),
        "structured.generate takes no member temperature",
    );
    assert_refused(
        &call(TEXT_ROUTE, text_contract(), json!({"prompt": "p"})),
        Keys::none(),
        "structured.generate needs schema",
    );
    assert_refused(
        &call(
            TEXT_ROUTE,
            text_contract(),
            json!({"prompt": "p", "schema": [1]}),
        ),
        test_keys(),
        "schema is a JSON file or object",
    );
    // 3. The key, before values.
    assert_refused(
        &call(
            TEXT_ROUTE,
            text_contract(),
            json!({"prompt": " ", "schema": {}}),
        ),
        Keys::none(),
        "OPENROUTER_API_KEY is not set",
    );
    // 4. Values, before files.
    let mut builder = CallBuilder::new("structured.generate", TEXT_ROUTE).contract(text_contract());
    let missing = json!({"file": "f".repeat(64)});
    let bad_text = builder.file("text/plain", b"\xff\xfe not utf-8");
    let built = builder
        .request(json!({"prompt": "\n", "schema": missing.clone(), "context": [bad_text.clone()]}))
        .build();
    assert_refused(
        &built,
        test_keys(),
        "a structured call needs its prompt as text",
    );
    for (request, reason) in [
        (
            json!({"prompt": "p", "schema": {}, "matte": "white"}),
            "matte white is not a colour",
        ),
        (
            json!({"prompt": "p", "schema": {}, "max_tokens": 0}),
            "max_tokens must be at least 1",
        ),
        (
            json!({"prompt": "p", "schema": {"type": "nonsense"}}),
            "a structured call's schema is not a JSON Schema (draft 2020-12)",
        ),
        (
            json!({"prompt": "p", "schema": {"$ref": "https://example.test/schema.json"}}),
            "a structured call's schema is not a JSON Schema (draft 2020-12)",
        ),
    ] {
        assert_refused(
            &call(TEXT_ROUTE, text_contract(), request),
            test_keys(),
            reason,
        );
    }
    // 5. Files.
    let mut builder = CallBuilder::new("structured.generate", TEXT_ROUTE).contract(text_contract());
    let bad_text = builder.file("text/plain", b"\xff\xfe not utf-8");
    let not_json = builder.file("application/json", b"{\"type\": ");
    let array = builder.file("application/json", b"[1, 2]");
    let not_picture = builder.file("image/png", b"\x89PNG\r\n\x1a\n broken");
    let schema = builder.file("application/json", colour_schema().to_string().as_bytes());
    let built = builder
        .request(json!({"prompt": "p", "schema": missing, "context": [bad_text]}))
        .build();
    assert_refused(&built, test_keys(), "schema has no bytes to send");
    for (request, reason) in [
        (
            json!({"prompt": "p", "schema": not_json}),
            "a structured call's schema is not a JSON file",
        ),
        (
            json!({"prompt": "p", "schema": array}),
            "a structured call's schema is a JSON object",
        ),
        (
            json!({"prompt": "p", "schema": schema, "context": [schema, bad_text]}),
            "context 2 is not UTF-8 text",
        ),
        (
            json!({"prompt": "p", "schema": schema, "context": [not_picture]}),
            "context 1 is not a decodable image/png",
        ),
        (
            json!({"prompt": "p", "schema": schema, "context": [{"file": "0".repeat(64)}]}),
            "context 1 has no bytes to send",
        ),
    ] {
        let mut call = built.call.clone();
        call.request = request;
        let transport = replay(Vec::new());
        assert_eq!(
            block_on(adapter_over(transport.clone(), test_keys()).send(&call)),
            Sent::Refused {
                reason: reason.into()
            }
        );
        assert!(transport.requests().is_empty());
    }
}

#[test]
fn an_unchanged_picture_is_not_decoded() {
    // The astra route sends the file as it is, so even bytes no decoder reads go unchanged.
    let mut builder =
        CallBuilder::new("structured.generate", ASTRA_ROUTE).contract(astra_contract());
    let odd = builder.file("image/x-made-up", b"made-up picture bytes");
    let call = builder
        .request(json!({"prompt": "p", "schema": colour_schema(), "context": [odd]}))
        .build();
    let transport = replay(vec![expect(ExpectBody::Any).reply(completion(
        json!({"content": "{\"name\": \"x\"}"}),
        json!({}),
    ))]);
    answered(send(&adapter_over(transport.clone(), test_keys()), &call));
    let body = sent_json(&transport, 0);
    assert_eq!(
        body["messages"][0]["content"][1]["image_url"]["url"],
        json!(wire::data_url("image/x-made-up", b"made-up picture bytes"))
    );
}

// ---------------------------------------------------------------------------------------------
// Credential hygiene

#[test]
fn the_key_never_reaches_a_reason_an_answer_or_a_url() {
    let leaky = json!({"error": {"message": format!("bad key {KEY}"), "code": format!("{KEY}")}});
    let transport = replay(vec![
        expect(ExpectBody::Any).reply(completion(
            json!({"content": "{\"name\": \"red\"}"}),
            json!({"cost": 0.01}),
        )),
        expect(ExpectBody::Any).reply(HttpResponse::json(401, &leaky)),
        expect(ExpectBody::Any).fail(TransportError::after_send(
            TransportErrorKind::Other,
            format!("reset while sending Bearer {KEY}"),
        )),
        expect(ExpectBody::Any).reply(HttpResponse::new(
            200,
            format!("not json {KEY}").into_bytes(),
        )),
    ]);
    let adapter = adapter_over(transport.clone(), test_keys());
    let call = call(
        TEXT_ROUTE,
        text_contract(),
        json!({"prompt": "p", "schema": colour_schema()}),
    );
    let mut seen = Vec::new();
    for _ in 0..4 {
        match send(&adapter, &call) {
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
        let body = serde_json::to_string(&sent_json_of(&request.body)).unwrap();
        assert!(!body.contains(KEY));
        assert!(!format!("{request:?}").contains(KEY));
    }
    let refused = assert_refusal_reason(&call);
    assert!(!refused.contains(KEY));
}

fn sent_json_of(body: &Body) -> Value {
    match body {
        Body::Json(value) => value.clone(),
        _ => Value::Null,
    }
}

fn assert_refusal_reason(call: &TestCall) -> String {
    let mut call = call.call.clone();
    call.request = json!({"prompt": "p", "schema": colour_schema(), "matte": KEY});
    let transport = replay(Vec::new());
    match block_on(adapter_over(transport.clone(), test_keys()).send(&call)) {
        Sent::Refused { reason } => {
            assert!(transport.requests().is_empty());
            reason
        }
        other => panic!("{other:?}"),
    }
}
