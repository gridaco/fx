//! FX values, canonical JSON and digests ([identity.md](../../../spec/identity.md) §1–§3, §5).
//!
//! Values are `serde_json::Value` with insertion order kept. Every number FX reads or computes is
//! normalized by [`number`]: an integral number within ±2^53 is stored as an integer, anything
//! else as a double, so two values that mean the same number compare equal.

use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::fmt;

/// The largest integer every double represents exactly, 2^53.
const EXACT: f64 = 9_007_199_254_740_992.0;

/// Why a value was refused. `code` names the rule, as the vectors in `spec/vectors/` do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub code: &'static str,
    pub message: String,
}

impl Refused {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refused {}

/// The FX number for a double. NaN and the infinities are not values.
pub fn number(x: f64) -> Result<Value, Refused> {
    if !x.is_finite() {
        return Err(Refused::new(
            "non_finite_number",
            format!("{x} is not a number FX can hold"),
        ));
    }
    if x.fract() == 0.0 && x.abs() <= EXACT {
        // -0.0 becomes 0: one number type, one zero.
        return Ok(Value::Number(Number::from(x as i64)));
    }
    Ok(Value::Number(Number::from_f64(x).expect("finite")))
}

/// A number as a double.
pub fn as_f64(n: &Number) -> f64 {
    if let Some(i) = n.as_i64() {
        i as f64
    } else if let Some(u) = n.as_u64() {
        u as f64
    } else {
        n.as_f64().expect("a JSON number")
    }
}

/// Reads an integer literal (no fraction, no exponent, an optional sign). The literal must be the
/// canonical form of the number it reads as, apart from the sign of zero and a leading `+`, so
/// FX always reads back what it writes: `1152921504606847000` is accepted (it reads as 2^60, whose
/// canonical form it is). Any other literal is refused, with code `integer_out_of_range`, for one
/// of two reasons:
/// - it is exact but written another way (`007`; `1152921504606846976`, which is 2^60 written
///   out): the message gives the canonical form to write;
/// - reading it rounds it (`9007199254740993`): the message says the number is beyond the
///   integers a number holds exactly.
pub fn integer_literal(text: &str) -> Result<Value, Refused> {
    let refused = |message: String| Refused::new("integer_out_of_range", message);
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(refused(format!(
            "{} is not an integer literal",
            shorten(text)
        )));
    }
    let beyond = || {
        refused(format!(
            "{} is beyond the integers a number holds exactly (2^53 = 9007199254740992); \
             quote it if it is an identifier, and give its input the string type",
            shorten(text)
        ))
    };
    // The integer the literal means, without leading zeros. A double's integers have at most 309
    // digits, so a longer one is beyond them however it parses.
    let significant = match digits.trim_start_matches('0') {
        "" => "0",
        significant => significant,
    };
    if significant.len() > 309 {
        return Err(beyond());
    }
    let x: f64 = text.parse().map_err(|_| beyond())?;
    if !x.is_finite() {
        return Err(beyond());
    }
    let canonical = format_number(x);
    let written = text.strip_prefix('+').unwrap_or(text);
    if written == canonical || (significant == "0" && digits == "0") {
        return number(x);
    }
    // `{:.0}` writes a double's exact decimal value, and an integer literal reads as an integral
    // double: equal digits mean the literal was exact, only not written canonically.
    if format!("{:.0}", x.abs()) != significant {
        return Err(beyond());
    }
    Err(refused(if canonical.contains('e') {
        // 10^21 and up: no integer literal is canonical, and an expression has no exponent.
        format!(
            "{} is not written in canonical form; it is the number {canonical}, so write it \
             with a fraction or an exponent",
            shorten(text)
        )
    } else {
        format!(
            "{} is not written in canonical form; write {canonical}",
            shorten(text)
        )
    }))
}

/// At most 40 characters of `text`, and `…` when there were more.
fn shorten(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(40).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// ECMAScript `Number.prototype.toString` of a finite double, which is how JCS writes numbers.
pub fn format_number(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let mut buffer = ryu_js::Buffer::new();
    buffer.format_finite(x).to_string()
}

fn number_text(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return format_number(u as f64);
    }
    format_number(n.as_f64().expect("a JSON number"))
}

fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn sorted(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by(|a, b| utf16_cmp(a.0, b.0));
    entries
}

/// RFC 8785 canonical JSON.
pub fn canon(value: &Value) -> String {
    let mut out = String::new();
    write_canon(&mut out, value);
    out
}

fn write_canon(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number_text(n)),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canon(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in sorted(map).into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                write_canon(out, item);
            }
            out.push('}');
        }
    }
}

/// A JSON value as FX writes it to a file: the canonical form spread over lines
/// ([identity.md](../../../spec/identity.md) §5, "Writing JSON").
pub fn write_json(value: &Value) -> String {
    let mut out = String::new();
    write_pretty(&mut out, value, 0);
    out
}

fn write_pretty(out: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Array(items) if !items.is_empty() => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, depth + 1);
                write_pretty(out, item, depth + 1);
            }
            newline(out, depth);
            out.push(']');
        }
        Value::Object(map) if !map.is_empty() => {
            out.push('{');
            for (i, (key, item)) in sorted(map).into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, depth + 1);
                write_string(out, key);
                out.push_str(": ");
                write_pretty(out, item, depth + 1);
            }
            newline(out, depth);
            out.push('}');
        }
        other => write_canon(out, other),
    }
}

fn newline(out: &mut String, depth: usize) {
    out.push('\n');
    out.extend(std::iter::repeat_n(' ', depth));
}

/// Lowercase hex SHA-256 of some bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in hash {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// `digest(v)`: the SHA-256 of the canonical form.
pub fn digest(value: &Value) -> String {
    sha256_hex(canon(value).as_bytes())
}

/// `file_digest(f)`: the SHA-256 of a file's raw bytes.
pub fn file_digest(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

/// Whether a string is a 64-character lowercase hex digest.
pub fn is_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The reserved-marker rule ([identity.md](../../../spec/identity.md) §3): user data may not hold
/// an object that looks like a runtime marker. `where_` names the value in the message.
pub fn check_markers(value: &Value, where_: &str) -> Result<(), Refused> {
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                check_markers(item, &format!("{where_}[{i}]"))?;
            }
            Ok(())
        }
        Value::Object(map) => {
            if map.len() == 1 {
                let (key, item) = map.iter().next().expect("one member");
                let reserved = match key.as_str() {
                    "file" => item.as_str().is_some_and(is_digest),
                    "missing" => item == &Value::Bool(true),
                    "failed" => item.is_string(),
                    "collection" => item.is_array(),
                    "pending" => true,
                    _ => false,
                };
                if reserved {
                    return Err(Refused::new(
                        "reserved_marker",
                        format!(
                            "{where_}: an object holding only `{key}` in this shape is reserved for FX's own values"
                        ),
                    ));
                }
            }
            for (key, item) in map {
                check_markers(item, &format!("{where_}.{key}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Reads JSON text strictly: I-JSON values only, no duplicate keys, no lone surrogates, numbers
/// normalized. Leading and trailing whitespace is allowed; anything else after the value is not.
pub fn parse_json(text: &str) -> Result<Value, Refused> {
    let mut reader = JsonReader {
        text: text.as_bytes(),
        at: 0,
        src: text,
    };
    reader.skip_space();
    let value = reader.value(0)?;
    reader.skip_space();
    if reader.at != reader.text.len() {
        return Err(reader.error("invalid_json", "text after the JSON value"));
    }
    Ok(value)
}

struct JsonReader<'a> {
    text: &'a [u8],
    src: &'a str,
    at: usize,
}

/// The deepest a value may nest: every reader ([`parse_json`], the YAML loader) refuses a value
/// inside more than this many arrays and objects, so the code that walks a value recursively
/// ([`check_markers`], [`canon`], [`write_json`], cloning, dropping) never goes deeper.
pub const MAX_DEPTH: usize = 512;

impl JsonReader<'_> {
    /// A refusal at the current position. `at` is a byte offset and may be inside a character
    /// (after a backslash, before the character it escapes), so lines are counted over bytes.
    fn error(&self, code: &'static str, what: &str) -> Refused {
        let before = &self.text[..self.at.min(self.text.len())];
        let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
        Refused::new(code, format!("line {line}: {what}"))
    }

    fn skip_space(&mut self) {
        while let Some(b) = self.text.get(self.at) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.at += 1;
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, literal: &str) -> bool {
        if self.text[self.at..].starts_with(literal.as_bytes()) {
            self.at += literal.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, Refused> {
        if depth > MAX_DEPTH {
            return Err(self.error("invalid_json", "nested too deeply"));
        }
        match self.text.get(self.at) {
            None => Err(self.error("invalid_json", "a value is missing")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') if self.eat("true") => Ok(Value::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Value::Bool(false)),
            Some(b'n') if self.eat("null") => Ok(Value::Null),
            Some(b'N') | Some(b'I') => {
                Err(self.error("non_finite_number", "NaN and infinities are not values"))
            }
            Some(b'-') if self.text[self.at..].starts_with(b"-Infinity") => {
                Err(self.error("non_finite_number", "NaN and infinities are not values"))
            }
            Some(b'-') | Some(b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("invalid_json", "not a JSON value")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, Refused> {
        self.at += 1;
        let mut map = Map::new();
        self.skip_space();
        if self.eat("}") {
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_space();
            if self.text.get(self.at) != Some(&b'"') {
                return Err(self.error("invalid_json", "an object key must be a string"));
            }
            let key = self.string()?;
            self.skip_space();
            if !self.eat(":") {
                return Err(self.error("invalid_json", "expected `:`"));
            }
            self.skip_space();
            let item = self.value(depth + 1)?;
            if map.contains_key(&key) {
                return Err(self.error("duplicate_key", &format!("the key {key:?} appears twice")));
            }
            map.insert(key, item);
            self.skip_space();
            if self.eat(",") {
                continue;
            }
            if self.eat("}") {
                return Ok(Value::Object(map));
            }
            return Err(self.error("invalid_json", "expected `,` or `}`"));
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, Refused> {
        self.at += 1;
        let mut items = Vec::new();
        self.skip_space();
        if self.eat("]") {
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_space();
            items.push(self.value(depth + 1)?);
            self.skip_space();
            if self.eat(",") {
                continue;
            }
            if self.eat("]") {
                return Ok(Value::Array(items));
            }
            return Err(self.error("invalid_json", "expected `,` or `]`"));
        }
    }

    fn hex4(&mut self) -> Result<u32, Refused> {
        let digits = self
            .text
            .get(self.at..self.at + 4)
            .ok_or_else(|| self.error("invalid_json", "a short \\u escape"))?;
        let text = std::str::from_utf8(digits)
            .map_err(|_| self.error("invalid_json", "a bad \\u escape"))?;
        let code = u32::from_str_radix(text, 16)
            .map_err(|_| self.error("invalid_json", "a bad \\u escape"))?;
        self.at += 4;
        Ok(code)
    }

    fn string(&mut self) -> Result<String, Refused> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let start = self.at;
            while let Some(&b) = self.text.get(self.at) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.at += 1;
            }
            out.push_str(&self.src[start..self.at]);
            match self.text.get(self.at) {
                None => return Err(self.error("invalid_json", "an unterminated string")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escape = *self
                        .text
                        .get(self.at)
                        .ok_or_else(|| self.error("invalid_json", "an unterminated string"))?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&first) {
                                if !self.eat("\\u") {
                                    return Err(self.error("lone_surrogate", "a lone surrogate"));
                                }
                                let second = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&second) {
                                    return Err(self.error("lone_surrogate", "a lone surrogate"));
                                }
                                0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&first) {
                                return Err(self.error("lone_surrogate", "a lone surrogate"));
                            } else {
                                first
                            };
                            out.push(char::from_u32(code).expect("a scalar value"));
                        }
                        _ => return Err(self.error("invalid_json", "an unknown escape")),
                    }
                }
                Some(_) => {
                    return Err(self.error("invalid_json", "a control character in a string"));
                }
            }
        }
    }

    fn number(&mut self) -> Result<Value, Refused> {
        let start = self.at;
        if self.text.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        let int_start = self.at;
        while self.text.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        let int_digits = &self.text[int_start..self.at];
        if int_digits.is_empty() || (int_digits.len() > 1 && int_digits[0] == b'0') {
            return Err(self.error("invalid_json", "a malformed number"));
        }
        let mut integer = true;
        if self.text.get(self.at) == Some(&b'.') {
            integer = false;
            self.at += 1;
            let frac = self.at;
            while self.text.get(self.at).is_some_and(u8::is_ascii_digit) {
                self.at += 1;
            }
            if self.at == frac {
                return Err(self.error("invalid_json", "a malformed number"));
            }
        }
        if matches!(self.text.get(self.at), Some(b'e') | Some(b'E')) {
            integer = false;
            self.at += 1;
            if matches!(self.text.get(self.at), Some(b'+') | Some(b'-')) {
                self.at += 1;
            }
            let exp = self.at;
            while self.text.get(self.at).is_some_and(u8::is_ascii_digit) {
                self.at += 1;
            }
            if self.at == exp {
                return Err(self.error("invalid_json", "a malformed number"));
            }
        }
        let text = &self.src[start..self.at];
        if integer {
            return integer_literal(text).map_err(|e| self.error(e.code, &e.message));
        }
        let x: f64 = text
            .parse()
            .map_err(|_| self.error("invalid_json", "a malformed number"))?;
        if !x.is_finite() {
            return Err(self.error(
                "number_overflow",
                &format!("{} is too large for a number", shorten(text)),
            ));
        }
        number(x)
    }
}

/// Builds a JSON object from pairs, keeping their order.
pub fn object<I, K>(pairs: I) -> Value
where
    I: IntoIterator<Item = (K, Value)>,
    K: Into<String>,
{
    Value::Object(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_have_one_type() {
        assert_eq!(number(1.0).unwrap(), Value::from(1));
        assert_eq!(number(-0.0).unwrap(), Value::from(0));
        assert_eq!(
            canon(&parse_json("[1.0, 1e21, 1e-7, 1e16, 0.5]").unwrap()),
            "[1,1e+21,1e-7,10000000000000000,0.5]"
        );
    }

    #[test]
    fn integers_that_would_round_are_refused() {
        assert!(parse_json("9007199254740992").is_ok());
        assert!(parse_json("10000000000000000").is_ok());
        assert!(parse_json("1152921504606847000").is_ok());
        assert_eq!(
            parse_json("9007199254740993").unwrap_err().code,
            "integer_out_of_range"
        );
        assert_eq!(
            parse_json("12345678901234567891").unwrap_err().code,
            "integer_out_of_range"
        );
        assert!(parse_json("-0").is_ok());
    }

    /// The message of a refused integer literal.
    fn integer_refusal(text: &str) -> String {
        let refused = integer_literal(text).unwrap_err();
        assert_eq!(refused.code, "integer_out_of_range", "{text}");
        refused.message
    }

    #[test]
    fn integer_literals_are_canonical() {
        for (text, value) in [
            ("0", 0.0),
            ("-0", 0.0),
            ("+0", 0.0),
            ("+7", 7.0),
            ("-7", -7.0),
            ("9007199254740992", 9007199254740992.0),
            ("-9007199254740992", -9007199254740992.0),
            // Reads as 2^60, whose canonical form it is.
            ("1152921504606847000", 1152921504606846976.0),
            ("100000000000000000000", 1e20),
        ] {
            assert_eq!(
                integer_literal(text).unwrap(),
                number(value).unwrap(),
                "{text}"
            );
        }
    }

    #[test]
    fn integer_literals_that_round_are_beyond_the_exact_integers() {
        let beyond = |text: &str| {
            format!(
                "{text} is beyond the integers a number holds exactly (2^53 = 9007199254740992); \
                 quote it if it is an identifier, and give its input the string type"
            )
        };
        for text in [
            "9007199254740993",
            "-9007199254740993",
            "+9007199254740993",
            "12345678901234567891",
            "1152921504606847001",
            "1000000000000000000001",
            "99999999999999999999999",
        ] {
            assert_eq!(integer_refusal(text), beyond(text));
        }
        // Too long to show whole, beyond a double's range, or longer than any double's integers.
        let long = format!("1{}", "0".repeat(5000));
        assert_eq!(integer_refusal(&long), beyond(&format!("{}…", &long[..40])));
        let huge = format!("-{}", "9".repeat(400));
        assert_eq!(integer_refusal(&huge), beyond(&format!("{}…", &huge[..40])));
        let overflow = format!("2{}", "0".repeat(308));
        assert_eq!(
            integer_refusal(&overflow),
            beyond(&format!("{}…", &overflow[..40]))
        );
    }

    #[test]
    fn exact_integer_literals_not_written_canonically_name_the_form() {
        for (text, canonical) in [
            ("007", "7"),
            ("00", "0"),
            ("-00", "0"),
            ("-007", "-7"),
            ("+007", "7"),
            ("1152921504606846976", "1152921504606847000"),
            ("-1152921504606846976", "-1152921504606847000"),
        ] {
            assert_eq!(
                integer_refusal(text),
                format!("{text} is not written in canonical form; write {canonical}")
            );
        }
        // Leading zeros of any length: the value is still exact.
        let padded = format!("{}7", "0".repeat(5000));
        assert_eq!(
            integer_refusal(&padded),
            format!(
                "{}… is not written in canonical form; write 7",
                &padded[..40]
            )
        );
        // 10^21 and up have no canonical integer literal.
        assert_eq!(
            integer_refusal("1000000000000000000000"),
            "1000000000000000000000 is not written in canonical form; it is the number 1e+21, \
             so write it with a fraction or an exponent"
        );
        // 2^70 written out.
        assert_eq!(
            integer_refusal("1180591620717411303424"),
            "1180591620717411303424 is not written in canonical form; it is the number \
             1.1805916207174113e+21, so write it with a fraction or an exponent"
        );
    }

    #[test]
    fn a_refused_integer_in_json_names_its_line() {
        assert_eq!(
            parse_json("[\n007]").unwrap_err().code,
            "invalid_json",
            "JSON has no leading zeros"
        );
        let refused = parse_json("[\n 9007199254740993]").unwrap_err();
        assert_eq!(refused.code, "integer_out_of_range");
        assert!(
            refused
                .message
                .starts_with("line 2: 9007199254740993 is beyond the integers"),
            "{}",
            refused.message
        );
    }

    #[test]
    fn an_escape_before_a_multibyte_character_is_refused() {
        // The position after the backslash is inside the character: the line count must not
        // slice the text there.
        for text in [
            "{\"a\": \"\\é\"}",
            "\"\\é\"",
            "\"\\😀\"",
            "[\n\"x\",\n\"\\中\"]",
            "\"\\u00é\"",
            "\"\\u中\"",
        ] {
            let refused = parse_json(text).unwrap_err();
            assert_eq!(refused.code, "invalid_json", "{text}");
        }
        assert_eq!(
            parse_json("{\"a\": \"\\é\"}").unwrap_err().message,
            "line 1: an unknown escape"
        );
        assert_eq!(
            parse_json("[\n\"x\",\n\"\\中\"]").unwrap_err().message,
            "line 3: an unknown escape"
        );
    }

    #[test]
    fn markers_are_reserved() {
        let hex = "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8";
        assert!(
            check_markers(
                &parse_json(&format!(r#"{{"file": "{hex}"}}"#)).unwrap(),
                "x"
            )
            .is_err()
        );
        assert!(check_markers(&parse_json(r#"{"file": "a.png"}"#).unwrap(), "x").is_ok());
        assert!(check_markers(&parse_json(r#"{"missing": false}"#).unwrap(), "x").is_ok());
    }
}
