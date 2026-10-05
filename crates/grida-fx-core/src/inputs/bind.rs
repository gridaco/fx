//! Reading the root workflow's inputs and binding files.
//!
//! Root inputs: each `--inputs` file is read with the strict loader (a file that is not a mapping:
//! `<name>: an inputs file maps input names to values`; reserved markers refused), merged
//! shallowly by top-level name in order, then flags on top (paths in flags are relative to the
//! working directory, paths in a file to its folder). An unknown name: `no input named a, b`.
//! Then anchoring (each file path made absolute against where it was written), defaults,
//! validation, and binding files by content (the suffix kind wins, content read for JSON kinds
//! and text kinds up to 1,000,000 bytes, decoded with [`crate::text::decode_text`];
//! `no file <label>` when missing). Errors are [`crate::ErrorKind::Input`] (exit 2). A message
//! never holds an absolute path the user did not type: name files as given (relative to where
//! they were written).
//!
//! FX decisions: `files` with `glob: true` expands relative patterns only (an absolute pattern is
//! refused); declared kinds are not enforced (as stage-gen's engine did): the declared kind
//! names a file only when its suffix is unknown.
//!
//! How files are named in messages: a path given as a flag is named as typed; a path written in
//! an inputs file is named by joining the inputs file's folder, as the user typed the inputs
//! file, with the path as written (`inputs/run.yaml` holding `brief: brief.txt` names
//! `inputs/brief.txt`); a glob match is named the same way from the folder its pattern was
//! written in. Validation sees each file path as its file name. A default that names a file is
//! relative to the working directory.

use super::{FILE_TAG, is_type, items_schema, member_schema, validate_val, with_defaults};
use crate::error::{Error, Result};
use crate::val::{FileContent, FileValue, Val};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

/// The largest text file whose content is read.
const TEXT_LIMIT: usize = 1_000_000;

/// The root workflow's inputs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RootInputs {
    /// As given: merged files and flags, anchored, files bound, no defaults (identity.md §10
    /// `inputs`, plain-projected by the plan digest). An optional input not given is absent.
    pub given: IndexMap<String, Val>,
    /// What `inputs.<name>` reads in expressions: defaults filled in, optional inputs not given
    /// are `null`. Order: names in first-given order, then defaults in schema order.
    pub values: IndexMap<String, Val>,
}

/// Loads the root inputs. `files` are absolute paths of `--inputs` files, labelled in messages by
/// `labels` (as typed); `flags` come from [`super::flags::parse_input_flags`]; `cwd` anchors them.
pub fn load_inputs(
    schema: &Value,
    files: &[(PathBuf, String)],
    flags: IndexMap<String, Value>,
    cwd: &Path,
) -> Result<RootInputs> {
    let mut labels = HashMap::new();
    let mut merged = Map::new();
    for (path, label) in files {
        let document = match crate::yaml::load_file(path, label)? {
            Value::Object(document) => document,
            _ => {
                return Err(Error::input(format!(
                    "{label}: an inputs file maps input names to values"
                )));
            }
        };
        let resolved = resolve(path);
        let base = resolved.parent().map(Path::to_path_buf).unwrap_or(resolved);
        let label_dir = Path::new(label).parent().map(posix).unwrap_or_default();
        let mut anchoring = Anchoring {
            base: &base,
            label_dir: &label_dir,
            labels: &mut labels,
        };
        for (name, value) in document {
            crate::value::check_markers(&value, &format!("inputs.{name}"))
                .map_err(|refused| Error::input(format!("{label}: {}", refused.message)))?;
            let anchored = anchoring.anchor(member_schema(schema, &name), value)?;
            merged.insert(name, anchored);
        }
    }
    let cwd = resolve(cwd);
    let mut anchoring = Anchoring {
        base: &cwd,
        label_dir: "",
        labels: &mut labels,
    };
    for (name, value) in flags {
        let anchored = anchoring.anchor(member_schema(schema, &name), value)?;
        merged.insert(name, anchored);
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    let mut unknown: Vec<&String> = merged
        .keys()
        .filter(|name| !properties.is_some_and(|p| p.contains_key(name.as_str())))
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        let names: Vec<&str> = unknown.iter().map(|n| n.as_str()).collect();
        return Err(Error::input(format!("no input named {}", names.join(", "))));
    }
    let given = Value::Object(merged);
    let defaulted = with_defaults(schema, given.clone());
    let defaulted = anchor_defaults(schema, defaulted, &cwd, &mut labels);
    let checked = json_to_val(&file_names(schema, defaulted.clone()));
    validate_val(schema, &checked).map_err(Error::input)?;
    let mut binder = Binder {
        labels: &labels,
        read: HashMap::new(),
    };
    let values = binder.bind(schema, &defaulted)?;
    let given = binder.bind(schema, &given)?;
    Ok(RootInputs {
        given: into_map(given),
        values: into_map(values),
    })
}

/// Anchors file paths in a value at `base`: every value under an `x-fx-file` tag becomes an
/// absolute path (null stays null); a `files` value is always a list, and with `glob: true` a
/// relative pattern holding `*`, `?` or `[` becomes the files it matches, sorted.
pub fn anchor(schema: &Value, value: Value, base: &Path) -> Result<Value> {
    let base = resolve(base);
    let mut labels = HashMap::new();
    Anchoring {
        base: &base,
        label_dir: "",
        labels: &mut labels,
    }
    .anchor(schema, value)
}

/// Reads one input file as a file value with content. `label` names it.
pub fn read_input_file(path: &Path, label: &str, declared_kind: &str) -> Result<FileValue> {
    if !path.is_file() {
        return Err(Error::input(format!("no file {label}")));
    }
    let bytes = std::fs::read(path).map_err(|error| Error::io(label, &error))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| label.to_string());
    let kind = crate::kinds::effective_kind(&name, declared_kind);
    let content = if crate::kinds::is_json(&kind) {
        let text = crate::text::decode_text(&bytes, label)?;
        let json = crate::value::parse_json(&text)
            .map_err(|refused| Error::input(format!("{label}: {}", refused.message)))?;
        crate::value::check_markers(&json, label)
            .map_err(|refused| Error::input(refused.message))?;
        Some(FileContent::Json(json))
    } else if crate::kinds::is_text(&kind) && bytes.len() <= TEXT_LIMIT {
        Some(FileContent::Text(crate::text::decode_text(&bytes, label)?))
    } else {
        None
    };
    Ok(FileValue {
        digest: crate::value::file_digest(&bytes),
        kind,
        name,
        size: bytes.len() as u64,
        key: None,
        content,
        location: Some(std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())),
    })
}

/// Reads a file a used workflow is given: `(path text, declared kind)` → a file value, or the
/// problem text (`no input file <path>`).
pub type GivenFileReader<'a> = dyn FnMut(&str, &str) -> std::result::Result<FileValue, String> + 'a;

/// Binds the inputs a step gives a used workflow: defaults, then files (values that are null,
/// already files, pending or failed are kept; a path is read through `read`), then validation
/// unless anything is open (files validated as their name). Returns the bound values and the
/// troubles (each a problem text at `<where>.with`). On a binding failure the given values come
/// back unbound, as gnode does.
pub fn bind_given(
    schema: &Value,
    given: IndexMap<String, Val>,
    read: &mut GivenFileReader<'_>,
) -> (IndexMap<String, Val>, Vec<String>) {
    let defaulted = super::with_defaults_val(schema, Val::Object(given.clone()));
    let bound = match bind_open(schema, defaulted, read) {
        Ok(bound) => bound,
        Err(trouble) => return (given, vec![trouble]),
    };
    let mut troubles = Vec::new();
    if !is_open(&bound)
        && let Err(message) = validate_val(schema, &as_checked(&bound))
    {
        troubles.push(message);
    }
    match bound {
        Val::Object(map) => (map, troubles),
        _ => (given, troubles),
    }
}

// ------------------------------------------------------------------------------ helpers

/// A JSON value as a runtime value (numbers as doubles; no marker is interpreted: callers have
/// refused reserved markers where they read user data).
pub(crate) fn json_to_val(value: &Value) -> Val {
    match value {
        Value::Null => Val::Null,
        Value::Bool(b) => Val::Bool(*b),
        Value::Number(n) => Val::Number(crate::value::as_f64(n)),
        Value::String(s) => Val::Str(s.clone()),
        Value::Array(items) => Val::List(items.iter().map(json_to_val).collect()),
        Value::Object(map) => Val::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_val(v)))
                .collect(),
        ),
    }
}

fn into_map(value: Val) -> IndexMap<String, Val> {
    match value {
        Val::Object(map) => map,
        _ => IndexMap::new(),
    }
}

/// A path's components joined by `/` (a leading `/` kept).
fn posix(path: &Path) -> String {
    let mut out = String::new();
    for component in path.components() {
        if component == Component::RootDir {
            out.push('/');
            continue;
        }
        if !out.is_empty() && !out.ends_with('/') {
            out.push('/');
        }
        out.push_str(&component.as_os_str().to_string_lossy());
    }
    out
}

/// `folder/text`, or `text` alone when the folder is empty or the text is absolute.
fn join_label(folder: &str, text: &str) -> String {
    if folder.is_empty() || Path::new(text).is_absolute() {
        text.to_string()
    } else {
        format!("{}/{text}", folder.trim_end_matches('/'))
    }
}

/// Python's `Path.resolve()` (not strict): absolute, symbolic links resolved where the path
/// exists, `.` and `..` applied to the resolved prefix.
pub(crate) fn resolve(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if let Ok(real) = absolute.canonicalize() {
        return real;
    }
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => {
                out.push(part);
                if let Ok(real) = out.canonicalize() {
                    out = real;
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The text of a value used as a path (Python's `str()` for the values gnode accepted).
fn path_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => crate::value::format_number(crate::value::as_f64(n)),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".into(),
        other => crate::value::canon(other),
    }
}

/// The text of a runtime value used as a path.
fn val_path_text(value: &Val) -> String {
    match value {
        Val::Str(s) => s.clone(),
        Val::Number(x) => crate::value::format_number(*x),
        Val::Bool(b) => b.to_string(),
        Val::Null => "null".into(),
        Val::Missing => "MISSING".into(),
        Val::File(file) => file.name.clone(),
        other => super::repr_val(other),
    }
}

fn tag_many(tag: &Value) -> bool {
    tag.get("many").is_some_and(crate::docs::truthy)
}

fn tag_kind(tag: &Value) -> String {
    tag.get("kind")
        .and_then(Value::as_str)
        .unwrap_or("file")
        .to_string()
}

/// A file's name without its last suffix (Python's `Path.stem`): `pic.png` → `pic`,
/// `.bashrc` → `.bashrc`, `a.` → `a.`.
fn stem(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name[..i].to_string(),
        _ => name.to_string(),
    }
}

fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

// ------------------------------------------------------------------------------ anchoring

struct Anchoring<'a> {
    /// Where the paths were written, resolved.
    base: &'a Path,
    /// The same folder as the user named it (`""` for the working directory).
    label_dir: &'a str,
    /// Anchored path → how messages name it.
    labels: &'a mut HashMap<String, String>,
}

impl Anchoring<'_> {
    fn anchor(&mut self, schema: &Value, value: Value) -> Result<Value> {
        if let Some(tag) = schema.get(FILE_TAG) {
            if value.is_null() {
                return Ok(Value::Null);
            }
            if tag_many(tag) {
                let glob = tag.get("glob").is_some_and(crate::docs::truthy);
                let patterns = match value {
                    Value::Array(items) => items,
                    single => vec![single],
                };
                let mut paths = Vec::new();
                for pattern in &patterns {
                    let text = path_text(pattern);
                    if glob && text.contains(['*', '?', '[']) {
                        paths.extend(self.glob(&text)?);
                    } else {
                        paths.push(self.one(&text));
                    }
                }
                return Ok(Value::Array(paths));
            }
            return Ok(self.one(&path_text(&value)));
        }
        match value {
            Value::Object(map) if is_type(schema, "object") => {
                let mut out = Map::new();
                for (name, item) in map {
                    let anchored = self.anchor(member_schema(schema, &name), item)?;
                    out.insert(name, anchored);
                }
                Ok(Value::Object(out))
            }
            Value::Array(items) if is_type(schema, "array") => {
                let item_schema = items_schema(schema);
                let anchored: Result<Vec<Value>> = items
                    .into_iter()
                    .map(|item| self.anchor(item_schema, item))
                    .collect();
                Ok(Value::Array(anchored?))
            }
            other => Ok(other),
        }
    }

    /// One path as written: absolute against `base`, its label recorded.
    fn one(&mut self, text: &str) -> Value {
        let path = resolve(&self.base.join(text))
            .to_string_lossy()
            .into_owned();
        self.labels
            .entry(path.clone())
            .or_insert_with(|| join_label(self.label_dir, text));
        Value::String(path)
    }

    /// A glob pattern relative to `base`: the matching files, sorted, resolved.
    fn glob(&mut self, pattern: &str) -> Result<Vec<Value>> {
        if Path::new(pattern).is_absolute() {
            return Err(Error::input(format!(
                "{pattern}: a glob pattern is relative to where it is written"
            )));
        }
        let mut matches = glob(self.base, pattern)?;
        matches.sort();
        let mut out = Vec::new();
        for found in matches.into_iter().filter(|p| p.is_file()) {
            let relative = found
                .strip_prefix(self.base)
                .map(posix)
                .unwrap_or_else(|_| base_name(&found.to_string_lossy()));
            let path = resolve(&found).to_string_lossy().into_owned();
            self.labels
                .entry(path.clone())
                .or_insert_with(|| join_label(self.label_dir, &relative));
            out.push(Value::String(path));
        }
        Ok(out)
    }
}

/// A file default that is still relative (defaults are not anchored where inputs are written)
/// is relative to the working directory.
fn anchor_defaults(
    schema: &Value,
    value: Value,
    cwd: &Path,
    labels: &mut HashMap<String, String>,
) -> Value {
    if let Some(tag) = schema.get(FILE_TAG) {
        let mut fix = |item: Value| match item {
            Value::String(text) if !Path::new(&text).is_absolute() => {
                let path = resolve(&cwd.join(&text)).to_string_lossy().into_owned();
                labels.entry(path.clone()).or_insert(text);
                Value::String(path)
            }
            other => other,
        };
        return match value {
            Value::Null => Value::Null,
            Value::Array(items) if tag_many(tag) => {
                Value::Array(items.into_iter().map(&mut fix).collect())
            }
            single if tag_many(tag) => Value::Array(vec![fix(single)]),
            single => fix(single),
        };
    }
    match value {
        Value::Object(map) if is_type(schema, "object") => Value::Object(
            map.into_iter()
                .map(|(name, item)| {
                    let anchored = anchor_defaults(member_schema(schema, &name), item, cwd, labels);
                    (name, anchored)
                })
                .collect(),
        ),
        Value::Array(items) if is_type(schema, "array") => {
            let item_schema = items_schema(schema);
            Value::Array(
                items
                    .into_iter()
                    .map(|item| anchor_defaults(item_schema, item, cwd, labels))
                    .collect(),
            )
        }
        other => other,
    }
}

/// What validation sees: each anchored file path as its file name.
fn file_names(schema: &Value, value: Value) -> Value {
    if let Some(tag) = schema.get(FILE_TAG) {
        let name = |item: Value| match item {
            Value::String(path) => Value::String(base_name(&path)),
            other => other,
        };
        return match value {
            Value::Array(items) if tag_many(tag) => {
                Value::Array(items.into_iter().map(name).collect())
            }
            other => name(other),
        };
    }
    match value {
        Value::Object(map) if is_type(schema, "object") => Value::Object(
            map.into_iter()
                .map(|(n, item)| {
                    let named = file_names(member_schema(schema, &n), item);
                    (n, named)
                })
                .collect(),
        ),
        Value::Array(items) if is_type(schema, "array") => {
            let item_schema = items_schema(schema);
            Value::Array(
                items
                    .into_iter()
                    .map(|item| file_names(item_schema, item))
                    .collect(),
            )
        }
        other => other,
    }
}

// ------------------------------------------------------------------------------ binding

struct Binder<'a> {
    labels: &'a HashMap<String, String>,
    /// Files read so far, by anchored path.
    read: HashMap<String, FileValue>,
}

impl Binder<'_> {
    fn file(&mut self, path: &str, kind: &str) -> Result<FileValue> {
        if let Some(found) = self.read.get(path) {
            return Ok(found.clone());
        }
        let label = self
            .labels
            .get(path)
            .cloned()
            .unwrap_or_else(|| base_name(path));
        let file = read_input_file(Path::new(path), &label, kind)?;
        self.read.insert(path.to_string(), file.clone());
        Ok(file)
    }

    fn bind(&mut self, schema: &Value, value: &Value) -> Result<Val> {
        if let Some(tag) = schema.get(FILE_TAG) {
            if value.is_null() {
                return Ok(Val::Null);
            }
            let kind = tag_kind(tag);
            if tag_many(tag) {
                let items = match value {
                    Value::Array(items) => items.clone(),
                    single => vec![single.clone()],
                };
                let mut files = Vec::with_capacity(items.len());
                for item in &items {
                    let file = self.file(&path_text(item), &kind)?;
                    let key = stem(&file.name);
                    files.push(Val::File(Box::new(file.with_key(&key))));
                }
                return Ok(Val::List(files));
            }
            return Ok(Val::File(Box::new(self.file(&path_text(value), &kind)?)));
        }
        match value {
            Value::Object(map) if is_type(schema, "object") => {
                let mut out = IndexMap::with_capacity(map.len());
                for (name, item) in map {
                    out.insert(name.clone(), self.bind(member_schema(schema, name), item)?);
                }
                Ok(Val::Object(out))
            }
            Value::Array(items) if is_type(schema, "array") => {
                let item_schema = items_schema(schema);
                let bound: Result<Vec<Val>> = items
                    .iter()
                    .map(|item| self.bind(item_schema, item))
                    .collect();
                Ok(Val::List(bound?))
            }
            other => Ok(json_to_val(other)),
        }
    }
}

/// Whether a pending value or a failed result is anywhere inside.
fn is_open(value: &Val) -> bool {
    match value {
        Val::Pending(_) | Val::Failed(_) => true,
        Val::List(items) => items.iter().any(is_open),
        Val::Object(map) => map.values().any(is_open),
        Val::Collection(collection) => collection.items.iter().any(|(_, v)| is_open(v)),
        _ => false,
    }
}

/// gnode's `_bind_open`: file inputs read through `read`, open values and files kept.
fn bind_open(
    schema: &Value,
    value: Val,
    read: &mut GivenFileReader<'_>,
) -> std::result::Result<Val, String> {
    if let Some(tag) = schema.get(FILE_TAG) {
        if matches!(value, Val::Null | Val::File(_)) || is_open(&value) {
            return Ok(value);
        }
        let kind = tag_kind(tag);
        if tag_many(tag)
            && let Val::List(items) = value
        {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                if matches!(item, Val::File(_)) || is_open(&item) {
                    out.push(item);
                } else {
                    out.push(Val::File(Box::new(read(&val_path_text(&item), &kind)?)));
                }
            }
            return Ok(Val::List(out));
        }
        return Ok(Val::File(Box::new(read(&val_path_text(&value), &kind)?)));
    }
    match value {
        Val::Object(map) if is_type(schema, "object") => {
            let mut out = IndexMap::with_capacity(map.len());
            for (name, item) in map {
                let bound = bind_open(member_schema(schema, &name), item, read)?;
                out.insert(name, bound);
            }
            Ok(Val::Object(out))
        }
        Val::List(items) if is_type(schema, "array") => {
            let item_schema = items_schema(schema);
            let bound: std::result::Result<Vec<Val>, String> = items
                .into_iter()
                .map(|item| bind_open(item_schema, item, read))
                .collect();
            Ok(Val::List(bound?))
        }
        other => Ok(other),
    }
}

/// What validation sees of bound values: a file as its name.
fn as_checked(value: &Val) -> Val {
    match value {
        Val::File(file) => Val::Str(file.name.clone()),
        Val::List(items) => Val::List(items.iter().map(as_checked).collect()),
        Val::Object(map) => Val::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), as_checked(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

// ------------------------------------------------------------------------------ globs

/// The paths under `base` matching a relative glob pattern, as Python's `Path.glob` finds them:
/// `*`, `?` and `[…]` within one name (hidden names included, case-sensitive), `**` for any
/// number of folders (symbolic links to folders are not followed). Unsorted, distinct.
fn glob(base: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let mut segments = Vec::new();
    for part in pattern.split('/').filter(|p| !p.is_empty() && *p != ".") {
        if part == "**" {
            segments.push(Segment::Any);
        } else if part.contains(['*', '?', '[']) {
            segments.push(Segment::Match(
                fnmatch(part).map_err(|message| Error::input(format!("{pattern}: {message}")))?,
            ));
        } else {
            segments.push(Segment::Name(part.to_string()));
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    walk(base, &segments, &mut out, &mut seen);
    Ok(out)
}

enum Segment {
    Name(String),
    Match(regex::Regex),
    Any,
}

fn walk(folder: &Path, segments: &[Segment], out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let Some((first, rest)) = segments.split_first() else {
        if seen.insert(folder.to_path_buf()) {
            out.push(folder.to_path_buf());
        }
        return;
    };
    match first {
        Segment::Name(name) => {
            let path = folder.join(name);
            if rest.is_empty() {
                if path.exists() {
                    walk(&path, rest, out, seen);
                }
            } else if path.is_dir() {
                walk(&path, rest, out, seen);
            }
        }
        Segment::Match(re) => {
            for (name, path, is_dir) in entries(folder) {
                if re.is_match(&name) && (rest.is_empty() || is_dir) {
                    walk(&path, rest, out, seen);
                }
            }
        }
        Segment::Any => {
            walk(folder, rest, out, seen);
            for (_, path, is_dir) in entries(folder) {
                let link = path
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink());
                if is_dir && !link {
                    walk(&path, segments, out, seen);
                }
            }
        }
    }
}

/// A folder's entries: name, path, and whether it is a folder (following links).
fn entries(folder: &Path) -> Vec<(String, PathBuf, bool)> {
    let Ok(listing) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf, bool)> = listing
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            let path = entry.path();
            let is_dir = path.is_dir();
            (
                entry.file_name().to_string_lossy().into_owned(),
                path,
                is_dir,
            )
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// Python's `fnmatch.translate` of one name pattern, as a whole-name regular expression.
fn fnmatch(pattern: &str) -> std::result::Result<regex::Regex, String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::from("(?s)^");
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '*' => {
                while i < chars.len() && chars[i] == '*' {
                    i += 1;
                }
                out.push_str(".*");
            }
            '?' => out.push('.'),
            '[' => {
                let mut j = i;
                if j < chars.len() && chars[j] == '!' {
                    j += 1;
                }
                if j < chars.len() && chars[j] == ']' {
                    j += 1;
                }
                while j < chars.len() && chars[j] != ']' {
                    j += 1;
                }
                if j >= chars.len() {
                    out.push_str("\\[");
                } else {
                    out.push_str(&char_class(&chars[i..j]));
                    i = j + 1;
                }
            }
            other => out.push_str(&regex::escape(&other.to_string())),
        }
    }
    out.push('$');
    regex::Regex::new(&out).map_err(|_| format!("{pattern} is not a glob pattern"))
}

/// A class that never matches. Python writes `(?!)`, which the `regex` crate refuses (it has no
/// look-around); the class of no character compiles and matches nothing in the same way.
const NEVER: &str = r"[^\x00-\x{10FFFF}]";

/// A bracket expression's contents (`!` negates; `a-z` ranges; reversed ranges match nothing).
fn char_class(stuff: &[char]) -> String {
    let (negated, body) = match stuff.first() {
        Some('!') => (true, &stuff[1..]),
        _ => (false, stuff),
    };
    if body.is_empty() {
        // Unreachable from the scan (a `]` right after `[` or `[!` is literal); kept total.
        return if negated { ".".into() } else { NEVER.into() };
    }
    let escape = |c: char| -> String {
        if c.is_ascii_punctuation() {
            format!("\\{c}")
        } else {
            c.to_string()
        }
    };
    let mut items = Vec::new();
    let mut k = 0;
    while k < body.len() {
        if k + 2 < body.len() && body[k + 1] == '-' {
            let (low, high) = (body[k], body[k + 2]);
            if low <= high {
                items.push(format!("{}-{}", escape(low), escape(high)));
            }
            k += 3;
        } else {
            items.push(escape(body[k]));
            k += 1;
        }
    }
    if items.is_empty() {
        return if negated { ".".into() } else { NEVER.into() };
    }
    format!("[{}{}]", if negated { "^" } else { "" }, items.concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_follow_python() {
        assert_eq!(stem("pic.png"), "pic");
        assert_eq!(stem("a.tar.gz"), "a.tar");
        assert_eq!(stem(".bashrc"), ".bashrc");
        assert_eq!(stem("a."), "a.");
        assert_eq!(stem("plain"), "plain");
    }

    #[test]
    fn fnmatch_translates_like_python() {
        let m = |p: &str, s: &str| fnmatch(p).unwrap().is_match(s);
        assert!(m("*.txt", "a.txt"));
        assert!(m("*.txt", ".hidden.txt"));
        assert!(!m("*.txt", "a.txt.bak"));
        assert!(m("?.png", "a.png"));
        assert!(!m("?.png", "ab.png"));
        assert!(m("[ab].md", "b.md"));
        assert!(!m("[!ab].md", "b.md"));
        assert!(m("[!ab].md", "c.md"));
        assert!(m("[a-c]x", "bx"));
        assert!(m("[]]", "]"));
        assert!(m("a[", "a["));
        assert!(m("x.(1)+", "x.(1)+"));
        assert!(!m("A*", "a"));
    }

    #[test]
    fn a_reversed_range_matches_nothing() {
        // Python: fnmatch.translate('[b-a].png') == '(?s:(?!)\\.png)\\Z'.
        let m = |p: &str, s: &str| fnmatch(p).unwrap().is_match(s);
        for name in [
            "a.png",
            "b.png",
            "-.png",
            ".png",
            "\u{10FFFF}.png",
            "\0.png",
        ] {
            assert!(!m("[b-a].png", name), "{name}");
        }
        // A negated empty class matches any one character, as Python's `.` does.
        assert!(m("[!b-a].png", "a.png"));
        assert!(m("[!b-a].png", "\n.png"));
        assert!(!m("[!b-a].png", ".png"));
        // The rest of a class still counts.
        assert!(m("[b-ax].png", "x.png"));
        assert!(!m("[b-ax].png", "a.png"));
        assert_eq!(char_class(&[]), NEVER);
    }

    #[test]
    fn a_reversed_range_globs_to_nothing_and_other_patterns_still_bind() {
        let folder = tempfile::tempdir().unwrap();
        let pics = folder.path().join("pics");
        std::fs::create_dir(&pics).unwrap();
        for name in ["a.png", "b.png"] {
            std::fs::write(pics.join(name), b"x").unwrap();
        }
        assert!(glob(folder.path(), "pics/[b-a].png").unwrap().is_empty());
        assert_eq!(
            glob(folder.path(), "pics/[a-a].png").unwrap(),
            [pics.join("a.png")]
        );
    }

    #[test]
    fn posix_paths() {
        assert_eq!(posix(Path::new("")), "");
        assert_eq!(posix(Path::new("inputs")), "inputs");
        assert_eq!(posix(Path::new("./a/b")), "./a/b");
        assert_eq!(posix(Path::new("/abs/dir")), "/abs/dir");
        assert_eq!(posix(Path::new("../x")), "../x");
    }

    #[test]
    fn join_label_keeps_what_the_user_typed() {
        assert_eq!(join_label("", "a.txt"), "a.txt");
        assert_eq!(join_label("inputs", "a.txt"), "inputs/a.txt");
        assert_eq!(join_label("inputs", "/abs/a.txt"), "/abs/a.txt");
    }
}
