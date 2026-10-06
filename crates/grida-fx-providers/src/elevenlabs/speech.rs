//! ElevenLabs speech: `POST {base}/text-to-speech/<voice>?output_format=mp3_44100_192` with
//! `{"text", "model_id": <route model>, "voice_settings"?: {"stability"}, "language_code"?}`
//! (spec/providers.md §9.5). Refused before sending: blank text or over 5000 characters, a voice
//! that is not `^[A-Za-z0-9_-]{1,128}$`, `stability` outside 0..1, no key. `language_code` null or
//! `""` is not sent; `max_chars` (pricing only) is never sent.
//!
//! The text goes out verbatim, delivery tags included. The voice is checked, not encoded: it is a
//! path segment, so anything outside the voice-id alphabet (`/`, `?`, `#`, spaces) is refused
//! rather than allowed to change the endpoint. A `language_code` that is blank but not empty is
//! refused (`language_code must not be blank`): the provider would refuse it after billing the
//! hold. `voice_settings` holds `stability` alone, as a float.

use super::{CREDENTIAL_HEADER, check_route, float, number, post, refused, within};
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, RequestAdapter, Sent};
use crate::capabilities;
use crate::setup::Client;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::LazyLock;

/// The label of every reason.
pub const LABEL: &str = "ElevenLabs speech generation";

/// The capability this adapter serves.
pub const CAPABILITY: &str = "speech.generate";

/// The contract's `adapter` (spec/providers.md §9.5).
pub const ADAPTER: &str = "elevenlabs-speech";

pub const MAX_TEXT_CHARS: usize = 5000;
pub const STABILITY: std::ops::RangeInclusive<f64> = 0.0..=1.0;

/// What a voice id may be: it becomes a path segment as it is.
pub static VOICE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,128}$").expect("a valid pattern"));

/// ElevenLabs' speech adapter (module doc).
#[derive(Debug, Clone)]
pub struct ElevenLabsSpeech {
    client: Client,
}

impl ElevenLabsSpeech {
    pub fn new(client: Client) -> ElevenLabsSpeech {
        ElevenLabsSpeech { client }
    }
}

impl RequestAdapter for ElevenLabsSpeech {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            // spec/providers.md §5, steps 1 to 3.
            if let Err(reason) = check_route(call, CAPABILITY, ADAPTER)
                .and_then(|()| capabilities::check_request(CAPABILITY, &call.request))
            {
                return refused(&self.client, &reason);
            }
            let credential = match self.client.credential(CREDENTIAL_HEADER, "") {
                Ok(credential) => credential,
                Err(reason) => return refused(&self.client, &reason),
            };
            // Step 4: the values.
            let (voice, body) = match body(call) {
                Ok(checked) => checked,
                Err(reason) => return refused(&self.client, &reason),
            };
            let path = format!("text-to-speech/{voice}");
            post(&self.client, LABEL, &path, credential, body).await
        })
    }

    fn check(&self, _call: &CallRequest, answer: &Answer) -> Result<(), String> {
        super::check_audio(&self.client, LABEL, answer)
    }
}

/// The voice and the wire body of a request whose shape `capabilities::check_request` accepted,
/// or the refusal of its first out-of-range value (spec/providers.md §9.5 "Speech").
fn body(call: &CallRequest) -> Result<(&str, Map<String, Value>), String> {
    let request = &call.request;
    let text = request.get("text").and_then(Value::as_str).unwrap_or("");
    if text.trim().is_empty() {
        return Err("a speech call needs its text".into());
    }
    let voice = request.get("voice").and_then(Value::as_str).unwrap_or("");
    if voice.trim().is_empty() {
        return Err("a speech call needs its provider voice".into());
    }
    if !VOICE_ID.is_match(voice) {
        return Err("the provider voice is not a voice id".into());
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(format!(
            "speech text must be at most {MAX_TEXT_CHARS} characters"
        ));
    }
    let stability = number(request, "stability");
    if stability.is_some_and(|stability| !within(stability, &STABILITY)) {
        return Err("stability must be between 0 and 1".into());
    }
    let language = request
        .get("language_code")
        .and_then(Value::as_str)
        .filter(|code| !code.is_empty());
    if language.is_some_and(|code| code.trim().is_empty()) {
        return Err("language_code must not be blank".into());
    }
    let mut body = Map::new();
    body.insert("text".into(), Value::from(text));
    body.insert("model_id".into(), Value::from(call.route.model.as_str()));
    if let Some(stability) = stability {
        let mut settings = Map::new();
        settings.insert("stability".into(), float(stability));
        body.insert("voice_settings".into(), Value::Object(settings));
    }
    if let Some(code) = language {
        body.insert("language_code".into(), Value::from(code));
    }
    Ok((voice, body))
}
