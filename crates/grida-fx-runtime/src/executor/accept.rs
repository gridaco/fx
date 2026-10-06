//! Accepting a body's result (spec/protocol.md §3.4, §5.3 steps 1–5).
//!
//! 1. facts: those reported with `fact`, in order, then the result's `facts` on top; marks:
//!    those reported with `annotate`, then the result's `marks`;
//! 2. checks: no undeclared output port (`<type name> returned undeclared outputs <names>`), every
//!    non-optional port present (`<type name> did not return its output <port>`), each value's
//!    shape matching its port (`output <port> is a list`, `output <port> is a keyed collection`,
//!    `output <port> is one file`); no fact named `cost_usd`, no reserved marker in a fact or a
//!    mark, and no fact or mark nested deeper than the run's record can hold
//!    (`events::check_depth`); every mark checked as `annotate` checks one (`marks[<i>]: <the
//!    annotate refusal>`). A failed check fails the node and is not retried;
//! 3. an `annotations` port the type declares and the body left out, with marks, gets
//!    `{"kind": "fx-annotations-v1", "annotations": marks}` written by the engine (spec/identity.md
//!    §5 "Writing JSON"), kind `annotations`;
//! 4. (the judge's verdict is checked by the executor);
//! 5. outputs are stored: `{"work_path", "kind"?}` reads the file under the work dir (POSIX,
//!    relative, inside it: `outside_work_dir` otherwise), kind by the suffix rule when not given;
//!    `{"file": ref}` must name a file the run was handed (`unknown_file`) and keeps its digest;
//!    each stored file's display name is `<instance path>/<port>`, `<path>/<port>[<i>]` for a list
//!    item, `<path>/<port>[<key>]` for a keyed item (key set). Values come back as `Val`s with
//!    `location` and `content` set (`Store::file_value`).
//!
//! Details:
//! - `<names>` is the sorted list of undeclared names as Python prints a list of strings
//!   (`['x', 'y']`), as FX's predecessor wrote it; a name given `null` counts too.
//! - The other refusals read `the fact cost_usd is the engine's: a node may not report it`,
//!   the reserved-marker sentence of `value::check_markers` (`fact <name>: …`, `marks[<i>]: …`),
//!   `output <label>: <work_path> is not a file inside the work dir`, `output <label>: cannot
//!   read <work_path>: <reason>`, `output <label> names a file this run was not handed`, and
//!   `output <port> holds the key <key> twice`.
//! - Every check, the work paths and the handed files included, is made before anything is
//!   stored, so a refused result stores nothing.
//! - A passed-through file keeps the engine's digest, kind and size (never what the host wrote in
//!   the ref) and takes the output's display name and key.
//! - The engine's annotations file is written only for a one-file port named `annotations`.
//! - A store that cannot be written is not a refusal of the result: the executor stops the run
//!   ([`accept_or_stop`]); [`accept`] reports it as a refusal for callers that only fail the node.
//!   A file the body wrote that cannot be read, or that changes while it is stored, is the
//!   body's: a refusal (`<instance path>/<port>: cannot read …`, `… changed while it was being
//!   stored`).
//! - [`unreadable_result`] says what is wrong with a `run` result FX cannot read at all.

use super::InstanceJob;
use crate::engine::RunFiles;
use crate::events::check_depth;
use crate::store::records::FileEntry;
use crate::store::{Store, StoreError};
use grida_fx_core::error::io_reason;
use grida_fx_core::kinds;
use grida_fx_core::spec::Shape;
use grida_fx_core::text::py_repr_str;
use grida_fx_core::val::{Collection, Val};
use grida_fx_core::value::{check_markers, write_json};
use grida_fx_protocol::{Mark, OutputValue, PortOutput, RunResult};
use indexmap::IndexMap;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// The kind of the engine's annotations document.
pub const ANNOTATIONS_KIND: &str = "fx-annotations-v1";

/// An accepted result.
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted {
    pub outputs: IndexMap<String, Val>,
    /// Merged node facts (without the engine's `cost_usd`, which the executor adds).
    pub facts: IndexMap<String, Value>,
}

/// A refused result: the node fails with `message` (not retried), keeping `facts` that pass the
/// checks.
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub message: String,
    pub facts: IndexMap<String, Value>,
}

/// Why [`accept_or_stop`] did not accept a result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NotAccepted {
    /// The result breaks a rule: the node fails.
    Refused(Refused),
    /// The store could not be written: the run stops. The facts are those a refusal would keep.
    Store {
        message: String,
        facts: IndexMap<String, Value>,
    },
}

/// Checks and stores a result (module doc).
pub fn accept(
    job: &InstanceJob,
    result: &RunResult,
    reported_facts: &IndexMap<String, Value>,
    reported_marks: &[Mark],
    work_dir: &Path,
    store: &Store,
    files: &RunFiles,
) -> Result<Accepted, Refused> {
    accept_or_stop(
        job,
        result,
        reported_facts,
        reported_marks,
        work_dir,
        store,
        files,
    )
    .map_err(|not| match not {
        NotAccepted::Refused(refused) => refused,
        NotAccepted::Store { message, facts } => Refused { message, facts },
    })
}

/// [`accept`], telling a store failure apart from a refusal.
pub(crate) fn accept_or_stop(
    job: &InstanceJob,
    result: &RunResult,
    reported_facts: &IndexMap<String, Value>,
    reported_marks: &[Mark],
    work_dir: &Path,
    store: &Store,
    files: &RunFiles,
) -> Result<Accepted, NotAccepted> {
    // 1. Facts and marks.
    let mut facts = reported_facts.clone();
    if let Some(given) = &result.facts {
        for (name, value) in given {
            facts.insert(name.clone(), value.clone());
        }
    }
    let mut marks = reported_marks.to_vec();
    marks.extend(result.marks.iter().flatten().cloned());
    let kept = || failure_facts(reported_facts, Some(&facts_object(result.facts.as_ref())));
    let refuse = |message: String| {
        NotAccepted::Refused(Refused {
            message,
            facts: kept(),
        })
    };

    // 2. Checks (outputs, then facts and marks), 3, and the checks of 5: what each port will
    // store.
    let planned = plan_outputs(job, result, &marks).map_err(refuse)?;
    for (name, value) in &facts {
        if name == "cost_usd" {
            return Err(refuse(
                "the fact cost_usd is the engine's: a node may not report it".into(),
            ));
        }
        check_markers(value, &format!("fact {name}")).map_err(|r| refuse(r.message))?;
        check_depth(value, &format!("fact {name}")).map_err(refuse)?;
    }
    for (index, mark) in marks.iter().enumerate() {
        super::requests::check_mark(mark).map_err(|m| refuse(format!("marks[{index}]: {m}")))?;
    }
    let marks_json = marks_value(&marks).map_err(refuse)?;
    if let Value::Array(items) = &marks_json {
        for (index, mark) in items.iter().enumerate() {
            check_markers(mark, &format!("marks[{index}]")).map_err(|r| refuse(r.message))?;
            check_depth(mark, &format!("marks[{index}]")).map_err(refuse)?;
        }
    }

    let planned = resolve(planned, work_dir, files).map_err(refuse)?;

    // 5. Storing.
    let stored = store_outputs(planned, &marks_json, store).map_err(|error| match error {
        // What the body wrote, not the store, is at fault: the node fails.
        StoreError::Source(sentence) => refuse(sentence),
        other => NotAccepted::Store {
            message: other.to_string(),
            facts: kept(),
        },
    })?;
    Ok(Accepted {
        outputs: stored,
        facts,
    })
}

/// The facts of a failed run kept under §5.3's rules: reported facts, then `data.facts` of a
/// `node_failure` or `node_error`, leaving out any that step 2 would refuse.
pub fn failure_facts(
    reported_facts: &IndexMap<String, Value>,
    error_data: Option<&Value>,
) -> IndexMap<String, Value> {
    let mut kept = IndexMap::new();
    let mut keep = |name: &String, value: &Value| {
        if name != "cost_usd"
            && check_markers(value, name).is_ok()
            && check_depth(value, name).is_ok()
        {
            kept.insert(name.clone(), value.clone());
        }
    };
    for (name, value) in reported_facts {
        keep(name, value);
    }
    if let Some(Value::Object(given)) = error_data.and_then(|data| data.get("facts")) {
        for (name, value) in given {
            keep(name, value);
        }
    }
    kept
}

/// The result's facts as an error's `data` would hold them, for [`failure_facts`].
fn facts_object(facts: Option<&IndexMap<String, Value>>) -> Value {
    let facts: serde_json::Map<String, Value> = facts
        .map(|f| f.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    serde_json::json!({ "facts": facts })
}

/// The marks as JSON, in order.
fn marks_value(marks: &[Mark]) -> Result<Value, String> {
    serde_json::to_value(marks).map_err(|error| format!("a mark FX cannot write: {error}"))
}

/// Where one stored output file comes from.
#[derive(Debug)]
enum Source {
    /// A file the body wrote: its resolved path and kind.
    Work { path: PathBuf, kind: String },
    /// A file the run was handed.
    Handed {
        digest: String,
        kind: String,
        size: u64,
    },
    /// The engine's annotations document.
    Annotations,
}

/// What the body gave for one output file, before it is checked against the work dir and the
/// run's files.
#[derive(Debug)]
enum Given<'a> {
    Value(&'a OutputValue),
    Annotations,
}

/// One output file to store: `S` is what it is made from ([`Given`], then [`Source`]).
#[derive(Debug)]
struct Item<S> {
    source: S,
    /// The display name, `<path>/<label>`.
    name: String,
    /// How messages name the output: `<port>`, `<port>[<i>]`, `<port>[<key>]`.
    label: String,
    key: Option<String>,
}

/// One port's planned value.
#[derive(Debug)]
enum Planned<S> {
    One(Item<S>),
    List(Vec<Item<S>>),
    Keyed(Vec<(String, Item<S>)>),
}

impl<S> Planned<S> {
    /// The same value with each item's source mapped.
    fn try_map<T>(
        self,
        f: &mut dyn FnMut(Item<S>) -> Result<Item<T>, String>,
    ) -> Result<Planned<T>, String> {
        Ok(match self {
            Planned::One(item) => Planned::One(f(item)?),
            Planned::List(items) => {
                Planned::List(items.into_iter().map(&mut *f).collect::<Result<_, _>>()?)
            }
            Planned::Keyed(items) => Planned::Keyed(
                items
                    .into_iter()
                    .map(|(key, item)| Ok((key, f(item)?)))
                    .collect::<Result<_, String>>()?,
            ),
        })
    }
}

/// The checks of step 5: each work path and each handed file.
fn resolve(
    planned: Vec<(String, Planned<Given<'_>>)>,
    work_dir: &Path,
    files: &RunFiles,
) -> Result<Vec<(String, Planned<Source>)>, String> {
    let mut resolved = Vec::with_capacity(planned.len());
    for (port, value) in planned {
        let value = value.try_map(&mut |item: Item<Given<'_>>| {
            let source = match item.source {
                Given::Value(value) => source_of(value, &item.label, work_dir, files)?,
                Given::Annotations => Source::Annotations,
            };
            Ok(Item {
                source,
                name: item.name,
                label: item.label,
                key: item.key,
            })
        })?;
        resolved.push((port, value));
    }
    Ok(resolved)
}

/// Step 2 (outputs) and step 3: every port's files, nothing checked against the work dir yet.
fn plan_outputs<'a>(
    job: &InstanceJob,
    result: &'a RunResult,
    marks: &[Mark],
) -> Result<Vec<(String, Planned<Given<'a>>)>, String> {
    let spec = &job.spec;
    let mut undeclared: Vec<&String> = result
        .outputs
        .keys()
        .filter(|name| !spec.outputs.contains_key(*name))
        .collect();
    if !undeclared.is_empty() {
        undeclared.sort();
        let names: Vec<String> = undeclared.iter().map(|n| py_repr_str(n)).collect();
        return Err(format!(
            "{} returned undeclared outputs [{}]",
            spec.name,
            names.join(", ")
        ));
    }
    let mut planned = Vec::with_capacity(spec.outputs.len());
    for (port_name, port) in &spec.outputs {
        let base = format!("{}/{port_name}", job.path);
        let Some(value) = result.outputs.get(port_name).and_then(Option::as_ref) else {
            if port_name == "annotations" && port.shape == Shape::One && !marks.is_empty() {
                planned.push((
                    port_name.clone(),
                    Planned::One(Item {
                        source: Given::Annotations,
                        name: base,
                        label: port_name.clone(),
                        key: None,
                    }),
                ));
                continue;
            }
            if !port.optional {
                return Err(format!(
                    "{} did not return its output {port_name}",
                    spec.name
                ));
            }
            continue;
        };
        let one = |item: &'a OutputValue, label: &str, key: Option<String>| Item {
            source: Given::Value(item),
            name: format!("{}/{label}", job.path),
            label: label.to_string(),
            key,
        };
        let value = match (port.shape, value) {
            (Shape::One, PortOutput::One(item)) => Planned::One(one(item, port_name, None)),
            (Shape::One, _) => return Err(format!("output {port_name} is one file")),
            (Shape::List, PortOutput::List { list }) => {
                let mut items = Vec::with_capacity(list.len());
                for (index, item) in list.iter().enumerate() {
                    items.push(one(item, &format!("{port_name}[{index}]"), None));
                }
                Planned::List(items)
            }
            (Shape::List, _) => return Err(format!("output {port_name} is a list")),
            (Shape::Keyed, PortOutput::Collection { collection }) => {
                let mut items: Vec<(String, Item<Given<'a>>)> =
                    Vec::with_capacity(collection.len());
                for (key, item) in collection {
                    if items.iter().any(|(seen, _)| seen == key) {
                        return Err(format!(
                            "output {port_name} holds the key {} twice",
                            py_repr_str(key)
                        ));
                    }
                    let label = format!("{port_name}[{key}]");
                    items.push((key.clone(), one(item, &label, Some(key.clone()))));
                }
                Planned::Keyed(items)
            }
            (Shape::Keyed, _) => return Err(format!("output {port_name} is a keyed collection")),
        };
        planned.push((port_name.clone(), value));
    }
    Ok(planned)
}

/// Where an output file comes from, checked (module doc).
fn source_of(
    item: &OutputValue,
    label: &str,
    work_dir: &Path,
    files: &RunFiles,
) -> Result<Source, String> {
    match item {
        OutputValue::Work { work_path, kind } => {
            let path = inside_work_dir(work_dir, work_path).ok_or_else(|| {
                format!("output {label}: {work_path} is not a file inside the work dir")
            })?;
            if let Err(error) = std::fs::File::open(&path) {
                return Err(format!(
                    "output {label}: cannot read {work_path}: {}",
                    io_reason(&error)
                ));
            }
            Ok(Source::Work {
                path,
                kind: kind
                    .clone()
                    .unwrap_or_else(|| kinds::kind_of(work_path).to_string()),
            })
        }
        OutputValue::File { file } => {
            let handed = files
                .get(&file.digest)
                .ok_or_else(|| format!("output {label} names a file this run was not handed"))?;
            Ok(Source::Handed {
                digest: handed.digest,
                kind: handed.kind,
                size: handed.size,
            })
        }
    }
}

/// The existing regular file `work_path` names under `work_dir`, resolved; `None` when the path
/// is not POSIX and relative, holds `..`, leaves the work dir once links are resolved, or names
/// no regular file.
pub(crate) fn inside_work_dir(work_dir: &Path, work_path: &str) -> Option<PathBuf> {
    if work_path.is_empty() || work_path.starts_with('/') {
        return None;
    }
    if cfg!(windows) && (work_path.contains('\\') || work_path.contains(':')) {
        return None;
    }
    let relative = Path::new(work_path);
    if !relative
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let root = std::fs::canonicalize(work_dir).ok()?;
    let resolved = std::fs::canonicalize(root.join(relative)).ok()?;
    if !resolved.starts_with(&root) || resolved == root {
        return None;
    }
    std::fs::metadata(&resolved)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|_| resolved)
}

/// Step 5: stores each planned file and returns the values (module doc). An error is the
/// store's, or a [`StoreError::Source`] for a file the body wrote.
fn store_outputs(
    planned: Vec<(String, Planned<Source>)>,
    marks: &Value,
    store: &Store,
) -> Result<IndexMap<String, Val>, StoreError> {
    let mut outputs = IndexMap::with_capacity(planned.len());
    for (port, value) in planned {
        let value = match value {
            Planned::One(item) => store_item(item, marks, store)?,
            Planned::List(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(store_item(item, marks, store)?);
                }
                Val::List(values)
            }
            Planned::Keyed(items) => {
                let mut collection = Collection::default();
                for (key, item) in items {
                    collection
                        .items
                        .push((key, store_item(item, marks, store)?));
                }
                Val::Collection(Box::new(collection))
            }
        };
        outputs.insert(port, value);
    }
    Ok(outputs)
}

/// Stores one file and reads it back as a value.
fn store_item(item: Item<Source>, marks: &Value, store: &Store) -> Result<Val, StoreError> {
    let (digest, kind, size) = match item.source {
        Source::Work { path, kind } => {
            let stored = store.put_file(&path, &item.name)?;
            (stored.digest, kind, stored.size)
        }
        Source::Handed { digest, kind, size } => (digest, kind, size),
        Source::Annotations => {
            let document = serde_json::json!({
                "kind": ANNOTATIONS_KIND,
                "annotations": marks,
            });
            let stored = store.put_bytes(write_json(&document).as_bytes())?;
            (stored.digest, "annotations".to_string(), stored.size)
        }
    };
    let entry = FileEntry {
        digest,
        kind,
        name: item.name,
        size,
        key: item.key,
    };
    let file = store.file_value(&entry)?;
    Ok(Val::File(Box::new(file)))
}

/// What is wrong with a `run` result that does not read as one (`error` is why it did not), as
/// a sentence that names the place: the first member that is not what spec/protocol.md §3.4 and
/// §5.3 describe.
pub fn unreadable_result(value: &Value, error: &serde_json::Error) -> String {
    let Value::Object(result) = value else {
        return "the result is not an object".to_string();
    };
    if let Some(name) = result
        .keys()
        .find(|name| !matches!(name.as_str(), "outputs" | "facts" | "marks"))
    {
        return format!("the result has a member {name}, which is not outputs, facts or marks");
    }
    let Some(Value::Object(outputs)) = result.get("outputs") else {
        return "the result's outputs is not an object of ports".to_string();
    };
    for (port, output) in outputs {
        if let Some(problem) = unreadable_output(port, output) {
            return problem;
        }
    }
    match result.get("facts") {
        None | Some(Value::Null | Value::Object(_)) => {}
        Some(_) => return "the result's facts is not an object".to_string(),
    }
    match result.get("marks") {
        None | Some(Value::Null) => {}
        Some(Value::Array(marks)) => {
            for (index, mark) in marks.iter().enumerate() {
                if let Err(error) = serde_json::from_value::<Mark>(mark.clone()) {
                    return format!("marks[{index}]: {}", crate::host::read_reason(&error));
                }
            }
        }
        Some(_) => return "the result's marks is not a list".to_string(),
    }
    crate::host::read_reason(error)
}

/// What is wrong with one port's value, when something is.
fn unreadable_output(port: &str, output: &Value) -> Option<String> {
    let one = |label: &str, item: &Value| unreadable_file(label, item);
    match output {
        Value::Null => None,
        Value::Object(members) if members.contains_key("list") => match &members["list"] {
            Value::Array(items) if members.len() == 1 => items
                .iter()
                .enumerate()
                .find_map(|(index, item)| one(&format!("{port}[{index}]"), item)),
            _ => Some(format!("output {port}: a list is {{\"list\": [file, …]}}")),
        },
        Value::Object(members) if members.contains_key("collection") => {
            let pairs = match &members["collection"] {
                Value::Array(pairs) if members.len() == 1 => pairs,
                _ => {
                    return Some(format!(
                        "output {port}: a keyed collection is {{\"collection\": [[key, file], …]}}"
                    ));
                }
            };
            pairs
                .iter()
                .find_map(|pair| match pair.as_array().map(Vec::as_slice) {
                    Some([Value::String(key), item]) => one(&format!("{port}[{key}]"), item),
                    _ => Some(format!(
                        "output {port}: a keyed collection's item is [key, file]"
                    )),
                })
        }
        item => one(port, item),
    }
}

/// What is wrong with one output file, when something is.
fn unreadable_file(label: &str, item: &Value) -> Option<String> {
    let shapes = "a file is {\"work_path\", \"kind\"?} or {\"file\": <ref>}";
    let Value::Object(members) = item else {
        return Some(format!("output {label}: {shapes}"));
    };
    if let Some(reference) = members.get("file") {
        if members.len() != 1 {
            return Some(format!("output {label}: {shapes}"));
        }
        return serde_json::from_value::<grida_fx_protocol::FileRef>(reference.clone())
            .err()
            .map(|error| {
                let reason = crate::host::read_reason(&error);
                match reason.strip_prefix("it ") {
                    Some(rest) => format!("output {label}: the file ref {rest}"),
                    None => format!("output {label}: the file ref is not one: {reason}"),
                }
            });
    }
    let work_path_ok = matches!(members.get("work_path"), Some(Value::String(_)));
    let kind_ok = matches!(members.get("kind"), None | Some(Value::String(_)));
    let only_known = members
        .keys()
        .all(|name| matches!(name.as_str(), "work_path" | "kind"));
    if work_path_ok && kind_ok && only_known {
        None
    } else {
        Some(format!("output {label}: {shapes}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::JobBody;
    use grida_fx_core::spec::{NodeSpec, Port, Retry};
    use grida_fx_core::val::FileValue;
    use grida_fx_protocol::FileRef;
    use serde_json::json;
    use std::sync::Arc;

    fn job(outputs: &[(&str, &str)]) -> InstanceJob {
        let spec = NodeSpec {
            name: "maker".into(),
            description: None,
            inputs: IndexMap::new(),
            params: IndexMap::new(),
            outputs: outputs
                .iter()
                .map(|(n, p)| (n.to_string(), Port::parse(p).unwrap()))
                .collect(),
            judge: false,
            capability: None,
            calls: IndexMap::new(),
            resources: Vec::new(),
            tools: Vec::new(),
            view: None,
            version: Some(1),
            retry: Retry::Service,
        };
        InstanceJob {
            id: "a['k']#1".into(),
            path: "a['k']".into(),
            step: "a".into(),
            key: Some("k".into()),
            takes: vec![1],
            uses: "./nodes/n.py#maker".into(),
            type_identity: "t".into(),
            spec: Arc::new(spec),
            body: JobBody::Project {
                path: "nodes/n.py".into(),
                attribute: "maker".into(),
            },
            with: IndexMap::new(),
            identity: None,
            read: Default::default(),
            calls: IndexMap::new(),
            routes: IndexMap::new(),
            limits: IndexMap::new(),
            scopes: Vec::new(),
            resources: IndexMap::new(),
            tools: IndexMap::new(),
            timeout_s: None,
            retry: Retry::Service,
            picked: None,
        }
    }

    fn result(value: Value) -> RunResult {
        serde_json::from_value(value).unwrap()
    }

    fn refused(job: &InstanceJob, result: &RunResult, work: &Path) -> Refused {
        let store = Store::open(&work.join("store"));
        accept(
            job,
            result,
            &IndexMap::new(),
            &[],
            work,
            &store,
            &RunFiles::new(),
        )
        .unwrap_err()
    }

    #[test]
    fn undeclared_outputs_are_named_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let job = job(&[("x", "text?")]);
        let r = result(json!({"outputs": {"z": null, "y": {"work_path": "a.txt"}}}));
        assert_eq!(
            refused(&job, &r, dir.path()).message,
            "maker returned undeclared outputs ['y', 'z']"
        );
    }

    #[test]
    fn required_outputs_must_be_present() {
        let dir = tempfile::tempdir().unwrap();
        let job = job(&[("a", "text?"), ("x", "text")]);
        for outputs in [json!({}), json!({"x": null})] {
            let r = result(json!({ "outputs": outputs }));
            assert_eq!(
                refused(&job, &r, dir.path()).message,
                "maker did not return its output x"
            );
        }
    }

    #[test]
    fn shapes_must_match_their_ports() {
        let dir = tempfile::tempdir().unwrap();
        let cases = [
            (
                "text[]",
                json!({"work_path": "a.txt"}),
                "output x is a list",
            ),
            (
                "text{}",
                json!({"list": [{"work_path": "a.txt"}]}),
                "output x is a keyed collection",
            ),
            (
                "text",
                json!({"list": [{"work_path": "a.txt"}]}),
                "output x is one file",
            ),
            (
                "text",
                json!({"collection": [["k", {"work_path": "a.txt"}]]}),
                "output x is one file",
            ),
            (
                "text[]",
                json!({"collection": [["k", {"work_path": "a.txt"}]]}),
                "output x is a list",
            ),
        ];
        for (port, value, message) in cases {
            let job = job(&[("x", port)]);
            let r = result(json!({"outputs": {"x": value}}));
            assert_eq!(refused(&job, &r, dir.path()).message, message, "{port}");
        }
    }

    #[test]
    fn the_engine_owns_cost_usd_and_markers_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let job = job(&[("x", "text?")]);
        let r = result(json!({"outputs": {}, "facts": {"score": 3, "cost_usd": 1}}));
        let refusal = refused(&job, &r, dir.path());
        assert_eq!(
            refusal.message,
            "the fact cost_usd is the engine's: a node may not report it"
        );
        assert_eq!(
            refusal.facts,
            IndexMap::from([("score".to_string(), json!(3))])
        );

        let digest = "a".repeat(64);
        let r = result(json!({"outputs": {}, "facts": {"ok": 1, "bad": {"file": digest}}}));
        let refusal = refused(&job, &r, dir.path());
        assert_eq!(
            refusal.message,
            "fact bad: an object holding only `file` in this shape is reserved for FX's own values"
        );
        assert_eq!(
            refusal.facts,
            IndexMap::from([("ok".to_string(), json!(1))])
        );

        let r =
            result(json!({"outputs": {}, "marks": [{"label": "a"}, {"extra": {"missing": true}}]}));
        assert_eq!(
            refused(&job, &r, dir.path()).message,
            "marks[1].extra: an object holding only `missing` in this shape is reserved for FX's own values"
        );
    }

    #[test]
    fn result_marks_are_checked_as_annotate_checks_them() {
        let dir = tempfile::tempdir().unwrap();
        let job = job(&[("annotations", "annotations?")]);
        let r = result(
            json!({"outputs": {}, "marks": [{"shape": "box", "box": [0, 0, 1, 1]},
                                                       {"shape": "point"}]}),
        );
        assert_eq!(
            refused(&job, &r, dir.path()).message,
            "marks[1]: a point mark needs at: [x, y]"
        );
    }

    #[test]
    fn facts_and_marks_the_record_cannot_hold_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let job = job(&[("x", "text?")]);
        let deep = |levels: usize| {
            let mut value = json!(1);
            for _ in 0..levels {
                value = json!([value]);
            }
            value
        };
        let held = result(json!({"outputs": {}, "facts": {"x": deep(crate::events::VALUE_DEPTH)}}));
        let store = Store::open(&dir.path().join("store"));
        let accepted = accept(
            &job,
            &held,
            &IndexMap::new(),
            &[],
            dir.path(),
            &store,
            &RunFiles::new(),
        );
        assert!(accepted.is_ok(), "{accepted:?}");
        let r = result(json!({"outputs": {}, "facts": {"ok": 1, "x": deep(510)}}));
        let refusal = refused(&job, &r, dir.path());
        assert_eq!(
            refusal.message,
            "fact x is nested deeper than 509 levels, which the run's record cannot hold"
        );
        assert_eq!(
            refusal.facts,
            IndexMap::from([("ok".to_string(), json!(1))])
        );
        let r = result(json!({"outputs": {}, "marks": [{"label": "a", "extra": deep(510)}]}));
        assert_eq!(
            refused(&job, &r, dir.path()).message,
            "marks[0] is nested deeper than 509 levels, which the run's record cannot hold"
        );
    }

    #[test]
    fn an_output_the_body_cannot_have_read_fails_the_node() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("secret.txt"), "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                work.join("secret.txt"),
                std::fs::Permissions::from_mode(0o000),
            )
            .unwrap();
            if std::fs::File::open(work.join("secret.txt")).is_ok() {
                // Running as a user that reads anything: nothing to check.
                return;
            }
            let job = job(&[("x", "text")]);
            let r = result(json!({"outputs": {"x": {"work_path": "secret.txt"}}}));
            let store = Store::open(&dir.path().join("store"));
            let not = accept_or_stop(
                &job,
                &r,
                &IndexMap::new(),
                &[],
                &work,
                &store,
                &RunFiles::new(),
            )
            .unwrap_err();
            assert!(
                matches!(&not, NotAccepted::Refused(refused)
                    if refused.message.contains("cannot read secret.txt: permission denied")),
                "{not:?}"
            );
        }
    }

    #[test]
    fn a_result_fx_cannot_read_says_where() {
        let reason = |value: Value| {
            let error = serde_json::from_value::<RunResult>(value.clone()).unwrap_err();
            unreadable_result(&value, &error)
        };
        assert_eq!(reason(json!([])), "the result is not an object");
        assert_eq!(
            reason(json!({"outputs": {}, "notes": 1})),
            "the result has a member notes, which is not outputs, facts or marks"
        );
        assert_eq!(
            reason(json!({"facts": {}})),
            "the result's outputs is not an object of ports"
        );
        assert_eq!(
            reason(json!({"outputs": {"text": "out/a.txt"}})),
            "output text: a file is {\"work_path\", \"kind\"?} or {\"file\": <ref>}"
        );
        assert_eq!(
            reason(json!({"outputs": {"seq": {"list": [{"work_path": "a"}, {"path": "b"}]}}})),
            "output seq[1]: a file is {\"work_path\", \"kind\"?} or {\"file\": <ref>}"
        );
        assert_eq!(
            reason(json!({"outputs": {"all": {"collection": [["k"]]}}})),
            "output all: a keyed collection's item is [key, file]"
        );
        assert_eq!(
            reason(json!({"outputs": {"x": {"file": {"digest": "d"}}}})),
            "output x: the file ref has no kind"
        );
        assert_eq!(
            reason(json!({"outputs": {}, "facts": []})),
            "the result's facts is not an object"
        );
        assert_eq!(
            reason(json!({"outputs": {}, "marks": {}})),
            "the result's marks is not a list"
        );
    }

    #[test]
    fn reported_facts_come_first_and_the_result_wins() {
        let dir = tempfile::tempdir().unwrap();
        let job = job(&[("x", "text")]);
        let r = result(json!({"outputs": {}, "facts": {"b": 2, "a": 9}}));
        let reported = IndexMap::from([("a".to_string(), json!(1)), ("c".to_string(), json!(3))]);
        let store = Store::open(&dir.path().join("store"));
        let refusal = accept(
            &job,
            &r,
            &reported,
            &[],
            dir.path(),
            &store,
            &RunFiles::new(),
        )
        .unwrap_err();
        assert_eq!(refusal.message, "maker did not return its output x");
        assert_eq!(
            refusal.facts,
            IndexMap::from([
                ("a".to_string(), json!(9)),
                ("c".to_string(), json!(3)),
                ("b".to_string(), json!(2)),
            ])
        );
    }

    #[test]
    fn work_paths_stay_inside_the_work_dir() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(work.join("sub")).unwrap();
        std::fs::write(work.join("sub/a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("outside.txt"), "o").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("outside.txt"), work.join("link.txt")).unwrap();
        assert!(inside_work_dir(&work, "sub/a.txt").is_some());
        assert!(inside_work_dir(&work, "./sub/a.txt").is_some());
        for bad in [
            "",
            "/etc/passwd",
            "../outside.txt",
            "sub/../../outside.txt",
            "sub/../sub/a.txt",
            "sub",
            ".",
            "missing.txt",
        ] {
            assert!(inside_work_dir(&work, bad).is_none(), "{bad:?}");
        }
        #[cfg(unix)]
        assert!(inside_work_dir(&work, "link.txt").is_none());

        let job = job(&[("x", "text")]);
        let r = result(json!({"outputs": {"x": {"work_path": "../outside.txt"}}}));
        assert_eq!(
            refused(&job, &r, &work).message,
            "output x: ../outside.txt is not a file inside the work dir"
        );
        let job = self::job(&[("x", "text[]")]);
        let r = result(
            json!({"outputs": {"x": {"list": [{"work_path": "sub/a.txt"}, {"work_path": "nope"}]}}}),
        );
        assert_eq!(
            refused(&job, &r, &work).message,
            "output x[1]: nope is not a file inside the work dir"
        );
    }

    #[test]
    fn passed_through_files_must_have_been_handed() {
        let dir = tempfile::tempdir().unwrap();
        let job = job(&[("x", "image{}")]);
        let file_ref = FileRef {
            digest: "b".repeat(64),
            kind: "image/png".into(),
            size: 4,
            name: "p.png".into(),
            key: None,
            path: "/x".into(),
            facts: IndexMap::new(),
        };
        let r = result(json!({"outputs": {"x": {"collection": [["k1", {"file": file_ref}]]}}}));
        assert_eq!(
            refused(&job, &r, dir.path()).message,
            "output x[k1] names a file this run was not handed"
        );
        let r = result(json!({"outputs": {"x": {"collection": [
            ["k", {"file": file_ref}], ["k", {"file": file_ref}]
        ]}}}));
        assert_eq!(
            refused(&job, &r, dir.path()).message,
            "output x holds the key 'k' twice"
        );
    }

    #[test]
    fn failure_facts_drop_what_the_checks_would_refuse() {
        let reported = IndexMap::from([
            ("a".to_string(), json!(1)),
            ("cost_usd".to_string(), json!(2)),
        ]);
        let data = json!({"facts": {"b": {"missing": true}, "c": [3], "a": 4}, "exception": "X"});
        assert_eq!(
            failure_facts(&reported, Some(&data)),
            IndexMap::from([("a".to_string(), json!(4)), ("c".to_string(), json!([3]))])
        );
        assert_eq!(
            failure_facts(&reported, Some(&json!({"facts": [1]}))),
            IndexMap::from([("a".to_string(), json!(1))])
        );
        assert_eq!(
            failure_facts(&IndexMap::new(), None),
            IndexMap::<String, Value>::new()
        );
    }

    // --------------------------------------------------------------------- with the store

    fn handed(store: &Store, bytes: &[u8], kind: &str, name: &str) -> FileValue {
        let stored = store.put_bytes(bytes).unwrap();
        FileValue {
            digest: stored.digest,
            kind: kind.into(),
            name: name.into(),
            size: stored.size,
            key: None,
            content: None,
            location: None,
        }
    }

    #[test]
    fn outputs_are_stored_under_their_display_names() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("a.txt"), "alpha").unwrap();
        std::fs::write(work.join("b.json"), "{\"b\": 1}").unwrap();
        let store = Store::open(&dir.path().join("store"));
        let files = RunFiles::new();
        let input = handed(&store, b"png!", "image/png", "in.png");
        files.insert(&input);
        let job = job(&[
            ("text", "text"),
            ("parts", "file[]"),
            ("keyed", "image{}"),
            ("none", "text?"),
        ]);
        let r = result(json!({"outputs": {
            "text": {"work_path": "a.txt"},
            "parts": {"list": [{"work_path": "b.json"}, {"work_path": "a.txt", "kind": "text/markdown"}]},
            "keyed": {"collection": [["hero", {"file": {
                "digest": input.digest, "kind": "text/plain", "size": 1, "name": "x", "path": "/x", "facts": {}
            }}]]},
        }}));
        let accepted = accept(&job, &r, &IndexMap::new(), &[], &work, &store, &files).unwrap();
        let Val::File(text) = &accepted.outputs["text"] else {
            panic!("one file");
        };
        assert_eq!(text.name, "a['k']/text");
        assert_eq!(text.kind, "text/plain");
        assert_eq!(
            text.content,
            Some(grida_fx_core::val::FileContent::Text("alpha".into()))
        );
        assert!(text.location.as_ref().unwrap().starts_with(store.root()));
        let Val::List(parts) = &accepted.outputs["parts"] else {
            panic!("a list");
        };
        let names: Vec<(&str, &str)> = parts
            .iter()
            .map(|v| match v {
                Val::File(f) => (f.name.as_str(), f.kind.as_str()),
                _ => panic!("files"),
            })
            .collect();
        assert_eq!(
            names,
            [
                ("a['k']/parts[0]", "json"),
                ("a['k']/parts[1]", "text/markdown")
            ]
        );
        let Val::Collection(keyed) = &accepted.outputs["keyed"] else {
            panic!("a collection");
        };
        let Val::File(hero) = &keyed.items[0].1 else {
            panic!("a file");
        };
        assert_eq!(keyed.items[0].0, "hero");
        assert_eq!(hero.digest, input.digest);
        assert_eq!(hero.kind, "image/png");
        assert_eq!(hero.size, 4);
        assert_eq!(hero.name, "a['k']/keyed[hero]");
        assert_eq!(hero.key.as_deref(), Some("hero"));
        assert!(!accepted.outputs.contains_key("none"));
    }

    #[test]
    fn marks_become_the_annotations_output() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let store = Store::open(&dir.path().join("store"));
        let job = job(&[("annotations", "annotations")]);
        let reported = vec![Mark {
            label: Some("first".into()),
            ..Mark::default()
        }];
        let r = result(
            json!({"outputs": {}, "marks": [{"shape": "point", "at": [0.5, 1.0], "tag": "t"}]}),
        );
        let accepted = accept(
            &job,
            &r,
            &IndexMap::new(),
            &reported,
            &work,
            &store,
            &RunFiles::new(),
        )
        .unwrap();
        let Val::File(file) = &accepted.outputs["annotations"] else {
            panic!("one file");
        };
        assert_eq!(file.kind, "annotations");
        assert_eq!(file.name, "a['k']/annotations");
        let bytes = std::fs::read(file.location.as_ref().unwrap()).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "{\n \"annotations\": [\n  {\n   \"label\": \"first\"\n  },\n  {\n   \"at\": [\n    0.5,\n    1\n   ],\n   \"shape\": \"point\",\n   \"tag\": \"t\"\n  }\n ],\n \"kind\": \"fx-annotations-v1\"\n}"
        );

        // Without marks, a required annotations port is still missing.
        let refusal = accept(
            &job,
            &result(json!({"outputs": {}})),
            &IndexMap::new(),
            &[],
            &work,
            &store,
            &RunFiles::new(),
        )
        .unwrap_err();
        assert_eq!(
            refusal.message,
            "maker did not return its output annotations"
        );
    }
}
