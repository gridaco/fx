//! The strict YAML subset FX reads (spec/yaml.md), built on yaml-rust2's event parser.
//!
//! [`load`] turns a stream into one FX value ([`serde_json::Value`], numbers normalized by
//! [`crate::value::number`] / [`crate::value::integer_literal`]) or refuses it with a
//! [`YamlError`] carrying the file label, line and column. Every rule of yaml.md is enforced
//! here: encoding (UTF-8, one BOM), U+0085/U+2028/U+2029 refused, one document, empty stream is
//! `{}`, no anchors/aliases/merge keys/tags/directives/complex keys/collection keys, keys are
//! strings, no duplicate keys, plain scalars resolved by [`resolve::resolve_plain`], quoted and
//! block scalars always strings. The reserved-marker rule (identity.md §3) is NOT applied here;
//! callers that read user data apply [`crate::value::check_markers`].
//!
//! The checks run in four passes, so that a stream breaking several rules is refused for the same
//! one by every implementation (`tools/digest.py` holds an independent one):
//! 1. the characters: UTF-8, the byte-order mark, the refused line breaks, then YAML's printable
//!    set (YAML 1.2 §5.1; yaml-rust2 would read a NUL as the end of the stream);
//! 2. the tokens of the whole stream: directives and explicit (`? `) keys, and YAML syntax;
//! 3. the events of the whole stream: a second document, aliases, anchors and tags;
//! 4. the values, in document order: nesting depth, collection keys, empty keys, merge keys,
//!    duplicate keys and plain scalars.
//!
//! A value nested inside more than [`crate::value::MAX_DEPTH`] (512) collections is refused as
//! `invalid_yaml`, as the JSON reader refuses it: everything downstream walks values recursively.
//!
//! The subset is YAML 1.2's syntax as yaml-rust2 reads it, so a few streams that YAML 1.1 readers
//! refuse load (a tab separating tokens, a `...` before the only document, `?` inside a flow
//! plain scalar), and a few they accept are refused (a `#` right after a quoted scalar, an
//! unindented continuation line of a quoted scalar). An empty plain key (`: v`), which YAML 1.2
//! reads as null and YAML 1.1 refuses, is refused.
//!
//! Tests: `tests/yaml_vectors.rs` runs every vector in `spec/vectors/yaml/` (accept: compare by
//! `canon`; refuse: must be refused, for the rule its `.txt` names) plus unit tests for line and
//! column reporting.

pub mod resolve;
pub mod write;

use crate::error::{Error, ErrorKind};
use crate::text::py_repr_str;
use serde_json::{Map, Value};
use std::fmt;
use std::path::Path;
use yaml_rust2::parser::{Event, Parser};
use yaml_rust2::scanner::{Marker, ScanError, Scanner, TScalarStyle, TokenType};

pub use write::{needs_quotes, write_yaml};

/// A refused YAML stream. `file` is the label the caller gave (a project-relative path or the
/// name the user typed); `line` and `column` are 1-based, 0 when the rule concerns the whole
/// stream (encoding). `rule` names the yaml.md rule the refusal applies:
///
/// | `rule` | yaml.md |
/// |---|---|
/// | `invalid_utf8` | Encoding: a stream is UTF-8 |
/// | `byte_order_mark` | Encoding: a second byte-order mark |
/// | `line_break` | Line breaks: U+0085, U+2028, U+2029 |
/// | `multiple_documents` | One document |
/// | `directive` | Not supported: directives |
/// | `anchor`, `alias` | Not supported: anchors and aliases |
/// | `tag` | Not supported: tags |
/// | `merge_key` | Not supported: merge keys |
/// | `complex_key` | Not supported: complex keys (`? `) |
/// | `collection_key` | Not supported: a key that is itself a collection |
/// | `duplicate_key` | Mapping keys: no duplicates |
/// | `empty_key` | Mapping keys: keys are strings (an empty plain key has no text) |
/// | `ambiguous_scalar` | Values: an ambiguous plain scalar; quote it |
/// | `integer_out_of_range` | Values: a decimal integer that is not the canonical form of the number it reads as (reading would round it, or it is exact but written another way) |
/// | `number_overflow` | Values: a decimal float that overflows to infinity |
/// | `invalid_yaml` | not YAML 1.2 at all (a syntax error, a non-printable character), or nested more than 512 deep |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlError {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub rule: &'static str,
    pub message: String,
}

impl fmt::Display for YamlError {
    /// `<file>:<line>:<column>: <message>`; without a position, `<file>: <message>`. An ambiguous
    /// scalar's message ends with `; quote it` (yaml.md "Values").
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "{}: {}", self.file, self.message)
        } else {
            write!(
                f,
                "{}:{}:{}: {}",
                self.file, self.line, self.column, self.message
            )
        }
    }
}

impl std::error::Error for YamlError {}

impl From<YamlError> for Error {
    fn from(error: YamlError) -> Self {
        Error::new(ErrorKind::Yaml, error.to_string())
    }
}

/// Loads one YAML stream in the strict subset. `file` labels errors.
pub fn load(bytes: &[u8], file: &str) -> Result<Value, YamlError> {
    let loader = Loader { file };
    let text = loader.characters(bytes)?;
    if !loader.tokens(text)? {
        // Nothing but comments, blank lines and `...` end markers.
        return Ok(Value::Object(Map::new()));
    }
    let events = loader.events(text)?;
    loader.build(events)
}

/// Reads and loads a file. `label` is how messages name it. A missing or unreadable file is an
/// [`ErrorKind::Io`] error; a refused stream is an [`ErrorKind::Yaml`] error.
pub fn load_file(path: &Path, label: &str) -> Result<Value, Error> {
    let bytes = std::fs::read(path).map_err(|error| Error::io(label, &error))?;
    Ok(load(&bytes, label)?)
}

/// One `load` call: the label its errors carry.
struct Loader<'a> {
    file: &'a str,
}

/// A 1-based position in the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Position {
    line: usize,
    column: usize,
}

impl From<Marker> for Position {
    fn from(mark: Marker) -> Self {
        // yaml-rust2 counts lines from 1 and columns from 0, both in characters.
        Self {
            line: mark.line(),
            column: mark.col() + 1,
        }
    }
}

/// A node event of the stream, kept for the value pass.
enum Node {
    Scalar(String, TScalarStyle),
    SequenceStart,
    MappingStart,
    End,
}

/// A collection being built by the value pass.
enum Open {
    Sequence(Vec<Value>),
    Mapping {
        map: Map<String, Value>,
        /// The key read and waiting for its value.
        key: Option<String>,
    },
}

impl Loader<'_> {
    fn error(&self, at: Option<Position>, rule: &'static str, message: String) -> YamlError {
        let (line, column) = at.map_or((0, 0), |at| (at.line, at.column));
        YamlError {
            file: self.file.to_string(),
            line,
            column,
            rule,
            message,
        }
    }

    fn syntax(&self, error: &ScanError) -> YamlError {
        let at = Position::from(*error.marker());
        // A marker yaml-rust2 never placed has line 0: report the stream as a whole.
        let at = (at.line > 0).then_some(at);
        self.error(at, "invalid_yaml", error.info().to_string())
    }

    /// Pass 1: the characters. Returns the text without its byte-order mark.
    fn characters<'b>(&self, bytes: &'b [u8]) -> Result<&'b str, YamlError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| self.error(None, "invalid_utf8", "is not UTF-8".into()))?;
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
        if text.starts_with('\u{FEFF}') {
            return Err(self.error(
                Some(Position { line: 1, column: 1 }),
                "byte_order_mark",
                "a second byte-order mark".into(),
            ));
        }
        let at = |index: usize| position_of(text, index);
        if let Some((index, c)) = text
            .char_indices()
            .find(|(_, c)| matches!(c, '\u{85}' | '\u{2028}' | '\u{2029}'))
        {
            return Err(self.error(
                Some(at(index)),
                "line_break",
                format!(
                    "U+{:04X} is refused anywhere in a stream (YAML 1.1 reads it as a line \
                     break, YAML 1.2 as content)",
                    c as u32
                ),
            ));
        }
        if let Some((index, c)) = text.char_indices().find(|(_, c)| !is_yaml_printable(*c)) {
            return Err(self.error(
                Some(at(index)),
                "invalid_yaml",
                format!(
                    "U+{:04X} is not a printable character, which a YAML stream holds only \
                     escaped in double quotes",
                    c as u32
                ),
            ));
        }
        Ok(text)
    }

    /// Pass 2: the tokens. Refuses directives and explicit keys; returns whether the stream has
    /// any content (a token other than the stream's start and end and `...` end markers).
    fn tokens(&self, text: &str) -> Result<bool, YamlError> {
        let lines = LineTable::new(text);
        let mut scanner = Scanner::new(text.chars());
        let mut content = false;
        loop {
            let token = match scanner.next_token() {
                Ok(Some(token)) => token,
                Ok(None) => return Ok(content),
                Err(error) => return Err(self.syntax(&error)),
            };
            let at = Position::from(token.0);
            match token.1 {
                TokenType::StreamStart(_) | TokenType::DocumentEnd => {}
                TokenType::StreamEnd => return Ok(content),
                TokenType::VersionDirective(..) | TokenType::TagDirective(..) => {
                    return Err(self.error(
                        Some(at),
                        "directive",
                        format!(
                            "directives ({}) are not supported",
                            lines.directive_name(at).unwrap_or("%")
                        ),
                    ));
                }
                // An explicit key's token is its `?` indicator, followed by a blank or a line
                // break; an implicit key's token is the key's first character, and a plain
                // scalar starting with `?` has a non-blank after it.
                TokenType::Key if lines.is_explicit_key(at) => {
                    return Err(self.error(
                        Some(at),
                        "complex_key",
                        "complex keys (? ) are not supported".into(),
                    ));
                }
                _ => content = true,
            }
        }
    }

    /// Pass 3: the events. Refuses a second document, aliases, anchors and tags; returns the node
    /// events of the one document with their positions.
    fn events(&self, text: &str) -> Result<Vec<(Node, Position)>, YamlError> {
        let mut parser = Parser::new_from_str(text);
        let mut nodes = Vec::new();
        let mut documents = 0;
        // Where the last `...` end marker was.
        let mut end_marker = None;
        loop {
            let (event, mark) = match parser.next_token() {
                Ok(next) => next,
                // yaml-rust2 refuses an alias of an unknown anchor while parsing; any alias is
                // refused, and this is where the first one is.
                Err(error) if error.info() == "while parsing node, found unknown anchor" => {
                    return Err(self.error(
                        Some(Position::from(*error.marker())),
                        "alias",
                        "aliases are not supported".into(),
                    ));
                }
                Err(error) => return Err(self.syntax(&error)),
            };
            let at = Position::from(mark);
            let (anchor, tag) = match &event {
                Event::Scalar(_, _, anchor, tag) => (*anchor, tag.is_some()),
                Event::SequenceStart(anchor, tag) | Event::MappingStart(anchor, tag) => {
                    (*anchor, tag.is_some())
                }
                _ => (0, false),
            };
            if anchor != 0 || tag {
                // Properties are refused where they start, at `&name` or `!tag` (the event's
                // marker is the node's content). Each anchor or tag token makes one node event,
                // so the first node event with an anchor is the first anchor token, and the same
                // for tags.
                let (first_anchor, first_tag) = first_properties(text);
                let start = [
                    first_anchor.filter(|_| anchor != 0),
                    first_tag.filter(|_| tag),
                ]
                .into_iter()
                .flatten()
                .min_by_key(|p| (p.line, p.column))
                .unwrap_or(at);
                return Err(if anchor != 0 {
                    self.error(Some(start), "anchor", "anchors are not supported".into())
                } else {
                    self.error(Some(start), "tag", "tags are not supported".into())
                });
            }
            match event {
                Event::StreamEnd => return Ok(nodes),
                Event::DocumentStart => {
                    documents += 1;
                    if documents > 1 {
                        let start = LineTable::new(text).document_start(at, end_marker);
                        return Err(self.error(
                            Some(start),
                            "multiple_documents",
                            "a second document".into(),
                        ));
                    }
                }
                Event::DocumentEnd => end_marker = Some(at),
                Event::Alias(_) => {
                    return Err(self.error(Some(at), "alias", "aliases are not supported".into()));
                }
                Event::Scalar(text, style, ..) => nodes.push((Node::Scalar(text, style), at)),
                Event::SequenceStart(..) => nodes.push((Node::SequenceStart, at)),
                Event::MappingStart(..) => nodes.push((Node::MappingStart, at)),
                Event::SequenceEnd | Event::MappingEnd => nodes.push((Node::End, at)),
                Event::Nothing | Event::StreamStart => {}
            }
        }
    }

    /// Refuses a value inside more than [`crate::value::MAX_DEPTH`] collections; `depth` is how
    /// many collections are open around the value at `at`.
    fn nesting(&self, depth: usize, at: Position) -> Result<(), YamlError> {
        if depth > crate::value::MAX_DEPTH {
            return Err(self.error(
                Some(at),
                "invalid_yaml",
                format!(
                    "nested too deeply: a value inside more than {} collections",
                    crate::value::MAX_DEPTH
                ),
            ));
        }
        Ok(())
    }

    /// Pass 4: the values, in document order, without recursion. A value nests at most
    /// [`crate::value::MAX_DEPTH`] deep, so what reads the result recursively stays bounded.
    fn build(&self, nodes: Vec<(Node, Position)>) -> Result<Value, YamlError> {
        let mut stack: Vec<Open> = Vec::new();
        let mut nodes = nodes.into_iter();
        // A document with no content is an empty plain scalar: the empty mapping. A written `~`
        // or `null` is not empty, and stays null.
        let root = loop {
            let Some((node, at)) = nodes.next() else {
                // No document at all (a stream of `...` markers).
                return Ok(Value::Object(Map::new()));
            };
            let value = match node {
                Node::SequenceStart | Node::MappingStart => {
                    if let Some(Open::Mapping { key: None, .. }) = stack.last() {
                        return Err(self.error(
                            Some(at),
                            "collection_key",
                            "a key must be a string, not a collection".into(),
                        ));
                    }
                    self.nesting(stack.len(), at)?;
                    stack.push(match node {
                        Node::SequenceStart => Open::Sequence(Vec::new()),
                        _ => Open::Mapping {
                            map: Map::new(),
                            key: None,
                        },
                    });
                    continue;
                }
                Node::End => match stack.pop() {
                    Some(Open::Sequence(items)) => Value::Array(items),
                    Some(Open::Mapping { map, .. }) => Value::Object(map),
                    None => continue,
                },
                Node::Scalar(text, style) => {
                    if let Some(Open::Mapping { map, key }) = stack.last_mut()
                        && key.is_none()
                    {
                        // A key is its text, whatever its style. An empty plain key
                        // (`: v`) has no text: YAML 1.2 reads it as null, and YAML 1.1
                        // readers refuse it.
                        if style == TScalarStyle::Plain && text.is_empty() {
                            return Err(self.error(
                                Some(at),
                                "empty_key",
                                "a key cannot be empty; write \"\" for the empty string".into(),
                            ));
                        }
                        if style == TScalarStyle::Plain && text == "<<" {
                            return Err(self.error(
                                Some(at),
                                "merge_key",
                                "merge keys are not supported".into(),
                            ));
                        }
                        if map.contains_key(&text) {
                            return Err(self.error(
                                Some(at),
                                "duplicate_key",
                                format!("duplicate key {}", py_repr_str(&text)),
                            ));
                        }
                        *key = Some(text);
                        continue;
                    }
                    self.nesting(stack.len(), at)?;
                    if style != TScalarStyle::Plain {
                        Value::String(text)
                    } else if stack.is_empty() && text.is_empty() {
                        Value::Object(Map::new())
                    } else {
                        resolve::resolve_plain(&text).map_err(|refused| {
                            self.error(Some(at), refused.rule, refused.message)
                        })?
                    }
                }
            };
            match stack.last_mut() {
                None => break value,
                Some(Open::Sequence(items)) => items.push(value),
                Some(Open::Mapping { map, key }) => {
                    if let Some(key) = key.take() {
                        map.insert(key, value);
                    }
                }
            }
        };
        Ok(root)
    }
}

/// YAML 1.2's printable characters (`c-printable`): tab, line feed, carriage return, the
/// printable ASCII range, U+0085, and U+00A0 up, apart from surrogates, U+FFFE and U+FFFF.
fn is_yaml_printable(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\r' | ' '..='~' | '\u{85}' | '\u{A0}'..='\u{FFFD}' | '\u{10000}'..
    )
}

/// The 1-based line and column (in characters) of the byte `index` of `text`. Lines end at `\n`,
/// `\r\n` or a lone `\r`, as YAML's line breaks do.
fn position_of(text: &str, index: usize) -> Position {
    let before = &text[..index];
    let mut line = 1;
    let mut line_start = 0;
    let bytes = before.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                i += 2;
                line += 1;
                line_start = i;
            }
            b'\r' | b'\n' => {
                i += 1;
                line += 1;
                line_start = i;
            }
            _ => i += 1,
        }
    }
    Position {
        line,
        column: before[line_start..].chars().count() + 1,
    }
}

/// The positions of the first anchor (`&name`) and the first tag (`!tag`) tokens, if any.
fn first_properties(text: &str) -> (Option<Position>, Option<Position>) {
    let (mut anchor, mut tag) = (None, None);
    for token in Scanner::new(text.chars()) {
        match token.1 {
            TokenType::Anchor(_) if anchor.is_none() => anchor = Some(Position::from(token.0)),
            TokenType::Tag(..) if tag.is_none() => tag = Some(Position::from(token.0)),
            _ => {}
        }
        if anchor.is_some() && tag.is_some() {
            break;
        }
    }
    (anchor, tag)
}

/// The stream's lines, to look at the characters at a token's position.
struct LineTable<'a> {
    lines: Vec<&'a str>,
}

impl<'a> LineTable<'a> {
    fn new(text: &'a str) -> Self {
        let mut lines = Vec::new();
        let bytes = text.as_bytes();
        let mut start = 0;
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\r' | b'\n' => {
                    lines.push(&text[start..i]);
                    i += if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                        2
                    } else {
                        1
                    };
                    start = i;
                }
                _ => i += 1,
            }
        }
        lines.push(&text[start..]);
        Self { lines }
    }

    /// The rest of the line from `at`.
    fn rest(&self, at: Position) -> Option<&'a str> {
        let line = self.lines.get(at.line.checked_sub(1)?)?;
        let offset = line
            .char_indices()
            .nth(at.column.saturating_sub(1))
            .map_or(line.len(), |(i, _)| i);
        Some(&line[offset..])
    }

    /// Whether a key token at `at` is a `?` indicator: `?` followed by a blank or the line's end.
    fn is_explicit_key(&self, at: Position) -> bool {
        self.rest(at).is_some_and(|rest| {
            rest.strip_prefix('?')
                .is_some_and(|after| after.is_empty() || after.starts_with([' ', '\t']))
        })
    }

    /// Where a document whose start event is at `at` starts. An explicit document starts at its
    /// `---`. An implicit one follows a `...` end marker, and starts at the first character after
    /// it that is not white space or a comment (yaml-rust2 places the start of a block mapping at
    /// its first `:`).
    fn document_start(&self, at: Position, end_marker: Option<Position>) -> Position {
        let explicit = at.column == 1 && self.rest(at).is_some_and(|rest| rest.starts_with("---"));
        let Some(end) = end_marker.filter(|_| !explicit) else {
            return at;
        };
        let Some(rest) = self.rest(end).and_then(|rest| rest.strip_prefix("...")) else {
            return at;
        };
        let mut first = Some((end.column + 3, rest));
        for line in end.line.. {
            let (column, text) = match first.take() {
                Some(start) => start,
                None => match self.lines.get(line - 1) {
                    Some(text) => (1, *text),
                    None => return at,
                },
            };
            for (i, c) in text.chars().enumerate() {
                match c {
                    ' ' | '\t' => {}
                    '#' => break,
                    _ => {
                        return Position {
                            line,
                            column: column + i,
                        };
                    }
                }
            }
        }
        at
    }

    /// The `%NAME` of a directive at `at`.
    fn directive_name(&self, at: Position) -> Option<&'a str> {
        let rest = self.rest(at)?;
        let end = rest.find([' ', '\t']).unwrap_or(rest.len());
        Some(&rest[..end]).filter(|name| name.starts_with('%'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn refused(text: &str) -> YamlError {
        load(text.as_bytes(), "f.yaml").expect_err(text)
    }

    fn at(error: &YamlError) -> (usize, usize, &'static str) {
        (error.line, error.column, error.rule)
    }

    #[test]
    fn duplicate_key_position() {
        let error = refused("name: acme\nname: img-a\n");
        assert_eq!(at(&error), (2, 1, "duplicate_key"));
        assert_eq!(error.to_string(), "f.yaml:2:1: duplicate key 'name'");
        let error = refused("image:\n  size: 512\n  size: 512\n");
        assert_eq!(at(&error), (3, 3, "duplicate_key"));
        let error = refused("image: {size: 512, size: 1024}\n");
        assert_eq!(at(&error), (1, 20, "duplicate_key"));
        let error = refused("1: first\n\"1\": second\n");
        assert_eq!(at(&error), (2, 1, "duplicate_key"));
        assert_eq!(error.message, "duplicate key '1'");
        let error = refused("\"it's\": 1\n'it''s': 2\n");
        assert_eq!(error.message, "duplicate key \"it's\"");
    }

    #[test]
    fn ambiguous_scalar_position() {
        let error = refused("enabled: yes\n");
        assert_eq!(at(&error), (1, 10, "ambiguous_scalar"));
        assert_eq!(
            error.to_string(),
            "f.yaml:1:10: plain scalar 'yes' is ambiguous (a word that YAML 1.1 or another \
             letter case reads as a boolean or null); quote it"
        );
        assert_eq!(
            at(&refused("flags:\n  - fast\n  - No\n")),
            (3, 5, "ambiguous_scalar")
        );
        assert_eq!(
            at(&refused("flags: [fast, off]\n")),
            (1, 15, "ambiguous_scalar")
        );
        assert_eq!(
            at(&refused("a:\n  b: [1, {c: 017}]\n")),
            (2, 14, "ambiguous_scalar")
        );
        // Columns count characters, not bytes.
        assert_eq!(at(&refused("é: [ü, on]\n")), (1, 8, "ambiguous_scalar"));
        // A leading byte-order mark is not a column.
        assert_eq!(at(&refused("\u{FEFF}a: on\n")), (1, 4, "ambiguous_scalar"));
        // CRLF line endings count as one line break.
        assert_eq!(
            at(&refused("a: 1\r\nb: on\r\n")),
            (2, 4, "ambiguous_scalar")
        );
        // A refused top-level scalar.
        assert_eq!(at(&refused("--- 017\n")), (1, 5, "ambiguous_scalar"));
        assert_eq!(at(&refused("x: 1e400\n")), (1, 4, "number_overflow"));
        assert_eq!(
            at(&refused("x: 9007199254740993\n")),
            (1, 4, "integer_out_of_range")
        );
    }

    #[test]
    fn values_come_first_in_document_order() {
        // The value of the first key is read before the second key is checked.
        assert_eq!(at(&refused("a: yes\na: 1\n")), (1, 4, "ambiguous_scalar"));
        assert_eq!(at(&refused("a: 1\na: yes\n")), (2, 1, "duplicate_key"));
    }

    #[test]
    fn events_before_values() {
        // Anchors, tags and documents are checked over the whole stream before any value.
        assert_eq!(at(&refused("a: on\nb: &x 1\n")), (2, 4, "anchor"));
        assert_eq!(at(&refused("a: on\nb: !!str 1\n")), (2, 4, "tag"));
        assert_eq!(
            at(&refused("a: on\n---\nb: 1\n")),
            (2, 1, "multiple_documents")
        );
        // Tokens before events.
        assert_eq!(at(&refused("a: &x 1\n? b\n: 2\n")), (2, 1, "complex_key"));
    }

    #[test]
    fn properties_are_refused_where_they_start() {
        assert_eq!(
            at(&refused("base: &size 512\nwidth: *size\n")),
            (1, 7, "anchor")
        );
        assert_eq!(at(&refused("base: &base\n  size: 512\n")), (1, 7, "anchor"));
        assert_eq!(at(&refused("size: !!str 512\n")), (1, 7, "tag"));
        assert_eq!(at(&refused("sizes: !!seq [512]\n")), (1, 8, "tag"));
        assert_eq!(at(&refused("a: !t &x 1\n")), (1, 4, "anchor"));
        assert_eq!(at(&refused("a: &x !t 1\n")), (1, 4, "anchor"));
        assert_eq!(at(&refused("a: !t\n")), (1, 4, "tag"));
        assert_eq!(at(&refused("&x a: 1\n")), (1, 1, "anchor"));
        assert_eq!(at(&refused("a: *nowhere\n")), (1, 4, "alias"));
        assert_eq!(at(&refused("- ! x\n")), (1, 3, "tag"));
    }

    #[test]
    fn explicit_keys_and_lookalikes() {
        assert_eq!(at(&refused("? name\n: acme\n")), (1, 1, "complex_key"));
        assert_eq!(at(&refused("? [w, h]\n: size\n")), (1, 1, "complex_key"));
        assert_eq!(at(&refused("a:\n  ?\n  : x\n")), (2, 3, "complex_key"));
        assert_eq!(at(&refused("{? a: 1}\n")), (1, 2, "complex_key"));
        assert_eq!(at(&refused("[? a : 1]\n")), (1, 2, "complex_key"));
        // yaml-rust2 refuses a tab after `?` as a syntax error.
        assert_eq!(refused("?\tx: 1\n").line, 1);
        // A plain scalar may start with `?` when a non-blank follows.
        assert_eq!(load(b"?a: 1\n", "f").unwrap(), json!({"?a": 1}));
        assert_eq!(load(b"x: ?a\n", "f").unwrap(), json!({"x": "?a"}));
        assert_eq!(load(b"[?a]\n", "f").unwrap(), json!(["?a"]));
        // A single-pair mapping in a flow sequence has an implicit key.
        assert_eq!(load(b"[a: 1]\n", "f").unwrap(), json!([{"a": 1}]));
    }

    #[test]
    fn collection_keys() {
        assert_eq!(
            at(&refused("{width: 512}: size\n")),
            (1, 1, "collection_key")
        );
        assert_eq!(at(&refused("[w, h]: size\n")), (1, 1, "collection_key"));
        assert_eq!(
            at(&refused("a:\n  [w, h]: size\n")),
            (2, 3, "collection_key")
        );
        assert_eq!(at(&refused("[[a]: 1]\n")), (1, 2, "collection_key"));
    }

    #[test]
    fn merge_keys() {
        assert_eq!(
            at(&refused("image:\n  <<: {size: 1024}\n  name: img-a\n")),
            (2, 3, "merge_key")
        );
        assert_eq!(at(&refused("{<<: 1}\n")), (1, 2, "merge_key"));
        assert_eq!(
            load(b"\"<<\": 1\nb: {'<<': 2}\nc: <<\n", "f").unwrap(),
            json!({"<<": 1, "b": {"<<": 2}, "c": "<<"})
        );
    }

    #[test]
    fn directives() {
        let error = refused("%YAML 1.2\n---\nname: acme\n");
        assert_eq!(at(&error), (1, 1, "directive"));
        assert_eq!(error.message, "directives (%YAML) are not supported");
        assert_eq!(
            refused("%TAG ! tag:acme.test,2026:\n---\na: 1\n").message,
            "directives (%TAG) are not supported"
        );
        assert_eq!(
            refused("%FOO bar\n---\na: 1\n").message,
            "directives (%FOO) are not supported"
        );
        // A `%` inside a value is content.
        assert_eq!(load(b"a: 50%\n", "f").unwrap(), json!({"a": "50%"}));
    }

    #[test]
    fn documents() {
        assert_eq!(
            at(&refused("name: acme\n---\nname: img-a\n")),
            (2, 1, "multiple_documents")
        );
        assert_eq!(
            at(&refused("---\nname: acme\n---\nname: img-a\n")),
            (3, 1, "multiple_documents")
        );
        assert_eq!(
            at(&refused("a: 1\n...\nb: 2\n")),
            (3, 1, "multiple_documents")
        );
        assert_eq!(
            at(&refused("a: 1\n...\n---\n")),
            (3, 1, "multiple_documents")
        );
        assert_eq!(
            at(&refused("a: 1\n... # end\n\n  # note\n  b: 2\n")),
            (5, 3, "multiple_documents")
        );
        assert_eq!(refused("a: 1\n... - x\n").rule, "invalid_yaml");
        assert_eq!(
            at(&refused("a: 1\n...\n- x\n")),
            (3, 1, "multiple_documents")
        );
        assert_eq!(at(&refused("a: 1\n...\nx\n")), (3, 1, "multiple_documents"));
        assert_eq!(load(b"...\n...\n", "f").unwrap(), json!({}));
        assert_eq!(load(b"...\n---\na: 1\n", "f").unwrap(), json!({"a": 1}));
        assert_eq!(load(b"a: 1\n...\n...\n", "f").unwrap(), json!({"a": 1}));
        assert_eq!(load(b"--- ''\n", "f").unwrap(), json!(""));
        assert_eq!(load(b"--- null\n", "f").unwrap(), json!(null));
        assert_eq!(load(b"---\n...\n", "f").unwrap(), json!({}));
        assert_eq!(load(b"--- |\n  x\n", "f").unwrap(), json!("x\n"));
    }

    #[test]
    fn encoding_and_characters() {
        let error = refused("name: acme\u{0}");
        assert_eq!(at(&error), (1, 11, "invalid_yaml"));
        let error = load(b"name: acme\xff", "f.yaml").unwrap_err();
        assert_eq!(at(&error), (0, 0, "invalid_utf8"));
        assert_eq!(error.to_string(), "f.yaml: is not UTF-8");
        let error = load(b"\xEF\xBB\xBF\xEF\xBB\xBFa: 1", "f.yaml").unwrap_err();
        assert_eq!(at(&error), (1, 1, "byte_order_mark"));
        let error = refused("a: 1\r\nb: x\u{2028}y\n");
        assert_eq!(at(&error), (2, 5, "line_break"));
        assert!(
            error
                .message
                .starts_with("U+2028 is refused anywhere in a stream")
        );
        assert_eq!(at(&refused("a: 1\rb: \u{85}\n")), (2, 4, "line_break"));
        assert_eq!(at(&refused("# \u{7f}\n")), (1, 3, "invalid_yaml"));
        assert_eq!(at(&refused("a: \"\u{9f}\"\n")), (1, 5, "invalid_yaml"));
        assert_eq!(at(&refused("a: \u{fffe}\n")), (1, 4, "invalid_yaml"));
        // A byte-order mark inside the stream is content, as in other YAML readers.
        assert_eq!(
            load("a: \"x\u{FEFF}\"\n".as_bytes(), "f").unwrap(),
            json!({"a": "x\u{FEFF}"})
        );
        // Escapes in double quotes may name any character.
        assert_eq!(
            load(b"a: \"\\u2028\\x85\\0\\u0001\"\n", "f").unwrap(),
            json!({"a": "\u{2028}\u{85}\u{0}\u{1}"})
        );
    }

    #[test]
    fn syntax_errors_have_positions() {
        let error = refused("a: b: c\n");
        assert_eq!(error.rule, "invalid_yaml");
        assert!(error.line >= 1, "{error}");
        let error = refused("a: [1, 2\n");
        assert_eq!(error.rule, "invalid_yaml");
        assert!(error.to_string().starts_with("f.yaml:"), "{error}");
    }

    #[test]
    fn keys_are_text() {
        assert_eq!(
            load(b"on: 1\n1: 2\ntrue: 3\n~: 4\n'': 5\n1.0: 6\n", "f").unwrap(),
            json!({"on": 1, "1": 2, "true": 3, "~": 4, "": 5, "1.0": 6})
        );
        // An empty plain key has no text.
        assert_eq!(refused("a: 1\n: v\n").rule, "empty_key");
        assert_eq!(refused("a: 1\n: v\n").line, 2);
        assert_eq!(refused("{: 1}\n").rule, "empty_key");
        assert_eq!(refused("[: 1]\n").rule, "empty_key");
        assert_eq!(refused("a:\n  :\n    c: 1\n").rule, "empty_key");
        // An empty value is null.
        assert_eq!(
            load(b"{a, b: }\n", "f").unwrap(),
            json!({"a": null, "b": null})
        );
        // Keys keep their order.
        let value = load(b"b: 1\na: 2\nc: 3\n", "f").unwrap();
        let keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["b", "a", "c"]);
    }

    /// Runs `work` on a thread with the 2 MB stack Rust gives a spawned thread by default.
    fn on_small_stack(work: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(work)
            .unwrap()
            .join()
            .unwrap();
    }

    const TOO_DEEP: &str = "nested too deeply: a value inside more than 512 collections";

    #[test]
    fn nesting_stops_where_the_json_reader_stops() {
        use crate::value::{canon, check_markers, parse_json, write_json};
        on_small_stack(|| {
            // A value inside 512 collections loads, and is the value the JSON reader reads.
            let deepest = load(format!("{}x\n", "- ".repeat(512)).as_bytes(), "f").unwrap();
            let json = format!("{}\"x\"{}", "[".repeat(512), "]".repeat(512));
            assert_eq!(deepest, parse_json(&json).unwrap());
            // Everything that walks a value recursively gets through it on a small stack.
            check_markers(&deepest, "value").unwrap();
            assert_eq!(canon(&deepest), json);
            let pretty = write_json(&deepest);
            assert_eq!(parse_json(&pretty).unwrap(), deepest);
            let written = write_yaml(&deepest);
            assert_eq!(load(written.as_bytes(), "f").unwrap(), deepest);
            drop(deepest.clone());

            // One more, and both readers refuse it.
            let json = format!("[{json}]");
            assert!(
                parse_json(&json)
                    .unwrap_err()
                    .message
                    .ends_with("nested too deeply")
            );
            let error = refused(&format!("{}x\n", "- ".repeat(513)));
            assert_eq!(at(&error), (1, 1027, "invalid_yaml"));
            assert_eq!(error.to_string(), format!("f.yaml:1:1027: {TOO_DEEP}"));
            // A collection is refused where it starts.
            let error = refused(&format!("{}[]\n", "- ".repeat(513)));
            assert_eq!(at(&error), (1, 1027, "invalid_yaml"));
            let error = refused(&format!("{}- {{}}\n", "- ".repeat(512)));
            assert_eq!(at(&error), (1, 1027, "invalid_yaml"));
            // Mappings nest the same way; keys are not values. `levels` mappings, one per line,
            // the last holding `value`.
            let mappings = |levels: usize, value: &str| {
                let mut text = String::new();
                for level in 0..levels {
                    text.push_str(&"  ".repeat(level));
                    text.push_str("a:");
                    text.push(if level + 1 == levels { ' ' } else { '\n' });
                }
                format!("{text}{value}\n")
            };
            assert!(load(mappings(512, "x").as_bytes(), "f").is_ok());
            assert!(load(mappings(512, "{}").as_bytes(), "f").is_ok());
            assert_eq!(
                at(&refused(&mappings(513, "x"))),
                (513, 1028, "invalid_yaml")
            );
            assert_eq!(
                at(&refused(&mappings(513, "{}"))),
                (513, 1028, "invalid_yaml")
            );

            // Far deeper: refused at the same place, not a crash.
            let error = refused(&format!("{}x\n", "- ".repeat(100_000)));
            assert_eq!(at(&error), (1, 1027, "invalid_yaml"));
        });
    }

    #[test]
    fn flow_nesting_is_bounded_by_the_parser() {
        // yaml-rust2 refuses flow collections nested deeper than 255.
        let text = format!("{}{}", "[".repeat(300), "]".repeat(300));
        assert_eq!(refused(&text).rule, "invalid_yaml");
        let text = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert!(load(text.as_bytes(), "f").is_ok());
    }

    #[test]
    fn load_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let error = load_file(&dir.path().join("missing.yaml"), "missing.yaml").unwrap_err();
        assert_eq!(error.kind, ErrorKind::Io);
        assert_eq!(error.message, "missing.yaml: no such file");
        let path = dir.path().join("on.yaml");
        std::fs::write(&path, "a: on\n").unwrap();
        let error = load_file(&path, "inputs/on.yaml").unwrap_err();
        assert_eq!(error.kind, ErrorKind::Yaml);
        assert!(error.message.starts_with("inputs/on.yaml:1:4: "), "{error}");
        assert!(error.message.ends_with("quote it"), "{error}");
        std::fs::write(&path, "a: 'on'\n").unwrap();
        assert_eq!(load_file(&path, "x").unwrap(), json!({"a": "on"}));
    }
}
