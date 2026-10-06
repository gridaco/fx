//! The paid built-ins (`fx/image.generate@1`, …): body-less types whose one capability call the
//! engine makes itself (spec/protocol.md §4 "Engine types"). No host is involved.
//!
//! - The request is the instance's params without `vars` (spent on rendering the prompt;
//!   spec/identity.md §9), with each input port's value on top: a file as `{"file": digest}`, a
//!   list as a list of them, a keyed collection as an object by key. A text or JSON file given to a
//!   param is its content. The result is the canonical request (the conformance case
//!   `cache-replay` pins its key).
//! - The call goes through `calls::call` like any other (bound 1).
//! - Outputs: each declared output port takes the answer's file of its name, else the first file
//!   whose kind's family matches the port's; the file is stored again under the node's own display
//!   name (`<path>/<port>`). When the answer's `data` is an object: a string `verdict` becomes the
//!   fact `verdict`; each member of `facts` becomes a fact; a declared `json` output the answer did
//!   not fill is written from `data.json` (or the whole `data`) as engine JSON.
//! - The `cost_usd` fact and the result record are the executor's (as for any body).
//!
//! Details of the request: params come in the instance's with-value order, then inputs. An input
//! (or an item of a list or collection) that is missing or `null` is left out: a provider is
//! never sent a marker. Any other file given to a param is `{"file": digest}`; a missing param
//! value is `null`; a failed or pending value cannot be sent and is refused. Every file the
//! request names is handed to the run (and copied into the store when it is not there yet, so an
//! adapter can read it). A fact named `cost_usd` in `data.facts` is left out: that fact is the
//! engine's.

use super::{InstanceJob, JobBody};
use crate::calls::{CallCounter, CallError};
use crate::engine::{Cancel, RunFiles, Services};
use crate::store::records::FileEntry;
use grida_fx_core::kinds::{family, is_json, is_text};
use grida_fx_core::text::decode_text;
use grida_fx_core::val::{FileContent, FileValue, Val};
use grida_fx_core::value::{parse_json, write_json};
use grida_fx_protocol::{ErrorCode, RpcError};
use indexmap::IndexMap;
use serde_json::{Map, Value};

/// What a paid built-in produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Produced {
    pub outputs: IndexMap<String, Val>,
    pub facts: IndexMap<String, Value>,
}

/// The param the engine spends on rendering and never sends.
const VARS: &str = "vars";

/// The fact that is the engine's alone.
const COST_FACT: &str = "cost_usd";

/// The canonical request of a paid built-in (module doc).
pub fn request_of(job: &InstanceJob) -> Result<Value, String> {
    let mut request = Map::new();
    for (name, value) in &job.with {
        if name == VARS || !job.spec.params.contains_key(name) {
            continue;
        }
        request.insert(name.clone(), param_json(value, name)?);
    }
    for (name, value) in &job.with {
        if !job.spec.inputs.contains_key(name) {
            continue;
        }
        if let Some(value) = input_json(value, name)? {
            request.insert(name.clone(), value);
        }
    }
    Ok(Value::Object(request))
}

/// A param's value as the provider receives it.
fn param_json(value: &Val, where_: &str) -> Result<Value, String> {
    Ok(match value {
        Val::Null | Val::Missing => Value::Null,
        Val::File(file) => match file_content(file)? {
            Some(FileContent::Text(text)) => Value::String(text),
            Some(FileContent::Json(content)) => content,
            None => file_json(file),
        },
        Val::List(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(i, item)| param_json(item, &format!("{where_}[{i}]")))
                .collect::<Result<_, _>>()?,
        ),
        Val::Object(members) => Value::Object(
            members
                .iter()
                .map(|(k, v)| Ok((k.clone(), param_json(v, &format!("{where_}.{k}"))?)))
                .collect::<Result<_, String>>()?,
        ),
        Val::Collection(collection) => Value::Object(
            collection
                .items
                .iter()
                .map(|(k, v)| Ok((k.clone(), param_json(v, &format!("{where_}[{k}]"))?)))
                .collect::<Result<_, String>>()?,
        ),
        Val::Failed(_) | Val::Pending(_) | Val::View(_) => return Err(unsendable(value, where_)),
        scalar => scalar.plain().ok_or_else(|| unsendable(value, where_))?,
    })
}

/// An input port's value as the provider receives it: `None` when there is nothing to send.
fn input_json(value: &Val, where_: &str) -> Result<Option<Value>, String> {
    Ok(match value {
        Val::Null | Val::Missing => None,
        Val::File(file) => Some(file_json(file)),
        Val::List(items) => {
            let mut sent = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                if let Some(item) = input_json(item, &format!("{where_}[{i}]"))? {
                    sent.push(item);
                }
            }
            Some(Value::Array(sent))
        }
        Val::Collection(collection) => {
            Some(keyed(collection.items.iter().map(|(k, v)| (k, v)), where_)?)
        }
        Val::Object(members) => Some(keyed(members.iter(), where_)?),
        other => {
            return Err(format!(
                "input {where_} holds {}, not a file",
                other.kind_word()
            ));
        }
    })
}

/// A keyed input as an object by key.
fn keyed<'a>(
    items: impl Iterator<Item = (&'a String, &'a Val)>,
    where_: &str,
) -> Result<Value, String> {
    let mut sent = Map::new();
    for (key, item) in items {
        if let Some(item) = input_json(item, &format!("{where_}[{key}]"))? {
            sent.insert(key.clone(), item);
        }
    }
    Ok(Value::Object(sent))
}

fn unsendable(value: &Val, where_: &str) -> String {
    format!(
        "{where_} holds {}, which a paid call cannot send",
        value.kind_word()
    )
}

/// `{"file": digest}`.
fn file_json(file: &FileValue) -> Value {
    let mut object = Map::new();
    object.insert("file".into(), Value::from(file.digest.as_str()));
    Value::Object(object)
}

/// The content of a text or JSON file (spec/identity.md §5), read from its copy when the value
/// does not carry it (a project file named by a `./` path); `None` for any other kind.
fn file_content(file: &FileValue) -> Result<Option<FileContent>, String> {
    if let Some(content) = &file.content {
        return Ok(Some(content.clone()));
    }
    if !is_json(&file.kind) && !is_text(&file.kind) {
        return Ok(None);
    }
    let bytes = file.read_bytes()?;
    let text = decode_text(&bytes, &file.name).map_err(|e| e.to_string())?;
    if is_json(&file.kind) {
        let content =
            parse_json(&text).map_err(|e| format!("{} is not JSON: {}", file.name, e.message))?;
        Ok(Some(FileContent::Json(content)))
    } else {
        Ok(Some(FileContent::Text(text)))
    }
}

/// Every file inside a with-value, in order.
fn files_of<'a>(value: &'a Val, found: &mut Vec<&'a FileValue>) {
    match value {
        Val::File(file) => found.push(file),
        Val::List(items) => items.iter().for_each(|item| files_of(item, found)),
        Val::Object(members) => members.values().for_each(|item| files_of(item, found)),
        Val::Collection(collection) => collection
            .items
            .iter()
            .for_each(|(_, item)| files_of(item, found)),
        _ => {}
    }
}

fn failed_call(code: ErrorCode, message: impl Into<String>) -> CallError {
    CallError::Rpc(RpcError::new(code, message))
}

/// Makes the call and maps its answer (module doc).
pub async fn run(
    services: &Services,
    job: &InstanceJob,
    counter: &CallCounter,
    files: &RunFiles,
    cancel: Cancel,
) -> Result<Produced, CallError> {
    let JobBody::Capability { capability } = &job.body else {
        return Err(failed_call(
            ErrorCode::Internal,
            format!("{} is not a paid built-in", job.uses),
        ));
    };
    let store = &services.engine.store;

    // Hand the run every file its with-values hold, from the store.
    let mut held = Vec::new();
    for value in job.with.values() {
        files_of(value, &mut held);
    }
    for file in held {
        if store.has(&file.digest, file.size) {
            files.insert(file);
        } else {
            let adopted = store
                .adopt(file)
                .map_err(|e| failed_call(ErrorCode::Internal, e.to_string()))?;
            files.insert(&adopted);
        }
    }

    let request = request_of(job).map_err(|why| failed_call(ErrorCode::InvalidParams, why))?;
    // A paid built-in is one call of its own capability, whatever else the job lists.
    let mut site = job.call_site(cancel);
    site.calls = IndexMap::from([(capability.clone(), 1)]);
    let answer = crate::calls::call(services, &site, counter, files, capability, request).await?;
    let stored = |e: crate::store::StoreError| CallError::Fault(e.to_string());

    let mut outputs = IndexMap::new();
    for (port_name, port) in &job.spec.outputs {
        let file = answer.files.get(port_name).or_else(|| {
            answer
                .files
                .values()
                .find(|file| family(&file.kind) == port.family())
        });
        let Some(file) = file else { continue };
        let entry = FileEntry {
            digest: file.digest.clone(),
            kind: file.kind.clone(),
            name: format!("{}/{port_name}", job.path),
            size: file.size,
            key: None,
        };
        let value = store.file_value(&entry).map_err(stored)?;
        outputs.insert(port_name.clone(), Val::File(Box::new(value)));
    }

    let mut facts = IndexMap::new();
    if let Value::Object(data) = &answer.data {
        if let Some(Value::String(verdict)) = data.get("verdict") {
            facts.insert("verdict".to_string(), Value::from(verdict.as_str()));
        }
        if let Some(Value::Object(more)) = data.get("facts") {
            for (name, value) in more {
                if name != COST_FACT {
                    facts.insert(name.clone(), value.clone());
                }
            }
        }
        let json_port = "json";
        if job.spec.outputs.contains_key(json_port) && !outputs.contains_key(json_port) {
            let content = data.get(json_port).unwrap_or(&answer.data);
            let put = store
                .put_bytes(write_json(content).as_bytes())
                .map_err(stored)?;
            let entry = FileEntry {
                digest: put.digest,
                kind: "json".into(),
                name: format!("{}/{json_port}", job.path),
                size: put.size,
                key: None,
            };
            let value = store.file_value(&entry).map_err(stored)?;
            outputs.insert(json_port.to_string(), Val::File(Box::new(value)));
        }
    }
    Ok(Produced { outputs, facts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::val::Collection;
    use serde_json::json;

    fn file(n: u8, kind: &str) -> Val {
        Val::File(Box::new(FileValue {
            digest: format!("{n:064x}"),
            kind: kind.into(),
            name: format!("f{n}"),
            size: 1,
            key: None,
            content: None,
            location: None,
        }))
    }

    #[test]
    fn inputs_drop_what_does_not_exist() {
        assert_eq!(input_json(&Val::Missing, "a").unwrap(), None);
        let list = Val::List(vec![
            file(1, "image/png"),
            Val::Missing,
            file(2, "image/png"),
        ]);
        assert_eq!(
            input_json(&list, "refs").unwrap(),
            Some(json!([{"file": format!("{:064x}", 1)}, {"file": format!("{:064x}", 2)}]))
        );
        let keyed = Val::Collection(Box::new(Collection {
            items: vec![
                ("front".into(), file(3, "image/png")),
                ("back".into(), Val::Null),
            ],
            verdicts: IndexMap::new(),
        }));
        assert_eq!(
            input_json(&keyed, "views").unwrap(),
            Some(json!({"front": {"file": format!("{:064x}", 3)}}))
        );
        let refused = input_json(&Val::Str("x".into()), "image").unwrap_err();
        assert_eq!(refused, "input image holds text, not a file");
    }

    #[test]
    fn params_carry_content_files_and_nulls() {
        let mut text = FileValue {
            digest: format!("{:064x}", 4),
            kind: "text/plain".into(),
            name: "brief.txt".into(),
            size: 5,
            key: None,
            content: Some(FileContent::Text("Hello.\r\n".into())),
            location: None,
        };
        assert_eq!(
            param_json(&Val::File(Box::new(text.clone())), "text").unwrap(),
            json!("Hello.\r\n")
        );
        text.kind = "json".into();
        text.content = Some(FileContent::Json(json!({"b": 1, "a": [2]})));
        assert_eq!(
            param_json(&Val::File(Box::new(text)), "schema").unwrap(),
            json!({"b": 1, "a": [2]})
        );
        assert_eq!(
            param_json(&file(5, "image/png"), "p").unwrap(),
            json!({"file": format!("{:064x}", 5)})
        );
        assert_eq!(param_json(&Val::Missing, "p").unwrap(), Value::Null);
        assert_eq!(param_json(&Val::Number(2.0), "p").unwrap(), json!(2));
        let refused = param_json(&Val::Failed("draw#1".into()), "p").unwrap_err();
        assert_eq!(
            refused,
            "p holds a failed result, which a paid call cannot send"
        );
    }

    #[test]
    fn a_text_param_without_content_is_read_from_its_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brief.txt");
        std::fs::write(&path, b"\xEF\xBB\xBFHello.\r\n").unwrap();
        let value = FileValue {
            digest: format!("{:064x}", 6),
            kind: "text/plain".into(),
            name: "brief.txt".into(),
            size: 11,
            key: None,
            content: None,
            location: Some(path),
        };
        assert_eq!(
            param_json(&Val::File(Box::new(value)), "text").unwrap(),
            json!("Hello.\r\n")
        );
    }
}
