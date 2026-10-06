//! Wire helpers every adapter shares (spec/providers.md §4–§6): base URLs, JSON bodies, strict
//! base64 and data URLs, media types and byte signatures, reported costs, safe error details,
//! request files, image sizes and the image checks.
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
// Base URLs

/// Normalizes a provider base URL (spec/providers.md §3): trimmed, trailing `/` removed; an
/// `http`/`https` URL with a host, no userinfo, no query, no fragment and a valid port; `http`
/// only for a loopback host (`localhost`, or a loopback IP). `label` names the provider in the
/// refusal (`OpenAI base_url must …`).
pub fn normalize_base_url(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim().trim_end_matches('/');
    let shape = || {
        format!("{label} base_url must be an HTTP(S) URL without credentials, query, or fragment")
    };
    let url = match url::Url::parse(trimmed) {
        Ok(url) => url,
        Err(url::ParseError::InvalidPort) => {
            return Err(format!("{label} base_url must use a valid network port"));
        }
        Err(_) => return Err(shape()),
    };
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || trimmed.contains('?')
        || trimmed.contains('#')
    {
        return Err(shape());
    }
    if url.scheme() == "http" && !is_loopback(&url) {
        return Err(format!(
            "{label} base_url must use HTTPS unless it targets a loopback host"
        ));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain
            .trim_end_matches('.')
            .eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
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
/// OpenRouter use (spec/providers.md §8):
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

/// A reported cost in US dollars (spec/providers.md §6): a JSON number that is finite and not
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

/// The GPT Image exact-size envelope (spec/providers.md §9.1, §9.3), checked in this order: edges
/// multiples of 16; the longer edge at most 3840; aspect ratio at most 3:1; between 655 360 and
/// 8 294 400 pixels. `label` starts each sentence (`OpenAI image size …`).
pub fn check_size_envelope(width: u32, height: u32, label: &str) -> Result<(), String> {
    let (long, short) = (width.max(height) as u64, width.min(height) as u64);
    if !width.is_multiple_of(16) || !height.is_multiple_of(16) {
        return Err(format!("{label} image size edges must be multiples of 16"));
    }
    if long > 3840 {
        return Err(format!(
            "{label} image size edges must not exceed 3840 pixels"
        ));
    }
    if long > 3 * short {
        return Err(format!(
            "{label} image size aspect ratio must not exceed 3:1"
        ));
    }
    let area = width as u64 * height as u64;
    if !(655_360..=8_294_400).contains(&area) {
        return Err(format!(
            "{label} image size must contain between 655360 and 8294400 pixels"
        ));
    }
    Ok(())
}

/// The image checks after an answer (spec/providers.md §4.4; each refusal fails the attempt as
/// billed), in this order:
/// 1. the kind is `image/png` and the bytes carry the PNG signature:
///    `the answer is <kind>, not image/png`;
/// 2. the bytes decode, every row of them ([`png_decodes`]): `the image data is not decodable`;
/// 3. an exact `size`: `the image is <w>x<h>, not <W>x<H>`;
/// 4. `background: transparent`: an alpha channel (the `has_alpha` fact):
///    `the picture asked for as transparent has no alpha channel`;
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
}
