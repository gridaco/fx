//! The agent loop (spec/protocol.md §6.2): the engine runs a body's whole tool-using model loop
//! when the host sends `agent.run`. Each turn is one `agent.turn` capability call through the
//! call path (cached, bounded, reserved, retried and settled like any other); the body's tools are
//! served by `tool.invoke` and its check by `agent.check`, sent back to the host.
//!
//! The loop, exactly as §6.2 fixes it (it fixes every turn's request and therefore its key):
//! 1. the transcript starts with `{"role": "user", "content": instructions}`, plus `"images"`;
//! 2. tools sent: the body's in order, then, with `submit`, `{"name": "submit", "description":
//!    "Finish: submit the answer this task asks for.", "parameters": submit}`; a body tool named
//!    `submit` is refused with `-32602`;
//! 3. each turn requests `{"system", "messages", "tools", "tool_choice"}` (+ `"max_tokens"`):
//!    `messages` windowed ([`window`]), `tool_choice` `"required"` with `submit`, else `"auto"`.
//!    The reply's data is `{"text", "tool_calls": [{"id", "name", "arguments"}]}`, appended as
//!    `{"role": "assistant", "content": text, "tool_calls": calls}` (a missing `text` is `""`;
//!    a reply that is not that shape fails the call with `call_failed`);
//! 4. no tool calls: ends with `{text}` without `submit`; with it, appends the user message
//!    `Finish by calling submit.` and goes on;
//! 5. calls in order, each answered by `{"role": "tool", "name", "tool_call_id"?, "content",
//!    "images"?}`: `submit` is validated against the schema ([`submit_refusal`]) then sent to
//!    `agent.check` when `check` is true; accepted ends the loop at once with `{submitted}` (later
//!    calls of the turn unanswered); refused is answered `refused: <why>` cut to 500 characters.
//!    An unknown name is `no tool named <name>`. Any other goes to `tool.invoke`: the content is
//!    `text(content)` (spec/identity.md §5), or the error text;
//! 6. `recent_images: n` keeps only the newest n pictures across each request's transcript; a
//!    message that lost pictures gets `\n[<k> older picture(s) not shown]` appended;
//! 7. after `max_steps` turns: `agent_unfinished`, `the agent did not finish within <n> turns`.
//!
//! A tool's or the check's `node_failure` (or a check's `node_error`) ends the loop with that same
//! error. The result is `{text}` or `{submitted}`, the whole unwindowed `transcript`, `turns`, and
//! `cost_usd`: what the uncached turns were charged.
//!
//! Before the first turn, `agent.run` is refused with `-32602` when `max_steps` is 0, a body tool
//! is named `submit`, a tool's `parameters` or the `submit` schema holds a reserved marker
//! (spec/protocol.md §3.2), or `submit` is not a JSON Schema (draft 2020-12). The reply shape is
//! read leniently where the predecessor was: a missing or `null` `text` is `""`, missing or
//! `null` `tool_calls` are none, and a call without `arguments` has `{}`; members other than
//! those named are not kept. Anything else (a reply that is not an object, a call without a
//! name, an `id` that is neither absent, `null` nor a string) fails the turn with `call_failed`.

use crate::calls::{CallAnswer, CallError};
use grida_fx_core::money::Usd;
use grida_fx_core::val::Val;
use grida_fx_core::value::check_markers;
use grida_fx_protocol::{
    AgentAnswer, AgentMessage, AgentRole, AgentRunParams, AgentRunResult, AgentToolCall, ErrorCode,
    FileValue, RpcError, ToolInvokeResult,
};
use grida_fx_providers::BoxFuture;
use indexmap::IndexMap;
use jsonschema::Validator;
use jsonschema::paths::LocationSegment;
use serde_json::{Map, Value};

/// The capability every turn calls.
pub const AGENT_TURN: &str = "agent.turn";

/// The name of the agent's own tool.
const SUBMIT: &str = "submit";

/// The submit tool's description.
const SUBMIT_DESCRIPTION: &str = "Finish: submit the answer this task asks for.";

/// The most characters of a refusal the model is told.
const REFUSAL_CHARS: usize = 500;

/// Makes one `agent.turn` call: the call path, bound to the run.
pub trait TurnCaller: Send + Sync {
    fn turn(&self, request: Value) -> BoxFuture<'_, Result<CallAnswer, CallError>>;
}

/// The body's side of the loop, served by the host.
pub trait AgentBody: Send + Sync {
    /// `tool.invoke` (spec/protocol.md §5.4).
    fn invoke<'a>(
        &'a self,
        agent_id: &'a str,
        call_id: Option<&'a str>,
        name: &'a str,
        arguments: &'a IndexMap<String, Value>,
    ) -> BoxFuture<'a, Result<ToolInvokeResult, RpcError>>;

    /// `agent.check` (spec/protocol.md §5.5): `Ok(None)` accepts, `Ok(Some(why))` refuses.
    fn check<'a>(
        &'a self,
        agent_id: &'a str,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<Option<String>, RpcError>>;
}

/// Why the loop ended without an answer.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentError {
    /// `agent_unfinished`, or a refused `agent.run` (`-32602`), as the error to answer with.
    Rpc(RpcError),
    /// A turn's call failed.
    Call(CallError),
}

impl AgentError {
    /// The error `agent.run` is answered with.
    pub fn to_rpc(&self) -> RpcError {
        match self {
            AgentError::Rpc(error) => error.clone(),
            AgentError::Call(error) => error.to_rpc(),
        }
    }
}

fn invalid(message: impl Into<String>) -> AgentError {
    AgentError::Rpc(RpcError::new(ErrorCode::InvalidParams, message))
}

/// A reply that is not `{"text", "tool_calls"}`: the turn's call failed.
fn bad_reply(answer: &CallAnswer, why: &str) -> AgentError {
    let mut data = Map::new();
    data.insert("capability".into(), Value::from(AGENT_TURN));
    data.insert("key".into(), Value::from(answer.key.as_str()));
    AgentError::Call(CallError::Rpc(
        RpcError::new(
            ErrorCode::CallFailed,
            format!(
                "{AGENT_TURN} answered something other than {{\"text\", \"tool_calls\"}}: {why}"
            ),
        )
        .with_data(Value::Object(data)),
    ))
}

/// The first `n` characters of a text.
fn cut(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// Serializes a protocol value; a failure is a fault in the engine.
fn json<T: serde::Serialize>(value: &T) -> Result<Value, AgentError> {
    serde_json::to_value(value).map_err(|e| {
        AgentError::Rpc(RpcError::new(
            ErrorCode::Internal,
            format!("the agent loop could not write its request: {e}"),
        ))
    })
}

/// One model reply, read (module doc).
struct Reply {
    text: String,
    calls: Vec<AgentToolCall>,
}

fn read_reply(answer: &CallAnswer) -> Result<Reply, AgentError> {
    let Value::Object(data) = &answer.data else {
        return Err(bad_reply(answer, "the reply is not an object"));
    };
    let text = match data.get("text") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(_) => return Err(bad_reply(answer, "text is not a string")),
    };
    let listed = match data.get("tool_calls") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(calls)) => calls.clone(),
        Some(_) => return Err(bad_reply(answer, "tool_calls is not a list")),
    };
    let mut calls = Vec::with_capacity(listed.len());
    for (i, call) in listed.iter().enumerate() {
        let Value::Object(call) = call else {
            return Err(bad_reply(
                answer,
                &format!("tool_calls[{i}] is not an object"),
            ));
        };
        let id = match call.get("id") {
            None => None,
            Some(id @ (Value::Null | Value::String(_))) => Some(id.clone()),
            Some(_) => {
                return Err(bad_reply(
                    answer,
                    &format!("tool_calls[{i}].id is not a string"),
                ));
            }
        };
        let Some(Value::String(name)) = call.get("name") else {
            return Err(bad_reply(answer, &format!("tool_calls[{i}] has no name")));
        };
        let arguments = match call.get("arguments") {
            None => IndexMap::new(),
            Some(Value::Object(arguments)) => arguments
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            Some(_) => {
                return Err(bad_reply(
                    answer,
                    &format!("tool_calls[{i}].arguments is not an object"),
                ));
            }
        };
        calls.push(AgentToolCall {
            id,
            name: name.clone(),
            arguments,
        });
    }
    Ok(Reply { text, calls })
}

/// The tool message answering `call`.
fn tool_message(call: &AgentToolCall, content: String, images: Vec<FileValue>) -> AgentMessage {
    AgentMessage {
        role: AgentRole::Tool,
        content,
        images: (!images.is_empty()).then_some(images),
        tool_calls: None,
        name: Some(call.name.clone()),
        tool_call_id: match &call.id {
            Some(Value::String(id)) => Some(id.clone()),
            _ => None,
        },
    }
}

/// Compiles a submit schema (draft 2020-12).
fn compile(schema: &Value) -> Result<Validator, String> {
    jsonschema::draft202012::new(schema)
        .map_err(|error| format!("the submit schema is not a JSON Schema: {error}"))
}

/// One segment of an instance location, ordered as the predecessor sorted them: indexes by
/// number, names by text, a shorter location first.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Segment {
    Index(usize),
    Name(String),
}

impl Segment {
    fn text(&self) -> String {
        match self {
            Segment::Index(i) => i.to_string(),
            Segment::Name(name) => name.clone(),
        }
    }
}

/// [`submit_refusal`] with a compiled schema.
fn refusal_by(validator: &Validator, value: &Value) -> Option<String> {
    let mut problems: Vec<(Vec<Segment>, String)> = validator
        .iter_errors(value)
        .map(|error| {
            let at = error
                .instance_path()
                .segments()
                .map(|segment| match segment {
                    LocationSegment::Index(i) => Segment::Index(i),
                    LocationSegment::Property(name) => Segment::Name(name.to_string()),
                })
                .collect();
            (at, error.to_string())
        })
        .collect();
    // Stable: errors at one location keep the validator's order.
    problems.sort_by(|a, b| a.0.cmp(&b.0));
    let (at, message) = problems.into_iter().next()?;
    let where_ = if at.is_empty() {
        "the answer".to_string()
    } else {
        at.iter().map(Segment::text).collect::<Vec<_>>().join("/")
    };
    Some(cut(&format!("{where_}: {message}"), REFUSAL_CHARS))
}

/// Runs the loop (module doc).
pub async fn run_agent(
    params: &AgentRunParams,
    turns: &dyn TurnCaller,
    body: &dyn AgentBody,
) -> Result<AgentRunResult, AgentError> {
    if params.max_steps == 0 {
        return Err(invalid("an agent takes at least 1 step (max_steps)"));
    }
    let mut tools = Vec::with_capacity(params.tools.len() + 1);
    for (i, tool) in params.tools.iter().enumerate() {
        if tool.name == SUBMIT {
            return Err(invalid(
                "submit is the agent's own tool; name yours otherwise",
            ));
        }
        let parameters = Value::Object(tool.parameters.clone().into_iter().collect());
        check_markers(&parameters, &format!("tools[{i}].parameters"))
            .map_err(|refused| invalid(refused.message))?;
        tools.push(json(tool)?);
    }
    let submit = match &params.submit {
        None => None,
        Some(schema) => {
            let schema = Value::Object(schema.clone().into_iter().collect());
            check_markers(&schema, SUBMIT).map_err(|refused| invalid(refused.message))?;
            let validator = compile(&schema).map_err(invalid)?;
            let mut tool = Map::new();
            tool.insert("name".into(), Value::from(SUBMIT));
            tool.insert("description".into(), Value::from(SUBMIT_DESCRIPTION));
            tool.insert("parameters".into(), schema);
            tools.push(Value::Object(tool));
            Some(validator)
        }
    };
    let tool_choice = if submit.is_some() { "required" } else { "auto" };

    let mut transcript = vec![AgentMessage {
        role: AgentRole::User,
        content: params.instructions.clone(),
        images: params.images.clone().filter(|images| !images.is_empty()),
        tool_calls: None,
        name: None,
        tool_call_id: None,
    }];
    let mut cost = Usd::ZERO;
    let mut made = 0u64;
    let finish =
        |answer: AgentAnswer, transcript: Vec<AgentMessage>, made: u64, cost: Usd| AgentRunResult {
            answer,
            transcript,
            turns: made,
            cost_usd: cost.to_value().as_f64().unwrap_or(0.0),
        };

    while made < params.max_steps {
        let mut request = Map::new();
        request.insert("system".into(), Value::from(params.system.as_str()));
        request.insert(
            "messages".into(),
            json(&window(&transcript, params.recent_images))?,
        );
        request.insert("tools".into(), Value::Array(tools.clone()));
        request.insert("tool_choice".into(), Value::from(tool_choice));
        if let Some(max_tokens) = params.max_tokens {
            request.insert("max_tokens".into(), Value::from(max_tokens));
        }
        let answer = turns
            .turn(Value::Object(request))
            .await
            .map_err(AgentError::Call)?;
        made += 1;
        if !answer.cached {
            cost = cost + answer.charged;
        }
        let reply = read_reply(&answer)?;
        transcript.push(AgentMessage {
            role: AgentRole::Assistant,
            content: reply.text.clone(),
            images: None,
            tool_calls: Some(reply.calls.clone()),
            name: None,
            tool_call_id: None,
        });
        if reply.calls.is_empty() {
            if submit.is_none() {
                let answer = AgentAnswer::Text { text: reply.text };
                return Ok(finish(answer, transcript, made, cost));
            }
            transcript.push(AgentMessage {
                role: AgentRole::User,
                content: format!("Finish by calling {SUBMIT}."),
                images: None,
                tool_calls: None,
                name: None,
                tool_call_id: None,
            });
            continue;
        }
        for call in &reply.calls {
            if let (true, Some(validator)) = (call.name == SUBMIT, &submit) {
                let value = Value::Object(call.arguments.clone().into_iter().collect());
                let refusal = match refusal_by(validator, &value) {
                    Some(refusal) => Some(refusal),
                    None if params.check => body
                        .check(&params.agent_id, &value)
                        .await
                        .map_err(AgentError::Rpc)?
                        .map(|why| cut(&why, REFUSAL_CHARS)),
                    None => None,
                };
                match refusal {
                    None => {
                        let answer = AgentAnswer::Submitted { submitted: value };
                        return Ok(finish(answer, transcript, made, cost));
                    }
                    Some(why) => {
                        transcript.push(tool_message(call, format!("refused: {why}"), Vec::new()));
                    }
                }
                continue;
            }
            if !params.tools.iter().any(|tool| tool.name == call.name) {
                let content = format!("no tool named {}", call.name);
                transcript.push(tool_message(call, content, Vec::new()));
                continue;
            }
            let call_id = match &call.id {
                Some(Value::String(id)) => Some(id.as_str()),
                _ => None,
            };
            let invoked = body
                .invoke(&params.agent_id, call_id, &call.name, &call.arguments)
                .await
                .map_err(AgentError::Rpc)?;
            let message = match invoked {
                ToolInvokeResult::Content { content, images } => {
                    // `text(v)` (spec/identity.md §5); a JSON value read as a runtime value holds
                    // no step view, so rendering it cannot fail.
                    let text = Val::from_json(&content).text().unwrap_or_default();
                    tool_message(call, text, images.unwrap_or_default())
                }
                ToolInvokeResult::Error { error } => tool_message(call, error, Vec::new()),
            };
            transcript.push(message);
        }
    }
    Err(AgentError::Rpc(RpcError::new(
        ErrorCode::AgentUnfinished,
        format!("the agent did not finish within {} turns", params.max_steps),
    )))
}

/// The transcript as one request sends it (step 6 of the module doc).
pub fn window(transcript: &[AgentMessage], recent_images: Option<u64>) -> Vec<AgentMessage> {
    let Some(recent) = recent_images else {
        return transcript.to_vec();
    };
    let mut keep = usize::try_from(recent).unwrap_or(usize::MAX);
    let mut sent = Vec::with_capacity(transcript.len());
    for message in transcript.iter().rev() {
        let images = match &message.images {
            Some(images) if !images.is_empty() => images,
            _ => {
                sent.push(message.clone());
                continue;
            }
        };
        let shown = &images[images.len().saturating_sub(keep)..];
        keep -= shown.len();
        let dropped = images.len() - shown.len();
        let mut copy = message.clone();
        copy.images = (!shown.is_empty()).then(|| shown.to_vec());
        if dropped > 0 {
            copy.content = format!("{}\n[{dropped} older picture(s) not shown]", copy.content);
        }
        sent.push(copy);
    }
    sent.reverse();
    sent
}

/// Why a submitted value does not meet the schema: `<where>: <message>` cut to 500 characters,
/// `<where>` the first error's location (by location order) joined by `/`, or `the answer`; `None`
/// when it meets it.
pub fn submit_refusal(schema: &Value, value: &Value) -> Option<String> {
    match compile(schema) {
        Ok(validator) => refusal_by(&validator, value),
        Err(why) => Some(cut(&format!("the answer: {why}"), REFUSAL_CHARS)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn picture(n: u8) -> FileValue {
        FileValue {
            file: format!("{n:064x}"),
        }
    }

    fn message(content: &str, images: Vec<FileValue>) -> AgentMessage {
        AgentMessage {
            role: AgentRole::User,
            content: content.into(),
            images: (!images.is_empty()).then_some(images),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn the_window_keeps_the_newest_pictures() {
        let transcript = vec![
            message("a", vec![picture(1), picture(2)]),
            message("b", vec![]),
            message("c", vec![picture(3), picture(4), picture(5)]),
        ];
        assert_eq!(window(&transcript, None), transcript);
        let sent = window(&transcript, Some(2));
        assert_eq!(sent[2].images, Some(vec![picture(4), picture(5)]));
        assert_eq!(sent[2].content, "c\n[1 older picture(s) not shown]");
        assert_eq!(sent[1], transcript[1]);
        assert_eq!(sent[0].images, None);
        assert_eq!(sent[0].content, "a\n[2 older picture(s) not shown]");
        let sent = window(&transcript, Some(4));
        assert_eq!(sent[2], transcript[2]);
        assert_eq!(sent[0].images, Some(vec![picture(2)]));
        assert_eq!(sent[0].content, "a\n[1 older picture(s) not shown]");
        let sent = window(&transcript, Some(0));
        assert!(sent.iter().all(|m| m.images.is_none()));
        assert_eq!(sent[2].content, "c\n[3 older picture(s) not shown]");
        // The transcript itself is never windowed.
        assert_eq!(transcript[0].images.as_ref().map(Vec::len), Some(2));
    }

    #[test]
    fn refusals_name_the_first_place_by_location() {
        let schema = json!({
            "type": "object",
            "properties": {
                "answer": {"type": "string"},
                "items": {"type": "array", "items": {"type": "integer"}}
            },
            "required": ["answer"]
        });
        assert_eq!(submit_refusal(&schema, &json!({"answer": "x"})), None);
        let refusal = submit_refusal(&schema, &json!({"answer": 3})).unwrap();
        assert!(refusal.starts_with("answer: "), "{refusal}");
        assert!(refusal.contains("string"), "{refusal}");
        let refusal = submit_refusal(&schema, &json!({"answer": 3, "items": [1, "b"]})).unwrap();
        assert!(refusal.starts_with("answer: "), "{refusal}");
        let refusal = submit_refusal(&schema, &json!({"answer": "x", "items": [1, "b"]})).unwrap();
        assert!(refusal.starts_with("items/1: "), "{refusal}");
        let refusal = submit_refusal(&schema, &json!({})).unwrap();
        assert!(refusal.starts_with("the answer: "), "{refusal}");
        assert!(refusal.contains("answer"), "{refusal}");
    }

    #[test]
    fn refusals_are_cut_to_500_characters() {
        let long = "é".repeat(600);
        let schema = json!({"const": long});
        let refusal = submit_refusal(&schema, &json!("other")).unwrap();
        assert_eq!(refusal.chars().count(), 500);
        assert!(refusal.starts_with("the answer: "));
    }

    #[test]
    fn segments_sort_indexes_before_names_and_prefixes_first() {
        let mut paths = vec![
            vec![Segment::Name("b".into())],
            vec![Segment::Name("a".into()), Segment::Index(10)],
            vec![Segment::Name("a".into()), Segment::Index(2)],
            vec![Segment::Name("a".into())],
            vec![],
            vec![Segment::Index(0)],
        ];
        paths.sort();
        assert_eq!(
            paths,
            vec![
                vec![],
                vec![Segment::Index(0)],
                vec![Segment::Name("a".into())],
                vec![Segment::Name("a".into()), Segment::Index(2)],
                vec![Segment::Name("a".into()), Segment::Index(10)],
                vec![Segment::Name("b".into())],
            ]
        );
    }
}
