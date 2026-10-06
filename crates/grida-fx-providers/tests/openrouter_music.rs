//! OpenRouter music (spec/providers.md §9.2; spec/capabilities.md §9), offline: every exchange is
//! scripted on a `ReplayTransport` that asserts each request, and every refusal runs over
//! `NoNetwork`, which panics on any send. Audio is `testing::media::MP3` followed by
//! `testing::media::MP3_FRAME` (an ID3 tag, then an MPEG frame sync), here and in the fixture
//! `fixtures/openrouter/music/stream-two-chunks.json`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{Answer, RequestAdapter, Sent};
use grida_fx_providers::keys::Keys;
use grida_fx_providers::openrouter::{self, music::OpenRouterMusic};
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

const ROUTE: &str = "google/lyria-3-pro-preview@openrouter";
const MODEL: &str = "google/lyria-3-pro-preview";
const URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const KEY: &str = "test-openrouter-key";

fn mp3() -> Vec<u8> {
    [media::MP3, media::MP3_FRAME].concat()
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn call(request: Value) -> TestCall {
    CallBuilder::new("music.generate", ROUTE)
        .contract(json!({"adapter": "openrouter-music", "adapter_behavior": 1}))
        .request(request)
        .build()
}

fn adapter_over(transport: Arc<dyn Transport>, keys: Keys) -> OpenRouterMusic {
    OpenRouterMusic::new(openrouter::client(&setup(transport, keys)))
}

/// The body every music call sends for `prompt`, members in order.
fn body(prompt: &str) -> Value {
    json!({
        "model": MODEL,
        "messages": [{"role": "user", "content": prompt}],
        "modalities": ["text", "audio"],
        "audio": {"format": "mp3"},
        "stream": true,
    })
}

/// Sends `call` over one scripted exchange answering `reply`; the request must carry exactly the
/// body for `prompt` and be the only send.
fn send_one(call: &TestCall, prompt: &str, reply: Reply) -> Sent {
    let transport = Arc::new(ReplayTransport::new(vec![Exchange {
        expect: Expect::new(Method::Post, URL, Lane::Provider)
            .credential("authorization", &format!("Bearer {KEY}"))
            .body(ExpectBody::JsonText(
                serde_json::to_string(&body(prompt)).unwrap(),
            )),
        reply,
    }]));
    let sent = block_on(adapter_over(transport.clone(), test_keys()).send(call));
    transport.assert_done();
    assert_eq!(transport.requests().len(), 1, "one send, never a retry");
    sent
}

/// One 200 answer of `body`.
fn answer_with(body: &str, content_type: Option<&str>) -> Sent {
    let mut response = HttpResponse::new(200, body.as_bytes().to_vec());
    if let Some(content_type) = content_type {
        response = response.with_header("content-type", content_type);
    }
    send_one(
        &call(json!({"prompt": "p"})),
        "p",
        Reply::Response(response),
    )
}

/// An SSE text of these events, each `data: <json>` and a blank line, then `data: [DONE]`.
fn stream(events: &[Value]) -> String {
    let mut text: String = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect();
    text.push_str("data: [DONE]\n\n");
    text
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

fn refused(call: &TestCall, keys: Keys) -> String {
    match block_on(adapter_over(Arc::new(NoNetwork), keys).send(call)) {
        Sent::Refused { reason } => reason,
        other => panic!("expected Refused, got {other:?}"),
    }
}

// M1 -------------------------------------------------------------------------------------------

#[test]
fn a_stream_is_assembled_from_its_chunks_and_costed_from_its_last_usage() {
    let encoded = b64(&mp3());
    let cut = (encoded.len() / 2) / 4 * 4;
    let first = json!({"choices": [{"delta": {"role": "assistant",
        "audio": {"data": &encoded[..cut], "format": "mp3"}}}], "usage": {"cost": 0.9}});
    let last = json!({"choices": [{"delta": {"content": "a calm theme",
        "audio": {"data": &encoded[cut..]}}}], "usage": {"prompt_tokens": 12, "cost": 0.0512345}});
    let text = format!(
        ": OPENROUTER PROCESSING\n\ndata: {first}\n\n: OPENROUTER PROCESSING\n\n\
         data: {last}\n\ndata: [DONE]\n\n"
    );
    let call = call(json!({"prompt": "a calm harbour theme", "duration": 30}));
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::new(Method::Post, URL, Lane::Provider)
            .credential("authorization", &format!("Bearer {KEY}"))
            .body(ExpectBody::JsonText(
                r#"{"model":"google/lyria-3-pro-preview","messages":[{"role":"user","content":"a calm harbour theme"}],"modalities":["text","audio"],"audio":{"format":"mp3"},"stream":true}"#.into(),
            ))
            .reply(
                HttpResponse::new(200, text.into_bytes())
                    .with_header("content-type", "text/event-stream")
                    .with_header("x-request-id", "music-1"),
            ),
    ]));
    let adapter = adapter_over(transport.clone(), test_keys());
    let answer = answered(block_on(adapter.send(&call)));
    transport.assert_done();
    let request = &transport.requests()[0];
    assert_eq!(request.timeout, Duration::from_secs(900));
    assert_eq!(request.max_response_bytes, 128 * 1024 * 1024);
    assert!(request.headers.is_empty(), "{:?}", request.headers);
    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.cost, Some(Usd(51_235)));
    assert_eq!(answer.files.len(), 1);
    assert_eq!(answer.files["audio"].kind, "audio/mpeg");
    assert_eq!(answer.files["audio"].bytes, mp3());
    assert_eq!(adapter.check(&call, &answer), Ok(()));
}

#[test]
fn the_fixture_stream_plays() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/openrouter/music/stream-two-chunks.json");
    let transport = Arc::new(ReplayTransport::from_fixture(&path));
    let adapter = adapter_over(transport.clone(), test_keys());
    let call = call(json!({"prompt": "a calm harbour theme, 30 seconds", "duration": null}));
    let answer = answered(block_on(adapter.send(&call)));
    transport.assert_done();
    assert_eq!(answer.files["audio"].bytes, mp3());
    assert_eq!(answer.cost, Some(Usd(51_235)));
    assert_eq!(adapter.check(&call, &answer), Ok(()));
}

#[test]
fn a_stream_is_recognised_by_its_data_lines_alone() {
    let encoded = b64(&mp3());
    let text = stream(&[
        json!({"choices": [{"delta": {"audio": {"data": &encoded[..8]}}}]}),
        json!({"choices": [{"delta": {"audio": {"data": &encoded[8..]}}}]}),
    ]);
    let answer = answered(answer_with(&text, Some("application/octet-stream")));
    assert_eq!(answer.files["audio"].bytes, mp3());
    assert_eq!(answer.cost, None);
    // `data:` with no space, CRLF line breaks, and comment lines between events.
    let text = format!(
        ":keep-alive\r\ndata:{}\r\n\r\n: OPENROUTER PROCESSING\r\ndata:[DONE]\r\n",
        json!({"output_audio": {"data": encoded, "format": "audio/mp3"}, "usage": {"cost": 0}})
    );
    let answer = answered(answer_with(&text, None));
    assert_eq!(answer.files["audio"].bytes, mp3());
    assert_eq!(answer.cost, Some(Usd(0)));
}

// M2 -------------------------------------------------------------------------------------------

#[test]
fn buffered_shapes_are_collected() {
    let encoded = b64(&mp3());
    let data_url = format!("data:audio/mpeg;base64,{encoded}");
    let shapes = [
        json!({"steps": [{"type": "model_output", "content": [
            {"type": "text", "text": "structure"},
            {"type": "audio", "data": encoded, "media_type": "audio/mpeg"}]}],
            "usage": {"cost": 0.25}}),
        json!({"output_audio": {"data": data_url}, "usage": {"cost": 0.25}}),
        json!({"output": [{"content": [
            {"type": "output_text", "text": "a theme"},
            {"type": "output_audio", "audio": {"data": encoded, "mime_type": "audio/mp3"}}]}],
            "usage": {"cost": 0.25}}),
        json!({"choices": [{"message": {"content": "done",
            "audio": {"data": encoded, "format": "mp3", "transcript": "la"}}}],
            "usage": {"cost": 0.25}}),
        json!({"choices": [{"message": {"content": [
            {"type": "input_audio", "data": encoded, "content_type": "audio/mpeg3"}]}}],
            "usage": {"cost": 0.25}}),
        // Split across two places: the chunks are concatenated in reading order.
        json!({"choices": [{"message": {"audio": {"data": &encoded[..12]}}}],
               "output_audio": {"data": &encoded[12..]},
               "usage": {"cost": 0.25}}),
    ];
    for shape in shapes {
        let answer = answered(answer_with(&shape.to_string(), Some("application/json")));
        assert_eq!(answer.files["audio"].bytes, mp3(), "{shape}");
        assert_eq!(answer.files["audio"].kind, "audio/mpeg");
        assert_eq!(answer.cost, Some(Usd(250_000)));
        assert_eq!(answer.data, Value::Null);
    }
}

#[test]
fn steps_other_than_model_output_are_read_past() {
    let shape = json!({"steps": [{"type": "reasoning", "content": [
        {"type": "audio", "data": b64(&mp3())}]}]});
    let (reason, cost, retryable) = failed(answer_with(&shape.to_string(), None));
    assert_eq!(reason, "OpenRouter music generation returned no audio data");
    assert_eq!((cost, retryable), (None, true));
}

// M3 -------------------------------------------------------------------------------------------

#[test]
fn bodies_that_cannot_become_an_answer_fail_the_attempt() {
    let encoded = b64(&mp3());
    let sse = Some("text/event-stream");
    let cases: Vec<(String, Option<&str>, &str, Option<Usd>)> = vec![
        (
            String::new(),
            sse,
            "OpenRouter music generation returned an empty response",
            None,
        ),
        (
            " \n\n ".into(),
            None,
            "OpenRouter music generation returned an empty response",
            None,
        ),
        (
            stream(&[
                json!({"choices": [{"delta": {"audio": {"data": encoded}}}], "usage": {"cost": 0.02}}),
                json!({"type": "error", "message": "upstream failed with a secret prompt"}),
            ]),
            sse,
            "OpenRouter music stream reported an error",
            Some(Usd(20_000)),
        ),
        (
            stream(&[json!({"error": {"code": 502, "message": "Provider returned error"}})]),
            sse,
            "OpenRouter music stream reported an error: code=502",
            None,
        ),
        (
            ": OPENROUTER PROCESSING\n\n: OPENROUTER PROCESSING\n\ndata: [DONE]\n\n".into(),
            sse,
            "OpenRouter music stream contained no events",
            None,
        ),
        (
            "data: {not json\n\n".into(),
            None,
            "OpenRouter music stream contained invalid JSON",
            None,
        ),
        (
            "data: [1, 2]\n\n".into(),
            sse,
            "OpenRouter music stream contained a non-object event",
            None,
        ),
        (
            stream(&[
                json!({"choices": [{"delta": {"audio": {"data": &encoded[..16], "format": "mp3"}}}]}),
                json!({"choices": [{"delta": {"audio": {"data": &encoded[16..], "format": "wav"}}}],
                       "usage": {"cost": 0.3}}),
            ]),
            sse,
            "OpenRouter music generation declared conflicting media types: audio/mpeg, audio/wav",
            Some(Usd(300_000)),
        ),
        (
            stream(&[
                json!({"choices": [{"delta": {"audio": {"data": b64(media::WAV), "format": "wav"}}}]}),
            ]),
            sse,
            "OpenRouter music generation: requested mp3 but received audio/wav",
            None,
        ),
        (
            stream(&[
                json!({"output_audio": {"data": format!("data:audio/ogg;base64,{encoded}")}}),
            ]),
            sse,
            "OpenRouter music generation: requested mp3 but received audio/ogg",
            None,
        ),
        (
            // Each chunk padded on its own: the concatenation is not one base64 text.
            stream(&[
                json!({"choices": [{"delta": {"audio": {"data": b64(&mp3()[..10])}}}]}),
                json!({"choices": [{"delta": {"audio": {"data": b64(&mp3()[10..])}}}],
                       "usage": {"cost": 0.4}}),
            ]),
            sse,
            "OpenRouter music audio data is not valid base64",
            Some(Usd(400_000)),
        ),
        (
            stream(&[
                json!({"choices": [{"delta": {"content": "only words"}}], "usage": {"cost": 0.01}}),
            ]),
            sse,
            "OpenRouter music generation returned no audio data",
            Some(Usd(10_000)),
        ),
        (
            stream(&[json!({"output_audio": {"data": encoded, "media_type": "text/plain"}})]),
            sse,
            "OpenRouter music generation: expected audio media type, received text/plain",
            None,
        ),
        (
            json!({"output_audio": {"data": "not base64!"}, "usage": {"cost": 0.05}}).to_string(),
            Some("application/json"),
            "OpenRouter music audio data is not valid base64",
            Some(Usd(50_000)),
        ),
        (
            "<html>bad gateway</html>".into(),
            Some("text/html"),
            "OpenRouter music generation returned invalid JSON",
            None,
        ),
        (
            "[1]".into(),
            None,
            "OpenRouter music generation returned a non-object JSON response",
            None,
        ),
    ];
    for (body, content_type, reason, cost) in cases {
        assert_eq!(
            failed(answer_with(&body, content_type)),
            (reason.to_string(), cost, true),
            "{body}"
        );
    }
}

#[test]
fn a_structural_failure_names_the_request_id() {
    let response = HttpResponse::new(200, Vec::new()).with_header("x-request-id", "music-9");
    let sent = send_one(
        &call(json!({"prompt": "p"})),
        "p",
        Reply::Response(response),
    );
    assert_eq!(
        failed(sent).0,
        "OpenRouter music generation returned an empty response (request music-9)"
    );
}

// M4 -------------------------------------------------------------------------------------------

#[test]
fn bytes_without_the_mp3_signature_fail_the_check() {
    let call = call(json!({"prompt": "p"}));
    let adapter = adapter_over(Arc::new(NoNetwork), test_keys());
    let body = json!({"choices": [{"message": {"audio": {"data": "AAAA", "format": "mp3"}}}]});
    let answer = answered(answer_with(&body.to_string(), None));
    assert_eq!(answer.files["audio"].bytes, [0, 0, 0]);
    assert_eq!(
        adapter.check(&call, &answer),
        Err("audio bytes do not match declared media type audio/mpeg".into())
    );
    let frame =
        Answer::new(Value::Null, None).with_file("audio", "audio/mpeg", media::MP3_FRAME.to_vec());
    assert_eq!(adapter.check(&call, &frame), Ok(()));
    assert_eq!(
        adapter.check(&call, &Answer::new(Value::Null, None)),
        Err("the answer has no audio".into())
    );
}

// Refusals ---------------------------------------------------------------------------------------

#[test]
fn foreign_contracts_are_refused_before_anything_else() {
    for contract in [
        json!({"adapter": "openrouter-music", "adapter_behavior": 2}),
        json!({"adapter": "openrouter-music", "adapter_behavior": "1"}),
        json!({"adapter": "openrouter-structured", "adapter_behavior": 1}),
        json!({"adapter": "openrouter-structured"}),
        json!({"adapter_behavior": 2}),
    ] {
        let call = CallBuilder::new("music.generate", ROUTE)
            .contract(contract.clone())
            .request(json!({"prompt": " ", "voice": "v"}))
            .build();
        assert_eq!(
            refused(&call, Keys::none()),
            "google/lyria-3-pro-preview@openrouter is not a music.generate route this adapter serves",
            "{contract}"
        );
    }
    // Each member is checked when present (spec/providers.md §5 step 1).
    for contract in [
        json!({}),
        json!({"adapter": "openrouter-music"}),
        json!({"adapter_behavior": 1}),
    ] {
        let call = CallBuilder::new("music.generate", ROUTE)
            .contract(contract.clone())
            .request(json!({"prompt": "p"}))
            .build();
        assert_eq!(
            refused(&call, Keys::none()),
            "OPENROUTER_API_KEY is not set",
            "{contract}"
        );
    }
    // A behaviour written as 1.0 is the same number.
    let call = CallBuilder::new("music.generate", ROUTE)
        .contract(json!({"adapter": "openrouter-music", "adapter_behavior": 1.0}))
        .request(json!({"prompt": " "}))
        .build();
    assert_eq!(
        refused(&call, test_keys()),
        "a music call needs its prompt as text"
    );
    let sound = CallBuilder::new("sound.generate", ROUTE)
        .contract(json!({"adapter": "openrouter-music", "adapter_behavior": 1}))
        .request(json!({"prompt": "p"}))
        .build();
    assert_eq!(
        refused(&sound, test_keys()),
        "google/lyria-3-pro-preview@openrouter is not a sound.generate route this adapter serves"
    );
}

#[test]
fn requests_are_refused_in_order() {
    assert_eq!(
        refused(&call(json!({"prompt": "p", "voice": "v"})), Keys::none()),
        "music.generate takes no member voice"
    );
    assert_eq!(
        refused(&call(json!({"duration": 30})), test_keys()),
        "music.generate needs prompt"
    );
    assert_eq!(
        refused(&call(json!({"prompt": "p", "duration": "30"})), test_keys()),
        "duration is a number"
    );
    assert_eq!(
        refused(&call(json!({"prompt": " "})), Keys::none()),
        "OPENROUTER_API_KEY is not set"
    );
    assert_eq!(
        refused(&call(json!({"prompt": "\t\n"})), test_keys()),
        "a music call needs its prompt as text"
    );
}

// Statuses and transport phases ------------------------------------------------------------------

#[test]
fn statuses_and_phases_classify_per_spec() {
    let envelope = json!({"error": {"code": 400, "message": "Provider returned error"}});
    let replies = vec![
        (
            Reply::Response(HttpResponse::json(429, &json!({}))),
            "not_received",
        ),
        (
            Reply::Response(HttpResponse::json(503, &envelope)),
            "not_received",
        ),
        (Reply::Response(HttpResponse::json(400, &envelope)), "zero"),
        (Reply::Response(HttpResponse::json(402, &envelope)), "zero"),
        (
            Reply::Response(HttpResponse::json(400, &json!("x"))),
            "unknown",
        ),
        (
            Reply::Response(HttpResponse::json(500, &json!({}))),
            "retry",
        ),
        (
            Reply::Response(HttpResponse::json(302, &json!({}))),
            "unknown",
        ),
        (
            Reply::Error(TransportError::not_sent(
                TransportErrorKind::Connect,
                "refused",
            )),
            "not_received",
        ),
        (
            Reply::Error(TransportError::after_send(
                TransportErrorKind::Timeout,
                "deadline",
            )),
            "retry",
        ),
        (
            Reply::Zeros {
                status: 200,
                headers: Vec::new(),
                len: 128 * 1024 * 1024 + 1,
            },
            "retry",
        ),
    ];
    for (reply, class) in replies {
        let shown = format!("{reply:?}").chars().take(80).collect::<String>();
        let sent = send_one(&call(json!({"prompt": "p"})), "p", reply);
        let ok = matches!(
            (class, &sent),
            ("not_received", Sent::NotReceived { .. })
                | (
                    "zero",
                    Sent::Failed {
                        cost: Some(Usd::ZERO),
                        retryable: false,
                        ..
                    }
                )
                | (
                    "unknown",
                    Sent::Failed {
                        cost: None,
                        retryable: false,
                        ..
                    }
                )
                | (
                    "retry",
                    Sent::Failed {
                        cost: None,
                        retryable: true,
                        ..
                    }
                )
        );
        assert!(ok, "{shown}: expected {class}, got {sent:?}");
    }
    let sent =
        block_on(adapter_over(Arc::new(Offline), test_keys()).send(&call(json!({"prompt": "p"}))));
    assert_eq!(
        sent,
        Sent::Refused {
            reason: "OpenRouter music generation was not sent: the network is off (GRIDA_FX_NETWORK=off)"
                .into()
        }
    );
}

// Credential hygiene -----------------------------------------------------------------------------

#[test]
fn the_key_never_leaves_the_credential_slot() {
    let leaky = json!({"error": {"code": KEY, "message": format!("bad key {KEY}")}});
    let encoded = b64(&mp3());
    let bodies = vec![
        (401, leaky.to_string(), None),
        (
            200,
            stream(&[json!({"error": {"code": KEY, "type": KEY}})]),
            Some("text/event-stream"),
        ),
        (
            200,
            stream(&[json!({"output_audio": {"data": encoded, "format": KEY}})]),
            None,
        ),
        (
            200,
            stream(&[json!({"output_audio": {"data": encoded}, "note": KEY})]),
            None,
        ),
    ];
    for (status, body, content_type) in bodies {
        let mut response =
            HttpResponse::new(status, body.into_bytes()).with_header("x-request-id", KEY);
        if let Some(content_type) = content_type {
            response = response.with_header("content-type", content_type);
        }
        let transport = Arc::new(ReplayTransport::new(vec![
            Expect::new(Method::Post, URL, Lane::Provider)
                .credential("authorization", &format!("Bearer {KEY}"))
                .body(ExpectBody::Any)
                .reply(response),
        ]));
        let sent = block_on(
            adapter_over(transport.clone(), test_keys()).send(&call(json!({"prompt": "p"}))),
        );
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
            assert!(!format!("{:?}", request.body).contains(KEY));
            let credential = request
                .credential
                .expect("the provider lane carries the key");
            assert_eq!(
                (credential.header, credential.prefix),
                ("authorization", "Bearer ")
            );
        }
    }
}
