//! Errors and problems.
//!
//! Two channels, never mixed (conformance/README.md, "Exit status"):
//! - an [`Error`] stops the command: unreadable or invalid input, a broken environment. The CLI
//!   prints `grida-fx: <message>` on stderr and exits 2.
//! - a [`Problem`] is collected into the plan: something the workflow author must fix. The plan is
//!   still produced and printed; the planning verbs exit 1 (`plan` only with `--check`).

use serde::Serialize;
use std::fmt;

/// What class of error an [`Error`] is. Every kind exits 2; the kind only helps callers and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// A command-line mistake: a bad flag, a bad `--arg`, a missing argument.
    Usage,
    /// A document that is not valid YAML in the strict subset (yaml.md), with file, line and column.
    Yaml,
    /// A document outside its schema, or of the wrong `fx:` kind.
    Document,
    /// Workflow inputs that do not fit the workflow's declared inputs.
    Input,
    /// The plan cannot be made: no such workflow, a builder that failed, a takes file of the wrong shape.
    Plan,
    /// A route table that cannot be read (identity.md §7, §12).
    Route,
    /// A file that cannot be read or written; the message names it, never with a private absolute path.
    Io,
    /// A node host that could not start or broke the protocol.
    Host,
    /// A command or feature that arrives with a later step ("not available until the runner lands").
    Unavailable,
    /// A fault in the engine itself.
    Internal,
}

/// A fatal error: the command stops and exits 2. `message` is a sentence for a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

/// The core's result type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Usage, message)
    }

    pub fn document(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Document, message)
    }

    pub fn input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Input, message)
    }

    pub fn plan(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Plan, message)
    }

    pub fn route(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Route, message)
    }

    pub fn host(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Host, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    /// A command of a later step: `"<verb> is not available until the runner lands"`.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unavailable, message)
    }

    /// A file that could not be read or written. `label` is the name as the user wrote it (or a
    /// project-relative path), never an absolute path the user did not type.
    pub fn io(label: &str, error: &std::io::Error) -> Self {
        Self::new(ErrorKind::Io, format!("{label}: {}", io_reason(error)))
    }
}

/// A short reason for an I/O error, without the path the OS adds: "no such file", "not a
/// directory", or the OS text.
pub fn io_reason(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "no such file".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => error.to_string(),
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<crate::value::Refused> for Error {
    fn from(refused: crate::value::Refused) -> Self {
        Self::new(ErrorKind::Input, refused.message)
    }
}

/// A planning problem: where in the workflow, and what is wrong. Rendered `"{where}: {message}"`; serialized `{"where", "message"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Problem {
    #[serde(rename = "where")]
    pub where_: String,
    pub message: String,
}

impl Problem {
    pub fn new(where_: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            where_: where_.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.where_, self.message)
    }
}

/// Problems deduplicated by `(where, message)`, the first of each kept, order kept
/// (the order they were found, without repeats).
pub fn unique(problems: Vec<Problem>) -> Vec<Problem> {
    let mut seen = std::collections::HashSet::new();
    problems
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_keeps_the_first() {
        let a = Problem::new("a", "x");
        let b = Problem::new("b", "x");
        assert_eq!(unique(vec![a.clone(), b.clone(), a.clone()]), vec![a, b]);
    }
}
