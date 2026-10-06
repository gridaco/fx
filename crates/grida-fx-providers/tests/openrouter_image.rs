//! OpenRouter images (spec/providers.md §9.2; spec/capabilities.md §2–§3), offline: every
//! exchange is scripted on a `ReplayTransport` that asserts each request, and every refusal runs
//! over `NoNetwork`, which panics on any send. Media are built in code (`testing::media`).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{Answer, RequestAdapter, Sent};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::openrouter::{self, image::OpenRouterImages};
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, media, setup, test_keys};
use grida_fx_providers::transport::replay::{
    Exchange, Expect, ExpectBody, NoNetwork, ReplayTransport, Reply,
};
use grida_fx_providers::transport::{
    HttpResponse, Lane, Method, Offline, Transport, TransportError, TransportErrorKind,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const ROUTE: &str = "openai/gpt-image-2.5-sunburst@openrouter";
const MODEL: &str = "openai/gpt-image-2.5-sunburst";
const URL: &str = "https://openrouter.ai/api/v1/images";
const KEY: &str = "test-openrouter-key";

fn contract(variant: &str) -> Value {
    json!({
        "route_id": format!("image.sunburst.openrouter.images.{variant}"),
        "adapter": "fx-openrouter-image-v1",
        "adapter_behavior": "3",
        "surface": "openrouter-images",
    })
}

fn generate(request: Value) -> TestCall {
    CallBuilder::new("image.generate", ROUTE)
        .contract(contract("generation"))
        .request(request)
        .build()
}

fn adapter_over(transport: Arc<dyn Transport>, keys: Keys) -> OpenRouterImages {
    OpenRouterImages::new(openrouter::client(&setup(transport, keys)))
}

fn adapter(transport: &Arc<ReplayTransport>) -> OpenRouterImages {
    adapter_over(transport.clone(), test_keys())
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn data_url(kind: &str, bytes: &[u8]) -> String {
    format!("data:{kind};base64,{}", b64(bytes))
}

/// The body of a generate in the adapter's member order.
fn generate_body(prompt: &str, size: Option<&str>, background: &str) -> Value {
    let mut body = json!({
        "model": MODEL,
        "prompt": prompt,
        "n": 1,
        "provider": {"allow_fallbacks": false, "options": {"openai": {"moderation": "low"}}},
    });
    if let Some(size) = size {
        body["size"] = json!(size);
    }
    body["quality"] = json!("max");
    body["background"] = json!(background);
    body
}

/// A request the adapter must send: the JSON body as its exact compact encoding (member order
/// included) with the test key on `authorization`.
fn expect(body: &Value) -> Expect {
    Expect::new(Method::Post, URL, Lane::Provider)
        .credential("authorization", &format!("Bearer {KEY}"))
        .body(ExpectBody::JsonText(serde_json::to_string(body).unwrap()))
}

/// A 200 answer of one image.
fn image_answer(bytes: &[u8], media_type: Option<&str>, usage: Option<Value>) -> HttpResponse {
    let mut item = json!({"b64_json": b64(bytes)});
    if let Some(media_type) = media_type {
        item["media_type"] = json!(media_type);
    }
    let mut body = json!({"created": 731, "data": [item]});
    if let Some(usage) = usage {
        body["usage"] = usage;
    }
    HttpResponse::json(200, &body).with_header("x-request-id", "gen-1")
}

/// Sends `call` over one scripted exchange answering `reply`, asserting the request carried
/// exactly `body` (any body when `body` is `None`) and that it was the only send.
fn send_one(call: &TestCall, body: Option<&Value>, reply: Reply) -> Sent {
    let expected = match body {
        Some(body) => expect(body),
        None => Expect::new(Method::Post, URL, Lane::Provider)
            .credential("authorization", &format!("Bearer {KEY}"))
            .body(ExpectBody::Any),
    };
    let transport = Arc::new(ReplayTransport::new(vec![Exchange {
        expect: expected,
        reply,
    }]));
    let sent = block_on(adapter(&transport).send(call));
    transport.assert_done();
    assert_eq!(transport.requests().len(), 1, "one send, never a retry");
    sent
}

fn refused(call: &TestCall) -> String {
    refused_with(call, test_keys())
}

fn refused_with(call: &TestCall, keys: Keys) -> String {
    match block_on(adapter_over(Arc::new(NoNetwork), keys).send(call)) {
        Sent::Refused { reason } => reason,
        other => panic!("expected Refused, got {other:?}"),
    }
}

fn answered(sent: Sent) -> Answer {
    match sent {
        Sent::Answered(answer) => answer,
        other => panic!("expected Answered, got {other:?}"),
    }
}

fn failed(sent: Sent) -> (String, Option<Usd>, bool) {
    match sent {
        Sent::Failed {
            reason,
            cost,
            retryable,
        } => (reason, cost, retryable),
        other => panic!("expected Failed, got {other:?}"),
    }
}

// I1 -------------------------------------------------------------------------------------------

#[test]
fn an_edit_sends_the_full_body_and_reports_its_cost() {
    let reference = media::png(4, 4, None);
    let mut builder = CallBuilder::new("image.edit", ROUTE).contract(contract("reference"));
    let image = builder.file("image/png", &reference);
    let call = builder
        .request(json!({
            "prompt": "a lantern on a pier, dusk",
            "image": image,
            "size": "2560x1440",
            "background": "opaque",
            "references": null,
            "mask": null,
        }))
        .build();
    let body = json!({
        "model": MODEL,
        "prompt": "a lantern on a pier, dusk",
        "n": 1,
        "provider": {"allow_fallbacks": false, "options": {"openai": {"moderation": "low"}}},
        "size": "2560x1440",
        "quality": "max",
        "background": "opaque",
        "input_references": [
            {"type": "image_url", "image_url": {"url": data_url("image/png", &reference)}}
        ],
    });
    let png = media::png(2560, 1440, None);
    let transport = Arc::new(ReplayTransport::new(vec![expect(&body).reply(
        image_answer(
            &png,
            Some("image/png"),
            Some(json!({"cost": 0.227414, "prompt_tokens": 10})),
        ),
    )]));
    let adapter = adapter(&transport);
    let answer = answered(block_on(adapter.send(&call)));
    transport.assert_done();
    let request = &transport.requests()[0];
    assert_eq!(request.timeout, Duration::from_secs(600));
    assert_eq!(request.max_response_bytes, 64 * 1024 * 1024);
    assert!(request.headers.is_empty(), "{:?}", request.headers);
    assert_eq!(answer.cost, Some(Usd(227_414)));
    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.files.len(), 1);
    assert_eq!(answer.files["image"].kind, "image/png");
    assert_eq!(answer.files["image"].bytes, png);
    assert_eq!(adapter.check(&call, &answer), Ok(()));
}

#[test]
fn an_edit_sends_image_then_references_in_order() {
    let pictures: Vec<Vec<u8>> = (1..=3).map(|n| media::png(n, n, None)).collect();
    let mut builder = CallBuilder::new("image.edit", ROUTE).contract(contract("reference"));
    let image = builder.file("image/png", &pictures[0]);
    let first = builder.file("image/webp", &pictures[1]);
    let second = builder.file("image/jpeg", &pictures[2]);
    let call = builder
        .request(json!({"prompt": "p", "image": image, "references": [first, second]}))
        .build();
    let mut body = generate_body("p", None, "auto");
    body["input_references"] = json!([
        {"type": "image_url", "image_url": {"url": data_url("image/png", &pictures[0])}},
        {"type": "image_url", "image_url": {"url": data_url("image/webp", &pictures[1])}},
        {"type": "image_url", "image_url": {"url": data_url("image/jpeg", &pictures[2])}},
    ]);
    let png = media::png(2, 2, None);
    let sent = send_one(
        &call,
        Some(&body),
        Reply::Response(image_answer(&png, None, None)),
    );
    assert_eq!(answered(sent).files["image"].bytes, png);
}

#[test]
fn an_edit_takes_sixteen_pictures() {
    let mut builder = CallBuilder::new("image.edit", ROUTE).contract(contract("reference"));
    let image = builder.file("image/png", &media::png(1, 1, None));
    let references: Vec<Value> = (2..=16)
        .map(|n| builder.file("image/png", &media::png(n, 1, None)))
        .collect();
    let call = builder
        .request(json!({"prompt": "p", "image": image, "references": references}))
        .build();
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::new(Method::Post, URL, Lane::Provider)
            .credential("authorization", &format!("Bearer {KEY}"))
            .body(ExpectBody::Any)
            .reply(image_answer(&media::png(2, 2, None), None, None)),
    ]));
    answered(block_on(adapter(&transport).send(&call)));
    let request = &transport.requests()[0];
    let grida_fx_providers::transport::Body::Json(body) = &request.body else {
        panic!("a JSON body")
    };
    assert_eq!(body["input_references"].as_array().unwrap().len(), 16);
}

// I2 -------------------------------------------------------------------------------------------

#[test]
fn a_generate_without_size_sends_auto_background_and_no_size() {
    let call = generate(json!({"prompt": "a kite", "size": null, "background": null}));
    let png = media::png(2, 2, None);
    let sent = send_one(
        &call,
        Some(&generate_body("a kite", None, "auto")),
        Reply::Response(image_answer(&png, None, None)),
    );
    let answer = answered(sent);
    assert_eq!(answer.cost, None);
    assert_eq!(answer.files["image"].kind, "image/png");
    let adapter = adapter_over(Arc::new(NoNetwork), test_keys());
    assert_eq!(adapter.check(&call, &answer), Ok(()));
}

#[test]
fn a_size_of_auto_is_sent_as_auto() {
    let call = generate(json!({"prompt": "a kite", "size": "auto", "background": "opaque"}));
    let png = media::png(3, 2, None);
    let sent = send_one(
        &call,
        Some(&generate_body("a kite", Some("auto"), "opaque")),
        Reply::Response(image_answer(&png, Some("image/png"), None)),
    );
    let answer = answered(sent);
    let adapter = adapter_over(Arc::new(NoNetwork), test_keys());
    assert_eq!(adapter.check(&call, &answer), Ok(()));
}

#[test]
fn every_verified_size_is_sent_verbatim() {
    for size in openrouter::image::ALLOWED_SIZES {
        let call = generate(json!({"prompt": "p", "size": size}));
        let sent = send_one(
            &call,
            Some(&generate_body("p", Some(size), "auto")),
            Reply::Response(image_answer(&media::png(1, 1, None), None, None)),
        );
        answered(sent);
    }
}

#[test]
fn a_fixture_exchange_plays() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/openrouter/image/generate-rate-limited.json");
    let transport = Arc::new(ReplayTransport::from_fixture(&path));
    let call = generate(json!({"prompt": "a kite", "size": "1024x1024"}));
    let sent = block_on(adapter(&transport).send(&call));
    transport.assert_done();
    let Sent::NotReceived {
        reason,
        retry_after,
    } = sent
    else {
        panic!("expected NotReceived, got {sent:?}")
    };
    // The wait the provider asked for is honoured and named (classify's sentence).
    assert_eq!(retry_after, Some(Duration::from_secs(20)));
    assert!(
        reason.starts_with("OpenRouter image generation was rate limited (HTTP 429)"),
        "{reason}"
    );
    assert!(reason.contains("20"), "{reason}");
}

// I3 -------------------------------------------------------------------------------------------

#[test]
fn foreign_contracts_are_refused_before_anything_else() {
    let not_served = "openai/gpt-image-2.5-sunburst@openrouter is not a image.generate route this adapter serves";
    for contract in [
        json!({"adapter": "fx-openrouter-image-v1", "adapter_behavior": 3}),
        json!({"adapter": "fx-openrouter-image-v1", "adapter_behavior": "1"}),
        json!({"adapter": "gnode-openrouter-image-v1", "adapter_behavior": "3"}),
        json!({"adapter": "fx-openai-image-v1", "adapter_behavior": "3"}),
    ] {
        // A malformed request and a missing key too: the contract is judged first.
        let call = CallBuilder::new("image.generate", ROUTE)
            .contract(contract.clone())
            .request(json!({"prompt": " ", "quality": "low"}))
            .build();
        assert_eq!(refused_with(&call, Keys::none()), not_served, "{contract}");
    }
    // Each member is checked when present (spec/providers.md §5 step 1): without one, the next
    // step judges the call.
    for contract in [
        json!({}),
        json!({"adapter": "fx-openrouter-image-v1"}),
        json!({"adapter_behavior": "3"}),
    ] {
        let call = CallBuilder::new("image.generate", ROUTE)
            .contract(contract.clone())
            .request(json!({"prompt": "a kite"}))
            .build();
        assert_eq!(
            refused_with(&call, Keys::none()),
            "OPENROUTER_API_KEY is not set",
            "{contract}"
        );
    }
    let music = CallBuilder::new("music.generate", ROUTE)
        .contract(contract("generation"))
        .request(json!({"prompt": "p"}))
        .build();
    assert_eq!(
        refused(&music),
        "openai/gpt-image-2.5-sunburst@openrouter is not a music.generate route this adapter serves"
    );
}

#[test]
fn requests_that_do_not_fit_the_capability_are_refused() {
    assert_eq!(
        refused(&generate(json!({"prompt": "p", "quality": "low"}))),
        "image.generate takes no member quality"
    );
    assert_eq!(
        refused(&generate(json!({"prompt": "p", "mask": null}))),
        "image.generate takes no member mask"
    );
    assert_eq!(
        refused(&generate(json!({"size": "auto"}))),
        "image.generate needs prompt"
    );
    assert_eq!(refused(&generate(json!({"prompt": 3}))), "prompt is text");
    assert_eq!(
        refused(&generate(json!({"prompt": "p", "size": 1024}))),
        "size is text"
    );
    let edit = CallBuilder::new("image.edit", ROUTE)
        .contract(contract("reference"))
        .request(json!({"prompt": "p"}))
        .build();
    assert_eq!(refused(&edit), "image.edit needs image");
    assert_eq!(
        refused(
            &CallBuilder::new("image.generate", ROUTE)
                .contract(contract("generation"))
                .request(json!(["p"]))
                .build()
        ),
        "a request for image.generate is a JSON object"
    );
}

#[test]
fn a_missing_key_is_refused_before_the_values() {
    let call = generate(json!({"prompt": "  ", "background": "transparent"}));
    assert_eq!(
        refused_with(&call, Keys::none()),
        "OPENROUTER_API_KEY is not set"
    );
}

#[test]
fn values_the_route_cannot_take_are_refused_in_order() {
    let cases = [
        (
            json!({"prompt": " \n", "background": "transparent", "size": "7x7"}),
            "an image call needs its prompt as text",
        ),
        (
            json!({"prompt": "p", "background": "transparent"}),
            "OpenRouter image generation does not support transparent backgrounds",
        ),
        (
            json!({"prompt": "p", "background": "white", "size": "7x7"}),
            "background must be auto, opaque or transparent",
        ),
        (
            json!({"prompt": "p", "size": "1536x1024"}),
            "OpenRouter serves no exact size 1536x1024",
        ),
        (
            json!({"prompt": "p", "size": "2560X1440"}),
            "OpenRouter serves no exact size 2560X1440",
        ),
        (
            json!({"prompt": "p", "size": ""}),
            "OpenRouter serves no exact size",
        ),
    ];
    for (request, reason) in cases {
        assert_eq!(refused(&generate(request.clone())), reason, "{request}");
    }
}

#[test]
fn references_on_a_generate_are_refused() {
    let mut builder = CallBuilder::new("image.generate", ROUTE).contract(contract("generation"));
    let reference = builder.file("image/png", &media::png(1, 1, None));
    let call = builder
        .request(json!({"prompt": "p", "references": [reference]}))
        .build();
    assert_eq!(
        refused(&call),
        "OpenRouter image generation takes no references"
    );
    // An empty list is no references.
    let empty = generate(json!({"prompt": "p", "references": []}));
    let sent = send_one(
        &empty,
        Some(&generate_body("p", None, "auto")),
        Reply::Response(image_answer(&media::png(1, 1, None), None, None)),
    );
    answered(sent);
}

#[test]
fn masks_and_more_than_sixteen_pictures_are_refused_on_an_edit() {
    let mut builder = CallBuilder::new("image.edit", ROUTE).contract(contract("reference"));
    let image = builder.file("image/png", &media::png(1, 1, None));
    let mask = builder.file("image/png", &media::png(1, 1, Some(0)));
    let references: Vec<Value> = (2..=17)
        .map(|n| builder.file("image/png", &media::png(n, 1, None)))
        .collect();
    let mut call = builder
        .request(json!({"prompt": "p", "image": image, "mask": mask, "background": "transparent"}))
        .build();
    assert_eq!(
        refused(&call),
        "OpenRouter image generation has no masked-edit route"
    );
    call.call.request = json!({"prompt": "p", "image": image, "references": references});
    assert_eq!(
        refused(&call),
        "OpenRouter image edits support at most 16 input images"
    );
}

#[test]
fn files_that_are_not_pictures_or_have_no_bytes_are_refused() {
    let mut builder = CallBuilder::new("image.edit", ROUTE).contract(contract("reference"));
    let image = builder.file("image/png", &media::png(1, 1, None));
    let audio = builder.file("audio/mpeg", media::MP3);
    let empty = builder.file("image/png", b"");
    let mut call = builder
        .request(
            json!({"prompt": "p", "image": image.clone(), "references": [image.clone(), audio]}),
        )
        .build();
    assert_eq!(refused(&call), "references[1] is audio/mpeg, not a picture");
    call.call.request = json!({"prompt": "p", "image": empty});
    assert_eq!(refused(&call), "image has no bytes to send");
    call.call.request = json!({"prompt": "p", "image": {"file": "0".repeat(64)}});
    assert_eq!(refused(&call), "image has no bytes to send");
    call.call.request =
        json!({"prompt": "p", "image": image, "references": [{"file": "f".repeat(64)}]});
    assert_eq!(refused(&call), "references[0] has no bytes to send");
}

// I4 -------------------------------------------------------------------------------------------

#[test]
fn statuses_classify_per_spec() {
    let envelope = json!({"error": {"code": 400, "message": "Provider returned error"}});
    let cases: Vec<(u16, Value, &str)> = vec![
        (429, json!({}), "not_received"),
        (503, envelope.clone(), "not_received"),
        (408, json!({}), "retry"),
        (
            408,
            json!({"error": {"code": 408, "message": "timed out"}}),
            "retry",
        ),
        (400, envelope.clone(), "zero"),
        (401, envelope.clone(), "zero"),
        (402, envelope.clone(), "zero"),
        (403, envelope.clone(), "zero"),
        (404, envelope.clone(), "zero"),
        (413, envelope.clone(), "zero"),
        (400, json!("not an envelope"), "unknown"),
        (500, json!({}), "retry"),
        (502, envelope.clone(), "retry"),
        (503, json!("overloaded"), "retry"),
        (504, json!({}), "retry"),
        (302, json!({}), "redirect"),
    ];
    for (status, body, class) in cases {
        let call = generate(json!({"prompt": "p"}));
        let sent = send_one(
            &call,
            Some(&generate_body("p", None, "auto")),
            Reply::Response(HttpResponse::json(status, &body)),
        );
        match (class, &sent) {
            ("not_received", Sent::NotReceived { .. }) => {}
            (
                "zero",
                Sent::Failed {
                    cost: Some(Usd::ZERO),
                    retryable: false,
                    ..
                },
            ) => {}
            (
                "unknown",
                Sent::Failed {
                    cost: None,
                    retryable: false,
                    ..
                },
            ) => {}
            (
                "retry",
                Sent::Failed {
                    cost: None,
                    retryable: true,
                    ..
                },
            ) => {}
            (
                "redirect",
                Sent::Failed {
                    cost: None,
                    retryable: false,
                    reason,
                },
            ) => assert_eq!(
                reason,
                "OpenRouter image generation was redirected (HTTP 302); check OPENROUTER_BASE_URL"
            ),
            _ => panic!("HTTP {status}: expected {class}, got {sent:?}"),
        }
    }
}

#[test]
fn a_status_reason_names_the_safe_detail_and_request_id() {
    let body = json!({"error": {"code": 402, "message": "Insufficient credits for this prompt",
                                "metadata": {"provider_name": "OpenAI"}}});
    let response = HttpResponse::json(402, &body).with_header("x-request-id", "req-42");
    let (reason, cost, retryable) = failed(send_one(
        &generate(json!({"prompt": "a secret prompt"})),
        None,
        Reply::Response(response),
    ));
    assert_eq!(
        reason,
        "OpenRouter image generation returned HTTP 402: code=402 (request req-42)"
    );
    assert_eq!((cost, retryable), (Some(Usd::ZERO), false));
}

// I5 -------------------------------------------------------------------------------------------

#[test]
fn transport_phases_decide_the_outcome() {
    let call = generate(json!({"prompt": "p"}));
    let body = generate_body("p", None, "auto");
    let sent = send_one(
        &call,
        Some(&body),
        Reply::Error(TransportError::not_sent(
            TransportErrorKind::Connect,
            "connection refused",
        )),
    );
    assert!(matches!(sent, Sent::NotReceived { .. }), "{sent:?}");
    let sent = send_one(
        &call,
        Some(&body),
        Reply::Error(TransportError::after_send(
            TransportErrorKind::Other,
            "connection reset",
        )),
    );
    assert_eq!(
        failed(sent),
        (
            "OpenRouter image generation failed: connection reset".into(),
            None,
            true
        )
    );
    let sent = send_one(
        &call,
        Some(&body),
        Reply::Error(TransportError::after_send(
            TransportErrorKind::Timeout,
            "the deadline passed",
        )),
    );
    assert!(matches!(
        sent,
        Sent::Failed {
            cost: None,
            retryable: true,
            ..
        }
    ));
}

#[test]
fn a_response_over_the_cap_is_a_failed_attempt() {
    let sent = send_one(
        &generate(json!({"prompt": "p"})),
        None,
        Reply::Zeros {
            status: 200,
            headers: Vec::new(),
            len: 64 * 1024 * 1024 + 1,
        },
    );
    let (reason, cost, retryable) = failed(sent);
    assert!(reason.contains("larger than 67108864 bytes"), "{reason}");
    assert_eq!((cost, retryable), (None, true));
}

#[test]
fn a_transport_that_refuses_the_request_is_a_refusal() {
    let adapter = adapter_over(Arc::new(Offline), test_keys());
    let sent = block_on(adapter.send(&generate(json!({"prompt": "p"}))));
    let Sent::Refused { reason } = sent else {
        panic!("expected Refused, got {sent:?}")
    };
    assert_eq!(
        reason,
        "OpenRouter image generation was not sent: the network is off (GRIDA_FX_NETWORK=off)"
    );
}

// I6 -------------------------------------------------------------------------------------------

#[test]
fn bodies_that_cannot_become_an_answer_fail_the_attempt() {
    let png = media::png(1, 1, None);
    let usage = json!({"cost": 0.15});
    let cases: Vec<(HttpResponse, &str, Option<Usd>)> = vec![
        (
            HttpResponse::new(200, b"<html>busy</html>".to_vec()),
            "OpenRouter image generation returned invalid JSON",
            None,
        ),
        (
            HttpResponse::json(200, &json!([{"b64_json": b64(&png)}])),
            "OpenRouter image generation returned a non-object JSON response",
            None,
        ),
        (
            HttpResponse::json(200, &json!({"data": [], "usage": usage})),
            "OpenRouter image generation returned no single image",
            Some(Usd(150_000)),
        ),
        (
            HttpResponse::json(200, &json!({"usage": usage})),
            "OpenRouter image generation returned no single image",
            Some(Usd(150_000)),
        ),
        (
            HttpResponse::json(
                200,
                &json!({"data": [{"b64_json": b64(&png)}, {"b64_json": b64(&png)}]}),
            ),
            "OpenRouter image generation returned no single image",
            None,
        ),
        (
            HttpResponse::json(200, &json!({"data": ["not an object"]})),
            "OpenRouter image generation returned no single image",
            None,
        ),
        (
            HttpResponse::json(
                200,
                &json!({"data": [{"b64_json": "broken", "media_type": "image/png"}], "usage": usage}),
            ),
            "OpenRouter image b64_json is not valid base64",
            Some(Usd(150_000)),
        ),
        (
            HttpResponse::json(
                200,
                &json!({"data": [{"url": "https://cdn.example.test/out.png?sig=abc"}], "usage": usage}),
            ),
            "OpenRouter image b64_json is not valid base64",
            Some(Usd(150_000)),
        ),
        (
            HttpResponse::json(200, &json!({"data": [{"b64_json": ""}]})),
            "OpenRouter image b64_json is not valid base64",
            None,
        ),
        (
            HttpResponse::json(
                200,
                &json!({"data": [{"b64_json": b64(b"GIF89asynthetic")}]}),
            ),
            "OpenRouter image response omitted media_type and bytes are not PNG, JPEG, or WebP",
            None,
        ),
        (
            HttpResponse::json(200, &json!({"data": [{"b64_json": b64(b"plain bytes")}]})),
            "OpenRouter image response omitted media_type and bytes are not PNG, JPEG, or WebP",
            None,
        ),
    ];
    for (response, reason, cost) in cases {
        let sent = send_one(
            &generate(json!({"prompt": "p"})),
            None,
            Reply::Response(response),
        );
        assert_eq!(failed(sent), (reason.to_string(), cost, true), "{reason}");
    }
}

#[test]
fn rejected_media_types_fail_the_attempt() {
    let cases: [(Value, &[u8]); 5] = [
        (json!("image/gif"), b"GIF89aforbidden"),
        (
            json!("image/png; charset=binary"),
            b"\x89PNG\r\n\x1a\nparameterized",
        ),
        (json!("image/bmp"), b"BMunsupported"),
        (json!("image/jpg"), b"\xff\xd8\xffalias"),
        (Value::Null, b"\x89PNG\r\n\x1a\nexplicit-null"),
    ];
    for (media_type, bytes) in cases {
        let response = HttpResponse::json(
            200,
            &json!({"data": [{"b64_json": b64(bytes), "media_type": media_type}],
                    "usage": {"cost": 0.2}}),
        )
        .with_header("x-request-id", "img-7");
        let sent = send_one(
            &generate(json!({"prompt": "p"})),
            None,
            Reply::Response(response),
        );
        assert_eq!(
            failed(sent),
            (
                "OpenRouter image media type must be parameter-free PNG, JPEG, or WebP (request img-7)"
                    .into(),
                Some(Usd(200_000)),
                true
            ),
            "{media_type}"
        );
    }
}

#[test]
fn kinds_are_declared_or_sniffed() {
    let cases: [(Option<&str>, &[u8], &str); 6] = [
        (None, b"\x89PNG\r\n\x1a\nsynthetic", "image/png"),
        (None, b"\xff\xd8\xffsynthetic", "image/jpeg"),
        (None, b"RIFF\x04\x00\x00\x00WEBPsynthetic", "image/webp"),
        (Some("image/jpeg"), b"\xff\xd8\xffsynthetic", "image/jpeg"),
        (
            Some("IMAGE/WEBP"),
            b"RIFF\x04\x00\x00\x00WEBPsynthetic",
            "image/webp",
        ),
        // A declared kind is kept as declared; the check judges the bytes.
        (Some("image/png"), b"\xff\xd8\xffsynthetic", "image/png"),
    ];
    for (declared, bytes, kind) in cases {
        let sent = send_one(
            &generate(json!({"prompt": "p"})),
            None,
            Reply::Response(image_answer(bytes, declared, None)),
        );
        let answer = answered(sent);
        assert_eq!(answer.files["image"].kind, kind);
        assert_eq!(answer.files["image"].bytes, bytes);
    }
}

#[test]
fn a_malformed_revised_prompt_is_ignored() {
    let png = media::png(1, 1, None);
    let response = HttpResponse::json(
        200,
        &json!({"revised_prompt": "", "data": [{"b64_json": b64(&png), "revised_prompt": 7}]}),
    );
    let answer = answered(send_one(
        &generate(json!({"prompt": "p"})),
        None,
        Reply::Response(response),
    ));
    assert_eq!(answer.data, Value::Null);
}

// I7 -------------------------------------------------------------------------------------------

#[test]
fn checks_refuse_what_the_route_did_not_ask_for() {
    let adapter = adapter_over(Arc::new(NoNetwork), test_keys());
    let image =
        |kind: &str, bytes: Vec<u8>| Answer::new(Value::Null, None).with_file("image", kind, bytes);
    let wide = generate(json!({"prompt": "p", "size": "2560x1440", "background": "opaque"}));
    assert_eq!(
        adapter.check(&wide, &image("image/jpeg", media::JPEG_HEAD.to_vec())),
        Err("the answer is image/jpeg, not image/png".into())
    );
    assert_eq!(
        adapter.check(&wide, &image("image/png", media::JPEG_HEAD.to_vec())),
        Err("the answer is image/jpeg, not image/png".into())
    );
    assert_eq!(
        adapter.check(&wide, &image("image/png", media::png(1024, 1024, None))),
        Err("the image is 1024x1024, not 2560x1440".into())
    );
    let opaque = generate(json!({"prompt": "p", "background": "opaque"}));
    assert_eq!(
        adapter.check(
            &opaque,
            &image("image/png", media::png_one_pixel_alpha(4, 4, 128))
        ),
        Err("the picture asked for as opaque has transparent pixels".into())
    );
    let auto = generate(json!({"prompt": "p"}));
    assert_eq!(
        adapter.check(
            &auto,
            &image("image/png", media::png_one_pixel_alpha(4, 4, 128))
        ),
        Ok(())
    );
    let mut truncated = media::png(8, 8, Some(255));
    truncated.truncate(truncated.len() - 20);
    assert_eq!(
        adapter.check(&auto, &image("image/png", truncated)),
        Err("the image data is not decodable".into())
    );
    assert_eq!(
        adapter.check(
            &auto,
            &image("image/png", b"\x89PNG\r\n\x1a\ntruncated".to_vec())
        ),
        Err("the image data is not decodable".into())
    );
    assert_eq!(
        adapter.check(&auto, &Answer::new(Value::Null, None)),
        Err("the answer has no image".into())
    );
}

/// spec/capabilities.md §2 check 2 ("it decodes"): the image must decode fully, including an RGB
/// PNG, whose file facts need only its header.
#[test]
fn a_truncated_rgb_png_is_refused() {
    let adapter = adapter_over(Arc::new(NoNetwork), test_keys());
    let mut truncated = media::png(8, 8, None);
    truncated.truncate(truncated.len() - 20);
    let answer = Answer::new(Value::Null, None).with_file("image", "image/png", truncated);
    assert_eq!(
        adapter.check(&generate(json!({"prompt": "p"})), &answer),
        Err("the image data is not decodable".into())
    );
}

#[test]
fn a_wrong_size_from_the_provider_is_answered_then_refused_by_the_check() {
    let call = generate(json!({"prompt": "p", "size": "2560x1440"}));
    let adapter = adapter_over(Arc::new(NoNetwork), test_keys());
    let answer = answered(send_one(
        &call,
        Some(&generate_body("p", Some("2560x1440"), "auto")),
        Reply::Response(image_answer(
            &media::png(1024, 1024, None),
            Some("image/png"),
            Some(json!({"cost": 0.14})),
        )),
    ));
    assert_eq!(answer.cost, Some(Usd(140_000)));
    assert_eq!(
        adapter.check(&call, &answer),
        Err("the image is 1024x1024, not 2560x1440".into())
    );
}

// I8 -------------------------------------------------------------------------------------------

#[test]
fn costs_come_from_usage_cost_rounded_up() {
    let png = media::png(1, 1, None);
    let cases: Vec<(Option<Value>, Option<Usd>)> = vec![
        (Some(json!({"cost": 0})), Some(Usd(0))),
        (Some(json!({"cost": 0.210835})), Some(Usd(210_835))),
        (Some(json!({"cost": 1e-7})), Some(Usd(1))),
        (Some(json!({"cost": 0.00012345})), Some(Usd(124))),
        (Some(json!({"cost": true})), None),
        (Some(json!({"cost": -1})), None),
        (Some(json!({"cost": "0.10"})), None),
        (Some(json!({"cost": null})), None),
        (Some(json!({"images": 1})), None),
        (Some(Value::Null), None),
        (Some(json!([0.1])), None),
        (None, None),
    ];
    for (usage, cost) in cases {
        let answer = answered(send_one(
            &generate(json!({"prompt": "p"})),
            None,
            Reply::Response(image_answer(&png, None, usage.clone())),
        ));
        assert_eq!(answer.cost, cost, "{usage:?}");
    }
}

// Credential hygiene -----------------------------------------------------------------------------

#[test]
fn the_key_never_leaves_the_credential_slot() {
    let leaky = json!({"error": {"code": KEY, "type": "auth", "param": KEY,
                                 "message": format!("invalid key {KEY}")}});
    let png = media::png(1, 1, None);
    let responses = vec![
        HttpResponse::json(401, &leaky).with_header("x-request-id", KEY),
        HttpResponse::json(500, &leaky),
        HttpResponse::json(200, &json!({"data": [{"b64_json": KEY}], "note": KEY})),
        HttpResponse::json(
            200,
            &json!({"data": [{"b64_json": b64(&png), "media_type": KEY}]}),
        ),
        image_answer(&png, None, Some(json!({"cost": 0.1, "key": KEY}))),
    ];
    for response in responses {
        let transport = Arc::new(ReplayTransport::new(vec![
            Expect::new(Method::Post, URL, Lane::Provider)
                .credential("authorization", &format!("Bearer {KEY}"))
                .body(ExpectBody::Any)
                .reply(response),
        ]));
        let call = generate(json!({"prompt": "p"}));
        let sent = block_on(adapter(&transport).send(&call));
        let shown = match &sent {
            Sent::Answered(answer) => answer.data.to_string(),
            Sent::Failed { reason, .. }
            | Sent::NotReceived { reason, .. }
            | Sent::Refused { reason } => reason.clone(),
        };
        assert!(!shown.contains(KEY), "{shown}");
        for request in transport.requests() {
            assert!(!request.url.contains(KEY));
            assert!(request.headers.iter().all(|(_, v)| !v.contains(KEY)));
            let credential = request
                .credential
                .expect("the provider lane carries the key");
            assert_eq!(credential.header, "authorization");
            assert_eq!(credential.prefix, "Bearer ");
            assert!(!format!("{:?}", request.body).contains(KEY));
        }
    }
}
