//! OpenAI: `image.generate` and `image.edit` on the Images API, with native alpha
//! (spec/providers.md §9.1; spec/capabilities.md §2–§3).
//!
//! One [`RequestAdapter`], registered for both capabilities on provider `openai`; the endpoint is
//! chosen by `call.route.capability`:
//! - `image.generate`: `POST {base}/images/generations`, a JSON body;
//! - `image.edit`: `POST {base}/images/edits`, `multipart/form-data` (text fields, then each input
//!   image as `image[]` named `reference-NN.<ext>`, then the mask as `mask` named
//!   `reference-00.<ext>`).
//!
//! Credential `authorization: Bearer <OPENAI_API_KEY>`; base `OPENAI_BASE_URL`, default
//! `https://api.openai.com/v1`. Deadline 600 s per send; response cap 64 MiB. Wire constants
//! (`n: 1`, `output_format: png`, `quality: max`, `moderation: low`; never `input_fidelity`,
//! `response_format`, `seed`) are spec/providers.md §9.1's. The answer is the one `b64_json` PNG
//! as file `image`, `data: null`, `cost: None` (OpenAI reports no cost). [`check`] is
//! [`crate::wire::check_image`]. Status classes: spec/providers.md §9.1's table.
//!
//! A send, in order (spec/providers.md §5):
//! 1. the route: capability `image.generate` or `image.edit` on provider `openai`, and a contract
//!    whose `adapter` (when present) is [`ADAPTER`] and whose `adapter_behavior` (when present) is
//!    [`ADAPTER_BEHAVIOR`]; else `<model@provider> is not a <capability> route this adapter
//!    serves`;
//! 2. [`crate::capabilities::check_request`];
//! 3. the key: `OPENAI_API_KEY is not set`;
//! 4. values: a blank prompt, an unknown `background`, a `size` that is not `auto` or `WxH` or
//!    leaves the envelope, references on a generation, more than [`MAX_INPUT_IMAGES`] inputs;
//! 5. files (`image`, then `references[i]`, then `mask`): a digest the call does not carry or an
//!    empty store copy, a kind other than PNG, JPEG or WebP, bytes that do not decode as their
//!    kind;
//! 6. one exchange, classified by `OpenAiImages::classify` (spec/providers.md §4.2, §4.3 and
//!    §9.1's overrides).
//!
//! Every reason goes through [`Client::reason`] (redacted, bounded) and names the provider's
//! request id when the response carries a safe one.
//!
//! [`check`]: RequestAdapter::check

use crate::BoxFuture;
use crate::adapter::{Adapter, Answer, CallRequest, RequestAdapter, RouteRef, Sent};
use crate::capabilities;
use crate::keys::KeyName;
use crate::registry::Adapters;
use crate::setup::{Client, Setup};
use crate::transport::{
    Body, HttpRequest, HttpResponse, Lane, Method, Part, Phase, TransportError, TransportErrorKind,
};
use crate::wire;
use grida_fx_core::money::Usd;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Duration;

/// The label of every reason.
pub const LABEL: &str = "OpenAI image generation";

/// The whole-exchange deadline of one send.
pub const DEADLINE: Duration = Duration::from_secs(600);

/// The response cap.
pub const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// The most input images of an edit (`image` plus `references`; the mask does not count).
pub const MAX_INPUT_IMAGES: usize = 16;

/// The route contract's `adapter` this adapter serves (spec/providers.md §5 step 1, §9.1).
pub const ADAPTER: &str = "fx-openai-image-v1";

/// The route contract's `adapter_behavior` this adapter serves: the text `"1"`, as every image
/// route carries it (spec/providers.md §10). The number `1` is another contract, with another
/// fingerprint, and is not served.
pub const ADAPTER_BEHAVIOR: &str = "1";

/// The variable a redirect points at (spec/providers.md §3, §4.3).
const BASE_URL_VARIABLE: &str = "OPENAI_BASE_URL";

/// The refusal of a `size` that is not `auto` or `WxH`.
const SIZE_SHAPE: &str = "OpenAI image size must be auto or WIDTHxHEIGHT";

/// The image kinds OpenAI takes as inputs and masks, with their file name extensions.
const INPUT_KINDS: [(&str, &str); 3] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
];

/// The Images API adapter (module doc).
#[derive(Debug, Clone)]
pub struct OpenAiImages {
    client: Client,
}

impl OpenAiImages {
    pub fn new(setup: &Setup) -> OpenAiImages {
        OpenAiImages {
            client: setup.client(KeyName::OpenAi, &setup.endpoints.openai),
        }
    }

    /// Steps 1 to 5 of the module doc: the one request to send, or the refusal sentence (not yet
    /// redacted).
    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let edit = served(&call.route)?;
        capabilities::check_request(&call.route.capability, &call.request)?;
        let credential = self.client.credential("authorization", "Bearer ")?;
        let values = Values::read(&call.request, edit)?;
        let fields = fields(&call.route.model, &values);
        let request = |path: &str| {
            HttpRequest::new(Method::Post, self.client.url(path), Lane::Provider)
                .credential(credential.clone())
                .timeout(DEADLINE)
                .max_response_bytes(MAX_RESPONSE_BYTES)
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
        if let Some(mask) = call.request.get("mask").filter(|v| !v.is_null()) {
            let (kind, extension, bytes) = input_file(call, mask, "mask")?;
            parts.push(Part::file(
                "mask",
                &format!("reference-00.{extension}"),
                kind,
                bytes,
            ));
        }
        Ok(request("images/edits").body(Body::Multipart(parts)))
    }

    /// What one exchange's response means (spec/providers.md §4.3, §4.4 and §9.1).
    fn classify(&self, response: &HttpResponse) -> Sent {
        let status = response.status;
        let id = wire::request_id(response);
        let reason = |text: String| -> String {
            match &id {
                Some(id) => self.client.reason(&format!("{text} (request {id})")),
                None => self.client.reason(&text),
            }
        };
        if response.is_success() {
            return match answer(&response.body) {
                Ok(answer) => Sent::Answered(answer),
                Err(text) => Sent::Failed {
                    reason: reason(text),
                    cost: None,
                    retryable: true,
                },
            };
        }
        if (300..400).contains(&status) {
            return Sent::Failed {
                reason: reason(format!(
                    "{LABEL} was redirected (HTTP {status}); check {BASE_URL_VARIABLE}"
                )),
                cost: None,
                retryable: false,
            };
        }
        let detail = wire::safe_error_detail(&response.body, &self.client.redactor);
        let said = wire::status_reason(LABEL, status, detail.as_deref());
        let error = ErrorFields::of(&response.body);
        match status {
            // OpenAI's 408 is its own timeout: the request arrived, and the work may be done.
            408 => Sent::Failed {
                reason: reason(said),
                cost: None,
                retryable: true,
            },
            429 if error.is_quota() => Sent::Failed {
                reason: reason(said),
                cost: Some(Usd::ZERO),
                retryable: false,
            },
            429 => {
                let text = match retry_after_text(response) {
                    Some(seconds) => format!("{said}; retry-after {seconds}"),
                    None => said,
                };
                not_received(reason(text), crate::adapter::retry_after(response))
            }
            400 if error.is_moderation() => Sent::Failed {
                reason: reason(said),
                cost: None,
                retryable: false,
            },
            400..=499 => Sent::Failed {
                reason: reason(said),
                cost: Some(Usd::ZERO),
                retryable: false,
            },
            // 5xx, and any status no class names: the provider may have done the work.
            _ => Sent::Failed {
                reason: reason(said),
                cost: None,
                retryable: true,
            },
        }
    }

    /// A failed exchange (spec/providers.md §4.2).
    fn transport_failure(&self, error: &TransportError) -> Sent {
        if error.kind == TransportErrorKind::Refused {
            return Sent::Refused {
                reason: self
                    .client
                    .reason(&format!("{LABEL} was not sent: {}", error.reason)),
            };
        }
        match error.phase {
            Phase::NotSent => not_received(
                self.client
                    .reason(&format!("{LABEL} was not sent: {}", error.reason)),
                None,
            ),
            Phase::AfterSend => Sent::Failed {
                reason: self
                    .client
                    .reason(&format!("{LABEL} failed after sending: {}", error.reason)),
                cost: None,
                retryable: true,
            },
        }
    }
}

impl RequestAdapter for OpenAiImages {
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
            match self.client.send(request).await {
                Ok(response) => self.classify(&response),
                Err(error) => self.transport_failure(&error),
            }
        })
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(image) = answer.files.get("image") else {
            return Err("the answer has no image".into());
        };
        let size =
            wire::parse_size(call.request.get("size")).map_err(|_| SIZE_SHAPE.to_string())?;
        crate::checks::image(call, image, size)
    }
}

/// Registers the adapter for `image.generate` and `image.edit` on `openai`.
pub fn register(adapters: &mut Adapters, setup: &Setup) {
    let images = Arc::new(OpenAiImages::new(setup));
    adapters.register("image.generate", "openai", Adapter::Request(images.clone()));
    adapters.register("image.edit", "openai", Adapter::Request(images));
}

/// Step 1: whether the route is one this adapter serves, and if so whether it is an edit.
fn served(route: &RouteRef) -> Result<bool, String> {
    let refused = || {
        format!(
            "{} is not a {} route this adapter serves",
            route.id(),
            route.capability
        )
    };
    let edit = match route.capability.as_str() {
        "image.generate" => false,
        "image.edit" => true,
        _ => return Err(refused()),
    };
    if route.provider != "openai" {
        return Err(refused());
    }
    let contract = match &route.contract {
        Value::Null => return Ok(edit),
        Value::Object(contract) => contract,
        _ => return Err(refused()),
    };
    let adapter_ok = match contract.get("adapter") {
        None | Some(Value::Null) => true,
        Some(Value::String(adapter)) => adapter == ADAPTER,
        Some(_) => false,
    };
    let behavior_ok = match contract.get("adapter_behavior") {
        None | Some(Value::Null) => true,
        Some(Value::String(behavior)) => behavior == ADAPTER_BEHAVIOR,
        Some(_) => false,
    };
    if adapter_ok && behavior_ok {
        Ok(edit)
    } else {
        Err(refused())
    }
}

/// The request's values after step 4 (spec/providers.md §9.1).
#[derive(Debug)]
struct Values<'a> {
    /// Verbatim: the prompt is never trimmed or rewritten.
    prompt: &'a str,
    /// `auto` when absent.
    background: &'a str,
    /// The request's `size` text, sent only when given (`auto` included).
    size: Option<&'a str>,
    /// An edit's further input images.
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
            None | Some(Value::Null) => "auto",
            Some(Value::String(b)) if matches!(b.as_str(), "auto" | "opaque" | "transparent") => {
                b.as_str()
            }
            Some(_) => return Err("background must be auto, opaque or transparent".into()),
        };
        let size = match request.get("size") {
            None | Some(Value::Null) => None,
            Some(value) => {
                if let Some((width, height)) =
                    wire::parse_size(Some(value)).map_err(|_| SIZE_SHAPE.to_string())?
                {
                    wire::check_size_envelope(width, height, "OpenAI")?;
                }
                Some(value.as_str().ok_or_else(|| SIZE_SHAPE.to_string())?)
            }
        };
        let references = request
            .get("references")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if !edit && !references.is_empty() {
            return Err("OpenAI image generation takes no references".into());
        }
        if edit && 1 + references.len() > MAX_INPUT_IMAGES {
            return Err(format!(
                "OpenAI image edits support at most {MAX_INPUT_IMAGES} input images"
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

/// The wire fields, in the order both bodies send them (spec/providers.md §9.1).
fn fields(model: &str, values: &Values<'_>) -> Vec<(&'static str, Value)> {
    let mut fields = vec![
        ("model", json!(model)),
        ("prompt", json!(values.prompt)),
        ("n", json!(1)),
        ("output_format", json!("png")),
    ];
    if let Some(size) = values.size {
        fields.push(("size", json!(size)));
    }
    fields.push(("quality", json!("max")));
    fields.push(("background", json!(values.background)));
    fields.push(("moderation", json!("low")));
    fields
}

/// A field's multipart text: a string as itself, a number in its decimal form (`n` is `1`).
fn field_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Step 5 for one input image or mask: its kind, its file name extension and its bytes.
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
            "{member} is {}; OpenAI takes PNG, JPEG or WebP",
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

/// A 2xx body as the answer, or the structural failure's sentence (spec/providers.md §4.4).
/// `revised_prompt`, `usage`, `created` and any URL are ignored; a URL is never fetched.
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
        .ok_or_else(|| "OpenAI image b64_json is not valid base64".to_string())?;
    let kind = wire::sniff_image(&bytes).unwrap_or("application/octet-stream");
    Ok(Answer::new(Value::Null, None).with_file("image", kind, bytes))
}

/// The `type` and `code` of an error envelope, for the status overrides of spec/providers.md
/// §9.1.
#[derive(Debug, Default)]
struct ErrorFields {
    kind: Option<String>,
    code: Option<String>,
}

impl ErrorFields {
    fn of(body: &[u8]) -> ErrorFields {
        let Some(error) = wire::error_envelope(body) else {
            return ErrorFields::default();
        };
        let text = |key: &str| {
            error
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_ascii_lowercase)
        };
        ErrorFields {
            kind: text("type"),
            code: text("code"),
        }
    }

    fn fields(&self) -> impl Iterator<Item = &str> {
        self.kind.iter().chain(self.code.iter()).map(String::as_str)
    }

    /// A 429 that means the quota is exhausted, not a rate limit.
    fn is_quota(&self) -> bool {
        self.fields().any(|field| field == "insufficient_quota")
    }

    /// A 400 from the safety system: `moderation_blocked`, or a type or code naming `safety` or a
    /// `content_policy`.
    fn is_moderation(&self) -> bool {
        self.fields().any(|field| {
            field == "moderation_blocked"
                || field.contains("safety")
                || field.contains("content_policy")
        })
    }
}

/// A `retry-after` header that is a plain number of seconds, as the reason shows it. The wait
/// itself is the shared parser's ([`crate::adapter::retry_after`]), which also reads an HTTP date
/// and `retry-after-ms`.
fn retry_after_text(response: &HttpResponse) -> Option<String> {
    let text = response.header("retry-after")?.trim();
    let mut pieces = text.splitn(2, '.');
    let whole = pieces.next().unwrap_or("");
    let fraction = pieces.next();
    let digits = |part: &str, most: usize| {
        !part.is_empty() && part.len() <= most && part.bytes().all(|b| b.is_ascii_digit())
    };
    if !digits(whole, 9) || fraction.is_some_and(|f| !digits(f, 9)) {
        return None;
    }
    Some(text.to_string())
}

/// `NotReceived`: the provider took nothing. `retry_after` is the wait the provider asked for;
/// the engine waits at least that long before resending (spec/providers.md §4.3).
fn not_received(reason: String, retry_after: Option<Duration>) -> Sent {
    Sent::NotReceived {
        reason,
        retry_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(capability: &str, provider: &str, contract: Value) -> RouteRef {
        RouteRef {
            capability: capability.into(),
            model: "img-a".into(),
            provider: provider.into(),
            contract,
        }
    }

    #[test]
    fn routes_are_served_by_capability_provider_and_contract() {
        let ours = json!({"adapter": ADAPTER, "adapter_behavior": "1", "surface": "x"});
        assert_eq!(
            served(&route("image.generate", "openai", ours.clone())),
            Ok(false)
        );
        assert_eq!(served(&route("image.edit", "openai", ours)), Ok(true));
        assert_eq!(served(&route("image.edit", "openai", json!({}))), Ok(true));
        assert_eq!(
            served(&route(
                "image.edit",
                "openai",
                json!({"adapter": null, "adapter_behavior": null})
            )),
            Ok(true)
        );
        assert_eq!(
            served(&route("image.edit", "openai", Value::Null)),
            Ok(true)
        );
        for (capability, provider, contract) in [
            (
                "image.generate",
                "openai",
                json!({"adapter": "gnode-openai-image-v1"}),
            ),
            (
                "image.generate",
                "openai",
                json!({"adapter": "fx-openrouter-image-v1"}),
            ),
            ("image.generate", "openai", json!({"adapter": 1})),
            ("image.generate", "openai", json!({"adapter_behavior": "2"})),
            // routes-caps F3: the behaviour is the text "1", as OpenRouter's is the text "3".
            ("image.generate", "openai", json!({"adapter_behavior": 1})),
            ("image.generate", "openai", json!({"adapter_behavior": 1.0})),
            ("image.generate", "openai", json!({"adapter_behavior": 1.5})),
            ("image.generate", "openai", json!([])),
            ("image.generate", "openai", json!("fx-openai-image-v1")),
            (
                "image.generate",
                "openai",
                json!({"adapter_behavior": true}),
            ),
            ("background.remove", "openai", json!({})),
            ("image.generate", "acme", json!({})),
        ] {
            assert_eq!(
                served(&route(capability, provider, contract.clone())),
                Err(format!(
                    "img-a@{provider} is not a {capability} route this adapter serves"
                )),
                "{capability} {provider} {contract}"
            );
        }
    }

    #[test]
    fn fields_keep_the_wire_order() {
        let request = json!({"prompt": " a kite ", "size": "auto"});
        let values = Values::read(&request, false).unwrap();
        let names: Vec<&str> = fields("img-a", &values)
            .iter()
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(
            names,
            [
                "model",
                "prompt",
                "n",
                "output_format",
                "size",
                "quality",
                "background",
                "moderation"
            ]
        );
        assert_eq!(values.prompt, " a kite ");
        assert_eq!(values.background, "auto");
        let without = json!({"prompt": "p", "size": null, "background": null});
        let values = Values::read(&without, false).unwrap();
        assert_eq!(values.size, None);
        assert!(
            !fields("img-a", &values)
                .iter()
                .any(|(name, _)| *name == "size")
        );
        assert_eq!(field_text(&json!(1)), "1");
        assert_eq!(field_text(&json!("png")), "png");
    }

    #[test]
    fn retry_after_shows_plain_numbers_only() {
        let wait = |value: &str| {
            retry_after_text(&HttpResponse::new(429, Vec::new()).with_header("Retry-After", value))
        };
        assert_eq!(wait("20"), Some("20".to_string()));
        assert_eq!(wait(" 1.5 "), Some("1.5".to_string()));
        for ignored in [
            "",
            "-1",
            "1e3",
            "1.",
            ".5",
            "Wed, 21 Oct 2026 07:28:00 GMT",
            "9999999999",
            "20s",
        ] {
            assert_eq!(wait(ignored), None, "{ignored:?}");
        }
        assert_eq!(retry_after_text(&HttpResponse::new(429, Vec::new())), None);
    }

    #[test]
    fn error_fields_name_quota_and_moderation() {
        let fields = |error: Value| ErrorFields::of(json!({"error": error}).to_string().as_bytes());
        assert!(fields(json!({"code": "insufficient_quota"})).is_quota());
        assert!(fields(json!({"type": "insufficient_quota", "code": null})).is_quota());
        assert!(!fields(json!({"code": "rate_limit_exceeded"})).is_quota());
        assert!(fields(json!({"code": "moderation_blocked"})).is_moderation());
        assert!(fields(json!({"type": "image_generation_safety_error"})).is_moderation());
        assert!(fields(json!({"code": "Content_Policy_Violation"})).is_moderation());
        assert!(!fields(json!({"code": "invalid_value", "message": "safety"})).is_moderation());
        assert!(!ErrorFields::of(b"not json").is_moderation());
    }
}
