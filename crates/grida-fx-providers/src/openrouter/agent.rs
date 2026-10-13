//! OpenRouter agent turns: one `agent.turn` (spec/protocol.md §6.2 step 3) as one
//! `POST {base}/chat/completions` with strict function tools (src/openrouter/README.md;
//! spec/capabilities.md §5).
//!
//! Contract: `adapter: openrouter-tool-loop`, `adapter_behavior: 1`, optional `request_policy`
//! ([`super::structured::RequestPolicy`]). The engine's transcript maps to the wire: a system
//! message when `system` is non-empty; user messages with pictures as content parts (`detail` from
//! the policy); assistant messages with `content: null` when empty and `tool_calls` whose
//! `arguments` string is passed through byte for byte; tool messages as `{"role": "tool",
//! "tool_call_id", "content"}` (no `name`), their pictures regrouped into a following user message
//! `Pictures the tools returned.`. Tools: `{"type": "function", "function": {"name",
//! "description", "parameters": <strict schema>, "strict": true}}`. The answer is
//! `data: {"text", "tool_calls": [{"id", "name", "arguments": <object>}]}`, `cost` from
//! `usage` ([`super::usage_cost`]); the engine checks its shape. Deadline 600 s.
//!
//! `send`, in the order of spec/adapters.md §4 (the engine makes steps 1 and 2 before it, the
//! contract's other members through `check_contract`; the adapter makes them again for a direct
//! caller):
//! 1. the contract (`structured::read_contract`, no `pictures` member);
//! 2. `capabilities::check_request`;
//! 3. the key (`authorization: Bearer <OPENROUTER_API_KEY>`);
//! 4. values: each tool's name (`^[a-z][a-z0-9_]{0,63}$`), non-blank description and object
//!    `parameters`; `max_tokens` at least 1; the transcript (known roles, text contents, an id on
//!    every assistant call and every tool message, pictures as file values);
//! 5. files: every picture, as a data URL of its kind. A file that is not `image/*` is refused
//!    before any file is read: `messages[<i>].images[<j>] is <kind>, not a picture`;
//! 6. a body over 200 MiB is refused; otherwise one exchange.
//!
//! Pictures are held while tool messages follow one another and are sent, before the next
//! message that is not a tool message and after the last message, as one user message
//! [`TOOL_PICTURES`]: only user messages carry pictures on the wire. `tool_choice` is `auto` only
//! when the request says `auto`, else `required`.

use super::structured;
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Identity, RequestAdapter, Sent};
use crate::capabilities;
use crate::chat;
use crate::setup::Client;
use crate::transport::{Credential, HttpResponse};
use crate::wire;
use serde_json::{Map, Value, json};
use std::time::Duration;

pub use crate::chat::TOOL_PICTURES;

/// The label of every reason.
pub const LABEL: &str = "OpenRouter tool loop";

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "openrouter-tool-loop";

pub const DEADLINE: Duration = Duration::from_secs(600);

/// The capability this adapter serves.
pub const CAPABILITY: &str = "agent.turn";

/// The contract this adapter serves: [`CONTRACT_ADAPTER`], behaviour the number 1.
pub const IDENTITY: Identity = Identity {
    adapter: CONTRACT_ADAPTER,
    behavior: Some("1"),
};

/// OpenRouter's agent-turn adapter (module doc).
#[derive(Debug, Clone)]
pub struct OpenRouterAgent {
    client: Client,
}

/// A turn ready to send.
struct Prepared {
    credential: Credential,
    body: Value,
}

impl OpenRouterAgent {
    pub fn new(client: Client) -> OpenRouterAgent {
        OpenRouterAgent { client }
    }

    /// Steps 1 to 5 of the module doc, and the body.
    fn prepare(&self, call: &CallRequest) -> Result<Prepared, String> {
        // 1. The contract.
        let contract = structured::read_contract(&call.route, CAPABILITY, CONTRACT_ADAPTER, false)?;
        // 2. The capability.
        capabilities::check_request(CAPABILITY, &call.request)?;
        // 3. The key.
        let credential = self.client.credential("authorization", "Bearer ")?;
        // 4 and 5. Values, then files.
        let policy = &contract.policy;
        let prompt = chat::agent_prompt(call, policy.image_detail.as_deref())?;
        // The body (src/openrouter/README.md).
        let mut body = Map::new();
        body.insert(
            "model".into(),
            Value::String(call.route.model.trim().into()),
        );
        body.insert("messages".into(), Value::Array(prompt.messages));
        body.insert("tools".into(), Value::Array(prompt.tools));
        body.insert("tool_choice".into(), json!(prompt.tool_choice));
        body.insert("provider".into(), policy.provider.clone());
        if let Some(reasoning) = policy.reasoning() {
            body.insert("reasoning".into(), reasoning);
        }
        if let Some(max_tokens) = prompt.max_tokens {
            body.insert("max_tokens".into(), json!(max_tokens));
        }
        let body = Value::Object(body);
        if wire::body_exceeds(&body, super::MAX_REQUEST_BYTES) {
            return Err(super::BODY_TOO_LARGE.into());
        }
        Ok(Prepared { credential, body })
    }

    /// A 2xx's body as an answer (src/openrouter/README.md).
    fn parse(&self, response: &HttpResponse) -> Sent {
        let body = match wire::json_object(&response.body, LABEL) {
            Ok(body) => body,
            Err(reason) => return structured::structural(&self.client, response, &reason, None),
        };
        let cost = structured::reported_cost(&body);
        let failed = |what: &str| {
            structured::structural(&self.client, response, &format!("{LABEL} {what}"), cost)
        };
        let Some(message) = chat::first_message(&body) else {
            return failed("returned no message");
        };
        match chat::agent_reply(message) {
            Ok(reply) => Sent::Answered(Answer::new(reply, cost)),
            Err(what) => failed(what),
        }
    }
}

impl RequestAdapter for OpenRouterAgent {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            let prepared = match self.prepare(call) {
                Ok(prepared) => prepared,
                Err(reason) => {
                    return Sent::Refused {
                        reason: self.client.reason(&reason),
                    };
                }
            };
            let request = structured::chat_request(
                &self.client,
                prepared.credential,
                prepared.body,
                DEADLINE,
            );
            match super::classify(&self.client, LABEL, self.client.send(request).await) {
                Ok(response) => self.parse(&response),
                Err(sent) => sent,
            }
        })
    }

    fn identity(&self) -> Option<Identity> {
        Some(IDENTITY)
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        structured::read_contract(&call.route, CAPABILITY, CONTRACT_ADAPTER, false).map(|_| ())
    }
}
