//! OpenRouter images on any model: `POST {base}/images` for `image.generate` and `image.edit`, the
//! default of both on `openrouter` (src/openrouter/README.md, "Images on any model";
//! spec/providers.md §2). It sends what OpenRouter's image models share, and nothing FX chose
//! for one model: a model's own parameters come in `provider_options.openrouter`.
//!
//! Contract `adapter: openrouter-image`, `adapter_behavior: "1"` (a string). Refusals, in the
//! order of spec/adapters.md §4: the contract; the capability's shape
//! ([`crate::capabilities::check_request`]); the key; then the values (provider options that set a
//! member of [`OWNED`], [`crate::wire::provider_options`]; then [`crate::wire::generic_image`]);
//! then the files (each picture an `image/*` file with bytes); and last a body over
//! [`super::MAX_REQUEST_BYTES`].
//!
//! Body, members in this order: `{"model", "prompt", "input_references"?}`, then the provider
//! options. `input_references` when the call has pictures (an edit's `image`, then `references`;
//! a generate's `references`, which only a route that declares `image_input` takes), each
//! `{"type": "image_url", "image_url": {"url": <data URL>}}`.
//!
//! Answer: `data` must hold exactly one object, else `Failed { cost, retryable: false }`: FX asked
//! for no count, so another attempt would draw as many. Its `b64_json` is strict base64 (a `url`
//! is never fetched), and the bytes, whatever they are, become a PNG ([`crate::wire::to_png`], on
//! a blocking thread): file `image` (`image/png`), `data: null`, `cost` from `usage`
//! ([`super::usage_cost`]). Any other 2xx that cannot become an answer is `Failed { cost,
//! retryable: true }`. Deadline and response cap as GPT Image's ([`super::image`]). `check`: every
//! image route's ([`crate::checks::image`]).

use super::{BODY_TOO_LARGE, classify, not_served, serves, with_request_id};
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Identity, RequestAdapter, Sent};
use crate::setup::Client;
use crate::transport::{Body, HttpRequest, HttpResponse, Lane, Method};
use crate::wire;
use serde_json::{Map, Value, json};

/// The label of every reason.
pub const LABEL: &str = super::image::LABEL;

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "openrouter-image";

/// The contract this adapter serves: [`CONTRACT_ADAPTER`], and `adapter_behavior` the text `"1"`.
pub const IDENTITY: Identity = Identity {
    adapter: CONTRACT_ADAPTER,
    behavior: r#""1""#,
};

/// The members provider options may not set (spec/adapters.md §8; src/openrouter/README.md):
/// every member the body has, and those that would change what the answer holds.
pub const OWNED: &[&str] = &[
    "model",
    "prompt",
    "input_references",
    "n",
    "response_format",
    "stream",
];

/// The adapter, as its refusals of what it cannot send name it.
const WHO: &str = "OpenRouter's generic image adapter";

/// The answer's picture, as the refusals of its PNG name it.
const IMAGE: &str = "OpenRouter's image";

/// The capabilities this adapter is registered for.
const CAPABILITIES: [&str; 2] = ["image.generate", "image.edit"];

/// OpenRouter's image adapter for any model (module doc).
#[derive(Debug, Clone)]
pub struct OpenRouterGenericImages {
    client: Client,
}

impl OpenRouterGenericImages {
    pub fn new(client: Client) -> OpenRouterGenericImages {
        OpenRouterGenericImages { client }
    }

    /// Every refusal of spec/adapters.md §4, in order, then the request to send.
    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let route = &call.route;
        if !CAPABILITIES.contains(&route.capability.as_str())
            || !serves(&route.contract, CONTRACT_ADAPTER, |v| IDENTITY.behaves(v))
        {
            return Err(not_served(call));
        }
        crate::capabilities::check_request(&route.capability, &call.request)?;
        let credential = self.client.credential()?;
        let options = wire::provider_options(call, OWNED)?;
        let image = wire::generic_image(call, WHO, LABEL, true)?;

        let mut pictures = Vec::new();
        for (member, value) in &image.pictures {
            let file = wire::request_file(call, value, member)?;
            if !file.kind.starts_with("image/") {
                return Err(format!("{member} is {}, not a picture", file.kind));
            }
            let bytes = wire::read_file(file, member)?;
            pictures.push(json!({
                "type": "image_url",
                "image_url": {"url": wire::data_url(&file.kind, &bytes)},
            }));
        }

        let mut body = Map::new();
        body.insert("model".into(), json!(route.model.trim()));
        body.insert("prompt".into(), json!(image.prompt));
        if !pictures.is_empty() {
            body.insert("input_references".into(), Value::Array(pictures));
        }
        body.extend(options);
        let body = Value::Object(body);
        if wire::body_exceeds(&body, super::MAX_REQUEST_BYTES) {
            return Err(BODY_TOO_LARGE.into());
        }
        Ok(
            HttpRequest::new(Method::Post, self.client.url("images"), Lane::Provider)
                .credential(credential)
                .body(Body::Json(body))
                .timeout(super::image::DEADLINE)
                .max_response_bytes(super::image::MAX_RESPONSE_BYTES),
        )
    }

    /// A 2xx response as an answer, or a failure (module doc).
    async fn answer(&self, response: &HttpResponse) -> Sent {
        let failed = |reason: String, cost, retryable| Sent::Failed {
            reason: self.client.reason(&with_request_id(reason, response)),
            cost,
            retryable,
        };
        let payload = match wire::json_object(&response.body, LABEL) {
            Ok(payload) => payload,
            Err(reason) => return failed(reason, None, true),
        };
        let cost = super::usage_cost(payload.get("usage"));
        let item = match payload.get("data") {
            Some(Value::Array(items)) if items.len() == 1 => items[0].as_object(),
            _ => None,
        };
        let Some(item) = item else {
            return failed(format!("{LABEL} returned no single image"), cost, false);
        };
        let Some(bytes) = item
            .get("b64_json")
            .and_then(Value::as_str)
            .and_then(wire::strict_base64)
        else {
            return failed(
                "OpenRouter image b64_json is not valid base64".into(),
                cost,
                true,
            );
        };
        match wire::to_png_blocking(bytes, IMAGE).await {
            Ok(png) => {
                Sent::Answered(Answer::new(Value::Null, cost).with_file("image", "image/png", png))
            }
            Err(reason) => failed(reason, cost, true),
        }
    }
}

impl RequestAdapter for OpenRouterGenericImages {
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
                Ok(response) => self.answer(&response).await,
                Err(sent) => sent,
            }
        })
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(image) = answer.files.get("image") else {
            return Err("the answer has no image".into());
        };
        let size = wire::parse_size(call.request.get("size"))?;
        crate::checks::image(call, image, size)
    }

    fn identity(&self) -> Option<Identity> {
        Some(IDENTITY)
    }
}
