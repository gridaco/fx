//! fal background removal: `POST {run}/<model>` (`fal-ai/birefnet/v2`) with `{"image_url",
//! "model": "General Use (Light)", "operating_resolution": "1024x1024", "output_mask": false,
//! "refine_foreground": true, "output_format": "png", "mask_only": false, "sync_mode": true}`
//! (spec/providers.md §9.3). The answer `root.image.url` is a data URI (decoded) or a `*.fal.media`
//! URL (downloaded as for images). File `image` (`image/png`), `data: null`, cost from `usage.cost`.
//! `check`: a PNG with its signature. Deadline 300 s. Contract `adapter` must be
//! `fal-run-birefnet` when present. No route in the default table serves it yet (no published
//! price has been recorded).
//!
//! Refused before sending, in this order: another route's contract; a request that does not fit
//! `background.remove`; no `FAL_KEY`; an `image` with no bytes. Statuses are the run host's, as for
//! images (spec/providers.md §9.3). The answer may declare PNG, WebP or GIF; `width`, `height` and
//! `mask_image` are not read.

use super::FalClients;
use super::output::{self, Rules};
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, RequestAdapter, Sent};
use crate::transport::{Body, HttpRequest, Lane, Method};
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

/// The label of every reason.
pub const LABEL: &str = "fal background removal";

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "fal-run-birefnet";

/// The POST and any download together.
pub const DEADLINE: Duration = Duration::from_secs(300);

/// What a background removal may declare.
const RULES: Rules = Rules {
    allowed: &["image/png", "image/webp", "image/gif"],
    check_dimensions: false,
    max_bytes: super::image::MAX_OUTPUT_BYTES,
    allowed_words: "PNG, WebP, or GIF",
};

/// fal's background-removal adapter (module doc).
#[derive(Debug, Clone)]
pub struct FalBackground {
    clients: FalClients,
}

impl FalBackground {
    pub fn new(clients: FalClients) -> FalBackground {
        FalBackground { clients }
    }

    /// Steps 1 to 5 of spec/providers.md §5, then the request (module doc).
    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let client = &self.clients.run;
        if let Some(refusal) =
            super::contract_refusal(call, CONTRACT_ADAPTER, &["background.remove"])
        {
            return Err(refusal);
        }
        crate::capabilities::check_request(&call.route.capability, &call.request)?;
        let credential = client.credential(super::CREDENTIAL_HEADER, super::CREDENTIAL_PREFIX)?;
        let file = crate::wire::request_file(call, &call.request["image"], "image")?;
        let bytes = crate::wire::read_file(file, "image")?;

        let mut body = Map::new();
        body.insert(
            "image_url".into(),
            Value::String(super::picture_url(&file.kind, &bytes)),
        );
        body.insert("model".into(), json!("General Use (Light)"));
        body.insert("operating_resolution".into(), json!("1024x1024"));
        body.insert("output_mask".into(), json!(false));
        body.insert("refine_foreground".into(), json!(true));
        body.insert("output_format".into(), json!("png"));
        body.insert("mask_only".into(), json!(false));
        body.insert("sync_mode".into(), json!(true));

        let url = client.url(super::endpoint(&call.route.model));
        Ok(HttpRequest::new(Method::Post, url, Lane::Provider)
            .credential(credential)
            .body(Body::Json(Value::Object(body)))
            .timeout(DEADLINE)
            .max_response_bytes(super::image::MAX_RESPONSE_BYTES))
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
        let Some(Value::Object(image)) = super::answer_root(&payload).get("image") else {
            return failed(client.reason(&format!("{LABEL} returned no image")), cost);
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

impl RequestAdapter for FalBackground {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(self.send_once(call))
    }

    fn check(&self, _call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(file) = answer.files.get("image") else {
            return Err("the answer carries no image".into());
        };
        if file.kind == "image/png" && crate::wire::matches_signature("image/png", &file.bytes) {
            return Ok(());
        }
        let found = if file.kind == "image/png" {
            crate::wire::sniff_image(&file.bytes).unwrap_or("not an image")
        } else {
            file.kind.as_str()
        };
        Err(format!("the answer is {found}, not image/png"))
    }
}
