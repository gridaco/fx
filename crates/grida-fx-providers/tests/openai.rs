//! The OpenAI Images adapter over synthetic exchanges (spec/providers.md §4, §5 and §9.1;
//! spec/capabilities.md §2–§3). Nothing here reaches a network: every send goes to a
//! `ReplayTransport` that asserts the exact request, to `NoNetwork` (which panics on any send) or
//! to `Offline`. The key is the made-up `test-openai-key`; every picture is built in code (the
//! `testing::media` PNGs, and the JPEG and WebP encoders below).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{Answer, CallRequest, RequestAdapter, Sent};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::openai::{
    self, ADAPTER, DEADLINE, LABEL, MAX_INPUT_IMAGES, MAX_RESPONSE_BYTES, OpenAiImages,
};
use grida_fx_providers::registry::Adapters;
use grida_fx_providers::routes::default_table;
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, media, setup, test_keys};
use grida_fx_providers::transport::replay::{
    Exchange, Expect, ExpectBody, NoNetwork, ReplayTransport, Reply,
};
use grida_fx_providers::transport::{
    Body, HttpRequest, HttpResponse, Lane, Method, Offline, Part, Transport, TransportError,
    TransportErrorKind,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const KEY: &str = "test-openai-key";
const ROUTE: &str = "img-a@openai";
const GENERATIONS: &str = "https://api.openai.com/v1/images/generations";
const EDITS: &str = "https://api.openai.com/v1/images/edits";

// ---------------------------------------------------------------------------------------------
// Helpers

fn contract() -> Value {
    json!({"adapter": ADAPTER, "adapter_behavior": "1", "surface": "openai-images"})
}

fn generate(request: Value) -> TestCall {
    CallBuilder::new("image.generate", ROUTE)
        .contract(contract())
        .request(request)
        .build()
}

fn edit_builder() -> CallBuilder {
    CallBuilder::new("image.edit", ROUTE).contract(contract())
}

fn adapter(transport: Arc<dyn Transport>, keys: Keys) -> OpenAiImages {
    OpenAiImages::new(&setup(transport, keys))
}

fn send(transport: &Arc<ReplayTransport>, call: &CallRequest) -> Sent {
    block_on(adapter(transport.clone(), test_keys()).send(call))
}

/// The refusal of `call` with `keys`, over a transport that panics on any send.
fn refused_with(keys: Keys, call: &CallRequest) -> String {
    match block_on(adapter(Arc::new(NoNetwork), keys).send(call)) {
        Sent::Refused { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn refused(call: &CallRequest) -> String {
    refused_with(test_keys(), call)
}

/// What the adapter must send to `url`: the credential, the provider lane, a `POST`.
fn expect(url: &str, body: ExpectBody) -> Expect {
    Expect::new(Method::Post, url, Lane::Provider)
        .credential("authorization", &format!("Bearer {KEY}"))
        .body(body)
}

/// A 200 in the Images API's shape, with every member the adapter must ignore.
fn ok(image: &[u8]) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({
            "created": 731,
            "data": [{"b64_json": STANDARD.encode(image), "revised_prompt": "A revised kite."}],
            "usage": {"total_tokens": 42, "input_tokens_details": {"text_tokens": 7}},
        }),
    )
    .with_header("x-request-id", "openai-image-1")
}

/// One generation of `a kite` answered with `response`: the outcome, after asserting exactly
/// one request was made.
fn outcome(response: HttpResponse) -> Sent {
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(GENERATIONS, ExpectBody::Any).reply(response),
    ]));
    let sent = send(&transport, &generate(json!({"prompt": "a kite"})));
    transport.assert_done();
    assert_eq!(transport.requests().len(), 1);
    sent
}

fn failed(sent: Sent) -> (String, Option<Usd>, bool) {
    match sent {
        Sent::Failed {
            reason,
            cost,
            retryable,
        } => (reason, cost, retryable),
        other => panic!("expected a failure, got {other:?}"),
    }
}

fn not_received(sent: Sent) -> String {
    match sent {
        Sent::NotReceived { reason, .. } => reason,
        other => panic!("expected NotReceived, got {other:?}"),
    }
}

fn answered(sent: Sent) -> Answer {
    match sent {
        Sent::Answered(answer) => answer,
        other => panic!("expected an answer, got {other:?}"),
    }
}

/// The text fields of an edit, in order.
fn text_parts(prompt: &str, size: Option<&str>, background: &str) -> Vec<Part> {
    let mut parts = vec![
        Part::text("model", "img-a"),
        Part::text("prompt", prompt),
        Part::text("n", "1"),
        Part::text("output_format", "png"),
    ];
    if let Some(size) = size {
        parts.push(Part::text("size", size));
    }
    parts.push(Part::text("quality", "max"));
    parts.push(Part::text("background", background));
    parts.push(Part::text("moderation", "low"));
    parts
}

/// A decodable baseline JPEG, built in code.
fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let data = vec![120u8; (width * height * 3) as usize];
    image::codecs::jpeg::JpegEncoder::new(&mut bytes)
        .encode(&data, width, height, image::ExtendedColorType::Rgb8)
        .expect("a JPEG");
    bytes
}

/// A decodable lossless WebP with alpha, built in code.
fn webp(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let data = vec![200u8; (width * height * 4) as usize];
    image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
        .encode(&data, width, height, image::ExtendedColorType::Rgba8)
        .expect("a WebP");
    bytes
}

const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x00\x00\x00;";

// ---------------------------------------------------------------------------------------------
// Registration

#[test]
fn the_adapter_serves_both_image_capabilities_on_openai() {
    let mut adapters = Adapters::new();
    openai::register(&mut adapters, &setup(Arc::new(NoNetwork), test_keys()));
    assert!(adapters.serves("image.generate", "openai"));
    assert!(adapters.serves("image.edit", "openai"));
    assert_eq!(adapters.served().len(), 2);
}

#[test]
fn the_default_tables_openai_routes_pass_the_contract_check() {
    let table = default_table().unwrap();
    let routes: Vec<_> = table
        .entries
        .iter()
        .filter(|(_, route)| route.provider == "openai")
        .collect();
    assert_eq!(routes.len(), 2);
    for ((capability, id), route) in routes {
        let call = CallBuilder::new(capability, id)
            .contract(route.contract.clone())
            .request(json!({"prompt": "a kite"}))
            .build();
        // Past the contract (step 1); the request then fails its own steps, never step 1.
        let reason = refused_with(Keys::none(), &call);
        assert!(
            !reason.contains("route this adapter serves"),
            "{id} {capability}: {reason}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The wire

#[test]
fn generate_sends_the_exact_json_with_the_credential_only() {
    let image = media::png(4, 4, Some(128));
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(
            GENERATIONS,
            ExpectBody::JsonText(
                r#"{"model":"img-a","prompt":"One isolated hand-painted sprite.","n":1,"output_format":"png","size":"1536x1024","quality":"max","background":"transparent","moderation":"low"}"#
                    .into(),
            ),
        )
        .reply(ok(&image)),
    ]));
    let call = generate(json!({
        "background": "transparent",
        "size": "1536x1024",
        "prompt": "One isolated hand-painted sprite.",
        "references": null,
    }));
    let answer = answered(send(&transport, &call));
    transport.assert_done();

    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.cost, None);
    let names: Vec<&str> = answer.files.keys().map(String::as_str).collect();
    assert_eq!(names, ["image"]);
    assert_eq!(answer.files["image"].kind, "image/png");
    assert_eq!(answer.files["image"].bytes, image);

    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.timeout, DEADLINE);
    assert_eq!(request.max_response_bytes, MAX_RESPONSE_BYTES);
    assert_eq!(request.lane, Lane::Provider);
    assert!(request.headers.is_empty(), "{:?}", request.headers);
    let credential = request.credential.as_ref().unwrap();
    assert_eq!(credential.header, "authorization");
    assert_eq!(credential.header_value(), format!("Bearer {KEY}"));
}

#[test]
fn generate_sends_auto_background_and_size_only_when_given() {
    let image = media::png(2, 2, None);
    let body = |size: Option<&str>| {
        let mut body = serde_json::Map::new();
        body.insert("model".into(), json!("img-a"));
        body.insert("prompt".into(), json!("  a kite \n"));
        body.insert("n".into(), json!(1));
        body.insert("output_format".into(), json!("png"));
        if let Some(size) = size {
            body.insert("size".into(), json!(size));
        }
        body.insert("quality".into(), json!("max"));
        body.insert("background".into(), json!("auto"));
        body.insert("moderation".into(), json!("low"));
        ExpectBody::JsonText(serde_json::to_string(&body).unwrap())
    };
    let sizes = [None, Some("auto"), Some("1024x1024"), Some("3840x2160")];
    let transport = Arc::new(ReplayTransport::new(
        sizes
            .iter()
            .map(|size| expect(GENERATIONS, body(*size)).reply(ok(&image)))
            .collect(),
    ));
    for size in sizes {
        let request = match size {
            Some(size) => json!({"prompt": "  a kite \n", "size": size, "references": []}),
            None => json!({"prompt": "  a kite \n", "size": null, "background": null}),
        };
        answered(send(&transport, &generate(request)));
    }
    transport.assert_done();
}

#[test]
fn edit_sends_text_fields_then_each_input_image_in_order() {
    let first = media::png(2, 2, Some(255));
    let second = media::png(3, 3, None);
    let photo = jpeg(4, 4);
    let sticker = webp(4, 4);
    let mut builder = edit_builder();
    let image = builder.file("image/png", &first);
    let r1 = builder.file("image/png", &second);
    let r2 = builder.file("image/jpeg", &photo);
    let r3 = builder.file("image/webp", &sticker);
    let call = builder
        .request(json!({
            "prompt": "Blend them.",
            "image": image,
            "references": [r1, r2, r3],
            "size": "auto",
            "background": "transparent",
            "mask": null,
        }))
        .build();
    let mut parts = text_parts("Blend them.", Some("auto"), "transparent");
    parts.push(Part::file(
        "image[]",
        "reference-01.png",
        "image/png",
        first.clone(),
    ));
    parts.push(Part::file(
        "image[]",
        "reference-02.png",
        "image/png",
        second,
    ));
    parts.push(Part::file(
        "image[]",
        "reference-03.jpg",
        "image/jpeg",
        photo,
    ));
    parts.push(Part::file(
        "image[]",
        "reference-04.webp",
        "image/webp",
        sticker,
    ));
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(EDITS, ExpectBody::Multipart(parts)).reply(ok(&first)),
    ]));
    let answer = answered(send(&transport, &call));
    transport.assert_done();
    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.cost, None);

    let request = &transport.requests()[0];
    assert_eq!(request.timeout, DEADLINE);
    assert_eq!(request.max_response_bytes, MAX_RESPONSE_BYTES);
    assert_eq!(request.lane, Lane::Provider);
    let Body::Multipart(sent) = &request.body else {
        panic!("an edit is multipart");
    };
    for never in [
        "input_fidelity",
        "response_format",
        "seed",
        "stream",
        "mask",
    ] {
        assert!(sent.iter().all(|part| part.name != never), "{never}");
    }
    assert!(
        sent.iter()
            .all(|part| !String::from_utf8_lossy(&part.data).starts_with("data:")),
        "no data URL on the wire"
    );
}

#[test]
fn a_masked_edit_sends_the_mask_last_as_reference_00() {
    let picture = media::png(2, 2, Some(255));
    let mask = media::png_one_pixel_alpha(2, 2, 0);
    let mut builder = edit_builder();
    let image = builder.file("image/png", &picture);
    let mask_file = builder.file("image/png", &mask);
    let call = builder
        .request(
            json!({"prompt": "Mend the seam.", "image": image, "mask": mask_file,
                        "size": "1024x1024"}),
        )
        .build();
    let mut parts = text_parts("Mend the seam.", Some("1024x1024"), "auto");
    parts.push(Part::file(
        "image[]",
        "reference-01.png",
        "image/png",
        picture.clone(),
    ));
    parts.push(Part::file("mask", "reference-00.png", "image/png", mask));
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(EDITS, ExpectBody::Multipart(parts)).reply(ok(&picture)),
    ]));
    answered(send(&transport, &call));
    transport.assert_done();
}

#[test]
fn an_edit_takes_sixteen_input_images() {
    let picture = media::png(2, 2, Some(255));
    let mut builder = edit_builder();
    let image = builder.file("image/png", &picture);
    let references = vec![image.clone(); MAX_INPUT_IMAGES - 1];
    let call = builder
        .request(json!({"prompt": "p", "image": image, "references": references}))
        .build();
    let mut parts = text_parts("p", None, "auto");
    for n in 1..=MAX_INPUT_IMAGES {
        parts.push(Part::file(
            "image[]",
            &format!("reference-{n:02}.png"),
            "image/png",
            picture.clone(),
        ));
    }
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(EDITS, ExpectBody::Multipart(parts)).reply(ok(&picture)),
    ]));
    answered(send(&transport, &call));
    transport.assert_done();
}

// ---------------------------------------------------------------------------------------------
// Refusals: nothing is sent (`NoNetwork` panics on any send)

#[test]
fn the_route_must_be_one_this_adapter_serves() {
    for contract in [
        json!({"adapter": "gnode-openai-image-v1", "adapter_behavior": "1"}),
        json!({"adapter": "fx-openrouter-image-v1"}),
        json!({"adapter": ADAPTER, "adapter_behavior": "2"}),
        // routes-caps F3: the behaviour is the text "1" (spec/providers.md §9.1, §10); the number
        // 1 is another contract with another fingerprint, refused as OpenRouter refuses 3.
        json!({"adapter": ADAPTER, "adapter_behavior": 1}),
    ] {
        let call = CallBuilder::new("image.generate", ROUTE)
            .contract(contract.clone())
            .request(json!({"prompt": "a kite"}))
            .build();
        assert_eq!(
            refused(&call),
            "img-a@openai is not a image.generate route this adapter serves",
            "{contract}"
        );
    }
    let call = CallBuilder::new("background.remove", ROUTE)
        .contract(contract())
        .request(json!({"image": {"file": "0".repeat(64)}}))
        .build();
    assert_eq!(
        refused(&call),
        "img-a@openai is not a background.remove route this adapter serves"
    );
    // A route without a contract is served.
    let bare = CallBuilder::new("image.generate", ROUTE)
        .request(json!({"prompt": "a kite"}))
        .build();
    assert_eq!(
        refused_with(Keys::none(), &bare),
        "OPENAI_API_KEY is not set"
    );
}

#[test]
fn requests_must_fit_the_capability() {
    let cases = [
        (json!({}), "image.generate needs prompt"),
        (
            json!({"prompt": "p", "quality": "low"}),
            "image.generate takes no member quality",
        ),
        (
            json!({"prompt": "p", "resolution": "2k"}),
            "image.generate takes no member resolution",
        ),
        (
            json!({"prompt": "p", "image": {"file": "0".repeat(64)}}),
            "image.generate takes no member image",
        ),
        (json!({"prompt": "p", "size": 1024}), "size is text"),
        (json!({"prompt": 7}), "prompt is text"),
        (
            json!({"prompt": "p", "background": false}),
            "background is text",
        ),
        (
            json!(["a kite"]),
            "a request for image.generate is a JSON object",
        ),
    ];
    for (request, reason) in cases {
        assert_eq!(refused(&generate(request.clone())), reason, "{request}");
    }
    let call = edit_builder()
        .request(json!({"prompt": "p", "mask": {"file": "0".repeat(64)}}))
        .build();
    assert_eq!(refused(&call), "image.edit needs image");
}

#[test]
fn a_missing_key_is_refused_after_the_shape_and_before_the_values() {
    let call = generate(json!({"prompt": "a kite"}));
    assert_eq!(
        refused_with(Keys::none(), &call),
        "OPENAI_API_KEY is not set"
    );
    // The capability's shape comes first...
    let call = generate(json!({"prompt": "a kite", "quality": "low"}));
    assert_eq!(
        refused_with(Keys::none(), &call),
        "image.generate takes no member quality"
    );
    // ...and the values after the key.
    let call = generate(json!({"prompt": "  "}));
    assert_eq!(
        refused_with(Keys::none(), &call),
        "OPENAI_API_KEY is not set"
    );
}

#[test]
fn values_are_refused_before_sending() {
    let cases = [
        (
            json!({"prompt": ""}),
            "an image call needs its prompt as text",
        ),
        (
            json!({"prompt": " \n\t "}),
            "an image call needs its prompt as text",
        ),
        (
            json!({"prompt": "p", "background": "sky"}),
            "background must be auto, opaque or transparent",
        ),
        (
            json!({"prompt": "p", "background": "Transparent"}),
            "background must be auto, opaque or transparent",
        ),
        (
            json!({"prompt": "p", "size": "1024"}),
            "OpenAI image size must be auto or WIDTHxHEIGHT",
        ),
        (
            json!({"prompt": "p", "size": "1024X1024"}),
            "OpenAI image size must be auto or WIDTHxHEIGHT",
        ),
        (
            json!({"prompt": "p", "size": "0x1024"}),
            "OpenAI image size must be auto or WIDTHxHEIGHT",
        ),
        (
            json!({"prompt": "p", "size": "Auto"}),
            "OpenAI image size must be auto or WIDTHxHEIGHT",
        ),
    ];
    for (request, reason) in cases {
        assert_eq!(refused(&generate(request.clone())), reason, "{request}");
    }
}

#[test]
fn sizes_outside_the_envelope_are_refused() {
    for (size, reason) in [
        (
            "1000x1024",
            "OpenAI image size edges must be multiples of 16",
        ),
        (
            "256x1024",
            "OpenAI image size aspect ratio must not exceed 3:1",
        ),
        (
            "3856x1024",
            "OpenAI image size edges must not exceed 3840 pixels",
        ),
        (
            "3840x2176",
            "OpenAI image size must contain between 655360 and 8294400 pixels",
        ),
        (
            "256x768",
            "OpenAI image size must contain between 655360 and 8294400 pixels",
        ),
    ] {
        assert_eq!(
            refused(&generate(json!({"prompt": "p", "size": size}))),
            reason,
            "{size}"
        );
        let mut builder = edit_builder();
        let image = builder.file("image/png", &media::png(2, 2, None));
        let call = builder
            .request(json!({"prompt": "p", "image": image, "size": size}))
            .build();
        assert_eq!(refused(&call), reason, "edit {size}");
    }
}

#[test]
fn generation_takes_no_references_and_edits_at_most_sixteen_images() {
    let mut builder = CallBuilder::new("image.generate", ROUTE).contract(contract());
    let reference = builder.file("image/png", &media::png(2, 2, None));
    let call = builder
        .request(json!({"prompt": "p", "references": [reference]}))
        .build();
    assert_eq!(
        refused(&call),
        "OpenAI image generation takes no references"
    );

    let mut builder = edit_builder();
    let image = builder.file("image/png", &media::png(2, 2, None));
    let references = vec![image.clone(); MAX_INPUT_IMAGES];
    let call = builder
        .request(json!({"prompt": "p", "image": image, "references": references}))
        .build();
    assert_eq!(
        refused(&call),
        "OpenAI image edits support at most 16 input images"
    );
}

#[test]
fn files_are_refused_by_digest_kind_and_bytes() {
    let png = media::png(2, 2, Some(255));
    let unknown = json!({"file": "f".repeat(64)});

    let call = edit_builder()
        .request(json!({"prompt": "p", "image": unknown}))
        .build();
    assert_eq!(refused(&call), "image has no bytes to send");

    let mut builder = edit_builder();
    let image = builder.file("image/png", &png);
    let call = builder
        .request(json!({"prompt": "p", "image": image, "references": [image, unknown]}))
        .build();
    assert_eq!(refused(&call), "references[1] has no bytes to send");

    let mut builder = edit_builder();
    let image = builder.file("image/png", &png);
    let call = builder
        .request(json!({"prompt": "p", "image": image, "mask": unknown}))
        .build();
    assert_eq!(refused(&call), "mask has no bytes to send");

    let mut builder = edit_builder();
    let empty = builder.file("image/png", b"");
    let call = builder
        .request(json!({"prompt": "p", "image": empty}))
        .build();
    assert_eq!(refused(&call), "image has no bytes to send");

    let mut builder = edit_builder();
    let image = builder.file("image/png", &png);
    let gif = builder.file("image/gif", GIF);
    let call = builder
        .request(json!({"prompt": "p", "image": image, "references": [gif]}))
        .build();
    assert_eq!(
        refused(&call),
        "references[0] is image/gif; OpenAI takes PNG, JPEG or WebP"
    );

    let mut builder = edit_builder();
    let image = builder.file("image/png", &png);
    let gif = builder.file("image/gif", GIF);
    let call = builder
        .request(json!({"prompt": "p", "image": image, "mask": gif}))
        .build();
    assert_eq!(
        refused(&call),
        "mask is image/gif; OpenAI takes PNG, JPEG or WebP"
    );

    let mut builder = edit_builder();
    let text = builder.file("text/plain", b"a kite");
    let call = builder
        .request(json!({"prompt": "p", "image": text}))
        .build();
    assert_eq!(
        refused(&call),
        "image is text/plain; OpenAI takes PNG, JPEG or WebP"
    );

    for (kind, bytes, member) in [
        ("image/png", jpeg(4, 4), "image"),
        ("image/png", b"\x89PNG\r\n\x1a\ntruncated".to_vec(), "image"),
        ("image/jpeg", media::JPEG_HEAD.to_vec(), "image"),
        ("image/webp", png.clone(), "image"),
    ] {
        let mut builder = edit_builder();
        let image = builder.file(kind, &bytes);
        let call = builder
            .request(json!({"prompt": "p", "image": image}))
            .build();
        assert_eq!(
            refused(&call),
            format!("{member} is not a decodable {kind}"),
            "{kind}"
        );
    }
}

#[test]
fn the_first_refusal_wins() {
    // A value before a file.
    let mut builder = edit_builder();
    let gif = builder.file("image/gif", GIF);
    let call = builder
        .request(json!({"prompt": " ", "image": gif}))
        .build();
    assert_eq!(refused(&call), "an image call needs its prompt as text");
    // The size before the count.
    let mut builder = edit_builder();
    let image = builder.file("image/png", &media::png(2, 2, None));
    let references = vec![image.clone(); MAX_INPUT_IMAGES];
    let call = builder
        .request(
            json!({"prompt": "p", "image": image, "references": references,
                        "size": "1x1"}),
        )
        .build();
    assert_eq!(
        refused(&call),
        "OpenAI image size edges must be multiples of 16"
    );
    // The contract before everything.
    let call = CallBuilder::new("image.generate", ROUTE)
        .contract(json!({"adapter": "other"}))
        .request(json!({"quality": "low"}))
        .build();
    assert_eq!(
        refused_with(Keys::none(), &call),
        "img-a@openai is not a image.generate route this adapter serves"
    );
}

// ---------------------------------------------------------------------------------------------
// 2xx bodies that are not an answer: billed, retryable

#[test]
fn a_malformed_success_is_a_billed_retryable_failure() {
    let image = media::png(2, 2, None);
    let b64 = STANDARD.encode(&image);
    let single = format!("{LABEL} returned no single image");
    let base64 = "OpenAI image b64_json is not valid base64".to_string();
    let cases: Vec<(HttpResponse, String)> = vec![
        (
            HttpResponse::json(200, &json!({"data": []})),
            single.clone(),
        ),
        (
            HttpResponse::json(
                200,
                &json!({"data": [{"b64_json": b64}, {"b64_json": b64}]}),
            ),
            single.clone(),
        ),
        (HttpResponse::json(200, &json!({})), single.clone()),
        (
            HttpResponse::json(200, &json!({"data": {"b64_json": b64}})),
            single.clone(),
        ),
        (HttpResponse::json(200, &json!({"data": [b64]})), single),
        (
            HttpResponse::json(200, &json!({"data": [{}]})),
            base64.clone(),
        ),
        (
            HttpResponse::json(200, &json!({"data": [{"b64_json": "not-base64!"}]})),
            base64.clone(),
        ),
        (
            HttpResponse::json(200, &json!({"data": [{"b64_json": ""}]})),
            base64.clone(),
        ),
        (
            HttpResponse::json(200, &json!({"data": [{"b64_json": "aGVs bG8="}]})),
            base64.clone(),
        ),
        (
            HttpResponse::json(200, &json!({"data": [{"b64_json": "-_-_"}]})),
            base64.clone(),
        ),
        (
            HttpResponse::json(200, &json!({"data": [{"b64_json": 7}]})),
            base64.clone(),
        ),
        (
            HttpResponse::json(
                200,
                &json!({"data": [{"url": "https://files.example.test/out.png?sig=abc"}]}),
            ),
            base64,
        ),
        (
            HttpResponse::new(200, b"<html>busy</html>".to_vec()),
            format!("{LABEL} returned invalid JSON"),
        ),
        (
            HttpResponse::json(200, &json!([{"b64_json": b64}])),
            format!("{LABEL} returned a non-object JSON response"),
        ),
    ];
    for (response, reason) in cases {
        let shown = String::from_utf8_lossy(&response.body).to_string();
        assert_eq!(failed(outcome(response)), (reason, None, true), "{shown}");
    }
}

#[test]
fn a_structural_failure_names_the_request_id() {
    let response =
        HttpResponse::json(200, &json!({"data": []})).with_header("x-request-id", "req_9");
    assert_eq!(
        failed(outcome(response)),
        (
            format!("{LABEL} returned no single image (request req_9)"),
            None,
            true
        )
    );
}

#[test]
fn an_answer_carries_only_the_image() {
    // A JPEG answer is answered as what it is; the check refuses it (below).
    let photo = jpeg(4, 4);
    let answer = answered(outcome(ok(&photo)));
    assert_eq!(answer.files["image"].kind, "image/jpeg");
    assert_eq!(answer.data, Value::Null);
    // Bytes no sniffer knows are application/octet-stream.
    let answer = answered(outcome(ok(b"plain bytes")));
    assert_eq!(answer.files["image"].kind, "application/octet-stream");
    // A malformed revised_prompt, usage or created never fails the attempt and never shows.
    let image = media::png(2, 2, None);
    let response = HttpResponse::json(
        200,
        &json!({"created": true, "usage": "lots",
                "revised_prompt": "x".repeat(20_001),
                "data": [{"b64_json": STANDARD.encode(&image), "revised_prompt": null}]}),
    );
    let answer = answered(outcome(response));
    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.cost, None);
    assert_eq!(answer.files["image"].bytes, image);
}

// ---------------------------------------------------------------------------------------------
// Statuses (spec/providers.md §4.3 with §9.1's overrides)

fn envelope(status: u16, error: Value) -> HttpResponse {
    HttpResponse::json(status, &json!({ "error": error }))
}

#[test]
fn deterministic_refusals_are_failed_at_zero_and_end_the_call() {
    let cases = [
        (
            envelope(
                401,
                json!({"message": "Incorrect API key provided.", "type": "invalid_request_error",
                       "code": "invalid_api_key", "param": null}),
            ),
            "OpenAI image generation returned HTTP 401: type=invalid_request_error; code=invalid_api_key",
        ),
        (
            envelope(
                403,
                json!({"message": "Your organization must be verified.", "type": "invalid_request_error"}),
            ),
            "OpenAI image generation returned HTTP 403: type=invalid_request_error",
        ),
        (
            envelope(
                404,
                json!({"message": "The model does not exist.", "type": "invalid_request_error",
                       "code": "model_not_found"}),
            ),
            "OpenAI image generation returned HTTP 404: type=invalid_request_error; code=model_not_found",
        ),
        (
            HttpResponse::new(413, b"<html>too large</html>".to_vec()),
            "OpenAI image generation returned HTTP 413",
        ),
        (
            envelope(422, json!({"message": "Unprocessable."})),
            "OpenAI image generation returned HTTP 422",
        ),
        (
            envelope(
                400,
                json!({"message": "The model 'img-a' does not support the 'input_fidelity' parameter.",
                       "type": "invalid_request_error", "param": "input_fidelity",
                       "code": "invalid_input_fidelity_model"}),
            ),
            "OpenAI image generation returned HTTP 400: type=invalid_request_error; code=invalid_input_fidelity_model; param=input_fidelity",
        ),
        (
            envelope(
                400,
                json!({"message": "unsupported parameter contains test-openai-key",
                       "type": "invalid_request_error", "code": "invalid_value", "param": "size"}),
            ),
            "OpenAI image generation returned HTTP 400: message=unsupported parameter; type=invalid_request_error; code=invalid_value; param=size",
        ),
        (
            HttpResponse::new(405, Vec::new()),
            "OpenAI image generation returned HTTP 405",
        ),
        (
            HttpResponse::new(415, Vec::new()),
            "OpenAI image generation returned HTTP 415",
        ),
    ];
    for (response, reason) in cases {
        let status = response.status;
        assert_eq!(
            failed(outcome(response)),
            (reason.to_string(), Some(Usd::ZERO), false),
            "HTTP {status}"
        );
    }
}

#[test]
fn moderation_is_billed_and_ends_the_call() {
    for error in [
        json!({"message": "Your request was rejected by the safety system.",
               "type": "image_generation_user_error", "code": "moderation_blocked"}),
        json!({"type": "moderation_blocked"}),
        json!({"code": "content_policy_violation"}),
        json!({"type": "safety_error"}),
    ] {
        let (reason, cost, retryable) = failed(outcome(envelope(400, error.clone())));
        assert_eq!((cost, retryable), (None, false), "{error}");
        assert!(
            reason.starts_with("OpenAI image generation returned HTTP 400"),
            "{reason}"
        );
    }
    let (reason, _, _) = failed(outcome(envelope(
        400,
        json!({"type": "image_generation_user_error", "code": "moderation_blocked"}),
    )));
    assert_eq!(
        reason,
        "OpenAI image generation returned HTTP 400: type=image_generation_user_error; code=moderation_blocked"
    );
    // Moderation names only change a 400.
    assert_eq!(
        failed(outcome(envelope(
            403,
            json!({"code": "moderation_blocked"})
        )))
        .1,
        Some(Usd::ZERO)
    );
}

#[test]
fn rate_limits_are_not_received_and_quota_ends_the_call() {
    let sent = outcome(
        envelope(
            429,
            json!({"message": "Rate limit reached.", "type": "requests", "code": "rate_limit_exceeded"}),
        )
        .with_header("retry-after", "20"),
    );
    assert!(
        matches!(&sent, Sent::NotReceived { retry_after: Some(wait), .. } if *wait == Duration::from_secs(20)),
        "{sent:?}"
    );
    let reason = not_received(sent);
    assert_eq!(
        reason,
        "OpenAI image generation returned HTTP 429: type=requests; code=rate_limit_exceeded; retry-after 20"
    );
    // An HTTP date is honoured as a wait but not shown in the reason.
    let sent = outcome(
        HttpResponse::new(429, Vec::new())
            .with_header("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")
            .with_header("x-request-id", "req_7"),
    );
    assert!(
        matches!(
            &sent,
            Sent::NotReceived {
                retry_after: Some(_),
                ..
            }
        ),
        "{sent:?}"
    );
    let reason = not_received(sent);
    assert_eq!(
        reason,
        "OpenAI image generation returned HTTP 429 (request req_7)"
    );
    for error in [
        json!({"message": "You exceeded your current quota.", "type": "insufficient_quota",
               "code": "insufficient_quota"}),
        json!({"type": "invalid_request_error", "code": "insufficient_quota"}),
        json!({"type": "insufficient_quota", "code": null}),
    ] {
        let (_, cost, retryable) = failed(outcome(
            envelope(429, error.clone()).with_header("retry-after", "20"),
        ));
        assert_eq!((cost, retryable), (Some(Usd::ZERO), false), "{error}");
    }
}

/// classify C1: OpenAI's 408 is its own timeout, after the request arrived. The work may be done,
/// so it is a billed, retryable failure (spec/providers.md §9.1), never a free resend.
#[test]
fn a_408_is_billed_and_retryable() {
    for (response, reason) in [
        (
            HttpResponse::new(408, Vec::new()),
            "OpenAI image generation returned HTTP 408",
        ),
        (
            envelope(
                408,
                json!({"message": "Request timed out.", "type": "timeout"}),
            )
            .with_header("retry-after", "3"),
            "OpenAI image generation returned HTTP 408: type=timeout",
        ),
    ] {
        assert_eq!(failed(outcome(response)), (reason.to_string(), None, true));
    }
}

#[test]
fn server_errors_are_billed_and_retryable() {
    for status in [500, 502, 503, 504, 599] {
        let response = envelope(
            status,
            json!({"message": "The server had an error.", "type": "server_error"}),
        );
        assert_eq!(
            failed(outcome(response)),
            (
                format!("OpenAI image generation returned HTTP {status}: type=server_error"),
                None,
                true
            ),
            "HTTP {status}"
        );
    }
    let response = HttpResponse::new(500, Vec::new()).with_header("x-request-id", "req_5");
    assert_eq!(
        failed(outcome(response)).0,
        "OpenAI image generation returned HTTP 500 (request req_5)"
    );
    // A request id that is not a safe field is left out.
    let response = HttpResponse::new(500, Vec::new()).with_header("x-request-id", "req 5; x");
    assert_eq!(
        failed(outcome(response)).0,
        "OpenAI image generation returned HTTP 500"
    );
}

#[test]
fn a_redirect_is_not_followed() {
    let response = HttpResponse::new(302, Vec::new()).with_header(
        "location",
        "https://elsewhere.example.test/v1/images/generations",
    );
    assert_eq!(
        failed(outcome(response)),
        (
            "OpenAI image generation was redirected (HTTP 302); check OPENAI_BASE_URL".into(),
            None,
            false
        )
    );
}

// ---------------------------------------------------------------------------------------------
// Transport failures (spec/providers.md §4.2)

fn failing(error: TransportError) -> Sent {
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(GENERATIONS, ExpectBody::Any).fail(error),
    ]));
    let sent = send(&transport, &generate(json!({"prompt": "a kite"})));
    transport.assert_done();
    sent
}

#[test]
fn transport_failures_follow_their_phase() {
    assert_eq!(
        not_received(failing(TransportError::not_sent(
            TransportErrorKind::Connect,
            "connection refused"
        ))),
        "OpenAI image generation was not sent: connection refused"
    );
    assert_eq!(
        not_received(failing(TransportError::not_sent(
            TransportErrorKind::Timeout,
            "the connection timed out"
        ))),
        "OpenAI image generation was not sent: the connection timed out"
    );
    assert_eq!(
        failed(failing(TransportError::after_send(
            TransportErrorKind::Other,
            "connection reset"
        ))),
        (
            "OpenAI image generation failed after sending: connection reset".into(),
            None,
            true
        )
    );
    assert_eq!(
        failed(failing(TransportError::after_send(
            TransportErrorKind::Timeout,
            "the deadline passed"
        ))),
        (
            "OpenAI image generation failed after sending: the deadline passed".into(),
            None,
            true
        )
    );
    match failing(TransportError::not_sent(
        TransportErrorKind::Refused,
        "refused by the transport",
    )) {
        Sent::Refused { reason } => assert_eq!(
            reason,
            "OpenAI image generation was not sent: refused by the transport"
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_response_over_the_cap_is_billed_and_retryable() {
    let transport = Arc::new(ReplayTransport::new(vec![Exchange {
        expect: expect(GENERATIONS, ExpectBody::Any),
        reply: Reply::Zeros {
            status: 200,
            headers: Vec::new(),
            len: MAX_RESPONSE_BYTES + 1,
        },
    }]));
    let sent = send(&transport, &generate(json!({"prompt": "a kite"})));
    transport.assert_done();
    assert_eq!(
        failed(sent),
        (
            format!(
                "OpenAI image generation failed after sending: the response is larger than {MAX_RESPONSE_BYTES} bytes"
            ),
            None,
            true
        )
    );
}

#[test]
fn the_network_turned_off_refuses_at_zero() {
    let images = adapter(Arc::new(Offline), test_keys());
    match block_on(images.send(&generate(json!({"prompt": "a kite"})))) {
        Sent::Refused { reason } => assert_eq!(
            reason,
            "OpenAI image generation was not sent: the network is off (GRIDA_FX_NETWORK=off)"
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// The check (spec/capabilities.md §2)

fn check(request: Value, kind: &str, bytes: Vec<u8>) -> Result<(), String> {
    let images = adapter(Arc::new(NoNetwork), test_keys());
    let answer = Answer::new(Value::Null, None).with_file("image", kind, bytes);
    images.check(&generate(request), &answer)
}

#[test]
fn the_check_holds_the_answer_to_the_request() {
    assert_eq!(
        check(
            json!({"prompt": "p", "size": "1024x1024"}),
            "image/png",
            media::png(16, 16, None)
        ),
        Err("the image is 16x16, not 1024x1024".into())
    );
    assert_eq!(
        check(
            json!({"prompt": "p", "background": "transparent"}),
            "image/png",
            media::png(4, 4, None)
        ),
        Err("the picture asked for as transparent has no alpha channel".into())
    );
    assert_eq!(
        check(
            json!({"prompt": "p", "background": "opaque"}),
            "image/png",
            media::png_one_pixel_alpha(4, 4, 254)
        ),
        Err("the picture asked for as opaque has transparent pixels".into())
    );
    assert_eq!(
        check(
            json!({"prompt": "p", "background": "opaque"}),
            "image/png",
            media::png(4, 4, None)
        ),
        Ok(())
    );
    assert_eq!(
        check(
            json!({"prompt": "p", "background": "transparent"}),
            "image/png",
            media::png(4, 4, Some(0))
        ),
        Ok(())
    );
    for size in [json!("auto"), Value::Null] {
        assert_eq!(
            check(
                json!({"prompt": "p", "size": size}),
                "image/png",
                media::png(3, 5, Some(9))
            ),
            Ok(())
        );
    }
    assert_eq!(
        check(json!({"prompt": "p"}), "image/jpeg", jpeg(4, 4)),
        Err("the answer is image/jpeg, not image/png".into())
    );
    assert_eq!(
        check(
            json!({"prompt": "p"}),
            "application/octet-stream",
            b"plain".to_vec()
        ),
        Err("the answer is application/octet-stream, not image/png".into())
    );
    assert_eq!(
        check(
            json!({"prompt": "p"}),
            "image/png",
            b"\x89PNG\r\n\x1a\ntruncated".to_vec()
        ),
        Err("the image data is not decodable".into())
    );
    let images = adapter(Arc::new(NoNetwork), test_keys());
    assert_eq!(
        images.check(
            &generate(json!({"prompt": "p"})),
            &Answer::new(Value::Null, None)
        ),
        Err("the answer has no image".into())
    );
}

#[test]
fn a_sent_answer_is_checked_against_its_edit() {
    let picture = media::png(2, 2, Some(255));
    let mut builder = edit_builder();
    let image = builder.file("image/png", &picture);
    let call = builder
        .request(json!({"prompt": "p", "image": image, "size": "1024x1024",
                        "background": "opaque"}))
        .build();
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(EDITS, ExpectBody::Any).reply(ok(&picture)),
    ]));
    let images = adapter(transport.clone(), test_keys());
    let answer = answered(block_on(images.send(&call)));
    transport.assert_done();
    assert_eq!(
        images.check(&call, &answer),
        Err("the image is 2x2, not 1024x1024".into())
    );
}

// ---------------------------------------------------------------------------------------------
// Credential hygiene

fn assert_clean(text: &str) {
    assert!(!text.contains(KEY), "the key leaked into {text:?}");
}

fn assert_request_clean(request: &HttpRequest) {
    assert_eq!(
        request.credential.as_ref().map(|c| c.header_value()),
        Some(format!("Bearer {KEY}"))
    );
    assert_clean(&request.url);
    for (name, value) in &request.headers {
        assert_clean(name);
        assert_clean(value);
    }
    match &request.body {
        Body::Json(value) => assert_clean(&value.to_string()),
        Body::Multipart(parts) => {
            for part in parts {
                assert_clean(&part.name);
                assert_clean(&String::from_utf8_lossy(&part.data));
            }
        }
        Body::Bytes { bytes, .. } => assert_clean(&String::from_utf8_lossy(bytes)),
        Body::Empty => {}
    }
}

#[test]
fn the_key_travels_only_in_the_credential() {
    let picture = media::png(2, 2, Some(255));
    let mut builder = edit_builder();
    let image = builder.file("image/png", &picture);
    let mask = builder.file("image/png", &picture);
    let edit = builder
        .request(json!({"prompt": "p", "image": image, "mask": mask}))
        .build();
    let leaky = json!({"error": {
        "message": format!("unsupported parameter: Incorrect API key provided: {KEY}"),
        "type": KEY, "code": format!("bad {KEY}"), "param": "size"}});
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(GENERATIONS, ExpectBody::Any).reply(ok(&picture)),
        expect(EDITS, ExpectBody::Any).reply(ok(&picture)),
        expect(GENERATIONS, ExpectBody::Any).reply(HttpResponse::json(400, &leaky)),
        expect(GENERATIONS, ExpectBody::Any)
            .reply(HttpResponse::json(401, &leaky).with_header("x-request-id", KEY)),
        expect(GENERATIONS, ExpectBody::Any)
            .reply(HttpResponse::new(500, Vec::new()).with_header("x-request-id", KEY)),
        expect(GENERATIONS, ExpectBody::Any)
            .reply(HttpResponse::json(429, &leaky).with_header("retry-after", "3")),
        expect(GENERATIONS, ExpectBody::Any).fail(TransportError::after_send(
            TransportErrorKind::Other,
            format!("reset while sending {KEY}"),
        )),
        expect(GENERATIONS, ExpectBody::Any).fail(TransportError::not_sent(
            TransportErrorKind::Connect,
            format!("no route to {KEY}"),
        )),
    ]));
    let images = adapter(transport.clone(), test_keys());
    let generation = generate(json!({"prompt": "a kite"}));
    let mut outcomes = vec![block_on(images.send(&generation))];
    outcomes.push(block_on(images.send(&edit)));
    for _ in 0..6 {
        outcomes.push(block_on(images.send(&generation)));
    }
    transport.assert_done();

    for sent in &outcomes {
        match sent {
            Sent::Answered(answer) => {
                assert_eq!(answer.data, Value::Null);
                for file in answer.files.values() {
                    assert_clean(&file.kind);
                }
            }
            Sent::Failed { reason, .. }
            | Sent::NotReceived { reason, .. }
            | Sent::Refused { reason } => {
                assert_clean(reason);
                assert!(!reason.is_empty());
            }
        }
    }
    assert!(matches!(&outcomes[3], Sent::Failed { reason, .. } if reason.contains("[redacted]")));
    for request in transport.requests() {
        assert_request_clean(&request);
    }
    // A refusal never shows the key either, even one naming a member the author typed.
    let call = generate(json!({"prompt": "p", KEY: 1}));
    let reason = refused(&call);
    assert_clean(&reason);
    assert_eq!(reason, "image.generate takes no member [redacted]");
}
