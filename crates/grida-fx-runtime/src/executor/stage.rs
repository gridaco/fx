//! Staging a `run` (spec/protocol.md §3, §5.3): file refs, inputs, params and the rest of
//! `run`'s params.
//!
//! - [`file_ref`]: `{digest, kind, size, name, key?, path, facts}`; `path` is the store copy;
//!   `facts` are `grida_fx_core::facts::file_facts` of the bytes (a file whose facts cannot be
//!   computed, such as a broken image, is staged with `bytes` and `kind` only, and the reason is
//!   logged, never fatal here: planning already refused what an expression read). Every ref
//!   handed out is also put in the run's [`RunFiles`].
//! - inputs (§3.3): for each declared input present in `with` and not null or missing: one file
//!   → a ref; a list → `{"list": [ref, …]}`; a keyed collection, or an object of files given to a
//!   keyed port → `{"collection": [[key, ref], …]}` in order. Absent optional inputs are left
//!   out.
//! - params (§5.3): each declared param present in `with`, as JSON. A text file
//!   (`kinds::is_text`) arrives as its decoded content (`text::decode_text`: strict UTF-8, one BOM
//!   removed, line endings kept), a JSON file (`kinds::is_json`) as its parsed content; any other
//!   file is `null` at its RFC 6901 pointer and its ref goes to `param_files`; missing is `null`;
//!   a collection is an object by key; a failed value is `null` (a body only runs when nothing it
//!   reads failed).
//! - `work_dir`, `resources` (`{declared: absolute}`), `tools` (`{name: executable or null}`),
//!   `calls` (the resolved bounds), `timeout_s`, `instance {id, path, step, key, take}`, `type`,
//!   `body` (`{path, attribute}` or `{builtin}`).
//!
//! The shape of a staged input follows its port ([`grida_fx_core::spec::Shape`]):
//! - one file: the value must be a file;
//! - a list port takes a list (its items in order), a keyed collection (its items in order) or one
//!   file (a list of one);
//! - a keyed port takes a keyed collection, an object of files (by member name), or a list of
//!   files (each by its key, else by its stem, as a `files` input names them).
//!
//! Null, missing and failed items of a list or a collection are left out: the protocol has no way
//! to hand a body "nothing" inside a list of file refs. Anything else that is not a file fails the
//! attempt with a sentence (`input <name>[<i>] is text, not a file`).
//!
//! Paths handed to the host are absolute UTF-8 strings; a path that is not UTF-8 fails the
//! attempt with a sentence that names what it is, never the path itself.

use super::{InstanceJob, JobBody};
use crate::engine::RunFiles;
use crate::store::Store;
use grida_fx_core::error::io_reason;
use grida_fx_core::kinds;
use grida_fx_core::spec::{Port, Shape};
use grida_fx_core::val::{FileContent, FileValue, Val};
use grida_fx_protocol::{FileRef, RunBody, RunInstance, RunParams, StagedInput};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::path::Path;

/// A file as the host receives it (module doc).
pub fn file_ref(store: &Store, file: &FileValue, files: &RunFiles) -> Result<FileRef, String> {
    let path = store
        .file_path(&file.digest)
        .map_err(|error| format!("{}: {error}", file.name))?;
    let path_text = utf8(&path, || {
        format!(
            "the store's copy of {} is not at a UTF-8 path, which a node host cannot be handed",
            file.name
        )
    })?;
    let bytes = std::fs::read(&path).map_err(|error| {
        format!(
            "cannot read {} from the store: {}",
            file.name,
            io_reason(&error)
        )
    })?;
    let facts = facts_of(&bytes, file);
    let mut handed = file.clone();
    handed.location = Some(path);
    files.insert(&handed);
    Ok(FileRef {
        digest: file.digest.clone(),
        kind: file.kind.clone(),
        size: file.size,
        name: file.name.clone(),
        key: file.key.clone(),
        path: path_text,
        facts,
    })
}

/// The file facts of a staged file; `bytes` and `kind` only when the rest cannot be computed.
fn facts_of(bytes: &[u8], file: &FileValue) -> IndexMap<String, Value> {
    match grida_fx_core::facts::file_facts(bytes, &file.kind) {
        Ok(Value::Object(map)) => map.into_iter().collect(),
        Ok(_) => basic_facts(bytes, file),
        Err(reason) => {
            eprintln!(
                "warning: {}: its file facts cannot be read ({reason}); the body gets its size and kind only",
                file.name
            );
            basic_facts(bytes, file)
        }
    }
}

fn basic_facts(bytes: &[u8], file: &FileValue) -> IndexMap<String, Value> {
    IndexMap::from([
        ("bytes".to_string(), Value::from(bytes.len() as u64)),
        ("kind".to_string(), Value::from(file.kind.as_str())),
    ])
}

/// `run`'s params for one attempt (module doc).
pub fn run_params(
    job: &InstanceJob,
    run_id: &str,
    work_dir: &Path,
    store: &Store,
    files: &RunFiles,
) -> Result<RunParams, String> {
    let body = match &job.body {
        JobBody::Project { path, attribute } => RunBody::Project {
            path: path.clone(),
            attribute: attribute.clone(),
        },
        JobBody::Std { builtin } => RunBody::Builtin {
            builtin: builtin.clone(),
        },
        JobBody::Capability { .. } | JobBody::Select | JobBody::None => {
            return Err(format!("{} has no body a node host runs", job.uses));
        }
    };
    let mut stage = |file: &FileValue| file_ref(store, file, files);
    let read = |file: &FileValue| read_stored(store, file);
    let (inputs, params, param_files) = stage_values(job, &mut stage, &read)?;
    let work_dir = utf8(work_dir, || {
        "the run's work dir is not at a UTF-8 path, which a node host cannot be handed".to_string()
    })?;
    let mut resources = IndexMap::with_capacity(job.resources.len());
    for (declared, path) in &job.resources {
        let path = utf8(path, || {
            format!(
                "the resource {declared} is not at a UTF-8 path, which a node host cannot be handed"
            )
        })?;
        resources.insert(declared.clone(), path);
    }
    let mut tools = IndexMap::with_capacity(job.tools.len());
    for (name, executable) in &job.tools {
        let executable = match executable {
            Some(path) => Some(utf8(path, || {
                format!(
                    "the tool {name} is not at a UTF-8 path, which a node host cannot be handed"
                )
            })?),
            None => None,
        };
        tools.insert(name.clone(), executable);
    }
    Ok(RunParams {
        run_id: run_id.to_string(),
        instance: RunInstance {
            id: job.id.clone(),
            path: job.path.clone(),
            step: job.step.clone(),
            key: job.key.clone(),
            take: job.takes.iter().map(|&t| u64::from(t)).collect(),
        },
        type_: job.type_identity.clone(),
        body,
        params,
        param_files,
        inputs,
        work_dir,
        resources,
        tools,
        calls: job
            .calls
            .iter()
            .map(|(capability, &bound)| (capability.clone(), u64::from(bound)))
            .collect(),
        timeout_s: job.timeout_s,
    })
}

/// What [`stage_values`] produces: inputs, params and param files.
pub(crate) type Staged = (
    IndexMap<String, StagedInput>,
    IndexMap<String, Value>,
    IndexMap<String, FileRef>,
);

/// Stages the declared inputs and params of a job (module doc). `stage` makes a file's ref (and
/// hands it to the run); `read` reads a file's bytes. Both are injected so the conversion can be
/// tested without a store.
pub(crate) fn stage_values(
    job: &InstanceJob,
    stage: &mut dyn FnMut(&FileValue) -> Result<FileRef, String>,
    read: &dyn Fn(&FileValue) -> Result<Vec<u8>, String>,
) -> Result<Staged, String> {
    let mut inputs = IndexMap::new();
    for (name, port) in &job.spec.inputs {
        let Some(value) = job.with.get(name) else {
            continue;
        };
        if let Some(staged) = stage_input(name, port, value, stage)? {
            inputs.insert(name.clone(), staged);
        }
    }
    let mut params = IndexMap::new();
    let mut param_files = IndexMap::new();
    for name in job.spec.params.keys() {
        let Some(value) = job.with.get(name) else {
            continue;
        };
        let mut converter = Params {
            name,
            stage: &mut *stage,
            read,
            param_files: &mut param_files,
        };
        let json = converter.value(value, &format!("/{}", pointer_token(name)))?;
        params.insert(name.clone(), json);
    }
    Ok((inputs, params, param_files))
}

/// Whether a value stands for "nothing here": null, missing, or a failed result.
fn is_absent(value: &Val) -> bool {
    matches!(value, Val::Null | Val::Missing | Val::Failed(_))
}

/// One input port's staged value (module doc); `None` when there is nothing to hand.
fn stage_input(
    name: &str,
    port: &Port,
    value: &Val,
    stage: &mut dyn FnMut(&FileValue) -> Result<FileRef, String>,
) -> Result<Option<StagedInput>, String> {
    if is_absent(value) {
        return Ok(None);
    }
    let staged = match port.shape {
        Shape::One => match value {
            Val::File(file) => StagedInput::One(stage(file)?),
            other => {
                return Err(format!(
                    "input {name} takes one file, not {}",
                    other.kind_word()
                ));
            }
        },
        Shape::List => {
            let items: Vec<&Val> = match value {
                Val::List(items) => items.iter().collect(),
                Val::Collection(collection) => collection.items.iter().map(|(_, v)| v).collect(),
                file @ Val::File(_) => vec![file],
                other => {
                    return Err(format!(
                        "input {name} takes a list of files, not {}",
                        other.kind_word()
                    ));
                }
            };
            let mut list = Vec::with_capacity(items.len());
            for (index, item) in items.into_iter().enumerate() {
                match item {
                    Val::File(file) => list.push(stage(file)?),
                    absent if is_absent(absent) => {}
                    other => {
                        return Err(format!(
                            "input {name}[{index}] is {}, not a file",
                            other.kind_word()
                        ));
                    }
                }
            }
            StagedInput::List { list }
        }
        Shape::Keyed => {
            let items: Vec<(String, &Val)> = match value {
                Val::Collection(collection) => collection
                    .items
                    .iter()
                    .map(|(key, item)| (key.clone(), item))
                    .collect(),
                Val::Object(map) => map.iter().map(|(key, item)| (key.clone(), item)).collect(),
                Val::List(items) => {
                    let mut keyed = Vec::with_capacity(items.len());
                    for (index, item) in items.iter().enumerate() {
                        match item {
                            Val::File(file) => {
                                let key = file.key.clone().unwrap_or_else(|| file.stem());
                                keyed.push((key, item));
                            }
                            absent if is_absent(absent) => {}
                            other => {
                                return Err(format!(
                                    "input {name}[{index}] is {}, not a file",
                                    other.kind_word()
                                ));
                            }
                        }
                    }
                    keyed
                }
                other => {
                    return Err(format!(
                        "input {name} takes a keyed collection of files, not {}",
                        other.kind_word()
                    ));
                }
            };
            let mut collection = Vec::with_capacity(items.len());
            for (key, item) in items {
                match item {
                    Val::File(file) => {
                        let mut file_ref = stage(file)?;
                        file_ref.key = Some(key.clone());
                        collection.push((key, file_ref));
                    }
                    absent if is_absent(absent) => {}
                    other => {
                        return Err(format!(
                            "input {name}[{key}] is {}, not a file",
                            other.kind_word()
                        ));
                    }
                }
            }
            StagedInput::Collection { collection }
        }
    };
    Ok(Some(staged))
}

/// The conversion of one param's value to JSON (module doc).
struct Params<'a> {
    /// The param, for messages.
    name: &'a str,
    stage: &'a mut dyn FnMut(&FileValue) -> Result<FileRef, String>,
    read: &'a dyn Fn(&FileValue) -> Result<Vec<u8>, String>,
    param_files: &'a mut IndexMap<String, FileRef>,
}

impl Params<'_> {
    fn value(&mut self, value: &Val, pointer: &str) -> Result<Value, String> {
        Ok(match value {
            Val::Null | Val::Missing | Val::Failed(_) => Value::Null,
            Val::Bool(b) => Value::Bool(*b),
            Val::Number(x) => {
                grida_fx_core::value::number(*x).map_err(|refused| refused.message)?
            }
            Val::Str(text) => Value::String(text.clone()),
            Val::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for (index, item) in items.iter().enumerate() {
                    out.push(self.value(item, &format!("{pointer}/{index}"))?);
                }
                Value::Array(out)
            }
            Val::Object(map) => {
                let mut out = Map::with_capacity(map.len());
                for (key, item) in map {
                    let json = self.value(item, &format!("{pointer}/{}", pointer_token(key)))?;
                    out.insert(key.clone(), json);
                }
                Value::Object(out)
            }
            Val::Collection(collection) => {
                let mut out = Map::with_capacity(collection.items.len());
                for (key, item) in &collection.items {
                    let json = self.value(item, &format!("{pointer}/{}", pointer_token(key)))?;
                    out.insert(key.clone(), json);
                }
                Value::Object(out)
            }
            Val::File(file) => self.file(file, pointer)?,
            Val::Pending(_) | Val::View(_) => {
                return Err(format!(
                    "setting {} holds {}, which a body cannot be given",
                    self.name,
                    value.kind_word()
                ));
            }
        })
    }

    /// A file inside a param: its content for text and JSON kinds, else `null` and a param file.
    fn file(&mut self, file: &FileValue, pointer: &str) -> Result<Value, String> {
        if kinds::is_text(&file.kind) {
            if let Some(FileContent::Text(text)) = &file.content {
                return Ok(Value::String(text.clone()));
            }
            let bytes = (self.read)(file)?;
            let text = grida_fx_core::text::decode_text(&bytes, &file.name)
                .map_err(|error| error.to_string())?;
            return Ok(Value::String(text));
        }
        if kinds::is_json(&file.kind) {
            if let Some(FileContent::Json(json)) = &file.content {
                return Ok(json.clone());
            }
            let bytes = (self.read)(file)?;
            let text = grida_fx_core::text::decode_text(&bytes, &file.name)
                .map_err(|error| error.to_string())?;
            return grida_fx_core::value::parse_json(&text)
                .map_err(|refused| format!("{}: {}", file.name, refused.message));
        }
        let file_ref = (self.stage)(file)?;
        self.param_files.insert(pointer.to_string(), file_ref);
        Ok(Value::Null)
    }
}

/// One reference token of an RFC 6901 JSON pointer: `~` as `~0`, `/` as `~1`.
pub(crate) fn pointer_token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// A stored file's bytes, read from the store's copy.
fn read_stored(store: &Store, file: &FileValue) -> Result<Vec<u8>, String> {
    let path = store
        .file_path(&file.digest)
        .map_err(|error| format!("{}: {error}", file.name))?;
    std::fs::read(&path).map_err(|error| {
        format!(
            "cannot read {} from the store: {}",
            file.name,
            io_reason(&error)
        )
    })
}

/// A path as UTF-8 text, or the sentence `refused` gives.
fn utf8(path: &Path, refused: impl FnOnce() -> String) -> Result<String, String> {
    path.to_str().map(str::to_string).ok_or_else(refused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::val::Collection;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn file(name: &str, kind: &str, digest_char: char) -> FileValue {
        FileValue {
            digest: std::iter::repeat_n(digest_char, 64).collect(),
            kind: kind.into(),
            name: name.into(),
            size: 3,
            key: None,
            content: None,
            location: None,
        }
    }

    fn spec(inputs: &[(&str, &str)], params: &[&str]) -> grida_fx_core::spec::NodeSpec {
        grida_fx_core::spec::NodeSpec {
            name: "t".into(),
            description: None,
            inputs: inputs
                .iter()
                .map(|(n, p)| (n.to_string(), Port::parse(p).unwrap()))
                .collect(),
            params: params
                .iter()
                .map(|n| (n.to_string(), serde_json::json!({})))
                .collect(),
            outputs: IndexMap::new(),
            judge: false,
            capability: None,
            calls: IndexMap::new(),
            resources: Vec::new(),
            tools: Vec::new(),
            view: None,
            version: Some(1),
            retry: grida_fx_core::spec::Retry::Service,
        }
    }

    fn job(spec: grida_fx_core::spec::NodeSpec, with: Vec<(&str, Val)>) -> InstanceJob {
        InstanceJob {
            id: "a#1".into(),
            path: "a".into(),
            step: "a".into(),
            key: None,
            takes: vec![1],
            uses: "./nodes/n.py#t".into(),
            type_identity: "t".into(),
            spec: Arc::new(spec),
            body: JobBody::Project {
                path: "nodes/n.py".into(),
                attribute: "t".into(),
            },
            with: with.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            identity: None,
            read: Default::default(),
            calls: IndexMap::new(),
            routes: IndexMap::new(),
            limits: IndexMap::new(),
            scopes: Vec::new(),
            resources: IndexMap::new(),
            tools: IndexMap::new(),
            timeout_s: None,
            retry: grida_fx_core::spec::Retry::Service,
            picked: None,
        }
    }

    /// Stages with fake refs (`path` = `/store/<name>`) and bytes from a map by name.
    fn staged(job: &InstanceJob, bytes: &HashMap<&str, &[u8]>) -> Result<Staged, String> {
        let handed = RefCell::new(Vec::new());
        let mut stage = |file: &FileValue| -> Result<FileRef, String> {
            handed.borrow_mut().push(file.name.clone());
            Ok(FileRef {
                digest: file.digest.clone(),
                kind: file.kind.clone(),
                size: file.size,
                name: file.name.clone(),
                key: file.key.clone(),
                path: format!("/store/{}", file.name),
                facts: IndexMap::new(),
            })
        };
        let read = |file: &FileValue| -> Result<Vec<u8>, String> {
            bytes
                .get(file.name.as_str())
                .map(|b| b.to_vec())
                .ok_or_else(|| format!("no bytes for {}", file.name))
        };
        stage_values(job, &mut stage, &read)
    }

    #[test]
    fn text_params_lose_one_bom_and_keep_line_endings() {
        let job = job(
            spec(&[], &["brief"]),
            vec![(
                "brief",
                Val::File(Box::new(file("b.md", "text/markdown", 'a'))),
            )],
        );
        let bytes = HashMap::from([("b.md", "\u{FEFF}\u{FEFF}one\r\ntwo\n".as_bytes())]);
        let (inputs, params, param_files) = staged(&job, &bytes).unwrap();
        assert!(inputs.is_empty());
        assert!(param_files.is_empty());
        assert_eq!(params["brief"], Value::from("\u{FEFF}one\r\ntwo\n"));
    }

    #[test]
    fn text_that_is_not_utf8_fails() {
        let job = job(
            spec(&[], &["brief"]),
            vec![(
                "brief",
                Val::File(Box::new(file("b.txt", "text/plain", 'a'))),
            )],
        );
        let bytes = HashMap::from([("b.txt", &b"\xff\xfe"[..])]);
        assert_eq!(staged(&job, &bytes).unwrap_err(), "b.txt is not UTF-8 text");
    }

    #[test]
    fn json_params_are_parsed_and_content_is_used_when_known() {
        let mut known = file("k.json", "json", 'b');
        known.content = Some(FileContent::Json(serde_json::json!({"from": "content"})));
        let job = job(
            spec(&[], &["doc", "known"]),
            vec![
                ("doc", Val::File(Box::new(file("d.json", "json", 'a')))),
                ("known", Val::File(Box::new(known))),
            ],
        );
        let bytes = HashMap::from([("d.json", &b"\xef\xbb\xbf{\"a\": [1, 2.0]}\n"[..])]);
        let (_, params, param_files) = staged(&job, &bytes).unwrap();
        assert_eq!(params["doc"], serde_json::json!({"a": [1, 2]}));
        assert_eq!(params["known"], serde_json::json!({"from": "content"}));
        assert!(param_files.is_empty());

        let job2 = self::job(
            spec(&[], &["doc"]),
            vec![("doc", Val::File(Box::new(file("d.json", "json", 'a'))))],
        );
        let broken = HashMap::from([("d.json", &b"{\"a\": }"[..])]);
        assert!(staged(&job2, &broken).unwrap_err().starts_with("d.json: "),);
    }

    #[test]
    fn other_files_in_params_become_pointers() {
        let picture = Val::File(Box::new(file("p.png", "image/png", 'c')));
        let brief = Val::Object(IndexMap::from([
            ("look".to_string(), picture.clone()),
            ("a/b~c".to_string(), picture.clone()),
            ("note".to_string(), Val::Str("x".into())),
            ("gone".to_string(), Val::Missing),
            ("broken".to_string(), Val::Failed("z#1".into())),
        ]));
        let list = Val::List(vec![picture.clone(), Val::Number(2.0), Val::Bool(true)]);
        let keyed = Val::Collection(Box::new(Collection {
            items: vec![("k1".into(), picture.clone()), ("k2".into(), Val::Null)],
            verdicts: IndexMap::new(),
        }));
        let job = job(
            spec(&[], &["brief", "list", "keyed", "one", "absent"]),
            vec![
                ("brief", brief),
                ("list", list),
                ("keyed", keyed),
                ("one", picture),
            ],
        );
        let (_, params, param_files) = staged(&job, &HashMap::new()).unwrap();
        assert_eq!(
            Value::Object(params.into_iter().collect()),
            serde_json::json!({
                "brief": {"look": null, "a/b~c": null, "note": "x", "gone": null, "broken": null},
                "list": [null, 2, true],
                "keyed": {"k1": null, "k2": null},
                "one": null,
            })
        );
        assert_eq!(
            param_files.keys().collect::<Vec<_>>(),
            [
                "/brief/look",
                "/brief/a~1b~0c",
                "/list/0",
                "/keyed/k1",
                "/one"
            ]
        );
        assert!(param_files.values().all(|r| r.path == "/store/p.png"));
    }

    #[test]
    fn pending_params_are_refused() {
        let pending = Val::Pending(Box::new(grida_fx_core::val::Pending::of("x#1", None)));
        let job = job(spec(&[], &["p"]), vec![("p", pending)]);
        assert_eq!(
            staged(&job, &HashMap::new()).unwrap_err(),
            "setting p holds a pending value, which a body cannot be given"
        );
    }

    #[test]
    fn inputs_follow_their_ports() {
        let a = file("a.png", "image/png", 'a');
        let mut b = file("b.png", "image/png", 'b');
        b.key = Some("bee".into());
        let fa = Val::File(Box::new(a.clone()));
        let fb = Val::File(Box::new(b.clone()));
        let job = job(
            spec(
                &[
                    ("one", "image"),
                    ("list", "image[]"),
                    ("list_of_one", "image[]"),
                    ("list_from_keyed", "image[]"),
                    ("keyed", "image{}"),
                    ("keyed_object", "image{}"),
                    ("keyed_list", "image{}"),
                    ("absent", "image?"),
                    ("null", "image?"),
                    ("undeclared_in_with", "image?"),
                ],
                &[],
            ),
            vec![
                ("one", fa.clone()),
                (
                    "list",
                    Val::List(vec![fa.clone(), Val::Missing, fb.clone()]),
                ),
                ("list_of_one", fb.clone()),
                (
                    "list_from_keyed",
                    Val::Collection(Box::new(Collection {
                        items: vec![("x".into(), fb.clone()), ("y".into(), fa.clone())],
                        verdicts: IndexMap::new(),
                    })),
                ),
                (
                    "keyed",
                    Val::Collection(Box::new(Collection {
                        items: vec![
                            ("x".into(), fb.clone()),
                            ("gone".into(), Val::Missing),
                            ("y".into(), fa.clone()),
                        ],
                        verdicts: IndexMap::new(),
                    })),
                ),
                (
                    "keyed_object",
                    Val::Object(IndexMap::from([("o".to_string(), fa.clone())])),
                ),
                ("keyed_list", Val::List(vec![fa.clone(), fb.clone()])),
                ("null", Val::Null),
                ("absent", Val::Missing),
            ],
        );
        let (inputs, _, _) = staged(&job, &HashMap::new()).unwrap();
        let json = serde_json::to_value(&inputs).unwrap();
        let keys_of = |port: &str| -> Vec<Value> {
            json[port]["collection"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pair| {
                    serde_json::json!([pair[0], pair[1]["name"], pair[1].get("key").cloned()])
                })
                .collect()
        };
        assert_eq!(json["one"]["name"], "a.png");
        assert!(json["one"].get("key").is_none());
        let names = |port: &str| -> Vec<Value> {
            json[port]["list"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["name"].clone())
                .collect()
        };
        assert_eq!(names("list"), ["a.png", "b.png"]);
        assert_eq!(names("list_of_one"), ["b.png"]);
        assert_eq!(names("list_from_keyed"), ["b.png", "a.png"]);
        assert_eq!(
            keys_of("keyed"),
            [
                serde_json::json!(["x", "b.png", "x"]),
                serde_json::json!(["y", "a.png", "y"])
            ]
        );
        assert_eq!(
            keys_of("keyed_object"),
            [serde_json::json!(["o", "a.png", "o"])]
        );
        // A list given to a keyed port: each file by its key, else its stem.
        assert_eq!(
            keys_of("keyed_list"),
            [
                serde_json::json!(["a", "a.png", "a"]),
                serde_json::json!(["bee", "b.png", "bee"])
            ]
        );
        assert_eq!(
            inputs.keys().collect::<Vec<_>>(),
            [
                "one",
                "list",
                "list_of_one",
                "list_from_keyed",
                "keyed",
                "keyed_object",
                "keyed_list"
            ]
        );
    }

    #[test]
    fn inputs_that_are_not_files_are_refused() {
        let cases = [
            (
                "image",
                Val::Str("x".into()),
                "input i takes one file, not text",
            ),
            (
                "image",
                Val::List(vec![]),
                "input i takes one file, not a list",
            ),
            (
                "image[]",
                Val::List(vec![Val::Number(1.0)]),
                "input i[0] is a number, not a file",
            ),
            (
                "image[]",
                Val::Str("x".into()),
                "input i takes a list of files, not text",
            ),
            (
                "image{}",
                Val::Object(IndexMap::from([("k".to_string(), Val::Bool(true))])),
                "input i[k] is a boolean, not a file",
            ),
            (
                "image{}",
                Val::Number(3.0),
                "input i takes a keyed collection of files, not a number",
            ),
        ];
        for (port, value, message) in cases {
            let job = job(spec(&[("i", port)], &[]), vec![("i", value)]);
            assert_eq!(staged(&job, &HashMap::new()).unwrap_err(), message);
        }
    }

    #[test]
    fn pointer_tokens_escape() {
        assert_eq!(pointer_token("a/b~c"), "a~1b~0c");
        assert_eq!(pointer_token("~1"), "~01");
        assert_eq!(pointer_token("plain"), "plain");
    }
}
