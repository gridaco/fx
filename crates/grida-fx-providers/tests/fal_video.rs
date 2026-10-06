//! fal `video.generate`, a queue long job (spec/providers.md §7, §9.3), over synthetic exchanges
//! and a `FakeClock`, so polling takes microseconds. Every test that sends asserts each request;
//! every refusal runs over `NoNetwork`. Clips are the committed facts vectors
//! (`spec/vectors/facts/mp4/`) and `testing::media::MP4_HEAD`.

use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{Answer, CallRequest, Collected, LongJob, Submitted};
use grida_fx_providers::clock::{Clock, FakeClock};
use grida_fx_providers::fal::FalClients;
use grida_fx_providers::fal::video::{
    self, COLLECT_DEADLINE, ClipFacts, FalVideo, OUTSTANDING, POLL, READ_DEADLINE, check_clip,
};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::testing::{
    CallBuilder, TestCall, block_on, media, setup_with_clock, test_keys,
};
use grida_fx_providers::transport::replay::{
    Exchange, Expect, ExpectBody, NoNetwork, ReplayTransport, Reply,
};
use grida_fx_providers::transport::{
    Body, HttpResponse, Lane, Transport, TransportError, TransportErrorKind,
};
use grida_fx_providers::wire::data_url;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const ROUTE: &str = "google/gemini-omni-flash/v1.1/image-to-video@fal";
const SUBMIT_URL: &str = "https://queue.fal.run/google/gemini-omni-flash/v1.1/image-to-video";
const STATUS_URL: &str = "https://queue.fal.run/google/gemini-omni-flash/requests/req-1/status";
const RESPONSE_URL: &str = "https://queue.fal.run/google/gemini-omni-flash/requests/req-1";
const VIDEO_URL: &str = "https://v3b.fal.media/files/clip.mp4";
const KEY: &str = "test-fal-key";
/// 64×48, 24 fps, 2 s (spec/vectors/facts/mp4/expected.json).
const CLIP: &[u8] = include_bytes!("../../../spec/vectors/facts/mp4/h264_24fps.mp4");
/// An MP4 with an audio track only.
const AUDIO_ONLY: &[u8] = include_bytes!("../../../spec/vectors/facts/mp4/audio_only.mp4");

fn handle() -> Value {
    json!({"request_id": "req-1",
           "status_path": "google/gemini-omni-flash/requests/req-1/status",
           "response_path": "google/gemini-omni-flash/requests/req-1"})
}

fn video_with(transport: Arc<dyn Transport>, keys: Keys, clock: Arc<FakeClock>) -> FalVideo {
    let clock: Arc<dyn Clock> = clock;
    FalVideo::new(
        FalClients::new(&setup_with_clock(transport, keys, Arc::clone(&clock))),
        clock,
    )
}

fn video(transport: &Arc<ReplayTransport>, clock: &Arc<FakeClock>) -> FalVideo {
    video_with(transport.clone(), test_keys(), clock.clone())
}

/// A call with a first frame; `request` members replace or add to the defaults (a `null` value
/// removes the member).
fn call(request: Value) -> TestCall {
    let mut builder = CallBuilder::new("video.generate", ROUTE)
        .contract(json!({"adapter": "fal-queue", "adapter_behavior": 1}));
    let first = builder.file("image/png", &media::png(2, 2, Some(255)));
    let mut base = json!({"prompt": "A lantern sways.", "first_frame": first, "duration": 3,
                          "resolution": "360p", "aspect_ratio": "9:16"});
    for (name, value) in request.as_object().unwrap() {
        if value.is_null() {
            base.as_object_mut().unwrap().remove(name);
        } else {
            base[name] = value.clone();
        }
    }
    builder.request(base).build()
}

fn status_get() -> Expect {
    Expect::get(STATUS_URL).credential("authorization", "Key test-fal-key")
}

fn result_get() -> Expect {
    Expect::get(RESPONSE_URL).credential("authorization", "Key test-fal-key")
}

fn ok(value: Value) -> HttpResponse {
    HttpResponse::json(200, &value)
}

fn completed() -> Exchange {
    status_get().reply(ok(json!({"status": "COMPLETED", "request_id": "req-1"})))
}

fn result() -> Exchange {
    result_get().reply(ok(
        json!({"video": {"url": VIDEO_URL, "content_type": "video/mp4"}}),
    ))
}

fn clip_reply() -> HttpResponse {
    HttpResponse::new(200, CLIP.to_vec()).with_header("content-type", "video/mp4")
}

fn collect(
    transport: &Arc<ReplayTransport>,
    clock: &Arc<FakeClock>,
    call: &CallRequest,
) -> Collected {
    let collected = block_on(video(transport, clock).collect(call, &handle()));
    transport.assert_done();
    collected
}

fn submit(transport: &Arc<ReplayTransport>, call: &CallRequest) -> Submitted {
    let submitted = block_on(video(transport, &Arc::new(FakeClock::new())).submit(call));
    transport.assert_done();
    submitted
}

fn refused(call: &CallRequest, keys: Keys) -> String {
    let adapter = video_with(Arc::new(NoNetwork), keys, Arc::new(FakeClock::new()));
    match block_on(adapter.submit(call)) {
        Submitted::Refused { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn ended(collected: Collected) -> String {
    match collected {
        Collected::Ended { reason } => reason,
        other => panic!("expected Ended, got {other:?}"),
    }
}

fn unreachable(collected: Collected) -> String {
    match collected {
        Collected::Unreachable { reason } => reason,
        other => panic!("expected Unreachable, got {other:?}"),
    }
}

// 6. Submit ------------------------------------------------------------------------------------

#[test]
fn the_submit_body_holds_both_frames_and_an_integer_duration() {
    let mut builder = CallBuilder::new("video.generate", ROUTE);
    let first_bytes = media::png(2, 2, Some(255));
    let last_bytes = media::png(3, 3, None);
    let first = builder.file("image/png", &first_bytes);
    let last = builder.file("image/webp", &last_bytes);
    let call = builder
        .request(
            json!({"prompt": "A lantern sways.", "first_frame": first, "last_frame": last,
                        "duration": 5.0, "resolution": "1080p", "aspect_ratio": "16:9"}),
        )
        .build();
    let body = json!({"prompt": "A lantern sways.", "image_url": data_url("image/png", &first_bytes),
                      "end_image_url": data_url("image/webp", &last_bytes),
                      "aspect_ratio": "16:9", "resolution": "1080p", "duration": 5});
    let answer = json!({"request_id": "req-1", "status_url": STATUS_URL, "response_url": RESPONSE_URL,
                        "cancel_url": format!("{RESPONSE_URL}/cancel")});
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::post_json(SUBMIT_URL, Value::Null)
            .credential("authorization", "Key test-fal-key")
            .body(ExpectBody::JsonText(serde_json::to_string(&body).unwrap()))
            .reply(ok(answer)),
    ]));
    assert_eq!(
        submit(&transport, &call),
        Submitted::Accepted { handle: handle() }
    );
    let request = &transport.requests()[0];
    assert_eq!(request.lane, Lane::Provider);
    assert_eq!(request.timeout, video::SUBMIT_DEADLINE);
}

#[test]
fn a_submit_without_last_frame_uses_the_default_tier_and_ratio() {
    let call = call(json!({"resolution": null, "aspect_ratio": null}));
    let first = call.files.values().next().unwrap();
    let first_bytes = std::fs::read(&first.path).unwrap();
    let body = json!({"prompt": "A lantern sways.", "image_url": data_url("image/png", &first_bytes),
                      "aspect_ratio": "9:16", "resolution": "720p", "duration": 3});
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::post_json(SUBMIT_URL, Value::Null)
            .credential("authorization", "Key test-fal-key")
            .body(ExpectBody::JsonText(serde_json::to_string(&body).unwrap()))
            .reply(ok(json!({"request_id": "req-1", "status_url": STATUS_URL,
                             "response_url": RESPONSE_URL}))),
    ]));
    assert_eq!(
        submit(&transport, &call),
        Submitted::Accepted { handle: handle() }
    );

    let explicit_nulls = {
        let mut builder = CallBuilder::new("video.generate", ROUTE);
        let first = builder.file("image/png", &first_bytes);
        builder
            .request(
                json!({"prompt": "A lantern sways.", "first_frame": first, "duration": 3,
                            "resolution": null, "aspect_ratio": null, "last_frame": null}),
            )
            .build()
    };
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::post_json(SUBMIT_URL, body)
            .credential("authorization", "Key test-fal-key")
            .reply(ok(json!({"request_id": "req-1", "status_url": STATUS_URL,
                             "response_url": RESPONSE_URL}))),
    ]));
    assert!(matches!(
        submit(&transport, &explicit_nulls),
        Submitted::Accepted { .. }
    ));
}

#[test]
fn submit_refusals_send_nothing() {
    let unknown = json!({"file": "f".repeat(64)});
    let cases: Vec<(Value, String)> = vec![
        (
            json!({"seed": 7}),
            "video.generate takes no member seed".into(),
        ),
        (json!({"prompt": 7}), "prompt is text".into()),
        (
            json!({"first_frame": "plate.png"}),
            "first_frame is a file".into(),
        ),
        (json!({"duration": "3"}), "duration is a number".into()),
        (json!({"duration": true}), "duration is a number".into()),
        (
            json!({"prompt": "  "}),
            "a clip needs its prompt as text".into(),
        ),
        (
            json!({"prompt": "é".repeat(video::MAX_PROMPT_CHARS + 1)}),
            "the prompt is longer than 20000 characters".into(),
        ),
        (
            json!({"first_frame": null}),
            "this route draws from a first frame".into(),
        ),
        (
            json!({"duration": 12}),
            "this route draws whole seconds from 3 to 10, not 12".into(),
        ),
        (
            json!({"duration": 3.5}),
            "this route draws whole seconds from 3 to 10, not 3.5".into(),
        ),
        (
            json!({"duration": 2}),
            "this route draws whole seconds from 3 to 10, not 2".into(),
        ),
        (
            json!({"duration": null}),
            "this route draws whole seconds from 3 to 10, not null".into(),
        ),
        (
            json!({"resolution": "8k"}),
            "this route draws no 8k clip at 9:16".into(),
        ),
        (
            json!({"aspect_ratio": "4:3"}),
            "this route draws no 360p clip at 4:3".into(),
        ),
        (
            json!({"first_frame": unknown}),
            "first_frame has no bytes to send".into(),
        ),
        (
            json!({"last_frame": unknown}),
            "last_frame has no bytes to send".into(),
        ),
    ];
    for (request, sentence) in cases {
        let call = call(request.clone());
        assert_eq!(refused(&call, test_keys()), sentence, "{request}");
    }
    // A frame that is not a picture is refused, not sent under a picture's label.
    for (member, kind) in [
        ("first_frame", "text/plain"),
        ("first_frame", "model/gltf-binary"),
        ("last_frame", "application/octet-stream"),
        ("last_frame", "audio/mpeg"),
    ] {
        let mut builder = CallBuilder::new("video.generate", ROUTE);
        let first = builder.file("image/png", &media::png(2, 2, Some(255)));
        let other = builder.file(kind, b"First line of a note, or other bytes");
        let mut request = json!({"prompt": "p", "first_frame": first, "duration": 3});
        request[member] = other;
        let call = builder.request(request).build();
        assert_eq!(
            refused(&call, test_keys()),
            format!("{member} is {kind}, not a picture")
        );
    }
    let foreign = CallBuilder::new("video.generate", ROUTE)
        .contract(json!({"adapter": "fal-run"}))
        .request(json!({"prompt": ""}))
        .build();
    assert_eq!(
        refused(&foreign, Keys::none()),
        "google/gemini-omni-flash/v1.1/image-to-video@fal is not a video.generate route this adapter serves"
    );
    assert_eq!(
        refused(&call(json!({"prompt": ""})), Keys::none()),
        "FAL_KEY is not set"
    );
    assert_eq!(
        refused(&call(json!({"prompt": "", "seed": 1})), Keys::none()),
        "video.generate takes no member seed"
    );
    assert_eq!(
        refused(
            &call(json!({"duration": 12, "first_frame": unknown})),
            test_keys()
        ),
        "this route draws whole seconds from 3 to 10, not 12"
    );
}

#[test]
fn submit_statuses_follow_the_queue_table() {
    let accepted = json!({"request_id": "req-1", "status_url": STATUS_URL,
                          "response_url": RESPONSE_URL});
    let foreign = json!({"request_id": "req-1",
                         "status_url": "https://queue.fal.run.evil.example/x/requests/req-1/status",
                         "response_url": RESPONSE_URL});
    let queried = json!({"request_id": "req-1", "status_url": format!("{STATUS_URL}?logs=1"),
                         "response_url": RESPONSE_URL});
    let signed_query = json!({"request_id": "req-1",
                              "status_url": format!("{STATUS_URL}?X-Amz-Signature=deadbeef"),
                              "response_url": RESPONSE_URL});
    let missing = json!({"request_id": "req-1", "status_url": STATUS_URL});
    let blank = json!({"request_id": " ", "status_url": STATUS_URL, "response_url": RESPONSE_URL});
    let unsafe_id = json!({"request_id": "https://v3b.fal.media/x?X-Amz-Signature=deadbeef",
                           "status_url": STATUS_URL, "response_url": RESPONSE_URL});
    let detail = br#"{"detail": [{"ctx": {"le": 10}}]}"#.to_vec();
    let not_received = |reason: &str| Submitted::not_received(reason);
    let waiting = |reason: &str, seconds: u64| Submitted::NotReceived {
        reason: reason.into(),
        retry_after: Some(Duration::from_secs(seconds)),
    };
    let uncertain = |reason: &str| Submitted::Uncertain {
        reason: reason.into(),
    };
    let refused_free = |status: u16| Submitted::Failed {
        reason: format!("fal video submission returned HTTP {status}"),
        cost: Some(Usd::ZERO),
        retryable: false,
    };
    let no_handle = "fal took the video job but returned no handle to collect it by";
    let outside = "a fal video job handle points outside fal's queue";
    let post = || {
        Expect::post_json(SUBMIT_URL, Value::Null)
            .credential("authorization", "Key test-fal-key")
            .body(ExpectBody::Any)
    };
    let cases: Vec<(Exchange, Submitted)> = vec![
        (
            post().reply(ok(accepted)),
            Submitted::Accepted { handle: handle() },
        ),
        (
            post().fail(TransportError::not_sent(
                TransportErrorKind::Connect,
                "connection refused",
            )),
            not_received("fal video submission was not sent: connection refused"),
        ),
        (
            post().fail(TransportError::not_sent(
                TransportErrorKind::Refused,
                "the network is off",
            )),
            Submitted::Refused {
                reason: "fal video submission was not sent: the network is off".into(),
            },
        ),
        (
            post().fail(TransportError::after_send(
                TransportErrorKind::Timeout,
                "read timed out",
            )),
            uncertain(
                "fal video submission ended without an answer; it may have been taken: read timed out",
            ),
        ),
        (
            post().reply(HttpResponse::new(200, b"<html>ok</html>".to_vec())),
            uncertain(no_handle),
        ),
        (post().reply(ok(json!([1]))), uncertain(no_handle)),
        // fal took the job: the reason names the request id it returned, when that is safe.
        (
            post().reply(ok(missing)),
            uncertain(&format!("{no_handle} (request req-1)")),
        ),
        (
            post().reply(ok(foreign)),
            uncertain(&format!("{outside} (request req-1)")),
        ),
        (
            post().reply(ok(signed_query)),
            uncertain(&format!("{outside} (request req-1)")),
        ),
        (post().reply(ok(blank)), uncertain(no_handle)),
        (post().reply(ok(unsafe_id.clone())), uncertain(no_handle)),
        (
            post().reply(ok(unsafe_id).with_header("x-request-id", "req_hdr")),
            uncertain(&format!("{no_handle} (request req_hdr)")),
        ),
        (
            post().reply(ok(queried)),
            Submitted::Accepted {
                handle: json!({"request_id": "req-1",
                               "status_path": "google/gemini-omni-flash/requests/req-1/status?logs=1",
                               "response_path": "google/gemini-omni-flash/requests/req-1"}),
            },
        ),
        (
            post().reply(
                HttpResponse::new(302, Vec::new()).with_header("location", "https://x.test"),
            ),
            uncertain("fal video submission was redirected (HTTP 302)"),
        ),
        (
            post().reply(HttpResponse::new(400, detail.clone())),
            refused_free(400),
        ),
        (
            post().reply(HttpResponse::new(401, detail.clone())),
            refused_free(401),
        ),
        (
            post().reply(HttpResponse::new(402, detail.clone())),
            refused_free(402),
        ),
        (
            post().reply(HttpResponse::new(422, detail.clone())),
            refused_free(422),
        ),
        (
            post()
                .reply(HttpResponse::new(422, detail.clone()).with_header("x-request-id", "req_9")),
            Submitted::Failed {
                reason: "fal video submission returned HTTP 422 (request req_9)".into(),
                cost: Some(Usd::ZERO),
                retryable: false,
            },
        ),
        (
            post().reply(HttpResponse::new(408, Vec::new())),
            not_received("fal video submission returned HTTP 408"),
        ),
        (
            post().reply(HttpResponse::new(429, Vec::new()).with_header("retry-after", "20")),
            waiting(
                "fal video submission was rate limited (HTTP 429); retry-after 20 s",
                20,
            ),
        ),
        (
            post().reply(HttpResponse::new(500, Vec::new())),
            not_received("fal video submission returned HTTP 500"),
        ),
        (
            post().reply(HttpResponse::new(502, Vec::new())),
            not_received("fal video submission returned HTTP 502"),
        ),
        (
            post().reply(HttpResponse::new(503, Vec::new())),
            not_received("fal video submission returned HTTP 503"),
        ),
        (
            post().reply(HttpResponse::new(504, Vec::new())),
            uncertain("fal video submission returned HTTP 504"),
        ),
        (
            post().reply(HttpResponse::new(501, Vec::new())),
            uncertain("fal video submission returned HTTP 501"),
        ),
    ];
    for (exchange, expected) in cases {
        let transport = Arc::new(ReplayTransport::new(vec![exchange]));
        assert_eq!(submit(&transport, &call(json!({}))), expected);
        assert_eq!(transport.requests().len(), 1);
    }
}

// 7. Collect -----------------------------------------------------------------------------------

#[test]
fn collect_polls_through_a_dropped_read_then_downloads_without_credential() {
    let clock = Arc::new(FakeClock::new());
    let transport = Arc::new(ReplayTransport::new(vec![
        status_get().fail(TransportError::after_send(
            TransportErrorKind::Other,
            "connection reset",
        )),
        status_get().reply(ok(json!({"status": "IN_QUEUE", "request_id": "req-1"}))),
        status_get().reply(ok(json!({"status": "IN_PROGRESS"}))),
        completed(),
        result_get().reply(ok(
            json!({"video": {"url": VIDEO_URL, "content_type": "video/mp4"},
                                     "usage": {"cost": 0.3}}),
        )),
        Expect::download(VIDEO_URL).reply(clip_reply()),
    ]));
    let call = call(json!({}));
    let Collected::Answered(answer) = collect(&transport, &clock, &call) else {
        panic!("an answer")
    };
    assert_eq!(
        answer.data,
        json!({"facts": {"width": 64, "height": 48, "duration_seconds": 2, "fps": 24}})
    );
    assert_eq!(answer.cost, Some(Usd(300_000)));
    assert_eq!(answer.files["video"].kind, "video/mp4");
    assert_eq!(answer.files["video"].bytes, CLIP);
    assert_eq!(clock.sleeps(), vec![POLL; 3]);
    let requests = transport.requests();
    for read in &requests[..5] {
        assert_eq!(read.lane, Lane::Provider);
        assert_eq!(read.timeout, READ_DEADLINE);
        assert!(read.credential.is_some());
    }
    let download = &requests[5];
    assert_eq!(download.lane, Lane::Download);
    assert!(download.credential.is_none());
    assert!(download.headers.is_empty(), "{:?}", download.headers);
    assert_eq!(download.max_response_bytes, video::MAX_VIDEO_BYTES);
    assert_eq!(download.timeout, COLLECT_DEADLINE - POLL * 3);
}

#[test]
fn a_fixture_plays_the_same_collect() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fal/video_collect.json");
    let transport = Arc::new(ReplayTransport::from_fixture(&path));
    let clock = Arc::new(FakeClock::new());
    let call = CallBuilder::new("video.generate", "acme/clip-model@fal")
        .request(json!({"prompt": "p", "first_frame": {"file": "f".repeat(64)}, "duration": 3}))
        .build();
    let handle = json!({"request_id": "req-1", "status_path": "acme/clip-model/requests/req-1/status",
                        "response_path": "acme/clip-model/requests/req-1"});
    let collected = block_on(video(&transport, &clock).collect(&call, &handle));
    transport.assert_done();
    let Collected::Answered(answer) = collected else {
        panic!("an answer")
    };
    assert_eq!(answer.cost, Some(Usd(300_000)));
    assert_eq!(answer.data["facts"]["fps"], json!(24));
}

#[test]
fn a_failed_job_ends_with_its_cut_error() {
    let clock = Arc::new(FakeClock::new());
    let transport = Arc::new(ReplayTransport::new(vec![status_get().reply(ok(json!({
        "status": "COMPLETED", "error": "boom ".repeat(200), "error_type": "model_error"
    })))]));
    let reason = ended(collect(&transport, &clock, &call(json!({}))));
    assert!(
        reason.starts_with("fal video job failed (model_error): boom boom"),
        "{reason}"
    );
    assert!(reason.chars().count() <= 500);
    let transport = Arc::new(ReplayTransport::new(vec![
        status_get().reply(ok(json!({"status": "IN_PROGRESS", "error": null}))),
        status_get().reply(ok(json!({"status": "FAILED", "error": {"message": "x"}}))),
    ]));
    assert_eq!(
        ended(collect(&transport, &clock, &call(json!({})))),
        r#"fal video job failed: {"message":"x"}"#
    );
}

#[test]
fn a_job_still_running_at_the_deadline_is_left_to_the_next_run() {
    let clock = Arc::new(FakeClock::scripted(&[
        Duration::ZERO,
        Duration::ZERO,
        COLLECT_DEADLINE - Duration::from_secs(1),
    ]));
    let transport = Arc::new(ReplayTransport::new(vec![
        status_get().reply(ok(json!({"status": "IN_PROGRESS"}))),
    ]));
    assert_eq!(
        unreachable(collect(&transport, &clock, &call(json!({})))),
        OUTSTANDING
    );
    assert!(clock.sleeps().is_empty());
}

#[test]
fn failed_reads_keep_polling_until_the_deadline() {
    let clock = Arc::new(FakeClock::new());
    // Reads at 0, 5, …, 1495 s; the wait after the last one reaches 1500 s, where no time is left
    // for another read.
    let reads = (COLLECT_DEADLINE.as_secs() / POLL.as_secs()) as usize;
    let exchanges: Vec<Exchange> = (0..reads)
        .map(|i| match i % 4 {
            0 => status_get().reply(HttpResponse::new(503, Vec::new())),
            1 => status_get().reply(HttpResponse::new(429, Vec::new())),
            2 => status_get().reply(HttpResponse::new(200, b"<html>busy</html>".to_vec())),
            _ => status_get().fail(TransportError::not_sent(
                TransportErrorKind::Connect,
                "refused",
            )),
        })
        .collect();
    let transport = Arc::new(ReplayTransport::new(exchanges));
    assert_eq!(
        unreachable(collect(&transport, &clock, &call(json!({})))),
        OUTSTANDING
    );
    assert_eq!(clock.sleeps().len(), reads);
    let timeouts: Vec<Duration> = transport.requests().iter().map(|r| r.timeout).collect();
    assert_eq!(timeouts[0], READ_DEADLINE);
    assert_eq!(*timeouts.last().unwrap(), POLL);
}

#[test]
fn the_last_reads_are_bounded_by_the_time_left() {
    let clock = Arc::new(FakeClock::scripted(&[
        Duration::ZERO,
        COLLECT_DEADLINE - Duration::from_secs(20),
        COLLECT_DEADLINE - Duration::from_secs(1),
    ]));
    let transport = Arc::new(ReplayTransport::new(vec![
        status_get().reply(HttpResponse::new(500, Vec::new())),
    ]));
    let collected = block_on(video(&transport, &clock).collect(&call(json!({})), &handle()));
    assert_eq!(unreachable(collected), OUTSTANDING);
    assert_eq!(transport.requests()[0].timeout, Duration::from_secs(20));
}

#[test]
fn status_reads_that_end_or_stop_collecting() {
    let cases: Vec<(HttpResponse, Result<&str, &str>)> = vec![
        (
            HttpResponse::new(401, Vec::new()),
            Err("fal video job status returned HTTP 401"),
        ),
        (
            HttpResponse::new(403, Vec::new()),
            Err("fal video job status returned HTTP 403"),
        ),
        (
            HttpResponse::new(302, Vec::new()).with_header("location", "https://x.test"),
            Err("fal video job status returned HTTP 302"),
        ),
        (
            HttpResponse::new(404, Vec::new()),
            Ok("fal video job status returned HTTP 404"),
        ),
        (
            HttpResponse::new(410, Vec::new()),
            Ok("fal video job status returned HTTP 410"),
        ),
        (
            HttpResponse::new(422, br#"{"detail": "secret body"}"#.to_vec()),
            Ok("fal video job status returned HTTP 422"),
        ),
        (
            ok(json!({"status": "IN_PROGRESS", "request_id": "req-2"})),
            Err("fal video job status answered about another job"),
        ),
    ];
    for (response, expected) in cases {
        let clock = Arc::new(FakeClock::new());
        let transport = Arc::new(ReplayTransport::new(vec![status_get().reply(response)]));
        let collected = collect(&transport, &clock, &call(json!({})));
        match expected {
            Ok(sentence) => assert_eq!(ended(collected), sentence),
            Err(sentence) => assert_eq!(unreachable(collected), sentence),
        }
    }
    let clock = Arc::new(FakeClock::new());
    let transport = Arc::new(ReplayTransport::new(vec![status_get().fail(
        TransportError::not_sent(TransportErrorKind::Refused, "the network is off"),
    )]));
    assert_eq!(
        unreachable(collect(&transport, &clock, &call(json!({})))),
        "fal video job status was not sent: the network is off"
    );
}

#[test]
fn a_408_or_non_json_status_is_read_again() {
    let clock = Arc::new(FakeClock::new());
    let transport = Arc::new(ReplayTransport::new(vec![
        status_get().reply(HttpResponse::new(408, Vec::new())),
        status_get().reply(HttpResponse::new(200, b"not json".to_vec())),
        completed(),
        result(),
        Expect::download(VIDEO_URL).reply(clip_reply()),
    ]));
    assert!(matches!(
        collect(&transport, &clock, &call(json!({}))),
        Collected::Answered(_)
    ));
    assert_eq!(clock.sleeps().len(), 2);
}

#[test]
fn result_reads() {
    let clock = Arc::new(FakeClock::new());
    let transport = Arc::new(ReplayTransport::new(vec![
        completed(),
        result_get().reply(HttpResponse::new(502, Vec::new())),
        result_get().reply(HttpResponse::new(200, b"<html/>".to_vec())),
        result_get().reply(ok(json!({"data": {"video": {"url": VIDEO_URL}}}))),
        Expect::download(VIDEO_URL).reply(clip_reply()),
    ]));
    let Collected::Answered(answer) = collect(&transport, &clock, &call(json!({}))) else {
        panic!("an answer")
    };
    assert_eq!(answer.cost, None);
    assert_eq!(clock.sleeps().len(), 2);

    let cases: Vec<(HttpResponse, Result<&str, &str>)> = vec![
        (
            ok(json!({"images": []})),
            Ok("fal video job result carries no video"),
        ),
        (
            ok(json!({"video": {}})),
            Ok("fal output video url must be non-empty"),
        ),
        (
            ok(json!({"video": {"url": "http://v3b.fal.media/files/clip.mp4"}})),
            Ok("fal output video url must be https"),
        ),
        (
            ok(json!({"video": {"url": "https://user:pw@v3b.fal.media/clip.mp4"}})),
            Ok("fal output video url must be https"),
        ),
        (
            HttpResponse::new(404, Vec::new()),
            Ok("fal video job result returned HTTP 404"),
        ),
        (
            HttpResponse::new(401, Vec::new()),
            Err("fal video job result returned HTTP 401"),
        ),
    ];
    for (response, expected) in cases {
        let clock = Arc::new(FakeClock::new());
        let transport = Arc::new(ReplayTransport::new(vec![
            completed(),
            result_get().reply(response),
        ]));
        let collected = collect(&transport, &clock, &call(json!({})));
        match expected {
            Ok(sentence) => assert_eq!(ended(collected), sentence),
            Err(sentence) => assert_eq!(unreachable(collected), sentence),
        }
    }
}

#[test]
fn downloads_that_end_or_stop_collecting() {
    let not_mp4 = b"\x1a\x45\xdf\xa3 a webm head".to_vec();
    let cases: Vec<(Exchange, Result<String, &str>)> = vec![
        (
            Expect::download(VIDEO_URL).reply(
                HttpResponse::new(302, Vec::new()).with_header("location", "https://x.test/a.mp4"),
            ),
            Err("fal output video download returned HTTP 302"),
        ),
        (
            Expect::download(VIDEO_URL).reply(HttpResponse::new(403, Vec::new())),
            Err("fal output video download returned HTTP 403"),
        ),
        (
            Expect::download(VIDEO_URL).reply(HttpResponse::new(410, Vec::new())),
            Ok("fal output video download returned HTTP 410".into()),
        ),
        (
            Expect::download(VIDEO_URL).reply(
                HttpResponse::new(200, b"\x1a\x45\xdf\xa3 webm".to_vec())
                    .with_header("content-type", "video/mp4"),
            ),
            Ok("fal output video is not an MP4 file".into()),
        ),
        (
            Exchange {
                expect: Expect::download(VIDEO_URL),
                reply: Reply::Zeros {
                    status: 200,
                    headers: vec![("content-type".into(), "video/mp4".into())],
                    len: video::MAX_VIDEO_BYTES + 1,
                },
            },
            Ok(format!(
                "fal output video download failed: the response is larger than {} bytes",
                video::MAX_VIDEO_BYTES
            )),
        ),
        (
            Expect::download(VIDEO_URL)
                .reply(HttpResponse::new(200, not_mp4).with_header("content-type", "video/mp4")),
            Ok("fal output video is not an MP4 file".into()),
        ),
        (
            Expect::download(VIDEO_URL).reply(HttpResponse::new(200, Vec::new())),
            Ok("fal output video download was empty".into()),
        ),
        (
            Expect::download(VIDEO_URL).reply(
                HttpResponse::new(200, AUDIO_ONLY.to_vec())
                    .with_header("content-type", "video/mp4"),
            ),
            Ok("the file carries no video stream".into()),
        ),
        (
            Expect::download(VIDEO_URL).fail(TransportError::not_sent(
                TransportErrorKind::Refused,
                "the network is off",
            )),
            Err("fal output video download was not sent: the network is off"),
        ),
    ];
    for (download, expected) in cases {
        let clock = Arc::new(FakeClock::new());
        let transport = Arc::new(ReplayTransport::new(vec![completed(), result(), download]));
        let collected = collect(&transport, &clock, &call(json!({})));
        match expected {
            Ok(sentence) => assert_eq!(ended(collected), sentence),
            Err(sentence) => assert_eq!(unreachable(collected), sentence),
        }
    }

    // The bytes start like an MP4 but carry no movie.
    let clock = Arc::new(FakeClock::new());
    let transport = Arc::new(ReplayTransport::new(vec![
        completed(),
        result(),
        Expect::download(VIDEO_URL).reply(
            HttpResponse::new(200, media::MP4_HEAD.to_vec()).with_header("content-type", "mp4"),
        ),
    ]));
    let reason = ended(collect(&transport, &clock, &call(json!({}))));
    assert!(
        reason.starts_with("the clip cannot be read: not an MP4 file"),
        "{reason}"
    );
}

#[test]
fn the_bytes_decide_the_kind_not_the_headers() {
    // A valid MP4 is answered whatever its content-type header or declared type says: §9.3 ends
    // a job only when the bytes are not MP4.
    let results = [
        json!({"video": {"url": VIDEO_URL}}),
        json!({"video": {"url": VIDEO_URL, "content_type": "video/webm"}}),
        json!({"video": {"url": VIDEO_URL, "content_type": "application/octet-stream"}}),
        json!({"video": {"url": VIDEO_URL, "content_type": 7}}),
    ];
    let headers = [
        None,
        Some("application/octet-stream"),
        Some("binary/octet-stream"),
        Some("video/quicktime"),
        Some("text/html"),
        Some(""),
    ];
    for result in &results {
        for header in headers {
            let mut reply = HttpResponse::new(200, CLIP.to_vec());
            if let Some(header) = header {
                reply = reply.with_header("content-type", header);
            }
            let clock = Arc::new(FakeClock::new());
            let transport = Arc::new(ReplayTransport::new(vec![
                completed(),
                result_get().reply(ok(result.clone())),
                Expect::download(VIDEO_URL).reply(reply),
            ]));
            let Collected::Answered(answer) = collect(&transport, &clock, &call(json!({}))) else {
                panic!("an answer for {result} with {header:?}")
            };
            assert_eq!(answer.files["video"].kind, "video/mp4");
            assert_eq!(answer.files["video"].bytes, CLIP);
        }
    }
}

#[test]
fn a_failed_download_is_unreachable_and_made_once() {
    // spec/providers.md §8: a result file is fetched once per collect. A failure a later collect
    // may not repeat leaves the job `submitted`; the next collect reads the result again.
    let cases: Vec<(Exchange, &str)> = vec![
        (
            Expect::download(VIDEO_URL).reply(HttpResponse::new(503, Vec::new())),
            "fal output video download returned HTTP 503",
        ),
        (
            Expect::download(VIDEO_URL).reply(HttpResponse::new(502, Vec::new())),
            "fal output video download returned HTTP 502",
        ),
        (
            Expect::download(VIDEO_URL).fail(TransportError::after_send(
                TransportErrorKind::Other,
                "connection reset",
            )),
            "fal output video download failed: connection reset",
        ),
        (
            Expect::download(VIDEO_URL).fail(TransportError::not_sent(
                TransportErrorKind::Connect,
                "connection refused",
            )),
            "fal output video download failed: connection refused",
        ),
        (
            Expect::download(VIDEO_URL).reply(HttpResponse::new(429, Vec::new())),
            "fal output video download returned HTTP 429",
        ),
        (
            Expect::download(VIDEO_URL).reply(HttpResponse::new(408, Vec::new())),
            "fal output video download returned HTTP 408",
        ),
        (
            Expect::download(VIDEO_URL).reply(HttpResponse::new(101, Vec::new())),
            "fal output video download returned HTTP 101",
        ),
    ];
    for (download, sentence) in cases {
        let clock = Arc::new(FakeClock::new());
        let transport = Arc::new(ReplayTransport::new(vec![completed(), result(), download]));
        assert_eq!(
            unreachable(collect(&transport, &clock, &call(json!({})))),
            sentence
        );
        let downloads = transport
            .requests()
            .iter()
            .filter(|request| request.lane == Lane::Download)
            .count();
        assert_eq!(downloads, 1, "{sentence}");
        assert!(clock.sleeps().is_empty(), "{sentence}");
    }
}

#[test]
fn a_video_url_is_fetched_byte_for_byte_and_never_quoted() {
    let signed = "https://v3b.fal.media/files/clip.mp4?k='x'&X-Amz-Signature=deadbeef";
    let clock = Arc::new(FakeClock::new());
    let transport = Arc::new(ReplayTransport::new(vec![
        completed(),
        result_get().reply(ok(json!({"video": {"url": signed}}))),
        Expect::download(signed).reply(HttpResponse::new(200, CLIP.to_vec())),
    ]));
    assert!(matches!(
        collect(&transport, &clock, &call(json!({}))),
        Collected::Answered(_)
    ));
    assert_eq!(transport.requests()[2].url, signed);

    // A failure's reason names neither the URL nor its parsed form.
    let clock = Arc::new(FakeClock::new());
    let transport = Arc::new(ReplayTransport::new(vec![
        completed(),
        result_get().reply(ok(json!({"video": {"url": signed}}))),
        Expect::download(signed).fail(TransportError::after_send(
            TransportErrorKind::Other,
            "error sending request for url (https://v3b.fal.media/files/clip.mp4?k=%27x%27&X-Amz-Signature=deadbeef)",
        )),
    ]));
    let reason = unreachable(collect(&transport, &clock, &call(json!({}))));
    assert!(!reason.contains("deadbeef"), "{reason}");
    assert!(!reason.contains("v3b.fal.media/files"), "{reason}");
}

#[test]
fn a_queue_reference_with_a_plain_query_is_collected() {
    let clock = Arc::new(FakeClock::new());
    let status = format!("{STATUS_URL}?logs=1");
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::get(&status)
            .credential("authorization", "Key test-fal-key")
            .reply(ok(json!({"status": "COMPLETED", "request_id": "req-1"}))),
        result(),
        Expect::download(VIDEO_URL).reply(clip_reply()),
    ]));
    let mut queried = handle();
    queried["status_path"] = json!("google/gemini-omni-flash/requests/req-1/status?logs=1");
    let collected = block_on(video(&transport, &clock).collect(&call(json!({})), &queried));
    transport.assert_done();
    assert!(matches!(collected, Collected::Answered(_)));
}

#[test]
fn a_key_straddling_the_cut_of_a_job_error_is_redacted_whole() {
    let clock = Arc::new(FakeClock::new());
    let error = format!("x{}yyyyyyyyy{KEY}", " ".repeat(470));
    let error_type = format!("{}ab{KEY}", "ab. ".repeat(22));
    let transport = Arc::new(ReplayTransport::new(vec![
        status_get().reply(ok(
            json!({"status": "FAILED", "request_id": "req-1", "error": error}),
        )),
        status_get().reply(ok(
            json!({"status": "FAILED", "request_id": "req-1", "error": "boom",
                                     "error_type": error_type}),
        )),
    ]));
    let adapter = video(&transport, &clock);
    let call = call(json!({}));
    let first = ended(block_on(adapter.collect(&call, &handle())));
    assert_eq!(first, "fal video job failed: x yyyyyyyyy[redacted]");
    let second = ended(block_on(adapter.collect(&call, &handle())));
    assert!(!second.contains(&KEY[..6]), "{second}");
    assert!(second.contains("ab[redacted]"), "{second}");
    transport.assert_done();
}

#[test]
fn handles_this_adapter_did_not_write_request_nothing() {
    let path = |status: &str| {
        json!({"request_id": "req-1", "status_path": status,
               "response_path": "google/gemini-omni-flash/requests/req-1"})
    };
    let mut extra = handle();
    extra["status_url"] = json!(STATUS_URL);
    let bad = [
        json!(null),
        json!("req-1"),
        json!({"request_id": "req-1", "status_url": STATUS_URL, "response_url": RESPONSE_URL}),
        json!({"request_id": "", "status_path": "a/status", "response_path": "a"}),
        json!({"status_path": "a/status", "response_path": "a"}),
        extra,
        path(""),
        path("/google/gemini-omni-flash/requests/req-1/status"),
        path(STATUS_URL),
        path("google/../../admin"),
        path("google/requests/req-1/status?token=abc"),
        path("google/requests/req-1/status?x=%2F"),
        path("google/requests/req-1/status#f"),
        path("google/requests/req-1/status?x=1#f"),
        json!({"request_id": "https://v3b.fal.media/x?sig=1", "status_path": "a/status",
               "response_path": "a"}),
        json!({"request_id": "req 1", "status_path": "a/status", "response_path": "a"}),
        path("google\\requests\\req-1"),
        path("google/%2e%2e/admin"),
    ];
    let adapter = video_with(Arc::new(NoNetwork), test_keys(), Arc::new(FakeClock::new()));
    for handle in bad {
        let collected = block_on(adapter.collect(&call(json!({})), &handle));
        assert_eq!(
            unreachable(collected),
            "a fal video job handle is not one this adapter wrote",
            "{handle}"
        );
    }
    let keyless = video_with(
        Arc::new(NoNetwork),
        Keys::none(),
        Arc::new(FakeClock::new()),
    );
    assert_eq!(
        unreachable(block_on(keyless.collect(&call(json!({})), &handle()))),
        "FAL_KEY is not set"
    );
    let foreign = CallBuilder::new("video.generate", ROUTE)
        .contract(json!({"adapter": "fal-run"}))
        .request(json!({"prompt": "p"}))
        .build();
    assert!(
        unreachable(block_on(adapter.collect(&foreign, &handle())))
            .contains("is not a video.generate route")
    );
}

// 8. Check -------------------------------------------------------------------------------------

#[test]
fn the_check_compares_size_and_duration() {
    let facts = |width, height, duration_seconds, fps| ClipFacts {
        width,
        height,
        duration_seconds,
        fps,
    };
    assert_eq!(
        check_clip(&facts(360, 640, 3.0, 24.0), "360p", "9:16", 3.0),
        Ok(())
    );
    assert_eq!(
        check_clip(&facts(360, 640, 3.04, 24.0), "360p", "9:16", 3.0),
        Ok(())
    );
    assert_eq!(
        check_clip(&facts(1280, 720, 7.95, 24.0), "720p", "16:9", 8.0),
        Ok(())
    );
    assert_eq!(
        check_clip(&facts(360, 640, 3.06, 24.0), "360p", "9:16", 3.0).unwrap_err(),
        "the clip runs 3.060 s, not 3 s"
    );
    assert_eq!(
        check_clip(&facts(360, 640, 3.125, 8.0), "360p", "9:16", 3.0),
        Ok(())
    );
    assert_eq!(
        check_clip(&facts(360, 640, 3.25, 8.0), "360p", "9:16", 3.0).unwrap_err(),
        "the clip runs 3.250 s, not 3 s"
    );
    assert_eq!(
        check_clip(&facts(640, 360, 3.0, 24.0), "360p", "9:16", 3.0).unwrap_err(),
        "the clip is 640x360, not 360x640"
    );
    assert_eq!(
        check_clip(&facts(2160, 3840, 10.0, 30.0), "4k", "9:16", 10.0),
        Ok(())
    );
    assert_eq!(ClipFacts::of_mp4(CLIP), Ok(facts(64, 48, 2.0, 24.0)));
    assert_eq!(
        ClipFacts::of_mp4(AUDIO_ONLY).unwrap_err(),
        "the file carries no video stream"
    );
}

#[test]
fn a_collected_clip_of_the_wrong_size_is_refused_by_the_check() {
    let clock = Arc::new(FakeClock::new());
    let transport = Arc::new(ReplayTransport::new(vec![
        completed(),
        result(),
        Expect::download(VIDEO_URL).reply(clip_reply()),
    ]));
    let call = call(json!({}));
    let Collected::Answered(answer) = collect(&transport, &clock, &call) else {
        panic!("an answer")
    };
    let adapter = video(&transport, &clock);
    assert_eq!(
        adapter.check(&call, &answer).unwrap_err(),
        "the clip is 64x48, not 360x640"
    );
    assert_eq!(
        adapter
            .check(&call, &Answer::new(answer.data.clone(), None))
            .unwrap_err(),
        "the answer carries no video"
    );
}

// 11. Secrets ----------------------------------------------------------------------------------

#[test]
fn the_key_travels_only_on_queue_requests() {
    let clock = Arc::new(FakeClock::new());
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::post_json(SUBMIT_URL, Value::Null)
            .credential("authorization", "Key test-fal-key")
            .body(ExpectBody::Any)
            .reply(ok(json!({"request_id": "req-1", "status_url": STATUS_URL,
                             "response_url": RESPONSE_URL}))),
        status_get().reply(HttpResponse::new(500, format!("echo {KEY}").into_bytes())),
        completed(),
        result(),
        Expect::download(VIDEO_URL).reply(clip_reply()),
        status_get().reply(ok(
            json!({"status": "COMPLETED", "error": format!("bad key {KEY}")}),
        )),
    ]));
    let adapter = video(&transport, &clock);
    let call = call(json!({}));
    let Submitted::Accepted { handle } = block_on(adapter.submit(&call)) else {
        panic!("accepted")
    };
    assert!(!handle.to_string().contains(KEY));
    assert!(!handle.to_string().contains("queue.fal.run"));
    let Collected::Answered(answer) = block_on(adapter.collect(&call, &handle)) else {
        panic!("an answer")
    };
    assert!(!answer.data.to_string().contains(KEY));
    let reason = ended(block_on(adapter.collect(&call, &handle)));
    assert_eq!(reason, "fal video job failed: bad key [redacted]");
    transport.assert_done();
    for request in transport.requests() {
        assert!(!request.url.contains(KEY));
        assert!(!format!("{request:?}").contains(KEY));
        if let Body::Json(body) = &request.body {
            assert!(!body.to_string().contains(KEY));
        }
        if request.url.starts_with("https://queue.fal.run/") {
            assert_eq!(
                request.credential.as_ref().unwrap().header_value(),
                format!("Key {KEY}")
            );
        } else {
            assert_eq!(request.lane, Lane::Download);
            assert!(request.credential.is_none());
            assert!(request.headers.is_empty());
        }
    }
}
