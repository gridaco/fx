//! The `openai-compatible` family's structured output: `POST {base}/chat/completions` with a strict
//! JSON-schema `response_format` (src/openai_compatible/README.md; spec/capabilities.md §4).
//!
//! Contract: `adapter: openai-compatible-structured`, `adapter_behavior: 1`, and optional
//! `provider_options` (spec/adapters.md §8). The prompt, the schema, the context files and the
//! answer are [`crate::chat`]'s, as on OpenRouter; pictures are always fitted into 1600 × 1600 and
//! flattened on `matte`, with no `detail`. The body is `{model, messages, response_format,
//! max_tokens}`, then the provider options. The answer is `data: {"json": <value>}`, at no cost,
//! with its token usage. `check`: the value against the request's original schema
//! ([`checks::structured`]). Deadline [`DEADLINE`].
//!
//! `send`, in the order of spec/adapters.md §4 (the engine makes steps 1 and 2 before it, the
//! contract's other members through `check_contract`; the adapter makes them again for a direct
//! caller):
//! 1. the contract ([`super::read_contract`]);
//! 2. `capabilities::check_request`;
//! 3. the key, when the provider is declared with one (`<VARIABLE> is not set`);
//! 4. values: provider options that set no member of [`OWNED`] ([`wire::provider_options`]), then
//!    [`chat::structured_prompt`]'s;
//! 5. files: [`chat::structured_prompt`]'s;
//! 6. a body over 200 MiB is refused; otherwise one exchange, classified by [`super::classify`].
//!
//! A 2xx that does not become an answer is `Failed { cost: None, retryable: true }`
//! (spec/adapters.md §3.4).

use super::Provider;
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Identity, RequestAdapter, Sent};
use crate::capabilities;
use crate::chat;
use crate::checks;
use crate::transport::{Credential, HttpResponse};
use crate::wire;
use serde_json::{Map, Value, json};
use std::time::Duration;

/// The capability this adapter serves.
pub const CAPABILITY: &str = "structured.generate";

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "openai-compatible-structured";

/// The contract this adapter serves: [`CONTRACT_ADAPTER`], behaviour the number 1.
pub const IDENTITY: Identity = Identity {
    adapter: CONTRACT_ADAPTER,
    behavior: "1",
};

/// The members provider options may not set (spec/adapters.md §8;
/// src/openai_compatible/README.md): every member the body has, `max_completion_tokens`, which
/// would set its token limit twice, and those that would change the answer.
pub const OWNED: &[&str] = &[
    "model",
    "messages",
    "response_format",
    "max_tokens",
    "max_completion_tokens",
    "stream",
    "n",
    "tools",
    "tool_choice",
];

/// The deadline of a structured call.
pub const DEADLINE: Duration = Duration::from_secs(1800);

/// A declared provider's structured-output adapter (module doc).
#[derive(Debug, Clone)]
pub struct CompatibleStructured {
    provider: Provider,
    /// `<name> structured generation`.
    label: String,
}

impl CompatibleStructured {
    pub fn new(provider: Provider) -> CompatibleStructured {
        let label = format!("{} structured generation", provider.name);
        CompatibleStructured { provider, label }
    }

    /// Steps 1 to 6 of the module doc but the exchange: the credential and the body.
    fn prepare(&self, call: &CallRequest) -> Result<(Option<Credential>, Value), String> {
        super::read_contract(&call.route, CAPABILITY, &IDENTITY)?;
        capabilities::check_request(CAPABILITY, &call.request)?;
        let credential = self.provider.client.optional_credential()?;
        let options = wire::provider_options(call, OWNED)?;
        let prompt = chat::structured_prompt(call, false, None)?;
        let mut body = Map::new();
        body.insert("model".into(), json!(call.route.model.trim()));
        body.insert("messages".into(), Value::Array(prompt.messages));
        body.insert("response_format".into(), prompt.response_format);
        body.insert("max_tokens".into(), json!(prompt.max_tokens));
        body.extend(options);
        let body = Value::Object(body);
        if wire::body_exceeds(&body, super::MAX_REQUEST_BYTES) {
            return Err(super::BODY_TOO_LARGE.into());
        }
        Ok((credential, body))
    }

    /// A 2xx's body as an answer.
    fn parse(&self, response: &HttpResponse) -> Sent {
        let client = &self.provider.client;
        let body = match wire::json_object(&response.body, &self.label) {
            Ok(body) => body,
            Err(reason) => return chat::structural(client, response, &reason, None),
        };
        let failed = |what: &str| {
            chat::structural(client, response, &format!("{} {what}", self.label), None)
        };
        let Some(message) = chat::first_message(&body) else {
            return failed("returned no message");
        };
        match chat::structured_value(message) {
            Ok(value) => Sent::Answered(super::answer(json!({"json": value}), &body)),
            Err(what) => failed(what),
        }
    }
}

impl RequestAdapter for CompatibleStructured {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            let client = &self.provider.client;
            let (credential, body) = match self.prepare(call) {
                Ok(prepared) => prepared,
                Err(reason) => {
                    return Sent::Refused {
                        reason: client.reason(&reason),
                    };
                }
            };
            let request = chat::completions_request(
                client,
                credential,
                body,
                DEADLINE,
                super::MAX_RESPONSE_BYTES,
            );
            match super::classify(&self.provider, &self.label, client.send(request).await) {
                Ok(response) => self.parse(&response),
                Err(sent) => sent,
            }
        })
    }

    /// The answer's value against the request's original schema (spec/capabilities.md §4;
    /// [`checks::structured`]).
    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        checks::structured(call, answer, &self.provider.client.redactor)
    }

    fn identity(&self) -> Option<Identity> {
        Some(IDENTITY)
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        super::read_contract(&call.route, CAPABILITY, &IDENTITY)
    }
}
