//! Plain-scalar resolution (spec/yaml.md "Values"): null, booleans, decimal integers (refused if
//! reading would round them, identity.md §1), decimal floats (refused on overflow), the ambiguous
//! forms (refused with "quote it"), and everything else as a string. Keys never come here: a
//! plain key is always its text.
//!
//! The forms are matched by hand, one function per pattern of yaml.md. `tools/digest.py` holds an
//! independent implementation of the same rules as regular expressions; the two must agree. Digits
//! are ASCII digits, and "any letter case" means the ASCII letter cases of the listed words.

use crate::value;
use serde_json::Value;

/// Why a plain scalar was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    /// The yaml.md rule, as the refuse vectors name it (`ambiguous_scalar`, `integer_out_of_range`,
    /// `number_overflow`).
    pub rule: &'static str,
    /// A sentence, e.g. ``plain scalar 'on' is ambiguous (…); quote it``.
    pub message: String,
}

/// Resolves the text of a plain scalar used as a value.
pub fn resolve_plain(text: &str) -> Result<Value, Unresolved> {
    match text {
        "" | "~" | "null" => return Ok(Value::Null),
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if is_decimal_integer(text) {
        return value::integer_literal(text).map_err(|refused| Unresolved {
            rule: refused.code,
            message: refused.message,
        });
    }
    if decimal(text).is_some_and(|form| !form.leading_zero) {
        // Every text `decimal` accepts is one Rust's float reader accepts, and it rounds
        // correctly; out-of-range exponents saturate to infinity or zero.
        let x: f64 = text.parse().map_err(|_| overflow(text))?;
        if !x.is_finite() {
            return Err(overflow(text));
        }
        return value::number(x).map_err(|_| overflow(text));
    }
    if let Some(reason) = ambiguity(text) {
        return Err(Unresolved {
            rule: "ambiguous_scalar",
            message: format!(
                "plain scalar {} is ambiguous ({reason}); quote it",
                crate::text::py_repr_str(text)
            ),
        });
    }
    Ok(Value::String(text.to_string()))
}

/// Whether `text` is one of the ambiguous forms of yaml.md (any letter case of yes/no/on/off,
/// non-lowercase true/false/null, leading zeros, 0x/0o/0b, underscores, sexagesimal, .inf/.nan,
/// YAML 1.1 timestamps).
pub fn is_ambiguous(text: &str) -> bool {
    matches!(
        resolve_plain(text),
        Err(Unresolved {
            rule: "ambiguous_scalar",
            ..
        })
    )
}

fn overflow(text: &str) -> Unresolved {
    Unresolved {
        rule: "number_overflow",
        message: format!("number {text} overflows to infinity"),
    }
}

/// Why a plain scalar that is neither null, a boolean nor a number is refused, if it is. The
/// checks run in this order, so a text that fits several forms gets the first reason.
fn ambiguity(text: &str) -> Option<&'static str> {
    if is_word(text) {
        return Some("a word that YAML 1.1 or another letter case reads as a boolean or null");
    }
    if decimal(text).is_some_and(|form| form.leading_zero) {
        return Some("a number with a leading zero");
    }
    if is_radix(text) {
        return Some("a hexadecimal, octal or binary number");
    }
    if text.contains('_') && is_number_like(&text.replace('_', "")) {
        return Some("a number once its underscores are removed");
    }
    if is_sexagesimal(text) {
        return Some("a sexagesimal number");
    }
    if is_special(text) {
        return Some("an infinity or NaN");
    }
    if is_timestamp(text) {
        return Some("a date or timestamp");
    }
    None
}

/// Any numeric reading of any YAML version: decimal (leading zeros too), radix, sexagesimal.
fn is_number_like(text: &str) -> bool {
    decimal(text).is_some() || is_radix(text) || is_sexagesimal(text)
}

/// `yes|no|on|off|true|false|null` in any ASCII letter case.
fn is_word(text: &str) -> bool {
    ["yes", "no", "on", "off", "true", "false", "null"]
        .iter()
        .any(|word| text.eq_ignore_ascii_case(word))
}

/// Strips one leading `-` or `+`.
fn unsigned(text: &str) -> &str {
    text.strip_prefix(['-', '+']).unwrap_or(text)
}

/// The number of leading ASCII digits.
fn digit_run(text: &str) -> usize {
    text.bytes().take_while(u8::is_ascii_digit).count()
}

/// `[-+]?(?:0|[1-9][0-9]*)`
fn is_decimal_integer(text: &str) -> bool {
    let digits = unsigned(text);
    !digits.is_empty()
        && digit_run(digits) == digits.len()
        && (digits == "0" || !digits.starts_with('0'))
}

/// A decimal number as some YAML version reads it.
struct Decimal {
    /// The integer part has more than one digit and starts with `0` (`017`, `00.5`, `01e3`).
    leading_zero: bool,
}

/// `[-+]?(?:\.[0-9]+|[0-9]+\.[0-9]*|[0-9]+)(?:[eE][-+]?[0-9]+)?`, leading zeros included.
fn decimal(text: &str) -> Option<Decimal> {
    let body = unsigned(text);
    let whole = digit_run(body);
    let mut rest = &body[whole..];
    if let Some(after_dot) = rest.strip_prefix('.') {
        let fraction = digit_run(after_dot);
        if whole == 0 && fraction == 0 {
            return None;
        }
        rest = &after_dot[fraction..];
    } else if whole == 0 {
        return None;
    }
    if let Some(exponent) = rest.strip_prefix(['e', 'E']) {
        let exponent = unsigned(exponent);
        let digits = digit_run(exponent);
        if digits == 0 {
            return None;
        }
        rest = &exponent[digits..];
    }
    if !rest.is_empty() {
        return None;
    }
    Some(Decimal {
        leading_zero: whole > 1 && body.starts_with('0'),
    })
}

/// `[-+]?0(?:[xX][0-9a-fA-F_]+|[oO][0-7_]+|[bB][01_]+)`
fn is_radix(text: &str) -> bool {
    let Some(rest) = unsigned(text).strip_prefix('0') else {
        return false;
    };
    let mut chars = rest.chars();
    let allowed: fn(char) -> bool = match chars.next() {
        Some('x' | 'X') => |c| c.is_ascii_hexdigit() || c == '_',
        Some('o' | 'O') => |c| ('0'..='7').contains(&c) || c == '_',
        Some('b' | 'B') => |c| c == '0' || c == '1' || c == '_',
        _ => return false,
    };
    let digits = chars.as_str();
    !digits.is_empty() && digits.chars().all(allowed)
}

/// `[-+]?[0-9]+(?::[0-9]+)+(?:\.[0-9]*)?`
fn is_sexagesimal(text: &str) -> bool {
    let body = unsigned(text);
    let (groups, fraction) = match body.split_once('.') {
        Some((groups, fraction)) => (groups, fraction),
        None => (body, ""),
    };
    if digit_run(fraction) != fraction.len() {
        return false;
    }
    let mut parts = groups.split(':');
    let mut count = 0;
    for part in parts.by_ref() {
        if part.is_empty() || digit_run(part) != part.len() {
            return false;
        }
        count += 1;
    }
    count >= 2
}

/// `[-+]?\.inf` or `\.nan`, in any ASCII letter case.
fn is_special(text: &str) -> bool {
    unsigned(text).eq_ignore_ascii_case(".inf") || text.eq_ignore_ascii_case(".nan")
}

/// YAML 1.1's timestamp type: `YYYY-MM-DD`, or a date with one- or two-digit month and day
/// followed by `T`, `t` or spaces/tabs and a time `H[H]:MM:SS[.fraction]`, then an optional zone
/// (`Z` or `±H[H][:MM]`) after optional spaces/tabs. A short date alone (`2026-1-5`) is a string.
fn is_timestamp(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits = |from: usize, count: usize| {
        bytes.len() >= from + count && bytes[from..from + count].iter().all(u8::is_ascii_digit)
    };
    // YYYY-MM-DD exactly.
    if bytes.len() == 10 && digits(0, 4) && bytes[4] == b'-' && digits(5, 2) && bytes[7] == b'-' {
        return digits(8, 2);
    }
    let take = |at: &mut usize, min: usize, max: usize| -> bool {
        let run = bytes[*at..]
            .iter()
            .take(max)
            .take_while(|b| b.is_ascii_digit())
            .count();
        if run < min {
            return false;
        }
        *at += run;
        true
    };
    let byte = |at: usize| bytes.get(at).copied();
    // The date with one- or two-digit month and day.
    if !(digits(0, 4) && byte(4) == Some(b'-')) {
        return false;
    }
    let mut at = 5;
    if !take(&mut at, 1, 2) || byte(at) != Some(b'-') {
        return false;
    }
    at += 1;
    if !take(&mut at, 1, 2) {
        return false;
    }
    // The separator: `T`, `t`, or one or more spaces and tabs.
    match byte(at) {
        Some(b'T' | b't') => at += 1,
        Some(b' ' | b'\t') => {
            while matches!(byte(at), Some(b' ' | b'\t')) {
                at += 1;
            }
        }
        _ => return false,
    }
    // The time.
    if !take(&mut at, 1, 2) || byte(at) != Some(b':') {
        return false;
    }
    at += 1;
    if !take(&mut at, 2, 2) || byte(at) != Some(b':') {
        return false;
    }
    at += 1;
    if !take(&mut at, 2, 2) {
        return false;
    }
    if byte(at) == Some(b'.') {
        at += 1;
        while byte(at).is_some_and(|b| b.is_ascii_digit()) {
            at += 1;
        }
    }
    if at == bytes.len() {
        return true;
    }
    // The zone, after optional spaces and tabs.
    while matches!(byte(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    match byte(at) {
        Some(b'Z') => at += 1,
        Some(b'-' | b'+') => {
            at += 1;
            if !take(&mut at, 1, 2) {
                return false;
            }
            if byte(at) == Some(b':') {
                at += 1;
                if !take(&mut at, 2, 2) {
                    return false;
                }
            }
        }
        _ => return false,
    }
    at == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(text: &str) -> &'static str {
        resolve_plain(text).expect_err(text).rule
    }

    #[test]
    fn null_bool_and_numbers() {
        for text in ["", "~", "null"] {
            assert_eq!(resolve_plain(text), Ok(Value::Null));
        }
        assert_eq!(resolve_plain("true"), Ok(json!(true)));
        assert_eq!(resolve_plain("false"), Ok(json!(false)));
        for (text, want) in [
            ("0", json!(0)),
            ("-0", json!(0)),
            ("+0", json!(0)),
            ("-5", json!(-5)),
            ("+5", json!(5)),
            ("1.0", json!(1)),
            (".5", json!(0.5)),
            ("5.", json!(5)),
            ("1e3", json!(1000)),
            ("-1.5e-3", json!(-0.0015)),
            ("2.5E+2", json!(250)),
            ("0e5", json!(0)),
            ("-0.0", json!(0)),
            ("0.", json!(0)),
            ("0.25", json!(0.25)),
            ("1e-400", json!(0)),
            ("9007199254740992", json!(9007199254740992_i64)),
        ] {
            assert_eq!(resolve_plain(text), Ok(want), "{text}");
        }
    }

    #[test]
    fn refused_numbers() {
        assert_eq!(rule("1e400"), "number_overflow");
        assert_eq!(rule("-1e400"), "number_overflow");
        assert_eq!(rule("9007199254740993"), "integer_out_of_range");
        assert_eq!(rule("12345678901234567891"), "integer_out_of_range");
        assert_eq!(
            resolve_plain("1e400").unwrap_err().message,
            "number 1e400 overflows to infinity"
        );
    }

    #[test]
    fn ambiguous_forms() {
        for text in [
            "yes",
            "No",
            "ON",
            "oFF",
            "True",
            "FALSE",
            "nULL",
            "017",
            "-017",
            "01.5",
            "00.5",
            "01e3",
            "00",
            "0x1F",
            "-0x1F",
            "0X1F",
            "0xff_ff",
            "0o17",
            "0O17",
            "0b101",
            "+0b1",
            "0B101",
            "0x_",
            "1_000",
            "1_000.5",
            ".5_0",
            "1_",
            "0x1_F",
            "1:30",
            "16:9",
            "-1:30",
            "1:30.5",
            "10:00",
            "1:2:3",
            "1:30.",
            ".inf",
            "-.Inf",
            "+.INF",
            ".NaN",
            ".nan",
            ".iNf",
            "2026-10-05",
            "2026-10-05T10:00:00Z",
            "2026-1-5 9:30:00",
            "2026-10-05 10:00:00",
            "2026-10-05t10:00:00",
            "2026-10-05T10:00:00.5",
            "2026-10-05T10:00:00.",
            "2001-12-14 21:59:43.10 -5",
            "2026-10-05T10:00:00+09:00",
            "2026-10-05\t10:00:00",
            "2026-10-05  10:00:00 Z",
        ] {
            assert_eq!(rule(text), "ambiguous_scalar", "{text}");
            assert!(is_ambiguous(text), "{text}");
        }
        let message = resolve_plain("yes").unwrap_err().message;
        assert!(message.starts_with("plain scalar 'yes' is ambiguous ("));
        assert!(message.ends_with("; quote it"));
    }

    #[test]
    fn reasons_follow_the_order_of_the_checks() {
        let reason = |text: &str| ambiguity(text).unwrap();
        assert_eq!(reason("017"), "a number with a leading zero");
        assert_eq!(reason("0x1F"), "a hexadecimal, octal or binary number");
        assert_eq!(reason("1_000"), "a number once its underscores are removed");
        assert_eq!(reason("16:9"), "a sexagesimal number");
        assert_eq!(reason(".inf"), "an infinity or NaN");
        assert_eq!(reason("2026-10-05"), "a date or timestamp");
        assert_eq!(
            reason("On"),
            "a word that YAML 1.1 or another letter case reads as a boolean or null"
        );
    }

    #[test]
    fn lookalikes_are_strings() {
        for text in [
            "0bad",
            "0ops",
            "0x",
            "0b12",
            "0o8",
            "0xg1",
            "-.nan",
            "+.NaN",
            "inf",
            "nan",
            ".infinity",
            "-inf",
            "_",
            "snake_case",
            "v1_2",
            "_x1",
            "1_a",
            "yesterday",
            "online",
            "offset",
            "nullable",
            "Truth",
            "nope",
            "y",
            "n",
            "Y",
            "N",
            "2026-1-5",
            "1.2.3",
            "1024x1024",
            "3d",
            "a:b",
            "acme.test:8080",
            "-leading",
            "1:",
            ":30",
            "1::30",
            "2026-10-05T10:00",
            "2026-10-05T10:00:00ZZ",
            "2026-10-05T1:00:0",
            "12026-10-05",
            "2026-100-05 10:00:00",
            ".",
            "+",
            "-",
            "1e",
            "e3",
            ".e3",
            "1.5.5",
            "١٢",
            "yeſ",
        ] {
            assert_eq!(
                resolve_plain(text),
                Ok(Value::String(text.into())),
                "{text}"
            );
            assert!(!is_ambiguous(text), "{text}");
        }
        assert!(!is_ambiguous("true"));
        assert!(!is_ambiguous("1"));
    }
}
