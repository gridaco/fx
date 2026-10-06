//! OpenRouter music (Lyria): `POST {base}/chat/completions` with `{"model", "messages": [{"role":
//! "user", "content": <prompt>}], "modalities": ["text", "audio"], "audio": {"format": "mp3"},
//! "stream": true}` (spec/providers.md §9.2; spec/capabilities.md §9). `duration` is never sent:
//! it stays in the call key only.
//!
//! Refusals, in the order of spec/providers.md §5: the contract (`adapter: openrouter-music`,
//! `adapter_behavior: 1`, each checked when present); the capability's shape; the key; a blank prompt
//! (`a music call needs its prompt as text`); a serialized body over
//! [`super::MAX_REQUEST_BYTES`].
//!
//! The response is SSE when its `content-type` names `text/event-stream` or any line starts
//! (after blanks) with `data:`; otherwise it is one buffered JSON object. In a stream only lines
//! that start with `data:` count, so `:` keep-alive comments are skipped, and so are blank
//! payloads and `[DONE]`. Every payload is one JSON object; an error event fails the attempt, and
//! so does a stream with no event. From each event (or the buffered object) the adapter
//! collects audio from `choices[].delta|message` (`audio`, and `audio`/`output_audio`/
//! `input_audio` blocks in a list `content`), `steps[]` of type `model_output`, `output[].content`
//! and a top-level `output_audio`. An audio object's `data` is a data URL or raw base64, and its
//! `media_type`, `mime_type`, `content_type` and `format` declare its type. The chunks are
//! concatenated and strictly decoded once; the declared types must agree and be `audio/mpeg` (the
//! default when none is declared). Text the model returns is read past: the answer's `data` is
//! `null`.
//!
//! The answer is file `audio` (`audio/mpeg`), `data: null`, and `cost` from the last `usage`
//! seen, an error event's included (spec/providers.md §6). A 2xx that cannot become an answer is
//! `Failed { cost, retryable: true }` with that cost when one was seen (spec/providers.md §4.4).
//! Deadline 900 s; response cap 128 MiB. `check`: the MP3 signature.

use super::{BODY_TOO_LARGE, body_exceeds, classify, not_served, serves, with_request_id};
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, RequestAdapter, Sent};
use crate::redact::Redactor;
use crate::setup::Client;
use crate::transport::{Body, HttpRequest, HttpResponse, Lane, Method};
use crate::wire;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::sync::LazyLock;
use std::time::Duration;

/// The label of every reason.
pub const LABEL: &str = "OpenRouter music generation";

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "openrouter-music";

pub const DEADLINE: Duration = Duration::from_secs(900);
pub const MAX_RESPONSE_BYTES: u64 = 128 * 1024 * 1024;

/// The contract `adapter_behavior` this adapter serves.
const CONTRACT_BEHAVIOR: f64 = 1.0;

/// The only output format the route asks for, and its media type.
const FORMAT: &str = "mp3";
const KIND: &str = "audio/mpeg";

/// A line that makes a response a stream: `data:` after optional blanks.
static DATA_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*data:").expect("a valid pattern"));

/// A declared media type that may be quoted in a reason.
static SAFE_MEDIA_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9!#$&^_.+/-]{1,96}$").expect("a valid pattern"));

/// OpenRouter's music adapter (module doc).
#[derive(Debug, Clone)]
pub struct OpenRouterMusic {
    client: Client,
}

impl OpenRouterMusic {
    pub fn new(client: Client) -> OpenRouterMusic {
        OpenRouterMusic { client }
    }

    /// Every refusal of spec/providers.md §5, in order, then the request to send.
    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let route = &call.route;
        if route.capability != "music.generate"
            || !serves(&route.contract, CONTRACT_ADAPTER, |v| {
                v.is_number() && v.as_f64() == Some(CONTRACT_BEHAVIOR)
            })
        {
            return Err(not_served(call));
        }
        crate::capabilities::check_request(&route.capability, &call.request)?;
        let credential = self.client.credential("authorization", "Bearer ")?;
        let prompt = call
            .request
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or("");
        if prompt.trim().is_empty() {
            return Err("a music call needs its prompt as text".into());
        }
        let body = json!({
            "model": route.model.trim(),
            "messages": [{"role": "user", "content": prompt}],
            "modalities": ["text", "audio"],
            "audio": {"format": FORMAT},
            "stream": true,
        });
        if body_exceeds(&body, super::MAX_REQUEST_BYTES) {
            return Err(BODY_TOO_LARGE.into());
        }
        Ok(HttpRequest::new(
            Method::Post,
            self.client.url("chat/completions"),
            Lane::Provider,
        )
        .credential(credential)
        .body(Body::Json(body))
        .timeout(DEADLINE)
        .max_response_bytes(MAX_RESPONSE_BYTES))
    }

    /// A 2xx response as an answer, or a structural failure (spec/providers.md §4.4).
    fn answer(&self, response: &HttpResponse) -> Sent {
        let mut track = Track::default();
        match track
            .read(response, &self.client.redactor)
            .and_then(|()| track.finish())
        {
            Ok(bytes) => Sent::Answered(
                Answer::new(Value::Null, track.cost()).with_file("audio", KIND, bytes),
            ),
            Err(reason) => Sent::Failed {
                reason: self.client.reason(&with_request_id(reason, response)),
                cost: track.cost(),
                retryable: true,
            },
        }
    }
}

impl RequestAdapter for OpenRouterMusic {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            let request = match self.prepare(call) {
                Ok(request) => request,
                Err(reason) => {
                    return Sent::Refused {
                        reason: self.client.reason(&reason),
                    };
                }
            };
            match classify(&self.client, LABEL, self.client.send(request).await) {
                Ok(response) => self.answer(&response),
                Err(sent) => sent,
            }
        })
    }

    fn check(&self, _call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(audio) = answer.files.get("audio") else {
            return Err("the answer has no audio".into());
        };
        if wire::matches_signature(&audio.kind, &audio.bytes) {
            Ok(())
        } else {
            Err(format!(
                "audio bytes do not match declared media type {}",
                audio.kind
            ))
        }
    }
}

/// What a response held: base64 chunks in order, the media types declared, and the last `usage`.
#[derive(Debug, Default)]
struct Track {
    chunks: Vec<String>,
    media_types: Vec<String>,
    usage: Option<Map<String, Value>>,
}

impl Track {
    /// The reported cost: the last `usage.cost` seen.
    fn cost(&self) -> Option<grida_fx_core::money::Usd> {
        self.usage
            .as_ref()
            .and_then(|usage| usage.get("cost"))
            .and_then(wire::usd_ceil)
    }

    /// Reads the response, as a stream or as one buffered object.
    fn read(&mut self, response: &HttpResponse, redactor: &Redactor) -> Result<(), String> {
        let text = String::from_utf8_lossy(&response.body);
        if text.trim().is_empty() {
            return Err(format!("{LABEL} returned an empty response"));
        }
        let streamed = response
            .header("content-type")
            .is_some_and(|t| t.to_ascii_lowercase().contains("text/event-stream"));
        if streamed || DATA_LINE.is_match(&text) {
            return self.stream(&text, redactor);
        }
        let payload = wire::json_object(&response.body, LABEL)?;
        if let Some(Value::Object(usage)) = payload.get("usage") {
            self.usage = Some(usage.clone());
        }
        self.collect(&payload)
    }

    /// The SSE events of `text` (module doc). A `usage` is kept before anything else in its event
    /// is judged, so a cost the provider reported is never dropped.
    fn stream(&mut self, text: &str, redactor: &Redactor) -> Result<(), String> {
        let mut events = 0usize;
        for line in text.split(['\n', '\r']) {
            let Some(payload) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            let Ok(event) = serde_json::from_str::<Value>(payload) else {
                return Err("OpenRouter music stream contained invalid JSON".into());
            };
            let Value::Object(event) = event else {
                return Err("OpenRouter music stream contained a non-object event".into());
            };
            if let Some(Value::Object(usage)) = event.get("usage") {
                self.usage = Some(usage.clone());
            }
            if event.get("type").and_then(Value::as_str) == Some("error")
                || event.get("error").is_some_and(Value::is_object)
            {
                let detail = wire::safe_error_detail(payload.as_bytes(), redactor);
                return Err(match detail {
                    Some(detail) => format!("OpenRouter music stream reported an error: {detail}"),
                    None => "OpenRouter music stream reported an error".into(),
                });
            }
            events += 1;
            self.collect(&event)?;
        }
        if events == 0 {
            return Err("OpenRouter music stream contained no events".into());
        }
        Ok(())
    }

    /// The audio of one event or buffered object.
    fn collect(&mut self, payload: &Map<String, Value>) -> Result<(), String> {
        for choice in array(payload.get("choices")) {
            for holder in ["delta", "message"] {
                let Some(holder) = choice.get(holder).and_then(Value::as_object) else {
                    continue;
                };
                if let Some(Value::Array(blocks)) = holder.get("content") {
                    self.blocks(blocks)?;
                }
                if let Some(Value::Object(audio)) = holder.get("audio") {
                    self.audio(audio)?;
                }
            }
        }
        for step in array(payload.get("steps")) {
            if step.get("type").and_then(Value::as_str) == Some("model_output")
                && let Some(Value::Array(blocks)) = step.get("content")
            {
                self.blocks(blocks)?;
            }
        }
        for output in array(payload.get("output")) {
            if let Some(Value::Array(blocks)) = output.get("content") {
                self.blocks(blocks)?;
            }
        }
        if let Some(Value::Object(audio)) = payload.get("output_audio") {
            self.audio(audio)?;
        }
        Ok(())
    }

    /// Content blocks: `audio`, `output_audio` and `input_audio` blocks carry audio, in a nested
    /// `audio` object when there is one, else in the block itself. Text blocks are read past.
    fn blocks(&mut self, blocks: &[Value]) -> Result<(), String> {
        for block in blocks.iter().filter_map(Value::as_object) {
            let kind = block.get("type").and_then(Value::as_str);
            if matches!(kind, Some("audio" | "output_audio" | "input_audio")) {
                match block.get("audio") {
                    Some(Value::Object(nested)) => self.audio(nested)?,
                    _ => self.audio(block)?,
                }
            }
        }
        Ok(())
    }

    /// One audio object: its `data` (a data URL or raw base64) and its declared media types.
    fn audio(&mut self, audio: &Map<String, Value>) -> Result<(), String> {
        if let Some(Value::String(data)) = audio.get("data")
            && !data.is_empty()
        {
            match wire::parse_data_url(data) {
                Some((media, payload)) => {
                    self.media_types.push(media_type(&media)?);
                    self.chunks.push(payload);
                }
                None => self.chunks.push(data.clone()),
            }
        }
        for key in ["media_type", "mime_type", "content_type", "format"] {
            if let Some(Value::String(value)) = audio.get(key)
                && !value.is_empty()
            {
                self.media_types.push(media_type(value)?);
            }
        }
        Ok(())
    }

    /// The decoded audio: the chunks concatenated, then strictly decoded once, after the media
    /// types are found to agree on MP3.
    fn finish(&self) -> Result<Vec<u8>, String> {
        if self.chunks.is_empty() {
            return Err(format!("{LABEL} returned no audio data"));
        }
        let mut declared: Vec<&str> = Vec::new();
        for media in &self.media_types {
            if !declared.contains(&media.as_str()) {
                declared.push(media);
            }
        }
        if declared.len() > 1 {
            return Err(format!(
                "{LABEL} declared conflicting media types: {}",
                declared.join(", ")
            ));
        }
        let kind = declared.first().copied().unwrap_or(KIND);
        if kind != KIND {
            return Err(format!("{LABEL}: requested {FORMAT} but received {kind}"));
        }
        wire::strict_base64(&self.chunks.concat())
            .ok_or_else(|| "OpenRouter music audio data is not valid base64".to_string())
    }
}

/// The objects of a list member; nothing when the member is not a list.
fn array(value: Option<&Value>) -> impl Iterator<Item = &Map<String, Value>> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
}

/// A declared media type, normalized within `audio` ([`wire::normalize_media_type`]). The value
/// reaches a reason only when it looks like a media type.
fn media_type(value: &str) -> Result<String, String> {
    wire::normalize_media_type(value, "audio").map_err(|_| {
        let shown = value
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if SAFE_MEDIA_TYPE.is_match(&shown) {
            format!("{LABEL}: expected audio media type, received {shown}")
        } else {
            format!("{LABEL}: expected audio media type")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(body: &str, content_type: Option<&str>) -> (Result<Vec<u8>, String>, Track) {
        let mut response = HttpResponse::new(200, body.as_bytes().to_vec());
        if let Some(content_type) = content_type {
            response = response.with_header("content-type", content_type);
        }
        let mut track = Track::default();
        let result = track
            .read(&response, &Redactor::default())
            .and_then(|()| track.finish());
        (result, track)
    }

    #[test]
    fn streams_split_on_any_line_break() {
        let body = "data: {\"output_audio\": {\"data\": \"SUQz\"}}\r\n\r\n\
                    data:{\"usage\": {\"cost\": 0.5}}\rdata: [DONE]\r\n";
        let (result, track) = track(body, None);
        assert_eq!(result.unwrap(), b"ID3");
        assert_eq!(track.cost(), Some(grida_fx_core::money::Usd(500_000)));
    }

    #[test]
    fn only_lines_starting_with_data_count() {
        // An indented `data:` makes the body a stream, but the line itself is not an event.
        let (result, _) = track("  data: {\"output_audio\": {\"data\": \"SUQz\"}}\n", None);
        assert_eq!(
            result.unwrap_err(),
            "OpenRouter music stream contained no events"
        );
        let (result, _) = track("event: x\nid: 1\n", Some("Text/Event-Stream"));
        assert_eq!(
            result.unwrap_err(),
            "OpenRouter music stream contained no events"
        );
    }

    #[test]
    fn media_types_normalize_and_stay_quotable_only_when_safe() {
        assert_eq!(media_type("MP3").unwrap(), "audio/mpeg");
        assert_eq!(media_type("audio/x-wav").unwrap(), "audio/wav");
        assert_eq!(
            media_type("text/plain").unwrap_err(),
            "OpenRouter music generation: expected audio media type, received text/plain"
        );
        assert_eq!(
            media_type("a prompt with spaces").unwrap_err(),
            "OpenRouter music generation: expected audio media type"
        );
    }

    #[test]
    fn the_last_usage_wins_even_when_a_later_event_fails() {
        let body = "data: {\"usage\": {\"cost\": 0.1}}\n\
                    data: {\"usage\": {\"cost\": 0.2}, \"error\": {\"code\": 502}}\n";
        let (result, track) = track(body, Some("text/event-stream"));
        assert_eq!(
            result.unwrap_err(),
            "OpenRouter music stream reported an error: code=502"
        );
        assert_eq!(track.cost(), Some(grida_fx_core::money::Usd(200_000)));
    }
}
