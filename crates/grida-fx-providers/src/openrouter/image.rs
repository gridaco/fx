//! OpenRouter images: `POST {base}/images` for both `image.generate` and `image.edit`
//! (spec/providers.md §9.2; spec/capabilities.md §2–§3).
//!
//! Contract: `adapter: fx-openrouter-image-v1`, `adapter_behavior: "3"` (a string), each checked
//! when present (`super::serves`). Refusals, in the order of spec/providers.md §5: the contract; the capability's shape
//! ([`crate::capabilities::check_request`]); the key; then the values (a blank prompt, a mask,
//! `background: transparent`, another unknown background, a `size` other than `auto` and the
//! seven verified sizes ([`ALLOWED_SIZES`]), references on a generate, an edit with more than
//! [`MAX_PICTURES`] pictures); then the files (each picture must be an `image/*` file with bytes);
//! and last a serialized body over [`super::MAX_REQUEST_BYTES`].
//!
//! Body, members in this order: `{"model", "prompt", "n": 1, "provider": {"allow_fallbacks":
//! false, "options": {"openai": {"moderation": "low"}}}, "size"?, "quality": "max", "background",
//! "input_references"?}`. `size` is sent when the request has one (`auto` included);
//! `background` always (`auto` when absent); `input_references` on an edit only, `image` first and
//! then `references`, each `{"type": "image_url", "image_url": {"url": <data URL>}}`.
//!
//! Answer: `data` must hold exactly one object whose `b64_json` is strict base64 (a `url` is never
//! fetched). Its kind is `media_type` when that member is present (a parameter-free PNG, JPEG or
//! WebP; `image/jpg`, `null` and anything else are refused), else sniffed from the bytes. The
//! answer is file `image` with that kind, `data: null`, and `cost` from `usage.cost`
//! (spec/providers.md §6). A 2xx that cannot become an answer is `Failed { cost, retryable:
//! true }`, with the reported cost when the body parsed (spec/providers.md §4.4).
//! `revised_prompt` is ignored. Deadline 600 s; response cap 64 MiB. `check`:
//! [`crate::wire::check_image`].

use super::{BODY_TOO_LARGE, body_exceeds, classify, not_served, serves, with_request_id};
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, RequestAdapter, Sent};
use crate::setup::Client;
use crate::transport::{Body, HttpRequest, HttpResponse, Lane, Method};
use crate::wire;
use serde_json::{Map, Value, json};
use std::time::Duration;

/// The label of every reason.
pub const LABEL: &str = "OpenRouter image generation";

/// The contract this adapter serves.
pub const CONTRACT_ADAPTER: &str = "fx-openrouter-image-v1";
pub const CONTRACT_BEHAVIOR: &str = "3";

/// The exact sizes the route returns exactly.
pub const ALLOWED_SIZES: [&str; 7] = [
    "1024x1024",
    "1152x2496",
    "1712x2560",
    "2064x1008",
    "2496x1152",
    "2560x1440",
    "2560x1712",
];

pub const DEADLINE: Duration = Duration::from_secs(600);
pub const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// The most pictures an edit sends: `image` plus its `references`.
pub const MAX_PICTURES: usize = 16;

/// The capabilities this adapter is registered for.
const CAPABILITIES: [&str; 2] = ["image.generate", "image.edit"];

/// The media types an answer may declare (spec/providers.md §9.2).
const ANSWER_KINDS: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];

/// OpenRouter's image adapter (module doc).
#[derive(Debug, Clone)]
pub struct OpenRouterImages {
    client: Client,
}

impl OpenRouterImages {
    pub fn new(client: Client) -> OpenRouterImages {
        OpenRouterImages { client }
    }

    /// Every refusal of spec/providers.md §5, in order, then the request to send.
    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let route = &call.route;
        let behavior = Value::String(CONTRACT_BEHAVIOR.to_string());
        if !CAPABILITIES.contains(&route.capability.as_str())
            || !serves(&route.contract, CONTRACT_ADAPTER, |v| v == &behavior)
        {
            return Err(not_served(call));
        }
        crate::capabilities::check_request(&route.capability, &call.request)?;
        let credential = self.client.credential("authorization", "Bearer ")?;

        let request = &call.request;
        let edit = route.capability == "image.edit";
        let prompt = request.get("prompt").and_then(Value::as_str).unwrap_or("");
        if prompt.trim().is_empty() {
            return Err("an image call needs its prompt as text".into());
        }
        if !wire::is_absent(request.get("mask")) {
            return Err("OpenRouter image generation has no masked-edit route".into());
        }
        let background = match request.get("background") {
            None | Some(Value::Null) => "auto",
            Some(value) => value.as_str().unwrap_or(""),
        };
        if background == "transparent" {
            return Err(
                "OpenRouter image generation does not support transparent backgrounds".into(),
            );
        }
        if !matches!(background, "auto" | "opaque") {
            return Err("background must be auto, opaque or transparent".into());
        }
        let size = match request.get("size") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let size = value.as_str().unwrap_or("");
                if size != "auto" && !ALLOWED_SIZES.contains(&size) {
                    return Err(format!("OpenRouter serves no exact size {size}"));
                }
                Some(size)
            }
        };
        let references: &[Value] = match request.get("references") {
            Some(Value::Array(items)) => items,
            _ => &[],
        };
        if !edit && !references.is_empty() {
            return Err("OpenRouter image generation takes no references".into());
        }
        if edit && 1 + references.len() > MAX_PICTURES {
            return Err(format!(
                "OpenRouter image edits support at most {MAX_PICTURES} input images"
            ));
        }

        let mut pictures = Vec::new();
        if edit {
            let mut members = vec![("image".to_string(), &request["image"])];
            members.extend(
                references
                    .iter()
                    .enumerate()
                    .map(|(i, value)| (format!("references[{i}]"), value)),
            );
            for (member, value) in members {
                let file = wire::request_file(call, value, &member)?;
                if !file.kind.starts_with("image/") {
                    return Err(format!("{member} is {}, not a picture", file.kind));
                }
                let bytes = wire::read_file(file, &member)?;
                pictures.push(json!({
                    "type": "image_url",
                    "image_url": {"url": wire::data_url(&file.kind, &bytes)},
                }));
            }
        }

        let mut body = Map::new();
        body.insert("model".into(), json!(route.model.trim()));
        body.insert("prompt".into(), json!(prompt));
        body.insert("n".into(), json!(1));
        body.insert(
            "provider".into(),
            json!({"allow_fallbacks": false, "options": {"openai": {"moderation": "low"}}}),
        );
        if let Some(size) = size {
            body.insert("size".into(), json!(size));
        }
        body.insert("quality".into(), json!("max"));
        body.insert("background".into(), json!(background));
        if edit {
            body.insert("input_references".into(), Value::Array(pictures));
        }
        let body = Value::Object(body);
        if body_exceeds(&body, super::MAX_REQUEST_BYTES) {
            return Err(BODY_TOO_LARGE.into());
        }
        Ok(
            HttpRequest::new(Method::Post, self.client.url("images"), Lane::Provider)
                .credential(credential)
                .body(Body::Json(body))
                .timeout(DEADLINE)
                .max_response_bytes(MAX_RESPONSE_BYTES),
        )
    }

    /// A 2xx response as an answer, or a structural failure (spec/providers.md §4.4).
    fn answer(&self, response: &HttpResponse) -> Sent {
        let failed = |reason: String, cost| Sent::Failed {
            reason: self.client.reason(&with_request_id(reason, response)),
            cost,
            retryable: true,
        };
        let payload = match wire::json_object(&response.body, LABEL) {
            Ok(payload) => payload,
            Err(reason) => return failed(reason, None),
        };
        let cost = payload
            .get("usage")
            .and_then(|usage| usage.get("cost"))
            .and_then(wire::usd_ceil);
        let item = match payload.get("data") {
            Some(Value::Array(items)) if items.len() == 1 => items[0].as_object(),
            _ => None,
        };
        let Some(item) = item else {
            return failed(format!("{LABEL} returned no single image"), cost);
        };
        let Some(bytes) = item
            .get("b64_json")
            .and_then(Value::as_str)
            .and_then(wire::strict_base64)
        else {
            return failed("OpenRouter image b64_json is not valid base64".into(), cost);
        };
        let kind = match item.get("media_type") {
            Some(declared) => match declared_kind(declared) {
                Some(kind) => kind,
                None => {
                    return failed(
                        "OpenRouter image media type must be parameter-free PNG, JPEG, or WebP"
                            .into(),
                        cost,
                    );
                }
            },
            None => match wire::sniff_image(&bytes).filter(|kind| ANSWER_KINDS.contains(kind)) {
                Some(kind) => kind,
                None => {
                    return failed(
                        "OpenRouter image response omitted media_type and bytes are not PNG, \
                         JPEG, or WebP"
                            .into(),
                        cost,
                    );
                }
            },
        };
        Sent::Answered(Answer::new(Value::Null, cost).with_file("image", kind, bytes))
    }
}

impl RequestAdapter for OpenRouterImages {
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

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(image) = answer.files.get("image") else {
            return Err("the answer has no image".into());
        };
        let size = wire::parse_size(call.request.get("size"))?;
        let background = call
            .request
            .get("background")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        wire::check_image(&image.kind, &image.bytes, size, background)
    }
}

/// A declared answer kind: a non-blank string with no parameters that is, lowercased, PNG, JPEG
/// or WebP. The alias `image/jpg` is not accepted.
fn declared_kind(value: &Value) -> Option<&'static str> {
    let text = value.as_str()?.trim();
    if text.is_empty() || text.contains(';') {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    ANSWER_KINDS.iter().copied().find(|kind| *kind == lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_kinds_are_parameter_free_png_jpeg_or_webp() {
        assert_eq!(declared_kind(&json!("image/png")), Some("image/png"));
        assert_eq!(declared_kind(&json!(" IMAGE/WEBP ")), Some("image/webp"));
        assert_eq!(declared_kind(&json!("image/jpeg")), Some("image/jpeg"));
        for bad in [
            json!("image/jpg"),
            json!("image/png; charset=binary"),
            json!("image/gif"),
            json!("image/bmp"),
            json!(""),
            json!(null),
            json!(1),
        ] {
            assert_eq!(declared_kind(&bad), None, "{bad}");
        }
    }
}
