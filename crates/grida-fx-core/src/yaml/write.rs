//! Writing YAML (yaml.md "Writing YAML"): `fx.lock` and takes files.
//!
//! Block style, two-space indentation, keys in the order the value holds them (callers sort),
//! and every string quoted when, read back as a plain scalar, it would resolve to something other
//! than itself or be refused (a digest like `123e4567…`, `on`, `017`, `2026-10-05`, `''`, text
//! with `: `, ` #`, leading indicators, …). Strings are quoted with double quotes and JSON-style
//! escapes. Numbers are written in their JCS form. The result ends with one `\n`.
//!
//! Round-trip property (tested): `load(write_yaml(v)) == v` for every FX value, apart from a
//! mapping key longer than 1024 characters, which YAML cannot write as an implicit key.

use super::resolve::resolve_plain;
use crate::value;
use serde_json::{Map, Value};

/// Writes `value` as a YAML document.
pub fn write_yaml(value: &Value) -> String {
    let mut out = String::new();
    match value {
        Value::Object(map) if !map.is_empty() => write_mapping(&mut out, map, 0, false),
        Value::Array(items) if !items.is_empty() => write_sequence(&mut out, items, 0, false),
        scalar => {
            out.push_str(&scalar_text(scalar));
            out.push('\n');
        }
    }
    out
}

/// Whether a string must be quoted to read back as itself.
pub fn needs_quotes(s: &str) -> bool {
    // Anything a plain scalar would read as something else, or refuse: '', null, ~, true, 1,
    // 1e3, on, 017, 2026-10-05, .inf, …
    match resolve_plain(s) {
        Ok(Value::String(read)) if read == s => {}
        _ => return true,
    }
    let Some(first) = s.chars().next() else {
        return true;
    };
    // YAML 1.1's merge and value types: FX reads them as text, YAML 1.1 readers (PyYAML among
    // them) do not, and a plain `<<` key is a merge key.
    if s == "<<" || s == "=" {
        return true;
    }
    // A leading indicator starts something other than a plain scalar (or might, for `-`, `?`
    // and `:`); `---` and `...` at the start of a line are document markers.
    if "-?:,[]{}#&*!|>'\"%@`".contains(first) || s.starts_with("...") {
        return true;
    }
    // Plain scalars lose leading and trailing white space; `: ` (or a final `:`) makes a key and
    // ` #` starts a comment.
    if s.starts_with(' ') || s.ends_with(' ') || s.ends_with(':') {
        return true;
    }
    if s.contains(": ") || s.contains(" #") {
        return true;
    }
    // Line breaks, tabs and other characters a plain scalar cannot hold as they are.
    s.chars().any(needs_escape)
}

/// Characters written as an escape inside double quotes: C0 and C1 controls (tab and line feed
/// included), DEL, the characters yaml.md refuses anywhere in a stream (U+0085, U+2028, U+2029),
/// the byte-order mark, and the noncharacters U+FFFE and U+FFFF, which YAML streams cannot hold.
fn needs_escape(c: char) -> bool {
    matches!(
        c,
        '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}' | '\u{feff}' | '\u{fffe}' | '\u{ffff}'
    )
}

fn write_indent(out: &mut String, indent: usize) {
    out.extend(std::iter::repeat_n(' ', indent));
}

/// Writes a non-empty mapping, one `key: value` per line at `indent`. With `inline`, the first
/// line's indentation has already been written (after a sequence's `- `).
fn write_mapping(out: &mut String, map: &Map<String, Value>, indent: usize, inline: bool) {
    for (i, (key, item)) in map.iter().enumerate() {
        if i > 0 || !inline {
            write_indent(out, indent);
        }
        out.push_str(&key_text(key));
        out.push(':');
        match item {
            Value::Object(child) if !child.is_empty() => {
                out.push('\n');
                write_mapping(out, child, indent + 2, false);
            }
            Value::Array(items) if !items.is_empty() => {
                out.push('\n');
                write_sequence(out, items, indent + 2, false);
            }
            scalar => {
                out.push(' ');
                out.push_str(&scalar_text(scalar));
                out.push('\n');
            }
        }
    }
}

/// Writes a non-empty sequence, one `- item` per line at `indent`. With `inline`, the first
/// line's indentation has already been written (after an outer `- `).
fn write_sequence(out: &mut String, items: &[Value], indent: usize, inline: bool) {
    for (i, item) in items.iter().enumerate() {
        if i > 0 || !inline {
            write_indent(out, indent);
        }
        out.push('-');
        match item {
            Value::Object(child) if !child.is_empty() => {
                out.push(' ');
                write_mapping(out, child, indent + 2, true);
            }
            Value::Array(inner) if !inner.is_empty() => {
                out.push(' ');
                write_sequence(out, inner, indent + 2, true);
            }
            scalar => {
                out.push(' ');
                out.push_str(&scalar_text(scalar));
                out.push('\n');
            }
        }
    }
}

/// A mapping key: plain when it reads back as itself. Plain keys are always their text, so only
/// the syntax matters, but quoting by the value rule keeps keys and values alike (and quotes
/// `<<`, which would be a merge key).
fn key_text(key: &str) -> String {
    if needs_quotes(key) {
        quoted(key)
    } else {
        key.to_string()
    }
}

/// A scalar or an empty collection, as it is written after `key: ` or `- `.
fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(true) => "true".into(),
        Value::Bool(false) => "false".into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => value::format_number(value::as_f64(n)),
        },
        Value::String(s) if needs_quotes(s) => quoted(s),
        Value::String(s) => s.clone(),
        Value::Array(_) => "[]".into(),
        Value::Object(_) => "{}".into(),
    }
}

/// A double-quoted scalar with JSON's escapes (YAML's double-quoted escapes include all of them).
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
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
            c if needs_escape(c) => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml::load;
    use serde_json::json;

    fn round_trip(value: &Value) {
        let text = write_yaml(value);
        let back = load(text.as_bytes(), "w.yaml")
            .unwrap_or_else(|error| panic!("{error}\n--- written:\n{text}"));
        assert_eq!(
            value::canon(&back),
            value::canon(value),
            "--- written:\n{text}"
        );
        // Keys keep their order, and writing is stable.
        assert_eq!(write_yaml(&back), text);
    }

    #[test]
    fn block_layout() {
        let value = json!({
            "fx": "lock/v1",
            "nodes": {"nodes/acme.py#tint@1": "2166e237877fb0c822e9db9e5ec66af1ac0029648b722c00c5e566bacd251337"},
            "list": [1, {"a": 1, "b": [true, null]}, [2, 3], [], {}],
            "empty": {},
        });
        assert_eq!(
            write_yaml(&value),
            "fx: lock/v1\n\
             nodes:\n  \
               nodes/acme.py#tint@1: 2166e237877fb0c822e9db9e5ec66af1ac0029648b722c00c5e566bacd251337\n\
             list:\n  \
               - 1\n  \
               - a: 1\n    \
                 b:\n      \
                   - true\n      \
                   - null\n  \
               - - 2\n    \
                 - 3\n  \
               - []\n  \
               - {}\n\
             empty: {}\n"
        );
        round_trip(&value);
    }

    #[test]
    fn top_level_values() {
        assert_eq!(write_yaml(&json!({})), "{}\n");
        assert_eq!(write_yaml(&json!([])), "[]\n");
        assert_eq!(write_yaml(&json!(null)), "null\n");
        assert_eq!(write_yaml(&json!("")), "\"\"\n");
        assert_eq!(write_yaml(&json!("...")), "\"...\"\n");
        assert_eq!(write_yaml(&json!("---")), "\"---\"\n");
        assert_eq!(write_yaml(&json!([1, "a"])), "- 1\n- a\n");
        for value in [
            json!({}),
            json!([]),
            json!(null),
            json!(""),
            json!("..."),
            json!("---"),
            json!("--- x"),
            json!("... x"),
            json!(7),
            json!("plain"),
            json!([[[1]], [[]], [{}]]),
        ] {
            round_trip(&value);
        }
    }

    #[test]
    fn numbers_in_jcs_form() {
        let value = json!({"a": 1.0, "b": 0.5, "c": 1e21, "d": 1e-7, "e": -2, "f": 1152921504606847000.0_f64});
        assert_eq!(
            write_yaml(&value),
            "a: 1\nb: 0.5\nc: 1e+21\nd: 1e-7\ne: -2\nf: 1152921504606847000\n"
        );
        round_trip(&value);
    }

    #[test]
    fn digests_and_ambiguous_strings_are_quoted() {
        assert_eq!(write_yaml(&json!({"d": "123e4567"})), "d: \"123e4567\"\n");
        for s in [
            "",
            "123e4567",
            "1e3",
            "on",
            "Off",
            "YES",
            "True",
            "null",
            "~",
            "true",
            "017",
            "1",
            "-1.5",
            "2026-10-05",
            "2026-10-05T10:00:00Z",
            "16:9",
            "1_000",
            ".inf",
            ".NaN",
            "0x1F",
            "9007199254740993",
            "1e400",
            "<<",
            "=",
        ] {
            assert!(needs_quotes(s), "{s}");
        }
        for s in [
            "a",
            "img-a@acme",
            "nodes/acme.py#tint@1",
            "a:b",
            "a#b",
            "y",
            "n",
            "0bad",
            "2026-1-5",
            "hello world",
            "é",
            "1.2.3",
            "a=b",
            "<<x",
        ] {
            assert!(!needs_quotes(s), "{s}");
        }
    }

    #[test]
    fn tricky_strings_round_trip() {
        let strings = [
            "",
            " ",
            "  padded  ",
            " lead",
            "trail ",
            "a: b",
            "a:",
            "a :b",
            "a #b",
            "a#b",
            "#c",
            "- x",
            "-",
            "-x",
            "? x",
            "?x",
            ":x",
            ": x",
            ",x",
            "[x",
            "]x",
            "{x",
            "}x",
            "&x",
            "*x",
            "!x",
            "|x",
            ">x",
            "'x",
            "\"x",
            "%x",
            "@x",
            "`x",
            "x,y",
            "x]",
            "x}",
            "x{y}",
            "x[0]",
            "a\nb",
            "a\n",
            "\n",
            "a\tb",
            "\t",
            "a\rb",
            "a\r\nb",
            "\u{0}",
            "\u{1}\u{7f}\u{80}",
            "\u{85}",
            "\u{2028}",
            "\u{2029}",
            "\u{feff}x",
            "x\u{feff}",
            "\u{fffe}\u{ffff}",
            "back\\slash",
            "quote\"d",
            "it's",
            "'",
            "\"",
            "é 日本 😀",
            "\u{a0}x\u{a0}",
            "<<",
            "...",
            "---",
            "...x",
            "--- x",
            "a ... b",
            "a --- b",
            "~",
            "null",
            "Null",
            "on",
            "yes",
            "y",
            "017",
            "0.5",
            "1e3",
            "123e4567",
            "2026-10-05",
            "16:9",
            "=",
            "a  b",
            "a   #b",
            "x:\ty",
            "x\t#y",
            "%YAML 1.2",
            "!!str",
            "&a",
            "*a",
            "a: ",
            "a ",
            "\\",
            "\u{1F600}",
            "\u{10FFFF}",
            "\u{e000}",
            "a\u{200b}b",
            "a\u{0}b",
            "${{ x }}",
        ];
        for s in strings {
            round_trip(&json!({"k": s}));
            round_trip(&json!([s]));
            round_trip(&json!(s));
            if s.chars().count() <= 1024 {
                round_trip(&json!({s: 1}));
                round_trip(&json!({"k": {s: [s, {s: s}]}}));
            }
        }
    }

    #[test]
    fn keys_are_quoted_when_needed() {
        let value = json!({"<<": 1, "on": 2, "1": 3, "": 4, "a: b": 5, "plain": 6});
        assert_eq!(
            write_yaml(&value),
            "\"<<\": 1\n\"on\": 2\n\"1\": 3\n\"\": 4\n\"a: b\": 5\nplain: 6\n"
        );
        round_trip(&value);
    }

    #[test]
    fn random_strings_round_trip() {
        // A fixed pseudo-random sequence over the characters that matter to YAML's syntax.
        const ALPHABET: &[char] = &[
            'a', 'b', 'x', 'y', 'e', 'E', 'n', 'o', 'O', 'f', 'T', 'Z', '0', '1', '7', '9', '.',
            '-', '+', '_', ':', '#', ' ', ' ', '\t', '\n', '\r', '?', ',', '[', ']', '{', '}', '&',
            '*', '!', '|', '>', '\'', '"', '%', '@', '`', '\\', '~', '<', '=', '\u{0}', '\u{7f}',
            '\u{85}', '\u{a0}', '\u{2028}', '\u{3000}', '\u{feff}', 'é', '😀',
        ];
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        for _ in 0..10_000 {
            let length = 1 + next(8);
            let s: String = (0..length)
                .map(|_| ALPHABET[next(ALPHABET.len())])
                .collect();
            round_trip(&json!({"k": s}));
            round_trip(&json!({s.clone(): [s]}));
        }
    }

    #[test]
    fn nesting_round_trips() {
        let value = json!({
            "a": [[1, [2, [3, {"x": [4, {"y": {}}]}]]], {"b": {"c": {"d": [[], [[]]]}}}],
            "e": {"f": [{"g": 1, "h": [{"i": null}]}]},
        });
        round_trip(&value);
        let mut deep = json!("leaf");
        for i in 0..50 {
            deep = if i % 2 == 0 {
                json!([deep, i])
            } else {
                json!({"k": deep, "n": i})
            };
        }
        round_trip(&deep);
    }
}
