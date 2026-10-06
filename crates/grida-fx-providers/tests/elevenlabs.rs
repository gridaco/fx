//! ElevenLabs sound and speech (spec/providers.md §9.5): every request asserted on a replay
//! transport, every refusal over `NoNetwork` (nothing may be sent), synthetic MP3 bytes only.
//!
//! Mirrors the reference engine's adapter tests (the request each route sends, absent members
//! left out, the status and transport mapping, the answer checks, the provider's error message
//! never quoted), adapted to FX's decisions: refusals before sending cost nothing, deterministic
//! 4xx answers are not retryable, and an answer carries no usage or request id.

use grida_fx_providers::clock::FakeClock;
use grida_fx_providers::elevenlabs::{
    self, DEADLINE, MAX_RESPONSE_BYTES, sound::ElevenLabsSound, speech::ElevenLabsSpeech,
};
use grida_fx_providers::testing::media::{MP3, MP3_FRAME, WAV};
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, setup, test_keys};
use grida_fx_providers::transport::replay::{
    Exchange, Expect, ExpectBody, NoNetwork, ReplayTransport, Reply,
};
use grida_fx_providers::transport::{
    HttpResponse, Lane, Method, Transport, TransportError, TransportErrorKind,
};
use grida_fx_providers::{
    Adapter, Adapters, Answer, Endpoints, KeyName, Keys, RequestAdapter, Sent, Setup,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const KEY: &str = "test-elevenlabs-key";
const SOUND_ROUTE: &str = "eleven_text_to_sound_v2@elevenlabs";
const SPEECH_ROUTE: &str = "eleven_v3@elevenlabs";
const SOUND_URL: &str = "https://api.elevenlabs.io/v1/sound-generation?output_format=mp3_44100_192";
const SOUND_LABEL: &str = "ElevenLabs sound generation";
const SPEECH_LABEL: &str = "ElevenLabs speech generation";

fn speech_url(voice: &str) -> String {
    format!("https://api.elevenlabs.io/v1/text-to-speech/{voice}?output_format=mp3_44100_192")
}

fn sound_contract() -> Value {
    json!({"adapter": "elevenlabs-sound-effect", "adapter_behavior": 1})
}

fn speech_contract() -> Value {
    json!({"adapter": "elevenlabs-speech", "adapter_behavior": 1})
}

fn sound_call(request: Value) -> TestCall {
    CallBuilder::new("sound.generate", SOUND_ROUTE)
        .contract(sound_contract())
        .request(request)
        .build()
}

fn speech_call(request: Value) -> TestCall {
    CallBuilder::new("speech.generate", SPEECH_ROUTE)
        .contract(speech_contract())
        .request(request)
        .build()
}

fn sound(transport: Arc<dyn Transport>, keys: Keys) -> ElevenLabsSound {
    ElevenLabsSound::new(elevenlabs::client(&setup(transport, keys)))
}

fn speech(transport: Arc<dyn Transport>, keys: Keys) -> ElevenLabsSpeech {
    ElevenLabsSpeech::new(elevenlabs::client(&setup(transport, keys)))
}

/// A request the adapters send: the credential, the two headers, the exact compact body.
fn expect(url: &str, body: &str) -> Expect {
    Expect::new(Method::Post, url, Lane::Provider)
        .credential("xi-api-key", KEY)
        .header("content-type", "application/json")
        .header("accept", "audio/mpeg")
        .body(ExpectBody::JsonText(body.into()))
}

fn expect_sound(body: &str) -> Expect {
    expect(SOUND_URL, body)
}

const DOOR: &str =
    r#"{"text":"wooden door opens","model_id":"eleven_text_to_sound_v2","loop":false}"#;

fn mp3(bytes: &[u8]) -> HttpResponse {
    HttpResponse::new(200, bytes.to_vec()).with_header("content-type", "audio/mpeg")
}

/// Sends one sound call through a transport scripted with `exchanges`, asserting every exchange
/// was used.
fn sound_with(exchanges: Vec<Exchange>, request: Value) -> (Sent, Arc<ReplayTransport>) {
    let transport = Arc::new(ReplayTransport::new(exchanges));
    let adapter = sound(transport.clone(), test_keys());
    let sent = block_on(adapter.send(&sound_call(request)));
    transport.assert_done();
    (sent, transport)
}

/// A sound call answered with `response`.
fn sound_answered_with(response: HttpResponse) -> Sent {
    sound_with(
        vec![expect_sound(DOOR).reply(response)],
        json!({"prompt": "wooden door opens"}),
    )
    .0
}

fn reason_of(sent: &Sent) -> &str {
    match sent {
        Sent::NotReceived { reason, .. }
        | Sent::Refused { reason }
        | Sent::Failed { reason, .. } => reason,
        Sent::Answered(_) => panic!("answered: {sent:?}"),
    }
}

fn answer_of(sent: Sent) -> Answer {
    match sent {
        Sent::Answered(answer) => answer,
        other => panic!("not answered: {other:?}"),
    }
}

/// The key appears nowhere in what the adapter reports.
fn assert_clean(sent: &Sent) {
    let shown = match sent {
        Sent::Answered(answer) => {
            let kinds: Vec<&str> = answer.files.values().map(|f| f.kind.as_str()).collect();
            format!("{} {kinds:?}", answer.data)
        }
        other => reason_of(other).to_string(),
    };
    assert!(!shown.contains(KEY), "the key leaked: {shown}");
    assert!(
        !shown.contains("secret"),
        "a provider message leaked: {shown}"
    );
}

/// No request that left carries the key anywhere but in its credential slot.
fn assert_key_only_in_credential(transport: &ReplayTransport) {
    for request in transport.requests() {
        assert!(!request.url.contains(KEY));
        assert!(request.headers.iter().all(|(_, v)| !v.contains(KEY)));
        assert_eq!(request.lane, Lane::Provider);
        assert_eq!(
            request.credential.as_ref().map(|c| c.header),
            Some("xi-api-key")
        );
    }
}

fn refused_without_sending(adapter: &dyn RequestAdapter, call: &TestCall) -> String {
    match block_on(adapter.send(call)) {
        Sent::Refused { reason } => {
            assert!(!reason.contains(KEY));
            reason
        }
        other => panic!("not refused: {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// 1. Happy paths

#[test]
fn a_sound_effect_sends_its_prompt_verbatim_and_answers_the_mp3() {
    let body = r#"{"text":"wooden door opens","model_id":"eleven_text_to_sound_v2","loop":false,"duration_seconds":0.6,"prompt_influence":0.3}"#;
    let (sent, transport) = sound_with(
        vec![
            expect_sound(body).reply(
                mp3(MP3_FRAME)
                    .with_header("request-id", "req-7")
                    .with_header("character-cost", "11"),
            ),
        ],
        json!({"prompt": "wooden door opens", "duration": 0.6, "loop": false, "prompt_influence": 0.3}),
    );
    assert_clean(&sent);
    let answer = answer_of(sent);
    assert_eq!(
        answer,
        Answer::new(Value::Null, None).with_file("audio", "audio/mpeg", MP3_FRAME.to_vec())
    );
    let adapter = sound(Arc::new(NoNetwork), test_keys());
    assert_eq!(adapter.check(&sound_call(json!({})), &answer), Ok(()));
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].timeout, DEADLINE);
    assert_eq!(requests[0].max_response_bytes, MAX_RESPONSE_BYTES);
    assert_eq!(DEADLINE.as_secs(), 120);
    assert_eq!(MAX_RESPONSE_BYTES, 64 * 1024 * 1024);
    assert_key_only_in_credential(&transport);
}

#[test]
fn a_speech_line_sends_text_voice_settings_and_language() {
    let body = r#"{"text":"[excited] いくよっ!","model_id":"eleven_v3","voice_settings":{"stability":0.5},"language_code":"ja"}"#;
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(&speech_url("voice-7"), body).reply(
            mp3(MP3)
                .with_header("request-id", "req-9")
                .with_header("character-cost", "23"),
        ),
    ]));
    let adapter = speech(transport.clone(), test_keys());
    let call = speech_call(json!({
        "text": "[excited] いくよっ!",
        "voice": "voice-7",
        "stability": 0.5,
        "language_code": "ja",
    }));
    let sent = block_on(adapter.send(&call));
    transport.assert_done();
    assert_clean(&sent);
    let answer = answer_of(sent);
    assert_eq!(
        answer,
        Answer::new(Value::Null, None).with_file("audio", "audio/mpeg", MP3.to_vec())
    );
    assert_eq!(adapter.check(&call, &answer), Ok(()));
    let request = &transport.requests()[0];
    assert_eq!(
        (request.timeout, request.max_response_bytes),
        (DEADLINE, MAX_RESPONSE_BYTES)
    );
    assert_key_only_in_credential(&transport);
}

#[test]
fn the_synthetic_fixture_replays_both_routes() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/elevenlabs/speech_busy_then_answered.json");
    let transport = Arc::new(ReplayTransport::from_fixture(&fixture));
    let speech = speech(transport.clone(), test_keys());
    let call = speech_call(json!({
        "text": "[excited] いくよっ!",
        "voice": "voice-7",
        "stability": 0.5,
        "language_code": "ja",
        "max_chars": 40,
    }));
    let busy = block_on(speech.send(&call));
    assert_eq!(
        busy,
        Sent::NotReceived {
            reason: "ElevenLabs speech generation returned HTTP 429: system_busy; retry-after 2"
                .into(),
            retry_after: Some(Duration::from_secs(2)),
        }
    );
    let answer = answer_of(block_on(speech.send(&call)));
    assert_eq!(answer.files["audio"].bytes, MP3_FRAME);
    assert_eq!(answer.data, Value::Null, "no usage, no request id");
    assert_eq!(speech.check(&call, &answer), Ok(()));
    let sound = sound(transport.clone(), test_keys());
    let call = sound_call(json!({
        "prompt": "wooden door opens", "duration": 0.6, "loop": false, "prompt_influence": 0.3,
    }));
    let answer = answer_of(block_on(sound.send(&call)));
    assert_eq!(answer.files["audio"].kind, "audio/mpeg", "no content-type");
    assert_eq!(answer.files["audio"].bytes, MP3);
    assert_eq!(sound.check(&call, &answer), Ok(()));
    transport.assert_done();
}

#[test]
fn both_adapters_register_on_elevenlabs() {
    let mut adapters = Adapters::new();
    elevenlabs::register(&mut adapters, &setup(Arc::new(NoNetwork), test_keys()));
    assert_eq!(
        adapters.served(),
        vec![
            ("sound.generate".to_string(), "elevenlabs".to_string()),
            ("speech.generate".to_string(), "elevenlabs".to_string()),
        ]
    );
    let call = sound_call(json!({"prompt": "door"}));
    assert!(matches!(
        adapters.serving(&call.route),
        Some(Adapter::Request(_))
    ));
}

#[test]
fn the_base_url_comes_from_the_endpoints() {
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::new(
            Method::Post,
            "http://127.0.0.1:8089/v1/sound-generation?output_format=mp3_44100_192",
            Lane::Provider,
        )
        .credential("xi-api-key", KEY)
        .body(ExpectBody::JsonText(DOOR.into()))
        .reply(mp3(MP3)),
    ]));
    let setup = Setup {
        transport: transport.clone(),
        keys: test_keys(),
        endpoints: Endpoints {
            elevenlabs: "http://127.0.0.1:8089/v1".into(),
            ..Endpoints::default()
        },
        clock: Arc::new(FakeClock::new()),
    };
    let adapter = ElevenLabsSound::new(elevenlabs::client(&setup));
    let sent = block_on(adapter.send(&sound_call(json!({"prompt": "wooden door opens"}))));
    assert!(matches!(sent, Sent::Answered(_)));
    transport.assert_done();
}

// ---------------------------------------------------------------------------------------------
// 2. and 3. Absent members, and numbers as floats

#[test]
fn unset_sound_values_are_left_out_and_loop_defaults_to_false() {
    for request in [
        json!({"prompt": "wooden door opens"}),
        json!({"prompt": "wooden door opens", "duration": null, "prompt_influence": null, "loop": null}),
    ] {
        let (sent, _) = sound_with(vec![expect_sound(DOOR).reply(mp3(MP3))], request);
        assert!(matches!(sent, Sent::Answered(_)));
    }
    let looped =
        r#"{"text":"rain on a tin roof","model_id":"eleven_text_to_sound_v2","loop":true}"#;
    let (sent, _) = sound_with(
        vec![expect_sound(looped).reply(mp3(MP3))],
        json!({"prompt": "rain on a tin roof", "loop": true}),
    );
    assert!(matches!(sent, Sent::Answered(_)));
}

#[test]
fn whole_numbers_go_out_as_floats() {
    let body = r#"{"text":"heavy axe swing","model_id":"eleven_text_to_sound_v2","loop":false,"duration_seconds":1.0,"prompt_influence":0.0}"#;
    let (sent, _) = sound_with(
        vec![expect_sound(body).reply(mp3(MP3))],
        json!({"prompt": "heavy axe swing", "duration": 1, "prompt_influence": 0}),
    );
    assert!(matches!(sent, Sent::Answered(_)));
    let body = r#"{"text":"hello","model_id":"eleven_v3","voice_settings":{"stability":1.0}}"#;
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(&speech_url("v1"), body).reply(mp3(MP3)),
    ]));
    let sent = block_on(speech(transport.clone(), test_keys()).send(&speech_call(
        json!({"text": "hello", "voice": "v1", "stability": 1}),
    )));
    assert!(matches!(sent, Sent::Answered(_)));
    transport.assert_done();
}

#[test]
fn the_prompt_is_not_trimmed() {
    let body = r#"{"text":"  door creaks \n","model_id":"eleven_text_to_sound_v2","loop":false}"#;
    let (sent, _) = sound_with(
        vec![expect_sound(body).reply(mp3(MP3))],
        json!({"prompt": "  door creaks \n"}),
    );
    assert!(matches!(sent, Sent::Answered(_)));
}

#[test]
fn unset_speech_values_and_max_chars_are_never_sent() {
    let plain = r#"{"text":"hello there","model_id":"eleven_v3"}"#;
    for request in [
        json!({"text": "hello there", "voice": "6awt6FKyZGV0HyQEwisX"}),
        json!({"text": "hello there", "voice": "6awt6FKyZGV0HyQEwisX", "language_code": null}),
        json!({"text": "hello there", "voice": "6awt6FKyZGV0HyQEwisX", "language_code": ""}),
        json!({"text": "hello there", "voice": "6awt6FKyZGV0HyQEwisX", "stability": null}),
        json!({"text": "hello there", "voice": "6awt6FKyZGV0HyQEwisX", "max_chars": 200}),
    ] {
        let transport = Arc::new(ReplayTransport::new(vec![
            expect(&speech_url("6awt6FKyZGV0HyQEwisX"), plain).reply(mp3(MP3)),
        ]));
        let sent = block_on(speech(transport.clone(), test_keys()).send(&speech_call(request)));
        assert!(matches!(sent, Sent::Answered(_)), "{sent:?}");
        transport.assert_done();
    }
}

// ---------------------------------------------------------------------------------------------
// 4. and 11. Refusals before sending: nothing reaches the transport

#[test]
fn sound_refusals_send_nothing_and_say_why() {
    let adapter = sound(Arc::new(NoNetwork), test_keys());
    let cases: Vec<(Value, &str)> = vec![
        (
            json!({"prompt": "   \n"}),
            "a sound call needs its prompt as text",
        ),
        (
            json!({"prompt": ""}),
            "a sound call needs its prompt as text",
        ),
        (
            json!({"prompt": "x".repeat(451)}),
            "sound effect prompt must be at most 450 characters",
        ),
        (
            json!({"prompt": "い".repeat(451)}),
            "sound effect prompt must be at most 450 characters",
        ),
        (
            json!({"prompt": "door", "duration": 0.4}),
            "duration must be between 0.5 and 30",
        ),
        (
            json!({"prompt": "door", "duration": 30.01}),
            "duration must be between 0.5 and 30",
        ),
        (
            json!({"prompt": "door", "duration": -1}),
            "duration must be between 0.5 and 30",
        ),
        (
            json!({"prompt": "door", "prompt_influence": 1.5}),
            "prompt_influence must be between 0 and 1",
        ),
        (
            json!({"prompt": "door", "prompt_influence": -0.1}),
            "prompt_influence must be between 0 and 1",
        ),
        // The capability's shape (spec/capabilities.md §1) comes before the values.
        (json!({}), "sound.generate needs prompt"),
        (json!({"prompt": null}), "sound.generate needs prompt"),
        (json!({"prompt": 7}), "prompt is text"),
        (
            json!({"prompt": "door", "duration": "1"}),
            "duration is a number",
        ),
        (
            json!({"prompt": "door", "duration": true}),
            "duration is a number",
        ),
        (
            json!({"prompt": "door", "prompt_influence": "0.3"}),
            "prompt_influence is a number",
        ),
        (
            json!({"prompt": "door", "loop": "yes"}),
            "loop is true or false",
        ),
        (
            json!({"prompt": "door", "seed": 7}),
            "sound.generate takes no member seed",
        ),
        (
            json!({"prompt": "", "duration": "x"}),
            "duration is a number",
        ),
    ];
    for (request, sentence) in cases {
        let call = sound_call(request.clone());
        assert_eq!(
            refused_without_sending(&adapter, &call),
            sentence,
            "{request}"
        );
    }
}

#[test]
fn sound_bounds_are_inclusive_and_count_characters() {
    for (request, body) in [
        (
            json!({"prompt": "い".repeat(450)}),
            json!({"text": "い".repeat(450), "model_id": "eleven_text_to_sound_v2", "loop": false}),
        ),
        (
            json!({"prompt": "door", "duration": 0.5, "prompt_influence": 0}),
            json!({"text": "door", "model_id": "eleven_text_to_sound_v2", "loop": false, "duration_seconds": 0.5, "prompt_influence": 0.0}),
        ),
        (
            json!({"prompt": "door", "duration": 30, "prompt_influence": 1}),
            json!({"text": "door", "model_id": "eleven_text_to_sound_v2", "loop": false, "duration_seconds": 30.0, "prompt_influence": 1.0}),
        ),
    ] {
        let exchange = Expect::post_json(SOUND_URL, body)
            .credential("xi-api-key", KEY)
            .reply(mp3(MP3));
        let (sent, _) = sound_with(vec![exchange], request);
        assert!(matches!(sent, Sent::Answered(_)), "{sent:?}");
    }
}

#[test]
fn speech_refusals_send_nothing_and_say_why() {
    let adapter = speech(Arc::new(NoNetwork), test_keys());
    let cases: Vec<(Value, &str)> = vec![
        (
            json!({"text": " ", "voice": "v1"}),
            "a speech call needs its text",
        ),
        (
            json!({"text": "hi", "voice": " "}),
            "a speech call needs its provider voice",
        ),
        (
            json!({"text": "hi", "voice": ""}),
            "a speech call needs its provider voice",
        ),
        (
            json!({"text": "hi", "voice": "../voices"}),
            "the provider voice is not a voice id",
        ),
        (
            json!({"text": "hi", "voice": "v1?output_format=pcm_16000"}),
            "the provider voice is not a voice id",
        ),
        (
            json!({"text": "hi", "voice": "v1#frag"}),
            "the provider voice is not a voice id",
        ),
        (
            json!({"text": "hi", "voice": " v1"}),
            "the provider voice is not a voice id",
        ),
        (
            json!({"text": "hi", "voice": "v%2F1"}),
            "the provider voice is not a voice id",
        ),
        (
            json!({"text": "hi", "voice": "a".repeat(129)}),
            "the provider voice is not a voice id",
        ),
        (
            json!({"text": "x".repeat(5001), "voice": "v1"}),
            "speech text must be at most 5000 characters",
        ),
        (
            json!({"text": "hi", "voice": "v1", "stability": 1.5}),
            "stability must be between 0 and 1",
        ),
        (
            json!({"text": "hi", "voice": "v1", "stability": -0.01}),
            "stability must be between 0 and 1",
        ),
        (
            json!({"text": "hi", "voice": "v1", "language_code": "  "}),
            "language_code must not be blank",
        ),
        // The text is checked before the voice, and the voice before the length.
        (
            json!({"text": "", "voice": "a/b"}),
            "a speech call needs its text",
        ),
        (
            json!({"text": "x".repeat(5001), "voice": "a/b"}),
            "the provider voice is not a voice id",
        ),
        // The capability's shape first.
        (json!({"text": "hi"}), "speech.generate needs voice"),
        (json!({"voice": "v1"}), "speech.generate needs text"),
        (json!({"text": "hi", "voice": 7}), "voice is text"),
        (
            json!({"text": "hi", "voice": "v1", "stability": "0.5"}),
            "stability is a number",
        ),
        (
            json!({"text": "hi", "voice": "v1", "language_code": 1}),
            "language_code is text",
        ),
        (
            json!({"text": "hi", "voice": "v1", "max_chars": 1.5}),
            "max_chars is a whole number",
        ),
        (
            json!({"text": "hi", "voice": "v1", "seed": 1}),
            "speech.generate takes no member seed",
        ),
    ];
    for (request, sentence) in cases {
        let call = speech_call(request.clone());
        assert_eq!(
            refused_without_sending(&adapter, &call),
            sentence,
            "{request}"
        );
    }
}

#[test]
fn speech_bounds_are_inclusive() {
    let voice = "a".repeat(128);
    let text = "い".repeat(5000);
    let transport = Arc::new(ReplayTransport::new(vec![
        Expect::post_json(
            speech_url(&voice),
            json!({"text": text, "model_id": "eleven_v3", "voice_settings": {"stability": 0.0}}),
        )
        .credential("xi-api-key", KEY)
        .reply(mp3(MP3)),
    ]));
    let sent = block_on(speech(transport.clone(), test_keys()).send(&speech_call(
        json!({"text": text, "voice": voice, "stability": 0}),
    )));
    assert!(matches!(sent, Sent::Answered(_)), "{sent:?}");
    transport.assert_done();
}

#[test]
fn a_missing_or_blank_key_is_refused_after_the_shape_and_before_the_values() {
    let blank = Keys::from_pairs(&[(KeyName::ElevenLabs, "   ")]);
    for keys in [Keys::none(), blank] {
        let sound = sound(Arc::new(NoNetwork), keys.clone());
        let speech = speech(Arc::new(NoNetwork), keys);
        // A bad value is not reached without the key.
        for request in [
            json!({"prompt": "door"}),
            json!({"prompt": "door", "duration": 99}),
        ] {
            assert_eq!(
                refused_without_sending(&sound, &sound_call(request)),
                "ELEVENLABS_API_KEY is not set"
            );
        }
        assert_eq!(
            refused_without_sending(&speech, &speech_call(json!({"text": "hi", "voice": "a/b"}))),
            "ELEVENLABS_API_KEY is not set"
        );
        // The shape comes first.
        assert_eq!(
            refused_without_sending(&sound, &sound_call(json!({"prompt": "door", "pitch": 2}))),
            "sound.generate takes no member pitch"
        );
    }
}

#[test]
fn a_route_this_adapter_does_not_serve_is_refused_first() {
    let sound = sound(Arc::new(NoNetwork), Keys::none());
    let wrong_adapter = CallBuilder::new("sound.generate", SOUND_ROUTE)
        .contract(json!({"adapter": "elevenlabs-speech", "adapter_behavior": 1}))
        .request(json!({"seed": 1}))
        .build();
    assert_eq!(
        refused_without_sending(&sound, &wrong_adapter),
        "eleven_text_to_sound_v2@elevenlabs is not a sound.generate route this adapter serves"
    );
    let wrong_behavior = CallBuilder::new("sound.generate", SOUND_ROUTE)
        .contract(json!({"adapter": "elevenlabs-sound-effect", "adapter_behavior": 2}))
        .request(json!({"prompt": "door"}))
        .build();
    assert_eq!(
        refused_without_sending(&sound, &wrong_behavior),
        "eleven_text_to_sound_v2@elevenlabs is not a sound.generate route this adapter serves"
    );
    let speech = speech(Arc::new(NoNetwork), test_keys());
    let sound_route = CallBuilder::new("speech.generate", SPEECH_ROUTE)
        .contract(sound_contract())
        .request(json!({"text": "hi", "voice": "v1"}))
        .build();
    assert_eq!(
        refused_without_sending(&speech, &sound_route),
        "eleven_v3@elevenlabs is not a speech.generate route this adapter serves"
    );
    let other_capability = CallBuilder::new("music.generate", SPEECH_ROUTE)
        .request(json!({"prompt": "hi"}))
        .build();
    assert_eq!(
        refused_without_sending(&speech, &other_capability),
        "eleven_v3@elevenlabs is not a speech.generate route this adapter serves"
    );
    // A route without a contract is served.
    let transport = Arc::new(ReplayTransport::new(vec![
        expect_sound(DOOR).reply(mp3(MP3)),
    ]));
    let bare = CallBuilder::new("sound.generate", SOUND_ROUTE)
        .request(json!({"prompt": "wooden door opens"}))
        .build();
    assert!(matches!(
        block_on(self::sound(transport.clone(), test_keys()).send(&bare)),
        Sent::Answered(_)
    ));
    transport.assert_done();
}

// ---------------------------------------------------------------------------------------------
// 5. and 6. The answer's kind and the checks

#[test]
fn the_kind_is_the_normalized_content_type() {
    let check = |response: HttpResponse| {
        let answer = answer_of(sound_answered_with(response));
        let kind = answer.files["audio"].kind.clone();
        let checked =
            sound(Arc::new(NoNetwork), test_keys()).check(&sound_call(json!({})), &answer);
        (kind, checked)
    };
    let ok = |kind: &str| (kind.to_string(), Ok(()));
    assert_eq!(
        check(HttpResponse::new(200, MP3.to_vec())),
        ok("audio/mpeg")
    );
    for header in [
        "audio/mpeg",
        "audio/mp3",
        "audio/mpeg3",
        "AUDIO/MPEG; bitrate=192",
    ] {
        assert_eq!(
            check(HttpResponse::new(200, MP3.to_vec()).with_header("Content-Type", header)),
            ok("audio/mpeg"),
            "{header}"
        );
    }
    assert_eq!(
        check(HttpResponse::new(200, MP3.to_vec()).with_header("content-type", "audio/wav")),
        (
            "audio/wav".to_string(),
            Err("requested mp3 but received audio/wav".to_string())
        )
    );
    assert_eq!(
        check(HttpResponse::new(200, WAV.to_vec()).with_header("content-type", "audio/x-wav")),
        (
            "audio/wav".to_string(),
            Err("requested mp3 but received audio/wav".to_string())
        )
    );
    assert_eq!(
        check(
            HttpResponse::new(200, MP3.to_vec())
                .with_header("content-type", "application/json; charset=utf-8")
        ),
        (
            "application/json".to_string(),
            Err("requested mp3 but received application/json".to_string())
        )
    );
    assert_eq!(
        check(HttpResponse::new(200, MP3.to_vec()).with_header("content-type", "")),
        (
            "application/octet-stream".to_string(),
            Err("requested mp3 but received application/octet-stream".to_string())
        )
    );
}

#[test]
fn checks_refuse_empty_and_mislabelled_audio_in_order() {
    let sound = sound(Arc::new(NoNetwork), test_keys());
    let speech = speech(Arc::new(NoNetwork), test_keys());
    let call = sound_call(json!({}));
    let answer = |kind: &str, bytes: &[u8]| {
        Answer::new(Value::Null, None).with_file("audio", kind, bytes.to_vec())
    };
    assert_eq!(
        sound.check(&call, &answer("audio/mpeg", b"")),
        Err("ElevenLabs sound generation returned no audio data".into())
    );
    assert_eq!(
        speech.check(&call, &answer("audio/wav", b"")),
        Err("ElevenLabs speech generation returned no audio data".into()),
        "empty comes before the kind"
    );
    assert_eq!(
        sound.check(&call, &Answer::new(Value::Null, None)),
        Err("ElevenLabs sound generation returned no audio data".into())
    );
    assert_eq!(
        sound.check(&call, &answer("audio/wav", b"not audio")),
        Err("requested mp3 but received audio/wav".into()),
        "the kind comes before the signature"
    );
    assert_eq!(
        sound.check(&call, &answer("audio/mpeg", b"not audio")),
        Err("audio bytes do not match declared media type audio/mpeg".into())
    );
    assert_eq!(
        sound.check(&call, &answer("audio/mpeg", b"\xff")),
        Err("audio bytes do not match declared media type audio/mpeg".into())
    );
    assert_eq!(sound.check(&call, &answer("audio/mpeg", MP3)), Ok(()));
    assert_eq!(sound.check(&call, &answer("audio/mpeg", MP3_FRAME)), Ok(()));
    assert_eq!(speech.check(&call, &answer("audio/mpeg", b"ID3")), Ok(()));
}

#[test]
fn an_empty_or_bad_answer_is_answered_then_refused_by_the_check() {
    let sound = sound(Arc::new(NoNetwork), test_keys());
    let call = sound_call(json!({}));
    let empty = answer_of(sound_answered_with(mp3(b"")));
    assert_eq!(
        sound.check(&call, &empty),
        Err("ElevenLabs sound generation returned no audio data".into())
    );
    let noise = answer_of(sound_answered_with(mp3(b"not audio")));
    assert_eq!(
        sound.check(&call, &noise),
        Err("audio bytes do not match declared media type audio/mpeg".into())
    );
    let speech = speech(Arc::new(NoNetwork), test_keys());
    assert_eq!(
        speech.check(&call, &empty),
        Err("ElevenLabs speech generation returned no audio data".into())
    );
}

// ---------------------------------------------------------------------------------------------
// 7. and 9. Statuses and reasons

#[test]
fn statuses_map_to_outcomes() {
    let not_received = |status: u16| {
        Sent::not_received(format!(
            "ElevenLabs sound generation returned HTTP {status}"
        ))
    };
    let failed = |status: u16, retryable: bool| Sent::Failed {
        reason: format!("ElevenLabs sound generation returned HTTP {status}"),
        cost: None,
        retryable,
    };
    for status in [408, 429] {
        assert_eq!(
            sound_answered_with(HttpResponse::new(status, Vec::new())),
            not_received(status)
        );
    }
    for status in [400, 401, 402, 403, 404, 405, 409, 413, 415, 422, 451] {
        assert_eq!(
            sound_answered_with(HttpResponse::new(status, Vec::new())),
            failed(status, false),
            "{status}"
        );
    }
    for status in [500, 502, 503, 504, 599, 101, 600] {
        assert_eq!(
            sound_answered_with(HttpResponse::new(status, Vec::new())),
            failed(status, true),
            "{status}"
        );
    }
    for status in [301, 302, 307, 308] {
        assert_eq!(
            sound_answered_with(
                HttpResponse::new(status, Vec::new())
                    .with_header("location", "https://elsewhere.example.test/x?sig=1")
            ),
            Sent::Failed {
                reason: format!(
                    "ElevenLabs sound generation was redirected (HTTP {status}); check ELEVENLABS_BASE_URL"
                ),
                cost: None,
                retryable: false,
            },
            "{status}: one request, never followed"
        );
    }
}

#[test]
fn speech_statuses_carry_their_label() {
    let transport = Arc::new(ReplayTransport::new(vec![
        expect(&speech_url("v1"), r#"{"text":"hi","model_id":"eleven_v3"}"#).reply(
            HttpResponse::json(
                422,
                &json!({"detail": "voice not found: secret test-elevenlabs-key"}),
            ),
        ),
        expect(&speech_url("v1"), r#"{"text":"hi","model_id":"eleven_v3"}"#)
            .reply(HttpResponse::new(503, b"busy".to_vec())),
    ]));
    let adapter = speech(transport.clone(), test_keys());
    let call = speech_call(json!({"text": "hi", "voice": "v1"}));
    let refused = block_on(adapter.send(&call));
    assert_clean(&refused);
    assert_eq!(
        refused,
        Sent::Failed {
            reason: "ElevenLabs speech generation returned HTTP 422".into(),
            cost: None,
            retryable: false,
        }
    );
    assert_eq!(
        block_on(adapter.send(&call)),
        Sent::Failed {
            reason: "ElevenLabs speech generation returned HTTP 503".into(),
            cost: None,
            retryable: true,
        }
    );
    transport.assert_done();
}

#[test]
fn the_provider_message_is_never_quoted_but_a_safe_status_is() {
    let sent = sound_answered_with(HttpResponse::json(
        429,
        &json!({"detail": {"message": "secret quota"}}),
    ));
    assert_clean(&sent);
    assert_eq!(
        sent,
        Sent::not_received("ElevenLabs sound generation returned HTTP 429")
    );
    let sent = sound_answered_with(HttpResponse::json(
        401,
        &json!({"detail": {"status": "invalid_api_key", "message": "secret: test-elevenlabs-key is not valid"}}),
    ));
    assert_clean(&sent);
    assert_eq!(
        reason_of(&sent),
        "ElevenLabs sound generation returned HTTP 401: invalid_api_key"
    );
    let sent = sound_answered_with(
        HttpResponse::json(
            429,
            &json!({"detail": {"status": "too_many_concurrent_requests", "message": "secret"}}),
        )
        .with_header("retry-after", "7"),
    );
    assert_eq!(
        sent,
        Sent::NotReceived {
            reason: "ElevenLabs sound generation returned HTTP 429: too_many_concurrent_requests; retry-after 7"
                .into(),
            retry_after: Some(Duration::from_secs(7)),
        }
    );
    // A status that is not a safe field, or that is the key itself, is left out.
    for status in ["quota exceeded", KEY, "https://x.test/?k=1", ""] {
        let sent = sound_answered_with(HttpResponse::json(
            403,
            &json!({"detail": {"status": status}}),
        ));
        assert_clean(&sent);
        assert_eq!(
            reason_of(&sent),
            "ElevenLabs sound generation returned HTTP 403",
            "{status}"
        );
    }
    // A FastAPI validation list is not quoted either.
    let sent = sound_answered_with(HttpResponse::json(
        422,
        &json!({"detail": [{"loc": ["body", "text"], "msg": "secret field required"}]}),
    ));
    assert_clean(&sent);
    assert_eq!(
        reason_of(&sent),
        "ElevenLabs sound generation returned HTTP 422"
    );
}

#[test]
fn an_unsafe_retry_after_is_left_out_of_the_reason() {
    let sent = sound_answered_with(
        HttpResponse::new(429, Vec::new())
            .with_header("retry-after", "soon; see https://x.test/?k=1"),
    );
    assert_eq!(
        sent,
        Sent::not_received("ElevenLabs sound generation returned HTTP 429")
    );
}

// ---------------------------------------------------------------------------------------------
// 8. and 12. Transport failures and the response cap

#[test]
fn transport_failures_carry_their_phase() {
    let fail = |error: TransportError| {
        sound_with(
            vec![expect_sound(DOOR).fail(error)],
            json!({"prompt": "wooden door opens"}),
        )
        .0
    };
    assert_eq!(
        fail(TransportError::not_sent(
            TransportErrorKind::Connect,
            "could not connect to https://api.elevenlabs.io"
        )),
        Sent::not_received(
            "ElevenLabs sound generation was not sent: could not connect to https://api.elevenlabs.io"
        )
    );
    assert_eq!(
        fail(TransportError::not_sent(
            TransportErrorKind::Timeout,
            "connect timed out"
        )),
        Sent::not_received("ElevenLabs sound generation was not sent: connect timed out")
    );
    assert_eq!(
        fail(TransportError::not_sent(
            TransportErrorKind::Refused,
            "the network is off (GRIDA_FX_NETWORK=off)"
        )),
        Sent::Refused {
            reason: "ElevenLabs sound generation was not sent: the network is off (GRIDA_FX_NETWORK=off)"
                .into()
        }
    );
    for (kind, reason) in [
        (TransportErrorKind::Timeout, "read timed out"),
        (TransportErrorKind::Other, "connection reset"),
        (TransportErrorKind::Other, "the body was truncated"),
    ] {
        assert_eq!(
            fail(TransportError::after_send(kind, reason)),
            Sent::Failed {
                reason: format!(
                    "ElevenLabs sound generation failed after the request left: {reason}"
                ),
                cost: None,
                retryable: true,
            }
        );
    }
    // A transport reason is redacted like every other.
    let sent = fail(TransportError::after_send(
        TransportErrorKind::Other,
        "reset while sending xi-api-key test-elevenlabs-key",
    ));
    assert_clean(&sent);
    assert!(reason_of(&sent).contains("[redacted]"));
}

#[test]
fn a_response_over_the_cap_is_a_retryable_failure() {
    let (sent, transport) = sound_with(
        vec![Exchange {
            expect: expect_sound(DOOR),
            reply: Reply::Zeros {
                status: 200,
                headers: vec![("content-type".into(), "audio/mpeg".into())],
                len: MAX_RESPONSE_BYTES + 1,
            },
        }],
        json!({"prompt": "wooden door opens"}),
    );
    assert_eq!(
        sent,
        Sent::Failed {
            reason: format!(
                "ElevenLabs sound generation failed after the request left: the response is larger than {MAX_RESPONSE_BYTES} bytes"
            ),
            cost: None,
            retryable: true,
        }
    );
    assert_eq!(transport.requests().len(), 1);
}

#[test]
fn the_network_switched_off_refuses_at_no_cost() {
    let transport = Arc::new(grida_fx_providers::transport::Offline);
    let sent = block_on(sound(transport, test_keys()).send(&sound_call(json!({"prompt": "door"}))));
    assert_eq!(
        sent,
        Sent::Refused {
            reason: "ElevenLabs sound generation was not sent: the network is off (GRIDA_FX_NETWORK=off)"
                .into()
        }
    );
}

#[test]
fn the_labels_are_the_spec_s() {
    assert_eq!(elevenlabs::sound::LABEL, SOUND_LABEL);
    assert_eq!(elevenlabs::speech::LABEL, SPEECH_LABEL);
    assert_eq!(elevenlabs::OUTPUT_FORMAT, "mp3_44100_192");
}
