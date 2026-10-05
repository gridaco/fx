//! The lexer: at each position the first alternative that matches wins, in this order: space,
//! number `\d+(\.\d+)?`, string (single or double quoted, backslash pairs), operator (two-char
//! first), name `[A-Za-z_][A-Za-z0-9_]*`. Anything else: `unexpected {repr(ch)} at {pos} in
//! {repr(source)}`. A final `End` token at `len(source)` is appended. Positions count characters,
//! not bytes.
//!
//! Numbers have no sign, exponent, leading or trailing dot: `1e3` is the number `1` and the name
//! `e3`. Strings have no C escapes. `true`, `false` and `null` are literals, except after a dot.
//!
//! FX decision: digits are the ASCII digits `0-9` and whitespace is ASCII whitespace
//! ([`is_space`]); gnode accepted Unicode ones.

use super::ExprError;
use crate::text::py_repr_str;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Number,
    Str,
    Op,
    Name,
    End,
}

/// A token: its kind, its exact source text (a string token keeps its quotes) and its character
/// position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub at: usize,
}

/// The two-character operators, tried before the one-character ones.
const OPS2: [&str; 7] = ["&&", "||", "??", "==", "!=", "<=", ">="];

/// The one-character operators.
const OPS1: &str = "-+*/<>!().,[]";

/// Whitespace between tokens, and what `strip()` removes around a template's inner text: the
/// ASCII whitespace characters space, tab, line feed, carriage return, vertical tab and form
/// feed (Python's `\s` restricted to ASCII).
pub fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{0B}' | '\u{0C}')
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Splits `source` (the stripped inner text of a `${{ }}`) into tokens.
pub fn lex(source: &str) -> Result<Vec<Token>, ExprError> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        let start = at;
        let kind = if is_space(c) {
            while at < chars.len() && is_space(chars[at]) {
                at += 1;
            }
            None
        } else if c.is_ascii_digit() {
            at = digits(&chars, at);
            if chars.get(at) == Some(&'.') && chars.get(at + 1).is_some_and(char::is_ascii_digit) {
                at = digits(&chars, at + 1);
            }
            Some(TokenKind::Number)
        } else if c == '\'' || c == '"' {
            match string_end(&chars, at) {
                Some(end) => {
                    at = end;
                    Some(TokenKind::Str)
                }
                None => return Err(unexpected(c, at, source)),
            }
        } else if let Some(op) = operator(&chars, at) {
            at += op;
            Some(TokenKind::Op)
        } else if is_name_start(c) {
            at += 1;
            while at < chars.len() && is_name_char(chars[at]) {
                at += 1;
            }
            Some(TokenKind::Name)
        } else {
            return Err(unexpected(c, at, source));
        };
        if let Some(kind) = kind {
            tokens.push(Token {
                kind,
                text: chars[start..at].iter().collect(),
                at: start,
            });
        }
    }
    tokens.push(Token {
        kind: TokenKind::End,
        text: String::new(),
        at: chars.len(),
    });
    Ok(tokens)
}

fn digits(chars: &[char], mut at: usize) -> usize {
    while at < chars.len() && chars[at].is_ascii_digit() {
        at += 1;
    }
    at
}

/// The end (one past the closing quote) of the string starting at `at`: any character but the
/// quote or a backslash, or a backslash and any character but a line feed (regex `.` without
/// DOTALL). `None` when the string never closes.
fn string_end(chars: &[char], at: usize) -> Option<usize> {
    let quote = chars[at];
    let mut i = at + 1;
    while i < chars.len() {
        match chars[i] {
            c if c == quote => return Some(i + 1),
            '\\' => match chars.get(i + 1) {
                Some(&next) if next != '\n' => i += 2,
                _ => return None,
            },
            _ => i += 1,
        }
    }
    None
}

/// The length of the operator at `at`, if one starts there.
fn operator(chars: &[char], at: usize) -> Option<usize> {
    if let Some(&next) = chars.get(at + 1) {
        let pair: String = [chars[at], next].iter().collect();
        if OPS2.contains(&pair.as_str()) {
            return Some(2);
        }
    }
    OPS1.contains(chars[at]).then_some(1)
}

fn unexpected(c: char, at: usize, source: &str) -> ExprError {
    ExprError::new(format!(
        "unexpected {} at {at} in {}",
        py_repr_str(&c.to_string()),
        py_repr_str(source)
    ))
}

/// A string token's value: quotes dropped, every `\X` replaced by `X` (no C escapes).
pub fn unquote(token_text: &str) -> String {
    let mut chars = token_text.chars();
    chars.next();
    chars.next_back();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // A lexed string never ends on a lone backslash; keep it if one does.
            out.push(chars.next().unwrap_or('\\'));
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<(TokenKind, String, usize)> {
        lex(source)
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, t.text, t.at))
            .collect()
    }

    #[test]
    fn tokens_and_positions() {
        use TokenKind::*;
        assert_eq!(
            kinds("a.b >= 1.5&&'x'"),
            vec![
                (Name, "a".into(), 0),
                (Op, ".".into(), 1),
                (Name, "b".into(), 2),
                (Op, ">=".into(), 4),
                (Number, "1.5".into(), 7),
                (Op, "&&".into(), 10),
                (Str, "'x'".into(), 12),
                (End, "".into(), 15),
            ]
        );
        // `1.` is a number and a dot; `1e3` a number and a name.
        assert_eq!(
            kinds("1."),
            vec![
                (Number, "1".into(), 0),
                (Op, ".".into(), 1),
                (End, "".into(), 2)
            ]
        );
        assert_eq!(kinds("1e3")[1], (Name, "e3".into(), 1));
        // Positions count characters, not bytes.
        assert_eq!(kinds("'é' + x")[2], (Name, "x".into(), 6));
    }

    #[test]
    fn strings() {
        assert_eq!(unquote(r"'a\nb'"), "anb");
        assert_eq!(unquote(r"'a\\b'"), r"a\b");
        assert_eq!(unquote(r"'it\'s'"), "it's");
        assert_eq!(unquote("\"it's\""), "it's");
        assert_eq!(unquote("'a\nb'"), "a\nb");
        assert_eq!(kinds("'a\nb'")[0].1, "'a\nb'");
        // A backslash before a line feed ends the string alternative: the error is at the quote.
        assert!(lex("'a\\\nb'").unwrap_err().0.starts_with("unexpected "));
        assert!(lex("'a\\\nb'").unwrap_err().0.contains(" at 0 in "));
    }

    #[test]
    fn ascii_only() {
        // Unicode digits and spaces are not part of the language.
        assert!(lex("١٢").unwrap_err().0.contains(" at 0 in "));
        assert!(lex("1\u{a0}+ 1").unwrap_err().0.contains(" at 1 in "));
        assert_eq!(kinds("1\u{0B}+\u{0C}1").len(), 4);
    }
}
