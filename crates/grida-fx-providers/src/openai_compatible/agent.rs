//! The `openai-compatible` family's agent turns: one `agent.turn` (spec/protocol.md §6.2 step 3) as
//! one `POST {base}/chat/completions` with strict function tools (src/openai_compatible/README.md;
//! spec/capabilities.md §5).
//!
//! Contract: `adapter: openai-compatible-tool-loop`, `adapter_behavior: 1`, and optional
//! `provider_options` (spec/adapters.md §8: the engine's agent loop sends none, so an agent run's
//! turns take only the route's). The transcript, the tools and the reply are [`crate::chat`]'s, as
//! on OpenRouter, with no `detail` on a picture. The body is `{model, messages, tools, tool_choice,
//! max_tokens?}`, then the provider options. The answer is `data: {"text", "tool_calls"}`, at no
//! cost, with its token usage; the engine checks its shape. Deadline [`DEADLINE`].
//!
//! `send`, in the order of spec/adapters.md §4 (the engine makes steps 1 and 2 before it, the
//! contract's other members through `check_contract`; the adapter makes them again for a direct
//! caller): the contract ([`super::read_contract`]); `capabilities::check_request`; the key, when
//! the provider is declared with one; provider options that set no member of [`OWNED`]
//! ([`wire::provider_options`]); [`chat::agent_prompt`]'s values and files; a body over 200 MiB;
//! then one exchange, classified by [`super::classify`]. A 2xx that does not become an answer is
//! `Failed { cost: None, retryable: true }`.

use super::Provider;
use crate::BoxFuture;
use crate::adapter::{CallRequest, Identity, RequestAdapter, Sent};
use crate::capabilities;
use crate::chat;
use crate::transport::{Credential, HttpResponse};
use crate::wire;
use serde_json::{Map, Value, json};
use std::time::Duration;

/// The capability this adapter serves.
pub const CAPABILITY: &str = "agent.turn";

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "openai-compatible-tool-loop";

/// The contract this adapter serves: [`CONTRACT_ADAPTER`], behaviour the number 1.
pub const IDENTITY: Identity = Identity {
    adapter: CONTRACT_ADAPTER,
    behavior: "1",
};

/// The members provider options may not set (spec/adapters.md §8;
/// src/openai_compatible/README.md), as [`super::structured::OWNED`].
pub const OWNED: &[&str] = &[
    "model",
    "messages",
    "tools",
    "tool_choice",
    "max_tokens",
    "max_completion_tokens",
    "stream",
    "n",
    "response_format",
];

/// The deadline of an agent turn.
pub const DEADLINE: Duration = Duration::from_secs(600);

/// A declared provider's agent-turn adapter (module doc).
#[derive(Debug, Clone)]
pub struct CompatibleAgent {
    provider: Provider,
    /// `<name> tool loop`.
    label: String,
}

impl CompatibleAgent {
    pub fn new(provider: Provider) -> CompatibleAgent {
        let label = format!("{} tool loop", provider.name);
        CompatibleAgent { provider, label }
    }

    /// The module doc's refusals, then the credential and the body.
    fn prepare(&self, call: &CallRequest) -> Result<(Option<Credential>, Value), String> {
        super::read_contract(&call.route, CAPABILITY, &IDENTITY)?;
        capabilities::check_request(CAPABILITY, &call.request)?;
        let credential = self.provider.client.optional_credential()?;
        let options = wire::provider_options(call, OWNED)?;
        let prompt = chat::agent_prompt(call, None)?;
        let mut body = Map::new();
        body.insert("model".into(), json!(call.route.model.trim()));
        body.insert("messages".into(), Value::Array(prompt.messages));
        body.insert("tools".into(), Value::Array(prompt.tools));
        body.insert("tool_choice".into(), json!(prompt.tool_choice));
        if let Some(max_tokens) = prompt.max_tokens {
            body.insert("max_tokens".into(), json!(max_tokens));
        }
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
        match chat::agent_reply(message) {
            Ok(reply) => Sent::Answered(super::answer(reply, &body)),
            Err(what) => failed(what),
        }
    }
}

impl RequestAdapter for CompatibleAgent {
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

    fn identity(&self) -> Option<Identity> {
        Some(IDENTITY)
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        super::read_contract(&call.route, CAPABILITY, &IDENTITY)
    }
}
