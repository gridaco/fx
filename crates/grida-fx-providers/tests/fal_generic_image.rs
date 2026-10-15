//! fal images on any model (src/fal/README.md, "Images on any model"), offline, on made-up
//! endpoints FX does not list: every exchange is scripted on a `ReplayTransport` that asserts each
//! request, and every refusal runs over `NoNetwork`. Media are built in code (`testing::media`).

use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{
    Adapter, Answer, CallRequest, RequestAdapter, RouteLimits, RouteRef, Sent,
};
use grida_fx_providers::fal::generic_image::{self, FalGenericImages};
use grida_fx_providers::fal::{FalClients, image, register};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::registry::Adapters;
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, media, setup, test_keys};
use grida_fx_providers::transport::replay::{Expect, ExpectBody, NoNetwork, ReplayTransport};
use grida_fx_providers::transport::{Body, HttpResponse, Lane, Transport};
use grida_fx_providers::wire::data_url;
use serde_json::{Value, json};
use std::sync::Arc;

const GENERATE: &str = "acme/img-a@fal";
const GENERATE_URL: &str = "https://fal.run/acme/img-a";
const EDIT: &str = "acme/img-a/edit@fal";
const EDIT_URL: &str = "https://fal.run/acme/img-a/edit";
const HOSTED: &str = "https://v3b.fal.media/files/output.jpeg";

fn adapter_over(transport: Arc<dyn Transport>, keys: Keys) -> FalGenericImages {
    FalGenericImages::new(FalClients::new(&setup(transport, keys)))
}

fn generate(request: Value) -> TestCall {
    CallBuilder::new("image.generate", GENERATE)
        .request(request)
        .build()
}

fn post(url: &str, body: &Value) -> Expect {
    Expect::post_json(url, Value::Null)
        .credential("authorization", "Key test-fal-key")
        .body(ExpectBody::JsonText(serde_json::to_string(body).unwrap()))
}

fn post_any(url: &str) -> Expect {
    Expect::post_json(url, Value::Null)
        .credential("authorization", "Key test-fal-key")
        .body(ExpectBody::Any)
}

fn send(transport: &Arc<ReplayTransport>, call: &CallRequest) -> Sent {
    let sent = block_on(adapter_over(transport.clone(), test_keys()).send(call));
    transport.assert_done();
    sent
}

fn refused(call: &CallRequest) -> String {
    refused_with(call, test_keys())
}

fn refused_with(call: &CallRequest, keys: Keys) -> String {
    match block_on(adapter_over(Arc::new(NoNetwork), keys).send(call)) {
        Sent::Refused { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn answered(sent: Sent) -> Answer {
    match sent {
        Sent::Answered(answer) => answer,
        other => panic!("expected an answer, got {other:?}"),
    }
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

fn images(items: Value) -> HttpResponse {
    HttpResponse::json(200, &json!({"images": items, "usage": {"cost": 0.03}}))
}

// The wire --------------------------------------------------------------------------------------

#[test]
fn a_generate_posts_the_prompt_to_the_endpoint_as_given_and_answers_a_png() {
    let call = generate(json!({"prompt": "One painted prop.", "size": "auto",
                               "background": "opaque"}));
    let jpeg = media::jpeg(8, 4);
    let transport = Arc::new(ReplayTransport::new(vec![
        post(GENERATE_URL, &json!({"prompt": "One painted prop."})).reply(images(
            json!([{"url": data_url("image/jpeg", &jpeg), "content_type": "image/jpeg",
                    "width": 8, "height": 4}]),
        )),
    ]));
    let answer = answered(send(&transport, &call));
    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.cost, Some(Usd(30_000)));
    let file = &answer.files["image"];
    assert_eq!(file.kind, "image/png");
    let decoded = ::image::load_from_memory(&file.bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (8, 4));
    let adapter = adapter_over(Arc::new(NoNetwork), test_keys());
    assert_eq!(adapter.check(&call, &answer), Ok(()));
    let request = &transport.requests()[0];
    assert_eq!(request.lane, Lane::Provider);
    assert_eq!(request.timeout, image::DEADLINE);
}

#[test]
fn an_edit_sends_its_pictures_then_the_provider_options_and_downloads_once() {
    let first = media::png(4, 4, None);
    let second = media::jpeg(2, 2);
    let mut builder = CallBuilder::new("image.edit", EDIT).contract(json!({
        "adapter": "fal-image",
        "adapter_behavior": "1",
        "provider_options": {"fal": {"seed": 1, "output_format": "png"}},
    }));
    let image = builder.file("image/png", &first);
    let reference = builder.file("image/jpeg", &second);
    let call = builder
        .request(json!({
            "prompt": "The same prop, at night.",
            "image": image,
            "references": [reference],
            "provider_options": {"fal": {"seed": 7, "enable_safety_checker": false},
                                 "openrouter": {"prompt": "never read"}},
        }))
        .build();
    let body = json!({
        "prompt": "The same prop, at night.",
        "image_urls": [data_url("image/png", &first), data_url("image/jpeg", &second)],
        "seed": 7,
        "output_format": "png",
        "enable_safety_checker": false,
    });
    let jpeg = media::jpeg(6, 6);
    let transport = Arc::new(ReplayTransport::new(vec![
        post(EDIT_URL, &body).reply(images(json!([{"url": HOSTED}]))),
        Expect::download(HOSTED)
            .header("accept", "image/*")
            .reply(HttpResponse::new(200, jpeg).with_header("content-type", "image/jpeg")),
    ]));
    let answer = answered(send(&transport, &call));
    assert_eq!(answer.files["image"].kind, "image/png");
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].lane, Lane::Download);
    assert!(requests[1].credential.is_none());
}

#[test]
fn a_png_answer_is_kept_as_it_is() {
    let png = media::png(3, 2, Some(0));
    let transport = Arc::new(ReplayTransport::new(vec![
        post_any(GENERATE_URL).reply(images(json!([{"url": data_url("image/png", &png)}]))),
    ]));
    let answer = answered(send(&transport, &generate(json!({"prompt": "p"}))));
    assert_eq!(answer.files["image"].bytes, png);
}

#[test]
fn provider_options_set_no_member_the_adapter_owns() {
    let mut builder = CallBuilder::new("image.edit", EDIT);
    let image = builder.file("image/png", &media::png(2, 2, None));
    let call = builder
        .request(json!({"prompt": "p", "image": image}))
        .build();
    let png = media::png(2, 2, None);
    let transport = Arc::new(ReplayTransport::new(vec![
        post_any(EDIT_URL).reply(images(json!([{"url": data_url("image/png", &png)}]))),
    ]));
    answered(send(&transport, &call));
    let Body::Json(Value::Object(sent)) = &transport.requests()[0].body else {
        panic!("a JSON body");
    };
    for name in sent.keys() {
        assert!(generic_image::OWNED.contains(&name.as_str()), "{name}");
    }
    for name in generic_image::OWNED {
        let step = generate(json!({"prompt": " ", "provider_options": {"fal": {*name: 1}}}));
        assert_eq!(
            refused(&step),
            format!("provider_options.fal.{name} is a member FX owns")
        );
        let route = CallBuilder::new("image.generate", GENERATE)
            .contract(json!({"provider_options": {"fal": {*name: 1}}}))
            .request(json!({"prompt": "p"}))
            .build();
        assert_eq!(
            refused(&route),
            format!("contract.provider_options.fal.{name} is a member FX owns")
        );
    }
    // What GPT Image's adapter sets and this one does not is the model's to take.
    for name in ["image_size", "quality", "background", "output_format"] {
        assert!(!generic_image::OWNED.contains(&name), "{name}");
        assert!(image::OWNED.contains(&name), "{name}");
    }
}

// Refusals before sending ----------------------------------------------------------------------

#[test]
fn what_it_cannot_send_is_refused_and_named() {
    let hint = "give the model's own parameter in provider_options.fal";
    let picture = json!({"file": "f".repeat(64)});
    for (request, sentence) in [
        (
            json!({"prompt": " "}),
            "an image call needs its prompt as text".to_string(),
        ),
        (
            json!({"prompt": "p", "background": "transparent"}),
            format!("fal's generic image adapter sends no transparent background: {hint}"),
        ),
        (
            json!({"prompt": "p", "size": "1536x1024"}),
            format!("fal's generic image adapter sends no exact size: {hint}"),
        ),
        (
            json!({"prompt": "p", "references": [picture]}),
            "fal image generation takes no references".to_string(),
        ),
    ] {
        assert_eq!(refused(&generate(request.clone())), sentence, "{request}");
    }
    let mut builder = CallBuilder::new("image.edit", EDIT);
    let image = builder.file("image/png", &media::png(2, 2, None));
    let masked = builder
        .request(json!({"prompt": "p", "image": image, "mask": image}))
        .build();
    assert_eq!(
        refused(&masked),
        format!("fal's generic image adapter sends no mask: {hint}")
    );
    let mut builder = CallBuilder::new("image.edit", EDIT).limits(RouteLimits {
        max_input_pictures: Some(1),
        ..RouteLimits::default()
    });
    let image = builder.file("image/png", &media::png(2, 2, None));
    let two = builder
        .request(json!({"prompt": "p", "image": image, "references": [image]}))
        .build();
    assert_eq!(
        refused(&two),
        "acme/img-a/edit@fal takes at most 1 input pictures, not 2"
    );
    let mut builder = CallBuilder::new("image.edit", EDIT);
    let text = builder.file("text/plain", b"a note");
    let not_a_picture = builder
        .request(json!({"prompt": "p", "image": text}))
        .build();
    assert_eq!(
        refused(&not_a_picture),
        "image is text/plain, not a picture"
    );
    assert_eq!(
        refused_with(&generate(json!({"prompt": " "})), Keys::none()),
        "FAL_KEY is not set"
    );
    let gpt = CallBuilder::new("image.generate", GENERATE)
        .contract(json!({"adapter": "fx-fal-image-v1"}))
        .request(json!({"prompt": "p"}))
        .build();
    assert_eq!(
        refused(&gpt),
        "acme/img-a@fal is not a image.generate route this adapter serves"
    );
}

// Answers ---------------------------------------------------------------------------------------

#[test]
fn an_answer_of_no_single_image_is_billed_and_not_drawn_again() {
    let png = media::png(2, 2, None);
    let one = json!({"url": data_url("image/png", &png)});
    for items in [json!([]), json!([one, one]), json!("x")] {
        let transport = Arc::new(ReplayTransport::new(vec![
            post_any(GENERATE_URL).reply(images(items.clone())),
        ]));
        let (reason, cost, retryable) = failed(send(&transport, &generate(json!({"prompt": "p"}))));
        assert_eq!(
            reason, "fal image generation returned no single image",
            "{items}"
        );
        assert_eq!((cost, retryable), (Some(Usd(30_000)), false), "{items}");
    }
}

#[test]
fn a_picture_that_cannot_be_read_fails_as_billed() {
    for (item, sentence) in [
        (
            json!({"url": data_url("image/gif", b"GIF89a")}),
            "fal image media type must be PNG, JPEG, or WebP",
        ),
        (
            json!({"url": "https://cdn.example.test/a.png"}),
            "fal hosted output must use HTTPS on fal.media without userinfo or a custom port",
        ),
        (
            json!({"url": data_url("image/jpeg", media::JPEG_HEAD)}),
            "fal's image is not decodable",
        ),
    ] {
        let transport = Arc::new(ReplayTransport::new(vec![
            post_any(GENERATE_URL).reply(images(json!([item.clone()]))),
        ]));
        let (reason, cost, retryable) = failed(send(&transport, &generate(json!({"prompt": "p"}))));
        assert_eq!(reason, sentence, "{item}");
        assert_eq!((cost, retryable), (Some(Usd(30_000)), true), "{item}");
    }
}

#[test]
fn a_refusal_on_the_run_host_is_unbilled() {
    let transport = Arc::new(ReplayTransport::new(vec![
        post_any(GENERATE_URL).reply(HttpResponse::new(422, b"{}".to_vec())),
    ]));
    let (reason, cost, retryable) = failed(send(&transport, &generate(json!({"prompt": "p"}))));
    assert_eq!(reason, "fal image generation returned HTTP 422");
    assert_eq!((cost, retryable), (Some(Usd::ZERO), false));
}

// The registry ----------------------------------------------------------------------------------

#[test]
fn it_is_the_default_and_gpt_image_s_adapter_serves_a_route_that_names_it() {
    let mut adapters = Adapters::new();
    register(&mut adapters, &setup(Arc::new(NoNetwork), test_keys()));
    for capability in ["image.generate", "image.edit"] {
        let route = |contract: Value| RouteRef {
            capability: capability.into(),
            model: "acme/img-a".into(),
            provider: "fal".into(),
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
            identity(json!({"adapter": "fx-fal-image-v1"})),
            Some(image::IDENTITY)
        );
        assert_eq!(identity(json!({"adapter": "fal-queue"})), None);
    }
}
