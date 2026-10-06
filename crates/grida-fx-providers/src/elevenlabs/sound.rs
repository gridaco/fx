//! ElevenLabs sound effects: `POST {base}/sound-generation?output_format=mp3_44100_192` with
//! `{"text": <prompt>, "model_id": <route model>, "loop", "duration_seconds"?,
//! "prompt_influence"?}` (spec/providers.md §9.5). Refused before sending: a blank prompt or one
//! over 450 characters, `duration` outside 0.5..30, `prompt_influence` outside 0..1, no key.
//!
//! The prompt goes out verbatim (not trimmed); `loop` is always sent (false when absent);
//! `duration` and `prompt_influence` are sent only when given, as floats (`1` goes out as `1.0`),
//! so the provider's defaults apply otherwise. Characters are counted as Unicode scalar values.

use super::{CREDENTIAL_HEADER, check_route, float, number, post, refused, within};
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, RequestAdapter, Sent};
use crate::capabilities;
use crate::setup::Client;
use serde_json::{Map, Value};

/// The label of every reason.
pub const LABEL: &str = "ElevenLabs sound generation";

/// The capability this adapter serves.
pub const CAPABILITY: &str = "sound.generate";

/// The contract's `adapter` (spec/providers.md §9.5).
pub const ADAPTER: &str = "elevenlabs-sound-effect";

pub const MAX_PROMPT_CHARS: usize = 450;
pub const DURATION_SECONDS: std::ops::RangeInclusive<f64> = 0.5..=30.0;
pub const PROMPT_INFLUENCE: std::ops::RangeInclusive<f64> = 0.0..=1.0;

/// ElevenLabs' sound-effect adapter (module doc).
#[derive(Debug, Clone)]
pub struct ElevenLabsSound {
    client: Client,
}

impl ElevenLabsSound {
    pub fn new(client: Client) -> ElevenLabsSound {
        ElevenLabsSound { client }
    }
}

impl RequestAdapter for ElevenLabsSound {
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
            let body = match body(call) {
                Ok(body) => body,
                Err(reason) => return refused(&self.client, &reason),
            };
            post(&self.client, LABEL, "sound-generation", credential, body).await
        })
    }

    fn check(&self, _call: &CallRequest, answer: &Answer) -> Result<(), String> {
        super::check_audio(&self.client, LABEL, answer)
    }
}

/// The wire body of a request whose shape `capabilities::check_request` accepted, or the
/// refusal of its first out-of-range value (spec/providers.md §9.5 "Sound").
fn body(call: &CallRequest) -> Result<Map<String, Value>, String> {
    let request = &call.request;
    let prompt = request.get("prompt").and_then(Value::as_str).unwrap_or("");
    if prompt.trim().is_empty() {
        return Err("a sound call needs its prompt as text".into());
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!(
            "sound effect prompt must be at most {MAX_PROMPT_CHARS} characters"
        ));
    }
    let duration = number(request, "duration");
    if duration.is_some_and(|seconds| !within(seconds, &DURATION_SECONDS)) {
        return Err("duration must be between 0.5 and 30".into());
    }
    let influence = number(request, "prompt_influence");
    if influence.is_some_and(|influence| !within(influence, &PROMPT_INFLUENCE)) {
        return Err("prompt_influence must be between 0 and 1".into());
    }
    let looped = request
        .get("loop")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut body = Map::new();
    body.insert("text".into(), Value::from(prompt));
    body.insert("model_id".into(), Value::from(call.route.model.as_str()));
    body.insert("loop".into(), Value::Bool(looped));
    if let Some(seconds) = duration {
        body.insert("duration_seconds".into(), float(seconds));
    }
    if let Some(influence) = influence {
        body.insert("prompt_influence".into(), float(influence));
    }
    Ok(body)
}
