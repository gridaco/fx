//! fal `image.generate` and `image.edit` (spec/providers.md §9.3) over synthetic exchanges. Every
//! test that sends asserts each request; every refusal runs over `NoNetwork`. Media are built in
//! code (`testing::media`).

use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{Answer, CallRequest, RequestAdapter, Sent};
use grida_fx_providers::fal::image::{self, FalImages};
use grida_fx_providers::fal::{FalClients, register};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::registry::Adapters;
use grida_fx_providers::testing::{CallBuilder, block_on, media, setup, test_keys};
use grida_fx_providers::transport::replay::{
    Exchange, Expect, ExpectBody, NoNetwork, ReplayTransport, Reply,
};
use grida_fx_providers::transport::{
    Body, HttpResponse, Lane, Transport, TransportError, TransportErrorKind,
};
use grida_fx_providers::wire::data_url;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const ROUTE: &str = "openai/gpt-image-2.5/sunburst@fal";
const GENERATE_URL: &str = "https://fal.run/openai/gpt-image-2.5/sunburst/text-to-image";
const EDIT_URL: &str = "https://fal.run/openai/gpt-image-2.5/sunburst/edit";
const HOSTED: &str = "https://v3b.fal.media/files/output.png";
const KEY: &str = "test-fal-key";

fn adapter(transport: Arc<dyn Transport>) -> FalImages {
    FalImages::new(FalClients::new(&setup(transport, test_keys())))
}

fn contract() -> Value {
    json!({"route_id": "image.sunburst.fal.text-to-image", "adapter": "fx-fal-image-v1",
           "adapter_behavior": "1", "surface": "fal-run"})
}

fn generate(request: Value) -> grida_fx_providers::testing::TestCall {
    CallBuilder::new("image.generate", ROUTE)
        .contract(contract())
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
    let sent = block_on(adapter(transport.clone()).send(call));
    transport.assert_done();
    sent
}

fn refused(call: &CallRequest) -> String {
    refused_with(call, test_keys())
}

fn refused_with(call: &CallRequest, keys: Keys) -> String {
    let adapter = FalImages::new(FalClients::new(&setup(Arc::new(NoNetwork), keys)));
    match block_on(adapter.send(call)) {
        Sent::Refused { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn data_png(width: u32, height: u32) -> (Vec<u8>, String) {
    let png = media::png(width, height, Some(255));
    let url = data_url("image/png", &png);
    (png, url)
}

fn answered(sent: Sent) -> Answer {
    match sent {
        Sent::Answered(answer) => answer,
        other => panic!("expected an answer, got {other:?}"),
    }
}

fn failed(sent: Sent) -> (String, Option<Usd>) {
    match sent {
        Sent::Failed {
            reason,
            cost,
            retryable: true,
        } => (reason, cost),
        other => panic!("expected a retryable failure, got {other:?}"),
    }
}

// 1. The wire -----------------------------------------------------------------------------------

#[test]
fn generate_sends_the_exact_body_to_text_to_image() {
    let call = generate(
        json!({"prompt": "One isolated painted prop.", "size": "1536x1024",
                               "background": "transparent"}),
    );
    let (png, url) = data_png(2, 2);
    let body = json!({"prompt": "One isolated painted prop.", "num_images": 1,
                      "image_size": {"width": 1536, "height": 1024}, "quality": "max",
                      "background": "transparent", "output_format": "png"});
    let transport =
        Arc::new(ReplayTransport::new(vec![post(GENERATE_URL, &body).reply(
            HttpResponse::json(200, &json!({"images": [{"url": url}]})),
        )]));
    let answer = answered(send(&transport, &call));
    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.cost, None);
    assert_eq!(answer.files["image"].kind, "image/png");
    assert_eq!(answer.files["image"].bytes, png);
    let request = &transport.requests()[0];
    assert_eq!(request.lane, Lane::Provider);
    assert_eq!(request.timeout, image::DEADLINE);
    assert!(request.headers.is_empty(), "{:?}", request.headers);
}

#[test]
fn edit_sends_image_then_references_then_the_mask() {
    let mut builder = CallBuilder::new("image.edit", ROUTE).contract(contract());
    let first = media::png(2, 2, Some(255));
    let second = media::JPEG_HEAD.to_vec();
    let third = b"RIFF\x00\x00\x00\x00WEBPVP8 ".to_vec();
    let mask = media::png(2, 2, Some(0));
    let image_file = builder.file("image/png", &first);
    let jpeg_file = builder.file("image/jpeg", &second);
    let other_file = builder.file("image/webp", &third);
    let mask_file = builder.file("image/png", &mask);
    let call = builder
        .request(
            json!({"prompt": "Paint the sky.", "size": "auto", "background": "opaque",
                        "image": image_file, "references": [jpeg_file, other_file],
                        "mask": mask_file}),
        )
        .build();
    let body = json!({"prompt": "Paint the sky.", "num_images": 1, "image_size": "auto",
                      "quality": "max", "background": "opaque", "output_format": "png",
                      "image_urls": [data_url("image/png", &first), data_url("image/jpeg", &second),
                                     data_url("image/webp", &third)],
                      "mask_url": data_url("image/png", &mask)});
    let (_, url) = data_png(2, 2);
    let transport =
        Arc::new(ReplayTransport::new(vec![post(EDIT_URL, &body).reply(
            HttpResponse::json(200, &json!({"images": [{"url": url}]})),
        )]));
    answered(send(&transport, &call));
}

#[test]
fn an_edit_without_references_or_mask_sends_one_picture() {
    let mut builder = CallBuilder::new("image.edit", ROUTE);
    let first = media::png(2, 2, Some(255));
    let image_file = builder.file("image/png", &first);
    let call = builder
        .request(
            json!({"prompt": "p", "image": image_file, "references": null, "mask": null,
                        "size": null, "background": null}),
        )
        .build();
    let body = json!({"prompt": "p", "num_images": 1, "quality": "max", "background": "auto",
                      "output_format": "png", "image_urls": [data_url("image/png", &first)]});
    let (_, url) = data_png(2, 2);
    let transport =
        Arc::new(ReplayTransport::new(vec![post(EDIT_URL, &body).reply(
            HttpResponse::json(200, &json!({"images": [{"url": url}]})),
        )]));
    answered(send(&transport, &call));
}

#[test]
fn an_absent_or_null_size_sends_no_image_size() {
    let (_, url) = data_png(2, 2);
    for request in [
        json!({"prompt": "p"}),
        json!({"prompt": "p", "size": null, "background": null, "references": []}),
    ] {
        let body = json!({"prompt": "p", "num_images": 1, "quality": "max",
                          "background": "auto", "output_format": "png"});
        let transport =
            Arc::new(ReplayTransport::new(vec![post(GENERATE_URL, &body).reply(
                HttpResponse::json(200, &json!({"images": [{"url": url}]})),
            )]));
        answered(send(&transport, &generate(request)));
    }
}

// 2. Refusals ----------------------------------------------------------------------------------

#[test]
fn refusals_send_nothing() {
    let unknown = json!({"file": "f".repeat(64)});
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "image.generate",
            json!({"prompt": "p", "quality": "low"}),
            "image.generate takes no member quality",
        ),
        (
            "image.generate",
            json!({"prompt": "p", "mask": unknown}),
            "image.generate takes no member mask",
        ),
        (
            "image.generate",
            json!({"size": "auto"}),
            "image.generate needs prompt",
        ),
        ("image.generate", json!({"prompt": 3}), "prompt is text"),
        (
            "image.edit",
            json!({"prompt": "p"}),
            "image.edit needs image",
        ),
        (
            "image.generate",
            json!({"prompt": " \n\t"}),
            "an image call needs its prompt as text",
        ),
        (
            "image.generate",
            json!({"prompt": "é".repeat(32_001)}),
            "the prompt is longer than 32000 characters",
        ),
        (
            "image.generate",
            json!({"prompt": "p", "background": "clear"}),
            "background must be auto, opaque or transparent",
        ),
        (
            "image.generate",
            json!({"prompt": "p", "references": [unknown]}),
            "fal image generation takes no references",
        ),
        (
            "image.generate",
            json!({"prompt": "p", "size": "big"}),
            "fal image size must be auto or WIDTHxHEIGHT",
        ),
        (
            "image.generate",
            json!({"prompt": "p", "size": "1024"}),
            "fal image size must be auto or WIDTHxHEIGHT",
        ),
        (
            "image.generate",
            json!({"prompt": "p", "size": "1000x1024"}),
            "fal image size edges must be multiples of 16",
        ),
        (
            "image.generate",
            json!({"prompt": "p", "size": "3856x1024"}),
            "fal image size edges must not exceed 3840 pixels",
        ),
        (
            "image.generate",
            json!({"prompt": "p", "size": "2400x768"}),
            "fal image size aspect ratio must not exceed 3:1",
        ),
        (
            "image.generate",
            json!({"prompt": "p", "size": "512x512"}),
            "fal image size must contain between 655360 and 8294400 pixels",
        ),
        (
            "image.generate",
            json!({"prompt": "p", "size": "3840x2176"}),
            "fal image size must contain between 655360 and 8294400 pixels",
        ),
        (
            "image.edit",
            json!({"prompt": "p", "image": unknown}),
            "image has no bytes to send",
        ),
    ];
    for (capability, request, sentence) in cases {
        let call = CallBuilder::new(capability, ROUTE).request(request).build();
        assert_eq!(refused(&call), sentence, "{capability} {}", call.request);
    }
}

#[test]
fn files_that_are_not_pictures_are_refused_before_sending() {
    // Each picture member in the order image, references[<i>], mask; the first non-picture wins.
    // Distinct bytes per file: the call's files are keyed by digest.
    let edit = |image: &str, references: &[&str], mask: Option<&str>| {
        let mut builder = CallBuilder::new("image.edit", ROUTE).contract(contract());
        let mut next = 0u32;
        let mut file = |kind: &str| {
            next += 1;
            builder.file(kind, &media::png(next, 1, Some(255)))
        };
        let image = file(image);
        let references: Vec<Value> = references.iter().map(|kind| file(kind)).collect();
        let mut request = json!({"prompt": "p", "image": image, "references": references});
        if let Some(kind) = mask {
            request["mask"] = file(kind);
        }
        builder.request(request).build()
    };
    let cases = [
        (
            edit("model/gltf-binary", &[], None),
            "image is model/gltf-binary, not a picture",
        ),
        (
            edit(
                "image/png",
                &["image/jpeg", "text/plain"],
                Some("image/png"),
            ),
            "references[1] is text/plain, not a picture",
        ),
        (
            edit("image/png", &[], Some("application/octet-stream")),
            "mask is application/octet-stream, not a picture",
        ),
        (
            edit("audio/mpeg", &["text/plain"], Some("file")),
            "image is audio/mpeg, not a picture",
        ),
    ];
    for (call, sentence) in cases {
        assert_eq!(refused(&call), sentence);
    }
}

#[test]
fn a_full_length_prompt_is_sent() {
    let prompt = "é".repeat(image::MAX_PROMPT_CHARS);
    let (_, url) = data_png(2, 2);
    let transport =
        Arc::new(ReplayTransport::new(vec![post_any(GENERATE_URL).reply(
            HttpResponse::json(200, &json!({"images": [{"url": url}]})),
        )]));
    answered(send(&transport, &generate(json!({"prompt": prompt}))));
}

#[test]
fn edits_take_at_most_sixteen_pictures() {
    let mut builder = CallBuilder::new("image.edit", ROUTE);
    let image_file = builder.file("image/png", &media::png(2, 2, None));
    let references: Vec<Value> = (0..16u8)
        .map(|i| builder.file("image/png", &[0x89, b'P', b'N', b'G', i]))
        .collect();
    let call = builder
        .request(json!({"prompt": "p", "image": image_file, "references": references}))
        .build();
    assert_eq!(
        refused(&call),
        "fal image edits support at most 16 input references"
    );

    let mut builder = CallBuilder::new("image.edit", ROUTE);
    let image_file = builder.file("image/png", &media::png(2, 2, None));
    let references: Vec<Value> = (0..15u8)
        .map(|i| builder.file("image/png", &[0x89, b'P', b'N', b'G', i]))
        .collect();
    let mask = builder.file("image/png", &media::png(2, 2, Some(0)));
    let call = builder
        .request(
            json!({"prompt": "p", "image": image_file, "references": references,
                        "mask": mask}),
        )
        .build();
    let (_, url) = data_png(2, 2);
    let transport =
        Arc::new(ReplayTransport::new(vec![post_any(EDIT_URL).reply(
            HttpResponse::json(200, &json!({"images": [{"url": url}]})),
        )]));
    answered(send(&transport, &call));
    let Body::Json(body) = &transport.requests()[0].body else {
        panic!("a JSON body")
    };
    assert_eq!(body["image_urls"].as_array().unwrap().len(), 16);
}

#[test]
fn unreadable_files_are_named() {
    let unknown = json!({"file": "f".repeat(64)});
    let mut builder = CallBuilder::new("image.edit", ROUTE);
    let image_file = builder.file("image/png", &media::png(2, 2, None));
    let reference = builder.file("image/png", &media::png(2, 2, None));
    let call = builder
        .request(json!({"prompt": "p", "image": image_file, "references": [reference, unknown]}))
        .build();
    assert_eq!(refused(&call), "references[1] has no bytes to send");

    let mut builder = CallBuilder::new("image.edit", ROUTE);
    let image_file = builder.file("image/png", &media::png(2, 2, None));
    let call = builder
        .request(json!({"prompt": "p", "image": image_file, "mask": unknown}))
        .build();
    assert_eq!(refused(&call), "mask has no bytes to send");

    // A store copy that cannot be read is the same refusal.
    let mut builder = CallBuilder::new("image.edit", ROUTE);
    let image_file = builder.file("image/png", &media::png(2, 2, None));
    let mut call = builder
        .request(json!({"prompt": "p", "image": image_file}))
        .build();
    let digest = call.call.request["image"]["file"]
        .as_str()
        .unwrap()
        .to_string();
    call.call.files.get_mut(&digest).unwrap().path = "/nonexistent/fx-test/missing".into();
    assert_eq!(refused(&call), "image has no bytes to send");
}

#[test]
fn the_first_refusal_wins() {
    // Contract before shape, shape before key, key before values, values before files.
    let foreign = CallBuilder::new("image.generate", ROUTE)
        .contract(json!({"adapter": "gnode-fal-image-v1"}))
        .request(json!({"prompt": "", "quality": "low"}))
        .build();
    assert_eq!(
        refused_with(&foreign, Keys::none()),
        "openai/gpt-image-2.5/sunburst@fal is not a image.generate route this adapter serves"
    );
    let shape = generate(json!({"prompt": "", "quality": "low"}));
    assert_eq!(
        refused_with(&shape, Keys::none()),
        "image.generate takes no member quality"
    );
    let key = generate(json!({"prompt": "", "size": "big"}));
    assert_eq!(refused_with(&key, Keys::none()), "FAL_KEY is not set");
    let prompt = generate(json!({"prompt": "", "background": "clear"}));
    assert_eq!(refused(&prompt), "an image call needs its prompt as text");
    let background = generate(json!({"prompt": "p", "background": "clear", "size": "big"}));
    assert_eq!(
        refused(&background),
        "background must be auto, opaque or transparent"
    );
    let size = CallBuilder::new("image.edit", ROUTE)
        .request(json!({"prompt": "p", "size": "big", "image": {"file": "f".repeat(64)}}))
        .build();
    assert_eq!(
        refused(&size),
        "fal image size must be auto or WIDTHxHEIGHT"
    );
    let other_capability = CallBuilder::new("video.generate", ROUTE)
        .request(json!({"prompt": "p"}))
        .build();
    assert_eq!(
        refused(&other_capability),
        "openai/gpt-image-2.5/sunburst@fal is not a video.generate route this adapter serves"
    );
}

// 3. Answers -----------------------------------------------------------------------------------

#[test]
fn data_uri_answers_at_the_root_or_under_data() {
    let (png, url) = data_png(3, 2);
    for payload in [
        json!({"images": [{"url": url, "content_type": "image/png", "width": 3, "height": 2}]}),
        json!({"data": {"images": [{"url": url}]}, "usage": {"cost": 0.4}}),
    ] {
        let transport = Arc::new(ReplayTransport::new(vec![
            post_any(GENERATE_URL).reply(HttpResponse::json(200, &payload)),
        ]));
        let answer = answered(send(&transport, &generate(json!({"prompt": "p"}))));
        assert_eq!(answer.files["image"].bytes, png);
        assert_eq!(answer.data, Value::Null);
    }
}

#[test]
fn a_hosted_answer_is_downloaded_once_with_only_accept() {
    let png = media::png(2, 2, Some(255));
    let transport = Arc::new(ReplayTransport::new(vec![
        post_any(GENERATE_URL).reply(HttpResponse::json(
            200,
            &json!({"images": [{"url": HOSTED, "content_type": "image/png", "width": 2,
                                "height": 2}]}),
        )),
        Expect::download(HOSTED)
            .header("accept", "image/*")
            .reply(HttpResponse::new(200, png.clone()).with_header("content-type", "image/png")),
    ]));
    let answer = answered(send(&transport, &generate(json!({"prompt": "p"}))));
    assert_eq!(answer.files["image"].bytes, png);
    assert_eq!(answer.data, Value::Null);
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    let download = &requests[1];
    assert_eq!(download.lane, Lane::Download);
    assert!(download.credential.is_none());
    assert_eq!(
        download.headers,
        vec![("accept".to_string(), "image/*".to_string())]
    );
    assert_eq!(download.body, Body::Empty);
    assert_eq!(download.max_response_bytes, image::MAX_OUTPUT_BYTES);
    assert!(download.timeout <= image::DEADLINE && download.timeout > Duration::from_secs(590));
    assert!(requests[0].credential.is_some());
}

#[test]
fn a_hosted_url_is_fetched_byte_for_byte_as_fal_gave_it() {
    // A signed URL may cover its raw query: FX hands the transport the provider's text, so `'`
    // is not re-encoded as `%27`, and an upper-case host or `:443` stays as fal wrote it.
    let png = media::png(2, 2, Some(255));
    for hosted in [
        "https://v3b.fal.media/files/output.png?k='x'&sig=a%2Fb~c",
        "https://V3B.FAL.MEDIA:443/files/output.png",
    ] {
        let transport = Arc::new(ReplayTransport::new(vec![
            post_any(GENERATE_URL).reply(HttpResponse::json(
                200,
                &json!({"images": [{"url": hosted}]}),
            )),
            Expect::download(hosted)
                .header("accept", "image/*")
                .reply(HttpResponse::new(200, png.clone())),
        ]));
        answered(send(&transport, &generate(json!({"prompt": "p"}))));
        assert_eq!(transport.requests()[1].url, hosted);
    }
}

#[test]
fn a_hosted_answer_without_declared_types_is_sniffed() {
    let jpeg = media::JPEG_HEAD.to_vec();
    let transport = Arc::new(ReplayTransport::new(vec![
        post_any(GENERATE_URL).reply(HttpResponse::json(
            200,
            &json!({"images": [{"url": HOSTED}]}),
        )),
        Expect::download(HOSTED).reply(HttpResponse::new(200, jpeg.clone())),
    ]));
    let answer = answered(send(&transport, &generate(json!({"prompt": "p"}))));
    assert_eq!(answer.files["image"].kind, "image/jpeg");
}

#[test]
fn untrusted_output_urls_are_never_fetched() {
    for hosted in [
        "http://v3b.fal.media/files/output.png",
        "https://127.0.0.1/output.png",
        "https://169.254.169.254/latest/meta-data",
        "https://cdn.example.test/output.png",
        "https://fal.media.evil.example/output.png",
        "https://user:secret@fal.media/output.png",
        "https://fal.media:8443/output.png",
    ] {
        let transport = Arc::new(ReplayTransport::new(vec![post_any(GENERATE_URL).reply(
            HttpResponse::json(200, &json!({"images": [{"url": hosted}]})),
        )]));
        let (reason, cost) = failed(send(&transport, &generate(json!({"prompt": "p"}))));
        assert_eq!(
            reason,
            "fal hosted output must use HTTPS on fal.media without userinfo or a custom port",
            "{hosted}"
        );
        assert_eq!(cost, None);
        assert_eq!(transport.requests().len(), 1, "{hosted}");
    }
}

#[test]
fn a_redirect_on_the_download_is_not_followed() {
    let transport = Arc::new(ReplayTransport::new(vec![
        post_any(GENERATE_URL).reply(HttpResponse::json(
            200,
            &json!({"images": [{"url": HOSTED}]}),
        )),
        Expect::download(HOSTED).reply(
            HttpResponse::new(302, Vec::new())
                .with_header("location", "https://cdn.example.test/elsewhere.png"),
        ),
    ]));
    let (reason, _) = failed(send(&transport, &generate(json!({"prompt": "p"}))));
    assert_eq!(reason, "fal output image download returned HTTP 302");
    assert_eq!(transport.requests().len(), 2);
}

#[test]
fn download_failures_fail_as_billed() {
    let png = media::png(2, 2, Some(255));
    let cases: Vec<(Value, Exchange, &str)> = vec![
        (
            json!({"url": HOSTED}),
            Expect::download(HOSTED).reply(
                HttpResponse::new(200, Vec::new())
                    .with_header("content-length", &(64 * 1024 * 1024 + 1).to_string()),
            ),
            "fal output image exceeds the 64 MiB safety limit",
        ),
        (
            json!({"url": HOSTED}),
            Exchange {
                expect: Expect::download(HOSTED),
                reply: Reply::Zeros {
                    status: 200,
                    headers: Vec::new(),
                    len: image::MAX_OUTPUT_BYTES + 1,
                },
            },
            "fal output image download failed: the response is larger than 67108864 bytes",
        ),
        (
            json!({"url": HOSTED}),
            Expect::download(HOSTED).reply(
                HttpResponse::new(200, png.clone())
                    .with_header("content-type", "application/octet-stream"),
            ),
            "fal image media type must be PNG, JPEG, or WebP",
        ),
        (
            json!({"url": HOSTED, "content_type": "image/png"}),
            Expect::download(HOSTED).reply(
                HttpResponse::new(200, png.clone()).with_header("content-type", "image/jpeg"),
            ),
            "fal output download media type does not match response metadata",
        ),
        (
            json!({"url": HOSTED}),
            Expect::download(HOSTED).reply(HttpResponse::new(200, Vec::new())),
            "fal output image download was empty",
        ),
        (
            json!({"url": HOSTED}),
            Expect::download(HOSTED).reply(HttpResponse::new(410, b"gone".to_vec())),
            "fal output image download returned HTTP 410",
        ),
        (
            json!({"url": HOSTED}),
            Expect::download(HOSTED).reply(HttpResponse::new(503, Vec::new())),
            "fal output image download returned HTTP 503",
        ),
        (
            json!({"url": HOSTED}),
            Expect::download(HOSTED).fail(TransportError::after_send(
                TransportErrorKind::Other,
                "connection reset",
            )),
            "fal output image download failed: connection reset",
        ),
        (
            json!({"url": HOSTED}),
            Expect::download(HOSTED).fail(TransportError::not_sent(
                TransportErrorKind::Connect,
                "connection refused",
            )),
            "fal output image download failed: connection refused",
        ),
        (
            json!({"url": HOSTED, "content_type": "image/webp"}),
            Expect::download(HOSTED).reply(HttpResponse::new(200, png.clone())),
            "fal output image is image/png, not the image/webp it was declared as",
        ),
    ];
    for (image_member, download, sentence) in cases {
        let transport = Arc::new(ReplayTransport::new(vec![
            post_any(GENERATE_URL).reply(HttpResponse::json(
                200,
                &json!({"images": [image_member], "usage": {"cost": 0.02}}),
            )),
            download,
        ]));
        let (reason, cost) = failed(send(&transport, &generate(json!({"prompt": "p"}))));
        assert_eq!(reason, sentence);
        assert_eq!(cost, Some(Usd(20_000)), "{sentence}");
    }
}

#[test]
fn malformed_payloads_fail_as_billed() {
    let (png, url) = data_png(2, 2);
    let jpeg_url = data_url("image/jpeg", media::JPEG_HEAD);
    let cases: Vec<(Value, &str)> = vec![
        (json!({}), "fal image generation returned no single image"),
        (
            json!({"images": []}),
            "fal image generation returned no single image",
        ),
        (
            json!({"images": [{"url": "one"}, {"url": "two"}]}),
            "fal image generation returned no single image",
        ),
        (
            json!({"images": ["not-an-object"]}),
            "fal image generation returned no single image",
        ),
        (
            json!({"images": [{}]}),
            "fal output image url must be non-empty",
        ),
        (
            json!({"images": [{"url": "  "}]}),
            "fal output image url must be non-empty",
        ),
        (
            json!({"images": [{"url": "data:image/png;base64,not-base64!"}]}),
            "fal output image data is not strict base64",
        ),
        (
            json!({"images": [{"url": data_url("image/png", b"hello")}]}),
            "fal output image is not a PNG, JPEG, WebP or GIF picture",
        ),
        (
            json!({"images": [{"url": jpeg_url, "content_type": "image/png"}]}),
            "fal data URI media type does not match response metadata",
        ),
        (
            json!({"images": [{"url": data_url("image/gif", &png)}]}),
            "fal image media type must be PNG, JPEG, or WebP",
        ),
        (
            json!({"images": [{"url": data_url("image/jpeg", &png)}]}),
            "fal output image is image/png, not the image/jpeg it was declared as",
        ),
        (
            json!({"images": [{"url": url, "content_type": 3}]}),
            "fal image media type must be PNG, JPEG, or WebP",
        ),
        (
            json!({"images": [{"url": url, "width": 32, "height": 32}]}),
            "fal output image dimensions do not match decoded bytes",
        ),
        (
            json!({"images": [{"url": url, "width": 2}]}),
            "fal output image dimensions must include both width and height",
        ),
        (
            json!({"images": [{"url": url, "width": true, "height": 2}]}),
            "fal output image width must be a positive integer",
        ),
    ];
    for (payload, sentence) in cases {
        let transport = Arc::new(ReplayTransport::new(vec![
            post_any(GENERATE_URL).reply(HttpResponse::json(200, &payload)),
        ]));
        let (reason, cost) = failed(send(&transport, &generate(json!({"prompt": "p"}))));
        assert_eq!(reason, sentence, "{payload}");
        assert_eq!(cost, None);
    }
    for (body, sentence) in [
        (
            b"<html>ok</html>".to_vec(),
            "fal image generation returned invalid JSON",
        ),
        (
            b"[1]".to_vec(),
            "fal image generation returned a non-object JSON response",
        ),
    ] {
        let transport = Arc::new(ReplayTransport::new(vec![
            post_any(GENERATE_URL).reply(HttpResponse::new(200, body)),
        ]));
        assert_eq!(
            failed(send(&transport, &generate(json!({"prompt": "p"})))),
            (sentence.to_string(), None)
        );
    }
}

// 4. Checks ------------------------------------------------------------------------------------

#[test]
fn checks_judge_the_answer() {
    let images = adapter(Arc::new(NoNetwork));
    let check = |request: Value, kind: &str, bytes: Vec<u8>| {
        let call = generate(request);
        images.check(
            &call,
            &Answer::new(Value::Null, None).with_file("image", kind, bytes),
        )
    };
    assert_eq!(
        check(
            json!({"prompt": "p"}),
            "image/jpeg",
            media::JPEG_HEAD.to_vec()
        )
        .unwrap_err(),
        "the answer is image/jpeg, not image/png"
    );
    assert_eq!(
        check(
            json!({"prompt": "p", "size": "1024x1024"}),
            "image/png",
            media::png(1024, 1008, None)
        )
        .unwrap_err(),
        "the image is 1024x1008, not 1024x1024"
    );
    assert_eq!(
        check(
            json!({"prompt": "p", "background": "transparent"}),
            "image/png",
            media::png(2, 2, None)
        )
        .unwrap_err(),
        "the picture asked for as transparent has no alpha channel"
    );
    assert_eq!(
        check(
            json!({"prompt": "p", "background": "opaque"}),
            "image/png",
            media::png_one_pixel_alpha(2, 2, 0)
        )
        .unwrap_err(),
        "the picture asked for as opaque has transparent pixels"
    );
    assert_eq!(
        check(
            json!({"prompt": "p", "size": "auto", "background": "opaque"}),
            "image/png",
            media::png(2, 2, Some(255))
        ),
        Ok(())
    );
    assert_eq!(
        images
            .check(
                &generate(json!({"prompt": "p"})),
                &Answer::new(Value::Null, None)
            )
            .unwrap_err(),
        "the answer carries no image"
    );
}

#[test]
fn a_jpeg_answer_is_answered_then_refused_by_the_check() {
    let call = generate(json!({"prompt": "p"}));
    let transport = Arc::new(ReplayTransport::new(vec![post_any(GENERATE_URL).reply(
        HttpResponse::json(
            200,
            &json!({"images": [{"url": data_url("image/jpeg", media::JPEG_HEAD)}]}),
        ),
    )]));
    let answer = answered(send(&transport, &call));
    assert_eq!(
        adapter(Arc::new(NoNetwork))
            .check(&call, &answer)
            .unwrap_err(),
        "the answer is image/jpeg, not image/png"
    );
}

// 5. Statuses ----------------------------------------------------------------------------------

#[test]
fn statuses_follow_the_fal_table() {
    let body = br#"{"detail": "secret detail test-fal-key"}"#.to_vec();
    let not_received = |reason: &str| Sent::not_received(reason);
    let waiting = |reason: &str, seconds: u64| Sent::NotReceived {
        reason: reason.into(),
        retry_after: Some(Duration::from_secs(seconds)),
    };
    let failed = |reason: &str, cost: Option<Usd>, retryable: bool| Sent::Failed {
        reason: reason.into(),
        cost,
        retryable,
    };
    let response = |status: u16| HttpResponse::new(status, body.clone());
    let cases: Vec<(Exchange, Sent)> = vec![
        (
            post_any(GENERATE_URL).fail(TransportError::not_sent(
                TransportErrorKind::Connect,
                "connection refused",
            )),
            not_received("fal image generation was not sent: connection refused"),
        ),
        (
            post_any(GENERATE_URL).fail(TransportError::not_sent(
                TransportErrorKind::Refused,
                "the network is off",
            )),
            Sent::Refused {
                reason: "fal image generation was not sent: the network is off".into(),
            },
        ),
        (
            post_any(GENERATE_URL).fail(TransportError::after_send(
                TransportErrorKind::Timeout,
                "the deadline passed",
            )),
            failed(
                "fal image generation failed: the deadline passed",
                None,
                true,
            ),
        ),
        (
            post_any(GENERATE_URL).reply(response(302).with_header("location", "https://x.test/")),
            failed(
                "fal image generation was redirected (HTTP 302); check FAL_BASE_URL",
                None,
                false,
            ),
        ),
        (
            post_any(GENERATE_URL).reply(response(401)),
            failed(
                "fal image generation returned HTTP 401",
                Some(Usd::ZERO),
                false,
            ),
        ),
        (
            post_any(GENERATE_URL).reply(response(402)),
            failed(
                "fal image generation returned HTTP 402",
                Some(Usd::ZERO),
                false,
            ),
        ),
        (
            post_any(GENERATE_URL).reply(response(422).with_header("x-request-id", "req_42")),
            failed(
                "fal image generation returned HTTP 422 (request req_42)",
                Some(Usd::ZERO),
                false,
            ),
        ),
        (
            post_any(GENERATE_URL).reply(response(418)),
            failed(
                "fal image generation returned HTTP 418",
                Some(Usd::ZERO),
                false,
            ),
        ),
        (
            post_any(GENERATE_URL).reply(response(408)),
            not_received("fal image generation returned HTTP 408"),
        ),
        (
            post_any(GENERATE_URL).reply(response(429).with_header("retry-after", "7")),
            waiting(
                "fal image generation was rate limited (HTTP 429); retry-after 7 s",
                7,
            ),
        ),
        (
            post_any(GENERATE_URL).reply(response(429)),
            not_received("fal image generation was rate limited (HTTP 429)"),
        ),
        (
            post_any(GENERATE_URL).reply(response(500)),
            failed("fal image generation returned HTTP 500", None, true),
        ),
        (
            post_any(GENERATE_URL).reply(response(503)),
            failed("fal image generation returned HTTP 503", None, true),
        ),
        (
            post_any(GENERATE_URL).reply(response(504)),
            failed("fal image generation returned HTTP 504", None, true),
        ),
    ];
    for (exchange, expected) in cases {
        let transport = Arc::new(ReplayTransport::new(vec![exchange]));
        let sent = send(&transport, &generate(json!({"prompt": "p"})));
        assert_eq!(sent, expected);
        assert_eq!(transport.requests().len(), 1);
    }
}

// 10. Costs ------------------------------------------------------------------------------------

#[test]
fn costs_come_from_the_top_level_usage() {
    let (_, url) = data_png(2, 2);
    let cases = [
        (json!({"cost": 0.4}), Some(Usd(400_000))),
        (json!({"cost": 0.00012345}), Some(Usd(124))),
        (json!({"cost": 0}), Some(Usd::ZERO)),
        (json!({}), None),
        (json!({"cost": -1}), None),
        (json!({"cost": "0.4"}), None),
        (json!({"cost": true}), None),
        (json!(null), None),
    ];
    for (usage, cost) in cases {
        let transport = Arc::new(ReplayTransport::new(vec![post_any(GENERATE_URL).reply(
            HttpResponse::json(200, &json!({"images": [{"url": url}], "usage": usage})),
        )]));
        let answer = answered(send(&transport, &generate(json!({"prompt": "p"}))));
        assert_eq!(answer.cost, cost, "{usage}");
    }
    let nested = Arc::new(ReplayTransport::new(vec![post_any(GENERATE_URL).reply(
        HttpResponse::json(
            200,
            &json!({"data": {"images": [{"url": url}], "usage": {"cost": 0.4}}}),
        ),
    )]));
    assert_eq!(
        answered(send(&nested, &generate(json!({"prompt": "p"})))).cost,
        None
    );
}

// 11. Secrets ----------------------------------------------------------------------------------

fn assert_no_key(text: &str) {
    assert!(!text.contains(KEY), "the key leaked into {text}");
}

#[test]
fn the_key_travels_only_as_the_post_credential() {
    let png = media::png(2, 2, Some(255));
    let signed = format!("{HOSTED}?sig=s3cr3t-signature");
    let transport = Arc::new(ReplayTransport::new(vec![
        post_any(GENERATE_URL).reply(HttpResponse::json(
            200,
            &json!({"images": [{"url": HOSTED}]}),
        )),
        Expect::download(HOSTED).reply(HttpResponse::new(200, png)),
        post_any(GENERATE_URL).reply(HttpResponse::new(
            401,
            format!(r#"{{"detail": "bad key {KEY}"}}"#).into_bytes(),
        )),
        post_any(GENERATE_URL).reply(HttpResponse::json(
            200,
            &json!({"images": [{"url": signed}]}),
        )),
        Expect::download(signed.clone()).fail(TransportError::after_send(
            TransportErrorKind::Other,
            format!("reset while reading {signed}"),
        )),
    ]));
    let images = adapter(transport.clone());
    let call = generate(json!({"prompt": "p"}));
    let answer = answered(block_on(images.send(&call)));
    assert_no_key(&answer.data.to_string());
    let Sent::Failed { reason, .. } = block_on(images.send(&call)) else {
        panic!("a failure")
    };
    assert_no_key(&reason);
    let Sent::Failed { reason, .. } = block_on(images.send(&call)) else {
        panic!("a failure")
    };
    assert_no_key(&reason);
    assert!(
        !reason.contains("s3cr3t"),
        "the signed URL leaked into {reason}"
    );
    assert_eq!(
        reason,
        "fal output image download failed: reset while reading [redacted]"
    );
    transport.assert_done();
    for request in transport.requests() {
        let body = match &request.body {
            Body::Json(value) => value.to_string(),
            Body::Empty => String::new(),
            other => panic!("unexpected body {other:?}"),
        };
        assert_no_key(&body);
        assert_no_key(&request.url);
        assert_no_key(&format!("{request:?}"));
        for (name, value) in &request.headers {
            assert_no_key(name);
            assert_no_key(value);
        }
        match request.lane {
            Lane::Provider => {
                assert_eq!(
                    request.credential.as_ref().unwrap().header_value(),
                    format!("Key {KEY}")
                );
                assert!(request.url.starts_with("https://fal.run/"));
            }
            _ => assert!(request.credential.is_none()),
        }
    }
}

#[test]
fn register_serves_the_four_fal_capabilities() {
    let mut adapters = Adapters::new();
    register(&mut adapters, &setup(Arc::new(NoNetwork), test_keys()));
    let served: Vec<(String, String)> = adapters.served();
    let names: Vec<&str> = served.iter().map(|(c, _)| c.as_str()).collect();
    assert_eq!(
        names,
        [
            "image.generate",
            "image.edit",
            "video.generate",
            "background.remove"
        ]
    );
    assert!(served.iter().all(|(_, p)| p == "fal"));
}
