//! The expression functions. Arguments are all evaluated, left to right, before the
//! arity check (`{name}() takes {n} values` / `takes {n} or more values`). `facts` and
//! `accepted` run before the pending check; any other function with a top-level pending argument
//! returns `derive_all(name, args)`.
//!
//! FX deltas: `digest(v)` uses spec/identity.md `digest(plain(v))` (first 16 hex characters);
//! `lookup` and `contains` compare keys and items with FX equality (string keys; booleans are not
//! numbers; an unhashable key just finds nothing); `min`/`max` results are plain numbers.
//!
//! A function whose result is made from the content of a list or object (`digest`, `join`,
//! `min`/`max` over a list, `contains` over a list) is also pending while that content holds a
//! pending value, so a pending token never ends up inside a known value; gnode rendered such
//! tokens into the result. `join` of a list holding a failed result is that failed result, as a
//! mixed template is.

use super::eval::{Scope, derive_all, kind, len, operand_number, repr};
use super::{ExprError, Func};
use crate::val::{Collection, Pending, Val, Verdict, plain_eq};
use crate::value::digest;
use std::collections::BTreeSet;

/// Calls a function on already evaluated arguments.
pub fn call<S: Scope + ?Sized>(
    scope: &mut S,
    func: Func,
    args: Vec<Val>,
) -> Result<Val, ExprError> {
    let name = func.name();
    let (low, high) = func.arity();
    if args.len() < low || high.is_some_and(|high| args.len() > high) {
        let count = if high == Some(low) {
            low.to_string()
        } else {
            format!("{low} or more")
        };
        return Err(ExprError::new(format!("{name}() takes {count} values")));
    }
    let mut args = args;
    match func {
        Func::Facts => {
            let value = args.swap_remove(0);
            return match value {
                Val::Pending(p) => Ok(Val::Pending(Box::new(p.derive("facts", &[])?))),
                other => scope.facts(&other),
            };
        }
        Func::Accepted => return accepted_of(scope, args.swap_remove(0)),
        _ => {}
    }
    if args.iter().any(|arg| matches!(arg, Val::Pending(_))) {
        return derive_all(scope, name, args);
    }
    match func {
        Func::Lookup => {
            let key = args.pop().unwrap_or(Val::Null);
            let table = args.pop().unwrap_or(Val::Null);
            let Val::Object(mut table) = table else {
                return Err(ExprError::new("lookup() needs a table"));
            };
            let found = match &key {
                Val::Str(k) => table.swap_remove(k.as_str()),
                _ => None,
            };
            found.ok_or_else(|| ExprError::new(format!("lookup(): no entry {}", repr(&key))))
        }
        Func::Min | Func::Max => {
            let values = match args.as_slice() {
                [Val::List(items)] => items.clone(),
                _ => args.clone(),
            };
            if values.is_empty() {
                return Err(ExprError::new(format!("{name}() of nothing")));
            }
            if values.iter().any(|v| matches!(v, Val::Pending(_))) {
                return derive_all(scope, name, args);
            }
            let mut best: Option<f64> = None;
            for value in &values {
                let x = operand_number(scope, value, name)?;
                best = Some(match best {
                    None => x,
                    Some(b) if func == Func::Min => b.min(x),
                    Some(b) => b.max(x),
                });
            }
            Ok(best.and_then(Val::number).unwrap_or(Val::Null))
        }
        Func::Len => {
            let n = len(scope, &args[0])?;
            Ok(Val::Number(n as f64))
        }
        Func::Contains => {
            let needle = args.pop().unwrap_or(Val::Null);
            let haystack = args.pop().unwrap_or(Val::Null);
            match (&haystack, &needle) {
                (Val::Str(h), Val::Str(n)) => Ok(Val::Bool(h.contains(n.as_str()))),
                (Val::List(items), _) => {
                    if haystack.contains_pending() || needle.contains_pending() {
                        return derive_all(scope, name, vec![haystack, needle]);
                    }
                    Ok(Val::Bool(items.iter().any(|item| plain_eq(item, &needle))))
                }
                (Val::Object(map), _) => Ok(Val::Bool(match &needle {
                    Val::Str(key) => map.contains_key(key.as_str()),
                    _ => false,
                })),
                _ => Err(ExprError::new(format!(
                    "contains() of {}",
                    kind(scope, &haystack)
                ))),
            }
        }
        Func::Concat => {
            if args.iter().all(|arg| matches!(arg, Val::List(_))) {
                let mut out = Vec::new();
                for arg in args {
                    if let Val::List(items) = arg {
                        out.extend(items);
                    }
                }
                return Ok(Val::List(out));
            }
            if args.iter().all(|arg| matches!(arg, Val::Str(_))) {
                let mut out = String::new();
                for arg in &args {
                    if let Val::Str(s) = arg {
                        out.push_str(s);
                    }
                }
                return Ok(Val::Str(out));
            }
            Err(ExprError::new(
                "concat() joins lists with lists or text with text",
            ))
        }
        Func::Join => {
            let (Val::List(items), Val::Str(separator)) = (&args[0], &args[1]) else {
                return Err(ExprError::new("join() takes a list and a separator"));
            };
            if args[0].contains_pending() {
                return derive_all(scope, name, args);
            }
            if let Some(id) = args[0].first_failed() {
                return Ok(Val::Failed(id.to_string()));
            }
            let mut texts = Vec::with_capacity(items.len());
            for item in items {
                texts.push(item.text()?);
            }
            Ok(Val::Str(texts.join(separator)))
        }
        Func::Stem => match &args[0] {
            Val::File(file) => Ok(Val::Str(file.stem())),
            Val::Str(path) => Ok(Val::Str(string_stem(path))),
            other => Err(ExprError::new(format!(
                "stem() needs a file or a path, not {}",
                kind(scope, other)
            ))),
        },
        Func::Digest => {
            let value = super::eval::finish(scope, args.swap_remove(0))?;
            if value.contains_pending() {
                return derive_all(scope, name, vec![value]);
            }
            let plain = value.plain().ok_or_else(ExprError::names_a_step)?;
            let mut hex = digest(&plain);
            hex.truncate(16);
            Ok(Val::Str(hex))
        }
        // Answered before the pending check above.
        Func::Facts => scope.facts(&args[0]),
        Func::Accepted => accepted_of(scope, args.swap_remove(0)),
    }
}

/// `stem()` of a path string: the last `/` segment, without everything from its last `.`
/// (`a/b/c.tar.gz` → `c.tar`, `.bashrc` → `""`, `a/b/` → `""`).
fn string_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(i) => name[..i].to_string(),
        None => name.to_string(),
    }
}

/// `accepted(v)` of an evaluated argument: a pending value derives; a `.*` result is finished
/// first; anything but a collection is refused.
fn accepted_of<S: Scope + ?Sized>(scope: &mut S, value: Val) -> Result<Val, ExprError> {
    let refused = || ExprError::new("accepted() needs a collection of a repeated step");
    let value = match value {
        Val::Pending(p) => return Ok(Val::Pending(Box::new(p.derive("accepted", &[])?))),
        Val::View(view) => match scope.finish_view(view) {
            Ok(finished) => finished,
            Err(e) if e == ExprError::names_a_step() => return Err(refused()),
            Err(e) => return Err(e),
        },
        other => other,
    };
    match value {
        Val::Collection(collection) => accepted(&collection),
        _ => Err(refused()),
    }
}

/// `accepted(c)` of a finished collection: pending while any verdict is undecided (refs: the
/// pending refs inside the items, possibly none), else the items whose verdict is accept or
/// unjudged, keeping the explicit accept verdicts.
pub fn accepted(collection: &Collection) -> Result<Val, ExprError> {
    let verdict = |key: &str| collection.verdicts.get(key);
    let undecided = collection
        .items
        .iter()
        .any(|(key, _)| matches!(verdict(key), Some(None)));
    if undecided {
        // As in gnode, the refs may be empty when every item is known but a verdict is not.
        let mut refs = BTreeSet::new();
        for (_, value) in &collection.items {
            refs.extend(value.pending_refs());
        }
        let whole = Val::Collection(Box::new(collection.clone())).token_form();
        let token = crate::value::object([("accepted", whole)]);
        return Ok(Val::Pending(Box::new(Pending::new(refs, &token))));
    }
    let kept: Vec<(String, Val)> = collection
        .items
        .iter()
        .filter(|(key, _)| matches!(verdict(key), None | Some(Some(Verdict::Accept))))
        .cloned()
        .collect();
    let verdicts = kept
        .iter()
        .filter(|(key, _)| matches!(verdict(key), Some(Some(Verdict::Accept))))
        .map(|(key, _)| (key.clone(), Some(Verdict::Accept)))
        .collect();
    Ok(Val::Collection(Box::new(Collection {
        items: kept,
        verdicts,
    })))
}
