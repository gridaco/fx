//! OpenRouter images on any model (src/openrouter/README.md, "Images on any model"), offline, on a
//! made-up model FX does not list: every exchange is scripted on a `ReplayTransport` that asserts
//! each request, and every refusal runs over `NoNetwork`, which panics on any send. Media are
//! built in code (`testing::media`).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{Adapter, Answer, RequestAdapter, RouteLimits, RouteRef, Sent};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::openrouter::generic_image::{self, OpenRouterGenericImages};
use grida_fx_providers::openrouter::{self, image};
use grida_fx_providers::registry::Adapters;
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, media, setup, test_keys};
use grida_fx_providers::transport::replay::{Expect, ExpectBody, NoNetwork, ReplayTransport};
use grida_fx_providers::transport::{Body, HttpResponse, Lane, Method, Transport};
use serde_json::{Value, json};
use std::sync::Arc;

const ROUTE: &str = "acme/img-a@openrouter";
const MODEL: &str = "acme/img-a";
const URL: &str = "https://openrouter.ai/api/v1/images";
const KEY: &str = "test-openrouter-key";

fn adapter_over(transport: Arc<dyn Transport>, keys: Keys) -> OpenRouterGenericImages {
    OpenRouterGenericImages::new(openrouter::client(&setup(transport, keys)))
}

/// A call as a route table completes a route that names no adapter.
fn call(capability: &str, request: Value) -> TestCall {
    CallBuilder::new(capability, ROUTE)
        .contract(json!({"adapter": "openrouter-image", "adapter_behavior": "1"}))
        .request(request)
        .build()
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn data_url(kind: &str, bytes: &[u8]) -> String {
    format!("data:{kind};base64,{}", b64(bytes))
}

fn expect(body: &Value) -> Expect {
    Expect::new(Method::Post, URL, Lane::Provider)
        .credential("authorization", &format!("Bearer {KEY}"))
        .body(ExpectBody::JsonText(serde_json::to_string(body).unwrap()))
}

fn expect_any() -> Expect {
    Expect::new(Method::Post, URL, Lane::Provider)
        .credential("authorization", &format!("Bearer {KEY}"))
        .body(ExpectBody::Any)
}

/// A 200 answer whose `data` is `items`.
fn answer_of(items: Value, usage: Option<Value>) -> HttpResponse {
    let mut body = json!({"created": 731, "data": items});
    if let Some(usage) = usage {
        body["usage"] = usage;
    }
    HttpResponse::json(200, &body).with_header("x-request-id", "gen-1")
}

fn one_image(bytes: &[u8]) -> HttpResponse {
    answer_of(
        json!([{"b64_json": b64(bytes)}]),
        Some(json!({"cost": 0.04})),
    )
}

fn send(transport: &Arc<ReplayTransport>, call: &TestCall) -> Sent {
    let sent = block_on(adapter_over(transport.clone(), test_keys()).send(call));
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

// The wire --------------------------------------------------------------------------------------

#[test]
fn a_generate_sends_the_model_and_the_prompt_and_answers_a_png() {
    let call = call(
        "image.generate",
        json!({"prompt": "a lantern on a pier", "size": "auto", "background": "opaque"}),
    );
    let body = json!({"model": MODEL, "prompt": "a lantern on a pier"});
    let jpeg = media::jpeg(8, 4);
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(&body).reply(one_image(&jpeg)),
    ]));
    let answer = answered(send(&transport, &call));
    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.cost, Some(Usd(40_000)));
    // The JPEG became a PNG of the same pixels, which every image route's check takes.
    let file = &answer.files["image"];
    assert_eq!(file.kind, "image/png");
    assert!(file.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    let decoded = ::image::load_from_memory(&file.bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (8, 4));
    let adapter = adapter_over(Arc::new(NoNetwork), test_keys());
    assert_eq!(adapter.check(&call, &answer), Ok(()));
    let request = &transport.requests()[0];
    assert_eq!(request.timeout, image::DEADLINE);
    assert_eq!(request.max_response_bytes, image::MAX_RESPONSE_BYTES);
}

#[test]
fn a_png_answer_is_kept_as_it_is() {
    let call = call("image.generate", json!({"prompt": "p"}));
    let png = media::png(3, 2, Some(0));
    let transport = Arc::new(ReplayTransport::new(vec![
        expect_any().reply(one_image(&png)),
    ]));
    let answer = answered(send(&transport, &call));
    assert_eq!(answer.files["image"].bytes, png);
}

#[test]
fn an_edit_sends_its_pictures_in_order_then_the_provider_options() {
    let first = media::png(4, 4, None);
    let second = media::jpeg(2, 2);
    let mut builder = CallBuilder::new("image.edit", ROUTE).contract(json!({
        "adapter": "openrouter-image",
        "adapter_behavior": "1",
        "provider_options": {"openrouter": {"seed": 1, "quality": "high"}},
    }));
    let image = builder.file("image/png", &first);
    let reference = builder.file("image/jpeg", &second);
    let call = builder
        .request(json!({
            "prompt": "the same lantern, at night",
            "image": image,
            "references": [reference],
            "provider_options": {"openrouter": {"seed": 7, "aspect_ratio": "16:9"},
                                 "fal": {"prompt": "never read"}},
        }))
        .build();
    let body = json!({
        "model": MODEL,
        "prompt": "the same lantern, at night",
        "input_references": [
            {"type": "image_url", "image_url": {"url": data_url("image/png", &first)}},
            {"type": "image_url", "image_url": {"url": data_url("image/jpeg", &second)}},
        ],
        "seed": 7,
        "quality": "high",
        "aspect_ratio": "16:9",
    });
    let png = media::png(2, 2, None);
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(&body).reply(one_image(&png)),
    ]));
    answered(send(&transport, &call));
}

#[test]
fn a_generate_sends_its_references_on_a_route_that_declares_image_input() {
    let reference = media::png(2, 2, None);
    let mut builder = CallBuilder::new("image.generate", ROUTE);
    let file = builder.file("image/png", &reference);
    let undeclared = builder
        .request(json!({"prompt": "p", "references": [file]}))
        .build();
    assert_eq!(
        refused(&undeclared),
        "acme/img-a@openrouter does not support image_input"
    );
    let mut builder = CallBuilder::new("image.generate", ROUTE).limits(RouteLimits {
        image_input: true,
        ..RouteLimits::default()
    });
    let file = builder.file("image/png", &reference);
    let call = builder
        .request(json!({"prompt": "p", "references": [file]}))
        .build();
    let body = json!({"model": MODEL, "prompt": "p", "input_references": [
        {"type": "image_url", "image_url": {"url": data_url("image/png", &reference)}},
    ]});
    let png = media::png(2, 2, None);
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(&body).reply(one_image(&png)),
    ]));
    answered(send(&transport, &call));
}

#[test]
fn provider_options_set_no_member_the_adapter_owns() {
    // Every member an edit's body has is one the adapter owns.
    let mut builder = CallBuilder::new("image.edit", ROUTE);
    let image = builder.file("image/png", &media::png(2, 2, None));
    let call = builder
        .request(json!({"prompt": "p", "image": image}))
        .build();
    let png = media::png(2, 2, None);
    let transport = Arc::new(ReplayTransport::new(vec![
        expect_any().reply(one_image(&png)),
    ]));
    answered(send(&transport, &call));
    let Body::Json(Value::Object(sent)) = &transport.requests()[0].body else {
        panic!("a JSON body");
    };
    for name in sent.keys() {
        assert!(generic_image::OWNED.contains(&name.as_str()), "{name}");
    }
    for name in generic_image::OWNED {
        let step = call_with_options(json!({}), json!({"openrouter": {*name: 1}}));
        assert_eq!(
            refused(&step),
            format!("provider_options.openrouter.{name} is a member FX owns")
        );
        let route = call_with_options(json!({"openrouter": {*name: 1}}), json!({}));
        assert_eq!(
            refused(&route),
            format!("contract.provider_options.openrouter.{name} is a member FX owns")
        );
    }
    // What GPT Image's adapter owns and this one does not set is the model's to take.
    for name in ["size", "quality", "background", "provider"] {
        assert!(!generic_image::OWNED.contains(&name), "{name}");
        assert!(image::OWNED.contains(&name), "{name}");
    }
}

fn call_with_options(route: Value, step: Value) -> TestCall {
    CallBuilder::new("image.generate", ROUTE)
        .contract(json!({"provider_options": route}))
        .request(json!({"prompt": " ", "provider_options": step}))
        .build()
}

// Refusals before sending ----------------------------------------------------------------------

#[test]
fn what_it_cannot_send_is_refused_and_named() {
    let hint = "give the model's own parameter in provider_options.openrouter";
    for (request, sentence) in [
        (
            json!({"prompt": " "}),
            "an image call needs its prompt as text".to_string(),
        ),
        (
            json!({"prompt": "p", "background": "transparent"}),
            format!("OpenRouter's generic image adapter sends no transparent background: {hint}"),
        ),
        (
            json!({"prompt": "p", "background": "clear"}),
            "background must be auto, opaque or transparent".to_string(),
        ),
        (
            json!({"prompt": "p", "size": "big"}),
            "size must be auto or WIDTHxHEIGHT".to_string(),
        ),
        (
            json!({"prompt": "p", "size": "1024x1024"}),
            format!("OpenRouter's generic image adapter sends no exact size: {hint}"),
        ),
    ] {
        assert_eq!(
            refused(&call("image.generate", request.clone())),
            sentence,
            "{request}"
        );
    }
    let mut builder = CallBuilder::new("image.edit", ROUTE);
    let image = builder.file("image/png", &media::png(2, 2, None));
    let mask = builder.file("image/png", &media::png(2, 2, Some(0)));
    let masked = builder
        .request(json!({"prompt": "p", "image": image, "mask": mask}))
        .build();
    assert_eq!(
        refused(&masked),
        format!("OpenRouter's generic image adapter sends no mask: {hint}")
    );
}

#[test]
fn the_route_s_picture_limit_is_held_and_files_are_pictures() {
    let mut builder = CallBuilder::new("image.edit", ROUTE).limits(RouteLimits {
        max_input_pictures: Some(2),
        ..RouteLimits::default()
    });
    let image = builder.file("image/png", &media::png(2, 2, None));
    let three = builder
        .request(json!({"prompt": "p", "image": image, "references": [image, image]}))
        .build();
    assert_eq!(
        refused(&three),
        "acme/img-a@openrouter takes at most 2 input pictures, not 3"
    );
    let mut builder = CallBuilder::new("image.edit", ROUTE);
    let text = builder.file("text/plain", b"a note");
    let unknown = json!({"file": "f".repeat(64)});
    let not_a_picture = builder
        .request(json!({"prompt": "p", "image": text}))
        .build();
    assert_eq!(
        refused(&not_a_picture),
        "image is text/plain, not a picture"
    );
    let no_bytes = CallBuilder::new("image.edit", ROUTE)
        .request(json!({"prompt": "p", "image": unknown}))
        .build();
    assert_eq!(refused(&no_bytes), "image has no bytes to send");
    assert_eq!(
        refused_with(
            &call("image.generate", json!({"prompt": " "})),
            Keys::none()
        ),
        "OPENROUTER_API_KEY is not set"
    );
}

#[test]
fn a_route_naming_another_adapter_is_not_this_one_s() {
    for contract in [
        json!({"adapter": "fx-openrouter-image-v1"}),
        json!({"adapter": "openrouter-image", "adapter_behavior": "3"}),
        json!({"adapter_behavior": 1}),
    ] {
        let call = CallBuilder::new("image.generate", ROUTE)
            .contract(contract.clone())
            .request(json!({"prompt": "p"}))
            .build();
        assert_eq!(
            refused(&call),
            "acme/img-a@openrouter is not a image.generate route this adapter serves",
            "{contract}"
        );
    }
}

// Answers ---------------------------------------------------------------------------------------

#[test]
fn an_answer_of_no_single_image_is_billed_and_not_drawn_again() {
    let png = media::png(2, 2, None);
    for items in [
        json!([]),
        json!([{"b64_json": b64(&png)}, {"b64_json": b64(&png)}]),
        json!(null),
    ] {
        let transport = Arc::new(ReplayTransport::new(vec![
            expect_any().reply(answer_of(items.clone(), Some(json!({"cost": 0.08})))),
        ]));
        let (reason, cost, retryable) = failed(send(
            &transport,
            &call("image.generate", json!({"prompt": "p"})),
        ));
        assert_eq!(
            reason, "OpenRouter image generation returned no single image (request gen-1)",
            "{items}"
        );
        assert_eq!((cost, retryable), (Some(Usd(80_000)), false), "{items}");
    }
}

#[test]
fn bytes_that_are_no_picture_fail_as_billed() {
    for (items, sentence) in [
        (
            json!([{"b64_json": "not base64!"}]),
            "OpenRouter image b64_json is not valid base64 (request gen-1)",
        ),
        (
            json!([{"b64_json": b64(b"not a picture at all")}]),
            "OpenRouter's image is not decodable (request gen-1)",
        ),
        (
            json!([{"url": "https://cdn.example.test/a.png"}]),
            "OpenRouter image b64_json is not valid base64 (request gen-1)",
        ),
    ] {
        let transport = Arc::new(ReplayTransport::new(vec![
            expect_any().reply(answer_of(items.clone(), Some(json!({"cost": 0.08})))),
        ]));
        let (reason, cost, retryable) = failed(send(
            &transport,
            &call("image.generate", json!({"prompt": "p"})),
        ));
        assert_eq!(reason, sentence, "{items}");
        assert_eq!((cost, retryable), (Some(Usd(80_000)), true), "{items}");
    }
}

#[test]
fn a_refusal_with_an_envelope_is_unbilled() {
    let transport = Arc::new(ReplayTransport::new(vec![expect_any().reply(
        HttpResponse::json(
            400,
            &json!({"error": {"code": 400, "message": "unknown model"}}),
        ),
    )]));
    let (reason, cost, retryable) = failed(send(
        &transport,
        &call("image.generate", json!({"prompt": "p"})),
    ));
    assert!(
        reason.starts_with("OpenRouter image generation returned HTTP 400"),
        "{reason}"
    );
    assert_eq!((cost, retryable), (Some(Usd::ZERO), false));
}

#[test]
fn the_check_is_every_image_route_s() {
    let adapter = adapter_over(Arc::new(NoNetwork), test_keys());
    let png = |alpha| {
        Answer::new(Value::Null, None).with_file("image", "image/png", media::png(2, 2, alpha))
    };
    let opaque = call(
        "image.generate",
        json!({"prompt": "p", "background": "opaque"}),
    );
    assert_eq!(adapter.check(&opaque, &png(None)), Ok(()));
    assert_eq!(
        adapter.check(&opaque, &png(Some(0))),
        Err("the picture asked for as opaque has transparent pixels".into())
    );
    let jpeg = Answer::new(Value::Null, None).with_file("image", "image/jpeg", media::jpeg(2, 2));
    assert_eq!(
        adapter.check(&call("image.generate", json!({"prompt": "p"})), &jpeg),
        Err("the answer is image/jpeg, not image/png".into())
    );
}

// The registry ----------------------------------------------------------------------------------

#[test]
fn it_is_the_default_and_gpt_image_s_adapter_serves_a_route_that_names_it() {
    let mut adapters = Adapters::new();
    openrouter::register(&mut adapters, &setup(Arc::new(NoNetwork), test_keys()));
    assert_eq!(adapters.served().len(), 5);
    assert_eq!(adapters.registered().len(), 7);
    for capability in ["image.generate", "image.edit"] {
        let route = |contract: Value| RouteRef {
            capability: capability.into(),
            model: MODEL.into(),
            provider: "openrouter".into(),
            contract,
            limits: RouteLimits::default(),
        };
        let identity = |contract| {
            adapters
                .serving(&route(contract))
                .and_then(Adapter::identity)
        };
        assert_eq!(identity(json!({})), Some(generic_image::IDENTITY));
        assert_eq!(
            identity(json!({"adapter": "fx-openrouter-image-v1"})),
            Some(image::IDENTITY)
        );
        assert_eq!(identity(json!({"adapter": "acme-image"})), None);
    }
}
