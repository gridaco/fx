//! Workflow inputs: the shorthand compiled to JSON Schema, defaults, validation, files
//! (fx-workflow-v1 `$defs/input*`). The rules are those of stage-gen's engine, with FX's names.
//!
//! - [`compile_inputs`] / [`compile_input`]: snake_case keywords become their JSON Schema
//!   spelling and are copied verbatim, in declared order; `file` → `{"type": "string",
//!   "x-fx-file": {"kind"}}`, `files` → an array of strings with `x-fx-file: {kind, many: true,
//!   glob}`, `list` (its `items`) → an array, `map` (its `values`) → an object's
//!   `additionalProperties`, and a mapping with neither `type` nor `$ref` is a nested object;
//!   `optional: true` without a default → `default: null` and `type: [t, "null"]`. Errors
//!   `<where>: an input is a mapping`, `unknown keyword`, `a list declares its items`,
//!   `a map declares its values`, `unknown type 'x'`, `$ref needs the referenced workflow`
//!   (`<where>` is `inputs.<name>`, `.<member>`, `[]` for items, `{}` for values). `$ref`: the
//!   file is relative to the workflow's home (nearest fx.yaml), a `#/` pointer is required
//!   (`$ref <r> names nothing`), and the referenced declaration replaces the reference.
//! - [`with_defaults`]: defaults at every depth; a nested group left out is filled in when none
//!   of its members is required; list items are filled, map values are not.
//! - [`validate`]: the compiled schema's keywords, with the message texts of Python's
//!   `jsonschema` 4.26, which stage-gen's engine printed (values in [`crate::text::py_repr`];
//!   FX: booleans are not numbers, `2.0` is an integer). Errors sorted by location; the first 8
//!   joined by `"; "`, each `"<location>: <message>"` with location `inputs`, `.name` per key,
//!   `[i]` per index. The subset the compiler emits is implemented by hand (type, enum,
//!   minimum/maximum, exclusive*, multipleOf, min/maxLength in code points, pattern,
//!   min/maxItems, uniqueItems, required, additionalProperties, properties, items); `format` is
//!   annotation only.
//! - [`bind`]: reading inputs files and flags, anchoring paths, binding files.
//! - [`flags`]: workflow input flags on the command line.
//!
//! FX additions: a `$ref` chain that comes back to a reference it is resolving is refused
//! (`$ref <r> refers back to itself`), and a `pattern` that is not a regular expression is
//! refused when it is compiled (`pattern <p> is not a regular expression`).

pub mod bind;
pub mod flags;

use crate::error::{Error, Result};
use crate::val::Val;
use crate::value::Refused;
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};

/// The `$schema` of compiled inputs.
pub const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

/// The tag marking a schema node that holds a file path (protocol.md §4).
pub const FILE_TAG: &str = "x-fx-file";

/// Resolves a `$ref` text to the referenced declaration.
pub type RefResolver<'a> = dyn FnMut(&str) -> Result<Value> + 'a;

/// Shorthand keywords and their JSON Schema spelling, copied verbatim.
const KEYWORDS: [(&str, &str); 16] = [
    ("description", "description"),
    ("default", "default"),
    ("enum", "enum"),
    ("minimum", "minimum"),
    ("maximum", "maximum"),
    ("exclusive_minimum", "exclusiveMinimum"),
    ("exclusive_maximum", "exclusiveMaximum"),
    ("multiple_of", "multipleOf"),
    ("min_length", "minLength"),
    ("max_length", "maxLength"),
    ("pattern", "pattern"),
    ("min_items", "minItems"),
    ("max_items", "maxItems"),
    ("unique_items", "uniqueItems"),
    ("format", "format"),
    ("examples", "examples"),
];

/// Keys of the shorthand that are not copied.
const SHORTHAND: [&str; 6] = ["type", "kind", "items", "values", "glob", "optional"];

fn keyword(key: &str) -> Option<&'static str> {
    KEYWORDS
        .iter()
        .find(|(short, _)| *short == key)
        .map(|(_, long)| *long)
}

/// A field is a mapping with `type` or `$ref`; any other mapping is a nested object.
fn is_field(declared: &Map<String, Value>) -> bool {
    declared.contains_key("type") || declared.contains_key("$ref")
}

/// Compiles a workflow's `inputs:` to one JSON Schema object: `{"$schema", "type": "object",
/// "properties", "additionalProperties": false, "required"?}` (required only when non-empty, in
/// declaration order). Without a resolver, a `$ref` input is an error.
pub fn compile_inputs(
    inputs: &IndexMap<String, Value>,
    resolve_ref: Option<&mut RefResolver<'_>>,
) -> Result<Value> {
    let mut compiler = Compiler {
        resolver: resolve_ref,
        chain: Vec::new(),
    };
    let mut properties = Map::new();
    for (name, declared) in inputs {
        properties.insert(
            name.clone(),
            compiler.compile(declared, &format!("inputs.{name}"))?,
        );
    }
    let mut schema = Map::new();
    schema.insert("$schema".into(), Value::from(DRAFT_2020_12));
    schema.insert("type".into(), Value::from("object"));
    schema.insert("properties".into(), Value::Object(properties));
    schema.insert("additionalProperties".into(), Value::Bool(false));
    let names: Vec<Value> = inputs
        .iter()
        .filter(|(_, declared)| required(declared))
        .map(|(name, _)| Value::from(name.as_str()))
        .collect();
    if !names.is_empty() {
        schema.insert("required".into(), Value::Array(names));
    }
    Ok(Value::Object(schema))
}

/// Compiles one declaration; `where_` starts as `inputs.<name>`.
pub fn compile_input(
    declared: &Value,
    where_: &str,
    resolve_ref: Option<&mut RefResolver<'_>>,
) -> Result<Value> {
    Compiler {
        resolver: resolve_ref,
        chain: Vec::new(),
    }
    .compile(declared, where_)
}

struct Compiler<'r, 'a> {
    resolver: Option<&'r mut RefResolver<'a>>,
    /// The references being resolved, outermost first.
    chain: Vec<String>,
}

impl Compiler<'_, '_> {
    fn compile(&mut self, declared: &Value, where_: &str) -> Result<Value> {
        let Some(declared) = declared.as_object() else {
            return Err(Error::input(format!("{where_}: an input is a mapping")));
        };
        if let Some(reference) = declared.get("$ref") {
            let reference = match reference {
                Value::String(text) => text.clone(),
                other => crate::value::canon(other),
            };
            let Some(resolver) = self.resolver.as_mut() else {
                return Err(Error::input(format!(
                    "{where_}: $ref needs the referenced workflow"
                )));
            };
            if self.chain.contains(&reference) {
                return Err(Error::input(format!(
                    "{where_}: $ref {reference} refers back to itself"
                )));
            }
            let resolved = resolver(&reference)?;
            self.chain.push(reference);
            let compiled = self.compile(&resolved, where_);
            self.chain.pop();
            return compiled;
        }
        if !declared.contains_key("type") {
            let mut properties = Map::new();
            for (name, field) in declared {
                properties.insert(
                    name.clone(),
                    self.compile(field, &format!("{where_}.{name}"))?,
                );
            }
            let mut schema = Map::new();
            schema.insert("type".into(), Value::from("object"));
            schema.insert("properties".into(), Value::Object(properties));
            schema.insert("additionalProperties".into(), Value::Bool(false));
            let names: Vec<Value> = declared
                .iter()
                .filter(|(_, field)| required(field))
                .map(|(name, _)| Value::from(name.as_str()))
                .collect();
            if !names.is_empty() {
                schema.insert("required".into(), Value::Array(names));
            }
            return Ok(Value::Object(schema));
        }
        let kind = &declared["type"];
        let mut unknown: Vec<&str> = declared
            .keys()
            .map(String::as_str)
            .filter(|key| keyword(key).is_none() && !SHORTHAND.contains(key))
            .collect();
        if !unknown.is_empty() {
            unknown.sort_unstable();
            return Err(Error::input(format!(
                "{where_}: unknown keyword {}",
                unknown.join(", ")
            )));
        }
        let mut schema = Map::new();
        for (key, value) in declared {
            if let Some(long) = keyword(key) {
                schema.insert(long.to_string(), value.clone());
            }
        }
        if let Some(Value::String(pattern)) = schema.get("pattern")
            && regex::Regex::new(pattern).is_err()
        {
            return Err(Error::input(format!(
                "{where_}: pattern {} is not a regular expression",
                crate::text::py_repr_str(pattern)
            )));
        }
        let file_kind = || declared.get("kind").cloned().unwrap_or(Value::from("file"));
        match kind.as_str() {
            Some(scalar @ ("string" | "integer" | "number" | "boolean")) => {
                schema.insert("type".into(), Value::from(scalar));
            }
            Some("file") => {
                schema.insert("type".into(), Value::from("string"));
                let mut tag = Map::new();
                tag.insert("kind".into(), file_kind());
                schema.insert(FILE_TAG.into(), Value::Object(tag));
            }
            Some("files") => {
                schema.insert("type".into(), Value::from("array"));
                let mut items = Map::new();
                items.insert("type".into(), Value::from("string"));
                schema.insert("items".into(), Value::Object(items));
                let mut tag = Map::new();
                tag.insert("kind".into(), file_kind());
                tag.insert("many".into(), Value::Bool(true));
                let glob = declared.get("glob").is_some_and(crate::docs::truthy);
                tag.insert("glob".into(), Value::Bool(glob));
                schema.insert(FILE_TAG.into(), Value::Object(tag));
            }
            Some("list") => {
                let items = match declared.get("items") {
                    Some(items @ Value::Object(_)) => items,
                    _ => {
                        return Err(Error::input(format!("{where_}: a list declares its items")));
                    }
                };
                schema.insert("type".into(), Value::from("array"));
                let compiled = self.compile(items, &format!("{where_}[]"))?;
                schema.insert("items".into(), compiled);
            }
            Some("map") => {
                let values = match declared.get("values") {
                    Some(values @ Value::Object(_)) => values,
                    _ => {
                        return Err(Error::input(format!("{where_}: a map declares its values")));
                    }
                };
                schema.insert("type".into(), Value::from("object"));
                let compiled = self.compile(values, &format!("{where_}{{}}"))?;
                schema.insert("additionalProperties".into(), compiled);
            }
            _ => {
                return Err(Error::input(format!(
                    "{where_}: unknown type {}",
                    crate::text::py_repr(kind)
                )));
            }
        }
        if declared.get("optional").is_some_and(crate::docs::truthy)
            && !schema.contains_key("default")
        {
            schema.insert("default".into(), Value::Null);
            let single = schema["type"].clone();
            schema.insert(
                "type".into(),
                Value::Array(vec![single, Value::from("null")]),
            );
        }
        Ok(Value::Object(schema))
    }
}

/// The reserved-marker rule (identity.md §3) over the defaults of a declaration in the
/// shorthand: every `default` of a field, at every depth (`<where>.default`; members
/// `<where>.<name>`, list items `<where>[]`, map values `<where>{}`, as compile errors name them).
/// Declarations themselves are not checked: an input may be named like a marker.
pub(crate) fn check_default_markers(declared: &Value, where_: &str) -> Result<(), Refused> {
    let Some(declared) = declared.as_object() else {
        return Ok(());
    };
    if !is_field(declared) {
        for (name, field) in declared {
            check_default_markers(field, &format!("{where_}.{name}"))?;
        }
        return Ok(());
    }
    if let Some(default) = declared.get("default") {
        crate::value::check_markers(default, &format!("{where_}.default"))?;
    }
    if declared.contains_key("$ref") {
        return Ok(());
    }
    if let Some(items) = declared.get("items") {
        check_default_markers(items, &format!("{where_}[]"))?;
    }
    if let Some(values) = declared.get("values") {
        check_default_markers(values, &format!("{where_}{{}}"))?;
    }
    Ok(())
}

/// Resolves a `$ref` (fx-workflow-v1 `input_ref`): `<file>#/<pointer>`, the file relative to the
/// workflow's home `home` (read with the strict loader), the pointer walked through objects only
/// (`$ref <reference> names nothing`; no `~0`/`~1` unescaping, no array indices). The file must
/// lie inside the project.
pub fn resolve_ref(home: &std::path::Path, reference: &str) -> Result<Value> {
    let nothing = || Error::input(format!("$ref {reference} names nothing"));
    let Some((file, pointer)) = reference.split_once('#') else {
        return Err(nothing());
    };
    if file.is_empty() || !pointer.starts_with('/') {
        return Err(nothing());
    }
    let outside = || Error::input(format!("$ref {reference}: {file} is outside the project"));
    if Path::new(file).is_absolute() {
        return Err(outside());
    }
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let joined = lexical(&home.join(file));
    if !joined.starts_with(&home) {
        return Err(outside());
    }
    if let Ok(real) = joined.canonicalize()
        && !real.starts_with(&home)
    {
        return Err(outside());
    }
    let document = crate::yaml::load_file(&joined, file)?;
    let mut node = &document;
    for part in pointer.split('/').filter(|part| !part.is_empty()) {
        node = node
            .as_object()
            .and_then(|map| map.get(part))
            .ok_or_else(nothing)?;
    }
    check_default_markers(node, &format!("$ref {reference}"))
        .map_err(|refused| Error::input(format!("{file}: {}", refused.message)))?;
    Ok(node.clone())
}

/// `path` with `.` and `..` removed lexically (no file system access).
pub(crate) fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether a declaration is required: a non-mapping is; a nested object is when any member is;
/// a field (also a `$ref`, judged on the referencing mapping) is when it has no `default` and is
/// not `optional`.
pub fn required(declared: &Value) -> bool {
    let Some(declared) = declared.as_object() else {
        return true;
    };
    if !is_field(declared) {
        return declared.values().any(required);
    }
    !declared.contains_key("default") && !declared.get("optional").is_some_and(crate::docs::truthy)
}

/// Whether a schema node is of type `kind`, also when it is optional (`[kind, "null"]`).
pub(crate) fn is_type(schema: &Value, kind: &str) -> bool {
    match schema.get("type") {
        Some(Value::String(t)) => t == kind,
        Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some(kind)),
        _ => false,
    }
}

/// The schema of an object's member: `properties[name]`, else `additionalProperties` when it is
/// a schema, else `{}`.
pub(crate) fn member_schema<'s>(schema: &'s Value, name: &str) -> &'s Value {
    static EMPTY: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let empty = EMPTY.get_or_init(|| Value::Object(Map::new()));
    if let Some(field) = schema.get("properties").and_then(|p| p.get(name)) {
        return field;
    }
    match schema.get("additionalProperties") {
        Some(extra @ Value::Object(_)) => extra,
        _ => empty,
    }
}

/// The schema of an array's items, or `{}`.
pub(crate) fn items_schema(schema: &Value) -> &Value {
    static EMPTY: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    schema
        .get("items")
        .unwrap_or_else(|| EMPTY.get_or_init(|| Value::Object(Map::new())))
}

/// Fills defaults at every depth: a property left out takes its `default`; a nested object left
/// out whose schema requires nothing is filled in from `{}`; given members and list items are
/// filled in recursively (map values are not: they have no `properties`).
pub fn with_defaults(schema: &Value, value: Value) -> Value {
    match value {
        Value::Object(mut map) if is_type(schema, "object") => {
            if let Some(Value::Object(properties)) = schema.get("properties") {
                for (name, field) in properties {
                    match map.get_mut(name) {
                        Some(given) => {
                            let taken = std::mem::take(given);
                            *given = with_defaults(field, taken);
                        }
                        None => {
                            if let Some(default) = field.get("default") {
                                map.insert(name.clone(), default.clone());
                            } else if field.get("properties").is_some()
                                && !field.get("required").is_some_and(crate::docs::truthy)
                            {
                                let filled = with_defaults(field, Value::Object(Map::new()));
                                map.insert(name.clone(), filled);
                            }
                        }
                    }
                }
            }
            Value::Object(map)
        }
        Value::Array(items) if is_type(schema, "array") => {
            let item_schema = items_schema(schema);
            Value::Array(
                items
                    .into_iter()
                    .map(|item| with_defaults(item_schema, item))
                    .collect(),
            )
        }
        other => other,
    }
}

/// [`with_defaults`] over runtime values: the same walk, defaults converted from JSON.
pub(crate) fn with_defaults_val(schema: &Value, value: Val) -> Val {
    match value {
        Val::Object(mut map) if is_type(schema, "object") => {
            if let Some(Value::Object(properties)) = schema.get("properties") {
                for (name, field) in properties {
                    match map.get_mut(name) {
                        Some(given) => {
                            let taken = std::mem::replace(given, Val::Null);
                            *given = with_defaults_val(field, taken);
                        }
                        None => {
                            if let Some(default) = field.get("default") {
                                map.insert(name.clone(), bind::json_to_val(default));
                            } else if field.get("properties").is_some()
                                && !field.get("required").is_some_and(crate::docs::truthy)
                            {
                                let filled = with_defaults_val(field, Val::Object(IndexMap::new()));
                                map.insert(name.clone(), filled);
                            }
                        }
                    }
                }
            }
            Val::Object(map)
        }
        Val::List(items) if is_type(schema, "array") => {
            let item_schema = items_schema(schema);
            Val::List(
                items
                    .into_iter()
                    .map(|item| with_defaults_val(item_schema, item))
                    .collect(),
            )
        }
        other => other,
    }
}

/// Validates a (defaulted, file-names-substituted) value; the error is the joined message text.
pub fn validate(schema: &Value, value: &Value) -> std::result::Result<(), String> {
    validate_val(schema, &bind::json_to_val(value))
}

/// [`validate`] over runtime values (a missing value prints as `MISSING`, as gnode's did).
pub(crate) fn validate_val(schema: &Value, value: &Val) -> std::result::Result<(), String> {
    let mut errors = Vec::new();
    let mut path = Vec::new();
    check(schema, value, &mut path, &mut errors);
    if errors.is_empty() {
        return Ok(());
    }
    errors.sort_by(|a, b| compare_paths(&a.0, &b.0));
    let lines: Vec<String> = errors
        .iter()
        .take(8)
        .map(|(path, message)| format!("{}: {message}", location(path)))
        .collect();
    Err(lines.join("; "))
}

/// One step of a location: an object member or a list index.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Key(String),
    Index(usize),
}

fn location(path: &[Part]) -> String {
    let mut out = String::from("inputs");
    for part in path {
        match part {
            Part::Key(key) => {
                out.push('.');
                out.push_str(key);
            }
            Part::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

/// Python's list comparison of two absolute paths (stable sort keeps keyword order within one
/// location). Keys compare by code point; an index and a key never share a parent.
fn compare_paths(a: &[Part], b: &[Part]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let order = match (x, y) {
            (Part::Key(x), Part::Key(y)) => x.cmp(y),
            (Part::Index(x), Part::Index(y)) => x.cmp(y),
            (Part::Index(_), Part::Key(_)) => Ordering::Less,
            (Part::Key(_), Part::Index(_)) => Ordering::Greater,
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

/// A JSON Schema number as a double, if it is one.
fn num(value: &Value) -> Option<f64> {
    value.as_number().map(crate::value::as_f64)
}

/// The JCS text of a schema number, as messages print limits.
fn number_text(value: &Value) -> String {
    crate::text::py_repr(value)
}

/// Whether a value is of a JSON Schema type (FX: booleans are not numbers, `2.0` is an integer).
fn of_type(value: &Val, kind: &str) -> bool {
    match kind {
        "null" => matches!(value, Val::Null),
        "boolean" => matches!(value, Val::Bool(_)),
        "number" => matches!(value, Val::Number(_)),
        "integer" => matches!(value, Val::Number(x) if x.fract() == 0.0),
        "string" => matches!(value, Val::Str(_)),
        "array" => matches!(value, Val::List(_)),
        "object" => matches!(value, Val::Object(_)),
        _ => false,
    }
}

/// Python's `repr` of a value as jsonschema's messages print it.
pub(crate) fn repr_val(value: &Val) -> String {
    repr(value)
}

fn repr(value: &Val) -> String {
    match value {
        Val::Null => "None".into(),
        Val::Bool(true) => "True".into(),
        Val::Bool(false) => "False".into(),
        Val::Number(x) => match crate::value::number(*x) {
            Ok(n) => crate::text::py_repr(&n),
            Err(_) => x.to_string(),
        },
        Val::Str(s) => crate::text::py_repr_str(s),
        Val::List(items) => {
            let parts: Vec<String> = items.iter().map(repr).collect();
            format!("[{}]", parts.join(", "))
        }
        Val::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", crate::text::py_repr_str(k), repr(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        Val::Missing => "MISSING".into(),
        Val::File(file) => crate::text::py_repr_str(&file.name),
        Val::Failed(id) => format!("Failed({})", crate::text::py_repr_str(id)),
        Val::Collection(collection) => {
            let parts: Vec<String> = collection
                .items
                .iter()
                .map(|(k, v)| format!("({}, {})", crate::text::py_repr_str(k), repr(v)))
                .collect();
            format!("Collection([{}])", parts.join(", "))
        }
        Val::Pending(_) => "Pending".into(),
        Val::View(_) => "View".into(),
    }
}

/// jsonschema's `equal`: numbers by value, booleans never equal to numbers, lists in order,
/// objects regardless of key order.
fn json_equal(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Null, Val::Null) | (Val::Missing, Val::Missing) => true,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Number(x), Val::Number(y)) => x == y,
        (Val::Str(x), Val::Str(y)) => x == y,
        (Val::List(x), Val::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_equal(p, q))
        }
        (Val::Object(x), Val::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_equal(v, w)))
        }
        _ => false,
    }
}

fn check(schema: &Value, value: &Val, path: &mut Vec<Part>, errors: &mut Vec<(Vec<Part>, String)>) {
    let Some(map) = schema.as_object() else {
        return;
    };
    for (key, rule) in map {
        let mut messages: Vec<String> = Vec::new();
        let mut deeper: Vec<(Part, &Value, &Val)> = Vec::new();
        match key.as_str() {
            "type" => {
                let types: Vec<&str> = match rule {
                    Value::String(t) => vec![t.as_str()],
                    Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
                    _ => continue,
                };
                if !types.iter().any(|t| of_type(value, t)) {
                    let names: Vec<String> =
                        types.iter().map(|t| crate::text::py_repr_str(t)).collect();
                    messages.push(format!(
                        "{} is not of type {}",
                        repr(value),
                        names.join(", ")
                    ));
                }
            }
            "enum" => {
                if let Value::Array(choices) = rule {
                    let choices: Vec<Val> = choices.iter().map(bind::json_to_val).collect();
                    if !choices.iter().any(|choice| json_equal(choice, value)) {
                        messages.push(format!(
                            "{} is not one of {}",
                            repr(value),
                            repr(&Val::List(choices))
                        ));
                    }
                }
            }
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf" => {
                let (Val::Number(x), Some(limit)) = (value, num(rule)) else {
                    continue;
                };
                let x = *x;
                let shown = repr(value);
                let limit_text = number_text(rule);
                let message = match key.as_str() {
                    "minimum" if x < limit => {
                        Some(format!("{shown} is less than the minimum of {limit_text}"))
                    }
                    "maximum" if x > limit => Some(format!(
                        "{shown} is greater than the maximum of {limit_text}"
                    )),
                    "exclusiveMinimum" if x <= limit => Some(format!(
                        "{shown} is less than or equal to the minimum of {limit_text}"
                    )),
                    "exclusiveMaximum" if x >= limit => Some(format!(
                        "{shown} is greater than or equal to the maximum of {limit_text}"
                    )),
                    "multipleOf" if !multiple_of(x, limit) => {
                        Some(format!("{shown} is not a multiple of {limit_text}"))
                    }
                    _ => None,
                };
                messages.extend(message);
            }
            "minLength" | "maxLength" | "minItems" | "maxItems" => {
                let length = match value {
                    Val::Str(s) if key.ends_with("Length") => s.chars().count(),
                    Val::List(items) if key.ends_with("Items") => items.len(),
                    _ => continue,
                };
                let Some(limit) = num(rule) else {
                    continue;
                };
                let length = length as f64;
                if key.starts_with("min") && length < limit {
                    let word = if limit == 1.0 {
                        "should be non-empty"
                    } else {
                        "is too short"
                    };
                    messages.push(format!("{} {word}", repr(value)));
                }
                if key.starts_with("max") && length > limit {
                    let word = if limit == 0.0 {
                        "is expected to be empty"
                    } else {
                        "is too long"
                    };
                    messages.push(format!("{} {word}", repr(value)));
                }
            }
            "pattern" => {
                let (Val::Str(s), Value::String(pattern)) = (value, rule) else {
                    continue;
                };
                let matched = regex::Regex::new(pattern).is_ok_and(|re| re.is_match(s));
                if !matched {
                    messages.push(format!(
                        "{} does not match {}",
                        repr(value),
                        crate::text::py_repr_str(pattern)
                    ));
                }
            }
            "uniqueItems" => {
                if let Val::List(items) = value
                    && crate::docs::truthy(rule)
                {
                    let repeated = items
                        .iter()
                        .enumerate()
                        .any(|(i, a)| items[..i].iter().any(|b| json_equal(a, b)));
                    if repeated {
                        messages.push(format!("{} has non-unique elements", repr(value)));
                    }
                }
            }
            "required" => {
                if let (Val::Object(given), Value::Array(names)) = (value, rule) {
                    for name in names.iter().filter_map(Value::as_str) {
                        if !given.contains_key(name) {
                            messages.push(format!(
                                "{} is a required property",
                                crate::text::py_repr_str(name)
                            ));
                        }
                    }
                }
            }
            "properties" => {
                if let (Val::Object(given), Value::Object(properties)) = (value, rule) {
                    for (name, field) in properties {
                        if let Some(member) = given.get(name) {
                            deeper.push((Part::Key(name.clone()), field, member));
                        }
                    }
                }
            }
            "additionalProperties" => {
                let Val::Object(given) = value else {
                    continue;
                };
                let known = map.get("properties").and_then(Value::as_object);
                let mut extras: Vec<&String> = given
                    .keys()
                    .filter(|name| !known.is_some_and(|k| k.contains_key(name.as_str())))
                    .collect();
                match rule {
                    Value::Object(_) => {
                        for name in extras {
                            deeper.push((Part::Key(name.clone()), rule, &given[name.as_str()]));
                        }
                    }
                    Value::Bool(false) if !extras.is_empty() => {
                        extras.sort();
                        let names: Vec<String> =
                            extras.iter().map(|n| crate::text::py_repr_str(n)).collect();
                        let verb = if extras.len() == 1 { "was" } else { "were" };
                        messages.push(format!(
                            "Additional properties are not allowed ({} {verb} unexpected)",
                            names.join(", ")
                        ));
                    }
                    _ => {}
                }
            }
            "items" => {
                if let Val::List(items) = value {
                    for (i, item) in items.iter().enumerate() {
                        deeper.push((Part::Index(i), rule, item));
                    }
                }
            }
            _ => {}
        }
        for message in messages {
            errors.push((path.clone(), message));
        }
        for (part, field, member) in deeper {
            path.push(part);
            check(field, member, path, errors);
            path.pop();
        }
    }
}

/// jsonschema's `multipleOf` with one number type: exact for whole numbers, else the quotient
/// must be a whole number.
fn multiple_of(x: f64, by: f64) -> bool {
    if by == 0.0 {
        return true;
    }
    if x.fract() == 0.0 && by.fract() == 0.0 {
        return x % by == 0.0;
    }
    let quotient = x / by;
    quotient.is_finite() && quotient.fract() == 0.0
}

/// `--` and the name with `_` written as `-`.
pub fn flag_name(name: &str) -> String {
    format!("--{}", name.replace('_', "-"))
}

#[cfg(test)]
mod tests;
