//! Portable file discovery and private native resolution (spec/files.md).
//!
//! No workflow source is loaded or executed. Artifact lookup shares the verified HTTP
//! inventory; source lookup uses only recorded labels and an explicit native source root.

use crate::{read, service};
use grida_fx_core::value::{digest, is_digest, parse_json};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const MAX_PLAN_BYTES: u64 = 16 * 1024 * 1024;

/// A safe refusal for the read-only native file API.
#[derive(Debug, Serialize)]
pub struct FileError {
    pub kind: &'static str,
    pub code: &'static str,
    pub message: &'static str,
}

impl FileError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            kind: "fx-file-error-v1",
            code,
            message,
        }
    }
}

fn invalid_source() -> FileError {
    FileError::new(
        "invalid_source",
        "The selected recorded source cannot be read.",
    )
}

fn not_found() -> FileError {
    FileError::new(
        "not_found",
        "The file target is not recorded in this source.",
    )
}

fn invalid_request() -> FileError {
    FileError::new("invalid_request", "The file reference is malformed.")
}

fn stale_subject() -> FileError {
    FileError::new(
        "stale_subject",
        "The file reference belongs to a different source.",
    )
}

enum RecordedSource {
    Run(PathBuf, Box<read::Snapshot>),
    Plan(Value),
}

impl RecordedSource {
    fn open(path: &Path) -> Result<Self, FileError> {
        let path = path.canonicalize().map_err(|_| invalid_source())?;
        if path.is_dir() {
            let snapshot = read::read_inventory(&path).map_err(|_| invalid_source())?;
            Ok(Self::Run(path, Box::new(snapshot)))
        } else {
            let file = fs::File::open(path).map_err(|_| invalid_source())?;
            if file.metadata().map_err(|_| invalid_source())?.len() > MAX_PLAN_BYTES {
                return Err(invalid_source());
            }
            let mut bytes = String::new();
            file.take(MAX_PLAN_BYTES + 1)
                .read_to_string(&mut bytes)
                .map_err(|_| invalid_source())?;
            if bytes.len() as u64 > MAX_PLAN_BYTES {
                return Err(invalid_source());
            }
            let graph = parse_json(&bytes).map_err(|_| invalid_source())?;
            crate::plan_source(graph.clone()).map_err(|_| invalid_source())?;
            Ok(Self::Plan(graph))
        }
    }

    fn graph(&self) -> &Value {
        match self {
            Self::Run(_, snapshot) => &snapshot.plan,
            Self::Plan(graph) => graph,
        }
    }

    fn list(&self, instance: Option<&str>, prefix: Option<&str>) -> Result<Value, FileError> {
        match self {
            Self::Run(_, snapshot) => inventory(&snapshot.plan, Some(snapshot), instance, prefix),
            Self::Plan(graph) => list_plan(graph, instance),
        }
    }
}

/// Enumerates known file targets in an existing run folder or saved graph JSON file.
pub fn list(path: &Path, instance: Option<&str>) -> Result<Value, FileError> {
    RecordedSource::open(path)?.list(instance, None)
}

/// The shared, portable inventory for a hosted run. `prefix` is its scoped HTTP route prefix.
pub(crate) fn list_run(
    root: &Path,
    instance: Option<&str>,
    prefix: Option<&str>,
) -> Result<Value, FileError> {
    let snapshot = read::read_inventory(root).map_err(|_| invalid_source())?;
    inventory(&snapshot.plan, Some(&snapshot), instance, prefix)
}

/// The shared, portable inventory for a materialized plan.
pub(crate) fn list_plan(graph: &Value, instance: Option<&str>) -> Result<Value, FileError> {
    inventory(graph, None, instance, None)
}

fn inventory(
    graph: &Value,
    snapshot: Option<&read::Snapshot>,
    instance: Option<&str>,
    prefix: Option<&str>,
) -> Result<Value, FileError> {
    let instances: Vec<Value> = match snapshot {
        Some(snapshot) => snapshot
            .document
            .nodes
            .iter()
            .map(|node| {
                json!({"id":node.id,"uses":node.uses,"with":node.parameters,"outputs":node.outputs})
            })
            .collect(),
        None => graph["instances"].as_array().cloned().unwrap_or_default(),
    };
    if instance.is_some_and(|id| !instances.iter().any(|node| node["id"] == id)) {
        return Err(FileError::new(
            "not_found",
            "The recorded instance does not exist in this source.",
        ));
    }
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |target: Value, role: &str, owner: Option<&str>, scope: Option<&str>| {
        let mut descriptor = json!({"target":target,"role":role});
        if let Some(id) = owner {
            descriptor["instance_id"] = json!(id);
        }
        if let Some(id) = scope {
            descriptor["scope_id"] = json!(id);
        }
        if seen.insert(descriptor.to_string()) {
            files.push(descriptor);
        }
    };
    if let Some(target) = source_target(graph["workflow"]["file"].as_str(), None, None) {
        push(target, "workflow", None, None);
    }
    let scopes = snapshot
        .and_then(|s| s.document.scopes.as_ref())
        .or_else(|| graph.get("scopes"))
        .and_then(Value::as_array);
    if let Some(scopes) = scopes {
        for scope in scopes {
            if scope["kind"] != "workflow" {
                continue;
            }
            if let Some(id) = instance
                && !scope["nodes"]
                    .as_array()
                    .is_some_and(|nodes| nodes.iter().any(|node| node == id))
            {
                continue;
            }
            if let Some(target) = source_target(scope["source"].as_str(), None, None) {
                push(target, "workflow", None, scope["id"].as_str());
            }
        }
    }
    let artifacts: BTreeMap<String, Value> = snapshot.map_or_else(BTreeMap::new, |snapshot| {
        snapshot
            .export_inventory()
            .into_iter()
            .filter_map(|(artifact, placed)| {
                let url = prefix
                    .filter(|_| placed)
                    .map(|prefix| format!("{prefix}/api/artifacts/{}", artifact.digest));
                let target = json!({"kind":"artifact","artifact":{
                    "digest":artifact.digest,"kind":artifact.kind,
                    "name":portable_name(&artifact.name, &artifact.digest),
                    "size":artifact.size,"url":url,
                }});
                valid_target(&target).then_some((artifact.digest, target))
            })
            .collect()
    });
    let mut attributed_artifacts = BTreeSet::new();
    for node in &instances {
        let Some(id) = node["id"].as_str() else {
            continue;
        };
        if instance.is_some_and(|wanted| wanted != id) {
            continue;
        }
        let mut digests = BTreeSet::new();
        file_digests(&node["with"], &mut digests);
        file_digests(&node["outputs"], &mut digests);
        for digest in digests {
            if let Some(target) = artifacts.get(&digest) {
                push(target.clone(), "artifact", Some(id), None);
                attributed_artifacts.insert(digest);
            }
        }
        let Some(uses) = node["uses"].as_str() else {
            continue;
        };
        let ty = &graph["types"][uses];
        let module = ty["identity"]
            .as_str()
            .or(Some(uses))
            .and_then(module_selector);
        let source_files = ty["source"]["files"].as_object();
        if let Some((path, symbol)) = module {
            let digest = source_files
                .and_then(|files| files.get(path))
                .and_then(Value::as_str);
            if let Some(target) = source_target(Some(path), Some(symbol), digest) {
                push(target, "node", Some(id), None);
            }
        }
        for (member, role) in [("files", "source"), ("resources", "resource")] {
            if let Some(entries) = ty["source"][member].as_object() {
                for (path, value) in entries {
                    if let Some(target) = source_target(Some(path), None, value.as_str()) {
                        push(target, role, Some(id), None);
                    }
                }
            }
        }
    }
    if instance.is_none() {
        for (digest, target) in artifacts {
            if !attributed_artifacts.contains(&digest) {
                push(target, "artifact", None, None);
            }
        }
    }
    Ok(json!({"kind":"fx-files-v1","files":files}))
}

fn file_digests(value: &Value, digests: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(file) = map.get("file").filter(|value| value.is_object()) {
                if let Some(digest) = file["digest"].as_str().filter(|digest| is_digest(digest)) {
                    digests.insert(digest.to_owned());
                }
            } else if !(map.len() == 1 && map.contains_key("value")) {
                for value in map.values() {
                    file_digests(value, digests);
                }
            }
        }
        Value::Array(items) => {
            for value in items {
                file_digests(value, digests);
            }
        }
        _ => {}
    }
}

fn module_selector(value: &str) -> Option<(&str, &str)> {
    let (path, attribute) = value.split_once('#')?;
    let path = path.strip_prefix("./").unwrap_or(path);
    let attribute = attribute.split('@').next()?;
    (portable_path(path) && identifier(attribute)).then_some((path, attribute))
}

fn source_target(
    source: Option<&str>,
    symbol: Option<&str>,
    recorded_digest: Option<&str>,
) -> Option<Value> {
    let source = source?;
    let (source, symbol) = if symbol.is_none()
        && let Some((path, function)) = source.rsplit_once(':')
        && identifier(function)
        && matches!(
            Path::new(path).extension().and_then(|value| value.to_str()),
            Some("py" | "ts" | "mts" | "js" | "mjs")
        ) {
        (path, Some(function))
    } else {
        (source, symbol)
    };
    if !portable_path(source) || symbol.is_some_and(|symbol| !identifier(symbol)) {
        return None;
    }
    let mut target = json!({"kind":"source","source":source});
    if let Some(symbol) = symbol {
        target["symbol"] = json!(symbol);
    }
    if let Some(digest) = recorded_digest.filter(|digest| is_digest(digest)) {
        target["recorded_digest"] = json!(digest);
    }
    Some(target)
}

fn portable_path(value: &str) -> bool {
    !value.is_empty()
        && !value.chars().any(char::is_control)
        && !value.contains(['\\', ':'])
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
        && !value.starts_with('/')
}

fn identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn portable_name<'a>(name: &'a str, fallback: &'a str) -> &'a str {
    if name.chars().any(char::is_control)
        || name.starts_with(['/', '\\'])
        || name
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
        || (name.as_bytes().get(1) == Some(&b':') && name.as_bytes()[0].is_ascii_alphabetic())
    {
        fallback
    } else {
        name
    }
}

fn file_url(value: &Value) -> bool {
    if value.is_null() {
        return true;
    }
    let Some(value) = value.as_str() else {
        return false;
    };
    if value.contains(['\\', '?', '#']) || value.chars().any(char::is_control) {
        return false;
    }
    if value.starts_with('/') {
        return !value.starts_with("//");
    }
    let Ok(url) = value.parse::<axum::http::Uri>() else {
        return false;
    };
    matches!(url.scheme_str(), Some("http" | "https"))
        && url
            .authority()
            .is_some_and(|authority| !authority.as_str().contains('@'))
}

fn valid_target(target: &Value) -> bool {
    let Some(object) = target.as_object() else {
        return false;
    };
    match target["kind"].as_str() {
        Some("source") => {
            object.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "kind" | "source" | "symbol" | "recorded_digest"
                )
            }) && target["source"].as_str().is_some_and(portable_path)
                && target
                    .get("symbol")
                    .is_none_or(|value| value.as_str().is_some_and(identifier))
                && target
                    .get("recorded_digest")
                    .is_none_or(|value| value.as_str().is_some_and(is_digest))
        }
        Some("artifact") => {
            let Some(artifact) = target["artifact"].as_object() else {
                return false;
            };
            object.len() == 2
                && artifact.len() == 5
                && target["artifact"]["digest"].as_str().is_some_and(is_digest)
                && target["artifact"]["kind"]
                    .as_str()
                    .is_some_and(|kind| !kind.is_empty() && !kind.chars().any(char::is_control))
                && target["artifact"]["name"]
                    .as_str()
                    .is_some_and(|name| portable_name(name, "") == name)
                && target["artifact"]["size"]
                    .as_u64()
                    .is_some_and(|size| size <= 9_007_199_254_740_991)
                && file_url(&target["artifact"]["url"])
        }
        _ => false,
    }
}

/// Resolves one exact portable reference against an explicitly bound native source.
pub fn resolve(
    path: &Path,
    reference: &Value,
    project: Option<&Path>,
    source_root: Option<&Path>,
) -> Result<Value, FileError> {
    if reference.as_object().is_none_or(|object| {
        object.len() != 2 || !object.contains_key("subject") || !object.contains_key("target")
    }) || !valid_target(&reference["target"])
    {
        return Err(invalid_request());
    }
    let source = RecordedSource::open(path)?;
    validate_subject(&source, &reference["subject"], project)?;
    let requested = &reference["target"];
    let inventory = source.list(None, None)?;
    let target = inventory["files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|descriptor| &descriptor["target"])
        .find(|target| same_target(target, requested))
        .cloned()
        .ok_or_else(not_found)?;
    let (local_path, source_match, reason) = match target["kind"].as_str() {
        Some("artifact") => {
            let digest = target["artifact"]["digest"]
                .as_str()
                .ok_or_else(invalid_request)?;
            match &source {
                RecordedSource::Run(root, snapshot) => {
                    match snapshot.open_artifact_with_path(root, digest) {
                        Ok((_, _, path)) => (Some(path), None, None),
                        Err(_) => (None, None, Some("Recorded artifact bytes are unavailable.")),
                    }
                }
                RecordedSource::Plan(_) => {
                    (None, None, Some("A plan has no placed artifact bytes."))
                }
            }
        }
        Some("source") => {
            let direct = inventory["files"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|descriptor| {
                    matches!(
                        descriptor["role"].as_str(),
                        Some("workflow" | "node" | "resource")
                    ) && descriptor["target"]["source"] == target["source"]
                });
            resolve_source(&target, project, source_root, direct)
        }
        _ => return Err(invalid_request()),
    };
    Ok(json!({
        "kind":"fx-file-resolution-v1", "target":target,
        "local_path":local_path, "source_match":source_match, "reason":reason,
    }))
}

fn same_target(recorded: &Value, requested: &Value) -> bool {
    if recorded["kind"] != requested["kind"] {
        return false;
    }
    if recorded["kind"] == "artifact" {
        return ["digest", "kind", "size"]
            .iter()
            .all(|key| recorded["artifact"][key] == requested["artifact"][key]);
    }
    recorded["source"] == requested["source"]
        && recorded.get("symbol") == requested.get("symbol")
        && requested
            .get("recorded_digest")
            .is_none_or(|digest| recorded.get("recorded_digest") == Some(digest))
}

fn resolve_source(
    target: &Value,
    project: Option<&Path>,
    source_root: Option<&Path>,
    direct: bool,
) -> (Option<PathBuf>, Option<&'static str>, Option<&'static str>) {
    let unavailable = |reason| (None, Some("unknown"), Some(reason));
    // A closure-only label can be relative to a source package instead of the workflow
    // home. Existing graphs do not record that base. Current project configuration can
    // change or disappear, so it cannot establish a historical label's namespace.
    if !direct && source_root.is_none() {
        return unavailable(
            "The closure source needs an explicit source-root binding for its complete label.",
        );
    }
    let Some(root) = source_root
        .or(project)
        .and_then(|root| root.canonicalize().ok())
    else {
        return unavailable("An explicit local source-root binding is required.");
    };
    let Some(source) = target["source"].as_str() else {
        return unavailable("The recorded source is unavailable.");
    };
    let path = match read::confined_file(&root, source) {
        Ok(path) => path,
        Err(_) => return unavailable("The current source file is unavailable in the bound root."),
    };
    let Some(expected) = target["recorded_digest"].as_str() else {
        return (Some(path), Some("unknown"), None);
    };
    let Ok(current_digest) = source_digest(&path) else {
        return unavailable("The current source file cannot be read.");
    };
    let state = if current_digest == expected {
        "match"
    } else {
        "changed"
    };
    (Some(path), Some(state), None)
}

fn source_digest(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn validate_subject(
    source: &RecordedSource,
    subject: &Value,
    project: Option<&Path>,
) -> Result<(), FileError> {
    let Some(subject_fields) = subject.as_object() else {
        return Err(invalid_request());
    };
    if subject_fields
        .keys()
        .any(|key| !matches!(key.as_str(), "kind" | "workflow" | "reference"))
    {
        return Err(invalid_request());
    }
    let expected_kind = match source {
        RecordedSource::Run(..) => "run",
        RecordedSource::Plan(_) => "plan",
    };
    if subject["kind"] != expected_kind {
        return Err(stale_subject());
    }
    if let Some(workflow) = subject.get("workflow") {
        if workflow.as_object().is_none_or(|fields| {
            !fields.contains_key("id")
                || fields
                    .keys()
                    .any(|key| !matches!(key.as_str(), "id" | "source"))
        }) {
            return Err(invalid_request());
        }
        let recorded = &source.graph()["workflow"];
        if workflow["id"] != recorded["id"]
            || workflow
                .get("source")
                .is_some_and(|value| recorded.get("file") != Some(value))
        {
            return Err(stale_subject());
        }
    }
    let Some(reference) = subject.get("reference") else {
        return Ok(());
    };
    if reference["kind"] != "service_entry" {
        return Err(stale_subject());
    }
    let (Some(project_id), Some(entry_id), Some(entry_kind)) = (
        reference["project_id"].as_str().filter(|id| is_digest(id)),
        reference["entry_id"].as_str().filter(|id| is_digest(id)),
        reference["entry_kind"].as_str(),
    ) else {
        return Err(invalid_request());
    };
    if reference.as_object().is_none_or(|fields| fields.len() != 4) || entry_kind != expected_kind {
        return Err(invalid_request());
    }
    let root = project
        .and_then(|path| path.canonicalize().ok())
        .ok_or_else(stale_subject)?;
    if digest(&json!(["fx-local-project-v1", root])) != project_id {
        return Err(stale_subject());
    }
    let matches = match source {
        RecordedSource::Run(run, _) => service::matches_run_entry(&root, entry_id, run),
        RecordedSource::Plan(graph) => service::matches_plan_entry(&root, entry_id, graph),
    };
    if !matches.unwrap_or(false) {
        return Err(stale_subject());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
