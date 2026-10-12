//! Chat Completions as FX speaks it, whichever provider serves the route: the structured-output
//! prompt and answer (spec/capabilities.md §4) and the agent turn's transcript, tools and reply
//! (§5), with the pictures they carry (spec/providers.md §9.2; Grida Gateway's adapters, §9.7,
//! send the same messages).
//!
//! What a provider adds is its own module's: the endpoint and credential, provider members of the
//! body (OpenRouter's routing and reasoning), how a status classifies, and what a call costs.
//!
//! **Structured output** ([`structured_prompt`], [`structured_value`]). The schema is a `json`
//! file or an inline object, sent after `$ref` inlining and strict canonicalization ([`schema`]),
//! named from its `title` (else `answer`). Text context files are appended to the prompt as
//! `--- context <i> ---\n<text>`; pictures are reduced to a [`PICTURE_EDGE`] px long edge,
//! flattened on `matte`, unless the route keeps them unchanged. The answer's value is
//! `message.parsed`, else the JSON text of `message.content`, unwrapped from `completionState`.
//!
//! **Agent turns** ([`agent_prompt`], [`agent_reply`]). The engine's transcript maps to the wire: a
//! system message when `system` is non-empty; user messages with pictures as content parts;
//! assistant messages with `content: null` when empty and `tool_calls` whose `arguments` string is
//! passed through byte for byte; tool messages as `{"role": "tool", "tool_call_id", "content"}`
//! (no `name`), their pictures regrouped into a following user message [`TOOL_PICTURES`]. Tools are
//! `{"type": "function", "function": {"name", "description", "parameters": <strict schema>,
//! "strict": true}}`. `tool_choice` is `auto` only when the request says `auto`, else `required`.
//! The reply is `{"text", "tool_calls": [{"id", "name", "arguments": <object>}]}`.

pub mod schema;

use crate::adapter::CallRequest;
use crate::capabilities;
use crate::checks;
use crate::wire;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::sync::LazyLock;

/// The default `max_tokens` of a structured call.
pub const DEFAULT_MAX_TOKENS: u64 = 16_000;

/// The long edge a reduced picture fits in.
pub const PICTURE_EDGE: u32 = 1600;

/// The colour reduced pictures are flattened on when the request gives none.
pub const DEFAULT_MATTE: &str = "#ffffff";

/// The user message that carries the pictures tools returned.
pub const TOOL_PICTURES: &str = "Pictures the tools returned.";

/// What a picture of an agent turn is called in a refusal.
const PICTURE: &str = "an agent picture";

static TOOL_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_]{0,63}$").expect("a valid pattern"));

/// The `image_url` part of a picture, with `detail` when the route gives one.
pub fn picture_part(url: String, detail: Option<&str>) -> Value {
    let mut image_url = Map::new();
    image_url.insert("url".into(), Value::String(url));
    if let Some(detail) = detail {
        image_url.insert("detail".into(), Value::String(detail.to_string()));
    }
    json!({"type": "image_url", "image_url": image_url})
}

/// `choices[0].message`, when an object.
pub fn first_message(body: &Map<String, Value>) -> Option<&Map<String, Value>> {
    body.get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(Value::as_object)
}

/// A message's text: a string `content`, or the `text` of every `{"type": "text", "text": <s>}`
/// part of a list, joined; anything else is `""`.
pub fn message_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect(),
        _ => String::new(),
    }
}

/// `max_tokens`: absent or `null` is `None`; otherwise a whole number of at least 1 (the type is
/// [`capabilities::check_request`]'s).
pub fn max_tokens(value: Option<&Value>) -> Result<Option<u64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let n = value.as_f64().unwrap_or(f64::NAN);
            if n.is_nan() || n < 1.0 {
                Err("max_tokens must be at least 1".into())
            } else if n > 9_007_199_254_740_991.0 || n.fract() != 0.0 {
                Err("max_tokens must be a whole number no larger than 9007199254740991".into())
            } else {
                Ok(Some(n as u64))
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Structured output

/// A structured call's messages, response format and `max_tokens`, ready for a provider's body.
#[derive(Debug, Clone, PartialEq)]
pub struct StructuredPrompt {
    pub messages: Vec<Value>,
    /// `{"type": "json_schema", "json_schema": {"name", "strict": true, "schema",
    /// "description"?}}`.
    pub response_format: Value,
    pub max_tokens: u64,
}

/// Steps 4 and 5 of a structured call (spec/providers.md §5, §9.2), for a request that passed its
/// capability's check: the values (a non-blank `prompt`; `matte` a colour; `max_tokens` at least
/// 1; an inline schema whose references inline and which compiles as a draft 2020-12 JSON
/// Schema), then the files (the schema file, then each context file, a picture or UTF-8 text).
/// Pictures go as they are when `pictures_unchanged`, else reduced; each part carries `detail`.
pub fn structured_prompt(
    call: &CallRequest,
    pictures_unchanged: bool,
    detail: Option<&str>,
) -> Result<StructuredPrompt, String> {
    // 4. Values.
    let request = &call.request;
    let prompt = match request.get("prompt") {
        Some(Value::String(prompt)) if !prompt.trim().is_empty() => prompt.clone(),
        _ => return Err("a structured call needs its prompt as text".into()),
    };
    let system = match request.get("system") {
        Some(Value::String(system)) if !system.trim().is_empty() => Some(system.clone()),
        _ => None,
    };
    let matte = match request.get("matte") {
        Some(Value::String(matte)) => {
            parse_matte(matte).ok_or_else(|| format!("matte {matte} is not a colour"))?
        }
        _ => parse_matte(DEFAULT_MATTE).unwrap_or([255; 4]),
    };
    let max_tokens = max_tokens(request.get("max_tokens"))?.unwrap_or(DEFAULT_MAX_TOKENS);
    let schema_value = request.get("schema").unwrap_or(&Value::Null);
    let inline = !capabilities::is_file_value(schema_value);
    let mut prepared_schema = None;
    if inline {
        prepared_schema = Some((schema_value.clone(), sent_schema(schema_value)?));
    }
    // 5. Files.
    let (original, sent) = match prepared_schema {
        Some(prepared) => prepared,
        None => {
            let original = checks::schema_file(call, schema_value)?;
            let sent = sent_schema(&original)?;
            (original, sent)
        }
    };
    let mut text = prompt;
    let mut pictures = Vec::new();
    let context: &[Value] = request
        .get("context")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    for (i, value) in context.iter().enumerate() {
        let what = format!("context {}", i + 1);
        let file = wire::request_file(call, value, &what)?;
        let bytes = wire::read_file(file, &what)?;
        if file.kind.starts_with("image/") {
            let url = if pictures_unchanged {
                wire::data_url(&file.kind, &bytes)
            } else {
                reduce(&bytes, matte)
                    .ok_or_else(|| format!("{what} is not a decodable {}", file.kind))?
            };
            pictures.push(picture_part(url, detail));
        } else {
            let body = String::from_utf8(bytes).map_err(|_| format!("{what} is not UTF-8 text"))?;
            text.push_str(&format!("\n\n--- {what} ---\n{body}"));
        }
    }
    let mut messages = Vec::new();
    if let Some(system) = system {
        messages.push(json!({"role": "system", "content": system}));
    }
    let content = if pictures.is_empty() {
        Value::String(text)
    } else {
        let mut parts = vec![json!({"type": "text", "text": text})];
        parts.extend(pictures);
        Value::Array(parts)
    };
    messages.push(json!({"role": "user", "content": content}));
    let mut json_schema = Map::new();
    json_schema.insert("name".into(), Value::String(schema_name(&original)));
    json_schema.insert("strict".into(), Value::Bool(true));
    json_schema.insert("schema".into(), sent);
    if let Some(Value::String(description)) = original.get("description")
        && !description.is_empty()
    {
        json_schema.insert("description".into(), Value::String(description.clone()));
    }
    Ok(StructuredPrompt {
        messages,
        response_format: json!({"type": "json_schema", "json_schema": json_schema}),
        max_tokens,
    })
}

/// The value a structured answer's message holds: `parsed` when an object, else the JSON text of
/// `content`, unwrapped from `completionState` wrappers. The failure is what the provider
/// `returned`.
pub fn structured_value(message: &Map<String, Value>) -> Result<Value, &'static str> {
    let value = match message.get("parsed") {
        Some(parsed @ Value::Object(_)) => parsed.clone(),
        _ => {
            let text = message_text(message.get("content"));
            if text.trim().is_empty() {
                return Err("returned empty content");
            }
            // serde_json refuses NaN and the infinities, and keeps a repeated member's last
            // value.
            match serde_json::from_str::<Value>(&text) {
                Ok(value) => value,
                Err(_) => return Err("returned invalid JSON content"),
            }
        }
    };
    Ok(unwrap_completion(value))
}

/// A matte colour: `#rgb`, `#rrggbb` or `#rrggbbaa` (hex digits in either case), as RGBA.
fn parse_matte(text: &str) -> Option<[u8; 4]> {
    let hex = text.strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    match hex.len() {
        3 => {
            let mut out = [255u8; 4];
            for (i, c) in hex.chars().enumerate() {
                out[i] = u8::try_from(c.to_digit(16)? * 17).ok()?;
            }
            Some(out)
        }
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

/// The size a `width` × `height` picture is reduced to so it fits in `edge` × `edge`, or `None`
/// when it already fits. The aspect ratio is kept; the derived edge is the floor or the ceiling
/// of its exact value, whichever keeps the ratio better (the floor on a tie), at least 1 — the
/// thumbnail rule of the reference engine, so 2400 × 3000 becomes 1280 × 1600.
fn fit(width: u32, height: u32, edge: u32) -> Option<(u32, u32)> {
    if width <= edge && height <= edge || width == 0 || height == 0 {
        return None;
    }
    let aspect = f64::from(width) / f64::from(height);
    let side = f64::from(edge);
    let pick = |exact: f64, key: &dyn Fn(f64) -> f64| -> u32 {
        let (floor, ceil) = (exact.floor(), exact.ceil());
        let chosen = if key(ceil) < key(floor) { ceil } else { floor };
        (chosen as u32).max(1)
    };
    if 1.0 >= aspect {
        let x = pick(side * aspect, &|n| (aspect - n / side).abs());
        Some((x, edge))
    } else {
        let y = pick(side / aspect, &|n| {
            if n == 0.0 {
                0.0
            } else {
                (aspect - side / n).abs()
            }
        });
        Some((edge, y))
    }
}

/// `src` (RGBA) over a ground of `matte` (RGBA), then without alpha: Porter-Duff "over", as the
/// reference engine's flattening composites and then drops the alpha channel.
fn over(src: [u8; 4], matte: [u8; 4]) -> [u8; 3] {
    let sa = u32::from(src[3]);
    let ma = u32::from(matte[3]);
    let out_alpha = sa * 255 + ma * (255 - sa);
    if out_alpha == 0 {
        return [0, 0, 0];
    }
    let channel = |i: usize| {
        let value = u32::from(src[i]) * sa * 255 + u32::from(matte[i]) * ma * (255 - sa);
        ((value + out_alpha / 2) / out_alpha) as u8
    };
    [channel(0), channel(1), channel(2)]
}

/// A picture reduced for a structured call (spec/providers.md §9.2): decoded, flattened on
/// `matte`, fitted into [`PICTURE_EDGE`] × [`PICTURE_EDGE`] with Lanczos3 when larger, and
/// encoded as an RGB PNG data URL. `None` when the bytes do not decode.
///
/// Flattening before resampling keeps transparent pixels' colours out of the result, as the
/// reference engine's premultiplied resampling does; with an opaque matte the two orders are
/// the same up to rounding. The bytes are wire-only (not keyed), so parity is not required.
fn reduce(bytes: &[u8], matte: [u8; 4]) -> Option<String> {
    use image::ImageEncoder as _;
    let picture = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = picture.dimensions();
    let mut flat = image::RgbImage::new(width, height);
    for (src, dst) in picture.pixels().zip(flat.pixels_mut()) {
        *dst = image::Rgb(over(src.0, matte));
    }
    drop(picture);
    let flat = match fit(width, height, PICTURE_EDGE) {
        Some((w, h)) => image::imageops::resize(&flat, w, h, image::imageops::FilterType::Lanczos3),
        None => flat,
    };
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            flat.as_raw(),
            flat.width(),
            flat.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(wire::data_url("image/png", &png))
}

/// The schema's name: `title` with every character outside `[A-Za-z0-9]` replaced by `_`, cut
/// to 64 characters; `answer` when there is no title or nothing is left.
fn schema_name(schema: &Value) -> String {
    let name: String = schema
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(64)
        .collect();
    if name.is_empty() {
        "answer".into()
    } else {
        name
    }
}

/// The schema as it is sent: references inlined, then strict. The original must compile, since
/// the answer is checked against it.
fn sent_schema(original: &Value) -> Result<Value, String> {
    let inlined = schema::inline_refs(original)?;
    checks::schema_validator(original)?;
    Ok(schema::strict(&inlined))
}

/// Unwraps the `completionState` envelopes a provider sometimes puts around an answer
/// (spec/providers.md §9.2): a string holding such an object is parsed; a complete envelope
/// gives its `entries` (as an object), `items` (as a list) or `value`; an incomplete or
/// malformed one is left as it is; anything else is unwrapped member by member.
fn unwrap_completion(value: Value) -> Value {
    match value {
        Value::String(text) => {
            let stripped = text.trim();
            if !stripped.starts_with('{') {
                return Value::String(text);
            }
            match serde_json::from_str::<Value>(stripped) {
                Ok(parsed @ Value::Object(_)) if parsed.get("completionState").is_some() => {
                    unwrap_completion(parsed)
                }
                _ => Value::String(text),
            }
        }
        Value::Array(items) => Value::Array(items.into_iter().map(unwrap_completion).collect()),
        Value::Object(map) => {
            if !map.contains_key("completionState") {
                return Value::Object(
                    map.into_iter()
                        .map(|(key, item)| (key, unwrap_completion(item)))
                        .collect(),
                );
            }
            if map.get("completionState").and_then(Value::as_str) != Some("complete") {
                return Value::Object(map);
            }
            if let Some(Value::Array(entries)) = map.get("entries") {
                let mut decoded = Map::new();
                for entry in entries {
                    match entry.as_array().map(Vec::as_slice) {
                        Some([Value::String(key), item]) => {
                            decoded.insert(key.clone(), unwrap_completion(item.clone()));
                        }
                        _ => return Value::Object(map),
                    }
                }
                return Value::Object(decoded);
            }
            if let Some(Value::Array(items)) = map.get("items") {
                return Value::Array(items.iter().cloned().map(unwrap_completion).collect());
            }
            if let Some(item) = map.get("value") {
                return unwrap_completion(item.clone());
            }
            Value::Object(map)
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------------------------
// Agent turns

/// An agent turn's messages, tools, `tool_choice` and `max_tokens`, ready for a provider's body.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentPrompt {
    pub messages: Vec<Value>,
    pub tools: Vec<Value>,
    /// `auto` or `required`.
    pub tool_choice: &'static str,
    pub max_tokens: Option<u64>,
}

/// Steps 4 and 5 of an agent turn (spec/providers.md §5, §9.2), for a request that passed its
/// capability's check: the values (each tool's name `^[a-z][a-z0-9_]{0,63}$`, non-blank
/// description and object `parameters`; `max_tokens` at least 1; the transcript: known roles, text
/// contents, an id on every assistant call and every tool message, pictures as file values), then
/// the files: every picture's kind before any file is read (`messages[<i>].images[<j>] is <kind>,
/// not a picture`), then each as a data URL of its kind. Each picture part carries `detail`.
pub fn agent_prompt(call: &CallRequest, detail: Option<&str>) -> Result<AgentPrompt, String> {
    // 4. Values.
    let request = &call.request;
    let list = |name: &str| -> &[Value] {
        request
            .get(name)
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    };
    let tools = wire_tools(list("tools"))?;
    let max_tokens = max_tokens(request.get("max_tokens"))?;
    let tool_choice = if request.get("tool_choice").and_then(Value::as_str) == Some("auto") {
        "auto"
    } else {
        "required"
    };
    let system = request.get("system").and_then(Value::as_str).unwrap_or("");
    wire_messages(system, list("messages"), detail, &mut |_, _| {
        Ok(String::new())
    })?;
    // 5. Files: every picture's kind before any file is read, then the bytes.
    wire_messages(system, list("messages"), detail, &mut |value, member| {
        let file = wire::request_file(call, value, PICTURE)?;
        if !file.kind.starts_with("image/") {
            return Err(format!("{member} is {}, not a picture", file.kind));
        }
        Ok(String::new())
    })?;
    let messages = wire_messages(system, list("messages"), detail, &mut |value, _| {
        let file = wire::request_file(call, value, PICTURE)?;
        let bytes = wire::read_file(file, PICTURE)?;
        Ok(wire::data_url(&file.kind, &bytes))
    })?;
    Ok(AgentPrompt {
        messages,
        tools,
        tool_choice,
        max_tokens,
    })
}

/// The reply a turn's message holds: `{"text", "tool_calls": [{"id", "name", "arguments"}]}`.
/// The failure is what the provider `returned`.
pub fn agent_reply(message: &Map<String, Value>) -> Result<Value, &'static str> {
    let text = message_text(message.get("content"));
    let calls = reply_calls(message.get("tool_calls"))?;
    Ok(json!({"text": text, "tool_calls": calls}))
}

/// The tools of a request as the wire takes them (module doc).
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
                "parameters": schema::strict(parameters),
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
    detail: Option<&str>,
    picture: &mut Picture<'_>,
) -> Result<Vec<Value>, String> {
    match entry.get("images") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(images)) if images.iter().all(capabilities::is_file_value) => images
            .iter()
            .enumerate()
            .map(|(j, image)| {
                picture(image, &format!("messages[{index}].images[{j}]"))
                    .map(|url| picture_part(url, detail))
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
    detail: Option<&str>,
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
                let images = pictures(entry, index, detail, picture)?;
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
                held.extend(pictures(entry, index, detail, picture)?);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_parts_carry_a_detail_when_given() {
        assert_eq!(
            picture_part("data:x".into(), Some("high")),
            json!({"type": "image_url", "image_url": {"url": "data:x", "detail": "high"}})
        );
        assert_eq!(
            picture_part("data:x".into(), None),
            json!({"type": "image_url", "image_url": {"url": "data:x"}})
        );
    }

    #[test]
    fn mattes_parse() {
        assert_eq!(parse_matte("#ffffff"), Some([255, 255, 255, 255]));
        assert_eq!(parse_matte("#FFF"), Some([255, 255, 255, 255]));
        assert_eq!(parse_matte("#0a1"), Some([0, 170, 17, 255]));
        assert_eq!(parse_matte("#10203040"), Some([16, 32, 48, 64]));
        for bad in [
            "white", "ffffff", "#ffff", "#fffffff", "#gggggg", "# fff", "", "#",
        ] {
            assert_eq!(parse_matte(bad), None, "{bad}");
        }
    }

    #[test]
    fn pictures_fit_as_the_reference_thumbnail_does() {
        assert_eq!(fit(2400, 3000, 1600), Some((1280, 1600)));
        assert_eq!(fit(3000, 2400, 1600), Some((1600, 1280)));
        assert_eq!(fit(1600, 1600, 1600), None);
        assert_eq!(fit(1000, 1600, 1600), None);
        assert_eq!(fit(3200, 3200, 1600), Some((1600, 1600)));
        assert_eq!(fit(1601, 10, 1600), Some((1600, 10)));
        assert_eq!(fit(100_000, 10, 1600), Some((1600, 1)));
        assert_eq!(fit(10, 100_000, 1600), Some((1, 1600)));
        // 1601 x 1000: 999.375 → 999 (closer to the ratio).
        assert_eq!(fit(1601, 1000, 1600), Some((1600, 999)));
        // 2000 x 1999: exact 1599.2 → 1599.
        assert_eq!(fit(1999, 2000, 1600), Some((1599, 1600)));
    }

    #[test]
    fn flattening_is_over_the_matte() {
        assert_eq!(over([10, 20, 30, 255], [255, 255, 255, 255]), [10, 20, 30]);
        assert_eq!(over([10, 20, 30, 0], [255, 255, 255, 255]), [255, 255, 255]);
        assert_eq!(over([0, 0, 0, 128], [255, 255, 255, 255]), [127, 127, 127]);
        assert_eq!(over([200, 40, 40, 128], [0, 0, 0, 0]), [200, 40, 40]);
        assert_eq!(over([1, 2, 3, 0], [9, 9, 9, 0]), [0, 0, 0]);
    }

    #[test]
    fn small_pictures_are_flattened_but_not_resized() {
        let mut png = Vec::new();
        {
            use image::ImageEncoder as _;
            let pixels: Vec<u8> = [200u8, 40, 40, 0].repeat(4);
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(&pixels, 2, 2, image::ExtendedColorType::Rgba8)
                .unwrap();
        }
        let url = reduce(&png, [0, 0, 255, 255]).unwrap();
        let (media, payload) = wire::parse_data_url(&url).unwrap();
        assert_eq!(media, "image/png");
        let bytes = wire::strict_base64(&payload).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!(decoded.color(), image::ColorType::Rgb8);
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
        assert_eq!(decoded.to_rgb8().get_pixel(0, 0).0, [0, 0, 255]);
        assert_eq!(reduce(b"not a picture", [255; 4]), None);
    }

    #[test]
    fn schema_names_are_sanitized() {
        assert_eq!(schema_name(&json!({"title": "Face box-v2"})), "Face_box_v2");
        assert_eq!(schema_name(&json!({"title": "Café"})), "Caf_");
        assert_eq!(
            schema_name(&json!({"title": "x".repeat(80)})),
            "x".repeat(64)
        );
        assert_eq!(schema_name(&json!({})), "answer");
        assert_eq!(schema_name(&json!({"title": ""})), "answer");
        assert_eq!(schema_name(&json!({"title": 5})), "answer");
    }

    #[test]
    fn max_tokens_is_a_whole_number_of_at_least_one() {
        assert_eq!(max_tokens(None), Ok(None));
        assert_eq!(max_tokens(Some(&Value::Null)), Ok(None));
        assert_eq!(max_tokens(Some(&json!(4000))), Ok(Some(4000)));
        assert_eq!(max_tokens(Some(&json!(4000.0))), Ok(Some(4000)));
        for bad in [json!(0), json!(-3), json!(0.5)] {
            assert_eq!(
                max_tokens(Some(&bad)).unwrap_err(),
                "max_tokens must be at least 1"
            );
        }
        assert!(max_tokens(Some(&json!(1e300))).is_err());
    }

    #[test]
    fn completion_state_wrappers_unwrap() {
        let wrapped = json!({
            "completionState": "complete",
            "entries": [
                ["status", {"completionState": "complete", "value": "located"}],
                ["boxes", {"completionState": "complete", "items": [1, {"completionState": "complete", "value": 2}]}],
                ["nested", "{\"completionState\": \"complete\", \"value\": true}"],
                ["text", "{not json"]
            ]
        });
        assert_eq!(
            unwrap_completion(wrapped),
            json!({"status": "located", "boxes": [1, 2], "nested": true, "text": "{not json"})
        );
        let incomplete = json!({"a": {"completionState": "partial", "value": 1}});
        assert_eq!(unwrap_completion(incomplete.clone()), incomplete);
        let malformed = json!({"completionState": "complete", "entries": [["a"]]});
        assert_eq!(unwrap_completion(malformed.clone()), malformed);
        let no_payload = json!({"completionState": "complete"});
        assert_eq!(unwrap_completion(no_payload.clone()), no_payload);
        assert_eq!(
            unwrap_completion(json!(" {\"completionState\":\"complete\",\"value\":[1]} ")),
            json!([1])
        );
        assert_eq!(
            unwrap_completion(json!("{\"other\": 1}")),
            json!("{\"other\": 1}")
        );
        assert_eq!(
            unwrap_completion(json!([{"x": {"completionState": "complete", "value": "y"}}])),
            json!([{"x": "y"}])
        );
    }

    #[test]
    fn message_text_joins_text_parts() {
        assert_eq!(message_text(Some(&json!("a"))), "a");
        assert_eq!(
            message_text(Some(&json!([
                {"type": "text", "text": "{\"a\":"},
                {"type": "image_url", "image_url": {"url": "x"}},
                {"type": "text", "text": 1},
                {"type": "text", "text": "1}"}
            ]))),
            "{\"a\":1}"
        );
        assert_eq!(message_text(Some(&Value::Null)), "");
        assert_eq!(message_text(None), "");
        assert_eq!(message_text(Some(&json!({"text": "x"}))), "");
    }

    #[test]
    fn structured_values_come_from_parsed_or_content() {
        let message = |value: Value| value.as_object().unwrap().clone();
        assert_eq!(
            structured_value(&message(json!({"parsed": {"a": 1}, "content": "{}"}))),
            Ok(json!({"a": 1}))
        );
        assert_eq!(
            structured_value(&message(json!({"content": "{\"a\": 2}"}))),
            Ok(json!({"a": 2}))
        );
        assert_eq!(
            structured_value(&message(json!({"content": "  "}))),
            Err("returned empty content")
        );
        assert_eq!(
            structured_value(&message(json!({"content": "{nope"}))),
            Err("returned invalid JSON content")
        );
    }

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
        let out = wire_messages("", &messages, Some("low"), &mut picture_url).unwrap();
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
        let refused =
            |message: Value| wire_messages("s", &[message], None, &mut picture_url).unwrap_err();
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
            None,
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
