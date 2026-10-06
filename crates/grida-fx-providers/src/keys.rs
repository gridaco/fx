//! Provider keys: the allowlisted loader (spec/providers.md §3).
//!
//! FX reads exactly five key variables, under their providers' usual names ([`KeyName`]), and
//! four base-URL variables ([`ALLOWED`]); an [`Environment`] holds what it found. Values come
//! from the process environment first; the planning project's `.env` file is read only for those
//! nine names and only when the process does not set them; `GRIDA_FX_DISABLE_DOTENV=1` (in the
//! process) turns the file off. A value is never printed,
//! logged, written to a record, an event, an error or a fixture: [`Secret`] prints as
//! `[redacted]`, and only a transport reads it ([`Secret::expose`]).
//!
//! **`.env` rules** ([`parse_dotenv`], [`read_dotenv`]), each refusal naming the variable and the
//! line, never the value:
//! - the file is optional (missing is fine); a symlink is refused (`.env must be a regular file`),
//!   dangling or not, so is anything but a regular file, and so is a file that is not UTF-8
//!   (`.env must be valid UTF-8`); a file that cannot be read is `.env could not be read`;
//! - lines are split on line breaks (those of Python's `str.splitlines`: `\n`, `\r\n`, `\r`,
//!   `\x0b`, `\x0c`, `\x1c`–`\x1e`, `\u{85}`, `\u{2028}`, `\u{2029}`), numbered from 1; blank
//!   lines and lines whose trimmed text starts with `#` are skipped;
//! - an assignment matches `^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*$`; a line that
//!   is not one but starts (after an optional `export`) with an allowlisted name is refused
//!   (`.env contains malformed assignment for <NAME> on line <n>`); names off the allowlist are
//!   skipped without reading their values;
//! - an allowlisted name twice: `.env contains duplicate key: <NAME>`;
//! - a value: empty → `.env contains an empty value for <NAME>`; starting with `"` → a JSON string
//!   literal (`.env contains malformed quoted value for <NAME> on line <n>`); starting with `'` →
//!   must end with `'`, the inside literal; otherwise unquoted: a trailing whitespace-then-`#`
//!   comment is removed, and a remaining `"` or `'` is refused;
//! - a decoded character below U+0020 or equal to U+007F: `.env contains an unsafe value for
//!   <NAME>`.
//!
//! Precedence per name: a non-blank process value (trimmed) wins; else the file's value (trimmed);
//! a blank value counts as unset. The file is read whenever it is not turned off, so a malformed
//! `.env` is refused even when the process sets every name.

use regex::Regex;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, LazyLock};

/// The variable that turns the `.env` file off (credential-free gates set it).
pub const DISABLE_DOTENV: &str = "GRIDA_FX_DISABLE_DOTENV";

/// A key's value. Prints as `[redacted]`; compared by value.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Arc<str>);

impl Secret {
    pub fn new(value: &str) -> Secret {
        Secret(Arc::from(value))
    }

    /// The value, for a transport to put on the wire and for redaction. Nothing else reads it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// The allowlisted key variables, one per provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyName {
    OpenAi,
    OpenRouter,
    Fal,
    Tripo,
    ElevenLabs,
}

impl KeyName {
    pub const ALL: [KeyName; 5] = [
        KeyName::OpenAi,
        KeyName::OpenRouter,
        KeyName::Fal,
        KeyName::Tripo,
        KeyName::ElevenLabs,
    ];

    /// The environment variable.
    pub fn variable(self) -> &'static str {
        match self {
            KeyName::OpenAi => "OPENAI_API_KEY",
            KeyName::OpenRouter => "OPENROUTER_API_KEY",
            KeyName::Fal => "FAL_KEY",
            KeyName::Tripo => "TRIPO_API_KEY",
            KeyName::ElevenLabs => "ELEVENLABS_API_KEY",
        }
    }

    /// The provider name routes use (`model@<provider>`).
    pub fn provider(self) -> &'static str {
        match self {
            KeyName::OpenAi => "openai",
            KeyName::OpenRouter => "openrouter",
            KeyName::Fal => "fal",
            KeyName::Tripo => "tripo",
            KeyName::ElevenLabs => "elevenlabs",
        }
    }

    /// The key a provider's routes need.
    pub fn of_provider(provider: &str) -> Option<KeyName> {
        KeyName::ALL.into_iter().find(|k| k.provider() == provider)
    }

    /// The sentence a call without this key is refused with.
    pub fn missing(self) -> String {
        format!("{} is not set", self.variable())
    }
}

/// Where a value came from, for `grida-fx doctor` (never the value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Environment,
    DotEnv,
}

/// Every variable the loader reads: the five keys, then the four base-URL variables
/// ([`crate::setup::BASE_URL_VARIABLES`]).
pub const ALLOWED: [&str; 9] = [
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "FAL_KEY",
    "TRIPO_API_KEY",
    "ELEVENLABS_API_KEY",
    "OPENAI_BASE_URL",
    "OPENROUTER_BASE_URL",
    "FAL_BASE_URL",
    "ELEVENLABS_BASE_URL",
];

/// The allowlisted variables of one invocation, after precedence (module doc). Values never
/// print.
#[derive(Clone, Default, PartialEq)]
pub struct Environment {
    values: BTreeMap<String, (String, KeySource)>,
}

impl Environment {
    /// Reads [`ALLOWED`] (module doc): `process` reads a process variable; `dotenv` is the
    /// planning project's `.env`, read unless `process` gives `GRIDA_FX_DISABLE_DOTENV` = `1`.
    /// Errors are the module doc's sentences.
    pub fn read(
        process: &dyn Fn(&str) -> Option<String>,
        dotenv: Option<&Path>,
    ) -> Result<Environment, String> {
        let disabled = process(DISABLE_DOTENV).is_some_and(|value| value.trim() == "1");
        let file = match dotenv {
            Some(path) if !disabled => read_dotenv(path, &ALLOWED)?,
            _ => BTreeMap::new(),
        };
        let mut values = BTreeMap::new();
        for name in ALLOWED {
            let from_process = process(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let found = match from_process {
                Some(value) => Some((value, KeySource::Environment)),
                None => file
                    .get(name)
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .map(|value| (value, KeySource::DotEnv)),
            };
            if let Some(found) = found {
                values.insert(name.to_string(), found);
            }
        }
        Ok(Environment { values })
    }

    /// Values given directly, as from the process (tests use made-up values). Names off the
    /// allowlist and blank values are left out.
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Environment {
        Environment {
            values: pairs
                .iter()
                .filter(|(name, value)| ALLOWED.contains(name) && !value.trim().is_empty())
                .map(|(name, value)| {
                    (
                        name.to_string(),
                        (value.trim().to_string(), KeySource::Environment),
                    )
                })
                .collect(),
        }
    }

    /// A variable's value (trimmed, non-blank).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(|(value, _)| value.as_str())
    }

    pub fn source(&self, name: &str) -> Option<KeySource> {
        self.values.get(name).map(|(_, source)| *source)
    }
}

impl fmt::Debug for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.values.iter().map(|(name, (_, source))| (name, source)))
            .finish()
    }
}

/// The keys of one invocation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Keys {
    found: BTreeMap<KeyName, (Secret, KeySource)>,
}

impl Keys {
    /// No keys.
    pub fn none() -> Keys {
        Keys::default()
    }

    /// Keys given directly, as from the environment (tests use made-up values).
    pub fn from_pairs(pairs: &[(KeyName, &str)]) -> Keys {
        Keys {
            found: pairs
                .iter()
                .map(|(name, value)| (*name, (Secret::new(value), KeySource::Environment)))
                .collect(),
        }
    }

    /// The five keys of an [`Environment`].
    pub fn from_environment(environment: &Environment) -> Keys {
        Keys {
            found: KeyName::ALL
                .into_iter()
                .filter_map(|name| {
                    let value = environment.get(name.variable())?;
                    let source = environment.source(name.variable())?;
                    Some((name, (Secret::new(value), source)))
                })
                .collect(),
        }
    }

    pub fn get(&self, name: KeyName) -> Option<&Secret> {
        self.found.get(&name).map(|(secret, _)| secret)
    }

    pub fn source(&self, name: KeyName) -> Option<KeySource> {
        self.found.get(&name).map(|(_, source)| *source)
    }

    /// The names that have a key, in [`KeyName::ALL`] order.
    pub fn present(&self) -> Vec<KeyName> {
        self.found.keys().copied().collect()
    }

    /// Every key's value, for redaction.
    pub fn secrets(&self) -> Vec<Secret> {
        self.found
            .values()
            .map(|(secret, _)| secret.clone())
            .collect()
    }
}

/// An assignment line (module doc).
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*$")
        .expect("a valid pattern")
});

/// A trailing comment of an unquoted value.
static TRAILING_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+#.*$").expect("a valid pattern"));

/// Parses a `.env` text (module doc), keeping only `allowed` names. Returns name → value.
pub fn parse_dotenv(text: &str, allowed: &[&str]) -> Result<BTreeMap<String, String>, String> {
    // A line that names an allowed variable without being an assignment.
    let mention = if allowed.is_empty() {
        None
    } else {
        let names: Vec<String> = allowed.iter().map(|name| regex::escape(name)).collect();
        Some(
            Regex::new(&format!(r"^\s*(?:export\s+)?({})\b", names.join("|")))
                .map_err(|_| "the allowed .env names do not form a pattern".to_string())?,
        )
    };
    let mut values = BTreeMap::new();
    for (index, line) in split_lines(text).into_iter().enumerate() {
        let number = index + 1;
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        let Some(assignment) = ASSIGNMENT.captures(line) else {
            if let Some(found) = mention.as_ref().and_then(|m| m.captures(line)) {
                return Err(format!(
                    ".env contains malformed assignment for {} on line {number}",
                    &found[1]
                ));
            }
            continue;
        };
        let name = &assignment[1];
        if !allowed.contains(&name) {
            // Off the allowlist: the value is never looked at.
            continue;
        }
        if values.contains_key(name) {
            return Err(format!(".env contains duplicate key: {name}"));
        }
        let value = dotenv_value(&assignment[2], name, number)?;
        if value.is_empty() {
            return Err(format!(".env contains an empty value for {name}"));
        }
        if value.chars().any(|c| c < '\u{20}' || c == '\u{7f}') {
            return Err(format!(".env contains an unsafe value for {name}"));
        }
        values.insert(name.to_string(), value);
    }
    Ok(values)
}

/// A value's text after `=` (module doc): JSON-quoted, single-quoted or unquoted.
fn dotenv_value(raw: &str, name: &str, number: usize) -> Result<String, String> {
    let malformed = || format!(".env contains malformed quoted value for {name} on line {number}");
    let value = raw.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.starts_with('"') {
        return match serde_json::from_str::<serde_json::Value>(value) {
            Ok(serde_json::Value::String(decoded)) => Ok(decoded),
            _ => Err(malformed()),
        };
    }
    if let Some(rest) = value.strip_prefix('\'') {
        return match rest.strip_suffix('\'') {
            Some(inside) => Ok(inside.to_string()),
            None => Err(malformed()),
        };
    }
    let unquoted = TRAILING_COMMENT.replace(value, "");
    let unquoted = unquoted.trim();
    if unquoted.contains(['"', '\'']) {
        return Err(malformed());
    }
    Ok(unquoted.to_string())
}

/// `text` split as Python's `str.splitlines()` splits it: on `\n`, `\r\n`, `\r`, `\x0b`, `\x0c`,
/// `\x1c`, `\x1d`, `\x1e`, U+0085, U+2028 and U+2029, with no empty last line after a final
/// break.
fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let breaks = matches!(
            c,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !breaks {
            continue;
        }
        lines.push(&text[start..at]);
        let mut end = at + c.len_utf8();
        if c == '\r' && chars.peek().is_some_and(|(_, next)| *next == '\n') {
            chars.next();
            end += 1;
        }
        start = end;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Reads a `.env` file (module doc): `Ok(empty)` when missing.
pub fn read_dotenv(path: &Path, allowed: &[&str]) -> Result<BTreeMap<String, String>, String> {
    let not_regular = || ".env must be a regular file".to_string();
    let unreadable = || ".env could not be read".to_string();
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if missing(&error) => return Ok(BTreeMap::new()),
        Err(_) => return Err(unreadable()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(not_regular());
    }
    let mut file = std::fs::File::open(path).map_err(|_| unreadable())?;
    // The file opened is the one inspected: a symlink swapped in between is refused.
    let opened = file.metadata().map_err(|_| unreadable())?;
    if !opened.is_file() || !same_file(&metadata, &opened) {
        return Err(not_regular());
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).map_err(|_| unreadable())?;
    let text = String::from_utf8(bytes).map_err(|_| ".env must be valid UTF-8".to_string())?;
    parse_dotenv(&text, allowed)
}

/// A missing file, or a path through something that is not a directory.
fn missing(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_print() {
        let keys = Keys::from_pairs(&[(KeyName::Fal, "fal-secret")]);
        assert!(!format!("{keys:?}").contains("fal-secret"));
        assert_eq!(format!("{}", keys.get(KeyName::Fal).unwrap()), "[redacted]");
        assert_eq!(keys.get(KeyName::Fal).unwrap().expose(), "fal-secret");
        assert_eq!(keys.present(), [KeyName::Fal]);
        assert!(keys.get(KeyName::Tripo).is_none());
    }

    #[test]
    fn keys_come_from_the_environment() {
        let environment = Environment::from_pairs(&[
            ("FAL_KEY", " fal-secret "),
            ("TRIPO_API_KEY", "  "),
            ("HOME", "/x"),
            ("OPENAI_BASE_URL", "https://proxy.example.test/v1"),
        ]);
        assert_eq!(environment.get("HOME"), None);
        assert_eq!(environment.get("TRIPO_API_KEY"), None);
        assert!(!format!("{environment:?}").contains("fal-secret"));
        let keys = Keys::from_environment(&environment);
        assert_eq!(keys.present(), [KeyName::Fal]);
        assert_eq!(keys.get(KeyName::Fal).unwrap().expose(), "fal-secret");
        assert_eq!(keys.source(KeyName::Fal), Some(KeySource::Environment));
    }

    #[test]
    fn the_allowlist_is_the_keys_then_the_base_urls() {
        let keys: Vec<&str> = KeyName::ALL.iter().map(|k| k.variable()).collect();
        assert_eq!(ALLOWED[..5], keys[..]);
        assert_eq!(ALLOWED[5..], crate::setup::BASE_URL_VARIABLES[..]);
    }

    #[test]
    fn names_follow_the_providers() {
        let variables: Vec<&str> = KeyName::ALL.iter().map(|k| k.variable()).collect();
        assert_eq!(
            variables,
            [
                "OPENAI_API_KEY",
                "OPENROUTER_API_KEY",
                "FAL_KEY",
                "TRIPO_API_KEY",
                "ELEVENLABS_API_KEY"
            ]
        );
        assert_eq!(KeyName::of_provider("tripo"), Some(KeyName::Tripo));
        assert_eq!(KeyName::of_provider("acme"), None);
        assert_eq!(KeyName::Fal.missing(), "FAL_KEY is not set");
    }
}
