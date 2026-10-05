//! Runtime values: what expressions evaluate to and what an instance's with-values hold
//! (spec/identity.md §3, §5).
//!
//! A [`Val`] is a JSON value or one of the engine's runtime values: a file, a missing value, a
//! failed upstream result, a keyed collection, a pending value, or a step view. Step views
//! ([`Val::View`]) only flow through the evaluator: the expander's scope owns them by
//! [`ViewId`], and `finish` turns them into values or refuses them, so no stored value ever
//! holds one.
//!
//! FX rules where gnode differs: one number type (`f64`, always finite; integers are exact
//! within ±2^53 because readers refuse anything else); booleans are not numbers;
//! `text(missing)` is `""`; numbers render in their JCS form; `plain` follows spec/identity.md §3
//! and a pending value has no plain form (an identity that would hold one is `null`).

use crate::expr::ExprError;
use crate::value::{as_f64, canon, digest, format_number, number, object};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// A step view (`steps.x`, `let`, a `.*` result, …), owned by the expander's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewId(pub usize);

/// What a file's content reads as in expressions: decoded text for text kinds, parsed JSON for
/// JSON kinds; absent for other kinds and for project files named by `./` paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileContent {
    Text(String),
    Json(Value),
}

/// A file known by its digest (spec/identity.md §2–§4).
///
/// Equality compares `digest`, `kind`, `name`, `size` and `key`; never `content` or `location`.
/// `plain` is `{"file": digest}` only.
#[derive(Debug, Clone)]
pub struct FileValue {
    /// `file_digest` of the bytes.
    pub digest: String,
    /// The file kind (identity.md §4).
    pub kind: String,
    /// A display name: a basename for workflow inputs, the project-relative POSIX path for a
    /// `./` project file, the port's name for a step result. Never part of an identity.
    pub name: String,
    pub size: u64,
    /// The key in a keyed collection (`files` inputs get their stem).
    pub key: Option<String>,
    pub content: Option<FileContent>,
    /// Where the engine reads the bytes (facts, prompt files). Absolute; never enters a record,
    /// a message or an output document.
    pub location: Option<PathBuf>,
}

impl PartialEq for FileValue {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest
            && self.kind == other.kind
            && self.name == other.name
            && self.size == other.size
            && self.key == other.key
    }
}

impl FileValue {
    /// The same file under a collection key.
    pub fn with_key(&self, key: &str) -> FileValue {
        FileValue {
            key: Some(key.to_string()),
            ..self.clone()
        }
    }

    /// The name without its last suffix (`x/pic.png` → `pic`, `.bashrc` → `.bashrc`): the key
    /// text of a file (spec/identity.md §11) and `stem()` of a file.
    pub fn stem(&self) -> String {
        path_stem(&self.name)
    }

    /// The file's bytes, read from `location`. The error names the file by its display name,
    /// never by its location.
    pub fn read_bytes(&self) -> Result<Vec<u8>, String> {
        let Some(location) = &self.location else {
            return Err(format!("{} has no local copy", self.name));
        };
        std::fs::read(location).map_err(|e| format!("cannot read {}: {}", self.name, io_reason(&e)))
    }
}

/// A judged take's verdict as the expander knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    Accept,
    Reject,
    /// A judge of the take failed.
    Failed,
}

/// A keyed collection: the finished result of `steps.x.*…`. Keys keep the repeat's order.
/// `verdicts` maps a key to the verdict of the take read for it: absent means unjudged (counts as
/// accepted), `None` means not decided yet (`accepted()` is then pending).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Collection {
    pub items: Vec<(String, Val)>,
    pub verdicts: IndexMap<String, Option<Verdict>>,
}

impl Collection {
    /// The values in order (`for_each` over a collection uses these).
    pub fn values(&self) -> Vec<Val> {
        self.items.iter().map(|(_, v)| v.clone()).collect()
    }

    /// The first item whose key equals `key`.
    pub fn get(&self, key: &str) -> Option<&Val> {
        self.items.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

/// A value only a run can produce.
///
/// `refs` are the instance ids it waits on: they are graph edges and MUST match gnode exactly.
/// A record shows the value by them alone, `{"pending": [refs, sorted]}` ([`Val::shown`]).
/// `token` names how the value is computed; it is internal and never reaches a record or a
/// message, but two textually identical expressions over the same references must get equal
/// tokens. FX computes tokens with gnode's shapes under FX's `digest`: `{"ref": …}` for a
/// reference, `{"op", "of", "extra"}` for a derivation of one pending value, `{"op", "args"}` for
/// an operation over several values, where a pending value inside is `{"pending": token}`
/// ([`Val::token_form`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub refs: BTreeSet<String>,
    pub token: String,
}

impl Pending {
    /// `Pending.of(ref, tok)`: refs `{ref}`, token `digest({"ref": tok or ref})`.
    pub fn of(reference: &str, token: Option<&str>) -> Pending {
        let named = token.unwrap_or(reference);
        Pending {
            refs: BTreeSet::from([reference.to_string()]),
            token: digest(&object([("ref", Value::from(named))])),
        }
    }

    /// A pending value over `refs` whose token is `digest(token_value)`.
    pub fn new(refs: BTreeSet<String>, token_value: &Value) -> Pending {
        Pending {
            refs,
            token: digest(token_value),
        }
    }

    /// `derive(p, op, *extra)`: same refs, token
    /// `digest({"op", "of": token, "extra": token_form(extra)})`.
    /// Refuses a view among `extra` (`this names a step; …`).
    pub fn derive(&self, op: &str, extra: &[Val]) -> Result<Pending, ExprError> {
        if extra.iter().any(Val::contains_view) {
            return Err(ExprError::names_a_step());
        }
        let extra = Value::Array(extra.iter().map(Val::token_form).collect());
        let token = object([
            ("op", Value::from(op)),
            ("of", Value::from(self.token.as_str())),
            ("extra", extra),
        ]);
        Ok(Pending::new(self.refs.clone(), &token))
    }

    /// `derive_all(op, *values)`: refs the union of the pending values' refs, token
    /// `digest({"op", "args": [token_form(v) …]})`. Refuses a view argument.
    ///
    /// The refs are every pending value's refs anywhere inside the values, so a value that only
    /// holds a pending value (a list with a pending element) still waits on it. For a pending
    /// value at the top this is exactly its own refs.
    pub fn derive_all(op: &str, values: &[Val]) -> Result<Pending, ExprError> {
        if values.iter().any(Val::contains_view) {
            return Err(ExprError::names_a_step());
        }
        let mut refs = BTreeSet::new();
        for value in values {
            value.collect_refs(&mut refs);
        }
        let args = Value::Array(values.iter().map(Val::token_form).collect());
        let token = object([("op", Value::from(op)), ("args", args)]);
        Ok(Pending::new(refs, &token))
    }
}

/// A runtime value.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Bool(bool),
    /// Always finite. Normalized to JSON with [`crate::value::number`].
    Number(f64),
    Str(String),
    List(Vec<Val>),
    Object(IndexMap<String, Val>),
    File(Box<FileValue>),
    /// The result of a step that is absent, skipped or rejected (`{"missing": true}`).
    Missing,
    /// A failed upstream result, by instance id (`{"failed": id}`).
    Failed(String),
    Collection(Box<Collection>),
    Pending(Box<Pending>),
    /// A step view; see the module doc.
    View(ViewId),
}

impl Val {
    /// A JSON value as a runtime value (no markers are interpreted: callers have already refused
    /// reserved markers where they read user data).
    pub fn from_json(value: &Value) -> Val {
        match value {
            Value::Null => Val::Null,
            Value::Bool(b) => Val::Bool(*b),
            Value::Number(n) => {
                let x = as_f64(n);
                Val::Number(if x == 0.0 { 0.0 } else { x })
            }
            Value::String(s) => Val::Str(s.clone()),
            Value::Array(items) => Val::List(items.iter().map(Val::from_json).collect()),
            Value::Object(map) => Val::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), Val::from_json(v)))
                    .collect(),
            ),
        }
    }

    /// A number, or `None` for NaN and the infinities.
    pub fn number(x: f64) -> Option<Val> {
        x.is_finite()
            .then_some(Val::Number(if x == 0.0 { 0.0 } else { x }))
    }

    /// The word for this value in type errors (gnode's words for JSON values, FX's words for
    /// runtime values, where gnode printed Python class names): `null`, `a boolean`, `a number`,
    /// `text`, `a list`, `an object`, `a file`, `a missing value`, `a failed result`,
    /// `a collection`, `a pending value`, `a step`.
    pub fn kind_word(&self) -> &'static str {
        match self {
            Val::Null => "null",
            Val::Bool(_) => "a boolean",
            Val::Number(_) => "a number",
            Val::Str(_) => "text",
            Val::List(_) => "a list",
            Val::Object(_) => "an object",
            Val::File(_) => "a file",
            Val::Missing => "a missing value",
            Val::Failed(_) => "a failed result",
            Val::Collection(_) => "a collection",
            Val::Pending(_) => "a pending value",
            Val::View(_) => "a step",
        }
    }

    /// Python's type name as gnode's `an instance key is text or a number, not {name}` prints it
    /// (`NoneType`, `list`, `dict`, `_Missing`, `Failed`, `Collection`, `Pending`).
    pub fn type_name(&self) -> &'static str {
        match self {
            Val::Null => "NoneType",
            Val::Bool(_) => "bool",
            Val::Number(x) if x.fract() == 0.0 => "int",
            Val::Number(_) => "float",
            Val::Str(_) => "str",
            Val::List(_) => "list",
            Val::Object(_) => "dict",
            Val::File(_) => "FileValue",
            Val::Missing => "_Missing",
            Val::Failed(_) => "Failed",
            Val::Collection(_) => "Collection",
            Val::Pending(_) => "Pending",
            Val::View(_) => "step",
        }
    }

    /// `null` or missing: what `??` replaces.
    pub fn is_nothing(&self) -> bool {
        matches!(self, Val::Null | Val::Missing)
    }

    /// Truthiness: booleans themselves; null and missing false; numbers `!= 0`;
    /// text, lists and objects non-empty; files, failed results, collections (even empty) and
    /// views true. Never asked of a pending value.
    pub fn truthy(&self) -> bool {
        match self {
            Val::Bool(b) => *b,
            Val::Null | Val::Missing => false,
            Val::Number(x) => *x != 0.0,
            Val::Str(s) => !s.is_empty(),
            Val::List(items) => !items.is_empty(),
            Val::Object(map) => !map.is_empty(),
            Val::File(_) | Val::Failed(_) | Val::Collection(_) | Val::Pending(_) | Val::View(_) => {
                true
            }
        }
    }

    /// Whether `found` holds for this value or anything inside it (lists, objects, collection
    /// items; never a file's content).
    fn any(&self, found: &impl Fn(&Val) -> bool) -> bool {
        if found(self) {
            return true;
        }
        match self {
            Val::List(items) => items.iter().any(|item| item.any(found)),
            Val::Object(map) => map.values().any(|item| item.any(found)),
            Val::Collection(c) => c.items.iter().any(|(_, item)| item.any(found)),
            _ => false,
        }
    }

    /// The first failed result inside, depth first, in order.
    pub(crate) fn first_failed(&self) -> Option<&str> {
        match self {
            Val::Failed(id) => Some(id),
            Val::List(items) => items.iter().find_map(Val::first_failed),
            Val::Object(map) => map.values().find_map(Val::first_failed),
            Val::Collection(c) => c.items.iter().find_map(|(_, item)| item.first_failed()),
            _ => None,
        }
    }

    fn collect_refs(&self, refs: &mut BTreeSet<String>) {
        match self {
            Val::Pending(p) => refs.extend(p.refs.iter().cloned()),
            Val::List(items) => items.iter().for_each(|item| item.collect_refs(refs)),
            Val::Object(map) => map.values().for_each(|item| item.collect_refs(refs)),
            Val::Collection(c) => c.items.iter().for_each(|(_, item)| item.collect_refs(refs)),
            _ => {}
        }
    }

    /// Whether a pending value is anywhere inside (lists, objects, collection items).
    pub fn contains_pending(&self) -> bool {
        self.any(&|v| matches!(v, Val::Pending(_)))
    }

    /// Whether a failed result is anywhere inside.
    pub fn contains_failed(&self) -> bool {
        self.any(&|v| matches!(v, Val::Failed(_)))
    }

    /// Whether a view is anywhere inside.
    pub fn contains_view(&self) -> bool {
        self.any(&|v| matches!(v, Val::View(_)))
    }

    /// The union of the refs of every pending value inside (an instance's `waiting_on`).
    pub fn pending_refs(&self) -> BTreeSet<String> {
        let mut refs = BTreeSet::new();
        self.collect_refs(&mut refs);
        refs
    }

    /// The plain projection for identities (identity.md §3); `None` when a pending value is inside.
    /// Numbers go through [`crate::value::number`]; objects keep their key order (canon sorts).
    /// A view has no plain form either (it never reaches an identity: `finish` refuses it first).
    pub fn plain(&self) -> Option<Value> {
        if self.any(&|v| matches!(v, Val::Pending(_) | Val::View(_))) {
            return None;
        }
        Some(self.shown())
    }

    /// The plain projection with each pending value shown as what it waits on,
    /// `{"pending": [instance ids, sorted, distinct]}` (its refs): the graph's `with`
    /// (fx-graph-v1) and messages use it, so no engine-internal token reaches a record. Never
    /// called on a value holding a view (a view shows as `null`).
    pub fn shown(&self) -> Value {
        self.show(false)
    }

    /// [`Val::shown`] with each pending value as `{"pending": token}`: what pending tokens are
    /// computed from (two pending values over the same refs stay apart). Internal; never printed.
    pub fn token_form(&self) -> Value {
        self.show(true)
    }

    fn show(&self, tokens: bool) -> Value {
        match self {
            Val::Null | Val::View(_) => Value::Null,
            Val::Bool(b) => Value::Bool(*b),
            Val::Number(x) => number(*x).unwrap_or(Value::Null),
            Val::Str(s) => Value::String(s.clone()),
            Val::List(items) => Value::Array(items.iter().map(|v| v.show(tokens)).collect()),
            Val::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), v.show(tokens)))
                    .collect::<Map<String, Value>>(),
            ),
            Val::File(f) => object([("file", Value::from(f.digest.as_str()))]),
            Val::Missing => object([("missing", Value::Bool(true))]),
            Val::Failed(id) => object([("failed", Value::from(id.as_str()))]),
            Val::Collection(c) => object([(
                "collection",
                Value::Array(
                    c.items
                        .iter()
                        .map(|(k, v)| Value::Array(vec![Value::from(k.as_str()), v.show(tokens)]))
                        .collect(),
                ),
            )]),
            Val::Pending(p) if tokens => object([("pending", Value::from(p.token.as_str()))]),
            Val::Pending(p) => object([(
                "pending",
                Value::Array(p.refs.iter().map(|id| Value::from(id.as_str())).collect()),
            )]),
        }
    }

    /// `text(v)` (identity.md §5 "Rendering"): string itself; `true`/`false`; null and missing
    /// `""`; numbers in JCS form; a text file its content; a JSON file its content compact in its
    /// own key order with JCS numbers; another file `sha256:<digest>`; objects, lists and
    /// collections `canon(plain(v))`. A view is refused with `this names a step; take its result,
    /// e.g. .outputs.image or .facts.verdict`. Callers handle pending and failed values first.
    ///
    /// A pending or failed value that reaches it anyway renders as its shown form
    /// (`{"pending":["a#1"]}`, `{"failed":"a#1"}`), where gnode rendered its token.
    pub fn text(&self) -> Result<String, ExprError> {
        if self.contains_view() {
            return Err(ExprError::names_a_step());
        }
        Ok(match self {
            Val::Str(s) => s.clone(),
            Val::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Val::Null | Val::Missing => String::new(),
            Val::Number(x) => format_number(*x),
            Val::File(f) => match &f.content {
                Some(FileContent::Text(text)) => text.clone(),
                Some(FileContent::Json(content)) => {
                    let mut out = String::new();
                    write_compact(&mut out, content);
                    out
                }
                None => format!("sha256:{}", f.digest),
            },
            other => canon(&other.shown()),
        })
    }

    /// The key text of a repeat item (identity.md §11): a string itself, a boolean
    /// `true`/`false`, a number its JCS form, a file its stem; anything else is refused with
    /// `an instance key is text or a number, not {type_name}`.
    pub fn key_text(&self) -> Result<String, ExprError> {
        match self {
            Val::Str(s) => Ok(s.clone()),
            Val::Bool(b) => Ok(if *b { "true" } else { "false" }.to_string()),
            Val::Number(x) => Ok(format_number(*x)),
            Val::File(f) => Ok(f.stem()),
            Val::View(_) => Err(ExprError::names_a_step()),
            other => Err(ExprError::new(format!(
                "an instance key is text or a number, not {}",
                other.type_name()
            ))),
        }
    }
}

/// Equality as `==` sees it (gnode's, with FX's rules): compares shown forms (pending values by
/// token), so `1 == 1.0`, `true != 1`, files equal by digest, missing equals missing,
/// `null != missing`; two views are equal only when they are the same view.
pub fn plain_eq(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::View(x), Val::View(y)) => x == y,
        (Val::View(_), _) | (_, Val::View(_)) => false,
        _ => a.token_form() == b.token_form(),
    }
}

/// `PurePosixPath(name).stem`: the last path segment without its last suffix. A leading dot
/// does not start a suffix, and neither does a trailing one (`.bashrc`, `a.`).
fn path_stem(name: &str) -> String {
    let last = name
        .split('/')
        .rev()
        .find(|segment| !segment.is_empty() && *segment != ".")
        .unwrap_or("");
    match last.rfind('.') {
        Some(i) if i > 0 && i + 1 < last.len() => last[..i].to_string(),
        _ => last.to_string(),
    }
}

/// The reason of an I/O error without any path in it.
fn io_reason(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "no such file".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => error.to_string(),
    }
}

/// JSON written compactly in the value's own key order, strings and numbers as `canon` writes
/// them (the text of a JSON file, identity.md §5).
fn write_compact(out: &mut String, value: &Value) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_compact(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&canon(&Value::from(key.as_str())));
                out.push(':');
                write_compact(out, item);
            }
            out.push('}');
        }
        scalar => out.push_str(&canon(scalar)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HEX: &str = "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8";

    fn file(name: &str, kind: &str, content: Option<FileContent>) -> Val {
        Val::File(Box::new(FileValue {
            digest: HEX.into(),
            kind: kind.into(),
            name: name.into(),
            size: 8,
            key: None,
            content,
            location: None,
        }))
    }

    fn pending(reference: &str) -> Val {
        Val::Pending(Box::new(Pending::of(reference, None)))
    }

    fn collection(items: Vec<(&str, Val)>) -> Val {
        Val::Collection(Box::new(Collection {
            items: items.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            verdicts: IndexMap::new(),
        }))
    }

    #[test]
    fn plain_forms_follow_identity_section_3() {
        let value = Val::List(vec![
            Val::Failed("draw#1".into()),
            Val::Missing,
            collection(vec![
                ("ada", file("a.txt", "text/plain", None)),
                ("bo", Val::Failed("entity['bo'].draw#1".into())),
            ]),
        ]);
        assert_eq!(
            canon(&value.plain().unwrap()),
            format!(
                r#"[{{"failed":"draw#1"}},{{"missing":true}},{{"collection":[["ada",{{"file":"{HEX}"}}],["bo",{{"failed":"entity['bo'].draw#1"}}]]}}]"#
            )
        );
        // One number type: 1.0 is 1, -0 is 0.
        assert_eq!(Val::Number(1.0).plain(), Some(json!(1)));
        assert_eq!(Val::Number(-0.0).plain(), Some(json!(0)));
        assert_eq!(Val::Number(0.5).plain(), Some(json!(0.5)));
        // A pending value anywhere inside means no plain form.
        assert_eq!(pending("a#1").plain(), None);
        assert_eq!(Val::List(vec![Val::Null, pending("a#1")]).plain(), None);
        assert_eq!(Val::View(ViewId(0)).plain(), None);
    }

    #[test]
    fn shown_forms_show_what_a_pending_value_waits_on() {
        let p = Pending::of("a#1", None);
        let token = p.token.clone();
        let two = Pending::derive_all(
            "+",
            &[
                Val::Pending(Box::new(Pending::of("b['k'].c#2.1", None))),
                Val::Pending(Box::new(p.clone())),
                Val::Pending(Box::new(Pending::of("a#1", None))),
            ],
        )
        .unwrap();
        let value = Val::Object(IndexMap::from([
            ("x".to_string(), Val::Pending(Box::new(p))),
            ("y".to_string(), Val::Str("s".into())),
            (
                "z".to_string(),
                Val::List(vec![Val::Pending(Box::new(two)), Val::Null]),
            ),
        ]));
        // A record names the instances, sorted and distinct; never the internal token.
        assert_eq!(
            value.shown(),
            json!({"x": {"pending": ["a#1"]}, "y": "s", "z": [{"pending": ["a#1", "b['k'].c#2.1"]}, null]})
        );
        assert!(!canon(&value.shown()).contains(&token));
        assert_eq!(value.token_form()["x"], json!({"pending": token}));
        // A pending value whose refs are not known yet waits on nothing the plan can name.
        let none = Pending::new(BTreeSet::new(), &json!({"accepted": []}));
        assert_eq!(Val::Pending(Box::new(none)).shown(), json!({"pending": []}));
        // Text that holds one shows the same form.
        assert_eq!(
            Val::List(vec![pending("a#1")]).text().unwrap(),
            r#"[{"pending":["a#1"]}]"#
        );
    }

    #[test]
    fn equal_refs_are_not_equal_pending_values() {
        let x = Pending::of("a#1", None)
            .derive("field", &[Val::Str("x".into())])
            .unwrap();
        let y = Pending::of("a#1", None)
            .derive("field", &[Val::Str("y".into())])
            .unwrap();
        let (x, y) = (Val::Pending(Box::new(x)), Val::Pending(Box::new(y)));
        assert_eq!(x.shown(), y.shown());
        assert!(!plain_eq(&x, &y));
        assert!(plain_eq(&x, &x.clone()));
        // Tokens over pending values inside lists keep them apart too.
        let over = |v: &Val| Pending::derive_all("join", &[Val::List(vec![v.clone()])]).unwrap();
        assert_ne!(over(&x).token, over(&y).token);
    }

    #[test]
    fn text_follows_identity_section_5() {
        assert_eq!(Val::Str("a".into()).text().unwrap(), "a");
        assert_eq!(Val::Bool(true).text().unwrap(), "true");
        assert_eq!(Val::Null.text().unwrap(), "");
        assert_eq!(Val::Missing.text().unwrap(), "");
        assert_eq!(
            Val::Number(0.1 + 0.2).text().unwrap(),
            "0.30000000000000004"
        );
        assert_eq!(Val::Number(1e21).text().unwrap(), "1e+21");
        assert_eq!(Val::Number(1e16).text().unwrap(), "10000000000000000");
        assert_eq!(Val::Number(1e-7).text().unwrap(), "1e-7");
        assert_eq!(Val::Number(1.0).text().unwrap(), "1");
        assert_eq!(Val::Number(-0.0).text().unwrap(), "0");
        assert_eq!(
            file(
                "n.txt",
                "text/plain",
                Some(FileContent::Text("hi\r\n".into()))
            )
            .text()
            .unwrap(),
            "hi\r\n"
        );
        let content = serde_json::from_str::<Value>(r#"{"z": 1.0, "a": ["é", 1.5]}"#).unwrap();
        assert_eq!(
            file("d.json", "json", Some(FileContent::Json(content)))
                .text()
                .unwrap(),
            r#"{"z":1,"a":["é",1.5]}"#
        );
        assert_eq!(
            file("p.png", "image/png", None).text().unwrap(),
            format!("sha256:{HEX}")
        );
        let object = Val::Object(IndexMap::from([
            ("z".to_string(), Val::Number(1.0)),
            (
                "a".to_string(),
                Val::List(vec![Val::Number(1e16), Val::Null]),
            ),
        ]));
        assert_eq!(
            object.text().unwrap(),
            r#"{"a":[10000000000000000,null],"z":1}"#
        );
        assert_eq!(
            collection(vec![("k", Val::Number(2.0))]).text().unwrap(),
            r#"{"collection":[["k",2]]}"#
        );
        assert_eq!(
            Val::View(ViewId(3)).text().unwrap_err(),
            ExprError::names_a_step()
        );
    }

    #[test]
    fn key_text_follows_identity_section_11() {
        assert_eq!(Val::Str("ada".into()).key_text().unwrap(), "ada");
        assert_eq!(Val::Bool(false).key_text().unwrap(), "false");
        assert_eq!(Val::Number(2.0).key_text().unwrap(), "2");
        assert_eq!(Val::Number(1.5).key_text().unwrap(), "1.5");
        assert_eq!(Val::Number(1e16).key_text().unwrap(), "10000000000000000");
        assert_eq!(Val::Number(-1.0).key_text().unwrap(), "-1");
        assert_eq!(
            file("x/pic.png", "image/png", None).key_text().unwrap(),
            "pic"
        );
        for (value, name) in [
            (Val::Null, "NoneType"),
            (Val::List(vec![]), "list"),
            (Val::Object(IndexMap::new()), "dict"),
            (Val::Missing, "_Missing"),
            (Val::Failed("a#1".into()), "Failed"),
            (collection(vec![]), "Collection"),
            (pending("a#1"), "Pending"),
        ] {
            assert_eq!(
                value.key_text().unwrap_err().0,
                format!("an instance key is text or a number, not {name}")
            );
        }
    }

    #[test]
    fn truthiness() {
        for value in [
            Val::Bool(true),
            Val::Number(-1.0),
            Val::Str("x".into()),
            Val::List(vec![Val::Null]),
            file("a.png", "image/png", None),
            Val::Failed("a#1".into()),
            collection(vec![]),
            Val::View(ViewId(0)),
        ] {
            assert!(value.truthy(), "{value:?}");
        }
        for value in [
            Val::Bool(false),
            Val::Null,
            Val::Missing,
            Val::Number(0.0),
            Val::Str(String::new()),
            Val::List(vec![]),
            Val::Object(IndexMap::new()),
        ] {
            assert!(!value.truthy(), "{value:?}");
        }
    }

    #[test]
    fn equality_compares_shown_forms() {
        assert!(plain_eq(&Val::Number(1.0), &Val::from_json(&json!(1))));
        assert!(!plain_eq(&Val::Bool(true), &Val::Number(1.0)));
        assert!(!plain_eq(&Val::Str("1".into()), &Val::Number(1.0)));
        assert!(plain_eq(&Val::Missing, &Val::Missing));
        assert!(!plain_eq(&Val::Null, &Val::Missing));
        // Files by digest only; objects whatever their key order.
        assert!(plain_eq(
            &file("a.png", "image/png", None),
            &file("b.txt", "text/plain", None)
        ));
        assert!(plain_eq(
            &Val::from_json(&json!({"a": 1, "b": 2})),
            &Val::from_json(&json!({"b": 2, "a": 1}))
        ));
        assert!(plain_eq(&Val::View(ViewId(1)), &Val::View(ViewId(1))));
        assert!(!plain_eq(&Val::View(ViewId(1)), &Val::View(ViewId(2))));
        assert!(!plain_eq(&Val::View(ViewId(1)), &Val::Null));
    }

    #[test]
    fn pending_token_shapes() {
        let p = Pending::of("draw#1", None);
        assert_eq!(p.refs, BTreeSet::from(["draw#1".to_string()]));
        assert_eq!(p.token, digest(&json!({"ref": "draw#1"})));
        assert_eq!(
            Pending::of("<item>", Some("x")).token,
            digest(&json!({"ref": "x"}))
        );
        let field = p.derive("field", &[Val::Str("text".into())]).unwrap();
        assert_eq!(field.refs, p.refs);
        assert_eq!(
            field.token,
            digest(&json!({"op": "field", "of": p.token, "extra": ["text"]}))
        );
        let q = Pending::of("b#1", None);
        let sum = Pending::derive_all("+", &[Val::Pending(Box::new(p.clone())), Val::Number(1.0)])
            .unwrap();
        assert_eq!(sum.refs, p.refs);
        assert_eq!(
            sum.token,
            digest(&json!({"op": "+", "args": [{"pending": p.token}, 1]}))
        );
        let both = Pending::derive_all(
            "text",
            &[
                Val::Str("a".into()),
                Val::List(vec![Val::Pending(Box::new(q.clone()))]),
                Val::Pending(Box::new(p.clone())),
            ],
        )
        .unwrap();
        assert_eq!(
            both.refs,
            BTreeSet::from(["b#1".to_string(), "draw#1".to_string()])
        );
        assert_eq!(
            Pending::derive_all("+", &[Val::View(ViewId(0))]).unwrap_err(),
            ExprError::names_a_step()
        );
        assert_eq!(
            p.derive("index", &[Val::View(ViewId(0))]).unwrap_err(),
            ExprError::names_a_step()
        );
    }

    #[test]
    fn nesting_queries() {
        let value = Val::List(vec![
            collection(vec![("k", pending("a#1"))]),
            Val::Object(IndexMap::from([(
                "f".to_string(),
                Val::Failed("b#1".into()),
            )])),
        ]);
        assert!(value.contains_pending());
        assert!(value.contains_failed());
        assert!(!value.contains_view());
        assert_eq!(value.pending_refs(), BTreeSet::from(["a#1".to_string()]));
        assert_eq!(value.first_failed(), Some("b#1"));
    }

    #[test]
    fn file_stems() {
        let stem = |name: &str| match file(name, "file", None) {
            Val::File(f) => f.stem(),
            _ => unreachable!(),
        };
        assert_eq!(stem("x/pic.png"), "pic");
        assert_eq!(stem(".bashrc"), ".bashrc");
        assert_eq!(stem("a.tar.gz"), "a.tar");
        assert_eq!(stem("noext"), "noext");
        assert_eq!(stem("a."), "a.");
    }

    #[test]
    fn reading_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"one\n").unwrap();
        let mut f = FileValue {
            digest: HEX.into(),
            kind: "text/plain".into(),
            name: "a.txt".into(),
            size: 4,
            key: None,
            content: None,
            location: Some(path),
        };
        assert_eq!(f.read_bytes().unwrap(), b"one\n");
        f.location = Some(dir.path().join("gone.txt"));
        let error = f.read_bytes().unwrap_err();
        assert!(error.starts_with("cannot read a.txt: "), "{error}");
        assert!(!error.contains(&dir.path().display().to_string()));
        f.location = None;
        assert_eq!(f.read_bytes().unwrap_err(), "a.txt has no local copy");
    }

    #[test]
    fn kind_words() {
        assert_eq!(Val::Missing.kind_word(), "a missing value");
        assert_eq!(Val::Failed("a".into()).kind_word(), "a failed result");
        assert_eq!(pending("a").kind_word(), "a pending value");
        assert_eq!(Val::View(ViewId(0)).kind_word(), "a step");
        assert_eq!(file("a", "file", None).kind_word(), "a file");
    }
}
