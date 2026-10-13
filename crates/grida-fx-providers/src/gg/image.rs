//! GG images: `image.generate` (`POST {base}/images/generations`, JSON) and `image.edit`
//! (`POST {base}/images/edits`, multipart), both answered synchronously (src/gg/README.md;
//! spec/capabilities.md §2–§3).
//!
//! One [`RequestAdapter`], registered for both capabilities on provider `gg`; the endpoint is
//! chosen by `call.route.capability`. Contract `adapter: gg-image`, behaviour `"1"` (the string,
//! as every image route carries it), and an optional `quality`: the model's quality setting, sent
//! as given (`max` for GPT Image). Deadline [`DEADLINE`]; response cap [`MAX_RESPONSE_BYTES`].
//!
//! The wire, in this order: `model`, `prompt` (verbatim), `n: 1`, `size` when the request gives
//! `WIDTHxHEIGHT` (`auto` and none are left out: the model chooses), the contract's `quality`, and
//! `background` when the request gives one other than `auto` (the default everywhere, and a member
//! some models do not take). An edit sends these as text fields, then each input picture (`image`,
//! then `references`) as a part `image[]` named `reference-NN.<png|jpg|webp>` from `01`. Every
//! request carries its attempt's `Idempotency-Key` ([`super::Gateway`]).
//!
//! The answer is `data[0].b64_json`, strict base64, as file `image` of the kind its bytes show,
//! with `data: null`, priced by [`super::Gateway::priced`]: GG's settled charge when it gives one,
//! and its `x-gg-reserved-mills` as the cost bound. The check is every image route's
//! ([`crate::checks::image`]).
//!
//! A send, in order (spec/adapters.md §4; the engine makes steps 1 and 2 before it, the
//! contract's `quality` through `check_contract`, and the adapter again for a direct caller):
//! 1. the route: `image.generate` or `image.edit`, and a contract this adapter serves;
//! 2. [`crate::capabilities::check_request`];
//! 3. the key: `GG_API_KEY is not set`;
//! 4. values: a blank prompt, an unknown `background`, a `size` that is neither `auto` nor
//!    `WIDTHxHEIGHT`, references on a generation, a mask, more than [`MAX_INPUT_IMAGES`] pictures;
//! 5. files (`image`, then `references[i]`): a digest the call does not carry or an empty store
//!    copy, a kind other than PNG, JPEG or WebP, bytes that do not decode as their kind;
//! 6. one exchange, classified by [`super::classify`] ([`super::Gateway::exchange`]).
//!
//! Model limits (sizes, counts, which members a model takes) are GG's to refuse, before any paid
//! work and at no cost.

use super::Gateway;
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Identity, RequestAdapter, Sent};
use crate::capabilities;
use crate::transport::{Body, HttpRequest, HttpResponse, Lane, Method, Part};
use crate::wire;
use serde_json::{Map, Value, json};
use std::time::Duration;

/// The label of every reason.
pub const LABEL: &str = "GG image generation";

/// The route contract's `adapter` this adapter serves.
pub const ADAPTER: &str = "gg-image";

/// The contract this adapter serves: [`ADAPTER`], and `adapter_behavior` the text `"1"`.
pub const IDENTITY: Identity = Identity {
    adapter: ADAPTER,
    behavior: Some(r#""1""#),
};

/// The whole-exchange deadline: GG answers within 600 s, then a margin for its answer to arrive.
pub const DEADLINE: Duration = Duration::from_secs(630);

/// The response cap.
pub const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// The most input pictures of an edit (`image` plus `references`).
pub const MAX_INPUT_IMAGES: usize = 16;

/// The refusal of a `size` that is not `auto` or `WxH`.
const SIZE_SHAPE: &str = "GG image size must be auto or WIDTHxHEIGHT";

/// The picture kinds GG takes as inputs, with their file name extensions.
const INPUT_KINDS: [(&str, &str); 3] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
];

/// GG's image adapter (module doc).
#[derive(Debug, Clone)]
pub struct GgImages {
    gateway: Gateway,
}

impl GgImages {
    pub fn new(gateway: Gateway) -> GgImages {
        GgImages { gateway }
    }

    /// Steps 1 to 5 of the module doc: the one request to send, or the refusal sentence (not yet
    /// redacted).
    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let quality = contract(call)?;
        let edit = call.route.capability == "image.edit";
        capabilities::check_request(&call.route.capability, &call.request)?;
        let credential = self.gateway.credential()?;
        let values = Values::read(&call.request, edit)?;
        let fields = fields(&call.route.model, &values, quality.as_deref());
        let request = |path: &str| {
            self.gateway.paid(
                HttpRequest::new(Method::Post, self.gateway.client.url(path), Lane::Provider)
                    .timeout(DEADLINE)
                    .max_response_bytes(MAX_RESPONSE_BYTES),
                credential.clone(),
                call,
            )
        };
        if !edit {
            let body: Map<String, Value> = fields
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect();
            return Ok(request("images/generations").body(Body::Json(Value::Object(body))));
        }
        let mut parts: Vec<Part> = fields
            .iter()
            .map(|(name, value)| Part::text(name, &field_text(value)))
            .collect();
        let mut inputs = vec![("image".to_string(), &call.request["image"])];
        inputs.extend(
            values
                .references
                .iter()
                .enumerate()
                .map(|(i, value)| (format!("references[{i}]"), value)),
        );
        for (index, (member, value)) in inputs.iter().enumerate() {
            let (kind, extension, bytes) = input_file(call, value, member)?;
            let filename = format!("reference-{:02}.{extension}", index + 1);
            parts.push(Part::file("image[]", &filename, kind, bytes));
        }
        Ok(request("images/edits").body(Body::Multipart(parts)))
    }

    /// A 2xx body as the answer, priced (spec/adapters.md §3.4): a structural failure is billed
    /// and may be tried again.
    async fn parse(&self, response: &HttpResponse) -> Sent {
        match answer(&response.body) {
            Ok(answer) => Sent::Answered(self.gateway.priced(answer, response).await),
            Err(text) => {
                let failed = Sent::Failed {
                    reason: self.gateway.reason(&super::with_request_id(text, response)),
                    cost: None,
                    retryable: true,
                };
                self.gateway.charged(failed, response).await
            }
        }
    }
}

impl RequestAdapter for GgImages {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            let request = match self.prepare(call) {
                Ok(request) => request,
                Err(reason) => {
                    return Sent::Refused {
                        reason: self.gateway.reason(&reason),
                    };
                }
            };
            match self.gateway.exchange(call, LABEL, request).await {
                Ok(response) => self.parse(&response).await,
                Err(sent) => sent,
            }
        })
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(image) = answer.files.get("image") else {
            return Err("the answer holds no image".into());
        };
        let size =
            wire::parse_size(call.request.get("size")).map_err(|_| SIZE_SHAPE.to_string())?;
        crate::checks::image(call, image, size)
    }

    fn identity(&self) -> Option<Identity> {
        Some(IDENTITY)
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        contract(call).map(|_| ())
    }
}

/// Step 1 (spec/adapters.md §4): the route's contract, and the `quality` it names, if any.
fn contract(call: &CallRequest) -> Result<Option<String>, String> {
    let contract = super::read_contract(
        call,
        &["image.generate", "image.edit"],
        ADAPTER,
        |behavior| IDENTITY.behaves(behavior),
        &["quality"],
    )?;
    match contract.get("quality") {
        None => Ok(None),
        Some(Value::String(quality)) if !quality.trim().is_empty() => Ok(Some(quality.clone())),
        Some(_) => Err(super::not_served(call, Some("quality is text"))),
    }
}

/// The request's values after step 4.
#[derive(Debug)]
struct Values<'a> {
    /// Verbatim: the prompt is never trimmed or rewritten.
    prompt: &'a str,
    /// `None` for `auto` or none.
    background: Option<&'a str>,
    /// `WIDTHxHEIGHT`; `None` for `auto` or none.
    size: Option<&'a str>,
    /// An edit's further input pictures.
    references: &'a [Value],
}

impl<'a> Values<'a> {
    /// Reads and checks the values of a request that passed its capability's check.
    fn read(request: &'a Value, edit: bool) -> Result<Values<'a>, String> {
        let prompt = request.get("prompt").and_then(Value::as_str).unwrap_or("");
        if prompt.trim().is_empty() {
            return Err("an image call needs its prompt as text".into());
        }
        let background = match request.get("background") {
            None | Some(Value::Null) => None,
            Some(Value::String(b)) if b == "auto" => None,
            Some(Value::String(b)) if matches!(b.as_str(), "opaque" | "transparent") => {
                Some(b.as_str())
            }
            Some(_) => return Err("background must be auto, opaque or transparent".into()),
        };
        let size =
            match wire::parse_size(request.get("size")).map_err(|_| SIZE_SHAPE.to_string())? {
                Some(_) => request.get("size").and_then(Value::as_str),
                None => None,
            };
        let references = request
            .get("references")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if !edit && !references.is_empty() {
            return Err("GG image generation takes no references".into());
        }
        if !wire::is_absent(request.get("mask")) {
            return Err("GG image edits take no mask".into());
        }
        if edit && 1 + references.len() > MAX_INPUT_IMAGES {
            return Err(format!(
                "GG image edits support at most {MAX_INPUT_IMAGES} input images"
            ));
        }
        Ok(Values {
            prompt,
            background,
            size,
            references,
        })
    }
}

/// The wire fields, in the order both bodies send them (module doc).
fn fields(model: &str, values: &Values<'_>, quality: Option<&str>) -> Vec<(&'static str, Value)> {
    let mut fields = vec![
        ("model", json!(model.trim())),
        ("prompt", json!(values.prompt)),
        ("n", json!(1)),
    ];
    if let Some(size) = values.size {
        fields.push(("size", json!(size)));
    }
    if let Some(quality) = quality {
        fields.push(("quality", json!(quality)));
    }
    if let Some(background) = values.background {
        fields.push(("background", json!(background)));
    }
    fields
}

/// A field's multipart text: a string as itself, a number in its JSON form (`n` is `1`).
fn field_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Step 5 for one input picture: its kind, its file name extension and its bytes.
fn input_file(
    call: &CallRequest,
    value: &Value,
    member: &str,
) -> Result<(&'static str, &'static str, Vec<u8>), String> {
    let file = wire::request_file(call, value, member)?;
    let Some((kind, extension)) = INPUT_KINDS
        .iter()
        .copied()
        .find(|(kind, _)| *kind == file.kind)
    else {
        return Err(format!(
            "{member} is {}; GG takes PNG, JPEG or WebP",
            file.kind
        ));
    };
    let bytes = wire::read_file(file, member)?;
    let decodes = wire::matches_signature(kind, &bytes)
        && matches!(grida_fx_core::facts::image_facts(&bytes, kind), Ok(Some(_)));
    if !decodes {
        return Err(format!("{member} is not a decodable {kind}"));
    }
    Ok((kind, extension, bytes))
}

/// A 2xx body as the answer, or the structural failure's sentence (spec/adapters.md §3.4).
/// `created`, `media_type` and any usage are not read: the bytes decide the kind.
fn answer(body: &[u8]) -> Result<Answer, String> {
    let payload = wire::json_object(body, LABEL)?;
    let item = match payload.get("data") {
        Some(Value::Array(items)) if items.len() == 1 => items[0].as_object(),
        _ => None,
    }
    .ok_or_else(|| format!("{LABEL} returned no single image"))?;
    let bytes = item
        .get("b64_json")
        .and_then(Value::as_str)
        .and_then(wire::strict_base64)
        .ok_or_else(|| "GG image b64_json is not valid base64".to_string())?;
    let kind = wire::sniff_image(&bytes).unwrap_or("application/octet-stream");
    Ok(Answer::new(Value::Null, None).with_file("image", kind, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_read_or_refused_in_order() {
        let read = |request: Value, edit: bool| {
            Values::read(&request, edit)
                .map(|v| (v.background.map(str::to_string), v.size.map(str::to_string)))
        };
        assert_eq!(
            read(
                json!({"prompt": " p ", "background": "auto", "size": "auto"}),
                false
            ),
            Ok((None, None))
        );
        assert_eq!(
            read(
                json!({"prompt": "p", "background": "transparent", "size": "1024x1536"}),
                false
            ),
            Ok((Some("transparent".into()), Some("1024x1536".into())))
        );
        let refused = |request: Value, edit: bool| Values::read(&request, edit).unwrap_err();
        assert_eq!(
            refused(json!({"prompt": "  "}), false),
            "an image call needs its prompt as text"
        );
        assert_eq!(
            refused(json!({"prompt": "p", "background": "white"}), false),
            "background must be auto, opaque or transparent"
        );
        assert_eq!(
            refused(json!({"prompt": "p", "size": "1K"}), false),
            "GG image size must be auto or WIDTHxHEIGHT"
        );
        assert_eq!(
            refused(json!({"prompt": "p", "references": [{"file": "a"}]}), false),
            "GG image generation takes no references"
        );
        assert_eq!(
            refused(json!({"prompt": "p", "mask": {"file": "a"}}), true),
            "GG image edits take no mask"
        );
        let many: Vec<Value> = (0..16).map(|_| json!({"file": "a"})).collect();
        assert_eq!(
            refused(json!({"prompt": "p", "references": many}), true),
            "GG image edits support at most 16 input images"
        );
    }

    #[test]
    fn fields_leave_out_what_the_model_chooses() {
        let request = json!({"prompt": "p", "background": "opaque", "size": "1024x1024"});
        let values = Values::read(&request, false).unwrap();
        let names: Vec<&str> = fields(" acme/img-a ", &values, Some("max"))
            .iter()
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(
            names,
            ["model", "prompt", "n", "size", "quality", "background"]
        );
        let bare = json!({"prompt": "p"});
        let values = Values::read(&bare, false).unwrap();
        assert_eq!(
            fields("acme/img-a", &values, None),
            vec![
                ("model", json!("acme/img-a")),
                ("prompt", json!("p")),
                ("n", json!(1))
            ]
        );
        assert_eq!(field_text(&json!(1)), "1");
        assert_eq!(field_text(&json!("max")), "max");
    }

    #[test]
    fn answers_take_one_strict_base64_picture() {
        let png = crate::testing::media::png(2, 2, Some(255));
        let body = |data: Value| serde_json::to_vec(&json!({"created": 1, "data": data})).unwrap();
        let ok = answer(&body(json!([{"b64_json": wire::data_url("x", &png).split_once(',').unwrap().1, "media_type": "image/jpeg"}]))).unwrap();
        assert_eq!(ok.files["image"].kind, "image/png", "the bytes decide");
        assert_eq!(ok.data, Value::Null);
        assert_eq!(ok.cost, None);
        assert_eq!(
            answer(&body(json!([]))).unwrap_err(),
            "GG image generation returned no single image"
        );
        assert_eq!(
            answer(&body(json!([{"b64_json": "not base64!"}]))).unwrap_err(),
            "GG image b64_json is not valid base64"
        );
        assert_eq!(
            answer(&body(json!([{"url": "https://example.test/x.png"}]))).unwrap_err(),
            "GG image b64_json is not valid base64"
        );
        assert_eq!(
            answer(b"<html>").unwrap_err(),
            "GG image generation returned invalid JSON"
        );
    }
}
