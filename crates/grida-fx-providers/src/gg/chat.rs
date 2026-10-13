//! GG text: `structured.generate` and `agent.turn`, each one `POST {base}/chat/completions`
//! answered synchronously (src/gg/README.md; spec/capabilities.md §4–§5).
//!
//! The messages, the strict JSON-schema response format, the strict function tools and the
//! answers are [`crate::chat`]'s, the same as OpenRouter's. The body adds nothing GG would have to
//! route: `{model, messages, response_format, max_completion_tokens, reasoning_effort?}` for
//! structured output, `{model, messages, tools, tool_choice, max_completion_tokens?,
//! reasoning_effort?}` for a turn. Every request carries its attempt's `Idempotency-Key`
//! ([`super::Gateway`]). An answer is priced by [`super::Gateway::priced`] (GG's settled charge
//! when it gives one, its `x-gg-reserved-mills` as the cost bound), and the tokens its `usage`
//! counts are the answer's usage.
//!
//! Contracts:
//! - [`GgStructured`]: `adapter: gg-structured`, behaviour `1`, optional `image_detail` (`auto`,
//!   `low` or `high`, on every picture), `reasoning_effort` (one of [`REASONING_EFFORTS`], sent as
//!   given) and `pictures` (`"unchanged"`: sent as they are; absent: fitted into 1600 × 1600 and
//!   flattened on `matte`). Deadline [`STRUCTURED_DEADLINE`].
//! - [`GgAgent`]: `adapter: gg-tool-loop`, behaviour `1`, optional `image_detail` and
//!   `reasoning_effort`. Deadline [`TURN_DEADLINE`].
//!
//! A send, in order (spec/adapters.md §4; the engine makes the first two before it, the
//! contract's other members through `check_contract`, and the adapter again for a direct
//! caller): the contract; the capability's check; the key
//! (`GG_API_KEY is not set`); the values and files of [`crate::chat`]; a body over
//! [`MAX_REQUEST_BYTES`], GG's limit, is refused (`request body exceeds 20 MiB`); then one
//! exchange, classified by [`super::classify`] ([`super::Gateway::exchange`]). A 2xx that does
//! not become an answer is `Failed { retryable: true }`, at GG's settled charge, else `None`
//! (spec/adapters.md §3.4).

use super::Gateway;
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Identity, RequestAdapter, Sent, Usage};
use crate::capabilities;
use crate::chat;
use crate::checks;
use crate::transport::{Body, HttpRequest, HttpResponse, Lane, Method};
use crate::wire;
use serde_json::{Map, Value, json};
use std::time::Duration;

/// The label of a structured call's reasons.
pub const STRUCTURED_LABEL: &str = "GG structured generation";

/// The label of an agent turn's reasons.
pub const TURN_LABEL: &str = "GG tool loop";

/// The contract `adapter` of [`GgStructured`].
pub const STRUCTURED_ADAPTER: &str = "gg-structured";

/// The contract `adapter` of [`GgAgent`].
pub const TURN_ADAPTER: &str = "gg-tool-loop";

/// The contract [`GgStructured`] serves: [`STRUCTURED_ADAPTER`], behaviour the number 1.
pub const STRUCTURED_IDENTITY: Identity = Identity {
    adapter: STRUCTURED_ADAPTER,
    behavior: Some("1"),
};

/// The contract [`GgAgent`] serves: [`TURN_ADAPTER`], behaviour the number 1.
pub const TURN_IDENTITY: Identity = Identity {
    adapter: TURN_ADAPTER,
    behavior: Some("1"),
};

/// The deadline of a structured call.
pub const STRUCTURED_DEADLINE: Duration = Duration::from_secs(900);

/// The deadline of an agent turn.
pub const TURN_DEADLINE: Duration = Duration::from_secs(600);

/// The response cap of a chat completion.
pub const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

/// GG's limit on a text request's body, base64 pictures included.
pub const MAX_REQUEST_BYTES: usize = 20 * 1024 * 1024;

/// The refusal of a body over [`MAX_REQUEST_BYTES`].
pub const BODY_TOO_LARGE: &str = "request body exceeds 20 MiB";

/// The `image_detail` values a contract may give.
pub const IMAGE_DETAILS: [&str; 3] = ["auto", "low", "high"];

/// The `reasoning_effort` values a contract may give.
pub const REASONING_EFFORTS: [&str; 6] = ["none", "minimal", "low", "medium", "high", "xhigh"];

/// What a chat contract asks: the pictures' `detail`, the reasoning effort, and whether
/// structured pictures go as they are.
struct Contract {
    detail: Option<String>,
    reasoning_effort: Option<String>,
    pictures_unchanged: bool,
}

/// Reads a GG chat contract (module doc) for the adapter of `identity` serving `capability`.
fn contract(
    call: &CallRequest,
    capability: &str,
    identity: &Identity,
    takes_pictures: bool,
) -> Result<Contract, String> {
    let members: &[&str] = if takes_pictures {
        &["image_detail", "reasoning_effort", "pictures"]
    } else {
        &["image_detail", "reasoning_effort"]
    };
    let contract = super::read_contract(
        call,
        &[capability],
        identity.adapter,
        |v| identity.behaves(v),
        members,
    )?;
    let detail = match contract.get("image_detail") {
        None => None,
        Some(Value::String(detail)) if IMAGE_DETAILS.contains(&detail.as_str()) => {
            Some(detail.clone())
        }
        Some(_) => {
            return Err(super::not_served(
                call,
                Some(&format!(
                    "image_detail is one of {}",
                    IMAGE_DETAILS.join(", ")
                )),
            ));
        }
    };
    let reasoning_effort = match contract.get("reasoning_effort") {
        None => None,
        Some(Value::String(effort)) if REASONING_EFFORTS.contains(&effort.as_str()) => {
            Some(effort.clone())
        }
        Some(_) => {
            return Err(super::not_served(
                call,
                Some(&format!(
                    "reasoning_effort is one of {}",
                    REASONING_EFFORTS.join(", ")
                )),
            ));
        }
    };
    let pictures_unchanged = match contract.get("pictures") {
        None => false,
        Some(Value::String(pictures)) if pictures == "unchanged" => true,
        Some(_) => {
            return Err(super::not_served(
                call,
                Some("pictures is \"unchanged\" or absent"),
            ));
        }
    };
    Ok(Contract {
        detail,
        reasoning_effort,
        pictures_unchanged,
    })
}

/// The one chat completions request of a call.
fn request(
    gateway: &Gateway,
    call: &CallRequest,
    body: Map<String, Value>,
    timeout: Duration,
) -> Result<HttpRequest, String> {
    let body = Value::Object(body);
    if wire::body_exceeds(&body, MAX_REQUEST_BYTES) {
        return Err(BODY_TOO_LARGE.into());
    }
    let credential = gateway.credential()?;
    Ok(gateway.paid(
        HttpRequest::new(
            Method::Post,
            gateway.client.url("chat/completions"),
            Lane::Provider,
        )
        .body(Body::Json(body))
        .timeout(timeout)
        .max_response_bytes(MAX_RESPONSE_BYTES),
        credential,
        call,
    ))
}

/// One exchange of `call`'s prepared request, and its answer: `answer` reads the first message.
async fn exchange(
    gateway: &Gateway,
    call: &CallRequest,
    label: &str,
    prepared: Result<HttpRequest, String>,
    answer: impl Fn(&Map<String, Value>) -> Result<Value, &'static str>,
) -> Sent {
    let request = match prepared {
        Ok(request) => request,
        Err(reason) => {
            return Sent::Refused {
                reason: gateway.reason(&reason),
            };
        }
    };
    let response = match gateway.exchange(call, label, request).await {
        Ok(response) => response,
        Err(sent) => return sent,
    };
    let read = wire::json_object(&response.body, label).and_then(|body| {
        let message =
            chat::first_message(&body).ok_or_else(|| format!("{label} returned no message"))?;
        let data = answer(message).map_err(|what| format!("{label} {what}"))?;
        Ok(match usage(&body) {
            Some(usage) => Answer::new(data, None).with_usage(usage),
            None => Answer::new(data, None),
        })
    });
    match read {
        Ok(answer) => Sent::Answered(gateway.priced(answer, &response).await),
        Err(reason) => {
            let failed = structural(gateway, &response, &reason);
            gateway.charged(failed, &response).await
        }
    }
}

/// The tokens a completion's `usage` counts, when it counts any: `prompt_tokens` (cache reads
/// included), `prompt_tokens_details.cached_tokens` and `completion_tokens` (reasoning included).
fn usage(body: &Map<String, Value>) -> Option<Usage> {
    let usage = body.get("usage")?.as_object()?;
    let count = |value: Option<&Value>| value.and_then(Value::as_u64);
    let counted = Usage {
        input_tokens: count(usage.get("prompt_tokens")),
        cached_input_tokens: count(
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens")),
        ),
        output_tokens: count(usage.get("completion_tokens")),
        ..Usage::default()
    };
    (counted != Usage::default()).then_some(counted)
}

/// A structural failure of a 2xx (spec/adapters.md §3.4): billed, retryable.
fn structural(gateway: &Gateway, response: &HttpResponse, reason: &str) -> Sent {
    Sent::Failed {
        reason: gateway.reason(&super::with_request_id(reason.to_string(), response)),
        cost: None,
        retryable: true,
    }
}

/// GG's structured-output adapter (module doc).
#[derive(Debug, Clone)]
pub struct GgStructured {
    gateway: Gateway,
}

impl GgStructured {
    pub fn new(gateway: Gateway) -> GgStructured {
        GgStructured { gateway }
    }

    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let capability = "structured.generate";
        let contract = contract(call, capability, &STRUCTURED_IDENTITY, true)?;
        capabilities::check_request(capability, &call.request)?;
        self.gateway.credential()?;
        let prompt = chat::structured_prompt(
            call,
            contract.pictures_unchanged,
            contract.detail.as_deref(),
        )?;
        let mut body = Map::new();
        body.insert("model".into(), json!(call.route.model.trim()));
        body.insert("messages".into(), Value::Array(prompt.messages));
        body.insert("response_format".into(), prompt.response_format);
        body.insert("max_completion_tokens".into(), json!(prompt.max_tokens));
        if let Some(effort) = contract.reasoning_effort {
            body.insert("reasoning_effort".into(), json!(effort));
        }
        request(&self.gateway, call, body, STRUCTURED_DEADLINE)
    }
}

impl RequestAdapter for GgStructured {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            exchange(
                &self.gateway,
                call,
                STRUCTURED_LABEL,
                self.prepare(call),
                |message| chat::structured_value(message).map(|value| json!({"json": value})),
            )
            .await
        })
    }

    /// The answer's value against the request's original schema (spec/capabilities.md §4).
    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        checks::structured(call, answer, &self.gateway.client.redactor)
    }

    fn identity(&self) -> Option<Identity> {
        Some(STRUCTURED_IDENTITY)
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        contract(call, "structured.generate", &STRUCTURED_IDENTITY, true).map(|_| ())
    }
}

/// GG's agent-turn adapter (module doc).
#[derive(Debug, Clone)]
pub struct GgAgent {
    gateway: Gateway,
}

impl GgAgent {
    pub fn new(gateway: Gateway) -> GgAgent {
        GgAgent { gateway }
    }

    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let capability = "agent.turn";
        let contract = contract(call, capability, &TURN_IDENTITY, false)?;
        capabilities::check_request(capability, &call.request)?;
        self.gateway.credential()?;
        let prompt = chat::agent_prompt(call, contract.detail.as_deref())?;
        let mut body = Map::new();
        body.insert("model".into(), json!(call.route.model.trim()));
        body.insert("messages".into(), Value::Array(prompt.messages));
        body.insert("tools".into(), Value::Array(prompt.tools));
        body.insert("tool_choice".into(), json!(prompt.tool_choice));
        if let Some(max_tokens) = prompt.max_tokens {
            body.insert("max_completion_tokens".into(), json!(max_tokens));
        }
        if let Some(effort) = contract.reasoning_effort {
            body.insert("reasoning_effort".into(), json!(effort));
        }
        request(&self.gateway, call, body, TURN_DEADLINE)
    }
}

impl RequestAdapter for GgAgent {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            exchange(
                &self.gateway,
                call,
                TURN_LABEL,
                self.prepare(call),
                chat::agent_reply,
            )
            .await
        })
    }

    fn identity(&self) -> Option<Identity> {
        Some(TURN_IDENTITY)
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        contract(call, "agent.turn", &TURN_IDENTITY, false).map(|_| ())
    }
}
