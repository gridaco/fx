//! OpenRouter agent turns: one `agent.turn` (spec/protocol.md §6.2 step 3) as one
//! `POST {base}/chat/completions` with strict function tools (spec/providers.md §9.2;
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
//! `send`, in the order of spec/providers.md §5:
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

use super::structured::{self, RequestPolicy};
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, RequestAdapter, Sent};
use crate::capabilities;
use crate::setup::Client;
use crate::transport::{Credential, HttpResponse};
use crate::wire;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::sync::LazyLock;
use std::time::Duration;

/// The label of every reason.
pub const LABEL: &str = "OpenRouter tool loop";

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "openrouter-tool-loop";

/// The user message that carries the pictures tools returned.
pub const TOOL_PICTURES: &str = "Pictures the tools returned.";

pub const DEADLINE: Duration = Duration::from_secs(600);

/// The capability this adapter serves.
pub const CAPABILITY: &str = "agent.turn";

/// What a picture is called in a refusal.
const PICTURE: &str = "an agent picture";

static TOOL_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_]{0,63}$").expect("a valid pattern"));

/// The tools of a request as the wire takes them (module doc, step 4).
fn wire_tools(tools: &[Value]) -> Result<Vec<Value>, String> {
    tools
        .iter()
        .enumerate()
        .map(|(i, tool)| {
            let Some(tool) = tool.as_object() else {
                return Err(format!(
                    "tool {} is an object with name, description and parameters",
                    i + 1
                ));
            };
            let name = match tool.get("name") {
                Some(Value::String(name)) if TOOL_NAME.is_match(name) => name,
                Some(Value::String(name)) => {
                    return Err(format!(
                        "tool name {} must be lower_snake_case, at most 64 characters",
                        Value::String(name.clone())
                    ));
                }
                _ => return Err(format!("tool {} has no name", i + 1)),
            };
            match tool.get("description") {
                Some(Value::String(description)) if !description.trim().is_empty() => {}
                _ => return Err(format!("tool {name} must carry a description")),
            }
            let Some(parameters @ Value::Object(_)) = tool.get("parameters") else {
                return Err(format!(
                    "tool {name} parameters must be a JSON Schema object"
                ));
            };
            Ok(json!({"type": "function", "function": {
                "name": name,
                "description": tool["description"],
                "parameters": super::schema::strict(parameters),
                "strict": true
            }}))
        })
        .collect()
}

/// A message's `content`: text, `""` when absent or `null`.
fn content<'a>(entry: &'a Map<String, Value>, role: &str) -> Result<&'a str, String> {
    match entry.get("content") {
        None | Some(Value::Null) => Ok(""),
        Some(Value::String(text)) => Ok(text),
        Some(_) => Err(format!(
            "an agent transcript's {role} message content is text"
        )),
    }
}

/// Makes a picture's URL from its file value and its place in the request
/// (`messages[<i>].images[<j>]`).
type Picture<'a> = dyn FnMut(&Value, &str) -> Result<String, String> + 'a;

/// The pictures of message `index` as `image_url` parts; `picture` makes each one's URL.
fn pictures(
    entry: &Map<String, Value>,
    index: usize,
    policy: &RequestPolicy,
    picture: &mut Picture<'_>,
) -> Result<Vec<Value>, String> {
    match entry.get("images") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(images)) if images.iter().all(capabilities::is_file_value) => images
            .iter()
            .enumerate()
            .map(|(j, image)| {
                picture(image, &format!("messages[{index}].images[{j}]"))
                    .map(|url| policy.picture_part(url))
            })
            .collect(),
        Some(_) => Err("an agent message's images are a list of files".into()),
    }
}

/// An assistant message's calls as the wire takes them; `arguments` text passes byte for byte.
fn assistant_calls(value: Option<&Value>) -> Result<Vec<Value>, String> {
    let calls = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(calls)) => calls,
        Some(_) => return Err("an assistant message's tool_calls are a list".into()),
    };
    calls
        .iter()
        .map(|call| {
            let Some(call) = call.as_object() else {
                return Err("a tool call is an object with id, name and arguments".into());
            };
            let id = match call.get("id") {
                Some(Value::String(id)) if !id.is_empty() => id,
                _ => return Err("a tool call without an id cannot be sent".into()),
            };
            let Some(Value::String(name)) = call.get("name") else {
                return Err("a tool call's name is text".into());
            };
            let arguments = match call.get("arguments") {
                Some(Value::String(text)) => text.clone(),
                Some(object @ Value::Object(_)) => serde_json::to_string(object)
                    .map_err(|_| "a tool call's arguments are JSON text or an object")?,
                None | Some(Value::Null) => "{}".to_string(),
                Some(_) => return Err("a tool call's arguments are JSON text or an object".into()),
            };
            Ok(json!({"id": id, "type": "function",
                "function": {"name": name, "arguments": arguments}}))
        })
        .collect()
}

/// A role as a refusal names it.
fn role_text(role: Option<&Value>) -> String {
    match role {
        Some(Value::String(role)) => role.clone(),
        Some(other) => other.to_string(),
        None => "null".into(),
    }
}

/// The user message carrying held tool pictures.
fn held_pictures(held: &mut Vec<Value>) -> Value {
    let mut parts = vec![json!({"type": "text", "text": TOOL_PICTURES})];
    parts.append(held);
    json!({"role": "user", "content": parts})
}

/// The transcript as chat messages (module doc). `picture` turns a file value into its URL; the
/// first pass gives every picture an empty URL, so value refusals come before file refusals.
fn wire_messages(
    system: &str,
    messages: &[Value],
    policy: &RequestPolicy,
    picture: &mut Picture<'_>,
) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    if !system.is_empty() {
        out.push(json!({"role": "system", "content": system}));
    }
    let mut held: Vec<Value> = Vec::new();
    for (index, entry) in messages.iter().enumerate() {
        let Some(entry) = entry.as_object() else {
            return Err("an agent transcript's messages are objects".into());
        };
        let role = entry.get("role");
        let name = role.and_then(Value::as_str);
        if name != Some("tool") && !held.is_empty() {
            out.push(held_pictures(&mut held));
        }
        match name {
            Some("user") => {
                let text = content(entry, "user")?;
                let images = pictures(entry, index, policy, picture)?;
                if images.is_empty() {
                    out.push(json!({"role": "user", "content": text}));
                } else {
                    let mut parts = vec![json!({"type": "text", "text": text})];
                    parts.extend(images);
                    out.push(json!({"role": "user", "content": parts}));
                }
            }
            Some("assistant") => {
                let text = content(entry, "assistant")?;
                let mut message = Map::new();
                message.insert("role".into(), json!("assistant"));
                message.insert(
                    "content".into(),
                    if text.is_empty() {
                        Value::Null
                    } else {
                        Value::String(text.into())
                    },
                );
                let calls = assistant_calls(entry.get("tool_calls"))?;
                if !calls.is_empty() {
                    message.insert("tool_calls".into(), Value::Array(calls));
                }
                out.push(Value::Object(message));
            }
            Some("tool") => {
                let id = match entry.get("tool_call_id") {
                    Some(Value::String(id)) if !id.is_empty() => id,
                    _ => return Err("a tool message without a tool_call_id cannot be sent".into()),
                };
                let text = content(entry, "tool")?;
                out.push(json!({"role": "tool", "tool_call_id": id, "content": text}));
                held.extend(pictures(entry, index, policy, picture)?);
            }
            _ => {
                return Err(format!(
                    "an agent transcript has no '{}' messages",
                    role_text(role)
                ));
            }
        }
    }
    if !held.is_empty() {
        out.push(held_pictures(&mut held));
    }
    Ok(out)
}

/// The calls of a reply (spec/providers.md §9.2), or the structural failure's sentence.
fn reply_calls(value: Option<&Value>) -> Result<Vec<Value>, &'static str> {
    let entries = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(entries)) => entries,
        Some(_) => return Err("returned malformed tool calls"),
    };
    entries
        .iter()
        .map(|entry| {
            let function = entry.get("function").and_then(Value::as_object);
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty());
            let name = function
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty());
            let (Some(id), Some(name), Some(function)) = (id, name, function) else {
                return Err("returned a tool call without an id or name");
            };
            let arguments = match function.get("arguments") {
                Some(object @ Value::Object(_)) => object.clone(),
                Some(Value::String(text)) => {
                    let text = if text.is_empty() { "{}" } else { text.as_str() };
                    // serde_json refuses NaN and the infinities.
                    match serde_json::from_str::<Value>(text) {
                        Ok(object @ Value::Object(_)) => object,
                        Ok(_) => return Err("returned non-object tool arguments"),
                        Err(_) => return Err("returned invalid JSON tool arguments"),
                    }
                }
                _ => return Err("returned a tool call without arguments"),
            };
            Ok(json!({"id": id, "name": name, "arguments": arguments}))
        })
        .collect()
}

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
        // 4. Values.
        let request = &call.request;
        let list = |name: &str| -> &[Value] {
            request
                .get(name)
                .and_then(Value::as_array)
                .map_or(&[], Vec::as_slice)
        };
        let tools = wire_tools(list("tools"))?;
        let max_tokens = structured::max_tokens(request.get("max_tokens"))?;
        let tool_choice = if request.get("tool_choice").and_then(Value::as_str) == Some("auto") {
            "auto"
        } else {
            "required"
        };
        let system = request.get("system").and_then(Value::as_str).unwrap_or("");
        let policy = &contract.policy;
        wire_messages(system, list("messages"), policy, &mut |_, _| {
            Ok(String::new())
        })?;
        // 5. Files: every picture's kind before any file is read, then the bytes.
        wire_messages(system, list("messages"), policy, &mut |value, member| {
            let file = wire::request_file(call, value, PICTURE)?;
            if !file.kind.starts_with("image/") {
                return Err(format!("{member} is {}, not a picture", file.kind));
            }
            Ok(String::new())
        })?;
        let messages = wire_messages(system, list("messages"), policy, &mut |value, _| {
            let file = wire::request_file(call, value, PICTURE)?;
            let bytes = wire::read_file(file, PICTURE)?;
            Ok(wire::data_url(&file.kind, &bytes))
        })?;
        // The body (spec/providers.md §9.2).
        let mut body = Map::new();
        body.insert(
            "model".into(),
            Value::String(call.route.model.trim().into()),
        );
        body.insert("messages".into(), Value::Array(messages));
        body.insert("tools".into(), Value::Array(tools));
        body.insert("tool_choice".into(), json!(tool_choice));
        body.insert("provider".into(), policy.provider.clone());
        if let Some(reasoning) = policy.reasoning() {
            body.insert("reasoning".into(), reasoning);
        }
        if let Some(max_tokens) = max_tokens {
            body.insert("max_tokens".into(), json!(max_tokens));
        }
        let body = Value::Object(body);
        if super::body_exceeds(&body, super::MAX_REQUEST_BYTES) {
            return Err(super::BODY_TOO_LARGE.into());
        }
        Ok(Prepared { credential, body })
    }

    /// A 2xx's body as an answer (spec/providers.md §9.2).
    fn parse(&self, response: &HttpResponse) -> Sent {
        let body = match wire::json_object(&response.body, LABEL) {
            Ok(body) => body,
            Err(reason) => return structured::structural(&self.client, response, &reason, None),
        };
        let cost = structured::reported_cost(&body);
        let failed = |what: &str| {
            structured::structural(&self.client, response, &format!("{LABEL} {what}"), cost)
        };
        let Some(message) = structured::first_message(&body) else {
            return failed("returned no message");
        };
        let text = structured::message_text(message.get("content"));
        match reply_calls(message.get("tool_calls")) {
            Ok(calls) => Sent::Answered(Answer::new(
                json!({"text": text, "tool_calls": calls}),
                cost,
            )),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture_url(value: &Value, _member: &str) -> Result<String, String> {
        Ok(format!(
            "data:image/png;base64,{}",
            value["file"].as_str().unwrap()
        ))
    }

    #[test]
    fn tools_are_strict_functions() {
        let tools = wire_tools(&[json!({
            "name": "render",
            "description": "Render.",
            "parameters": {"type": "object", "properties": {"scale": {"type": "number", "minimum": 0}}}
        })])
        .unwrap();
        assert_eq!(
            serde_json::to_string(&tools[0]).unwrap(),
            r#"{"type":"function","function":{"name":"render","description":"Render.","parameters":{"type":"object","properties":{"scale":{"type":"number"}},"required":["scale"],"additionalProperties":false},"strict":true}}"#
        );
    }

    #[test]
    fn bad_tools_are_refused() {
        let refused = |tool: Value| wire_tools(&[tool]).unwrap_err();
        let tool = |name: &str| json!({"name": name, "description": "d", "parameters": {}});
        for bad in ["Render", "1render", "render-view", "", &"x".repeat(65)] {
            assert!(
                refused(tool(bad)).ends_with("must be lower_snake_case, at most 64 characters"),
                "{bad}"
            );
        }
        assert!(wire_tools(&[tool(&"x".repeat(64))]).is_ok());
        assert_eq!(
            refused(json!({"description": "d", "parameters": {}})),
            "tool 1 has no name"
        );
        assert_eq!(
            refused(json!("render")),
            "tool 1 is an object with name, description and parameters"
        );
        assert_eq!(
            refused(json!({"name": "render", "description": "  ", "parameters": {}})),
            "tool render must carry a description"
        );
        assert_eq!(
            refused(json!({"name": "render", "description": "d", "parameters": true})),
            "tool render parameters must be a JSON Schema object"
        );
    }

    #[test]
    fn tool_pictures_are_regrouped_before_the_next_message_and_at_the_end() {
        let file = |d: &str| json!({"file": d});
        let messages = [
            json!({"role": "user", "content": "go", "images": [file("a")]}),
            json!({"role": "assistant", "content": "", "tool_calls": [
                {"id": "c1", "name": "render", "arguments": "{\"view\":\"back\"}"},
                {"id": "c2", "name": "render", "arguments": {"view": "side", "n": 1}}
            ]}),
            json!({"role": "tool", "name": "render", "tool_call_id": "c1", "content": "back", "images": [file("b")]}),
            json!({"role": "tool", "name": "render", "tool_call_id": "c2", "content": "side", "images": [file("c")]}),
            json!({"role": "assistant", "content": "done"}),
            json!({"role": "tool", "tool_call_id": "c3", "content": "late", "images": [file("d")]}),
        ];
        let policy = RequestPolicy {
            image_detail: Some("low".into()),
            ..RequestPolicy::default()
        };
        let out = wire_messages("", &messages, &policy, &mut picture_url).unwrap();
        let part = |d: &str| json!({"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{d}"), "detail": "low"}});
        assert_eq!(
            out,
            vec![
                json!({"role": "user", "content": [{"type": "text", "text": "go"}, part("a")]}),
                json!({"role": "assistant", "content": null, "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "render", "arguments": "{\"view\":\"back\"}"}},
                    {"id": "c2", "type": "function", "function": {"name": "render", "arguments": "{\"view\":\"side\",\"n\":1}"}}
                ]}),
                json!({"role": "tool", "tool_call_id": "c1", "content": "back"}),
                json!({"role": "tool", "tool_call_id": "c2", "content": "side"}),
                json!({"role": "user", "content": [{"type": "text", "text": TOOL_PICTURES}, part("b"), part("c")]}),
                json!({"role": "assistant", "content": "done"}),
                json!({"role": "tool", "tool_call_id": "c3", "content": "late"}),
                json!({"role": "user", "content": [{"type": "text", "text": TOOL_PICTURES}, part("d")]}),
            ]
        );
    }

    #[test]
    fn transcripts_that_cannot_be_sent_are_refused() {
        let refused = |message: Value| {
            wire_messages("s", &[message], &RequestPolicy::default(), &mut picture_url).unwrap_err()
        };
        assert_eq!(
            refused(
                json!({"role": "assistant", "content": "", "tool_calls": [{"id": null, "name": "x", "arguments": "{}"}]})
            ),
            "a tool call without an id cannot be sent"
        );
        assert_eq!(
            refused(json!({"role": "assistant", "tool_calls": [{"name": "x"}]})),
            "a tool call without an id cannot be sent"
        );
        assert_eq!(
            refused(json!({"role": "tool", "name": "x", "content": "y"})),
            "a tool message without a tool_call_id cannot be sent"
        );
        assert_eq!(
            refused(json!({"role": "system", "content": "y"})),
            "an agent transcript has no 'system' messages"
        );
        assert_eq!(
            refused(json!({"content": "y"})),
            "an agent transcript has no 'null' messages"
        );
        assert_eq!(
            refused(json!("hi")),
            "an agent transcript's messages are objects"
        );
        assert_eq!(
            refused(json!({"role": "user", "content": ["x"]})),
            "an agent transcript's user message content is text"
        );
        assert_eq!(
            refused(
                json!({"role": "user", "content": "x", "images": ["data:image/png;base64,AAAA"]})
            ),
            "an agent message's images are a list of files"
        );
        assert_eq!(
            refused(
                json!({"role": "assistant", "tool_calls": [{"id": "c", "name": "x", "arguments": 3}]})
            ),
            "a tool call's arguments are JSON text or an object"
        );
    }

    #[test]
    fn an_assistant_without_calls_or_content_sends_null_content() {
        let out = wire_messages(
            "",
            &[
                json!({"role": "assistant", "content": "", "tool_calls": []}),
                json!({"role": "assistant"}),
            ],
            &RequestPolicy::default(),
            &mut picture_url,
        )
        .unwrap();
        assert_eq!(
            out,
            vec![
                json!({"role": "assistant", "content": null}),
                json!({"role": "assistant", "content": null})
            ]
        );
        assert_eq!(
            serde_json::to_string(&out[0]).unwrap(),
            r#"{"role":"assistant","content":null}"#
        );
    }

    #[test]
    fn reply_calls_parse_or_fail() {
        assert_eq!(reply_calls(None), Ok(vec![]));
        assert_eq!(reply_calls(Some(&Value::Null)), Ok(vec![]));
        assert_eq!(
            reply_calls(Some(&json!([
                {"id": "a", "function": {"name": "render", "arguments": "{\"scale\": 0.4}"}},
                {"id": "b", "function": {"name": "submit", "arguments": {"ok": true}}},
                {"id": "c", "function": {"name": "look", "arguments": ""}},
                {"id": "d", "function": {"name": "dup", "arguments": "{\"k\": 1, \"k\": 2}"}}
            ]))),
            Ok(vec![
                json!({"id": "a", "name": "render", "arguments": {"scale": 0.4}}),
                json!({"id": "b", "name": "submit", "arguments": {"ok": true}}),
                json!({"id": "c", "name": "look", "arguments": {}}),
                json!({"id": "d", "name": "dup", "arguments": {"k": 2}}),
            ])
        );
        let failed = |calls: Value| reply_calls(Some(&calls)).unwrap_err();
        assert_eq!(failed(json!("nope")), "returned malformed tool calls");
        assert_eq!(
            failed(json!([{"id": "", "function": {"name": "render"}}])),
            "returned a tool call without an id or name"
        );
        assert_eq!(
            failed(json!([{"id": "c", "function": {"name": ""}}])),
            "returned a tool call without an id or name"
        );
        assert_eq!(
            failed(json!(["c"])),
            "returned a tool call without an id or name"
        );
        assert_eq!(
            failed(json!([{"id": "c", "function": {"name": "render", "arguments": "{"}}])),
            "returned invalid JSON tool arguments"
        );
        assert_eq!(
            failed(json!([{"id": "c", "function": {"name": "render", "arguments": "[1]"}}])),
            "returned non-object tool arguments"
        );
        assert_eq!(
            failed(
                json!([{"id": "c", "function": {"name": "render", "arguments": "{\"scale\": NaN}"}}])
            ),
            "returned invalid JSON tool arguments"
        );
        assert_eq!(
            failed(json!([{"id": "c", "function": {"name": "render"}}])),
            "returned a tool call without arguments"
        );
    }
}
