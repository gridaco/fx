//! fal images: `POST {run}/<model>/text-to-image` (`image.generate`) and `POST {run}/<model>/edit`
//! (`image.edit`), JSON bodies, then one download of a hosted output (spec/providers.md §9.3).
//!
//! Contract `adapter` must be `fx-fal-image-v1` when present. Body members in order: `prompt`,
//! `num_images: 1`, `image_size` (only when `size` is given: `"auto"` or `{"width", "height"}`),
//! `quality: "max"`, `background`, `output_format: "png"`, then for an edit `image_urls` (`image`
//! first, then `references`) and `mask_url`. The answer is `root.images[0]` (`root` = `data` when
//! that is an object, else the payload): a data URI decoded in place, or a `*.fal.media` URL
//! downloaded once on the download lane with `accept: image/*`, no redirects, at most 64 MiB.
//! Deadline 600 s for the POST and the download together. `cost`: `usage.cost` at the payload's
//! top level ([`crate::wire::usd_ceil`]), else `None`. `check`: [`crate::wire::check_image`].
//!
//! Refused before sending (spec/providers.md §5, §9.3), in this order: another route's contract;
//! a request that does not fit its capability; no `FAL_KEY`; a blank prompt or one over
//! [`MAX_PROMPT_CHARS`]; a `background` other than `auto`, `opaque` or `transparent`; references
//! on a generate; more than [`MAX_PICTURES`] pictures on an edit; a `size` that is not `auto` or
//! `WxH` inside the GPT Image envelope; a file with no bytes.

use super::FalClients;
use super::output::{self, Rules};
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, RequestAdapter, Sent};
use crate::transport::{Body, HttpRequest, Lane, Method};
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

/// The label of every reason.
pub const LABEL: &str = "fal image generation";

/// The label of a download's reasons.
pub const DOWNLOAD_LABEL: &str = output::DOWNLOAD_LABEL;

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "fx-fal-image-v1";

/// The POST and the download together.
pub const DEADLINE: Duration = Duration::from_secs(600);

/// The most bytes of one output image.
pub const MAX_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

/// The longest prompt, in Unicode scalar values.
pub const MAX_PROMPT_CHARS: usize = 32_000;

/// The most input pictures of an edit (`image` and `references`; the mask does not count).
pub const MAX_PICTURES: usize = 16;

/// The POST's response cap: an output picture inline as base64 is 4/3 of its bytes, plus JSON.
pub(crate) const MAX_RESPONSE_BYTES: u64 = MAX_OUTPUT_BYTES / 3 * 4 + 4 * 1024 * 1024;

/// What an image answer may declare (spec/providers.md §9.3).
const RULES: Rules = Rules {
    allowed: &["image/png", "image/jpeg", "image/webp"],
    check_dimensions: true,
    max_bytes: MAX_OUTPUT_BYTES,
    allowed_words: "PNG, JPEG, or WebP",
};

const BACKGROUNDS: [&str; 3] = ["auto", "opaque", "transparent"];

/// fal's image adapter (module doc).
#[derive(Debug, Clone)]
pub struct FalImages {
    clients: FalClients,
}

impl FalImages {
    pub fn new(clients: FalClients) -> FalImages {
        FalImages { clients }
    }

    /// Steps 1 to 5 of spec/providers.md §5, then the request (module doc).
    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let client = &self.clients.run;
        if let Some(refusal) =
            super::contract_refusal(call, CONTRACT_ADAPTER, &["image.generate", "image.edit"])
        {
            return Err(refusal);
        }
        crate::capabilities::check_request(&call.route.capability, &call.request)?;
        let credential = client.credential(super::CREDENTIAL_HEADER, super::CREDENTIAL_PREFIX)?;
        let request = &call.request;
        let edit = call.route.capability == "image.edit";

        let prompt = request.get("prompt").and_then(Value::as_str).unwrap_or("");
        if prompt.trim().is_empty() {
            return Err("an image call needs its prompt as text".into());
        }
        if prompt.chars().count() > MAX_PROMPT_CHARS {
            return Err(format!(
                "the prompt is longer than {MAX_PROMPT_CHARS} characters"
            ));
        }
        let background = background(request)?;
        let references: &[Value] = request
            .get("references")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice);
        if !edit && !references.is_empty() {
            return Err("fal image generation takes no references".into());
        }
        if edit && 1 + references.len() > MAX_PICTURES {
            return Err(format!(
                "fal image edits support at most {MAX_PICTURES} input references"
            ));
        }
        let image_size = image_size(request.get("size"))?;

        let mut body = Map::new();
        body.insert("prompt".into(), Value::String(prompt.to_string()));
        body.insert("num_images".into(), json!(1));
        if let Some(image_size) = image_size {
            body.insert("image_size".into(), image_size);
        }
        body.insert("quality".into(), json!("max"));
        body.insert("background".into(), json!(background));
        body.insert("output_format".into(), json!("png"));
        if edit {
            let mut urls = vec![picture(call, &request["image"], "image")?];
            for (i, reference) in references.iter().enumerate() {
                urls.push(picture(call, reference, &format!("references[{i}]"))?);
            }
            body.insert("image_urls".into(), Value::Array(urls));
            if let Some(mask) = request.get("mask").filter(|m| !m.is_null()) {
                body.insert("mask_url".into(), picture(call, mask, "mask")?);
            }
        }

        let action = if edit { "edit" } else { "text-to-image" };
        let url = client.url(&format!("{}/{action}", super::endpoint(&call.route.model)));
        Ok(HttpRequest::new(Method::Post, url, Lane::Provider)
            .credential(credential)
            .body(Body::Json(Value::Object(body)))
            .timeout(DEADLINE)
            .max_response_bytes(MAX_RESPONSE_BYTES))
    }

    async fn send_once(&self, call: &CallRequest) -> Sent {
        let started = Instant::now();
        let client = &self.clients.run;
        let request = match self.prepare(call) {
            Ok(request) => request,
            Err(reason) => {
                return Sent::Refused {
                    reason: client.reason(&reason),
                };
            }
        };
        let exchange = client.send(request).await;
        let response = match super::classify_run(client, LABEL, exchange) {
            Ok(response) => response,
            Err(sent) => return sent,
        };
        let failed = |reason: String, cost| Sent::Failed {
            reason,
            cost,
            retryable: true,
        };
        let payload = match crate::wire::json_object(&response.body, LABEL) {
            Ok(payload) => payload,
            Err(reason) => return failed(client.reason(&reason), None),
        };
        let cost = super::reported_cost(&payload);
        let root = super::answer_root(&payload);
        let image = match root
            .get("images")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
        {
            Some([Value::Object(image)]) => image,
            _ => {
                return failed(
                    client.reason(&format!("{LABEL} returned no single image")),
                    cost,
                );
            }
        };
        let remaining = DEADLINE.saturating_sub(started.elapsed());
        match output::read_picture(client, image, RULES, remaining).await {
            Ok(picture) => Sent::Answered(Answer::new(Value::Null, cost).with_file(
                "image",
                picture.kind,
                picture.bytes,
            )),
            Err(reason) => failed(reason, cost),
        }
    }
}

/// `background`: absent or `null` is `auto`.
fn background(request: &Value) -> Result<&str, String> {
    match request.get("background") {
        None | Some(Value::Null) => Ok("auto"),
        Some(Value::String(value)) if BACKGROUNDS.contains(&value.as_str()) => Ok(value),
        Some(_) => Err("background must be auto, opaque or transparent".into()),
    }
}

/// `image_size` for `size`: nothing when absent or `null`, `"auto"`, or `{"width", "height"}`
/// within the GPT Image envelope (spec/providers.md §9.1, with sentences starting `fal image
/// size`).
fn image_size(size: Option<&Value>) -> Result<Option<Value>, String> {
    match size {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(auto)) if auto == "auto" => Ok(Some(json!("auto"))),
        Some(size) => match crate::wire::parse_size(Some(size)) {
            Ok(Some((width, height))) => {
                crate::wire::check_size_envelope(width, height, "fal")?;
                Ok(Some(json!({"width": width, "height": height})))
            }
            _ => Err("fal image size must be auto or WIDTHxHEIGHT".into()),
        },
    }
}

/// A request picture as a data URL (step 5 of spec/providers.md §5).
fn picture(call: &CallRequest, value: &Value, member: &str) -> Result<Value, String> {
    let file = crate::wire::request_file(call, value, member)?;
    let bytes = crate::wire::read_file(file, member)?;
    Ok(Value::String(super::picture_url(&file.kind, &bytes)))
}

impl RequestAdapter for FalImages {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(self.send_once(call))
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(file) = answer.files.get("image") else {
            return Err("the answer carries no image".into());
        };
        let size = crate::wire::parse_size(call.request.get("size"))
            .ok()
            .flatten();
        let background = call
            .request
            .get("background")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        crate::wire::check_image(&file.kind, &file.bytes, size, background)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_become_image_size() {
        assert_eq!(image_size(None), Ok(None));
        assert_eq!(image_size(Some(&Value::Null)), Ok(None));
        assert_eq!(image_size(Some(&json!("auto"))), Ok(Some(json!("auto"))));
        assert_eq!(
            image_size(Some(&json!("1536x1024"))),
            Ok(Some(json!({"width": 1536, "height": 1024})))
        );
        for (size, sentence) in [
            ("big", "fal image size must be auto or WIDTHxHEIGHT"),
            ("1024", "fal image size must be auto or WIDTHxHEIGHT"),
            ("01x10", "fal image size must be auto or WIDTHxHEIGHT"),
            ("1000x1024", "fal image size edges must be multiples of 16"),
            (
                "3856x1024",
                "fal image size edges must not exceed 3840 pixels",
            ),
            (
                "2400x768",
                "fal image size aspect ratio must not exceed 3:1",
            ),
            (
                "512x512",
                "fal image size must contain between 655360 and 8294400 pixels",
            ),
            (
                "3840x2176",
                "fal image size must contain between 655360 and 8294400 pixels",
            ),
        ] {
            assert_eq!(
                image_size(Some(&json!(size))).unwrap_err(),
                sentence,
                "{size}"
            );
        }
    }

    #[test]
    fn backgrounds_default_to_auto() {
        assert_eq!(background(&json!({})), Ok("auto"));
        assert_eq!(background(&json!({"background": null})), Ok("auto"));
        assert_eq!(
            background(&json!({"background": "transparent"})),
            Ok("transparent")
        );
        assert_eq!(
            background(&json!({"background": "clear"})).unwrap_err(),
            "background must be auto, opaque or transparent"
        );
    }

    #[test]
    fn the_response_cap_holds_a_full_size_inline_picture() {
        let inline = MAX_OUTPUT_BYTES.div_ceil(3) * 4;
        assert!(MAX_RESPONSE_BYTES > inline);
    }
}
