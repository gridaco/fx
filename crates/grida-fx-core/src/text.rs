//! Text: decoding file content, prompt-file comments, and Python-style quoting for messages.
//!
//! - [`decode_text`] is identity.md §5 "Decoding": strict UTF-8, one leading byte-order mark
//!   removed, line endings kept. It is the one rule for inputs, cached outputs, prompt files and
//!   templates (FX differs from gnode here: gnode kept the BOM and used universal newlines).
//! - [`prompt_text`] is identity.md §5 "Prompt files": every `<!-- … -->` (non-greedy, across
//!   lines) removed together with one directly following `\n`.
//! - [`py_repr_str`] and [`py_repr`] reproduce Python's `repr`, which gnode's messages embed
//!   (`no field 'x'`, `key 'a' names two items`, `'x' is not of type 'integer'`). FX keeps those
//!   message texts.

use crate::error::{Error, Result};
use crate::value;
use serde_json::Value;

/// The byte-order mark.
const BOM: char = '\u{FEFF}';

/// Decodes file bytes as text (identity.md §5). `label` names the file in the error, which reads
/// `<label> is not UTF-8 text`.
pub fn decode_text(bytes: &[u8], label: &str) -> Result<String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::input(format!("{label} is not UTF-8 text")))?;
    Ok(text.strip_prefix(BOM).unwrap_or(text).to_string())
}

/// Removes prompt-file comments (identity.md §5): what the regular expression `<!--.*?-->\n?`
/// with DOTALL removes. An unclosed `<!--` is kept as text. A comment followed by `\r\n` loses
/// nothing extra.
pub fn prompt_text(source: &str) -> String {
    const OPEN: &str = "<!--";
    const CLOSE: &str = "-->";
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find(OPEN) {
        // The closing `-->` is searched after the whole `<!--`: `<!-->` is not a comment.
        let body = &rest[start + OPEN.len()..];
        let Some(end) = body.find(CLOSE) else {
            // No `-->` follows this `<!--`, so none follows a later one either.
            break;
        };
        out.push_str(&rest[..start]);
        let after = &body[end + CLOSE.len()..];
        rest = after.strip_prefix('\n').unwrap_or(after);
    }
    out.push_str(rest);
    out
}

/// Python's `repr` of a string: single quotes unless the text holds `'` and no `"`; escapes `\\`,
/// the chosen quote, `\n`, `\r`, `\t` and non-printable code points (`\xNN`, `\uNNNN`,
/// `\UNNNNNNNN`); printable non-ASCII stays as is.
///
/// Python decides printability with `str.isprintable`, from the Unicode database of its version.
/// This approximates it without the database: non-printable are the control characters (Cc),
/// every space separator but the ASCII space (Zs), the line and paragraph separators (Zl, Zp), the
/// format characters (Cf), private use (Co) and the noncharacters. Other unassigned code points
/// (Cn), which Python escapes too, are kept as they are.
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_printable(c) => out.push(c),
            c => {
                let code = c as u32;
                if code <= 0xff {
                    out.push_str(&format!("\\x{code:02x}"));
                } else if code <= 0xffff {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    out.push_str(&format!("\\U{code:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

/// An approximation of Python's `str.isprintable` for one character (see [`py_repr_str`]).
fn is_printable(c: char) -> bool {
    let code = c as u32;
    if code < 0x7f {
        return code >= 0x20;
    }
    !matches!(
        code,
        // Cc
        0x7f..=0x9f
        // Zs other than the ASCII space
        | 0xa0
        | 0x1680
        | 0x2000..=0x200a
        | 0x202f
        | 0x205f
        | 0x3000
        // Zl, Zp
        | 0x2028
        | 0x2029
        // Cf
        | 0xad
        | 0x600..=0x605
        | 0x61c
        | 0x6dd
        | 0x70f
        | 0x890..=0x891
        | 0x8e2
        | 0x180e
        | 0x200b..=0x200f
        | 0x202a..=0x202e
        | 0x2060..=0x2064
        | 0x2066..=0x206f
        | 0xfeff
        | 0xfff9..=0xfffb
        | 0x110bd
        | 0x110cd
        | 0x13430..=0x1343f
        | 0x1bca0..=0x1bca3
        | 0x1d173..=0x1d17a
        | 0xe0001
        | 0xe0020..=0xe007f
        // Co
        | 0xe000..=0xf8ff
        | 0xf0000..=0xffffd
        | 0x100000..=0x10fffd
        // Noncharacters
        | 0xfdd0..=0xfdef
    ) && (code & 0xfffe) != 0xfffe
}

/// Python's `repr` of a JSON value as gnode's messages print it: strings per [`py_repr_str`],
/// `True`/`False`/`None`, numbers in their JCS form (FX has one number type), lists `[1, 2]`,
/// objects `{'a': 1}` in their own key order.
pub fn py_repr(value: &Value) -> String {
    let mut out = String::new();
    write_repr(&mut out, value);
    out
}

fn write_repr(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("None"),
        Value::Bool(true) => out.push_str("True"),
        Value::Bool(false) => out.push_str("False"),
        Value::Number(n) => match n.as_i64() {
            Some(i) => out.push_str(&i.to_string()),
            None => out.push_str(&value::format_number(value::as_f64(n))),
        },
        Value::String(s) => out.push_str(&py_repr_str(s)),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_repr(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&py_repr_str(key));
                out.push_str(": ");
                write_repr(out, item);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use serde_json::json;

    #[test]
    fn decode_strips_one_bom_and_keeps_line_endings() {
        assert_eq!(decode_text(b"a\r\nb\rc\n", "x").unwrap(), "a\r\nb\rc\n");
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi", "x").unwrap(), "hi");
        assert_eq!(
            decode_text(b"\xEF\xBB\xBF\xEF\xBB\xBFhi", "x").unwrap(),
            "\u{FEFF}hi"
        );
        assert_eq!(decode_text(b"", "x").unwrap(), "");
        assert_eq!(decode_text("é".as_bytes(), "x").unwrap(), "é");
    }

    #[test]
    fn decode_refuses_invalid_utf8() {
        let error = decode_text(b"name: acme\xff", "prompts/a.md").unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
        assert_eq!(error.message, "prompts/a.md is not UTF-8 text");
        // UTF-16 with a BOM is not UTF-8.
        assert!(decode_text(b"\xff\xfea\x00", "x").is_err());
        // A lone surrogate encoded as UTF-8 (CESU) is refused.
        assert!(decode_text(b"\xed\xa0\x80", "x").is_err());
    }

    #[test]
    fn prompt_comments() {
        assert_eq!(prompt_text("<!-- ${{ broken( }} -->\nok"), "ok");
        assert_eq!(prompt_text("a<!-- x -->b"), "ab");
        assert_eq!(prompt_text("a<!-- x -->\n\nb"), "a\nb");
        assert_eq!(prompt_text("a<!-- x -->\r\nb"), "a\r\nb");
        assert_eq!(prompt_text("<!-- one\ntwo -->\nkept"), "kept");
        // Non-greedy: each comment ends at its first `-->`.
        assert_eq!(prompt_text("<!-- a -->x<!-- b -->y"), "xy");
        assert_eq!(prompt_text("<!-- a --> b -->"), " b -->");
        // Unclosed: kept, and so is everything after it.
        assert_eq!(prompt_text("a <!-- open"), "a <!-- open");
        assert_eq!(prompt_text("<!-- x -->\nb <!-- open"), "b <!-- open");
        // The `-->` must follow the whole `<!--`.
        assert_eq!(prompt_text("<!-->x"), "<!-->x");
        assert_eq!(prompt_text("<!--->x"), "<!--->x");
        assert_eq!(prompt_text("<!---->x"), "x");
        assert_eq!(prompt_text("<!---->\n"), "");
        assert_eq!(prompt_text("<<!-- x -->!-- y -->z"), "<!-- y -->z");
        assert_eq!(prompt_text("é<!-- ü -->ö"), "éö");
        assert_eq!(prompt_text(""), "");
    }

    #[test]
    fn repr_of_strings() {
        assert_eq!(py_repr_str("x"), "'x'");
        assert_eq!(py_repr_str(""), "''");
        assert_eq!(py_repr_str("it's"), "\"it's\"");
        assert_eq!(py_repr_str("say \"hi\""), "'say \"hi\"'");
        assert_eq!(py_repr_str("it's \"x\""), "'it\\'s \"x\"'");
        assert_eq!(py_repr_str("a\\b"), "'a\\\\b'");
        assert_eq!(py_repr_str("a\nb\rc\td"), "'a\\nb\\rc\\td'");
        assert_eq!(py_repr_str("\u{0}\u{1b}\u{7f}"), "'\\x00\\x1b\\x7f'");
        assert_eq!(py_repr_str("\u{85}\u{a0}\u{ad}"), "'\\x85\\xa0\\xad'");
        assert_eq!(
            py_repr_str("\u{2028}\u{2029}\u{feff}\u{200b}\u{3000}"),
            "'\\u2028\\u2029\\ufeff\\u200b\\u3000'"
        );
        assert_eq!(
            py_repr_str("\u{e0001}\u{f0000}"),
            "'\\U000e0001\\U000f0000'"
        );
        assert_eq!(py_repr_str("\u{fffe}\u{1ffff}"), "'\\ufffe\\U0001ffff'");
        assert_eq!(py_repr_str("é 日本 😀"), "'é 日本 😀'");
        assert_eq!(py_repr_str("${{ x"), "'${{ x'");
    }

    #[test]
    fn repr_of_values() {
        assert_eq!(py_repr(&json!(null)), "None");
        assert_eq!(py_repr(&json!(true)), "True");
        assert_eq!(py_repr(&json!(false)), "False");
        assert_eq!(py_repr(&json!(1)), "1");
        assert_eq!(py_repr(&json!(-3)), "-3");
        assert_eq!(py_repr(&json!(1.5)), "1.5");
        assert_eq!(py_repr(&json!(1e21)), "1e+21");
        assert_eq!(py_repr(&json!(0.1 + 0.2)), "0.30000000000000004");
        assert_eq!(py_repr(&json!([1, 2])), "[1, 2]");
        assert_eq!(py_repr(&json!([])), "[]");
        assert_eq!(py_repr(&json!({})), "{}");
        assert_eq!(py_repr(&json!({"a": 1})), "{'a': 1}");
        assert_eq!(
            py_repr(&json!({"b": [true, null], "a": {"it's": "x"}})),
            "{'b': [True, None], 'a': {\"it's\": 'x'}}"
        );
    }
}
