//! Evaluation.
//!
//! Strictly left to right, depth first; call arguments are all evaluated before the arity check.
//! Short-circuited operands are never evaluated (they record no reads and raise nothing). `??`
//! replaces only null and missing; `&&`/`||` return an operand. `==`/`!=` compare plain forms
//! with FX's equality ([`plain_eq`]); numeric operators take numbers only and refuse a result
//! that is not finite.
//!
//! Pending values propagate through unary operators, members, `.*`, indexes (target or index; a
//! repeat indexed by a pending key waits on the key and on every instance of the repeat),
//! `facts()` and `accepted()`; through binary operators unless a short circuit decides first
//! (`false && P` is `false`, `3 ?? P` is `3`); through functions with a pending argument; and
//! through mixed templates. A pending value inside a list or object does not make the list
//! pending, except where its content is read (equality, `digest`, `join`, `contains` over a list,
//! `min`/`max` over a list, text).
//!
//! Views are handled by the scope; every other value by the rules here. A view that must become
//! a plain value (next to a pending value, inside text, as `digest`'s argument) is finished
//! first: a `.*` result is its collection, any other view is refused (gnode crashed there).

use super::ast::{BinaryOp, Func, Literal, UnaryOp};
use super::functions;
use super::template::{render, template};
use super::{Expr, ExprError};
use crate::text::py_repr_str;
use crate::val::{FileContent, FileValue, Pending, Val, ViewId, plain_eq};
use crate::value::format_number;
use serde_json::Value;

/// What names mean, and how views behave. The expander's step scope and prompt-file scope, and
/// the runner's `prompt.render` scope, implement it.
pub trait Scope {
    /// Optional, observational access tracing. Hooks must not evaluate or modify values.
    fn begin_access(&mut self) {}
    fn end_access(&mut self, _ok: bool) {}
    fn access_member(&mut self, _name: &str) {}
    fn access_unknown_member(&mut self) {}

    /// A bare name. Unknown names are refused with the scope's own message (`unknown name 'x'` in
    /// a step scope, `a prompt sees vars and inputs, not 'x'` in a prompt file).
    fn root(&mut self, name: &str) -> Result<Val, ExprError>;

    /// `.name` on a view.
    fn view_member(&mut self, view: ViewId, name: &str) -> Result<Val, ExprError>;

    /// `[index]` on a view; `index` is not pending (the evaluator handles a pending index through
    /// [`Scope::view_item_pending`]).
    fn view_item(&mut self, view: ViewId, index: &Val) -> Result<Val, ExprError>;

    /// `[index]` on a view whose index is pending. `None` (the default) lets the evaluator finish
    /// the view and derive a pending value from both; the step scope answers for a repeat: a
    /// pending value over the index's refs and every instance of the repeat, since any of them
    /// may be the one picked.
    fn view_item_pending(
        &mut self,
        _view: ViewId,
        _index: &Pending,
    ) -> Option<Result<Val, ExprError>> {
        None
    }

    /// `.*` on a view (only repeats and `.*` results allow it).
    fn view_every(&mut self, view: ViewId) -> Result<Val, ExprError>;

    /// `len()` of a view.
    fn view_len(&mut self, view: ViewId) -> Result<usize, ExprError>;

    /// The word for a view in type errors (`a step`).
    fn view_kind(&self, _view: ViewId) -> &'static str {
        "a step"
    }

    /// Turns a view into a value when an expression ends on it: a `.*` result becomes a
    /// collection, a select's port its value; any other view is refused with exactly
    /// [`ExprError::names_a_step`]. The evaluator relies on that error to tell a step view, which
    /// `==` compares by identity and `accepted()` refuses with its own message, from a `.*`
    /// result that cannot be finished (`name what to take from each instance, …`).
    fn finish_view(&mut self, view: ViewId) -> Result<Val, ExprError>;

    /// `facts(v)` of a non-pending value. The step scope returns missing and failed values
    /// unchanged; every scope refuses non-files with `facts() needs a file, not {kind}`. Files go
    /// through [`crate::facts::file_facts`].
    fn facts(&mut self, value: &Val) -> Result<Val, ExprError>;
}

/// Evaluates an expression.
///
/// Expressions nest, and a reference to a step expands that step inside the evaluation, so every
/// frame between one step and the next is on the stack once per link of a chain of steps reading
/// each other. This function and the ones it calls before evaluating an operand are therefore
/// kept small: each kind of expression is its own function, never inlined.
pub fn evaluate<S: Scope + ?Sized>(expr: &Expr, scope: &mut S) -> Result<Val, ExprError> {
    match expr {
        Expr::Name(name) => scope.root(name),
        Expr::Field(..) | Expr::Index(..) | Expr::Every(..) => access(scope, expr),
        other => evaluate_other(scope, other),
    }
}

/// A chain of `.name`, `[index]` and `.*` in one frame: the innermost target first, then each
/// access in order (an index is evaluated right after the target it indexes), so
/// `steps.x.outputs.text` does not nest a frame per access.
#[inline(never)]
fn access<S: Scope + ?Sized>(scope: &mut S, expr: &Expr) -> Result<Val, ExprError> {
    scope.begin_access();
    let result = access_chain(scope, expr);
    scope.end_access(result.is_ok());
    result
}

#[inline(never)]
fn access_chain<S: Scope + ?Sized>(scope: &mut S, expr: &Expr) -> Result<Val, ExprError> {
    let mut at = links(expr);
    let mut value = evaluate(link(expr, at), scope)?;
    while at > 0 {
        at -= 1;
        value = apply(scope, link(expr, at), value)?;
    }
    Ok(value)
}

/// How many accesses an access chain holds.
fn links(expr: &Expr) -> usize {
    let mut count = 0;
    let mut current = expr;
    while let Expr::Field(target, _) | Expr::Index(target, _) | Expr::Every(target) = current {
        count += 1;
        current = target;
    }
    count
}

/// The expression `depth` targets inside `expr`: an access, or the innermost target when
/// `depth` is the chain's length.
fn link(expr: &Expr, depth: usize) -> &Expr {
    let mut current = expr;
    for _ in 0..depth {
        match current {
            Expr::Field(target, _) | Expr::Index(target, _) | Expr::Every(target) => {
                current = target
            }
            _ => break,
        }
    }
    current
}

/// One access of a chain applied to the value so far.
#[inline(never)]
fn apply<S: Scope + ?Sized>(scope: &mut S, link: &Expr, value: Val) -> Result<Val, ExprError> {
    match link {
        Expr::Field(_, name) => {
            scope.access_member(name);
            member(scope, value, name)
        }
        Expr::Index(_, index) => indexed(scope, value, index),
        _ => every(scope, value),
    }
}

/// `value[index]`: the index evaluated, then the item.
#[inline(never)]
fn indexed<S: Scope + ?Sized>(scope: &mut S, value: Val, index: &Expr) -> Result<Val, ExprError> {
    let index = evaluate(index, scope)?;
    if let Val::Str(name) = &index {
        scope.access_member(name);
    } else {
        scope.access_unknown_member();
    }
    item(scope, value, index)
}

/// Every expression but names and accesses.
#[inline(never)]
fn evaluate_other<S: Scope + ?Sized>(scope: &mut S, expr: &Expr) -> Result<Val, ExprError> {
    match expr {
        Expr::Unary(op, operand) => unary(scope, *op, operand),
        Expr::Binary(op, left, right) => binary(scope, *op, left, right),
        Expr::Call(func, args) => called(scope, *func, args),
        Expr::Literal(literal) => Ok(literal_value(literal)),
        // Handled by `evaluate`.
        Expr::Name(name) => scope.root(name),
        _ => access(scope, expr),
    }
}

fn literal_value(literal: &Literal) -> Val {
    match literal {
        Literal::Null => Val::Null,
        Literal::Bool(b) => Val::Bool(*b),
        Literal::Number(x) => Val::Number(*x),
        Literal::Str(s) => Val::Str(s.clone()),
    }
}

#[inline(never)]
fn unary<S: Scope + ?Sized>(scope: &mut S, op: UnaryOp, operand: &Expr) -> Result<Val, ExprError> {
    let value = evaluate(operand, scope)?;
    unary_of(scope, op, value)
}

#[inline(never)]
fn unary_of<S: Scope + ?Sized>(scope: &mut S, op: UnaryOp, value: Val) -> Result<Val, ExprError> {
    if let Val::Pending(p) = &value {
        return pending(p.derive(op.text(), &[]));
    }
    match op {
        UnaryOp::Not => Ok(Val::Bool(!value.truthy())),
        UnaryOp::Neg => {
            let x = operand_number(scope, &value, "-")?;
            finite(-x, "-")
        }
    }
}

#[inline(never)]
fn called<S: Scope + ?Sized>(scope: &mut S, func: Func, args: &[Expr]) -> Result<Val, ExprError> {
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        values.push(evaluate(arg, scope)?);
    }
    functions::call(scope, func, values)
}

/// A binary operator: the left operand, then the right one unless a short circuit decides.
#[inline(never)]
fn binary<S: Scope + ?Sized>(
    scope: &mut S,
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
) -> Result<Val, ExprError> {
    let left = evaluate(left, scope)?;
    let left = match short_circuit(op, left) {
        Ok(decided) => return Ok(decided),
        Err(left) => left,
    };
    if op == BinaryOp::Coalesce && !matches!(left, Val::Pending(_)) {
        // `left` is nothing: the value is the right operand's.
        return evaluate(right, scope);
    }
    let right = evaluate(right, scope)?;
    combine(scope, op, left, right)
}

/// What the left operand alone decides: `Ok` with the result, or `Err` with the operand back.
fn short_circuit(op: BinaryOp, left: Val) -> Result<Val, Val> {
    let pending = matches!(left, Val::Pending(_));
    match op {
        BinaryOp::Coalesce if !pending && !left.is_nothing() => Ok(left),
        BinaryOp::And if !pending && !left.truthy() => Ok(left),
        BinaryOp::Or if !pending && left.truthy() => Ok(left),
        _ => Err(left),
    }
}

/// A binary operator over both evaluated operands.
#[inline(never)]
fn combine<S: Scope + ?Sized>(
    scope: &mut S,
    op: BinaryOp,
    left: Val,
    right: Val,
) -> Result<Val, ExprError> {
    if matches!(left, Val::Pending(_)) || matches!(right, Val::Pending(_)) {
        return derive_all(scope, op.text(), vec![left, right]);
    }
    match op {
        BinaryOp::And | BinaryOp::Or | BinaryOp::Coalesce => Ok(right),
        BinaryOp::Eq | BinaryOp::Ne => {
            let left = comparable(scope, left)?;
            let right = comparable(scope, right)?;
            if left.contains_pending() || right.contains_pending() {
                return derive_all(scope, op.text(), vec![left, right]);
            }
            let equal = plain_eq(&left, &right);
            Ok(Val::Bool(if op == BinaryOp::Eq { equal } else { !equal }))
        }
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            let ordering = if let (Val::Str(a), Val::Str(b)) = (&left, &right) {
                // Code-point order, which UTF-8 byte order is.
                a.cmp(b)
            } else {
                let a = operand_number(scope, &left, op.text())?;
                let b = operand_number(scope, &right, op.text())?;
                // Both are finite, so they are ordered.
                a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
            };
            Ok(Val::Bool(match op {
                BinaryOp::Lt => ordering.is_lt(),
                BinaryOp::Le => ordering.is_le(),
                BinaryOp::Gt => ordering.is_gt(),
                _ => ordering.is_ge(),
            }))
        }
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
            if let (BinaryOp::Add, Val::Str(a), Val::Str(b)) = (op, &left, &right) {
                return Ok(Val::Str(format!("{a}{b}")));
            }
            let a = operand_number(scope, &left, op.text())?;
            let b = operand_number(scope, &right, op.text())?;
            let result = match op {
                BinaryOp::Add => a + b,
                BinaryOp::Sub => a - b,
                BinaryOp::Mul => a * b,
                _ => {
                    if b == 0.0 {
                        return Err(ExprError::new("division by zero"));
                    }
                    a / b
                }
            };
            finite(result, op.text())
        }
    }
}

/// An operand of `==`/`!=`: a `.*` result compares as its collection (gnode compared its plain
/// form); any other view stays a view and compares by identity.
fn comparable<S: Scope + ?Sized>(scope: &mut S, value: Val) -> Result<Val, ExprError> {
    match value {
        Val::View(view) => match scope.finish_view(view) {
            Ok(finished) => Ok(finished),
            Err(e) if e == ExprError::names_a_step() => Ok(Val::View(view)),
            Err(e) => Err(e),
        },
        other => Ok(other),
    }
}

/// An operand of a numeric operator or of `min`/`max`: the number, or `{op} needs numbers, not
/// {kind}` (booleans are not numbers).
pub(crate) fn operand_number<S: Scope + ?Sized>(
    scope: &S,
    value: &Val,
    op: &str,
) -> Result<f64, ExprError> {
    match value {
        Val::Number(x) if x.is_finite() => Ok(*x),
        Val::Number(_) => Err(ExprError::new(format!("{op} of a non-finite number"))),
        other => Err(ExprError::new(format!(
            "{op} needs numbers, not {}",
            kind(scope, other)
        ))),
    }
}

/// A computed number, refused when it is not finite (FX numbers never are).
fn finite(x: f64, op: &str) -> Result<Val, ExprError> {
    Val::number(x).ok_or_else(|| ExprError::new(format!("{op} of a non-finite number")))
}

/// The word for a value in type errors; views ask the scope.
pub(crate) fn kind<S: Scope + ?Sized>(scope: &S, value: &Val) -> &'static str {
    match value {
        Val::View(view) => scope.view_kind(*view),
        other => other.kind_word(),
    }
}

fn pending(result: Result<Pending, ExprError>) -> Result<Val, ExprError> {
    result.map(|p| Val::Pending(Box::new(p)))
}

/// `derive_all(op, values)` after finishing any view among the values (a `.*` result is its
/// collection; any other view is refused, where gnode crashed).
pub(crate) fn derive_all<S: Scope + ?Sized>(
    scope: &mut S,
    op: &str,
    values: Vec<Val>,
) -> Result<Val, ExprError> {
    let mut finished = Vec::with_capacity(values.len());
    for value in values {
        finished.push(finish(scope, value)?);
    }
    pending(Pending::derive_all(op, &finished))
}

/// Python's `repr` of a value as gnode's messages print it: strings quoted, `None`, `True`,
/// numbers in their JCS form, lists and objects in Python's notation.
pub(crate) fn repr(value: &Val) -> String {
    match value {
        Val::Null => "None".into(),
        Val::Bool(true) => "True".into(),
        Val::Bool(false) => "False".into(),
        Val::Number(x) => format_number(*x),
        Val::Str(s) => py_repr_str(s),
        Val::List(items) => format!(
            "[{}]",
            items.iter().map(repr).collect::<Vec<_>>().join(", ")
        ),
        Val::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Val::File(f) => py_repr_str(&f.name),
        Val::Missing => "MISSING".into(),
        Val::Failed(id) => format!("Failed({})", py_repr_str(id)),
        Val::Collection(c) => format!(
            "Collection({})",
            c.items
                .iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Val::Pending(_) => "Pending".into(),
        Val::View(_) => "step".into(),
    }
}

/// Resolves a document value: strings as templates (a string without `${{` stays itself), lists
/// and objects recursively in order (keys untouched), other scalars unchanged. The caller
/// finishes the result.
pub fn resolve<S: Scope + ?Sized>(value: &Value, scope: &mut S) -> Result<Val, ExprError> {
    match value {
        Value::String(s) => resolve_text(s, scope),
        other => resolve_other(other, scope),
    }
}

#[inline(never)]
fn resolve_text<S: Scope + ?Sized>(text: &str, scope: &mut S) -> Result<Val, ExprError> {
    match template(text) {
        Ok(None) => Ok(Val::Str(text.to_string())),
        Ok(Some(parsed)) => render(&parsed, scope),
        Err(error) => Err(error),
    }
}

#[inline(never)]
fn resolve_other<S: Scope + ?Sized>(value: &Value, scope: &mut S) -> Result<Val, ExprError> {
    match value {
        Value::String(s) => resolve_text(s, scope),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(resolve(item, scope)?);
            }
            Ok(Val::List(out))
        }
        Value::Object(map) => {
            let mut out = indexmap::IndexMap::with_capacity(map.len());
            for (key, item) in map {
                out.insert(key.clone(), resolve(item, scope)?);
            }
            Ok(Val::Object(out))
        }
        scalar => Ok(Val::from_json(scalar)),
    }
}

/// `.name` on any value: a field of an object (`no field 'x'`); a view through the scope; a file
/// per [`file_member`]; missing and failed values stay themselves; a collection maps the member
/// over its elements, dropping elements whose field is missing; anything else is refused
/// (`{kind} has no field 'x'`).
pub fn member<S: Scope + ?Sized>(scope: &mut S, value: Val, name: &str) -> Result<Val, ExprError> {
    // A view on its own path: `steps.x` expands `x` (see `evaluate`).
    match value {
        Val::View(view) => scope.view_member(view, name),
        other => member_of_value(scope, other, name),
    }
}

#[inline(never)]
fn member_of_value<S: Scope + ?Sized>(
    scope: &mut S,
    value: Val,
    name: &str,
) -> Result<Val, ExprError> {
    match value {
        Val::Pending(p) => pending(p.derive("field", &[Val::Str(name.to_string())])),
        Val::Object(mut map) => map
            .swap_remove(name)
            .ok_or_else(|| ExprError::new(format!("no field {}", py_repr_str(name)))),
        Val::View(view) => scope.view_member(view, name),
        Val::File(file) => file_member(&file, name),
        Val::Missing => Ok(Val::Missing),
        failed @ Val::Failed(_) => Ok(failed),
        Val::Collection(collection) => {
            // The same field of every element; elements whose field is missing are dropped.
            let collection = *collection;
            let mut items = Vec::with_capacity(collection.items.len());
            for (key, element) in collection.items {
                let field = member(scope, element, name)?;
                if field != Val::Missing {
                    items.push((key, field));
                }
            }
            Ok(Val::Collection(Box::new(crate::val::Collection {
                items,
                verdicts: collection.verdicts,
            })))
        }
        other => Err(ExprError::new(format!(
            "{} has no field {}",
            kind(scope, &other),
            py_repr_str(name)
        ))),
    }
}

/// `.name` of a file: a field of JSON-object content (`d.json has no field 'x'`); else `digest`,
/// `kind` or `key` (null when unset); else `a file has no field 'x'; use facts(file).x`.
fn file_member(file: &FileValue, name: &str) -> Result<Val, ExprError> {
    if let Some(FileContent::Json(Value::Object(map))) = &file.content {
        return map.get(name).map(Val::from_json).ok_or_else(|| {
            ExprError::new(format!("{} has no field {}", file.name, py_repr_str(name)))
        });
    }
    match name {
        "digest" => Ok(Val::Str(file.digest.clone())),
        "kind" => Ok(Val::Str(file.kind.clone())),
        "key" => Ok(file.key.clone().map_or(Val::Null, Val::Str)),
        _ => Err(ExprError::new(format!(
            "a file has no field {}; use facts(file).{name}",
            py_repr_str(name)
        ))),
    }
}

/// A whole number usable as an index, or `None`.
fn whole_index(value: &Val) -> Option<f64> {
    match value {
        Val::Number(x) if x.is_finite() && x.fract() == 0.0 => Some(*x),
        _ => None,
    }
}

/// A whole-number index as a position, counted from the end when negative, if it is inside a
/// sequence of `len`.
fn position(index: f64, len: usize) -> Option<usize> {
    let len = len as f64;
    let at = if index < 0.0 { index + len } else { index };
    // `at` is whole, so inside the range it converts exactly.
    (0.0 <= at && at < len).then_some(at as usize)
}

/// `[index]` on any value: an object's string key (`no entry …`); a list position, a whole
/// number counted from the end when negative; a view through the scope; a JSON file's content;
/// missing and failed values stay themselves; a collection by key, then by position (`no
/// instance …`); anything else is refused (`{kind} cannot be indexed`).
pub fn item<S: Scope + ?Sized>(scope: &mut S, value: Val, index: Val) -> Result<Val, ExprError> {
    if let (Val::View(view), Val::Pending(pending)) = (&value, &index)
        && let Some(result) = scope.view_item_pending(*view, pending)
    {
        return result;
    }
    if matches!(value, Val::Pending(_)) || matches!(index, Val::Pending(_)) {
        return derive_all(scope, "index", vec![value, index]);
    }
    match value {
        Val::Object(mut map) => {
            let found = match &index {
                Val::Str(key) => map.swap_remove(key.as_str()),
                _ => None,
            };
            found.ok_or_else(|| ExprError::new(format!("no entry {}", repr(&index))))
        }
        Val::List(mut items) => {
            let Some(i) = whole_index(&index) else {
                return Err(ExprError::new(match index {
                    Val::Number(x) => {
                        format!(
                            "a list index must be a whole number, not {}",
                            format_number(x)
                        )
                    }
                    other => format!(
                        "a list index must be a whole number, not {}",
                        kind(scope, &other)
                    ),
                }));
            };
            match position(i, items.len()) {
                Some(at) => Ok(items.swap_remove(at)),
                None => Err(ExprError::new(format!(
                    "index {} is outside a list of {}",
                    format_number(i),
                    items.len()
                ))),
            }
        }
        Val::View(view) => scope.view_item(view, &index),
        Val::File(file) => match &file.content {
            Some(FileContent::Json(content @ (Value::Array(_) | Value::Object(_)))) => {
                item(scope, Val::from_json(content), index)
            }
            _ => Err(ExprError::new(format!("{} cannot be indexed", file.name))),
        },
        Val::Missing => Ok(Val::Missing),
        failed @ Val::Failed(_) => Ok(failed),
        Val::Collection(collection) => {
            // The first item under that key; else a whole number counts positions.
            let at = match &index {
                Val::Str(key) => collection.items.iter().position(|(k, _)| k == key),
                _ => None,
            }
            .or_else(|| whole_index(&index).and_then(|i| position(i, collection.items.len())));
            match at {
                Some(at) => Ok(collection.items[at].1.clone()),
                None => Err(ExprError::new(format!("no instance {}", repr(&index)))),
            }
        }
        other => Err(ExprError::new(format!(
            "{} cannot be indexed",
            kind(scope, &other)
        ))),
    }
}

/// `.*` on any value: only views allow it (`.* applies to a repeated step`).
pub fn every<S: Scope + ?Sized>(scope: &mut S, value: Val) -> Result<Val, ExprError> {
    match value {
        Val::Pending(p) => pending(p.derive("every", &[])),
        Val::View(view) => scope.view_every(view),
        _ => Err(ExprError::new(".* applies to a repeated step")),
    }
}

/// `len(v)` of a non-pending value, as a number of items: characters of text, items of a list,
/// keys of an object, a file's text or JSON content, a collection's items, or the scope's count
/// for a view.
pub fn len<S: Scope + ?Sized>(scope: &mut S, value: &Val) -> Result<usize, ExprError> {
    match value {
        Val::Str(s) => Ok(s.chars().count()),
        Val::List(items) => Ok(items.len()),
        Val::Object(map) => Ok(map.len()),
        Val::View(view) => scope.view_len(*view),
        Val::Collection(c) => Ok(c.items.len()),
        Val::File(file) => match &file.content {
            Some(FileContent::Text(text)) => Ok(text.chars().count()),
            Some(FileContent::Json(Value::String(text))) => Ok(text.chars().count()),
            Some(FileContent::Json(Value::Array(items))) => Ok(items.len()),
            Some(FileContent::Json(Value::Object(map))) => Ok(map.len()),
            _ => Err(ExprError::new(format!("len() of a {} file", file.kind))),
        },
        other => Err(ExprError::new(format!("len() of {}", kind(scope, other)))),
    }
}

/// Finishes a value: views through [`Scope::finish_view`], lists and objects recursively, other
/// values unchanged.
pub fn finish<S: Scope + ?Sized>(scope: &mut S, value: Val) -> Result<Val, ExprError> {
    match value {
        Val::View(view) => scope.finish_view(view),
        Val::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(finish(scope, item)?);
            }
            Ok(Val::List(out))
        }
        Val::Object(map) => {
            let mut out = indexmap::IndexMap::with_capacity(map.len());
            for (key, item) in map {
                out.insert(key, finish(scope, item)?);
            }
            Ok(Val::Object(out))
        }
        other => Ok(other),
    }
}
