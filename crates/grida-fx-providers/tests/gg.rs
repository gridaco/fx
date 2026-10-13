//! Grida Gateway's adapters (src/gg/README.md) over synthetic exchanges and a `FakeClock`.
//! Nothing here reaches a network: every send goes to a `ReplayTransport` that asserts the request,
//! and every refusal runs over a transport that saw no request. Pictures are built in code
//! (`testing::media`); the clip is the committed facts vector (`spec/vectors/facts/mp4/`).

use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{
    Answer, CallRequest, Collected, LongJob, RequestAdapter, Sent, Submitted, Usage,
};
use grida_fx_providers::clock::{Clock, FakeClock};
use grida_fx_providers::gg::chat::{GgAgent, GgStructured};
use grida_fx_providers::gg::image::GgImages;
use grida_fx_providers::gg::mesh::{GgMesh, GgRig};
use grida_fx_providers::gg::video::{self, GgVideo, OUTSTANDING};
use grida_fx_providers::gg::{self, Gateway};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::testing::{
    CallBuilder, TestCall, block_on, media, setup, setup_with_clock, test_keys,
};
use grida_fx_providers::transport::replay::{
    Exchange, Expect, ExpectBody, NoNetwork, ReplayTransport,
};
use grida_fx_providers::transport::{
    HttpResponse, Lane, Method, Part, Transport, TransportError, TransportErrorKind,
};
use grida_fx_providers::wire;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const BASE: &str = "https://gateway.grida.ai/v1";
const BEARER: &str = "Bearer test-gg-key";
const SESSION: &str = "0123456789abcdef";
/// The `Idempotency-Key` of attempt 1 of a test call (its key is 64 zeros).
const IDEMPOTENCY: &str = "fx-0123456789abcdef-0000000000000000-1";
const IMAGE_ROUTE: &str = "openai/gpt-image-2.5-sunburst@gg";
const TEXT_ROUTE: &str = "openai/gpt-5.6-sol@gg";
const VIDEO_ROUTE: &str = "alibaba/wan-3.0@gg";
const JOB: &str = "req_Job-1.a";
const VIDEO_URL: &str = "https://media.example.test/files/clip.mp4?sig=abc";
/// 64×48, 24 fps, 2 s (spec/vectors/facts/mp4/expected.json).
const CLIP: &[u8] = include_bytes!("../../../spec/vectors/facts/mp4/h264_24fps.mp4");

fn gateway(transport: Arc<dyn Transport>, keys: Keys) -> Gateway {
    Gateway::with_session(&setup(transport, keys), SESSION)
}

fn replay(exchanges: Vec<Exchange>) -> Arc<ReplayTransport> {
    Arc::new(ReplayTransport::new(exchanges))
}

/// A paid POST: the credential and attempt 1's `Idempotency-Key`; any body unless a test says.
fn post(path: &str) -> Expect {
    Expect::new(Method::Post, format!("{BASE}/{path}"), Lane::Provider)
        .credential("authorization", BEARER)
        .header("idempotency-key", IDEMPOTENCY)
        .body(ExpectBody::Any)
}

fn image_contract() -> Value {
    json!({"adapter": "gg-image", "adapter_behavior": "1", "quality": "max"})
}

fn image_call(capability: &str, request: Value) -> TestCall {
    CallBuilder::new(capability, IMAGE_ROUTE)
        .contract(image_contract())
        .request(request)
        .build()
}

fn picture_answer(png: &[u8], reserved: &str) -> HttpResponse {
    let (_, b64) = wire::data_url("image/png", png)
        .split_once(',')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .unwrap();
    HttpResponse::json(
        200,
        &json!({"created": 1, "data": [{"b64_json": b64, "media_type": "image/png"}]}),
    )
    .with_header("x-gg-reserved-mills", reserved)
    .with_header("x-gg-price-revision", "rev-1")
    .with_header("x-request-id", "req_img-1")
}

fn answered(sent: Sent) -> Answer {
    match sent {
        Sent::Answered(answer) => answer,
        other => panic!("expected an answer, got {other:?}"),
    }
}

fn envelope(status: u16, code: &str) -> HttpResponse {
    HttpResponse::json(
        status,
        &json!({"error": {"code": code, "message": "m", "param": null, "type": "invalid_request_error"}}),
    )
}

// ---------------------------------------------------------------------------------------------
// Images

#[test]
fn a_generation_sends_the_model_s_members_and_answers_with_the_image() {
    let png = media::png(4, 4, Some(0));
    let transport = replay(vec![
        post("images/generations")
            .body(ExpectBody::JsonText(
                r#"{"model":"openai/gpt-image-2.5-sunburst","prompt":"A kite.","n":1,"size":"1024x1024","quality":"max","background":"transparent"}"#.into(),
            ))
            .reply(picture_answer(&png, "1000")),
    ]);
    let adapter = GgImages::new(gateway(transport.clone(), test_keys()));
    let call = image_call(
        "image.generate",
        json!({"prompt": "A kite.", "size": "1024x1024", "background": "transparent"}),
    );
    let answer = answered(block_on(adapter.send(&call)));
    transport.assert_done();
    assert_eq!(answer.files["image"].kind, "image/png");
    assert_eq!(answer.files["image"].bytes, png);
    assert_eq!(answer.data, Value::Null);
    assert_eq!(
        answer.cost, None,
        "a reservation is not a charge, and nothing settled"
    );
    let request = &transport.requests()[0];
    assert_eq!(request.timeout, Duration::from_secs(630));
    assert_eq!(request.max_response_bytes, 64 * 1024 * 1024);
    // The answer is 4x4, not the size asked for: the check refuses it, as every image route's does.
    assert_eq!(
        adapter.check(&call, &answer).unwrap_err(),
        "the image is 4x4, not 1024x1024"
    );
    let small = image_call(
        "image.generate",
        json!({"prompt": "A kite.", "background": "transparent"}),
    );
    assert_eq!(adapter.check(&small, &answer), Ok(()));
}

#[test]
fn what_the_model_chooses_is_left_out() {
    let transport = replay(vec![
        post("images/generations")
            .body(ExpectBody::JsonText(
                r#"{"model":"openai/gpt-image-2.5-sunburst","prompt":"A kite.","n":1,"quality":"max"}"#.into(),
            ))
            .reply(picture_answer(&media::png(2, 2, Some(255)), "8000")),
    ]);
    let adapter = GgImages::new(gateway(transport.clone(), test_keys()));
    let call = image_call(
        "image.generate",
        json!({"prompt": "A kite.", "size": "auto", "background": "auto"}),
    );
    assert!(matches!(block_on(adapter.send(&call)), Sent::Answered(_)));
    transport.assert_done();
    // A route that names no quality sends none.
    let transport = replay(vec![
        post("images/generations")
            .body(ExpectBody::JsonText(
                r#"{"model":"acme/img-a","prompt":"A kite.","n":1}"#.into(),
            ))
            .reply(picture_answer(&media::png(2, 2, Some(255)), "35")),
    ]);
    let adapter = GgImages::new(gateway(transport.clone(), test_keys()));
    let call = CallBuilder::new("image.generate", "acme/img-a@gg")
        .request(json!({"prompt": "A kite."}))
        .build();
    let answer = answered(block_on(adapter.send(&call)));
    transport.assert_done();
    assert_eq!(answer.cost, None);
}

#[test]
fn an_edit_sends_its_pictures_as_parts_in_order() {
    let first = media::png(3, 3, Some(255));
    let second = media::png(2, 2, None);
    let mut builder = CallBuilder::new("image.edit", IMAGE_ROUTE).contract(image_contract());
    let image = builder.file("image/png", &first);
    let reference = builder.file("image/png", &second);
    let call = builder
        .request(
            json!({"prompt": "Make it blue.", "background": "opaque", "size": "1536x1024",
                        "image": image, "references": [reference]}),
        )
        .build();
    let transport = replay(vec![
        post("images/edits")
            .body(ExpectBody::Multipart(vec![
                Part::text("model", "openai/gpt-image-2.5-sunburst"),
                Part::text("prompt", "Make it blue."),
                Part::text("n", "1"),
                Part::text("size", "1536x1024"),
                Part::text("quality", "max"),
                Part::text("background", "opaque"),
                Part::file("image[]", "reference-01.png", "image/png", first),
                Part::file("image[]", "reference-02.png", "image/png", second),
            ]))
            .reply(picture_answer(&media::png(2, 2, Some(255)), "2250")),
    ]);
    let adapter = GgImages::new(gateway(transport.clone(), test_keys()));
    let answer = answered(block_on(adapter.send(&call)));
    transport.assert_done();
    assert_eq!(answer.cost, None);
}

#[test]
fn image_refusals_send_nothing() {
    let refused = |call: &TestCall, keys: Keys| {
        let adapter = GgImages::new(gateway(Arc::new(NoNetwork), keys));
        match block_on(adapter.send(call)) {
            Sent::Refused { reason } => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    };
    let ok = json!({"prompt": "A kite."});
    assert_eq!(
        refused(&image_call("image.generate", ok.clone()), Keys::none()),
        "GG_API_KEY is not set"
    );
    let foreign = CallBuilder::new("image.generate", IMAGE_ROUTE)
        .contract(json!({"adapter": "fx-openai-image-v1", "adapter_behavior": "1"}))
        .request(ok.clone())
        .build();
    assert_eq!(
        refused(&foreign, test_keys()),
        "openai/gpt-image-2.5-sunburst@gg is not a image.generate route this adapter serves"
    );
    let number = CallBuilder::new("image.generate", IMAGE_ROUTE)
        .contract(json!({"adapter": "gg-image", "adapter_behavior": 1}))
        .request(ok.clone())
        .build();
    assert!(refused(&number, test_keys()).contains("is not a image.generate route"));
    let quality = CallBuilder::new("image.generate", IMAGE_ROUTE)
        .contract(json!({"adapter": "gg-image", "quality": 9}))
        .request(ok.clone())
        .build();
    assert!(refused(&quality, test_keys()).ends_with(": quality is text"));
    assert_eq!(
        refused(
            &image_call("image.generate", json!({"prompt": "p", "size": "2K"})),
            test_keys()
        ),
        "GG image size must be auto or WIDTHxHEIGHT"
    );
    // A mask, and a picture GG does not take.
    let mut builder = CallBuilder::new("image.edit", IMAGE_ROUTE).contract(image_contract());
    let image = builder.file("image/png", &media::png(2, 2, None));
    let mask = builder.file("image/png", &media::png(2, 2, Some(0)));
    let call = builder
        .request(json!({"prompt": "p", "image": image, "mask": mask}))
        .build();
    assert_eq!(refused(&call, test_keys()), "GG image edits take no mask");
    let mut builder = CallBuilder::new("image.edit", IMAGE_ROUTE).contract(image_contract());
    let image = builder.file("image/gif", b"GIF89a");
    let call = builder
        .request(json!({"prompt": "p", "image": image}))
        .build();
    assert_eq!(
        refused(&call, test_keys()),
        "image is image/gif; GG takes PNG, JPEG or WebP"
    );
    let mut builder = CallBuilder::new("image.edit", IMAGE_ROUTE).contract(image_contract());
    let image = builder.file("image/png", b"not a png");
    let call = builder
        .request(json!({"prompt": "p", "image": image}))
        .build();
    assert_eq!(
        refused(&call, test_keys()),
        "image is not a decodable image/png"
    );
}

#[test]
fn image_statuses_follow_gg_s_table() {
    let outcome = |response: HttpResponse| {
        let transport = replay(vec![post("images/generations").reply(response)]);
        let adapter = GgImages::new(gateway(transport.clone(), test_keys()));
        let sent = block_on(adapter.send(&image_call("image.generate", json!({"prompt": "p"}))));
        transport.assert_done();
        sent
    };
    assert_eq!(
        outcome(envelope(402, "insufficient_credit").with_header("x-request-id", "req_1")),
        Sent::Failed {
            reason: "GG image generation returned HTTP 402: type=invalid_request_error; \
                     code=insufficient_credit (request req_1); add Grida AI credits at \
                     https://console.grida.co"
                .into(),
            cost: Some(Usd::ZERO),
            retryable: false,
        }
    );
    assert!(matches!(
        outcome(envelope(400, "invalid_request")),
        Sent::Failed {
            cost: Some(Usd::ZERO),
            retryable: false,
            ..
        }
    ));
    assert!(matches!(
        outcome(envelope(429, "concurrency_limit_exceeded").with_header("retry-after", "2")),
        Sent::NotReceived {
            retry_after: Some(_),
            ..
        }
    ));
    assert!(matches!(
        outcome(envelope(403, "organization_inactive")),
        Sent::Failed {
            cost: Some(Usd::ZERO),
            retryable: false,
            ..
        }
    ));
    assert!(matches!(
        outcome(envelope(503, "temporarily_unavailable")),
        Sent::Failed {
            cost: None,
            retryable: true,
            ..
        }
    ));
    // A 2xx that is not an answer: billed, and another attempt may follow.
    assert_eq!(
        outcome(HttpResponse::json(200, &json!({"data": []})).with_header("x-request-id", "req_2")),
        Sent::Failed {
            reason: "GG image generation returned no single image (request req_2)".into(),
            cost: None,
            retryable: true,
        }
    );
    let transport = replay(vec![post("images/generations").fail(
        TransportError::not_sent(TransportErrorKind::Connect, "connection refused"),
    )]);
    let adapter = GgImages::new(gateway(transport.clone(), test_keys()));
    assert!(matches!(
        block_on(adapter.send(&image_call("image.generate", json!({"prompt": "p"})))),
        Sent::NotReceived { .. }
    ));
}

#[test]
fn every_send_of_one_attempt_carries_one_key() {
    let transport = replay(vec![
        post("images/generations").reply(envelope(429, "rate_limit_exceeded")),
        post("images/generations").reply(picture_answer(&media::png(2, 2, Some(255)), "1")),
    ]);
    let adapter = GgImages::new(gateway(transport.clone(), test_keys()));
    let call = image_call("image.generate", json!({"prompt": "p"}));
    assert!(matches!(
        block_on(adapter.send(&call)),
        Sent::NotReceived { .. }
    ));
    assert!(matches!(block_on(adapter.send(&call)), Sent::Answered(_)));
    transport.assert_done();
    // A new attempt is new work, under a new key.
    let mut second = image_call("image.generate", json!({"prompt": "p"}));
    second.call.attempt = 2;
    let transport = replay(vec![
        Expect::new(
            Method::Post,
            format!("{BASE}/images/generations"),
            Lane::Provider,
        )
        .credential("authorization", BEARER)
        .header("idempotency-key", "fx-0123456789abcdef-0000000000000000-2")
        .body(ExpectBody::Any)
        .reply(picture_answer(&media::png(2, 2, Some(255)), "1")),
    ]);
    let adapter = GgImages::new(gateway(transport.clone(), test_keys()));
    assert!(matches!(block_on(adapter.send(&second)), Sent::Answered(_)));
    transport.assert_done();
}

#[test]
fn a_busy_image_slot_renews_the_attempt_s_key() {
    let busy = HttpResponse::json(
        429,
        &json!({"error": {"code": "concurrency_limit_exceeded", "message": "m", "param": null,
                "type": "invalid_request_error", "request_not_admitted": true,
                "retry_with_new_idempotency_key": true}}),
    )
    .with_header("retry-after", "3");
    let renewed = |key: &str| {
        Expect::new(
            Method::Post,
            format!("{BASE}/images/generations"),
            Lane::Provider,
        )
        .credential("authorization", BEARER)
        .header("idempotency-key", key)
        .body(ExpectBody::Any)
    };
    let transport = replay(vec![
        post("images/generations").reply(busy.clone()),
        renewed(&format!("{IDEMPOTENCY}.1")).reply(busy),
        renewed(&format!("{IDEMPOTENCY}.2"))
            .reply(picture_answer(&media::png(2, 2, Some(255)), "1000")),
    ]);
    let adapter = GgImages::new(gateway(transport.clone(), test_keys()));
    let call = image_call("image.generate", json!({"prompt": "p"}));
    let Sent::NotReceived {
        reason,
        retry_after,
    } = block_on(adapter.send(&call))
    else {
        panic!("not received")
    };
    assert_eq!(retry_after, Some(Duration::from_secs(3)));
    assert!(
        reason.ends_with("nothing was admitted; it is sent again under a new Idempotency-Key"),
        "{reason}"
    );
    assert!(matches!(
        block_on(adapter.send(&call)),
        Sent::NotReceived { .. }
    ));
    assert!(matches!(block_on(adapter.send(&call)), Sent::Answered(_)));
    transport.assert_done();
}

// ---------------------------------------------------------------------------------------------
// Charges

fn billing_get() -> Expect {
    Expect::new(
        Method::Get,
        format!("{BASE}/requests/req_text-1/billing"),
        Lane::Provider,
    )
    .credential("authorization", BEARER)
}

fn billing(status: &str, charged: Value) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"object": "request_billing", "request_id": "req_text-1", "billing": {
            "status": status, "currency": "USD", "unit": "mill", "price_revision": "rev-1",
            "authorized_mills": "16.5928", "reserved_mills": if status == "settled" { "0" } else { "16.5928" },
            "charged_mills": charged, "refunded_mills": "0", "usage": null}}),
    )
}

/// A structured call answered with `answer`'s headers, over `exchanges` after it, on `clock`.
fn priced_structured(
    answer: HttpResponse,
    exchanges: Vec<Exchange>,
    clock: &Arc<FakeClock>,
) -> Answer {
    let mut all = vec![post("chat/completions").reply(answer)];
    all.extend(exchanges);
    let transport = replay(all);
    let setup = setup_with_clock(transport.clone(), test_keys(), clock.clone());
    let adapter = GgStructured::new(Gateway::with_session(&setup, SESSION));
    let call = CallBuilder::new("structured.generate", TEXT_ROUTE)
        .request(json!({"prompt": "p", "schema": {"type": "object"}}))
        .build();
    let answer = answered(block_on(adapter.send(&call)));
    transport.assert_done();
    answer
}

fn text_answer() -> HttpResponse {
    completion(json!({"role": "assistant", "content": "{}"}))
}

#[test]
fn a_charge_gg_settled_is_the_answer_s_cost() {
    let png = media::png(2, 2, Some(255));
    let transport = replay(vec![
        post("images/generations").reply(
            picture_answer(&png, "1000")
                .with_header("x-gg-billing-status", "settled")
                .with_header("x-gg-charged-mills", "185.5")
                .with_header("x-gg-billing-url", "/v1/requests/req_img-1/billing"),
        ),
    ]);
    let adapter = GgImages::new(gateway(transport.clone(), test_keys()));
    let answer = answered(block_on(
        adapter.send(&image_call("image.generate", json!({"prompt": "p"}))),
    ));
    transport.assert_done();
    assert_eq!(answer.cost, Some(Usd(185_500)), "nothing more is read");
}

#[test]
fn a_charge_not_yet_settled_is_read_from_its_billing() {
    let clock = Arc::new(FakeClock::new());
    let answer = priced_structured(
        text_answer()
            .with_header("x-gg-billing-status", "reserved")
            .with_header("x-gg-billing-url", "/v1/requests/req_text-1/billing"),
        vec![
            billing_get().reply(billing("pending", json!(null))),
            billing_get().reply(envelope(503, "settlement_pending")),
            billing_get().fail(TransportError::after_send(
                TransportErrorKind::Timeout,
                "slow",
            )),
            billing_get().reply(billing("settled", json!("0.4"))),
        ],
        &clock,
    );
    assert_eq!(answer.cost, Some(Usd(400)));
    assert_eq!(
        clock.sleeps(),
        [1, 2, 4, 8].map(Duration::from_secs),
        "waits double from a second"
    );
}

#[test]
fn billing_reads_end_at_a_refusal_or_the_wait() {
    let named = || text_answer().with_header("x-gg-billing-url", "/v1/requests/req_text-1/billing");
    // A refusal ends the wait: the authorization stays the bound.
    let clock = Arc::new(FakeClock::new());
    let answer = priced_structured(
        named(),
        vec![billing_get().reply(envelope(404, "not_found"))],
        &clock,
    );
    assert_eq!(
        answer.cost, None,
        "unknown: the engine charges the whole hold"
    );
    assert_eq!(clock.sleeps(), [Duration::from_secs(1)]);
    // A read about another request ends it too.
    let mut other = billing("settled", json!("1"));
    other.body = serde_json::to_vec(&json!({"object": "request_billing", "request_id": "req_x",
        "billing": {"status": "settled", "charged_mills": "1"}}))
    .unwrap();
    let answer = priced_structured(
        named(),
        vec![billing_get().reply(other)],
        &Arc::new(FakeClock::new()),
    );
    assert_eq!(answer.cost, None);
    // Billing that never settles: read until the wait is over.
    let clock = Arc::new(FakeClock::new());
    let answer = priced_structured(
        named(),
        (0..5)
            .map(|_| billing_get().reply(billing("pending", json!(null))))
            .collect(),
        &clock,
    );
    assert_eq!(answer.cost, None);
    assert_eq!(clock.sleeps(), [1, 2, 4, 8, 8].map(Duration::from_secs));
    assert!(clock.now() <= gg::CHARGE_WAIT);
    // No billing named, or one FX does not read: nothing more is sent.
    for header in [
        None,
        Some("https://elsewhere.example/v1/requests/req_text-1/billing"),
    ] {
        let answer = match header {
            Some(url) => text_answer().with_header("x-gg-billing-url", url),
            None => text_answer(),
        };
        let clock = Arc::new(FakeClock::new());
        assert_eq!(priced_structured(answer, vec![], &clock).cost, None);
        assert!(clock.sleeps().is_empty());
    }
}

// ---------------------------------------------------------------------------------------------
// Text

fn completion(message: Value) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"id": "gen-1", "object": "chat.completion", "created": 1, "model": "m",
                "choices": [{"index": 0, "message": message, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 39, "completion_tokens": 12, "total_tokens": 51,
                          "prompt_tokens_details": {"cached_tokens": 0},
                          "cost": 0.000396}}),
    )
    .with_header("x-gg-reserved-mills", "16.5928")
    .with_header("x-request-id", "req_text-1")
}

#[test]
fn a_structured_call_sends_a_strict_schema_and_answers_with_its_value() {
    let schema = json!({"title": "colour", "type": "object",
                        "properties": {"name": {"type": "string", "minLength": 1}},
                        "required": ["name"], "additionalProperties": false});
    let transport = replay(vec![
        post("chat/completions")
            .body(ExpectBody::JsonText(
                r#"{"model":"openai/gpt-5.6-sol","messages":[{"role":"system","content":"Answer briefly."},{"role":"user","content":"Name a colour."}],"response_format":{"type":"json_schema","json_schema":{"name":"colour","strict":true,"schema":{"title":"colour","type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}}},"max_completion_tokens":16000}"#.into(),
            ))
            .reply(completion(json!({"role": "assistant", "content": "{\"name\": \"red\"}"}))),
    ]);
    let adapter = GgStructured::new(gateway(transport.clone(), test_keys()));
    let call = CallBuilder::new("structured.generate", TEXT_ROUTE)
        .contract(json!({"adapter": "gg-structured", "adapter_behavior": 1}))
        .request(json!({"prompt": "Name a colour.", "system": "Answer briefly.", "schema": schema}))
        .build();
    let answer = answered(block_on(adapter.send(&call)));
    transport.assert_done();
    assert_eq!(answer.data, json!({"json": {"name": "red"}}));
    assert_eq!(answer.cost, None, "usage.cost is not GG's charge");
    assert_eq!(
        answer.usage.as_deref(),
        Some(&Usage {
            input_tokens: Some(39),
            cached_input_tokens: Some(0),
            output_tokens: Some(12),
            ..Usage::default()
        })
    );
    assert_eq!(adapter.check(&call, &answer), Ok(()));
    let request = &transport.requests()[0];
    assert_eq!(request.timeout, Duration::from_secs(900));
    let empty = Answer::new(json!({"json": {"name": ""}}), None);
    assert!(adapter.check(&call, &empty).is_err());
}

#[test]
fn structured_pictures_take_the_contract_s_detail() {
    let png = media::png(2, 2, None);
    let mut builder = CallBuilder::new("structured.generate", TEXT_ROUTE).contract(
        json!({"adapter": "gg-structured", "adapter_behavior": 1, "image_detail": "high",
               "reasoning_effort": "high", "pictures": "unchanged"}),
    );
    let picture = builder.file("image/png", &png);
    let call = builder
        .request(
            json!({"prompt": "Look.", "schema": {"type": "object"}, "context": [picture],
                        "max_tokens": 64}),
        )
        .build();
    let transport = replay(vec![
        post("chat/completions")
            .body(ExpectBody::Json(json!({
                "model": "openai/gpt-5.6-sol",
                "messages": [{"role": "user", "content": [
                    {"type": "text", "text": "Look."},
                    {"type": "image_url", "image_url": {"url": wire::data_url("image/png", &png), "detail": "high"}}
                ]}],
                "response_format": {"type": "json_schema", "json_schema": {"name": "answer", "strict": true, "schema": {"type": "object"}}},
                "max_completion_tokens": 64,
                "reasoning_effort": "high"
            })))
            .reply(completion(json!({"role": "assistant", "content": "{}"}))),
    ]);
    let adapter = GgStructured::new(gateway(transport.clone(), test_keys()));
    assert!(matches!(block_on(adapter.send(&call)), Sent::Answered(_)));
    transport.assert_done();
}

#[test]
fn an_agent_turn_sends_strict_tools_and_answers_with_calls() {
    let transport = replay(vec![
        post("chat/completions")
            .body(ExpectBody::Json(json!({
                "model": "openai/gpt-5.6-sol",
                "messages": [{"role": "system", "content": "Use tools."},
                             {"role": "user", "content": "What is 2+3?"}],
                "tools": [{"type": "function", "function": {"name": "submit", "description": "Submit.",
                    "parameters": {"type": "object", "properties": {"answer": {"type": "integer"}},
                                   "required": ["answer"], "additionalProperties": false},
                    "strict": true}}],
                "tool_choice": "required",
                "max_completion_tokens": 300
            })))
            .reply(completion(json!({"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "submit", "arguments": "{\"answer\":5}"}}
            ]}))),
    ]);
    let adapter = GgAgent::new(gateway(transport.clone(), test_keys()));
    let call = CallBuilder::new("agent.turn", TEXT_ROUTE)
        .contract(json!({"adapter": "gg-tool-loop", "adapter_behavior": 1}))
        .request(json!({"system": "Use tools.",
                        "messages": [{"role": "user", "content": "What is 2+3?"}],
                        "tools": [{"name": "submit", "description": "Submit.",
                                   "parameters": {"type": "object", "properties": {"answer": {"type": "integer"}}}}],
                        "tool_choice": "required", "max_tokens": 300}))
        .build();
    let answer = answered(block_on(adapter.send(&call)));
    transport.assert_done();
    assert_eq!(
        answer.data,
        json!({"text": "", "tool_calls": [{"id": "call_1", "name": "submit", "arguments": {"answer": 5}}]})
    );
    assert_eq!(answer.cost, None);
    assert_eq!(transport.requests()[0].timeout, Duration::from_secs(600));
}

#[test]
fn text_refusals_send_nothing() {
    let refused = |call: &TestCall| {
        let adapter = GgStructured::new(gateway(Arc::new(NoNetwork), test_keys()));
        match block_on(adapter.send(call)) {
            Sent::Refused { reason } => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    };
    let with = |contract: Value, request: Value| {
        CallBuilder::new("structured.generate", TEXT_ROUTE)
            .contract(contract)
            .request(request)
            .build()
    };
    let ours = json!({"adapter": "gg-structured", "adapter_behavior": 1});
    let request = json!({"prompt": "p", "schema": {"type": "object"}});
    assert_eq!(
        refused(&with(
            json!({"adapter": "gg-structured", "image_detail": "original"}),
            request.clone()
        )),
        "openai/gpt-5.6-sol@gg is not a structured.generate route this adapter serves: image_detail is one of auto, low, high"
    );
    assert_eq!(
        refused(&with(
            json!({"adapter": "gg-structured", "reasoning_effort": "max"}),
            request.clone()
        )),
        "openai/gpt-5.6-sol@gg is not a structured.generate route this adapter serves: reasoning_effort is one of none, minimal, low, medium, high, xhigh"
    );
    assert_eq!(
        refused(&with(
            json!({"adapter": "gg-structured", "request_policy": {}}),
            request.clone()
        )),
        "openai/gpt-5.6-sol@gg is not a structured.generate route this adapter serves: the contract takes no member request_policy"
    );
    assert_eq!(
        refused(&with(
            ours.clone(),
            json!({"prompt": " ", "schema": {"type": "object"}})
        )),
        "a structured call needs its prompt as text"
    );
    assert_eq!(
        refused(&with(
            ours,
            json!({"prompt": "x".repeat(21 * 1024 * 1024), "schema": {"type": "object"}})
        )),
        "request body exceeds 20 MiB"
    );
    let agent = GgAgent::new(gateway(Arc::new(NoNetwork), Keys::none()));
    let call = CallBuilder::new("agent.turn", TEXT_ROUTE)
        .request(json!({"system": "s", "messages": [], "tools": [], "tool_choice": "auto"}))
        .build();
    assert_eq!(
        block_on(agent.send(&call)),
        Sent::Refused {
            reason: "GG_API_KEY is not set".into()
        }
    );
}

// ---------------------------------------------------------------------------------------------
// Video

fn video_adapter(transport: &Arc<ReplayTransport>, clock: &Arc<FakeClock>) -> GgVideo {
    let clock: Arc<dyn Clock> = clock.clone();
    let setup = setup_with_clock(transport.clone(), test_keys(), Arc::clone(&clock));
    GgVideo::new(Gateway::with_session(&setup, SESSION), clock)
}

fn video_call(request: Value) -> TestCall {
    CallBuilder::new("video.generate", VIDEO_ROUTE)
        .contract(json!({"adapter": "gg-video", "adapter_behavior": 1}))
        .request(request)
        .build()
}

fn job(status: &str, result_status: &str, billing: Value) -> Value {
    let result = (result_status == "available").then(
        || json!({"videos": [{"url": VIDEO_URL, "media_type": "video/mp4", "expires_at": null}]}),
    );
    json!({"id": JOB, "object": "job", "request_id": JOB, "model": "alibaba/wan-3.0",
           "status": status, "result_status": result_status, "result": result, "error": null,
           "billing": billing, "operation_id": "video.alibaba.wan-3.0.text", "interface": "videos"})
}

fn reserved() -> Value {
    json!({"status": "reserved", "currency": "USD", "unit": "mill", "price_revision": "r",
           "authorized_mills": "6200", "reserved_mills": "6200", "charged_mills": null,
           "refunded_mills": "0", "usage": null})
}

fn settled(charged: &str) -> Value {
    json!({"status": "settled", "currency": "USD", "unit": "mill", "price_revision": "r",
           "authorized_mills": "6200", "reserved_mills": "0", "charged_mills": charged,
           "refunded_mills": "0", "usage": null})
}

fn job_get() -> Expect {
    Expect::get(format!("{BASE}/jobs/{JOB}")).credential("authorization", BEARER)
}

fn job_reply(value: Value) -> Exchange {
    job_get().reply(HttpResponse::json(200, &value))
}

fn clip() -> Exchange {
    Expect::download(VIDEO_URL).reply(HttpResponse::new(200, CLIP.to_vec()))
}

fn collect(
    transport: &Arc<ReplayTransport>,
    clock: &Arc<FakeClock>,
    call: &CallRequest,
) -> Collected {
    let collected =
        block_on(video_adapter(transport, clock).collect(call, &json!({"job_id": JOB})));
    transport.assert_done();
    collected
}

#[test]
fn a_video_is_submitted_once_with_its_tier() {
    let transport = replay(vec![
        post("videos")
            .body(ExpectBody::JsonText(
                r#"{"model":"alibaba/wan-3.0","prompt":"A paper boat drifts.","duration":2,"aspectRatio":"16:9","providerOptions":{"gg":{"resolutionTier":"480p"}}}"#.into(),
            ))
            .reply(
                HttpResponse::json(202, &job("accepted", "pending", reserved()))
                    .with_header("location", &format!("/v1/jobs/{JOB}")),
            ),
    ]);
    let adapter = video_adapter(&transport, &Arc::new(FakeClock::new()));
    let call = video_call(json!({"prompt": "A paper boat drifts.", "duration": 2,
                                 "resolution": "480p", "aspect_ratio": "16:9"}));
    assert_eq!(
        block_on(adapter.submit(&call)),
        Submitted::Accepted {
            handle: json!({"job_id": JOB})
        }
    );
    transport.assert_done();
}

#[test]
fn video_submit_statuses() {
    let submitted = |reply: Result<HttpResponse, TransportError>| {
        let exchange = match reply {
            Ok(response) => post("videos").reply(response),
            Err(error) => post("videos").fail(error),
        };
        let transport = replay(vec![exchange]);
        let adapter = video_adapter(&transport, &Arc::new(FakeClock::new()));
        let outcome = block_on(adapter.submit(&video_call(json!({"prompt": "p"}))));
        transport.assert_done();
        outcome
    };
    assert_eq!(
        submitted(Ok(
            HttpResponse::json(200, &json!({"object": "job"})).with_header("x-request-id", "req_9")
        )),
        Submitted::Uncertain {
            reason: format!(
                "GG took the video job but returned no job to collect it by (request req_9) \
                 (Idempotency-Key {IDEMPOTENCY})"
            )
        }
    );
    assert!(matches!(
        submitted(Ok(envelope(502, "upstream_unavailable"))),
        Submitted::Uncertain { .. }
    ));
    assert!(matches!(
        submitted(Err(TransportError::after_send(
            TransportErrorKind::Timeout,
            "timed out"
        ))),
        Submitted::Uncertain { .. }
    ));
    assert_eq!(
        submitted(Ok(envelope(400, "invalid_request"))),
        Submitted::Failed {
            reason: "GG video submission returned HTTP 400: type=invalid_request_error; code=invalid_request".into(),
            cost: Some(Usd::ZERO),
            retryable: false,
        }
    );
    assert!(matches!(
        submitted(Ok(envelope(429, "rate_limit_exceeded"))),
        Submitted::NotReceived { .. }
    ));
    assert!(matches!(
        submitted(Ok(envelope(429, "key_budget_exceeded"))),
        Submitted::Failed {
            cost: Some(Usd::ZERO),
            retryable: false,
            ..
        }
    ));
    assert!(matches!(
        submitted(Err(TransportError::not_sent(
            TransportErrorKind::Connect,
            "refused"
        ))),
        Submitted::NotReceived { .. }
    ));
}

#[test]
fn a_first_frame_goes_as_a_part_beside_the_input() {
    let png = media::png(2, 2, None);
    let mut builder = CallBuilder::new("video.generate", VIDEO_ROUTE);
    let frame = builder.file("image/png", &png);
    let call = builder
        .request(json!({"prompt": "It turns.", "duration": 2, "first_frame": frame}))
        .build();
    let transport = replay(vec![
        post("videos")
            .body(ExpectBody::Multipart(vec![
                Part::text(
                    "input",
                    r#"{"model":"alibaba/wan-3.0","prompt":"It turns.","duration":2}"#,
                ),
                Part::file("image", "first-frame.png", "image/png", png),
            ]))
            .reply(HttpResponse::json(
                202,
                &job("accepted", "pending", reserved()),
            )),
    ]);
    let adapter = video_adapter(&transport, &Arc::new(FakeClock::new()));
    assert_eq!(
        block_on(adapter.submit(&call)),
        Submitted::Accepted {
            handle: json!({"job_id": JOB})
        }
    );
    transport.assert_done();
}

#[test]
fn frames_gg_does_not_take_are_refused_before_anything_is_sent() {
    let adapter = GgVideo::new(
        gateway(Arc::new(NoNetwork), test_keys()),
        Arc::new(FakeClock::new()),
    );
    let refused = |member: &str, kind: &str, bytes: &[u8]| {
        let mut builder = CallBuilder::new("video.generate", VIDEO_ROUTE);
        let frame = builder.file(kind, bytes);
        let mut request = json!({"prompt": "p"});
        request[member] = frame;
        match block_on(adapter.submit(&builder.request(request).build())) {
            Submitted::Refused { reason } => reason,
            other => panic!("refused, not {other:?}"),
        }
    };
    let png = media::png(2, 2, None);
    assert_eq!(
        refused("last_frame", "image/png", &png),
        "GG video takes no last frame"
    );
    assert_eq!(
        refused("first_frame", "image/gif", b"GIF89a"),
        "first_frame is image/gif; GG takes PNG, JPEG or WebP"
    );
}

#[test]
fn a_finished_job_answers_with_its_settled_charge() {
    let transport = replay(vec![
        job_reply(job("running", "pending", reserved())),
        job_reply(job("succeeded", "available", settled("100"))),
        clip(),
    ]);
    let clock = Arc::new(FakeClock::new());
    let call = video_call(json!({"prompt": "p", "duration": 2}));
    let Collected::Answered(answer) = collect(&transport, &clock, &call) else {
        panic!("answered")
    };
    assert_eq!(answer.cost, Some(Usd(100_000)));
    assert_eq!(answer.files["video"].kind, "video/mp4");
    assert_eq!(
        answer.data,
        json!({"facts": {"width": 64, "height": 48, "duration_seconds": 2, "fps": 24}})
    );
    assert_eq!(clock.sleeps(), [video::POLL]);
    let adapter = video_adapter(&replay(Vec::new()), &clock);
    assert_eq!(adapter.check(&call, &answer), Ok(()));
    assert_eq!(
        adapter
            .check(&video_call(json!({"prompt": "p", "duration": 5})), &answer)
            .unwrap_err(),
        "the clip runs 2.000 s, not 5 s"
    );
    // The download carried no credential and the URL as GG gave it.
    let download = &transport.requests()[2];
    assert_eq!(download.url, VIDEO_URL);
    assert!(download.credential.is_none());
}

#[test]
fn billing_that_settles_after_the_result_is_waited_for() {
    let transport = replay(vec![
        job_reply(job("succeeded", "available", reserved())),
        clip(),
        job_reply(job("succeeded", "available", reserved())),
        job_reply(job("succeeded", "available", settled("400"))),
    ]);
    let clock = Arc::new(FakeClock::new());
    let Collected::Answered(answer) =
        collect(&transport, &clock, &video_call(json!({"prompt": "p"})))
    else {
        panic!("answered")
    };
    assert_eq!(answer.cost, Some(Usd(400_000)));
    // Billing that never settles: no cost, and the authorization as the bound.
    let mut exchanges = vec![job_reply(job("succeeded", "available", reserved())), clip()];
    let polls = (video::SETTLE_WAIT.as_secs() / video::POLL.as_secs()) as usize;
    exchanges.extend((0..polls).map(|_| job_reply(job("succeeded", "available", reserved()))));
    let transport = replay(exchanges);
    let Collected::Answered(answer) = collect(
        &transport,
        &Arc::new(FakeClock::new()),
        &video_call(json!({"prompt": "p"})),
    ) else {
        panic!("answered")
    };
    assert_eq!(answer.cost, None, "an authorization is not a charge");
}

#[test]
fn jobs_that_end_or_stop_collecting() {
    let clock = || Arc::new(FakeClock::new());
    let call = video_call(json!({"prompt": "p"}));
    let failed = json!({"code": "upstream_rejected", "message": "the provider refused it",
                        "type": "upstream_error", "param": null});
    let mut ended = job("failed", "unavailable", settled("0"));
    ended["error"] = failed;
    assert_eq!(
        collect(&replay(vec![job_reply(ended)]), &clock(), &call),
        Collected::Ended {
            reason: "GG video job failed without a result (unavailable): code=upstream_rejected"
                .into(),
            cost: Some(Usd::ZERO),
        }
    );
    assert!(matches!(
        collect(
            &replay(vec![job_reply(job("succeeded", "expired", reserved()))]),
            &clock(),
            &call
        ),
        Collected::Ended { cost: None, .. }
    ));
    assert!(matches!(
        collect(
            &replay(vec![job_get().reply(envelope(410, "job_expired"))]),
            &clock(),
            &call
        ),
        Collected::Ended { cost: None, .. }
    ));
    for status in [404, 401, 403] {
        assert!(
            matches!(
                collect(
                    &replay(vec![job_get().reply(envelope(status, "job_not_found"))]),
                    &clock(),
                    &call
                ),
                Collected::Unreachable { .. }
            ),
            "{status}"
        );
    }
    let mut other = job("running", "pending", reserved());
    other["id"] = json!("req_other");
    assert_eq!(
        collect(&replay(vec![job_reply(other)]), &clock(), &call),
        Collected::Unreachable {
            reason: "GG video job answered about another job".into()
        }
    );
    // Failed reads are polls that saw nothing.
    let transport = replay(vec![
        job_get().reply(envelope(503, "temporarily_unavailable")),
        job_get().fail(TransportError::after_send(
            TransportErrorKind::Other,
            "reset",
        )),
        job_get().reply(HttpResponse::new(200, b"<html>".to_vec())),
        job_reply(job("succeeded", "available", settled("100"))),
        clip(),
    ]);
    assert!(matches!(
        collect(&transport, &clock(), &call),
        Collected::Answered(_)
    ));
    // A handle this adapter did not write: nothing is requested.
    let adapter = video_adapter(&replay(Vec::new()), &clock());
    assert_eq!(
        block_on(adapter.collect(&call, &json!({"job_id": "../x"}))),
        Collected::Unreachable {
            reason: "a GG video job handle is not one this adapter wrote".into()
        }
    );
}

#[test]
fn a_job_still_running_at_the_deadline_is_collected_later() {
    let polls = (video::COLLECT_DEADLINE.as_secs() / video::POLL.as_secs()) as usize;
    let transport = replay(
        (0..polls)
            .map(|_| job_reply(job("running", "pending", reserved())))
            .collect(),
    );
    assert_eq!(
        collect(
            &transport,
            &Arc::new(FakeClock::new()),
            &video_call(json!({"prompt": "p"}))
        ),
        Collected::Unreachable {
            reason: OUTSTANDING.into()
        }
    );
}

#[test]
fn downloads_that_end_or_stop_collecting() {
    let call = video_call(json!({"prompt": "p"}));
    let with_download = |reply: Result<HttpResponse, TransportError>| {
        let download = match reply {
            Ok(response) => Expect::download(VIDEO_URL).reply(response),
            Err(error) => Expect::download(VIDEO_URL).fail(error),
        };
        let transport = replay(vec![
            job_reply(job("succeeded", "available", settled("100"))),
            download,
        ]);
        collect(&transport, &Arc::new(FakeClock::new()), &call)
    };
    for status in [403, 429, 500] {
        let collected = with_download(Ok(HttpResponse::new(status, Vec::new())));
        assert!(
            matches!(collected, Collected::Unreachable { .. }),
            "{status}"
        );
    }
    assert_eq!(
        with_download(Ok(HttpResponse::new(404, Vec::new()))),
        Collected::Ended {
            reason: "GG output video download returned HTTP 404".into(),
            cost: Some(Usd(100_000)),
        }
    );
    assert_eq!(
        with_download(Ok(HttpResponse::new(200, b"not a clip".to_vec()))),
        Collected::Ended {
            reason: "GG output video is not an MP4 file".into(),
            cost: Some(Usd(100_000)),
        }
    );
    assert!(matches!(
        with_download(Err(TransportError::after_send(
            TransportErrorKind::Other,
            "reset"
        ))),
        Collected::Unreachable { .. }
    ));
    // The signed URL never reaches a reason.
    let Collected::Unreachable { reason } = with_download(Err(TransportError::after_send(
        TransportErrorKind::Other,
        format!("reset while reading {VIDEO_URL}"),
    ))) else {
        panic!("unreachable")
    };
    assert!(!reason.contains("sig=abc"), "{reason}");
}

// ---------------------------------------------------------------------------------------------
// 3D

const MESH_ROUTE: &str = "tripo/p2@gg";
const RIG_ROUTE: &str = "tripo/rig-v1.0@gg";
const MODEL_URL: &str = "https://models.example.test/out/model.glb?sig=abc";

fn model_job(status: &str, result_status: &str, billing: Value) -> Value {
    let mut value = job(status, result_status, billing);
    value["result"] = if result_status == "available" {
        json!({"glb": {"url": MODEL_URL, "media_type": "model/gltf-binary", "expires_at": null}})
    } else {
        Value::Null
    };
    value
}

fn mesh_adapter(transport: &Arc<ReplayTransport>) -> GgMesh {
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::new());
    let setup = setup_with_clock(transport.clone(), test_keys(), Arc::clone(&clock));
    GgMesh::new(Gateway::with_session(&setup, SESSION), clock)
}

fn rig_adapter(transport: &Arc<ReplayTransport>) -> GgRig {
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::new());
    let setup = setup_with_clock(transport.clone(), test_keys(), Arc::clone(&clock));
    GgRig::new(Gateway::with_session(&setup, SESSION), clock)
}

fn mesh_contract() -> Value {
    json!({"adapter": "gg-mesh", "adapter_behavior": 1, "endpoint": "three-d.tripo.p2.multiview"})
}

fn rig_contract() -> Value {
    json!({"adapter": "gg-rig", "adapter_behavior": 1, "endpoint": "three-d.tripo.rig-v1.0.mesh"})
}

#[test]
fn a_mesh_sends_its_views_as_parts_and_answers_with_the_glb() {
    let front = media::png(2, 2, None);
    let back = media::JPEG_HEAD.to_vec();
    let mut builder = CallBuilder::new("mesh.generate", MESH_ROUTE).contract(mesh_contract());
    let front_file = builder.file("image/png", &front);
    let back_file = builder.file("image/jpeg", &back);
    let call = builder
        .request(
            json!({"views": {"back": back_file, "front": front_file}, "texture": true,
                        "pbr": false, "quad": false, "face_limit": 5000}),
        )
        .build();
    let transport = replay(vec![
        post("endpoints/three-d.tripo.p2.multiview")
            .body(ExpectBody::Multipart(vec![
                Part::text("input", r#"{"texture":true,"pbr":false,"face_limit":5000}"#),
                Part::file("images.front", "front.png", "image/png", front),
                Part::file("images.back", "back.jpg", "image/jpeg", back),
            ]))
            .reply(HttpResponse::json(
                202,
                &model_job("accepted", "pending", reserved()),
            )),
    ]);
    let adapter = mesh_adapter(&transport);
    assert_eq!(
        block_on(adapter.submit(&call)),
        Submitted::Accepted {
            handle: json!({"job_id": JOB})
        }
    );
    transport.assert_done();
    let transport = replay(vec![
        job_reply(model_job("running", "pending", reserved())),
        job_reply(model_job("succeeded", "available", settled("1500"))),
        Expect::download(MODEL_URL).reply(HttpResponse::new(200, media::GLB.to_vec())),
    ]);
    let collected = block_on(mesh_adapter(&transport).collect(&call, &json!({"job_id": JOB})));
    transport.assert_done();
    let Collected::Answered(answer) = collected else {
        panic!("answered, not {collected:?}")
    };
    assert_eq!(answer.files["model"].kind, "model/gltf-binary");
    assert_eq!(answer.files["model"].bytes, media::GLB);
    assert_eq!(
        answer.data,
        json!({"facts": {"model_kind": "model/gltf-binary"}})
    );
    assert_eq!(answer.cost, Some(Usd(1_500_000)));
    // A model that is not a GLB ends the job, at its charge.
    let transport = replay(vec![
        job_reply(model_job("succeeded", "available", settled("1500"))),
        Expect::download(MODEL_URL).reply(HttpResponse::new(200, media::FBX.to_vec())),
    ]);
    assert_eq!(
        block_on(mesh_adapter(&transport).collect(&call, &json!({"job_id": JOB}))),
        Collected::Ended {
            reason: "GG output model is not a GLB file".into(),
            cost: Some(Usd(1_500_000)),
        }
    );
}

#[test]
fn mesh_refusals_send_nothing() {
    let adapter = GgMesh::new(
        gateway(Arc::new(NoNetwork), test_keys()),
        Arc::new(FakeClock::new()),
    );
    let refused = |contract: Value, views: &[(&str, &str)]| {
        let mut builder = CallBuilder::new("mesh.generate", MESH_ROUTE).contract(contract);
        let mut named = serde_json::Map::new();
        for (name, kind) in views {
            let bytes = if *kind == "image/png" {
                media::png(2, 2, None)
            } else {
                media::GLB.to_vec()
            };
            named.insert(name.to_string(), builder.file(kind, &bytes));
        }
        let call = builder.request(json!({"views": named})).build();
        match block_on(adapter.submit(&call)) {
            Submitted::Refused { reason } => reason,
            other => panic!("refused, not {other:?}"),
        }
    };
    let two = [("front", "image/png"), ("left", "image/png")];
    assert_eq!(
        refused(json!({"adapter": "gg-mesh", "adapter_behavior": 1}), &two),
        "tripo/p2@gg is not a mesh.generate route this adapter serves: the contract names no GG endpoint"
    );
    assert_eq!(
        refused(
            json!({"adapter": "gg-mesh", "adapter_behavior": 1, "endpoint": "../jobs"}),
            &two
        ),
        "tripo/p2@gg is not a mesh.generate route this adapter serves: endpoint is one of GG's endpoint ids"
    );
    assert_eq!(
        refused(mesh_contract(), &[("front", "image/png")]),
        "a multiview job takes front and at least one of left, back, right"
    );
    assert_eq!(
        refused(
            mesh_contract(),
            &[("front", "image/png"), ("back", "model/gltf-binary")]
        ),
        "the back view is model/gltf-binary; GG takes PNG or JPEG"
    );
    assert_eq!(
        block_on(
            GgMesh::new(
                gateway(Arc::new(NoNetwork), Keys::from_pairs(&[])),
                Arc::new(FakeClock::new()),
            )
            .submit(
                &CallBuilder::new("mesh.generate", MESH_ROUTE)
                    .contract(mesh_contract())
                    .request(json!({"views": {"front": {"file": "a"}, "left": {"file": "b"}}}))
                    .build()
            )
        ),
        Submitted::Refused {
            reason: "GG_API_KEY is not set".into()
        }
    );
}

#[test]
fn a_rig_sends_the_model_and_answers_without_a_check() {
    let mut builder = CallBuilder::new("mesh.rig", RIG_ROUTE).contract(rig_contract());
    let model = builder.file("model/gltf-binary", media::GLB);
    let call = builder
        .request(json!({"model": model, "allow_negative_check": true}))
        .build();
    let transport = replay(vec![
        post("endpoints/three-d.tripo.rig-v1.0.mesh")
            .body(ExpectBody::Multipart(vec![
                Part::text("input", r#"{"rig_type":"biped","spec":"mixamo"}"#),
                Part::file(
                    "mesh",
                    "unrigged.glb",
                    "model/gltf-binary",
                    media::GLB.to_vec(),
                ),
            ]))
            .reply(HttpResponse::json(
                202,
                &model_job("accepted", "pending", reserved()),
            )),
    ]);
    assert_eq!(
        block_on(rig_adapter(&transport).submit(&call)),
        Submitted::Accepted {
            handle: json!({"job_id": JOB})
        }
    );
    transport.assert_done();
    let transport = replay(vec![
        job_reply(model_job("succeeded", "available", settled("250"))),
        Expect::download(MODEL_URL).reply(HttpResponse::new(200, media::GLB.to_vec())),
    ]);
    let Collected::Answered(answer) =
        block_on(rig_adapter(&transport).collect(&call, &json!({"job_id": JOB})))
    else {
        panic!("answered")
    };
    transport.assert_done();
    assert_eq!(
        answer.data,
        json!({"facts": {"riggable": null, "checked_rig_type": null, "advisory_override": false}})
    );
    assert_eq!(answer.cost, Some(Usd(250_000)));
    // A model that is not a GLB is refused before anything is sent.
    let mut builder = CallBuilder::new("mesh.rig", RIG_ROUTE).contract(rig_contract());
    let fbx = builder.file("model/fbx", media::FBX);
    let call = builder
        .request(json!({"model": fbx, "skeleton": "tripo"}))
        .build();
    assert_eq!(
        block_on(rig_adapter(&replay(vec![])).submit(&call)),
        Submitted::Refused {
            reason: "GG rigs a GLB, not model/fbx".into()
        }
    );
}

fn checked_rig_contract() -> Value {
    json!({"adapter": "gg-rig", "adapter_behavior": 1, "endpoint": "three-d.tripo.rig-v1.0.mesh",
           "check_endpoint": "three-d.tripo.rig-check"})
}

const CHECK: &str = "chk_1";

fn check_job(status: &str, result_status: &str, result: Value) -> Value {
    json!({"id": CHECK, "object": "job", "status": status, "result_status": result_status,
           "result": result, "error": null, "billing": settled("0"),
           "operation_id": "three-d.tripo.rig-check", "interface": "endpoints"})
}

fn check_post() -> Expect {
    Expect::new(
        Method::Post,
        format!("{BASE}/endpoints/three-d.tripo.rig-check"),
        Lane::Provider,
    )
    .credential("authorization", BEARER)
    .header("idempotency-key", &format!("{IDEMPOTENCY}-check"))
    .body(ExpectBody::Multipart(vec![
        Part::text("input", "{}"),
        Part::file(
            "mesh",
            "unrigged.glb",
            "model/gltf-binary",
            media::GLB.to_vec(),
        ),
    ]))
}

fn check_reply(value: Value) -> Exchange {
    Expect::get(format!("{BASE}/jobs/{CHECK}"))
        .credential("authorization", BEARER)
        .reply(HttpResponse::json(200, &value))
}

fn checked_rig_call(allow: bool) -> TestCall {
    let mut builder = CallBuilder::new("mesh.rig", RIG_ROUTE).contract(checked_rig_contract());
    let model = builder.file("model/gltf-binary", media::GLB);
    builder
        .request(json!({"model": model, "allow_negative_check": allow}))
        .build()
}

#[test]
fn a_rig_is_checked_for_free_first() {
    let transport = replay(vec![
        check_post().reply(HttpResponse::json(
            202,
            &check_job("accepted", "pending", Value::Null),
        )),
        check_reply(check_job("running", "pending", Value::Null)),
        check_reply(check_job(
            "succeeded",
            "available",
            json!({"riggable": true, "rig_type": "biped"}),
        )),
        post("endpoints/three-d.tripo.rig-v1.0.mesh").reply(HttpResponse::json(
            202,
            &model_job("accepted", "pending", reserved()),
        )),
    ]);
    let call = checked_rig_call(false);
    let handle = json!({"job_id": JOB, "check": {"job_id": CHECK, "riggable": true,
                        "rig_type": "biped"}, "advisory_override": false});
    assert_eq!(
        block_on(rig_adapter(&transport).submit(&call)),
        Submitted::Accepted {
            handle: handle.clone()
        }
    );
    transport.assert_done();
    // The answer's facts come from the handle.
    let transport = replay(vec![
        job_reply(model_job("succeeded", "available", settled("250"))),
        Expect::download(MODEL_URL).reply(HttpResponse::new(200, media::GLB.to_vec())),
    ]);
    let Collected::Answered(answer) = block_on(rig_adapter(&transport).collect(&call, &handle))
    else {
        panic!("answered")
    };
    assert_eq!(
        answer.data,
        json!({"facts": {"riggable": true, "checked_rig_type": "biped", "advisory_override": false}})
    );
}

#[test]
fn a_doubted_model_is_refused_unless_the_request_overrides_the_check() {
    let doubted = || {
        vec![
            check_post().reply(HttpResponse::json(
                202,
                &check_job("accepted", "pending", Value::Null),
            )),
            check_reply(check_job(
                "succeeded",
                "available",
                json!({"riggable": false, "rig_type": "quadruped"}),
            )),
        ]
    };
    let transport = replay(doubted());
    assert_eq!(
        block_on(rig_adapter(&transport).submit(&checked_rig_call(false))),
        Submitted::Refused {
            reason: "GG's rig check doubts this model (riggable=false, rig_type=quadruped)".into()
        }
    );
    transport.assert_done();
    let mut exchanges = doubted();
    exchanges.push(
        post("endpoints/three-d.tripo.rig-v1.0.mesh").reply(HttpResponse::json(
            202,
            &model_job("accepted", "pending", reserved()),
        )),
    );
    let transport = replay(exchanges);
    let Submitted::Accepted { handle } =
        block_on(rig_adapter(&transport).submit(&checked_rig_call(true)))
    else {
        panic!("accepted")
    };
    assert_eq!(handle["advisory_override"], json!(true));
    assert_eq!(handle["check"]["rig_type"], json!("quadruped"));
    // A check that ends without a result refuses; one GG refuses is refused; one that is not
    // answered is not received.
    let transport = replay(vec![
        check_post().reply(HttpResponse::json(
            202,
            &check_job("accepted", "pending", Value::Null),
        )),
        check_reply(check_job("failed", "unavailable", Value::Null)),
    ]);
    assert_eq!(
        block_on(rig_adapter(&transport).submit(&checked_rig_call(false))),
        Submitted::Refused {
            reason: "GG rig check job failed without a result (unavailable)".into()
        }
    );
    let transport = replay(vec![check_post().reply(envelope(400, "invalid_request"))]);
    assert!(matches!(
        block_on(rig_adapter(&transport).submit(&checked_rig_call(false))),
        Submitted::Refused { .. }
    ));
    let transport = replay(vec![
        check_post().reply(envelope(503, "temporarily_unavailable")),
    ]);
    assert!(matches!(
        block_on(rig_adapter(&transport).submit(&checked_rig_call(false))),
        Submitted::NotReceived { .. }
    ));
    let transport = replay(vec![
        check_post().reply(HttpResponse::json(
            202,
            &check_job("accepted", "pending", Value::Null),
        )),
        check_reply(check_job(
            "succeeded",
            "available",
            json!({"riggable": "yes"}),
        )),
    ]);
    assert_eq!(
        block_on(rig_adapter(&transport).submit(&checked_rig_call(false))),
        Submitted::NotReceived {
            reason: "GG's rig check answered without a verdict".into(),
            retry_after: None,
        }
    );
}

#[test]
fn a_failure_gg_accepted_costs_its_settled_charge() {
    // GG passed the request on and its provider refused it: GG settles what the provider charged.
    let rejected = envelope(502, "upstream_rejected")
        .with_header("x-request-id", "req_text-1")
        .with_header("x-gg-billing-url", "/v1/requests/req_text-1/billing");
    let clock = Arc::new(FakeClock::new());
    let transport = replay(vec![
        post("images/generations").reply(rejected.clone()),
        billing_get().reply(billing("settled", json!("0"))),
    ]);
    let setup = setup_with_clock(transport.clone(), test_keys(), clock.clone());
    let adapter = GgImages::new(Gateway::with_session(&setup, SESSION));
    let call = image_call("image.generate", json!({"prompt": "p"}));
    assert!(matches!(
        block_on(adapter.send(&call)),
        Sent::Failed {
            cost: Some(Usd::ZERO),
            retryable: false,
            ..
        }
    ));
    transport.assert_done();
    // A 2xx that is not an answer reads its charge the same way.
    let transport = replay(vec![
        post("chat/completions").reply(
            HttpResponse::json(200, &json!({"choices": []}))
                .with_header("x-gg-billing-status", "settled")
                .with_header("x-gg-charged-mills", "0.2"),
        ),
    ]);
    let adapter = GgStructured::new(gateway(transport.clone(), test_keys()));
    let call = CallBuilder::new("structured.generate", TEXT_ROUTE)
        .request(json!({"prompt": "p", "schema": {"type": "object"}}))
        .build();
    assert!(matches!(
        block_on(adapter.send(&call)),
        Sent::Failed {
            cost: Some(Usd(200)),
            retryable: true,
            ..
        }
    ));
    // A refusal before admission stays free, and reads nothing.
    let transport = replay(vec![
        post("images/generations").reply(
            envelope(400, "invalid_request")
                .with_header("x-gg-billing-url", "/v1/requests/req_text-1/billing"),
        ),
    ]);
    let adapter = GgImages::new(gateway(transport.clone(), test_keys()));
    assert!(matches!(
        block_on(adapter.send(&image_call("image.generate", json!({"prompt": "p"})))),
        Sent::Failed {
            cost: Some(Usd::ZERO),
            ..
        }
    ));
    transport.assert_done();
}
