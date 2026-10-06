//! ElevenLabs: `sound.generate` and `speech.generate`, both one synchronous request answered with
//! raw MP3 bytes (spec/providers.md §9.5; spec/capabilities.md §10–§11).
//!
//! Credential `xi-api-key: <ELEVENLABS_API_KEY>`; base `ELEVENLABS_BASE_URL`, default
//! `https://api.elevenlabs.io/v1`; query `output_format=mp3_44100_192`; headers
//! `content-type: application/json`, `accept: audio/mpeg`. Deadline 120 s; response cap 64 MiB.
//! The answer is file `audio` with the normalized media type (`audio/mpeg` when the header is
//! absent), `data: null`, `cost: None` (ElevenLabs reports characters, not dollars). `check`:
//! non-empty, `audio/mpeg`, MP3 signature, in that order. Status classes and reasons
//! (`ElevenLabs sound generation returned HTTP <n>`; never `detail.message`) are shared by both
//! adapters ([`classify`]).
//!
//! Both adapters refuse in the order of spec/providers.md §5: the route's contract
//! ([`check_route`]), the request's shape (`capabilities::check_request`), the key, then their own
//! values. Every reason, refusals included, goes through the client's redactor last.

pub mod sound;
pub mod speech;

use crate::adapter::{Adapter, Answer, CallRequest, Sent};
use crate::keys::KeyName;
use crate::registry::Adapters;
use crate::setup::{Client, Setup};
use crate::transport::{
    Body, HttpRequest, HttpResponse, Lane, Method, Phase, TransportError, TransportErrorKind,
};
use crate::wire;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// The query every request carries.
pub const OUTPUT_FORMAT: &str = "mp3_44100_192";

pub const DEADLINE: Duration = Duration::from_secs(120);
pub const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// The only kind both routes answer with.
pub const AUDIO_KIND: &str = "audio/mpeg";

/// The answer's one file.
pub const AUDIO_FILE: &str = "audio";

/// The credential header (no prefix).
pub const CREDENTIAL_HEADER: &str = "xi-api-key";

/// The variable that moves the base URL, named when a response redirects (spec/providers.md §4.3).
pub const BASE_URL_VARIABLE: &str = "ELEVENLABS_BASE_URL";

/// The contract's `adapter_behavior` both adapters serve (spec/providers.md §10: an integer on
/// every non-image route).
pub const ADAPTER_BEHAVIOR: u64 = 1;

/// The kind of an answer whose `content-type` is not a media type at all (RFC 9110 §8.3).
const UNKNOWN_KIND: &str = "application/octet-stream";

/// The shared client.
pub fn client(setup: &Setup) -> Client {
    setup.client(KeyName::ElevenLabs, &setup.endpoints.elevenlabs)
}

/// An exchange's outcome (spec/providers.md §9.5 table): a 2xx becomes the answer (file `audio`),
/// anything else a [`Sent`]. `label` is `ElevenLabs sound generation` or `ElevenLabs speech
/// generation`.
///
/// - a transport failure (spec/providers.md §4.2): refused by the transport → `Refused`;
///   otherwise `not_sent` → `NotReceived`; `after_send` (a timeout, a reset, a body over the
///   cap) → `Failed { cost: None, retryable: true }`;
/// - 2xx → `Answered`: the body as file `audio`, its kind the normalized `content-type`
///   (`audio/mpeg` when absent; a type outside the audio family is kept as its lowercased base
///   type so that [`check_audio`] refuses it), `data: null`, `cost: None`;
/// - 3xx → `Failed { cost: None, retryable: false }`: `<label> was redirected (HTTP <n>); check
///   ELEVENLABS_BASE_URL`;
/// - 408 and 429 → `NotReceived` (a `retry-after` header goes into the reason);
/// - any other 4xx → `Failed { cost: None, retryable: false }` (ElevenLabs does not document its
///   refusals as unbilled);
/// - 5xx, and any status outside 200–599 → `Failed { cost: None, retryable: true }`.
///
/// Status reasons are `<label> returned HTTP <n>`, plus `: <detail.status>` when the error body's
/// `detail.status` is text matching `^[A-Za-z0-9_.:-]{1,96}$` after redaction. The body's
/// `detail.message`, or a `detail` that is text, is never quoted.
pub fn classify(
    client: &Client,
    label: &str,
    exchange: Result<HttpResponse, TransportError>,
) -> Sent {
    let response = match exchange {
        Ok(response) => response,
        Err(error) => return transport_failure(client, label, &error),
    };
    let status = response.status;
    if response.is_success() {
        let kind = audio_kind(&response);
        return Sent::Answered(Answer::new(Value::Null, None).with_file(
            AUDIO_FILE,
            &kind,
            response.body,
        ));
    }
    if (300..400).contains(&status) {
        return Sent::Failed {
            reason: client.reason(&format!(
                "{label} was redirected (HTTP {status}); check {BASE_URL_VARIABLE}"
            )),
            cost: None,
            retryable: false,
        };
    }
    let detail = detail_status(client, &response.body);
    let reason = wire::status_reason(label, status, detail.as_deref());
    match status {
        408 | 429 => {
            let wait = crate::adapter::retry_after(&response);
            let reason = match retry_after_text(&response) {
                Some(text) => format!("{reason}; retry-after {text}"),
                None => reason,
            };
            not_received(client.reason(&reason), wait)
        }
        400..=499 => Sent::Failed {
            reason: client.reason(&reason),
            cost: None,
            retryable: false,
        },
        _ => Sent::Failed {
            reason: client.reason(&reason),
            cost: None,
            retryable: true,
        },
    }
}

/// The checks after an answer (module doc), with `label` in the empty-audio sentence:
/// `<label> returned no audio data`, `requested mp3 but received <kind>`, `audio bytes do not
/// match declared media type audio/mpeg`, in that order. The kind comes from the provider's
/// `content-type`, which may echo anything, a key included: every sentence goes through the
/// client's redactor (spec/providers.md §8).
pub fn check_audio(client: &Client, label: &str, answer: &Answer) -> Result<(), String> {
    let Some(audio) = answer
        .files
        .get(AUDIO_FILE)
        .filter(|file| !file.bytes.is_empty())
    else {
        return Err(client.reason(&format!("{label} returned no audio data")));
    };
    if audio.kind != AUDIO_KIND {
        let kind = crate::redact::bounded(&client.redactor.redact(&audio.kind), MAX_KIND_CHARS);
        return Err(client.reason(&format!("requested mp3 but received {kind}")));
    }
    if !wire::matches_signature(AUDIO_KIND, &audio.bytes) {
        return Err(client.reason(&format!(
            "audio bytes do not match declared media type {AUDIO_KIND}"
        )));
    }
    Ok(())
}

/// Registers both adapters on provider `elevenlabs`.
pub fn register(adapters: &mut Adapters, setup: &Setup) {
    let client = client(setup);
    adapters.register(
        "sound.generate",
        "elevenlabs",
        Adapter::Request(Arc::new(sound::ElevenLabsSound::new(client.clone()))),
    );
    adapters.register(
        "speech.generate",
        "elevenlabs",
        Adapter::Request(Arc::new(speech::ElevenLabsSpeech::new(client))),
    );
}

/// spec/providers.md §5 step 1: the call is on `capability`, and the route's contract, when it
/// names an `adapter` or an `adapter_behavior`, names `adapter` and [`ADAPTER_BEHAVIOR`]. An
/// absent contract (`{}` or `null`) is served, and a `null` member counts as absent. Refused with `<route> is not a <capability> route
/// this adapter serves`.
pub fn check_route(call: &CallRequest, capability: &str, adapter: &str) -> Result<(), String> {
    let route = &call.route;
    let refused = || {
        Err(format!(
            "{} is not a {capability} route this adapter serves",
            route.id()
        ))
    };
    if route.capability != capability {
        return refused();
    }
    let contract = match &route.contract {
        Value::Null => return Ok(()),
        Value::Object(contract) => contract,
        _ => return refused(),
    };
    let present = |name: &str| contract.get(name).filter(|value| !value.is_null());
    if present("adapter").is_some_and(|name| name.as_str() != Some(adapter)) {
        return refused();
    }
    if present("adapter_behavior")
        .is_some_and(|behavior| behavior.as_u64() != Some(ADAPTER_BEHAVIOR))
    {
        return refused();
    }
    Ok(())
}

/// A request member that `capabilities::check_request` typed as a number: `None` when absent or
/// `null`.
pub(crate) fn number(request: &Value, name: &str) -> Option<f64> {
    request.get(name).and_then(Value::as_f64)
}

/// Whether a number is finite and inside `range` (bounds included).
pub(crate) fn within(value: f64, range: &std::ops::RangeInclusive<f64>) -> bool {
    value.is_finite() && range.contains(&value)
}

/// A JSON number that is always written as a float (`1` goes out as `1.0`), as the reference
/// engine's clients sent these members.
pub(crate) fn float(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// Sends the request body once to `<base>/<path>?output_format=mp3_44100_192` and classifies the
/// exchange.
pub(crate) async fn post(
    client: &Client,
    label: &str,
    path: &str,
    credential: crate::transport::Credential,
    body: Map<String, Value>,
) -> Sent {
    let url = format!("{}?output_format={OUTPUT_FORMAT}", client.url(path));
    let request = HttpRequest::new(Method::Post, url, Lane::Provider)
        .credential(credential)
        .header("content-type", "application/json")
        .header("accept", AUDIO_KIND)
        .body(Body::Json(Value::Object(body)))
        .timeout(DEADLINE)
        .max_response_bytes(MAX_RESPONSE_BYTES);
    let exchange = client.send(request).await;
    classify(client, label, exchange)
}

/// A refusal before sending, redacted.
pub(crate) fn refused(client: &Client, reason: &str) -> Sent {
    Sent::Refused {
        reason: client.reason(reason),
    }
}

/// `NotReceived`, with the provider's `retry-after` when it gave one in seconds.
///
/// The engine waits at least `retry_after` before resending (spec/providers.md §4.3); the reason
/// names it too.
fn not_received(reason: String, retry_after: Option<Duration>) -> Sent {
    Sent::NotReceived {
        reason,
        retry_after,
    }
}

/// spec/providers.md §4.2.
fn transport_failure(client: &Client, label: &str, error: &TransportError) -> Sent {
    if error.kind == TransportErrorKind::Refused {
        return Sent::Refused {
            reason: client.reason(&format!("{label} was not sent: {}", error.reason)),
        };
    }
    match error.phase {
        Phase::NotSent => not_received(
            client.reason(&format!("{label} was not sent: {}", error.reason)),
            None,
        ),
        Phase::AfterSend => Sent::Failed {
            reason: client.reason(&format!(
                "{label} failed after the request left: {}",
                error.reason
            )),
            cost: None,
            retryable: true,
        },
    }
}

/// The longest kind a reason shows.
const MAX_KIND_CHARS: usize = 96;

static SAFE_FIELD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.:-]{1,96}$").expect("a valid pattern"));

static MEDIA_TYPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9!#$&^_.+-]{1,47}/[a-z0-9!#$&^_.+-]{1,48}$").expect("a valid pattern")
});

static RETRY_AFTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9 ,:+-]{1,40}$").expect("a valid pattern"));

/// The answer's kind (spec/providers.md §9.5 "Answer"): `audio/mpeg` without a `content-type`;
/// the normalized audio type; else the lowercased base type when it is a media type, and
/// `application/octet-stream` when it is not one, so that the check refuses either with a
/// sentence that holds no header text beyond a media type.
fn audio_kind(response: &HttpResponse) -> String {
    let Some(declared) = response.header("content-type") else {
        return AUDIO_KIND.to_string();
    };
    if let Ok(kind) = wire::normalize_media_type(declared, "audio") {
        return kind;
    }
    let base = declared
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if MEDIA_TYPE.is_match(&base) {
        base
    } else {
        UNKNOWN_KIND.to_string()
    }
}

/// The error body's `detail.status`, redacted, when it is a safe field. ElevenLabs answers errors
/// as `{"detail": {"status": "…", "message": "…"}}` (or a `detail` that is text, or a list);
/// only `status` is ever shown.
fn detail_status(client: &Client, body: &[u8]) -> Option<String> {
    let Ok(Value::Object(payload)) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    let status = payload.get("detail")?.get("status")?.as_str()?;
    let status = client.redactor.redact(status);
    SAFE_FIELD.is_match(&status).then_some(status)
}

/// The `retry-after` header as the reason shows it: seconds or an HTTP date, nothing else.
fn retry_after_text(response: &HttpResponse) -> Option<String> {
    let value = response.header("retry-after")?.trim();
    RETRY_AFTER.is_match(value).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::RouteRef;
    use crate::keys::Keys;
    use crate::testing::{setup, test_keys};
    use crate::transport::replay::NoNetwork;
    use indexmap::IndexMap;
    use serde_json::json;

    fn test_client() -> Client {
        client(&setup(Arc::new(NoNetwork), test_keys()))
    }

    fn call(capability: &str, contract: Value) -> CallRequest {
        CallRequest {
            route: RouteRef {
                capability: capability.into(),
                model: "eleven_text_to_sound_v2".into(),
                provider: "elevenlabs".into(),
                contract,
            },
            request: json!({}),
            files: IndexMap::new(),
            take: vec![1],
            key: "0".repeat(64),
            attempt: 1,
        }
    }

    #[test]
    fn routes_are_served_by_their_adapter_name_and_behavior() {
        let serves = |capability: &str, contract: Value| {
            check_route(
                &call(capability, contract),
                "sound.generate",
                "elevenlabs-sound-effect",
            )
        };
        let refusal = Err(
            "eleven_text_to_sound_v2@elevenlabs is not a sound.generate route this adapter serves"
                .to_string(),
        );
        assert_eq!(serves("sound.generate", json!({})), Ok(()));
        assert_eq!(serves("sound.generate", Value::Null), Ok(()));
        assert_eq!(
            serves(
                "sound.generate",
                json!({"adapter": "elevenlabs-sound-effect", "adapter_behavior": 1})
            ),
            Ok(())
        );
        assert_eq!(
            serves(
                "sound.generate",
                json!({"adapter": "elevenlabs-sound-effect"})
            ),
            Ok(())
        );
        assert_eq!(
            serves("sound.generate", json!({"adapter": "elevenlabs-speech"})),
            refusal
        );
        assert_eq!(
            serves(
                "sound.generate",
                json!({"adapter": "elevenlabs-sound-effect", "adapter_behavior": 2})
            ),
            refusal
        );
        assert_eq!(
            serves(
                "sound.generate",
                json!({"adapter": "elevenlabs-sound-effect", "adapter_behavior": "1"})
            ),
            refusal
        );
        assert_eq!(serves("sound.generate", json!({"adapter": null})), Ok(()));
        assert_eq!(serves("sound.generate", json!({"adapter": 7})), refusal);
        assert_eq!(serves("sound.generate", json!([])), refusal);
        assert_eq!(
            serves("speech.generate", json!({})),
            Err(
                "eleven_text_to_sound_v2@elevenlabs is not a sound.generate route this adapter serves"
                    .to_string()
            )
        );
    }

    #[test]
    fn numbers_are_read_and_bounded() {
        let request = json!({"a": 1, "b": 0.25, "c": null});
        assert_eq!(number(&request, "a"), Some(1.0));
        assert_eq!(number(&request, "b"), Some(0.25));
        assert_eq!(number(&request, "c"), None);
        assert_eq!(number(&request, "d"), None);
        assert!(within(0.5, &(0.5..=30.0)));
        assert!(within(30.0, &(0.5..=30.0)));
        assert!(!within(0.49, &(0.5..=30.0)));
        assert!(!within(f64::NAN, &(0.0..=1.0)));
        assert!(!within(f64::INFINITY, &(0.0..=f64::INFINITY)));
        assert_eq!(serde_json::to_string(&float(1.0)).unwrap(), "1.0");
        assert_eq!(serde_json::to_string(&float(0.6)).unwrap(), "0.6");
        assert_eq!(float(f64::NAN), Value::Null);
    }

    #[test]
    fn kinds_come_from_the_content_type() {
        let kind = |header: Option<&str>| {
            let mut response = HttpResponse::new(200, b"ID3".to_vec());
            if let Some(header) = header {
                response = response.with_header("content-type", header);
            }
            audio_kind(&response)
        };
        assert_eq!(kind(None), "audio/mpeg");
        assert_eq!(kind(Some("audio/mpeg")), "audio/mpeg");
        assert_eq!(kind(Some(" Audio/MPEG; charset=binary")), "audio/mpeg");
        assert_eq!(kind(Some("audio/mp3")), "audio/mpeg");
        assert_eq!(kind(Some("audio/mpeg3")), "audio/mpeg");
        assert_eq!(kind(Some("audio/wav")), "audio/wav");
        assert_eq!(
            kind(Some("Application/JSON; charset=utf-8")),
            "application/json"
        );
        assert_eq!(kind(Some("")), "application/octet-stream");
        assert_eq!(kind(Some("not a type")), "application/octet-stream");
        assert_eq!(kind(Some("audio/")), "application/octet-stream");
    }

    #[test]
    fn only_a_safe_detail_status_is_shown() {
        let client = test_client();
        let detail = |body: Value| detail_status(&client, &serde_json::to_vec(&body).unwrap());
        assert_eq!(
            detail(json!({"detail": {"status": "quota_exceeded", "message": "secret quota"}})),
            Some("quota_exceeded".into())
        );
        assert_eq!(
            detail(json!({"detail": {"status": "has spaces"}})),
            None,
            "not a safe field"
        );
        assert_eq!(
            detail(json!({"detail": {"status": "test-elevenlabs-key"}})),
            None,
            "a key is redacted and then no longer a safe field"
        );
        assert_eq!(detail(json!({"detail": "voice not found: secret"})), None);
        assert_eq!(detail(json!({"detail": [{"msg": "field required"}]})), None);
        assert_eq!(detail(json!({"detail": {"status": 7}})), None);
        assert_eq!(detail_status(&client, b"not json"), None);
    }

    #[test]
    fn retry_after_is_shown_when_safe() {
        let response =
            |value: &str| HttpResponse::new(429, Vec::new()).with_header("Retry-After", value);
        assert_eq!(retry_after_text(&response("5")), Some("5".into()));
        assert_eq!(
            retry_after_text(&response("Wed, 21 Oct 2015 07:28:00 GMT")),
            Some("Wed, 21 Oct 2015 07:28:00 GMT".into())
        );
        assert_eq!(retry_after_text(&response("5; https://x.test/?k=v")), None);
    }

    #[test]
    fn a_missing_key_is_named_by_its_variable() {
        let client = client(&setup(Arc::new(NoNetwork), Keys::none()));
        assert_eq!(
            client.credential(CREDENTIAL_HEADER, "").unwrap_err(),
            "ELEVENLABS_API_KEY is not set"
        );
        assert_eq!(client.base, "https://api.elevenlabs.io/v1");
    }
}
