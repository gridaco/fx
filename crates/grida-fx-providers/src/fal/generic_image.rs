//! fal images on any model: `POST {run}/<model>` for `image.generate` and `image.edit`, the route's
//! model being fal's endpoint id as given, then one download of a hosted output; the default of
//! both on `fal` (src/fal/README.md, "Images on any model"; spec/providers.md §2). It sends what
//! fal's image endpoints share, and nothing FX chose for one model: a model's own parameters come
//! in `provider_options.fal`.
//!
//! Contract `adapter: fal-image`, `adapter_behavior: "1"` (a string). Body members in order:
//! `prompt`, then `image_urls` on an edit (`image` first, then `references`, data URLs under their
//! own kind), then the provider options. Refused before sending, in the order of spec/adapters.md
//! §4: another route's contract; a request that does not fit its capability; no `FAL_KEY`;
//! provider options that set a member of [`OWNED`] ([`crate::wire::provider_options`]); what
//! [`crate::wire::generic_image`] refuses, references on a generate included; then, in the order
//! `image`, `references[<i>]`, a picture that is not an `image/*` file or has no bytes.
//!
//! The answer is `root.images`, which must hold exactly one object, else `Failed { cost,
//! retryable: false }`: FX asks for no count, so another attempt would draw as many. It is read as
//! GPT Image's ([`super::image`]: a data URL, or one `fal.media` download, a declared PNG, JPEG or
//! WebP), then becomes a PNG ([`crate::wire::to_png`], on a blocking thread): file `image`
//! (`image/png`), `data: null`. Deadline, caps and `cost` as GPT Image's. `check`: every image
//! route's ([`crate::checks::image`]).

use super::FalClients;
use super::image::{DEADLINE, LABEL, MAX_RESPONSE_BYTES, RULES};
use super::output;
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Identity, RequestAdapter, Sent};
use crate::transport::{Body, HttpRequest, Lane, Method};
use crate::wire;
use serde_json::{Map, Value};
use std::time::Instant;

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "fal-image";

/// The contract this adapter serves: [`CONTRACT_ADAPTER`], and `adapter_behavior` the text `"1"`.
pub const IDENTITY: Identity = Identity {
    adapter: CONTRACT_ADAPTER,
    behavior: r#""1""#,
};

/// The members provider options may not set (spec/adapters.md §8; src/fal/README.md): every
/// member the body has, `num_images`, which would change how many pictures the answer holds, and
/// `sync_mode`, which changes how fal returns its output.
pub const OWNED: &[&str] = &["prompt", "image_urls", "num_images", "sync_mode"];

/// The adapter, as its refusals of what it cannot send name it.
const WHO: &str = "fal's generic image adapter";

/// The answer's picture, as the refusals of its PNG name it.
const IMAGE: &str = "fal's image";

/// fal's image adapter for any model (module doc).
#[derive(Debug, Clone)]
pub struct FalGenericImages {
    clients: FalClients,
}

impl FalGenericImages {
    pub fn new(clients: FalClients) -> FalGenericImages {
        FalGenericImages { clients }
    }

    /// Steps 1 to 5 of spec/adapters.md §4, then the request (module doc).
    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let client = &self.clients.run;
        if let Some(refusal) =
            super::contract_refusal(call, &IDENTITY, &["image.generate", "image.edit"])
        {
            return Err(refusal);
        }
        crate::capabilities::check_request(&call.route.capability, &call.request)?;
        let credential = client.credential(super::CREDENTIAL_HEADER, super::CREDENTIAL_PREFIX)?;
        let options = wire::provider_options(call, OWNED)?;
        let image = wire::generic_image(call, WHO, LABEL, false)?;

        let mut body = Map::new();
        body.insert("prompt".into(), Value::String(image.prompt.to_string()));
        if !image.pictures.is_empty() {
            let mut urls = Vec::new();
            for (member, value) in &image.pictures {
                urls.push(Value::String(super::picture(call, value, member)?));
            }
            body.insert("image_urls".into(), Value::Array(urls));
        }
        body.extend(options);

        let url = client.url(super::endpoint(&call.route.model));
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
        let failed = |reason: String, cost, retryable| Sent::Failed {
            reason,
            cost,
            retryable,
        };
        let payload = match wire::json_object(&response.body, LABEL) {
            Ok(payload) => payload,
            Err(reason) => return failed(client.reason(&reason), None, true),
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
                let reason = client.reason(&format!("{LABEL} returned no single image"));
                return failed(reason, cost, false);
            }
        };
        let remaining = DEADLINE.saturating_sub(started.elapsed());
        let picture = match output::read_picture(client, image, RULES, remaining).await {
            Ok(picture) => picture,
            Err(reason) => return failed(reason, cost, true),
        };
        match wire::to_png_blocking(picture.bytes, IMAGE).await {
            Ok(png) => {
                Sent::Answered(Answer::new(Value::Null, cost).with_file("image", "image/png", png))
            }
            Err(reason) => failed(client.reason(&reason), cost, true),
        }
    }
}

impl RequestAdapter for FalGenericImages {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(self.send_once(call))
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(file) = answer.files.get("image") else {
            return Err("the answer carries no image".into());
        };
        let size = wire::parse_size(call.request.get("size"))?;
        crate::checks::image(call, file, size)
    }

    fn identity(&self) -> Option<Identity> {
        Some(IDENTITY)
    }
}
