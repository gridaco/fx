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
//!    `{"role": "assistant", "content": text, "tool_calls": calls}` (a missing `text` is `""`).
//!    In the transcript each call's `arguments` is a string: the canonical JSON text
//!    (spec/identity.md §2) of the object the model gave, as provider APIs carry it. What the
//!    model wrote therefore never reads as a file value or a reserved marker in the next turn's
//!    request, which the call path checks; the loop dispatches the object itself. A reply that is
//!    not that shape is refused by the call path's own check ([`check_reply`]) inside the retry
//!    owner, so it fails its attempt as billed and is never recorded; the loop reads the reply
//!    with the same rules and fails with `call_failed` should one reach it;
//! 4. no tool calls: ends with `{text}` without `submit`; with it, appends the user message
//!    `Finish by calling submit.` and goes on;
//! 5. calls in order, each answered by `{"role": "tool", "name", "tool_call_id"?, "content",
//!    "images"?}`: `submit` is validated against the schema ([`submit_refusal`], FX's own text for
//!    the first failed keyword) then sent to `agent.check` when `check` is true; accepted ends the
//!    loop at once with `{submitted}` (later calls of the turn unanswered); refused is answered
//!    `refused: <why>` cut to 500 characters.
//!    An unknown name is `no tool named <name>`. Any other goes to `tool.invoke`: the content is
//!    `text(content)` (spec/identity.md §5), or the error text;
//! 6. `recent_images: n` keeps only the newest n pictures across each request's transcript; a
//!    message that lost pictures gets `\n[<k> older picture(s) not shown]` appended;
//! 7. after `max_steps` turns: `agent_unfinished`, `the agent did not finish within <n> turns`.
//!
//! A tool's or the check's `node_failure` (or a check's `node_error`) ends the loop with that same
//! error. The result is `{text}` or `{submitted}`, the whole unwindowed `transcript`, `turns`, and
//! `cost_usd`: what the uncached turns were charged. [`run_agent_into`] builds the transcript in
//! the caller's vector, so an error that ends the loop after it started still has it: the host
//! is answered with it in the error's `data.transcript`.
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
use grida_fx_core::value::{canon, check_markers};
use grida_fx_protocol::{
    AgentAnswer, AgentMessage, AgentRole, AgentRunParams, AgentRunResult, AgentToolCall, ErrorCode,
    FileValue, RpcError, ToolInvokeResult,
};
use grida_fx_providers::BoxFuture;
use indexmap::IndexMap;
use jsonschema::error::{TypeKind, ValidationErrorKind};
use jsonschema::paths::{Location, LocationSegment};
use jsonschema::{ValidationError, Validator};
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

/// A reply that is not `{"text", "tool_calls"}` reached the loop: the turn's call failed.
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

/// Serializes a value the loop sends; a failure is a fault in the engine.
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
    calls: Vec<Call>,
}

/// One tool call of a reply, its arguments as the object the model gave.
struct Call {
    id: Option<Value>,
    name: String,
    arguments: IndexMap<String, Value>,
}

impl Call {
    /// The call as the transcript keeps it (module doc, step 3).
    fn recorded(&self) -> AgentToolCall {
        let arguments = Value::Object(self.arguments.clone().into_iter().collect());
        AgentToolCall {
            id: self.id.clone(),
            name: self.name.clone(),
            arguments: canon(&arguments),
        }
    }
}

/// Whether an `agent.turn` reply's data is `{"text", "tool_calls"}` as the loop reads it (module
/// doc): the call path's own check of every `agent.turn` answer. The refusal names what is wrong.
pub fn check_reply(data: &Value) -> Result<(), String> {
    read_reply(data)
        .map(|_| ())
        .map_err(|why| format!("the reply is not {{\"text\", \"tool_calls\"}}: {why}"))
}

fn read_reply(data: &Value) -> Result<Reply, String> {
    let Value::Object(data) = data else {
        return Err("the reply is not an object".into());
    };
    let text = match data.get("text") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(_) => return Err("text is not a string".into()),
    };
    let listed = match data.get("tool_calls") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(calls)) => calls.clone(),
        Some(_) => return Err("tool_calls is not a list".into()),
    };
    let mut calls = Vec::with_capacity(listed.len());
    for (i, call) in listed.iter().enumerate() {
        let Value::Object(call) = call else {
            return Err(format!("tool_calls[{i}] is not an object"));
        };
        let id = match call.get("id") {
            None => None,
            Some(id @ (Value::Null | Value::String(_))) => Some(id.clone()),
            Some(_) => return Err(format!("tool_calls[{i}].id is not a string")),
        };
        let Some(Value::String(name)) = call.get("name") else {
            return Err(format!("tool_calls[{i}] has no name"));
        };
        let arguments = match call.get("arguments") {
            None => IndexMap::new(),
            Some(Value::Object(arguments)) => arguments
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            Some(_) => return Err(format!("tool_calls[{i}].arguments is not an object")),
        };
        calls.push(Call {
            id,
            name: name.clone(),
            arguments,
        });
    }
    Ok(Reply { text, calls })
}

/// The tool message answering `call`.
fn tool_message(call: &Call, content: String, images: Vec<FileValue>) -> AgentMessage {
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

/// The segments of a location, for ordering.
fn segments(location: &Location) -> Vec<Segment> {
    location
        .segments()
        .map(|segment| match segment {
            LocationSegment::Index(i) => Segment::Index(i),
            LocationSegment::Property(name) => Segment::Name(name.to_string()),
        })
        .collect()
}

/// [`submit_refusal`] with the schema compiled.
fn refusal_by(validator: &Validator, schema: &Value, value: &Value) -> Option<String> {
    let mut problems: Vec<(Vec<Segment>, Vec<Segment>, String)> = validator
        .iter_errors(value)
        .map(|error| {
            (
                segments(error.instance_path()),
                segments(error.evaluation_path()),
                keyword_text(&error, schema, value),
            )
        })
        .collect();
    // By instance location, then by keyword location; stable, so the errors one keyword reports
    // at one place keep the schema's order (`required`).
    problems.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let (at, _, message) = problems.into_iter().next()?;
    let where_ = if at.is_empty() {
        "the answer".to_string()
    } else {
        at.iter().map(Segment::text).collect::<Vec<_>>().join("/")
    };
    Some(cut(&format!("{where_}: {message}"), REFUSAL_CHARS))
}

/// `n` and a noun, singular for 1.
fn count(n: impl std::fmt::Display + PartialEq<u64>, one: &str, many: &str) -> String {
    let noun = if n == 1 { one } else { many };
    format!("{n} {noun}")
}

/// Names as canonical JSON strings, in canonical order (UTF-16 code units), joined by `, `.
fn names(names: &[String]) -> String {
    let mut sorted: Vec<&String> = names.iter().collect();
    sorted.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    sorted
        .iter()
        .map(|name| canon(&Value::from(name.as_str())))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The members of `object` that the schema object holding an `additionalProperties` keyword at
/// `keyword` (a location in `schema`) does not cover by `properties` or `patternProperties`;
/// `None` when that schema object cannot be found.
fn not_covered(schema: &Value, keyword: &Location, object: &Value) -> Option<Vec<String>> {
    let Value::Object(members) = object else {
        return None;
    };
    let pointer = keyword.as_str();
    let parent = pointer.strip_suffix("/additionalProperties")?;
    let Value::Object(holder) = schema.pointer(parent)? else {
        return None;
    };
    let properties = holder.get("properties").and_then(Value::as_object);
    let patterns: Vec<Validator> = holder
        .get("patternProperties")
        .and_then(Value::as_object)
        .map(|patterns| {
            patterns
                .keys()
                .filter_map(|pattern| {
                    jsonschema::draft202012::new(&serde_json::json!({"pattern": pattern})).ok()
                })
                .collect()
        })
        .unwrap_or_default();
    Some(
        members
            .keys()
            .filter(|name| !properties.is_some_and(|properties| properties.contains_key(*name)))
            .filter(|name| {
                let name = Value::from(name.as_str());
                !patterns.iter().any(|pattern| pattern.is_valid(&name))
            })
            .cloned()
            .collect(),
    )
}

/// `the property "a" is not allowed`, or `the properties "a", "b" are not allowed`.
fn not_allowed(unexpected: &[String]) -> String {
    if unexpected.len() == 1 {
        format!("the property {} is not allowed", names(unexpected))
    } else {
        format!("the properties {} are not allowed", names(unexpected))
    }
}

/// FX's own text for one failed keyword (spec/protocol.md §6.2): it depends only on the keyword,
/// the schema's value for it and the value found at the error's place in `value` (`v`, as
/// canonical JSON), never on the validator's wording.
fn keyword_text(error: &ValidationError<'_>, schema: &Value, value: &Value) -> String {
    let found = value
        .pointer(error.instance_path().as_str())
        .unwrap_or_else(|| error.instance());
    let v = canon(found);
    // `additionalProperties`, however the validator reports it (one error for the object, or a
    // `false` schema met by its first member): the members it does not allow.
    let keyword = error.schema_path().as_str();
    if keyword == "/additionalProperties" || keyword.ends_with("/additionalProperties") {
        let unexpected =
            not_covered(schema, error.schema_path(), found).or_else(|| match error.kind() {
                ValidationErrorKind::AdditionalProperties { unexpected } => {
                    Some(unexpected.clone())
                }
                _ => None,
            });
        if let Some(unexpected) = unexpected.filter(|names| !names.is_empty()) {
            return not_allowed(&unexpected);
        }
    }
    match error.kind() {
        ValidationErrorKind::Type { kind } => {
            let mut types: Vec<String> = match kind {
                TypeKind::Single(one) => vec![one.to_string()],
                TypeKind::Multiple(set) => set.iter().map(|t| t.to_string()).collect(),
            };
            types.sort();
            let types: Vec<String> = types
                .iter()
                .map(|t| canon(&Value::from(t.as_str())))
                .collect();
            format!("{v} is not of type {}", types.join(" or "))
        }
        ValidationErrorKind::Enum { options } => format!("{v} is not one of {}", canon(options)),
        ValidationErrorKind::Constant { expected_value } => {
            format!("{v} is not {}", canon(expected_value))
        }
        ValidationErrorKind::Required { property } => {
            format!("{} is a required property", canon(property))
        }
        ValidationErrorKind::AdditionalProperties { unexpected }
        | ValidationErrorKind::UnevaluatedProperties { unexpected } => not_allowed(unexpected),
        ValidationErrorKind::MinLength { limit } => format!(
            "{v} is shorter than {}",
            count(*limit, "character", "characters")
        ),
        ValidationErrorKind::MaxLength { limit } => format!(
            "{v} is longer than {}",
            count(*limit, "character", "characters")
        ),
        ValidationErrorKind::Minimum { limit } => {
            format!("{v} is less than the minimum {}", canon(limit))
        }
        ValidationErrorKind::Maximum { limit } => {
            format!("{v} is greater than the maximum {}", canon(limit))
        }
        ValidationErrorKind::ExclusiveMinimum { limit } => {
            format!("{v} is not greater than {}", canon(limit))
        }
        ValidationErrorKind::ExclusiveMaximum { limit } => {
            format!("{v} is not less than {}", canon(limit))
        }
        ValidationErrorKind::MultipleOf { multiple_of } => {
            let n = serde_json::Number::from_f64(*multiple_of)
                .map_or_else(|| multiple_of.to_string(), |n| canon(&Value::Number(n)));
            format!("{v} is not a multiple of {n}")
        }
        ValidationErrorKind::MinItems { limit } => {
            format!("{v} has fewer than {}", count(*limit, "item", "items"))
        }
        ValidationErrorKind::MaxItems { limit } => {
            format!("{v} has more than {}", count(*limit, "item", "items"))
        }
        ValidationErrorKind::AdditionalItems { limit } => {
            format!(
                "{v} has more than {}",
                count(*limit as u64, "item", "items")
            )
        }
        ValidationErrorKind::UnevaluatedItems { .. } => {
            format!("{v} has items that no schema allows")
        }
        ValidationErrorKind::UniqueItems => format!("{v} has repeated items"),
        ValidationErrorKind::MinProperties { limit } => format!(
            "{v} has fewer than {}",
            count(*limit, "property", "properties")
        ),
        ValidationErrorKind::MaxProperties { limit } => format!(
            "{v} has more than {}",
            count(*limit, "property", "properties")
        ),
        ValidationErrorKind::Contains => format!("{v} does not hold the items contains asks for"),
        ValidationErrorKind::Pattern { pattern } => format!(
            "{v} does not match the pattern {}",
            canon(&Value::from(pattern.as_str()))
        ),
        ValidationErrorKind::Format { format } => format!(
            "{v} is not a valid {}",
            canon(&Value::from(format.as_str()))
        ),
        ValidationErrorKind::Not { .. } => format!("{v} matches the schema under not"),
        ValidationErrorKind::AnyOf { .. } => {
            format!("{v} matches none of the schemas under anyOf")
        }
        ValidationErrorKind::OneOfNotValid { .. } => {
            format!("{v} matches none of the schemas under oneOf")
        }
        ValidationErrorKind::OneOfMultipleValid { .. } => {
            format!("{v} matches more than one of the schemas under oneOf")
        }
        ValidationErrorKind::FalseSchema => format!("{v} is not allowed"),
        ValidationErrorKind::PropertyNames { error } => {
            let name = error.instance();
            format!("the property name {}", keyword_text(error, schema, name))
        }
        other => format!("{v} does not meet {}", canon(&Value::from(other.keyword()))),
    }
}

/// Runs the loop (module doc).
pub async fn run_agent(
    params: &AgentRunParams,
    turns: &dyn TurnCaller,
    body: &dyn AgentBody,
) -> Result<AgentRunResult, AgentError> {
    let mut transcript = Vec::new();
    run_agent_into(params, turns, body, &mut transcript).await
}

/// [`run_agent`], building the transcript in `transcript` (cleared first), so the caller still
/// has it when the loop ends with an error. It stays empty when `agent.run` is refused before the
/// first turn.
pub async fn run_agent_into(
    params: &AgentRunParams,
    turns: &dyn TurnCaller,
    body: &dyn AgentBody,
    transcript: &mut Vec<AgentMessage>,
) -> Result<AgentRunResult, AgentError> {
    transcript.clear();
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
            tool.insert("parameters".into(), schema.clone());
            tools.push(Value::Object(tool));
            Some((validator, schema))
        }
    };
    let tool_choice = if submit.is_some() { "required" } else { "auto" };

    transcript.push(AgentMessage {
        role: AgentRole::User,
        content: params.instructions.clone(),
        images: params.images.clone().filter(|images| !images.is_empty()),
        tool_calls: None,
        name: None,
        tool_call_id: None,
    });
    let mut cost = Usd::ZERO;
    let mut made = 0u64;
    let finish =
        |answer: AgentAnswer, transcript: &[AgentMessage], made: u64, cost: Usd| AgentRunResult {
            answer,
            transcript: transcript.to_vec(),
            turns: made,
            cost_usd: cost.to_value().as_f64().unwrap_or(0.0),
        };

    while made < params.max_steps {
        let mut request = Map::new();
        request.insert("system".into(), Value::from(params.system.as_str()));
        request.insert(
            "messages".into(),
            json(&window(transcript, params.recent_images))?,
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
        let reply = read_reply(&answer.data).map_err(|why| bad_reply(&answer, &why))?;
        transcript.push(AgentMessage {
            role: AgentRole::Assistant,
            content: reply.text.clone(),
            images: None,
            tool_calls: Some(reply.calls.iter().map(Call::recorded).collect()),
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
            if let (true, Some((validator, schema))) = (call.name == SUBMIT, &submit) {
                let value = Value::Object(call.arguments.clone().into_iter().collect());
                let refusal = match refusal_by(validator, schema, &value) {
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
        Ok(validator) => refusal_by(&validator, schema, value),
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
    fn refusals_are_fx_s_own_text_per_keyword() {
        let refused = |schema: Value, value: Value| submit_refusal(&schema, &value).unwrap();
        let cases = [
            (
                json!({"type": "string"}),
                json!(3),
                r#"3 is not of type "string""#,
            ),
            (
                json!({"type": ["string", "null", "integer"]}),
                json!(1.5),
                r#"1.5 is not of type "integer" or "null" or "string""#,
            ),
            (
                json!({"enum": ["accept", "reject"]}),
                json!("maybe"),
                r#""maybe" is not one of ["accept","reject"]"#,
            ),
            (
                json!({"const": {"b": 1, "a": 2}}),
                json!(1),
                r#"1 is not {"a":2,"b":1}"#,
            ),
            (
                json!({"required": ["b", "a"]}),
                json!({}),
                r#""b" is a required property"#,
            ),
            (
                json!({"properties": {"a": {}}, "additionalProperties": false}),
                json!({"z": 1, "y": 2, "a": 3}),
                r#"the properties "y", "z" are not allowed"#,
            ),
            (
                json!({"additionalProperties": false}),
                json!({"z": 1}),
                r#"the property "z" is not allowed"#,
            ),
            (
                json!({"additionalProperties": false}),
                json!({"z": 1, "y": 2}),
                r#"the properties "y", "z" are not allowed"#,
            ),
            (
                json!({"patternProperties": {"^x": {}}, "additionalProperties": false}),
                json!({"xa": 1, "b": 2}),
                r#"the property "b" is not allowed"#,
            ),
            (
                json!({"minLength": 3}),
                json!("é"),
                r#""é" is shorter than 3 characters"#,
            ),
            (
                json!({"maxLength": 1}),
                json!("ab"),
                r#""ab" is longer than 1 character"#,
            ),
            (
                json!({"minimum": 2}),
                json!(1),
                "1 is less than the minimum 2",
            ),
            (
                json!({"maximum": 2}),
                json!(3),
                "3 is greater than the maximum 2",
            ),
            (
                json!({"exclusiveMinimum": 2}),
                json!(2),
                "2 is not greater than 2",
            ),
            (
                json!({"exclusiveMaximum": 2}),
                json!(2),
                "2 is not less than 2",
            ),
            (
                json!({"multipleOf": 2}),
                json!(3),
                "3 is not a multiple of 2",
            ),
            (
                json!({"minItems": 2}),
                json!([1]),
                "[1] has fewer than 2 items",
            ),
            (
                json!({"maxItems": 1}),
                json!([1, 2]),
                "[1,2] has more than 1 item",
            ),
            (
                json!({"uniqueItems": true}),
                json!([1, 1]),
                "[1,1] has repeated items",
            ),
            (
                json!({"minProperties": 2}),
                json!({"a": 1}),
                r#"{"a":1} has fewer than 2 properties"#,
            ),
            (
                json!({"maxProperties": 1}),
                json!({"a": 1, "b": 2}),
                r#"{"a":1,"b":2} has more than 1 property"#,
            ),
            (
                json!({"pattern": "^a"}),
                json!("b"),
                r#""b" does not match the pattern "^a""#,
            ),
            (
                json!({"not": {"type": "string"}}),
                json!("s"),
                r#""s" matches the schema under not"#,
            ),
            (
                json!({"anyOf": [{"type": "string"}, {"type": "null"}]}),
                json!(1),
                "1 matches none of the schemas under anyOf",
            ),
            (
                json!({"oneOf": [{"type": "string"}, {"type": "null"}]}),
                json!(1),
                "1 matches none of the schemas under oneOf",
            ),
            (
                json!({"oneOf": [{"type": "number"}, {"type": "integer"}]}),
                json!(1),
                "1 matches more than one of the schemas under oneOf",
            ),
            (json!(false), json!(1), "1 is not allowed"),
            (
                json!({"propertyNames": {"pattern": "^[a-z]+$"}}),
                json!({"Bad": 1}),
                r#"the property name "Bad" does not match the pattern "^[a-z]+$""#,
            ),
        ];
        for (schema, value, message) in cases {
            assert_eq!(
                refused(schema.clone(), value.clone()),
                format!("the answer: {message}"),
                "{schema} {value}"
            );
        }
    }

    #[test]
    fn of_several_errors_the_first_by_place_then_by_keyword_is_told() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": {"type": "string", "minLength": 5, "pattern": "^x"},
                "b": {"type": "integer"}
            },
            "required": ["a", "b", "c"]
        });
        // The top first (required, in the schema's order), then by place.
        assert_eq!(
            submit_refusal(&schema, &json!({"a": "yy", "b": 1})).unwrap(),
            r#"the answer: "c" is a required property"#
        );
        // At one place, keywords by name: minLength before pattern.
        assert_eq!(
            submit_refusal(&schema, &json!({"a": "yy", "b": 1, "c": 0})).unwrap(),
            r#"a: "yy" is shorter than 5 characters"#
        );
        assert_eq!(
            submit_refusal(&schema, &json!({"a": "xxxxx", "b": "1", "c": 0})).unwrap(),
            r#"b: "1" is not of type "integer""#
        );
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
