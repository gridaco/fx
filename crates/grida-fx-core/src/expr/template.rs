//! Templates: strings holding `${{ … }}`.
//!
//! `template` splits with the non-greedy, DOTALL pattern `\$\{\{(.*?)\}\}`, parses every part
//! before anything is evaluated, and refuses a text part that still holds `${{`
//! (`unclosed ${{ in {repr(source)}`). There is no escape for a literal `${{`; an expression can
//! produce one (`${{ '${{' }}`), and results are never scanned again. `render` keeps the value's
//! type for a whole template and otherwise concatenates `text()` of every part (spec/identity.md
//! §5), all parts evaluated in order.
//!
//! FX rules for mixed templates: a pending part makes the whole string pending
//! (`derive_all("text", …)`); a failed part makes the whole string that failed result, so its
//! reader is blocked (gnode rendered it as `{"failed":"…"}` text and ran the reader); missing
//! renders as `""`.

use super::eval::{Scope, evaluate, finish};
use super::lexer::is_space;
use super::parser::parse;
use super::{Expr, ExprError};
use crate::text::py_repr_str;
use crate::val::{Pending, Val};

/// One part of a template.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    Expr(Expr),
}

/// A parsed template.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub source: String,
    pub parts: Vec<Part>,
}

impl Template {
    /// The single expression when the string is exactly one `${{ … }}` and nothing else.
    pub fn whole(&self) -> Option<&Expr> {
        match self.parts.as_slice() {
            [Part::Expr(e)] => Some(e),
            _ => None,
        }
    }
}

/// Parses a string as a template; `Ok(None)` when it holds no `${{`.
///
/// The first `}}` after a `${{` closes it, even inside a quoted string; the inner text is
/// stripped of ASCII whitespace ([`is_space`]) and parsed at once, so a syntax error in any part
/// fails the whole string. A `}}` with no `${{` before it is ordinary text.
pub fn template(source: &str) -> Result<Option<Template>, ExprError> {
    const OPEN: &str = "${{";
    const CLOSE: &str = "}}";
    if !source.contains(OPEN) {
        return Ok(None);
    }
    let mut parts = Vec::new();
    let mut position = 0;
    while let Some(found) = source[position..].find(OPEN) {
        let start = position + found;
        let inner_start = start + OPEN.len();
        let Some(length) = source[inner_start..].find(CLOSE) else {
            break;
        };
        let inner_end = inner_start + length;
        if start > position {
            parts.push(Part::Text(source[position..start].to_string()));
        }
        parts.push(Part::Expr(parse(
            source[inner_start..inner_end].trim_matches(is_space),
        )?));
        position = inner_end + CLOSE.len();
    }
    if position < source.len() {
        parts.push(Part::Text(source[position..].to_string()));
    }
    let unclosed = parts
        .iter()
        .any(|part| matches!(part, Part::Text(text) if text.contains(OPEN)));
    if unclosed {
        return Err(ExprError::new(format!(
            "unclosed ${{{{ in {}",
            py_repr_str(source)
        )));
    }
    Ok(Some(Template {
        source: source.to_string(),
        parts,
    }))
}

/// Renders a template in a scope.
///
/// A whole template is its expression's value, of any type. Otherwise every part is evaluated in
/// order (an error in any part is the error), views are finished (a `.*` result becomes its
/// collection; any other view is refused), and then:
/// - a part holding a pending value makes the string `derive_all("text", parts)`;
/// - else a part holding a failed result makes the string that failed result;
/// - else the text parts and `text()` of every value, concatenated (missing renders as `""`).
pub fn render<S: Scope + ?Sized>(template: &Template, scope: &mut S) -> Result<Val, ExprError> {
    // Small on purpose: a step referenced here expands inside it (see `evaluate`).
    if let Some(whole) = template.whole() {
        return evaluate(whole, scope);
    }
    match evaluate_parts(template, scope) {
        Ok(values) => join_parts(scope, values),
        Err(error) => Err(error),
    }
}

/// Every part's value, in order: text parts as text.
#[inline(never)]
fn evaluate_parts<S: Scope + ?Sized>(
    template: &Template,
    scope: &mut S,
) -> Result<Vec<Val>, ExprError> {
    let mut values = Vec::with_capacity(template.parts.len());
    for part in &template.parts {
        values.push(match part {
            Part::Text(text) => Val::Str(text.clone()),
            Part::Expr(expr) => evaluate(expr, scope)?,
        });
    }
    Ok(values)
}

/// The parts' values made one value (see [`render`]).
#[inline(never)]
fn join_parts<S: Scope + ?Sized>(scope: &mut S, values: Vec<Val>) -> Result<Val, ExprError> {
    let mut finished = Vec::with_capacity(values.len());
    for value in values {
        finished.push(finish(scope, value)?);
    }
    if finished.iter().any(Val::contains_pending) {
        return Ok(Val::Pending(Box::new(Pending::derive_all(
            "text", &finished,
        )?)));
    }
    if let Some(id) = finished.iter().find_map(Val::first_failed) {
        return Ok(Val::Failed(id.to_string()));
    }
    let mut text = String::new();
    for value in &finished {
        text.push_str(&value.text()?);
    }
    Ok(Val::Str(text))
}
