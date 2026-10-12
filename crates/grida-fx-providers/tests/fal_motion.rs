//! Reference-performance transfer through the shared fal queue lifecycle. All files and
//! exchanges are synthetic; no test contacts a provider or reads credentials.

use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{Answer, CallRequest, Collected, LongJob, Submitted};
use grida_fx_providers::clock::{Clock, FakeClock};
use grida_fx_providers::fal::FalClients;
use grida_fx_providers::fal::video::{FalVideo, MAX_MOTION_BYTES};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::stand_in::{StandInChecks, StandInFile};
use grida_fx_providers::testing::{
    CallBuilder, TestCall, block_on, media, setup_with_clock, test_keys,
};
use grida_fx_providers::transport::replay::{
    Exchange, Expect, ExpectBody, NoNetwork, ReplayTransport,
};
use grida_fx_providers::transport::{
    HttpResponse, Lane, Method, Transport, TransportError, TransportErrorKind,
};
use grida_fx_providers::wire::data_url;
use serde_json::{Value, json};
use std::sync::Arc;

const INITIATE: &str = "https://rest.fal.ai/storage/upload/initiate?storage_type=fal-cdn-v3";
const UPLOAD: &str = "https://v3.fal.media/upload/motion?signature=private-upload";
const HOSTED: &str = "https://v3.fal.media/files/motion.mp4";

fn initiate() -> Expect {
    Expect::post_json(
        INITIATE,
        json!({"content_type":"video/mp4","file_name":"motion.mp4"}),
    )
    .credential("authorization", "Key test-fal-key")
    .header(
        "x-fal-object-lifecycle",
        "{\"expiration_duration_seconds\":3600}",
    )
}

fn uploaded() -> Vec<Exchange> {
    uploaded_bytes(CLIP)
}

fn uploaded_bytes(bytes: &[u8]) -> Vec<Exchange> {
    vec![
        initiate().reply(HttpResponse::json(
            200,
            &json!({"upload_url":UPLOAD,"file_url":HOSTED}),
        )),
        Expect::new(Method::Put, UPLOAD, Lane::Upload)
            .body(ExpectBody::Bytes(bytes.to_vec()))
            .reply(HttpResponse::new(200, Vec::new())),
    ]
}

const ROUTE: &str = "fal-ai/kling-video/v3/pro/motion-control@fal";
const CLIP: &[u8] = include_bytes!("../../../spec/vectors/facts/mp4/h264_24fps.mp4");
const SHORT: &[u8] = include_bytes!("../../../spec/vectors/facts/mp4/faststart.mp4");
const AUDIO_ONLY: &[u8] = include_bytes!("../../../spec/vectors/facts/mp4/audio_only.mp4");
const STATUS: &str = "https://queue.fal.run/fal-ai/kling-video/requests/req-1/status";
const RESULT: &str = "https://queue.fal.run/fal-ai/kling-video/requests/req-1";

fn adapter(transport: Arc<dyn Transport>, keys: Keys) -> FalVideo {
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::new());
    FalVideo::motion_transfer(
        FalClients::new(&setup_with_clock(transport, keys, clock.clone())),
        clock,
    )
}

fn call_with_file(request: Value, kind: &str, bytes: &[u8]) -> TestCall {
    let mut builder = CallBuilder::new("video.motion_transfer", ROUTE)
        .contract(json!({"adapter": "fal-motion-control", "adapter_behavior": 1}));
    let character = builder.file("image/png", &media::png(2, 2, None));
    let motion = builder.file(kind, bytes);
    let mut base = json!({"prompt":"Follow the recorded performance.","duration":2,
        "character":character,"motion":motion});
    for (name, value) in request.as_object().unwrap() {
        if value.is_null() {
            base.as_object_mut().unwrap().remove(name);
        } else {
            base[name] = value.clone();
        }
    }
    builder.request(base).build()
}

fn call(request: Value) -> TestCall {
    call_with_file(request, "video/mp4", CLIP)
}

fn refused(call: &CallRequest, keys: Keys) -> String {
    match block_on(adapter(Arc::new(NoNetwork), keys).submit(call)) {
        Submitted::Refused { reason } => reason,
        other => panic!("expected refusal, got {other:?}"),
    }
}

#[test]
fn hosted_motion_and_inline_character_are_sent_without_generation_controls() {
    for (orientation, sound) in [("video", false), ("image", true)] {
        let call = call(json!({"orientation":orientation,"keep_original_sound":sound}));
        let image = media::png(2, 2, None);
        let body = json!({"prompt":"Follow the recorded performance.",
            "image_url":data_url("image/png",&image),"video_url":HOSTED,
            "character_orientation":orientation,"keep_original_sound":sound});
        let mut exchanges = uploaded();
        exchanges.push(
            Expect::post_json(
                "https://queue.fal.run/fal-ai/kling-video/v3/pro/motion-control",
                body,
            )
            .credential("authorization", "Key test-fal-key")
            .reply(HttpResponse::json(
                200,
                &json!({"request_id":"req-1",
                    "status_url":STATUS,"response_url":RESULT}),
            )),
        );
        let transport = Arc::new(ReplayTransport::new(exchanges));
        assert!(matches!(
            block_on(adapter(transport.clone(), test_keys()).submit(&call)),
            Submitted::Accepted { .. }
        ));
        transport.assert_done();
    }
}

#[test]
fn defaults_follow_the_video_and_omit_source_sound() {
    let call = call(json!({}));
    let image = media::png(2, 2, None);
    let mut exchanges = uploaded();
    exchanges.push(Expect::post_json("https://queue.fal.run/fal-ai/kling-video/v3/pro/motion-control",
            json!({"prompt":"Follow the recorded performance.","image_url":data_url("image/png",&image),
                "video_url":HOSTED,"character_orientation":"video","keep_original_sound":false}))
            .credential("authorization", "Key test-fal-key")
            .reply(HttpResponse::json(422,&json!({"detail":"invalid input"}))));
    let transport = Arc::new(ReplayTransport::new(exchanges));
    assert!(
        matches!(block_on(adapter(transport.clone(), test_keys()).submit(&call)),
        Submitted::Failed{cost:Some(cost),retryable:false,..} if cost==Usd::ZERO)
    );
    transport.assert_done();
}

#[test]
fn invalid_source_values_and_unsupported_controls_never_send() {
    for (request, expected) in [
        (
            json!({"duration":null}),
            "video.motion_transfer needs duration",
        ),
        (json!({"duration":"2"}), "duration is a number"),
        (json!({"duration":0}), "motion duration must be positive"),
        (json!({"duration":31}), "motion duration must be positive"),
        (
            json!({"orientation":"image","duration":11}),
            "motion duration must be positive",
        ),
        (
            json!({"orientation":"camera"}),
            "orientation is video or image",
        ),
        (
            json!({"duration":1}),
            "motion runs 2.000 s, not the declared 1.000 s",
        ),
        (
            json!({"resolution":"720p"}),
            "video.motion_transfer takes no member resolution",
        ),
        (
            json!({"video_url":"https://example.test/v.mp4"}),
            "video.motion_transfer takes no member video_url",
        ),
        (
            json!({"keep_original_sound":"false"}),
            "keep_original_sound is true or false",
        ),
        (
            json!({"motion":{"file":"a".repeat(64)}}),
            "motion has no bytes to send",
        ),
        (
            json!({"character":{"file":"b".repeat(64)}}),
            "character has no bytes to send",
        ),
    ] {
        assert!(
            refused(&call(request.clone()), test_keys()).starts_with(expected),
            "{request}"
        );
    }
    assert_eq!(
        refused(&call(json!({})), Keys::none()),
        "FAL_KEY is not set"
    );
    assert_eq!(
        refused(&call_with_file(json!({}), "image/png", CLIP), test_keys()),
        "motion is image/png, not video/mp4"
    );
    assert_eq!(
        refused(
            &call_with_file(json!({}), "video/mp4", b"invalid"),
            test_keys()
        ),
        "motion is not an MP4 file"
    );
    assert_eq!(
        refused(
            &call_with_file(json!({}), "video/mp4", AUDIO_ONLY),
            test_keys()
        ),
        "the file carries no video stream"
    );
    let mut large = call(json!({}));
    large
        .call
        .files
        .values_mut()
        .find(|f| f.kind == "video/mp4")
        .unwrap()
        .size = MAX_MOTION_BYTES + 1;
    assert!(refused(&large, test_keys()).starts_with("motion exceeds"));
}

#[test]
fn shared_collection_downloads_once_without_credentials_and_checks_duration() {
    let call = call(json!({}));
    let result_url = "https://v3b.fal.media/files/output.mp4?sig=private-token";
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::get(STATUS)
            .credential("authorization", "Key test-fal-key")
            .reply(HttpResponse::json(
                200,
                &json!({"status":"COMPLETED","request_id":"req-1"}),
            )),
        Expect::get(RESULT)
            .credential("authorization", "Key test-fal-key")
            .reply(HttpResponse::json(
                200,
                &json!({"video":{"url":result_url},"usage":{"cost":0.336}}),
            )),
        Expect::download(result_url).reply(HttpResponse::new(200, CLIP.to_vec())),
    ]));
    let adapter = adapter(transport.clone(), test_keys());
    let handle = json!({"request_id":"req-1","status_path":"fal-ai/kling-video/requests/req-1/status",
        "response_path":"fal-ai/kling-video/requests/req-1"});
    let Collected::Answered(answer) = block_on(adapter.collect(&call, &handle)) else {
        panic!("answer")
    };
    transport.assert_done();
    assert_eq!(answer.cost, Some(Usd::parse("0.336").unwrap()));
    assert_eq!(
        answer.data,
        json!({"facts":{"width":64,"height":48,"duration_seconds":2,"fps":24}})
    );
    assert!(!answer.data.to_string().contains("private-token"));
    assert_eq!(adapter.check(&call, &answer), Ok(()));
    // A different resolution is admitted, but a different performance length is not.
    let shorter = Answer::new(Value::Null, None).with_file("video", "video/mp4", SHORT.to_vec());
    assert!(
        adapter
            .check(&call, &shorter)
            .unwrap_err()
            .starts_with("the clip runs 1.000 s")
    );
}

#[test]
fn stand_in_answers_get_clip_facts_and_the_same_duration_check() {
    let call = call(json!({}));
    let files = |bytes: &[u8]| {
        indexmap::IndexMap::from([(
            "video".to_string(),
            StandInFile {
                kind: None,
                bytes: bytes.to_vec(),
            },
        )])
    };
    let checks = StandInChecks::new();
    let answer = checks.answer(&call, files(CLIP), Value::Null).unwrap();
    assert_eq!(answer.files["video"].kind, "video/mp4");
    assert_eq!(answer.data["facts"]["duration_seconds"], json!(2));
    assert!(
        checks
            .answer(&call, files(SHORT), Value::Null)
            .unwrap_err()
            .starts_with("the clip runs")
    );
    assert_eq!(
        checks
            .answer(&call, files(CLIP), json!({"facts":{}}))
            .unwrap_err(),
        "video.motion_transfer's data is taken from its file: answer null"
    );
}

#[test]
fn validation_failures_expose_only_bounded_safe_fields() {
    let call = call(json!({}));
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::get(STATUS)
            .credential("authorization", "Key test-fal-key")
            .reply(HttpResponse::json(200, &json!({"status":"COMPLETED"}))),
        Expect::get(RESULT)
            .credential("authorization", "Key test-fal-key")
            .reply(HttpResponse::json(422, &json!({"detail":[{
                "loc":["body","video_url"], "type":"video_duration_too_short",
                "msg":"Video must be at least 3 seconds; https://private.example/x?sig=secret data:video/mp4;base64,AAAA /Users/private/input.mp4 \"private caption\" Follow the recorded performance. Key test-fal-key",
                "input":"private-input-never-retained", "ctx":{"private":"secret-context"}
            }]}))),
    ]));
    let handle = json!({"request_id":"req-1","status_path":"fal-ai/kling-video/requests/req-1/status",
        "response_path":"fal-ai/kling-video/requests/req-1"});
    let Collected::Ended { reason, cost } =
        block_on(adapter(transport.clone(), test_keys()).collect(&call, &handle))
    else {
        panic!("validation must end the existing job")
    };
    transport.assert_done();
    assert_eq!(cost, None);
    assert!(reason.contains("loc=body.video_url; type=video_duration_too_short"));
    assert!(reason.contains("msg=Video must be at least 3 seconds"));
    assert!(reason.chars().count() <= 500);
    for private in [
        "https:",
        "data:",
        "/Users/",
        "private caption",
        "Follow the recorded",
        "test-fal-key",
        "private-input",
        "secret-context",
    ] {
        assert!(!reason.contains(private), "{reason}");
    }
}

#[test]
fn status_errors_use_the_same_request_and_path_scrubber_as_validation_errors() {
    let call = call(json!({}));
    let source_path = call.files.values().next().unwrap().path.to_str().unwrap();
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::get(STATUS)
            .credential("authorization", "Key test-fal-key")
            .reply(HttpResponse::json(200, &json!({
                "status":"COMPLETED", "error_type":"model_error",
                "error":format!("Invalid video: Follow the recorded performance. {source_path} https://private.example/file?signature=secret data:video/mp4;base64,AAAA /Users/private/input.mp4 \"private caption\" Key test-fal-key")
            }))),
    ]));
    let handle = json!({"request_id":"req-1","status_path":"fal-ai/kling-video/requests/req-1/status",
        "response_path":"fal-ai/kling-video/requests/req-1"});
    let Collected::Ended { reason, cost } =
        block_on(adapter(transport.clone(), test_keys()).collect(&call, &handle))
    else {
        panic!("a status error must end the existing job")
    };
    transport.assert_done();
    assert_eq!(cost, None);
    assert!(reason.starts_with("fal video job failed (model_error): Invalid video:"));
    assert!(reason.chars().count() <= 500);
    for private in [
        "Follow the recorded",
        source_path,
        "https:",
        "data:",
        "/Users/",
        "private caption",
        "test-fal-key",
    ] {
        assert!(!reason.contains(private), "{reason}");
    }
}

#[test]
fn upload_failures_never_submit_a_paid_job_or_keep_returned_urls() {
    let call = call(json!({}));
    for status in [400, 401, 403, 422, 429, 500] {
        let transport = Arc::new(ReplayTransport::new(vec![initiate().reply(
            HttpResponse::json(status, &json!({"detail":"private response"})),
        )]));
        let outcome = block_on(adapter(transport.clone(), test_keys()).submit(&call));
        transport.assert_done();
        match outcome {
            Submitted::Failed {
                cost,
                retryable,
                reason,
            } => {
                assert_eq!(cost, Some(Usd::ZERO));
                assert!(!retryable);
                assert!(!reason.contains("private response"));
            }
            Submitted::NotReceived { reason, .. } if matches!(status, 429 | 500) => {
                assert!(reason.contains("video submission was not sent"));
            }
            other => panic!("{status}: {other:?}"),
        }
    }
    for slot in [
        json!({}),
        json!({"upload_url":"https://foreign.example/upload","file_url":HOSTED}),
        json!({"upload_url":UPLOAD,"file_url":"https://foreign.example/motion.mp4"}),
    ] {
        let transport = Arc::new(ReplayTransport::new(vec![
            initiate().reply(HttpResponse::json(200, &slot)),
        ]));
        assert!(matches!(
            block_on(adapter(transport.clone(), test_keys()).submit(&call)),
            Submitted::Refused { .. }
        ));
        transport.assert_done();
    }
    let transport = Arc::new(ReplayTransport::new(vec![
        initiate().reply(HttpResponse::json(
            200,
            &json!({"upload_url":UPLOAD,"file_url":HOSTED}),
        )),
        Expect::new(Method::Put, UPLOAD, Lane::Upload)
            .body(ExpectBody::Bytes(CLIP.to_vec()))
            .fail(TransportError::after_send(
                TransportErrorKind::Timeout,
                format!("timeout at {UPLOAD} and {HOSTED}"),
            )),
    ]));
    let Submitted::NotReceived { reason, .. } =
        block_on(adapter(transport.clone(), test_keys()).submit(&call))
    else {
        panic!("the paid submit is known not sent even when a free upload timed out")
    };
    transport.assert_done();
    assert!(reason.contains("video submission was not sent"));
    assert!(!reason.contains("https:"));
    assert!(!reason.contains("private-upload"));
}

fn wan_call(request: Value, bytes: &[u8]) -> TestCall {
    let mut params = json!({"duration":4});
    for (key, value) in request.as_object().unwrap() {
        params[key] = value.clone();
    }
    let mut call = call_with_file(params, "video/mp4", bytes);
    call.call.route.model = "fal-ai/wan-motion".into();
    call.call.route.contract = json!({"adapter":"fal-wan-motion","adapter_behavior":1});
    call
}

#[test]
fn wan_profile_sends_retargeting_without_unrequested_identity_preprocessing() {
    let source = media::mp4(720, 1280, 24, 96);
    let call = wan_call(json!({}), &source);
    let mut exchanges = uploaded_bytes(&source);
    exchanges.push(Expect::post_json("https://queue.fal.run/fal-ai/wan-motion", json!({
        "prompt":"Follow the recorded performance.",
        "image_url":data_url("image/png", &media::png(2,2,None)),
        "video_url":HOSTED,
        "adapt_motion":true,"enhance_identity":false,"acceleration":"regular",
        "enable_safety_checker":true
    })).credential("authorization", "Key test-fal-key")
       .reply(HttpResponse::json(200,&json!({"request_id":"req-1","status_url":"https://queue.fal.run/fal-ai/wan-motion/requests/req-1/status","response_url":"https://queue.fal.run/fal-ai/wan-motion/requests/req-1"}))));
    let transport = Arc::new(ReplayTransport::new(exchanges));
    assert!(matches!(
        block_on(adapter(transport.clone(), test_keys()).submit(&call)),
        Submitted::Accepted { .. }
    ));
    transport.assert_done();
    let requests = transport.requests();
    assert!(requests[1].credential.is_none());
}

#[test]
fn wan_profile_refuses_unsupported_controls_and_frame_pricing_mismatches_before_upload() {
    let source = media::mp4(720, 1280, 24, 96);
    for (request, expected) in [
        (json!({"orientation":"image"}), "only video orientation"),
        (
            json!({"keep_original_sound":true}),
            "does not preserve original sound",
        ),
        (json!({"duration":2}), "3 to 10 seconds"),
        (json!({"duration":11}), "3 to 10 seconds"),
        (
            json!({"duration":4.1}),
            "requires 24 fps and a duration matching its frame count",
        ),
        (
            json!({"enhance_identity":true}),
            "takes no member enhance_identity",
        ),
    ] {
        let reason = refused(&wan_call(request.clone(), &source), test_keys());
        assert!(reason.contains(expected), "{request}: {reason}");
    }
    let source25 = media::mp4(720, 1280, 25, 100);
    assert!(refused(&wan_call(json!({}), &source25), test_keys()).contains("requires 24 fps"));
    let too_many = media::mp4(720, 1280, 24, 241);
    assert!(
        refused(&wan_call(json!({"duration":10}), &too_many), test_keys())
            .contains("72 to 240 frames")
    );
}

#[test]
fn wan_profile_uses_shared_answer_checks_and_stand_in_facts() {
    let source = media::mp4(720, 1280, 24, 96);
    let call = wan_call(json!({}), &source);
    let checks = StandInChecks::new();
    let files = indexmap::IndexMap::from([(
        "video".to_string(),
        StandInFile {
            kind: None,
            bytes: source,
        },
    )]);
    let answer = checks.answer(&call, files, Value::Null).unwrap();
    assert_eq!(answer.data["facts"]["duration_seconds"], json!(4));
    assert_eq!(answer.data["facts"]["fps"], json!(24));
}
