//! Wire helpers every adapter shares (spec/adapters.md §3–§5, §8): base URLs, JSON bodies, strict
//! base64 and data URLs, media types and byte signatures, reported costs, safe error details,
//! request files, provider options, what a generic image adapter sends, image sizes, the image
//! checks, and a picture encoded as PNG.
//!
//! These are complete and tested here; provider modules use them rather than their own copies, so
//! two providers never disagree on what "a PNG", "a reported cost" or "an exact size" means.

use crate::adapter::{CallRequest, RequestFile};
use crate::redact::Redactor;
use crate::transport::HttpResponse;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use grida_fx_core::money::Usd;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::LazyLock;

// ---------------------------------------------------------------------------------------------
// Base URLs and download hosts

/// Normalizes a provider base URL (spec/providers.md §1); the core's, which reads declared
/// defaults with it.
pub use grida_fx_core::providers::normalize_base_url;

pub(crate) use grida_fx_core::providers::is_loopback;

/// Why a result's URL is not downloaded ([`download_url`]): it does not parse, or it is not on its
/// provider's hosts. Each caller words its own sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffHost {
    Malformed,
    Outside,
}

/// A result's URL, parsed, when it may be downloaded from a provider whose declaration lists
/// `hosts` (spec/providers.md §1): `https`; a host that is a name, not an address, and is one of
/// `hosts` or lies below a `*.<domain>` entry's domain, compared in lower case without trailing
/// dots; no userinfo; no port but 443 (which the parser drops); no fragment. A query is allowed.
/// A download sends the provider's text as given; the transport parses it with the same parser,
/// so the host checked here is the host asked.
pub fn download_url(url: &str, hosts: &[String]) -> Result<url::Url, OffHost> {
    let parsed = url::Url::parse(url).map_err(|_| OffHost::Malformed)?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.fragment().is_some()
    {
        return Err(OffHost::Outside);
    }
    let Some(url::Host::Domain(domain)) = parsed.host() else {
        return Err(OffHost::Outside);
    };
    let host = domain.trim_end_matches('.').to_ascii_lowercase();
    let listed = hosts.iter().any(|entry| match entry.strip_prefix("*.") {
        Some(below) => {
            host.strip_suffix(below)
                .is_some_and(|rest| rest.ends_with('.'))
                && !host.starts_with('.')
                && !host.contains("..")
        }
        None => host == *entry,
    });
    if listed {
        Ok(parsed)
    } else {
        Err(OffHost::Outside)
    }
}

// ---------------------------------------------------------------------------------------------
// JSON bodies and statuses

/// The body as a JSON object: `<label> returned invalid JSON` / `<label> returned a non-object
/// JSON response`.
pub fn json_object(body: &[u8], label: &str) -> Result<Map<String, Value>, String> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("{label} returned a non-object JSON response")),
        Err(_) => Err(format!("{label} returned invalid JSON")),
    }
}

/// `<label> returned HTTP <status>`, plus `: <detail>` when there is one.
pub fn status_reason(label: &str, status: u16, detail: Option<&str>) -> String {
    match detail {
        Some(detail) if !detail.is_empty() => format!("{label} returned HTTP {status}: {detail}"),
        _ => format!("{label} returned HTTP {status}"),
    }
}

/// The provider's request id: the first of `x-request-id`, `x-openrouter-request-id`,
/// `request-id` that is present and matches `^[A-Za-z0-9_.:-]{1,96}$`. For reasons only; never
/// part of an answer's `data`.
pub fn request_id(response: &HttpResponse) -> Option<String> {
    ["x-request-id", "x-openrouter-request-id", "request-id"]
        .iter()
        .find_map(|name| response.header(name))
        .filter(|id| SAFE_FIELD.is_match(id))
        .map(str::to_string)
}

static SAFE_FIELD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.:-]{1,96}$").expect("a valid pattern"));

/// The allowlisted detail of an error body in the `{"error": {...}}` envelope OpenAI and
/// OpenRouter use (spec/adapters.md §7):
/// 1. `error` must be an object; when `error.metadata.raw` is a string holding JSON with an
///    `error` object, that inner object is used instead;
/// 2. from `message`, only fixed summaries, matched case-insensitively, de-duplicated, in this
///    order: `invalid schema` (`\binvalid (?:json )?schema\b`), `required must include every key`,
///    `unsupported parameter`, `response_format`, joined as `message=<a>, <b>`;
/// 3. `type`, `code`, `param`, each when a string or a non-boolean integer whose text matches
///    `^[A-Za-z0-9_.:-]{1,96}$` after redaction, as `key=value`;
/// 4. the parts joined with `; `, whitespace collapsed, bounded to 720 characters with `…`.
///
/// `None` when the body is not that envelope or nothing is kept. The raw message is never copied.
pub fn safe_error_detail(body: &[u8], redactor: &Redactor) -> Option<String> {
    let error = error_envelope(body)?;
    let mut parts = Vec::new();
    if let Some(Value::String(message)) = error.get("message") {
        let summaries: Vec<&str> = MESSAGE_SUMMARIES
            .iter()
            .filter(|(pattern, _)| pattern.is_match(message))
            .map(|(_, summary)| *summary)
            .collect();
        if !summaries.is_empty() {
            parts.push(format!("message={}", summaries.join(", ")));
        }
    }
    for key in ["type", "code", "param"] {
        let text = match error.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) if n.is_i64() || n.is_u64() => n.to_string(),
            _ => continue,
        };
        let text = redactor.redact(&text);
        if SAFE_FIELD.is_match(&text) {
            parts.push(format!("{key}={text}"));
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(crate::redact::bounded(&parts.join("; "), 720))
}

/// The `error` object of an OpenAI-style envelope, unwrapping `metadata.raw` (see
/// [`safe_error_detail`]).
pub fn error_envelope(body: &[u8]) -> Option<Map<String, Value>> {
    let Ok(Value::Object(payload)) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    let Some(Value::Object(error)) = payload.get("error") else {
        return None;
    };
    let inner = error
        .get("metadata")
        .and_then(|m| m.get("raw"))
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|raw| match raw.get("error") {
            Some(Value::Object(inner)) => Some(inner.clone()),
            _ => None,
        });
    Some(inner.unwrap_or_else(|| error.clone()))
}

static MESSAGE_SUMMARIES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"(?i)\binvalid (?:json )?schema\b", "invalid schema"),
        (
            r"(?i)\brequired must include every key\b",
            "required must include every key",
        ),
        (r"(?i)\bunsupported parameter\b", "unsupported parameter"),
        (r"(?i)\bresponse_format\b", "response_format"),
    ]
    .into_iter()
    .map(|(pattern, summary)| (Regex::new(pattern).expect("a valid pattern"), summary))
    .collect()
});

/// Whether the compact JSON encoding of `body` (what the transport sends) is longer than `limit`
/// bytes. Counts without building the encoding, and stops at the limit.
pub fn body_exceeds(body: &Value, limit: usize) -> bool {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes = self.bytes.saturating_add(buf.len());
            if self.bytes > self.limit {
                return Err(std::io::Error::other("over the limit"));
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, body).is_err() || counter.bytes > limit
}

// ---------------------------------------------------------------------------------------------
// Base64 and data URLs

static STRICT_BASE64: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$")
        .expect("a valid pattern")
});

/// Strict standard base64 (RFC 4648 §4): non-empty, a multiple of 4 long, the standard alphabet,
/// padding only at the end, no whitespace, decoding to at least one byte. `None` otherwise.
pub fn strict_base64(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.len().is_multiple_of(4) || !STRICT_BASE64.is_match(text) {
        return None;
    }
    STANDARD.decode(text).ok().filter(|bytes| !bytes.is_empty())
}

/// `data:<media>;base64,<standard base64 with padding>`.
pub fn data_url(media: &str, bytes: &[u8]) -> String {
    format!("data:{media};base64,{}", STANDARD.encode(bytes))
}

static DATA_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)^data:([^;,]+);base64,(.+)$").expect("a valid pattern"));

/// A `data:<media>;base64,<payload>` URL's media type and payload (not decoded).
pub fn parse_data_url(text: &str) -> Option<(String, String)> {
    let captures = DATA_URL.captures(text)?;
    Some((captures[1].to_string(), captures[2].to_string()))
}

// ---------------------------------------------------------------------------------------------
// Media types and signatures

static MEDIA_TYPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9!#$&^_.+-]+/[a-z0-9!#$&^_.+-]+$").expect("a valid pattern")
});

/// A declared media type, normalized within `family` (`image`, `audio`, `video`): lowercase,
/// parameters after `;` dropped, trimmed; the aliases `image/jpg` → `image/jpeg`; `mp3`,
/// `audio/mp3`, `audio/mpeg3` → `audio/mpeg`; `wav`, `audio/x-wav`, `audio/wave` → `audio/wav`;
/// `mp4`, `video/x-m4v` → `video/mp4`. Anything that is not then `<family>/<subtype>` is refused:
/// `expected <family> media type, received <value>`.
pub fn normalize_media_type(value: &str, family: &str) -> Result<String, String> {
    let base = value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let normalized = match base.as_str() {
        "image/jpg" => "image/jpeg",
        "mp3" | "audio/mp3" | "audio/mpeg3" => "audio/mpeg",
        "wav" | "audio/x-wav" | "audio/wave" => "audio/wav",
        "mp4" | "video/x-m4v" => "video/mp4",
        other => other,
    }
    .to_string();
    if MEDIA_TYPE.is_match(&normalized) && normalized.starts_with(&format!("{family}/")) {
        Ok(normalized)
    } else {
        Err(format!("expected {family} media type, received {base}"))
    }
}

/// The image kind the bytes start like: PNG, JPEG, WebP or GIF.
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

/// The model kind the bytes start like: `glTF` → `model/gltf-binary`, `Kaydara FBX Binary` →
/// `model/fbx`.
pub fn sniff_model(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"glTF") {
        Some("model/gltf-binary")
    } else if bytes.starts_with(b"Kaydara FBX Binary") {
        Some("model/fbx")
    } else {
        None
    }
}

/// Whether the bytes carry `kind`'s signature: the four images of [`sniff_image`];
/// `audio/mpeg` (`ID3`, or an MPEG frame sync `FF Ex`); `audio/wav` (`RIFF????WAVE`);
/// `video/mp4` (`ftyp` at bytes 4..8, at least 12 bytes); `video/webm` (`1A 45 DF A3`);
/// the two models of [`sniff_model`]. Any other kind: false.
pub fn matches_signature(kind: &str, bytes: &[u8]) -> bool {
    match kind {
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" => sniff_image(bytes) == Some(kind),
        "audio/mpeg" => {
            bytes.starts_with(b"ID3")
                || (bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0)
        }
        "audio/wav" => bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "video/mp4" => bytes.len() >= 12 && &bytes[4..8] == b"ftyp",
        "video/webm" => bytes.starts_with(b"\x1a\x45\xdf\xa3"),
        "model/gltf-binary" | "model/fbx" => sniff_model(bytes) == Some(kind),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Costs

/// A reported cost in US dollars (spec/adapters.md §5): a JSON number that is finite and not
/// negative (booleans, strings and `null` are not costs), converted exactly from its JCS text and
/// **rounded up** to whole micro-dollars, so a cost is never under-counted (`1e-7` → `Usd(1)`).
pub fn usd_ceil(value: &Value) -> Option<Usd> {
    match value {
        Value::Number(n) => {
            let x = grida_fx_core::value::as_f64(n);
            if !x.is_finite() {
                return None;
            }
            decimal_micros_ceil(&grida_fx_core::value::format_number(x), 1_000_000)
        }
        _ => None,
    }
}

/// A non-negative decimal text (`12.5`, `1e-7`, `-0`) times `micros_per_unit`, rounded up to whole
/// micro-dollars. `None` for anything else, a negative value, or an amount too large for `Usd`.
/// Tripo's credits use `micros_per_unit = 10_000` (one credit is USD 0.01).
pub fn decimal_micros_ceil(text: &str, micros_per_unit: i64) -> Option<Usd> {
    static DECIMAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^([+-]?)(\d+)(?:\.(\d*))?(?:[eE]([+-]?\d+))?$|^([+-]?)\.(\d+)(?:[eE]([+-]?\d+))?$",
        )
        .expect("a valid pattern")
    });
    let text = text.trim();
    let captures = DECIMAL.captures(text)?;
    let (sign, whole, fraction, exponent) = if captures.get(2).is_some() {
        (
            captures.get(1).map_or("", |m| m.as_str()),
            captures.get(2).map_or("", |m| m.as_str()),
            captures.get(3).map_or("", |m| m.as_str()),
            captures.get(4).map_or("", |m| m.as_str()),
        )
    } else {
        (
            captures.get(5).map_or("", |m| m.as_str()),
            "0",
            captures.get(6).map_or("", |m| m.as_str()),
            captures.get(7).map_or("", |m| m.as_str()),
        )
    };
    let digits = format!("{whole}{fraction}");
    let digits = digits.trim_start_matches('0');
    let exponent: i64 = if exponent.is_empty() {
        0
    } else {
        exponent.parse().ok()?
    };
    // value = digits × 10^(exponent − len(fraction))
    let scale = exponent - fraction.len() as i64;
    if digits.is_empty() {
        return Some(Usd::ZERO);
    }
    if sign == "-" {
        return None;
    }
    if digits.len() > 30 || micros_per_unit <= 0 {
        return None;
    }
    let mantissa: i128 = digits.parse().ok()?;
    let product = mantissa.checked_mul(i128::from(micros_per_unit))?;
    let micros = if scale >= 0 {
        let factor = 10i128.checked_pow(u32::try_from(scale).ok()?)?;
        product.checked_mul(factor)?
    } else {
        let Ok(power) = u32::try_from(-scale) else {
            return Some(Usd(1));
        };
        match 10i128.checked_pow(power) {
            Some(divisor) => (product + divisor - 1) / divisor,
            None => 1,
        }
    };
    i64::try_from(micros).ok().map(Usd)
}

// ---------------------------------------------------------------------------------------------
// Request members and files

/// Whether a request member is absent: missing or `null` (spec/capabilities.md §1).
pub fn is_absent(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null))
}

/// The file a request member names (`{"file": "<digest>"}`), from the call's files. Refused with
/// `<what> has no bytes to send` when the value is not a file value or the digest is unknown.
pub fn request_file<'a>(
    call: &'a CallRequest,
    value: &Value,
    what: &str,
) -> Result<&'a RequestFile, String> {
    let digest = match value {
        Value::Object(map) if map.len() == 1 => map.get("file").and_then(Value::as_str),
        _ => None,
    };
    digest
        .and_then(|digest| call.files.get(digest))
        .ok_or_else(|| format!("{what} has no bytes to send"))
}

/// A request file's bytes, read from its store copy: `<what> has no bytes to send` when the copy
/// cannot be read or is empty.
pub fn read_file(file: &RequestFile, what: &str) -> Result<Vec<u8>, String> {
    match std::fs::read(&file.path) {
        Ok(bytes) if !bytes.is_empty() => Ok(bytes),
        _ => Err(format!("{what} has no bytes to send")),
    }
}

// ---------------------------------------------------------------------------------------------
// Provider options

/// Where a call's provider options come from, in the order they merge (spec/adapters.md §8): the
/// route's contract, then the request.
fn option_sources(call: &CallRequest) -> [(Option<&Value>, &'static str); 2] {
    [
        (
            call.route.contract.get("provider_options"),
            "contract.provider_options",
        ),
        (call.request.get("provider_options"), "provider_options"),
    ]
}

/// The parameters an adapter adds to what it sends (spec/adapters.md §8): the route's
/// `contract.provider_options.<provider>`, then the request's `provider_options.<provider>`, for
/// the route's provider; a request parameter replaces the route's of its name in place, and the
/// request's others follow. Absent, `null` and `{}` add nothing, and other providers' members are
/// not read. Refused, the route's before the request's: a member that is not an object (`<where>
/// is an object`); a member that is a file value (`<where> holds a file: provider options are JSON
/// only`); a parameter in `owned`, the members the adapter owns (`<where>.<name> is a member FX
/// owns`); a file value anywhere in a parameter (`<where>.<name> holds a file: …`).
pub fn provider_options(call: &CallRequest, owned: &[&str]) -> Result<Map<String, Value>, String> {
    let provider = call.route.provider.as_str();
    let mut merged = Map::new();
    for (options, at) in option_sources(call) {
        let parameters = match options {
            None | Some(Value::Null) => continue,
            Some(Value::Object(by_provider)) => match by_provider.get(provider) {
                None | Some(Value::Null) => continue,
                Some(member) if is_file(member) => {
                    return Err(format!(
                        "{at}.{provider} holds a file: provider options are JSON only"
                    ));
                }
                Some(Value::Object(parameters)) => parameters,
                Some(_) => return Err(format!("{at}.{provider} is an object")),
            },
            Some(_) => return Err(format!("{at} is an object")),
        };
        for (name, value) in parameters {
            if owned.contains(&name.as_str()) {
                return Err(format!("{at}.{provider}.{name} is a member FX owns"));
            }
            if holds_file(value) {
                return Err(format!(
                    "{at}.{provider}.{name} holds a file: provider options are JSON only"
                ));
            }
            merged.insert(name.clone(), value.clone());
        }
    }
    Ok(merged)
}

/// A runtime's refusal of provider options (spec/adapters.md §8): `<who> takes no
/// provider_options` when the route's or the request's member for the route's provider is anything
/// but absent, `null` or `{}`. Other providers' members are not read.
pub fn no_provider_options(call: &CallRequest, who: &str) -> Result<(), String> {
    let provider = call.route.provider.as_str();
    for (options, _) in option_sources(call) {
        let member = match options {
            None | Some(Value::Null) => continue,
            Some(Value::Object(by_provider)) => by_provider.get(provider),
            Some(other) => Some(other),
        };
        match member {
            None | Some(Value::Null) => {}
            Some(Value::Object(parameters)) if parameters.is_empty() => {}
            Some(_) => return Err(format!("{who} takes no provider_options")),
        }
    }
    Ok(())
}

/// Whether `value` is a file value as the engine reads one (spec/protocol.md §3.2): an object
/// whose only member, `file`, is a digest. `{"file": "a.png"}` is ordinary data (spec/identity.md
/// §3), so a provider parameter of that shape is passed on.
fn is_file(value: &Value) -> bool {
    value.as_object().is_some_and(|map| {
        map.len() == 1
            && map
                .get("file")
                .and_then(Value::as_str)
                .is_some_and(grida_fx_core::value::is_digest)
    })
}

/// Whether a file value ([`is_file`]) is anywhere in `value`.
fn holds_file(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(holds_file),
        Value::Object(members) => is_file(value) || members.values().any(holds_file),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Generic image requests

/// What a generic image adapter sends of an image request: its prompt, and its input pictures,
/// `image` then `references` (`references[<i>]`, from 0), each with its member name.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericImage<'a> {
    pub prompt: &'a str,
    pub pictures: Vec<(String, &'a Value)>,
}

/// The values a generic image adapter sends (src/openrouter/README.md, src/fal/README.md, "Images
/// on any model"), after the refusals of step 4 of spec/adapters.md §4 that are not provider
/// options, in this order: a blank prompt; a mask; `background: transparent`, then a background
/// other than `auto`, `opaque` or `transparent`; a `size` that is not `auto` or `WxH`
/// ([`parse_size`]), then an exact size; references on a generate unless `generate_references`
/// (`<label> takes no references`), and then on a route that does not declare `image_input`
/// (`<route> does not support image_input`, the plan's sentence, spec/capabilities.md §1); more
/// pictures than the route's `max_input_pictures`. What the
/// adapter cannot send faithfully is refused as `<who> sends no <mask | transparent background |
/// exact size>: give the model's own parameter in provider_options.<provider>`; `auto` and
/// `opaque`, which the model's default and the image check cover, are not sent.
pub fn generic_image<'a>(
    call: &'a CallRequest,
    who: &str,
    label: &str,
    generate_references: bool,
) -> Result<GenericImage<'a>, String> {
    let request = &call.request;
    let not_sent = |what: &str| {
        format!(
            "{who} sends no {what}: give the model's own parameter in provider_options.{}",
            call.route.provider
        )
    };
    let prompt = request.get("prompt").and_then(Value::as_str).unwrap_or("");
    if prompt.trim().is_empty() {
        return Err("an image call needs its prompt as text".into());
    }
    if !is_absent(request.get("mask")) {
        return Err(not_sent("mask"));
    }
    match request.get("background") {
        None | Some(Value::Null) => {}
        Some(Value::String(b)) if b == "auto" || b == "opaque" => {}
        Some(Value::String(b)) if b == "transparent" => {
            return Err(not_sent("transparent background"));
        }
        Some(_) => return Err("background must be auto, opaque or transparent".into()),
    }
    if parse_size(request.get("size"))?.is_some() {
        return Err(not_sent("exact size"));
    }
    let edit = call.route.capability == "image.edit";
    let references: &[Value] = request
        .get("references")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    if !edit && !references.is_empty() {
        if !generate_references {
            return Err(format!("{label} takes no references"));
        }
        if !call.route.limits.image_input {
            return Err(format!("{} does not support image_input", call.route.id()));
        }
    }
    let mut pictures = Vec::new();
    if edit && let Some(image) = request.get("image") {
        pictures.push(("image".to_string(), image));
    }
    pictures.extend(
        references
            .iter()
            .enumerate()
            .map(|(i, value)| (format!("references[{i}]"), value)),
    );
    if let Some(most) = call.route.limits.max_input_pictures
        && pictures.len() > most as usize
    {
        return Err(format!(
            "{} takes at most {most} input pictures, not {}",
            call.route.id(),
            pictures.len()
        ));
    }
    Ok(GenericImage { prompt, pictures })
}

/// `options` as multipart text fields, in order: a string as itself, any other value as its JSON
/// text (spec/adapters.md §8).
pub fn option_fields(options: &Map<String, Value>) -> Vec<crate::transport::Part> {
    options
        .iter()
        .map(|(name, value)| {
            let text = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            crate::transport::Part::text(name, &text)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Image sizes and checks

/// An image request's `size`: `None` for absent, `null` or `"auto"`; `Some((w, h))` for `"WxH"`
/// (positive integers without leading zeros). Refused: `size must be auto or WIDTHxHEIGHT`.
pub fn parse_size(value: Option<&Value>) -> Result<Option<(u32, u32)>, String> {
    static SIZE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([1-9]\d{0,5})x([1-9]\d{0,5})$").expect("a valid pattern"));
    let refused = || "size must be auto or WIDTHxHEIGHT".to_string();
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s == "auto" => Ok(None),
        Some(Value::String(s)) => {
            let captures = SIZE.captures(s).ok_or_else(refused)?;
            let w = captures[1].parse().map_err(|_| refused())?;
            let h = captures[2].parse().map_err(|_| refused())?;
            Ok(Some((w, h)))
        }
        Some(_) => Err(refused()),
    }
}

/// The GPT Image exact-size envelope: what [`check_size_envelope`] holds a size to.
pub struct SizeEnvelope {
    /// Each edge is a multiple of this.
    pub step: u32,
    /// The longest edge.
    pub max_edge: u32,
    /// The most the longer edge may be of the shorter.
    pub max_aspect: u32,
    /// The fewest pixels.
    pub min_pixels: u64,
    /// The most pixels.
    pub max_pixels: u64,
}

/// GPT Image's envelope (src/openai/README.md; src/fal/README.md).
pub const SIZE_ENVELOPE: SizeEnvelope = SizeEnvelope {
    step: 16,
    max_edge: 3840,
    max_aspect: 3,
    min_pixels: 655_360,
    max_pixels: 8_294_400,
};

/// The GPT Image exact-size envelope ([`SIZE_ENVELOPE`]), checked in this order: edges multiples
/// of 16; the longer edge at most 3840; aspect ratio at most 3:1; between 655 360 and 8 294 400
/// pixels. `label` starts each sentence (`OpenAI image size …`).
pub fn check_size_envelope(width: u32, height: u32, label: &str) -> Result<(), String> {
    let SizeEnvelope {
        step,
        max_edge,
        max_aspect,
        min_pixels,
        max_pixels,
    } = SIZE_ENVELOPE;
    let (long, short) = (width.max(height) as u64, width.min(height) as u64);
    if !width.is_multiple_of(step) || !height.is_multiple_of(step) {
        return Err(format!(
            "{label} image size edges must be multiples of {step}"
        ));
    }
    if long > max_edge as u64 {
        return Err(format!(
            "{label} image size edges must not exceed {max_edge} pixels"
        ));
    }
    if long > max_aspect as u64 * short {
        return Err(format!(
            "{label} image size aspect ratio must not exceed {max_aspect}:1"
        ));
    }
    let area = width as u64 * height as u64;
    if !(min_pixels..=max_pixels).contains(&area) {
        return Err(format!(
            "{label} image size must contain between {min_pixels} and {max_pixels} pixels"
        ));
    }
    Ok(())
}

/// The image checks after an answer (spec/adapters.md §3.4; each refusal fails the attempt as
/// billed), in this order:
/// 1. the kind is `image/png` and the bytes carry the PNG signature:
///    `the answer is <kind>, not image/png`;
/// 2. the bytes decode, every row of them ([`png_decodes`]): `the image data is not decodable`;
/// 3. an exact `size`: `the image is <w>x<h>, not <W>x<H>`;
/// 4. `background: transparent`: an alpha channel (the `has_alpha` fact), else `the picture
///    asked for as transparent has no alpha channel`, and a pixel that is not opaque (the
///    `opaque` fact false), else `the picture asked for as transparent has no transparent
///    pixels`;
/// 5. `background: opaque`: every pixel opaque (the `opaque` fact):
///    `the picture asked for as opaque has transparent pixels`.
pub fn check_image(
    kind: &str,
    bytes: &[u8],
    size: Option<(u32, u32)>,
    background: &str,
) -> Result<(), String> {
    check_png(kind, bytes)?;
    let facts = match grida_fx_core::facts::image_facts(bytes, "image/png") {
        Ok(Some(facts)) if png_decodes(bytes) => facts,
        _ => return Err("the image data is not decodable".into()),
    };
    if let Some((w, h)) = size
        && (facts.width, facts.height) != (w, h)
    {
        return Err(format!(
            "the image is {}x{}, not {w}x{h}",
            facts.width, facts.height
        ));
    }
    match background {
        "transparent" if !facts.has_alpha => {
            Err("the picture asked for as transparent has no alpha channel".into())
        }
        "transparent" if facts.opaque => {
            Err("the picture asked for as transparent has no transparent pixels".into())
        }
        "opaque" if !facts.opaque => {
            Err("the picture asked for as opaque has transparent pixels".into())
        }
        _ => Ok(()),
    }
}

/// The first image check ([`check_image`] step 1; spec/capabilities.md §2 check 1, §12): the
/// kind is `image/png` and the bytes carry the PNG signature, else `the answer is <kind>, not
/// image/png`, where a file whose kind says PNG shows the kind its bytes start like (`not an
/// image` when none).
pub fn check_png(kind: &str, bytes: &[u8]) -> Result<(), String> {
    if kind == "image/png" && matches_signature("image/png", bytes) {
        return Ok(());
    }
    let found = if kind == "image/png" {
        sniff_image(bytes).unwrap_or("not an image")
    } else {
        kind
    };
    Err(format!("the answer is {found}, not image/png"))
}

/// A picture as PNG (spec/capabilities.md §1): a PNG as it is, any other picture the decoder reads
/// (JPEG, WebP, GIF: the first frame) decoded and encoded as PNG, the same pixels. `subject` names
/// the picture in the refusals: `<subject> is not decodable`, then [`not_encoded`]. Decoding a
/// large picture takes a while: an adapter calls it off the async threads ([`to_png_blocking`]).
pub fn to_png(bytes: Vec<u8>, subject: &str) -> Result<Vec<u8>, String> {
    if sniff_image(&bytes) == Some("image/png") {
        return Ok(bytes);
    }
    let picture =
        image::load_from_memory(&bytes).map_err(|_| format!("{subject} is not decodable"))?;
    let mut png = Vec::new();
    picture
        .write_with_encoder(image::codecs::png::PngEncoder::new(&mut png))
        .map_err(|_| not_encoded(subject))?;
    Ok(png)
}

/// `<subject> could not be encoded as PNG`.
pub fn not_encoded(subject: &str) -> String {
    format!("{subject} could not be encoded as PNG")
}

/// [`to_png`] on a blocking thread.
pub async fn to_png_blocking(bytes: Vec<u8>, subject: &'static str) -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(move || to_png(bytes, subject))
        .await
        .unwrap_or_else(|_| Err(not_encoded(subject)))
}

/// Whether a PNG's pixel data decodes in full. The file facts read only the header of a picture
/// without alpha, so pixel data cut short would pass them; this decodes every row.
pub fn png_decodes(bytes: &[u8]) -> bool {
    use image::ImageDecoder as _;
    let Ok(decoder) = image::codecs::png::PngDecoder::new(std::io::Cursor::new(bytes)) else {
        return false;
    };
    let Ok(len) = usize::try_from(decoder.total_bytes()) else {
        return false;
    };
    let mut pixels = vec![0u8; len];
    decoder.read_image(&mut pixels).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Secret;
    use serde_json::json;

    #[test]
    fn base_urls_normalize_or_refuse() {
        assert_eq!(
            normalize_base_url(" https://api.example.test/v1/ ", "Acme").unwrap(),
            "https://api.example.test/v1"
        );
        assert_eq!(
            normalize_base_url("http://localhost:8080", "Acme").unwrap(),
            "http://localhost:8080"
        );
        assert!(normalize_base_url("http://127.0.0.1:9/v1", "Acme").is_ok());
        assert!(normalize_base_url("http://[::1]/v1", "Acme").is_ok());
        assert_eq!(
            normalize_base_url("http://api.example.test", "Acme").unwrap_err(),
            "Acme base_url must use HTTPS unless it targets a loopback host"
        );
        for bad in [
            "https://user:pw@api.example.test",
            "https://api.example.test/v1?x=1",
            "https://api.example.test/v1#f",
            "ftp://api.example.test",
            "not a url",
        ] {
            assert_eq!(
                normalize_base_url(bad, "Acme").unwrap_err(),
                "Acme base_url must be an HTTP(S) URL without credentials, query, or fragment",
                "{bad}"
            );
        }
        assert_eq!(
            normalize_base_url("https://api.example.test:99999", "Acme").unwrap_err(),
            "Acme base_url must use a valid network port"
        );
    }

    #[test]
    fn downloads_are_held_to_the_declared_hosts() {
        let hosts = ["cdn.acme.test".to_string(), "*.media.acme.test".to_string()];
        for good in [
            "https://cdn.acme.test/a.png",
            "https://CDN.Acme.Test./a.png?sig=1",
            "https://cdn.acme.test:443/a.png",
            "https://v1.media.acme.test/a.png",
            "https://a.b.media.acme.test/a.png",
        ] {
            assert!(download_url(good, &hosts).is_ok(), "{good}");
        }
        for bad in [
            "http://cdn.acme.test/a.png",
            "https://media.acme.test/a.png",
            "https://evilmedia.acme.test/a.png",
            "https://media.acme.test.evil.example/a.png",
            "https://other.acme.test/a.png",
            "https://user:pw@cdn.acme.test/a.png",
            "https://cdn.acme.test:8443/a.png",
            "https://cdn.acme.test/a.png#frag",
            "https://127.0.0.1/a.png",
            "https://[::1]/a.png",
            "data:image/png;base64,aGVsbG8=",
        ] {
            assert_eq!(download_url(bad, &hosts), Err(OffHost::Outside), "{bad}");
        }
        for malformed in ["not a url", "", "https://cdn.acme.test:99999/a.png"] {
            assert_eq!(
                download_url(malformed, &hosts),
                Err(OffHost::Malformed),
                "{malformed}"
            );
        }
        assert_eq!(
            download_url("https://cdn.acme.test/a.png", &[]),
            Err(OffHost::Outside)
        );
    }

    #[test]
    fn json_bodies() {
        assert!(json_object(br#"{"a": 1}"#, "X").is_ok());
        assert_eq!(
            json_object(b"[1]", "X").unwrap_err(),
            "X returned a non-object JSON response"
        );
        assert_eq!(
            json_object(b"<html>", "X").unwrap_err(),
            "X returned invalid JSON"
        );
        assert_eq!(status_reason("X", 500, None), "X returned HTTP 500");
        assert_eq!(
            status_reason("X", 400, Some("code=y")),
            "X returned HTTP 400: code=y"
        );
    }

    #[test]
    fn safe_details_keep_only_allowlisted_fields() {
        let redactor = Redactor::new(vec![Secret::new("openai-secret")]);
        let body = json!({"error": {
            "message": "unsupported parameter contains openai-secret",
            "type": "invalid_request_error", "code": "invalid_value", "param": "size"}});
        assert_eq!(
            safe_error_detail(body.to_string().as_bytes(), &redactor).unwrap(),
            "message=unsupported parameter; type=invalid_request_error; code=invalid_value; param=size"
        );
        let raw = json!({"error": {"message": "Provider returned error", "code": 400,
            "metadata": {"raw": json!({"error": {"message": "Invalid schema: required must include every key",
                "code": "invalid_json_schema"}}).to_string()}}});
        assert_eq!(
            safe_error_detail(raw.to_string().as_bytes(), &redactor).unwrap(),
            "message=invalid schema, required must include every key; code=invalid_json_schema"
        );
        assert_eq!(safe_error_detail(br#"{"detail": "x"}"#, &redactor), None);
        let leaky = json!({"error": {"code": "has spaces and openai-secret"}});
        assert_eq!(
            safe_error_detail(leaky.to_string().as_bytes(), &redactor),
            None
        );
    }

    #[test]
    fn strict_base64_refuses_sloppy_text() {
        assert_eq!(strict_base64("aGVsbG8="), Some(b"hello".to_vec()));
        for bad in [
            "",
            "aGVsbG8",
            "aGVs bG8=",
            "aGVsbG8-",
            "=aGVsbG8",
            "aG=sbG8=",
            "not-base64!",
        ] {
            assert_eq!(strict_base64(bad), None, "{bad}");
        }
        let url = data_url("image/png", b"hello");
        assert_eq!(url, "data:image/png;base64,aGVsbG8=");
        assert_eq!(
            parse_data_url(&url),
            Some(("image/png".into(), "aGVsbG8=".into()))
        );
        assert_eq!(parse_data_url("https://x.test/a.png"), None);
    }

    #[test]
    fn media_types_normalize_within_a_family() {
        assert_eq!(
            normalize_media_type("Image/JPG; q=1", "image").unwrap(),
            "image/jpeg"
        );
        assert_eq!(normalize_media_type("mp3", "audio").unwrap(), "audio/mpeg");
        assert_eq!(
            normalize_media_type("audio/x-wav", "audio").unwrap(),
            "audio/wav"
        );
        assert_eq!(
            normalize_media_type("video/x-m4v", "video").unwrap(),
            "video/mp4"
        );
        assert_eq!(
            normalize_media_type("application/octet-stream", "image").unwrap_err(),
            "expected image media type, received application/octet-stream"
        );
        assert!(normalize_media_type("", "audio").is_err());
    }

    #[test]
    fn signatures() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff_image(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image(b"GIF89a"), Some("image/gif"));
        assert!(matches_signature("audio/mpeg", b"ID3\x04"));
        assert!(matches_signature("audio/mpeg", b"\xff\xfb\x90\x00"));
        assert!(!matches_signature("audio/mpeg", b"not audio"));
        assert!(matches_signature("audio/wav", b"RIFF\0\0\0\0WAVEfmt "));
        assert!(matches_signature(
            "video/mp4",
            b"\0\0\0\x18ftypmp42\0\0\0\0"
        ));
        assert_eq!(sniff_model(b"glTF\x02\0\0\0"), Some("model/gltf-binary"));
        assert_eq!(sniff_model(b"Kaydara FBX Binary  \0"), Some("model/fbx"));
        assert_eq!(sniff_model(b"PK\x03\x04"), None);
    }

    #[test]
    fn reported_costs_round_up() {
        assert_eq!(usd_ceil(&json!(0)), Some(Usd(0)));
        assert_eq!(usd_ceil(&json!(0.210835)), Some(Usd(210_835)));
        assert_eq!(usd_ceil(&json!(0.0000001)), Some(Usd(1)));
        assert_eq!(usd_ceil(&json!(0.00012345)), Some(Usd(124)));
        assert_eq!(usd_ceil(&json!(0.4)), Some(Usd(400_000)));
        for not_a_cost in [
            json!(true),
            json!(-1),
            json!("0.10"),
            Value::Null,
            json!({}),
        ] {
            assert_eq!(usd_ceil(&not_a_cost), None, "{not_a_cost}");
        }
        assert_eq!(decimal_micros_ceil("125", 10_000), Some(Usd(1_250_000)));
        assert_eq!(decimal_micros_ceil("12.5", 10_000), Some(Usd(125_000)));
        assert_eq!(decimal_micros_ceil(" 125 ", 10_000), Some(Usd(1_250_000)));
        assert_eq!(decimal_micros_ceil("0.00015", 10_000), Some(Usd(2)));
        assert_eq!(decimal_micros_ceil("0.00001", 10_000), Some(Usd(1)));
        assert_eq!(decimal_micros_ceil("-0", 10_000), Some(Usd(0)));
        assert_eq!(decimal_micros_ceil("1e-30", 1_000_000), Some(Usd(1)));
        assert_eq!(decimal_micros_ceil("-1", 10_000), None);
        assert_eq!(decimal_micros_ceil("abc", 10_000), None);
        assert_eq!(decimal_micros_ceil("1e40", 1_000_000), None);
    }

    #[test]
    fn sizes_and_their_envelope() {
        assert_eq!(parse_size(None), Ok(None));
        assert_eq!(parse_size(Some(&Value::Null)), Ok(None));
        assert_eq!(parse_size(Some(&json!("auto"))), Ok(None));
        assert_eq!(
            parse_size(Some(&json!("1536x1024"))),
            Ok(Some((1536, 1024)))
        );
        for bad in [json!("1024"), json!(1024), json!("01x10"), json!("x")] {
            assert_eq!(
                parse_size(Some(&bad)).unwrap_err(),
                "size must be auto or WIDTHxHEIGHT"
            );
        }
        let refused = |w, h| check_size_envelope(w, h, "OpenAI").unwrap_err();
        assert!(refused(1000, 1024).contains("multiples of 16"));
        assert!(refused(256, 1024).contains("aspect ratio"));
        assert!(refused(3856, 1024).contains("3840"));
        assert!(refused(3840, 2176).contains("between"));
        assert!(refused(256, 768).contains("between"));
        assert!(check_size_envelope(1024, 1024, "OpenAI").is_ok());
        assert!(check_size_envelope(3840, 2160, "OpenAI").is_ok());
    }

    #[test]
    fn bodies_are_measured_as_sent() {
        let body = json!({"model": "m", "prompt": "é"});
        let sent = serde_json::to_vec(&body).unwrap().len();
        assert!(!body_exceeds(&body, sent));
        assert!(body_exceeds(&body, sent - 1));
        assert!(body_exceeds(&json!({"prompt": "x".repeat(4096)}), 1024));
        // {"a":"xxxx"} is 12 bytes.
        assert!(!body_exceeds(&json!({"a": "xxxx"}), 12));
        assert!(body_exceeds(&json!({"a": "xxxx"}), 11));
    }

    fn option_call(contract: Value, request: Value) -> crate::testing::TestCall {
        crate::testing::CallBuilder::new("image.generate", "img-a@acme")
            .contract(contract)
            .request(request)
            .build()
    }

    #[test]
    fn provider_options_merge_the_route_s_under_the_request_s() {
        let options = |contract: Value, request: Value| {
            provider_options(&option_call(contract, request), &["prompt"])
        };
        // The route's first, in order; a request member replaces it in place, then the request's
        // others; other providers' members are not read.
        let merged = options(
            json!({"provider_options": {"acme": {"b": 1, "a": {"x": 1}}, "other": {"c": 3}}}),
            json!({"provider_options": {"acme": {"z": [1], "a": {"y": 2}}, "other": "x"}}),
        )
        .unwrap();
        assert_eq!(
            Value::Object(merged).to_string(),
            r#"{"b":1,"a":{"y":2},"z":[1]}"#
        );
        // Absent, null and {} add nothing; a parameter's null is passed on.
        for (contract, request) in [
            (json!({}), json!({})),
            (json!(null), json!({"provider_options": null})),
            (
                json!({"provider_options": null}),
                json!({"provider_options": {"acme": null}}),
            ),
            (
                json!({"provider_options": {"acme": {}}}),
                json!({"provider_options": {}}),
            ),
        ] {
            assert_eq!(options(contract, request), Ok(Map::new()));
        }
        let kept = options(
            json!({}),
            json!({"provider_options": {"acme": {"seed": null}}}),
        );
        assert_eq!(Value::Object(kept.unwrap()), json!({"seed": null}));
    }

    #[test]
    fn provider_options_are_refused_route_first() {
        let options = |contract: Value, request: Value| {
            provider_options(&option_call(contract, request), &["prompt", "n"])
        };
        let file = json!({"file": "a".repeat(64)});
        let cases = [
            (
                json!({"provider_options": []}),
                json!({}),
                "contract.provider_options is an object",
            ),
            (
                json!({"provider_options": {"acme": 1}}),
                json!({"provider_options": {"acme": 2}}),
                "contract.provider_options.acme is an object",
            ),
            (
                json!({}),
                json!({"provider_options": {"acme": "x"}}),
                "provider_options.acme is an object",
            ),
            (
                json!({"provider_options": {"acme": {"n": 2}}}),
                json!({"provider_options": {"acme": {"prompt": "p"}}}),
                "contract.provider_options.acme.n is a member FX owns",
            ),
            (
                json!({}),
                json!({"provider_options": {"acme": {"seed": 1, "prompt": "p"}}}),
                "provider_options.acme.prompt is a member FX owns",
            ),
            (
                json!({}),
                json!({"provider_options": {"acme": {"mask": {"layers": [file]}}}}),
                "provider_options.acme.mask holds a file: provider options are JSON only",
            ),
            (
                json!({"provider_options": {"acme": {"init": file}}}),
                json!({}),
                "contract.provider_options.acme.init holds a file: provider options are JSON only",
            ),
            // A member that is itself a file is not a parameter named `file`.
            (
                json!({"provider_options": {"acme": file}}),
                json!({}),
                "contract.provider_options.acme holds a file: provider options are JSON only",
            ),
            (
                json!({"provider_options": {"acme": {"seed": 1}}}),
                json!({"provider_options": {"acme": file}}),
                "provider_options.acme holds a file: provider options are JSON only",
            ),
        ];
        for (contract, request, refusal) in cases {
            assert_eq!(options(contract, request), Err(refusal.to_string()));
        }
        // Another provider's member is never read, so never refused.
        assert!(
            options(
                json!({"provider_options": {"other": {"prompt": file.clone()}}}),
                json!({"provider_options": {"other": 1}}),
            )
            .is_ok()
        );
    }

    #[test]
    fn provider_options_pass_on_data_shaped_like_a_file() {
        // Only a digest makes a file value (spec/identity.md §3): the rest is the provider's.
        for not_a_file in [
            json!({"file": "a.png"}),
            json!({"file": "A".repeat(64)}),
            json!({"file": "a".repeat(63)}),
            json!({"file": 1}),
        ] {
            let request = json!({"provider_options": {"acme": {
                "ref": not_a_file,
                "nested": {"layers": [not_a_file]},
            }}});
            let merged = provider_options(&option_call(json!({}), request.clone()), &["prompt"]);
            assert_eq!(
                Value::Object(merged.unwrap()),
                request["provider_options"]["acme"],
                "{not_a_file}"
            );
            let member = json!({"provider_options": {"acme": not_a_file}});
            let merged = provider_options(&option_call(member, json!({})), &["prompt"]);
            assert_eq!(Value::Object(merged.unwrap()), not_a_file);
        }
    }

    #[test]
    fn a_runtime_refuses_its_own_provider_options_only() {
        let refused = |contract: Value, request: Value| {
            no_provider_options(&option_call(contract, request), "Acme")
        };
        assert_eq!(refused(json!({}), json!({})), Ok(()));
        assert_eq!(
            refused(
                json!({"provider_options": {"acme": {}, "other": {"a": 1}}}),
                json!({"provider_options": {"acme": null, "other": {"a": 1}}}),
            ),
            Ok(())
        );
        for (contract, request) in [
            (json!({"provider_options": {"acme": {"a": 1}}}), json!({})),
            (json!({}), json!({"provider_options": {"acme": {"a": 1}}})),
            (json!({}), json!({"provider_options": {"acme": 1}})),
            (json!({"provider_options": "x"}), json!({})),
        ] {
            assert_eq!(
                refused(contract, request),
                Err("Acme takes no provider_options".to_string())
            );
        }
    }

    #[test]
    fn option_fields_are_text() {
        let options = json!({"style": "ink", "steps": 4, "extra": {"a": [1, "b"]}});
        let fields: Vec<(String, String)> = option_fields(options.as_object().unwrap())
            .into_iter()
            .map(|part| {
                (
                    part.name.clone(),
                    String::from_utf8(part.data.clone()).unwrap(),
                )
            })
            .collect();
        assert_eq!(
            fields,
            vec![
                ("style".to_string(), "ink".to_string()),
                ("steps".to_string(), "4".to_string()),
                ("extra".to_string(), r#"{"a":[1,"b"]}"#.to_string()),
            ]
        );
    }

    fn generic(capability: &str, request: Value, most: Option<u32>) -> Result<Vec<String>, String> {
        let mut call = crate::testing::CallBuilder::new(capability, "acme/img-a@acme")
            .request(request)
            .build();
        call.call.route.limits.max_input_pictures = most;
        generic_image(
            &call,
            "Acme's generic image adapter",
            "Acme image generation",
            false,
        )
        .map(|image| {
            image
                .pictures
                .into_iter()
                .map(|(member, _)| member)
                .collect()
        })
    }

    #[test]
    fn a_generic_image_sends_its_prompt_and_pictures_and_refuses_what_it_cannot_send() {
        let picture = json!({"file": "a".repeat(64)});
        let edit = json!({"prompt": "p", "image": picture, "references": [picture, picture],
                          "size": "auto", "background": "opaque"});
        assert_eq!(
            generic("image.edit", edit.clone(), Some(3)),
            Ok(vec![
                "image".into(),
                "references[0]".into(),
                "references[1]".into()
            ])
        );
        assert_eq!(
            generic(
                "image.generate",
                json!({"prompt": "p", "background": null}),
                None
            ),
            Ok(Vec::new())
        );
        let hint = "give the model's own parameter in provider_options.acme";
        let refused =
            |capability: &str, request: Value| generic(capability, request, Some(2)).unwrap_err();
        for (request, sentence) in [
            (
                json!({"prompt": " "}),
                "an image call needs its prompt as text".to_string(),
            ),
            (
                json!({"prompt": "p", "mask": picture}),
                format!("Acme's generic image adapter sends no mask: {hint}"),
            ),
            (
                json!({"prompt": "p", "background": "transparent"}),
                format!("Acme's generic image adapter sends no transparent background: {hint}"),
            ),
            (
                json!({"prompt": "p", "background": "clear"}),
                "background must be auto, opaque or transparent".to_string(),
            ),
            (
                json!({"prompt": "p", "size": "1K"}),
                "size must be auto or WIDTHxHEIGHT".to_string(),
            ),
            (
                json!({"prompt": "p", "size": "1024x1024"}),
                format!("Acme's generic image adapter sends no exact size: {hint}"),
            ),
            (
                json!({"prompt": "p", "references": [picture]}),
                "Acme image generation takes no references".to_string(),
            ),
        ] {
            assert_eq!(
                refused("image.generate", request.clone()),
                sentence,
                "{request}"
            );
        }
        assert_eq!(
            refused("image.edit", edit),
            "acme/img-a@acme takes at most 2 input pictures, not 3"
        );
    }

    #[test]
    fn a_generate_s_references_need_a_route_that_declares_image_input() {
        let picture = json!({"file": "a".repeat(64)});
        let request = json!({"prompt": "p", "references": [picture]});
        let mut call = crate::testing::CallBuilder::new("image.generate", "acme/img-a@acme")
            .request(request)
            .build();
        let sent = |call: &crate::testing::TestCall| {
            generic_image(call, "Acme's generic image adapter", "Acme", true)
                .map(|image| image.pictures.len())
        };
        assert_eq!(
            sent(&call),
            Err("acme/img-a@acme does not support image_input".into())
        );
        call.call.route.limits.image_input = true;
        assert_eq!(sent(&call), Ok(1));
    }

    #[test]
    fn a_picture_becomes_a_png_of_the_same_pixels() {
        let png = to_png(crate::testing::media::jpeg(8, 4), "Acme's image").unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (8, 4));
        assert!(!decoded.color().has_alpha());
        let already = crate::testing::media::png(2, 2, None);
        assert_eq!(to_png(already.clone(), "Acme's image").unwrap(), already);
        assert_eq!(
            to_png(b"not an image".to_vec(), "Acme's image"),
            Err("Acme's image is not decodable".into())
        );
        assert_eq!(
            not_encoded("Acme's image"),
            "Acme's image could not be encoded as PNG"
        );
        let blocking = crate::testing::block_on(to_png_blocking(
            crate::testing::media::jpeg(2, 2),
            "Acme's image",
        ));
        assert!(blocking.unwrap().starts_with(b"\x89PNG"));
    }
}
