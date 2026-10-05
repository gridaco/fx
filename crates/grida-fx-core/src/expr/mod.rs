//! The expression language: `${{ … }}` inside workflow strings.
//!
//! Only strings are evaluated: a document's keys never are, and numbers, booleans and null pass
//! through [`resolve`] unchanged.
//!
//! - [`lexer`]: tokens. FX decision: digits and whitespace are ASCII only (gnode accepted Unicode
//!   ones); `strip()` of a template's inner text strips the same whitespace.
//! - [`parser`]: a Pratt parser into [`ast::Expr`].
//! - [`template`]: splitting strings into text and expressions, and rendering them.
//! - [`eval`]: evaluation over a [`eval::Scope`], and [`resolve`] over documents.
//! - [`functions`]: the eleven functions.
//!
//! Values follow spec/identity.md: one number type, so `-1` is an ordinary number and an integer
//! literal that reading would round is refused (§1); `digest(v)` is the first 16 hex characters
//! of `digest(plain(v))` (§2, §3); text renders as §5 says. Messages keep gnode's wording, with
//! Python's `repr` from [`crate::text::py_repr_str`] and kind words from
//! [`crate::val::Val::kind_word`].

pub mod ast;
pub mod eval;
pub mod functions;
pub mod lexer;
pub mod parser;
pub mod template;

use std::fmt;

pub use ast::{BinaryOp, Expr, Func, Literal, UnaryOp};
pub use eval::{Scope, evaluate, every, finish, item, len, member, resolve};
pub use parser::parse;
pub use template::{Part, Template, render, template};

/// An expression error: a sentence, attached by the caller to a problem's `where`. The expander
/// turns it into `Problem(where, message)` and the value into missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError(pub String);

impl ExprError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// A stub of the skeleton.
    pub fn todo(what: &str) -> Self {
        Self(format!("{what} is not implemented yet"))
    }

    /// `this names a step; take its result, e.g. .outputs.image or .facts.verdict`.
    pub fn names_a_step() -> Self {
        Self::new("this names a step; take its result, e.g. .outputs.image or .facts.verdict")
    }
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExprError {}
