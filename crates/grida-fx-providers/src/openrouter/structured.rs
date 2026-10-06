//! OpenRouter structured output: `POST {base}/chat/completions` with a strict JSON-schema
//! `response_format` and provider routing from the route contract (spec/providers.md §9.2;
//! spec/capabilities.md §4).
//!
//! Contract: `adapter: openrouter-structured`, `adapter_behavior: 1`, optional `request_policy`
//! ([`RequestPolicy`]) and `pictures` (absent: reduce to a 1600 px long edge flattened on `matte`;
//! `"unchanged"`: send the file as is). The schema is a `json` file or an inline object; it is
//! sent after `$ref` inlining and strict canonicalization ([`super::schema`]), named from its
//! `title` (else `answer`). Text context files are appended to the prompt as
//! `--- context <i> ---\n<text>`. The answer's value (`message.parsed`, else the JSON text of
//! `message.content`, unwrapped from `completionState`) is `data: {"json": <value>}`; `cost` from
//! `usage` ([`super::usage_cost`]). `check`: the value against the request's **original**
//! schema (draft 2020-12; `format` not asserted), refused as `<path>: <message>`. Deadline 900 s
//! for `openai/gpt-6-astra`, 1800 s otherwise.
//!
//! `send`, in the order of spec/providers.md §5:
//! 1. the contract (`read_contract`): `adapter` and `adapter_behavior`, when present, are this
//!    adapter's; `request_policy` and `pictures` are valid; no other member;
//! 2. `capabilities::check_request`;
//! 3. the key (`authorization: Bearer <OPENROUTER_API_KEY>`);
//! 4. values: a non-blank `prompt`; `matte` a colour (`#rgb`, `#rrggbb`, `#rrggbbaa`);
//!    `max_tokens` at least 1; an inline schema whose references inline and which compiles as a
//!    draft 2020-12 JSON Schema;
//! 5. files: the schema file (UTF-8 JSON, an object, then as an inline schema); each context
//!    file, a picture (`image/*`, reduced unless the contract says `unchanged`) or UTF-8 text;
//! 6. a body over 200 MiB is refused; otherwise one exchange.
//!
//! Non-2xx and transport outcomes are [`super::classify`]'s (a transport's own refusal is
//! `Refused`, spec/providers.md §4.2). A 2xx that does not become an answer is
//! `Failed { cost: <the usage's cost when the body parsed>, retryable: true }` (§4.4).

use super::schema;
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, RequestAdapter, RouteRef, Sent};
use crate::capabilities;
use crate::checks;
use crate::setup::Client;
use crate::transport::{Body, Credential, HttpRequest, HttpResponse, Lane, Method};
use crate::wire;
use grida_fx_core::money::Usd;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::sync::LazyLock;
use std::time::Duration;

/// The label of every reason.
pub const LABEL: &str = "OpenRouter structured generation";

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "openrouter-structured";

/// The default `max_tokens`.
pub const DEFAULT_MAX_TOKENS: u64 = 16_000;

/// The long edge a reduced picture fits in.
pub const PICTURE_EDGE: u32 = 1600;

/// The capability this adapter serves.
pub const CAPABILITY: &str = "structured.generate";

/// The model whose structured calls get the shorter deadline (spec/providers.md §9.2, "Limits").
pub const FAST_DEADLINE_MODEL: &str = "openai/gpt-6-astra";

/// The deadline of a structured call on [`FAST_DEADLINE_MODEL`].
pub const FAST_DEADLINE: Duration = Duration::from_secs(900);

/// The deadline of every other structured call.
pub const DEADLINE: Duration = Duration::from_secs(1800);

/// The colour reduced pictures are flattened on when the request gives none.
pub const DEFAULT_MATTE: &str = "#ffffff";

/// The `reasoning.effort` values a policy may give.
pub const EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The `image_detail` values a policy may give.
pub const IMAGE_DETAILS: [&str; 4] = ["auto", "low", "high", "original"];

static PROVIDER_SLUG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9][a-z0-9._-]*(?:/[a-z0-9][a-z0-9._-]*)*$").expect("a valid pattern")
});

/// A route contract's `request_policy` (shared with [`super::agent`]): `provider`
/// (`require_parameters` must be true, the default; `only` a non-empty list of distinct slugs;
/// `allow_fallbacks` a bool), `reasoning.effort` (`none`, `minimal`, `low`, `medium`, `high`,
/// `xhigh`, `max`) and `image_detail` (`auto`, `low`, `high`, `original`). Anything else refuses.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestPolicy {
    /// The `provider` object sent, members in the order `require_parameters`, `only`,
    /// `allow_fallbacks`.
    pub provider: Value,
    pub effort: Option<String>,
    pub image_detail: Option<String>,
}

impl Default for RequestPolicy {
    /// No policy: `{"require_parameters": true}`, no effort, no detail.
    fn default() -> RequestPolicy {
        RequestPolicy {
            provider: json!({"require_parameters": true}),
            effort: None,
            image_detail: None,
        }
    }
}

impl RequestPolicy {
    /// Reads `contract.request_policy` (absent: `{"require_parameters": true}`, no effort, no
    /// detail). The refusal is a sentence.
    ///
    /// A member given as `null` counts as absent.
    pub fn from_contract(contract: &Value) -> Result<RequestPolicy, String> {
        let policy = match contract.get("request_policy") {
            None | Some(Value::Null) => return Ok(RequestPolicy::default()),
            Some(Value::Object(policy)) => policy,
            Some(_) => return Err("request_policy is an object".into()),
        };
        only_members(
            policy,
            "request_policy",
            &["provider", "reasoning", "image_detail"],
        )?;
        let provider = match policy.get("provider") {
            None | Some(Value::Null) => json!({"require_parameters": true}),
            Some(Value::Object(provider)) => provider_routing(provider)?,
            Some(_) => return Err("request_policy.provider is an object".into()),
        };
        let effort = match policy.get("reasoning") {
            None | Some(Value::Null) => None,
            Some(Value::Object(reasoning)) => {
                only_members(reasoning, "request_policy.reasoning", &["effort"])?;
                match reasoning.get("effort") {
                    Some(Value::String(effort)) if EFFORTS.contains(&effort.as_str()) => {
                        Some(effort.clone())
                    }
                    _ => {
                        return Err(format!(
                            "request_policy.reasoning.effort is one of {}",
                            EFFORTS.join(", ")
                        ));
                    }
                }
            }
            Some(_) => return Err("request_policy.reasoning is an object".into()),
        };
        let image_detail = match policy.get("image_detail") {
            None | Some(Value::Null) => None,
            Some(Value::String(detail)) if IMAGE_DETAILS.contains(&detail.as_str()) => {
                Some(detail.clone())
            }
            Some(_) => {
                return Err(format!(
                    "request_policy.image_detail is one of {}",
                    IMAGE_DETAILS.join(", ")
                ));
            }
        };
        Ok(RequestPolicy {
            provider,
            effort,
            image_detail,
        })
    }

    /// The `reasoning` member of a body, when the policy gives an effort.
    pub fn reasoning(&self) -> Option<Value> {
        self.effort.as_ref().map(|effort| json!({"effort": effort}))
    }

    /// The `image_url` part of a picture, with the policy's `detail`.
    pub fn picture_part(&self, url: String) -> Value {
        let mut image_url = Map::new();
        image_url.insert("url".into(), Value::String(url));
        if let Some(detail) = &self.image_detail {
            image_url.insert("detail".into(), Value::String(detail.clone()));
        }
        json!({"type": "image_url", "image_url": image_url})
    }
}

fn only_members(map: &Map<String, Value>, what: &str, allowed: &[&str]) -> Result<(), String> {
    match map.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(format!("{what} takes no member {key}")),
        None => Ok(()),
    }
}

fn provider_routing(provider: &Map<String, Value>) -> Result<Value, String> {
    only_members(
        provider,
        "request_policy.provider",
        &["require_parameters", "only", "allow_fallbacks"],
    )?;
    match provider.get("require_parameters") {
        None | Some(Value::Null) | Some(Value::Bool(true)) => {}
        Some(_) => return Err("request_policy.provider.require_parameters must be true".into()),
    }
    let mut out = Map::new();
    out.insert("require_parameters".into(), Value::Bool(true));
    match provider.get("only") {
        None | Some(Value::Null) => {}
        Some(Value::Array(slugs)) if valid_slugs(slugs) => {
            out.insert("only".into(), Value::Array(slugs.clone()));
        }
        Some(_) => {
            return Err(
                "request_policy.provider.only is a non-empty list of distinct provider slugs"
                    .into(),
            );
        }
    }
    match provider.get("allow_fallbacks") {
        None | Some(Value::Null) => {}
        Some(Value::Bool(allow)) => {
            out.insert("allow_fallbacks".into(), Value::Bool(*allow));
        }
        Some(_) => return Err("request_policy.provider.allow_fallbacks is true or false".into()),
    }
    Ok(Value::Object(out))
}

fn valid_slugs(slugs: &[Value]) -> bool {
    let mut seen: Vec<&str> = Vec::new();
    for slug in slugs {
        let Some(slug) = slug.as_str() else {
            return false;
        };
        if !PROVIDER_SLUG.is_match(slug) || seen.contains(&slug) {
            return false;
        }
        seen.push(slug);
    }
    !seen.is_empty()
}

/// What a route contract asks of an OpenRouter chat adapter (spec/providers.md §5 step 1,
/// §9.2): its policy, and for structured output whether pictures go unchanged.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ChatContract {
    pub(super) policy: RequestPolicy,
    pub(super) pictures_unchanged: bool,
}

/// Reads the route's contract for the adapter `adapter` serving `capability`. `adapter` and
/// `adapter_behavior` (the integer 1), when present, must be this adapter's; `request_policy` is
/// read by [`RequestPolicy::from_contract`]; `pictures` (absent or `"unchanged"`) only when
/// `takes_pictures`; no other member. Refused with `<route> is not a <capability> route this
/// adapter serves`, followed by `: <why>` when a member is not one it can serve.
pub(super) fn read_contract(
    route: &RouteRef,
    capability: &str,
    adapter: &str,
    takes_pictures: bool,
) -> Result<ChatContract, String> {
    let refused = |why: Option<String>| {
        let mut reason = format!(
            "{} is not a {capability} route this adapter serves",
            route.id()
        );
        if let Some(why) = why {
            reason.push_str(": ");
            reason.push_str(&why);
        }
        reason
    };
    if route.capability != capability {
        return Err(refused(None));
    }
    let empty = Map::new();
    let contract = match &route.contract {
        Value::Null => &empty,
        Value::Object(contract) => contract,
        _ => return Err(refused(None)),
    };
    match contract.get("adapter") {
        None | Some(Value::Null) => {}
        Some(Value::String(name)) if name == adapter => {}
        Some(_) => return Err(refused(None)),
    }
    match contract.get("adapter_behavior") {
        None | Some(Value::Null) => {}
        Some(Value::Number(n)) if n.as_u64() == Some(1) => {}
        Some(_) => return Err(refused(None)),
    }
    let mut allowed = vec!["adapter", "adapter_behavior", "request_policy"];
    if takes_pictures {
        allowed.push("pictures");
    }
    only_members(contract, "the contract", &allowed).map_err(|why| refused(Some(why)))?;
    let pictures_unchanged = match contract.get("pictures") {
        None | Some(Value::Null) => false,
        Some(Value::String(pictures)) if pictures == "unchanged" => true,
        Some(_) => {
            return Err(refused(Some(
                "pictures is \"unchanged\" or absent".to_string(),
            )));
        }
    };
    let policy = RequestPolicy::from_contract(&route.contract).map_err(|why| refused(Some(why)))?;
    Ok(ChatContract {
        policy,
        pictures_unchanged,
    })
}

// ---------------------------------------------------------------------------------------------
// Helpers shared with the agent adapter

/// The chat completions request carrying `body`.
pub(super) fn chat_request(
    client: &Client,
    credential: Credential,
    body: Value,
    timeout: Duration,
) -> HttpRequest {
    HttpRequest::new(Method::Post, client.url("chat/completions"), Lane::Provider)
        .credential(credential)
        .body(Body::Json(body))
        .timeout(timeout)
        .max_response_bytes(super::MAX_CHAT_RESPONSE_BYTES)
}

/// `choices[0].message`, when an object.
pub(super) fn first_message(body: &Map<String, Value>) -> Option<&Map<String, Value>> {
    body.get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(Value::as_object)
}

/// A message's text: a string `content`, or the `text` of every `{"type": "text", "text": <s>}`
/// part of a list, joined; anything else is `""`.
pub(super) fn message_text(content: Option<&Value>) -> String {
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

/// The cost `usage` reports ([`super::usage_cost`]).
pub(super) fn reported_cost(body: &Map<String, Value>) -> Option<Usd> {
    super::usage_cost(body.get("usage"))
}

/// A structural failure of a 2xx (spec/providers.md §4.4): billed, retryable.
pub(super) fn structural(
    client: &Client,
    response: &HttpResponse,
    reason: &str,
    cost: Option<Usd>,
) -> Sent {
    let reason = super::with_request_id(reason.to_string(), response);
    Sent::Failed {
        reason: client.reason(&reason),
        cost,
        retryable: true,
    }
}

/// `max_tokens`: absent or `null` is `None`; otherwise a whole number of at least 1 (the type is
/// [`capabilities::check_request`]'s).
pub(super) fn max_tokens(value: Option<&Value>) -> Result<Option<u64>, String> {
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
// Pictures

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

// ---------------------------------------------------------------------------------------------
// Schemas and answers

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
// The adapter

/// OpenRouter's structured-output adapter (module doc).
#[derive(Debug, Clone)]
pub struct OpenRouterStructured {
    client: Client,
}

/// A call ready to send.
struct Prepared {
    credential: Credential,
    body: Value,
    timeout: Duration,
}

impl OpenRouterStructured {
    pub fn new(client: Client) -> OpenRouterStructured {
        OpenRouterStructured { client }
    }

    /// Steps 1 to 5 of the module doc, and the body.
    fn prepare(&self, call: &CallRequest) -> Result<Prepared, String> {
        // 1. The contract.
        let contract = read_contract(&call.route, CAPABILITY, CONTRACT_ADAPTER, true)?;
        // 2. The capability.
        capabilities::check_request(CAPABILITY, &call.request)?;
        // 3. The key.
        let credential = self.client.credential("authorization", "Bearer ")?;
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
                let url = if contract.pictures_unchanged {
                    wire::data_url(&file.kind, &bytes)
                } else {
                    reduce(&bytes, matte)
                        .ok_or_else(|| format!("{what} is not a decodable {}", file.kind))?
                };
                pictures.push(contract.policy.picture_part(url));
            } else {
                let body =
                    String::from_utf8(bytes).map_err(|_| format!("{what} is not UTF-8 text"))?;
                text.push_str(&format!("\n\n--- {what} ---\n{body}"));
            }
        }
        // The body (spec/providers.md §9.2).
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
        let mut body = Map::new();
        body.insert(
            "model".into(),
            Value::String(call.route.model.trim().into()),
        );
        body.insert("messages".into(), Value::Array(messages));
        body.insert(
            "response_format".into(),
            json!({"type": "json_schema", "json_schema": json_schema}),
        );
        body.insert("provider".into(), contract.policy.provider.clone());
        if let Some(reasoning) = contract.policy.reasoning() {
            body.insert("reasoning".into(), reasoning);
        }
        body.insert("max_tokens".into(), json!(max_tokens));
        let body = Value::Object(body);
        if super::body_exceeds(&body, super::MAX_REQUEST_BYTES) {
            return Err(super::BODY_TOO_LARGE.into());
        }
        let timeout = if call.route.model.trim() == FAST_DEADLINE_MODEL {
            FAST_DEADLINE
        } else {
            DEADLINE
        };
        Ok(Prepared {
            credential,
            body,
            timeout,
        })
    }

    /// A 2xx's body as an answer (spec/providers.md §9.2).
    fn parse(&self, response: &HttpResponse) -> Sent {
        let body = match wire::json_object(&response.body, LABEL) {
            Ok(body) => body,
            Err(reason) => return structural(&self.client, response, &reason, None),
        };
        let cost = reported_cost(&body);
        let failed =
            |what: &str| structural(&self.client, response, &format!("{LABEL} {what}"), cost);
        let Some(message) = first_message(&body) else {
            return failed("returned no message");
        };
        let value = match message.get("parsed") {
            Some(parsed @ Value::Object(_)) => parsed.clone(),
            _ => {
                let text = message_text(message.get("content"));
                if text.trim().is_empty() {
                    return failed("returned empty content");
                }
                // serde_json refuses NaN and the infinities, and keeps a repeated member's last
                // value.
                match serde_json::from_str::<Value>(&text) {
                    Ok(value) => value,
                    Err(_) => return failed("returned invalid JSON content"),
                }
            }
        };
        let value = unwrap_completion(value);
        Sent::Answered(Answer::new(json!({"json": value}), cost))
    }
}

impl RequestAdapter for OpenRouterStructured {
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
            let request = chat_request(
                &self.client,
                prepared.credential,
                prepared.body,
                prepared.timeout,
            );
            match super::classify(&self.client, LABEL, self.client.send(request).await) {
                Ok(response) => self.parse(&response),
                Err(sent) => sent,
            }
        })
    }

    /// The answer's value against the request's original schema (spec/capabilities.md §4;
    /// [`checks::structured`]).
    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        checks::structured(call, answer, &self.client.redactor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(contract: Value) -> RouteRef {
        RouteRef {
            capability: CAPABILITY.into(),
            model: "acme/text-a".into(),
            provider: "openrouter".into(),
            contract,
        }
    }

    #[test]
    fn a_policy_reads_from_the_contract() {
        assert_eq!(
            RequestPolicy::from_contract(&json!({})).unwrap(),
            RequestPolicy::default()
        );
        assert_eq!(
            RequestPolicy::default().provider,
            json!({"require_parameters": true})
        );
        let policy = RequestPolicy::from_contract(&json!({"request_policy": {
            "image_detail": "high",
            "reasoning": {"effort": "high"},
            "provider": {"allow_fallbacks": false, "only": ["openai"], "require_parameters": true}
        }}))
        .unwrap();
        // Members in the order require_parameters, only, allow_fallbacks.
        assert_eq!(
            serde_json::to_string(&policy.provider).unwrap(),
            r#"{"require_parameters":true,"only":["openai"],"allow_fallbacks":false}"#
        );
        assert_eq!(policy.effort.as_deref(), Some("high"));
        assert_eq!(policy.image_detail.as_deref(), Some("high"));
        assert_eq!(policy.reasoning(), Some(json!({"effort": "high"})));
        assert_eq!(
            policy.picture_part("data:x".into()),
            json!({"type": "image_url", "image_url": {"url": "data:x", "detail": "high"}})
        );
        assert_eq!(
            RequestPolicy::default().picture_part("data:x".into()),
            json!({"type": "image_url", "image_url": {"url": "data:x"}})
        );
        // provider absent: the default; require_parameters absent: true.
        let bare = RequestPolicy::from_contract(
            &json!({"request_policy": {"provider": {"only": ["a/b-c.d_e"]}}}),
        )
        .unwrap();
        assert_eq!(
            bare.provider,
            json!({"require_parameters": true, "only": ["a/b-c.d_e"]})
        );
    }

    #[test]
    fn every_documented_effort_and_detail_is_accepted() {
        for effort in EFFORTS {
            let policy = RequestPolicy::from_contract(
                &json!({"request_policy": {"reasoning": {"effort": effort}}}),
            )
            .unwrap();
            assert_eq!(policy.reasoning(), Some(json!({"effort": effort})));
        }
        for detail in IMAGE_DETAILS {
            let policy =
                RequestPolicy::from_contract(&json!({"request_policy": {"image_detail": detail}}))
                    .unwrap();
            assert_eq!(policy.image_detail.as_deref(), Some(detail));
        }
    }

    #[test]
    fn bad_policies_are_refused_with_the_member_named() {
        let refused = |policy: Value| {
            RequestPolicy::from_contract(&json!({ "request_policy": policy })).unwrap_err()
        };
        assert_eq!(refused(json!([])), "request_policy is an object");
        assert_eq!(
            refused(json!({"temperature": 0})),
            "request_policy takes no member temperature"
        );
        assert_eq!(
            refused(json!({"provider": ["openai"]})),
            "request_policy.provider is an object"
        );
        assert_eq!(
            refused(json!({"provider": {"order": ["openai"]}})),
            "request_policy.provider takes no member order"
        );
        for bad in [json!(false), json!(1), json!("true")] {
            assert_eq!(
                refused(json!({"provider": {"require_parameters": bad}})),
                "request_policy.provider.require_parameters must be true"
            );
        }
        for bad in [
            json!([]),
            json!(["openai", "openai"]),
            json!([""]),
            json!(["openai "]),
            json!(["https://example.test/provider"]),
            json!([7]),
            json!(["openai\nsecret"]),
            json!(["Openai"]),
            json!("openai"),
        ] {
            assert_eq!(
                refused(json!({"provider": {"only": bad}})),
                "request_policy.provider.only is a non-empty list of distinct provider slugs",
                "{bad}"
            );
        }
        for bad in [json!(1), json!("false")] {
            assert_eq!(
                refused(json!({"provider": {"allow_fallbacks": bad}})),
                "request_policy.provider.allow_fallbacks is true or false"
            );
        }
        for bad in [
            json!({"effort": "ultra"}),
            json!({"effort": true}),
            json!({}),
        ] {
            assert_eq!(
                refused(json!({ "reasoning": bad })),
                "request_policy.reasoning.effort is one of none, minimal, low, medium, high, xhigh, max"
            );
        }
        assert_eq!(
            refused(json!({"reasoning": "high"})),
            "request_policy.reasoning is an object"
        );
        assert_eq!(
            refused(json!({"reasoning": {"effort": "high", "summary": "auto"}})),
            "request_policy.reasoning takes no member summary"
        );
        for bad in [json!("medium"), json!(1)] {
            assert_eq!(
                refused(json!({ "image_detail": bad })),
                "request_policy.image_detail is one of auto, low, high, original"
            );
        }
    }

    #[test]
    fn contracts_are_read_or_refused() {
        let read =
            |contract: Value| read_contract(&route(contract), CAPABILITY, CONTRACT_ADAPTER, true);
        let ok = read(json!({"adapter": "openrouter-structured", "adapter_behavior": 1})).unwrap();
        assert_eq!(ok.policy, RequestPolicy::default());
        assert!(!ok.pictures_unchanged);
        assert!(read(json!({})).is_ok());
        assert!(
            read(json!({"adapter": "openrouter-structured", "adapter_behavior": 1, "pictures": "unchanged"}))
                .unwrap()
                .pictures_unchanged
        );
        let foreign =
            "acme/text-a@openrouter is not a structured.generate route this adapter serves";
        for contract in [
            json!({"adapter": "openrouter-tool-loop", "adapter_behavior": 1}),
            json!({"adapter": "openrouter-structured", "adapter_behavior": 2}),
            json!({"adapter": "openrouter-structured", "adapter_behavior": "1"}),
            json!({"adapter": 1}),
            json!([]),
        ] {
            assert_eq!(read(contract.clone()).unwrap_err(), foreign, "{contract}");
        }
        assert_eq!(
            read(json!({"pictures": "reduced"})).unwrap_err(),
            format!("{foreign}: pictures is \"unchanged\" or absent")
        );
        assert_eq!(
            read(json!({"route_id": "x"})).unwrap_err(),
            format!("{foreign}: the contract takes no member route_id")
        );
        assert_eq!(
            read(json!({"request_policy": {"image_detail": "medium"}})).unwrap_err(),
            format!("{foreign}: request_policy.image_detail is one of auto, low, high, original")
        );
        // The agent's contract has no pictures.
        assert_eq!(
            read_contract(
                &RouteRef {
                    capability: "agent.turn".into(),
                    ..route(json!({"pictures": "unchanged"}))
                },
                "agent.turn",
                "openrouter-tool-loop",
                false
            )
            .unwrap_err(),
            "acme/text-a@openrouter is not a agent.turn route this adapter serves: the contract takes no member pictures"
        );
        // Another capability's route is not served.
        assert_eq!(
            read_contract(
                &route(json!({})),
                "agent.turn",
                "openrouter-tool-loop",
                false
            )
            .unwrap_err(),
            "acme/text-a@openrouter is not a agent.turn route this adapter serves"
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
}
