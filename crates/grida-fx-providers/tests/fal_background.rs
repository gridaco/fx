//! fal `background.remove` (spec/providers.md §9.3) over synthetic exchanges. Every test that
//! sends asserts each request; every refusal runs over `NoNetwork`. Media are built in code.

use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{Answer, CallRequest, RequestAdapter, Sent};
use grida_fx_providers::fal::FalClients;
use grida_fx_providers::fal::background::{self, FalBackground};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, media, setup, test_keys};
use grida_fx_providers::transport::replay::{
    Exchange, Expect, ExpectBody, NoNetwork, ReplayTransport,
};
use grida_fx_providers::transport::{
    Body, HttpResponse, Lane, Transport, TransportError, TransportErrorKind,
};
use grida_fx_providers::wire::data_url;
use serde_json::{Value, json};
use std::sync::Arc;

const ROUTE: &str = "fal-ai/birefnet/v2@fal";
const URL: &str = "https://fal.run/fal-ai/birefnet/v2";
const HOSTED: &str = "https://v3.fal.media/files/cutout.png";
const KEY: &str = "test-fal-key";

fn adapter(transport: Arc<dyn Transport>) -> FalBackground {
    FalBackground::new(FalClients::new(&setup(transport, test_keys())))
}

fn call_with(kind: &str, bytes: &[u8]) -> TestCall {
    let mut builder = CallBuilder::new("background.remove", ROUTE)
        .contract(json!({"adapter": "fal-run-birefnet", "adapter_behavior": 1}));
    let image = builder.file(kind, bytes);
    builder.request(json!({"image": image})).build()
}

fn call() -> TestCall {
    call_with("image/png", &media::png(2, 2, Some(255)))
}

fn post() -> Expect {
    Expect::post_json(URL, Value::Null)
        .credential("authorization", "Key test-fal-key")
        .body(ExpectBody::Any)
}

fn send(transport: &Arc<ReplayTransport>, call: &CallRequest) -> Sent {
    let sent = block_on(adapter(transport.clone()).send(call));
    transport.assert_done();
    sent
}

fn refused(call: &CallRequest, keys: Keys) -> String {
    let adapter = FalBackground::new(FalClients::new(&setup(Arc::new(NoNetwork), keys)));
    match block_on(adapter.send(call)) {
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

#[test]
fn the_body_carries_the_fixed_members_in_order() {
    let source = media::png(2, 2, Some(255));
    let call = call_with("image/png", &source);
    let body = json!({"image_url": data_url("image/png", &source), "model": "General Use (Light)",
                      "operating_resolution": "1024x1024", "output_mask": false,
                      "refine_foreground": true, "output_format": "png", "mask_only": false,
                      "sync_mode": true});
    let cutout = media::png(2, 2, Some(0));
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::post_json(URL, Value::Null)
            .credential("authorization", "Key test-fal-key")
            .body(ExpectBody::JsonText(serde_json::to_string(&body).unwrap()))
            .reply(HttpResponse::json(
                200,
                &json!({"image": {"url": data_url("image/png", &cutout), "content_type": "image/png",
                                  "width": 99, "height": 99},
                        "mask_image": {"url": "https://v3.fal.media/files/mask.png"}}),
            )),
    ]));
    let answer = answered(send(&transport, &call));
    assert_eq!(answer.files["image"].kind, "image/png");
    assert_eq!(answer.files["image"].bytes, cutout);
    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.cost, None);
    assert_eq!(adapter(Arc::new(NoNetwork)).check(&call, &answer), Ok(()));
    let request = &transport.requests()[0];
    assert_eq!(request.timeout, background::DEADLINE);
    assert_eq!(request.lane, Lane::Provider);
}

#[test]
fn a_file_that_is_not_a_picture_is_refused_before_sending() {
    for (kind, bytes) in [
        ("application/octet-stream", b"opaque store bytes".to_vec()),
        ("model/gltf-binary", media::GLB.to_vec()),
        ("text/plain", b"a note".to_vec()),
    ] {
        assert_eq!(
            refused(&call_with(kind, &bytes), test_keys()),
            format!("image is {kind}, not a picture")
        );
    }
    // Any picture kind goes under its own kind.
    let webp = b"RIFF\x00\x00\x00\x00WEBPVP8 ".to_vec();
    let transport =
        Arc::new(ReplayTransport::new(vec![post().reply(HttpResponse::json(
        200,
        &json!({"data": {"image": {"url": data_url("image/png", &media::png(1, 1, Some(0)))}}}),
    ))]));
    answered(send(&transport, &call_with("image/webp", &webp)));
    let Body::Json(body) = &transport.requests()[0].body else {
        panic!("a JSON body")
    };
    assert_eq!(body["image_url"], json!(data_url("image/webp", &webp)));
}

#[test]
fn a_hosted_cutout_is_downloaded_without_credential() {
    let cutout = media::png(2, 2, Some(0));
    let transport = Arc::new(ReplayTransport::new(vec![
        post().reply(HttpResponse::json(
            200,
            &json!({"image": {"url": HOSTED}, "usage": {"cost": 0.002}}),
        )),
        Expect::download(HOSTED)
            .header("accept", "image/*")
            .reply(HttpResponse::new(200, cutout.clone()).with_header("content-type", "image/png")),
    ]));
    let answer = answered(send(&transport, &call()));
    assert_eq!(answer.files["image"].bytes, cutout);
    assert_eq!(answer.cost, Some(Usd(2_000)));
    let download = &transport.requests()[1];
    assert_eq!(download.lane, Lane::Download);
    assert!(download.credential.is_none());
    assert_eq!(
        download.headers,
        vec![("accept".to_string(), "image/*".to_string())]
    );
    assert!(download.timeout <= background::DEADLINE);
}

#[test]
fn answers_that_fail_as_billed() {
    let png = media::png(2, 2, Some(0));
    let cases: Vec<(Value, &str)> = vec![
        (json!({}), "fal background removal returned no image"),
        (
            json!({"image": "x"}),
            "fal background removal returned no image",
        ),
        (
            json!({"image": {}}),
            "fal output image url must be non-empty",
        ),
        (
            json!({"image": {"url": data_url("image/jpeg", media::JPEG_HEAD)}}),
            "fal image media type must be PNG, WebP, or GIF",
        ),
        (
            json!({"image": {"url": data_url("image/png", &png), "content_type": "image/webp"}}),
            "fal data URI media type does not match response metadata",
        ),
        (
            json!({"image": {"url": "https://cdn.example.test/cutout.png"}}),
            "fal hosted output must use HTTPS on fal.media without userinfo or a custom port",
        ),
        (
            json!({"image": {"url": "data:image/png;base64,not-base64!"}}),
            "fal output image data is not strict base64",
        ),
    ];
    for (payload, sentence) in cases {
        let transport = Arc::new(ReplayTransport::new(vec![
            post().reply(HttpResponse::json(200, &payload)),
        ]));
        assert_eq!(
            send(&transport, &call()),
            Sent::Failed {
                reason: sentence.into(),
                cost: None,
                retryable: true
            },
            "{payload}"
        );
        assert_eq!(transport.requests().len(), 1);
    }
}

#[test]
fn the_check_wants_a_png() {
    let call = call();
    let check = |kind: &str, bytes: Vec<u8>| {
        adapter(Arc::new(NoNetwork)).check(
            &call,
            &Answer::new(Value::Null, None).with_file("image", kind, bytes),
        )
    };
    assert_eq!(check("image/png", media::png(1, 1, Some(0))), Ok(()));
    assert_eq!(
        check("image/webp", b"RIFF\0\0\0\0WEBPVP8 ".to_vec()).unwrap_err(),
        "the answer is image/webp, not image/png"
    );
    assert_eq!(
        check("image/png", media::JPEG_HEAD.to_vec()).unwrap_err(),
        "the answer is image/jpeg, not image/png"
    );
    assert_eq!(
        adapter(Arc::new(NoNetwork))
            .check(&call, &Answer::new(Value::Null, None))
            .unwrap_err(),
        "the answer carries no image"
    );

    // A WebP cutout is answered, then refused by the check.
    let webp = b"RIFF\x0c\0\0\0WEBPVP8 \0\0\0\0".to_vec();
    let transport = Arc::new(ReplayTransport::new(vec![post().reply(
        HttpResponse::json(
            200,
            &json!({"image": {"url": data_url("image/webp", &webp)}}),
        ),
    )]));
    let answer = answered(send(&transport, &call));
    assert_eq!(answer.files["image"].kind, "image/webp");
    assert_eq!(
        adapter(Arc::new(NoNetwork))
            .check(&call, &answer)
            .unwrap_err(),
        "the answer is image/webp, not image/png"
    );
}

#[test]
fn statuses_follow_the_run_host_table() {
    let cases: Vec<(Exchange, Sent)> = vec![
        (
            post().reply(HttpResponse::new(503, Vec::new())),
            Sent::Failed {
                reason: "fal background removal returned HTTP 503".into(),
                cost: None,
                retryable: true,
            },
        ),
        (
            post().reply(HttpResponse::new(422, b"{\"detail\": \"bad\"}".to_vec())),
            Sent::Failed {
                reason: "fal background removal returned HTTP 422".into(),
                cost: Some(Usd::ZERO),
                retryable: false,
            },
        ),
        (
            post().reply(HttpResponse::new(301, Vec::new())),
            Sent::Failed {
                reason: "fal background removal was redirected (HTTP 301); check FAL_BASE_URL"
                    .into(),
                cost: None,
                retryable: false,
            },
        ),
        (
            post().reply(HttpResponse::new(429, Vec::new())),
            Sent::not_received("fal background removal was rate limited (HTTP 429)"),
        ),
        (
            post().fail(TransportError::not_sent(
                TransportErrorKind::Connect,
                "dns failure",
            )),
            Sent::not_received("fal background removal was not sent: dns failure"),
        ),
        (
            post().fail(TransportError::after_send(
                TransportErrorKind::Other,
                "connection reset",
            )),
            Sent::Failed {
                reason: "fal background removal failed: connection reset".into(),
                cost: None,
                retryable: true,
            },
        ),
    ];
    for (exchange, expected) in cases {
        let transport = Arc::new(ReplayTransport::new(vec![exchange]));
        assert_eq!(send(&transport, &call()), expected);
        assert_eq!(transport.requests().len(), 1);
    }
}

#[test]
fn refusals_send_nothing() {
    let foreign = CallBuilder::new("background.remove", ROUTE)
        .contract(json!({"adapter": "gnode-fal-image-v1"}))
        .request(json!({"image": {"file": "f".repeat(64)}}))
        .build();
    assert_eq!(
        refused(&foreign, test_keys()),
        "fal-ai/birefnet/v2@fal is not a background.remove route this adapter serves"
    );
    let extra = CallBuilder::new("background.remove", ROUTE)
        .request(json!({"image": {"file": "f".repeat(64)}, "model": "Matting"}))
        .build();
    assert_eq!(
        refused(&extra, test_keys()),
        "background.remove takes no member model"
    );
    let missing = CallBuilder::new("background.remove", ROUTE)
        .request(json!({}))
        .build();
    assert_eq!(
        refused(&missing, test_keys()),
        "background.remove needs image"
    );
    let unknown = CallBuilder::new("background.remove", ROUTE)
        .request(json!({"image": {"file": "f".repeat(64)}}))
        .build();
    assert_eq!(refused(&unknown, Keys::none()), "FAL_KEY is not set");
    assert_eq!(refused(&unknown, test_keys()), "image has no bytes to send");
}

#[test]
fn costs_and_the_key() {
    let cutout = media::png(1, 1, Some(0));
    for (usage, cost) in [
        (json!({"cost": 0.4}), Some(Usd(400_000))),
        (json!({"cost": -0.1}), None),
        (json!({"cost": "0.4"}), None),
        (json!({"cost": false}), None),
    ] {
        let transport = Arc::new(ReplayTransport::new(vec![post().reply(
            HttpResponse::json(
                200,
                &json!({"image": {"url": data_url("image/png", &cutout)}, "usage": usage}),
            ),
        )]));
        assert_eq!(answered(send(&transport, &call())).cost, cost);
    }
    let transport = Arc::new(ReplayTransport::new(vec![
        post().reply(HttpResponse::json(200, &json!({"image": {"url": HOSTED}}))),
        Expect::download(HOSTED).reply(HttpResponse::new(
            404,
            format!("no file for {KEY}").into_bytes(),
        )),
    ]));
    let Sent::Failed { reason, .. } = send(&transport, &call()) else {
        panic!("a failure")
    };
    assert_eq!(reason, "fal output image download returned HTTP 404");
    for request in transport.requests() {
        assert!(!request.url.contains(KEY));
        assert!(!format!("{request:?}").contains(KEY));
        if let Body::Json(body) = &request.body {
            assert!(!body.to_string().contains(KEY));
        }
        match request.lane {
            Lane::Download => assert!(request.credential.is_none()),
            _ => assert_eq!(
                request.credential.as_ref().unwrap().header_value(),
                format!("Key {KEY}")
            ),
        }
    }
}
