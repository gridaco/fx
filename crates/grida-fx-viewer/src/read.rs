//! Read-only projection of one explicitly selected run folder.
//!
//! The bootstrap reads placed files only. It never discovers projects, opens a cache, loads
//! credentials, imports workflow code, or repairs a torn event tail. A missing input or a placed
//! filename collision remains unavailable; it is never substituted with another file's bytes.

use grida_fx_core::value::{is_digest, parse_json};
use grida_fx_runtime::folder::{
    capped, keyed_path, named, step_file_path, step_folder, takes_of_id,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::{Component, Path, PathBuf};

/// Browser response for one run, independent of the original author's SDK.
#[derive(Debug, Serialize)]
pub struct RunDocument {
    pub kind: &'static str,
    pub workflow: Value,
    pub run_name: String,
    pub state: String,
    pub stand_in: bool,
    pub charged_usd: Option<f64>,
    pub estimate: Value,
    pub inputs: Value,
    pub outputs: Value,
    pub nodes: Vec<Node>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Value>,
    pub artifacts: Vec<Artifact>,
    pub warnings: Vec<String>,
}

/// An executed or planned instance. Values keep their recorded FX encoding.
#[derive(Debug, Serialize)]
pub struct Node {
    pub id: String,
    pub path: String,
    pub title: String,
    pub uses: Option<String>,
    pub state: String,
    pub reads: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface_bindings: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judges: Option<Value>,
    #[serde(rename = "with")]
    pub parameters: Value,
    pub outputs: Value,
    pub cache: Option<String>,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
}

/// A recorded file, including files whose bytes are not in the selected folder.
#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    pub digest: String,
    pub kind: String,
    pub name: String,
    pub size: u64,
    pub available: bool,
    pub url: Option<String>,
}

#[derive(Debug)]
struct Candidate {
    artifact: Artifact,
    paths: Vec<String>,
}

/// The browser document and its private, confined artifact inventory.
pub(crate) struct Snapshot {
    pub document: RunDocument,
    candidates: BTreeMap<String, Candidate>,
}

#[cfg(test)]
thread_local! {
    static VERIFIED_PATHS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn take_verified_paths() -> Vec<String> {
    VERIFIED_PATHS.with(|paths| std::mem::take(&mut *paths.borrow_mut()))
}

impl Snapshot {
    /// Opens exactly recorded bytes. Validation and serving use the same file handle.
    pub fn open_artifact(&self, root: &Path, digest: &str) -> io::Result<(File, Artifact)> {
        let candidate = self
            .candidates
            .get(digest)
            .ok_or_else(|| missing("artifact is not recorded in this run"))?;
        for relative in &candidate.paths {
            if let Ok(file) = verified_file(root, relative, &candidate.artifact) {
                return Ok((file, candidate.artifact.clone()));
            }
        }
        Err(missing("recorded artifact bytes are unavailable"))
    }
}

fn missing(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, message)
}

/// Reject both lexical escapes and symbolic links, including links into the selected root.
/// Hard links are normal FX placed artifacts and are permitted.
pub(crate) fn confined_file(root: &Path, relative: &str) -> io::Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(part) = component else {
            return Err(missing("file is outside the selected run"));
        };
        path.push(part);
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(missing("symbolic links are not served"));
        }
    }
    if !path.is_file() || !path.canonicalize()?.starts_with(root) {
        return Err(missing("file is outside the selected run"));
    }
    Ok(path)
}

fn verified_file(root: &Path, relative: &str, artifact: &Artifact) -> io::Result<File> {
    #[cfg(test)]
    VERIFIED_PATHS.with(|paths| paths.borrow_mut().push(relative.to_string()));
    let path = confined_file(root, relative)?;
    let mut file = File::open(path)?;
    if file.metadata()?.len() != artifact.size {
        return Err(missing("artifact size differs from its record"));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if format!("{:x}", hash.finalize()) != artifact.digest {
        return Err(missing("artifact digest differs from its record"));
    }
    file.rewind()?;
    Ok(file)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn object(value: Option<&Value>) -> Value {
    value
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn node(value: &Value, steps: &Map<String, Value>, types: &Map<String, Value>) -> Option<Node> {
    let id = text(value, "id")?.to_string();
    let path = text(value, "path").unwrap_or(&id).to_string();
    let title = steps
        .get(&path)
        .and_then(|step| text(step, "title"))
        .unwrap_or(&path)
        .to_string();
    Some(Node {
        id,
        path,
        title,
        uses: text(value, "uses").map(str::to_string),
        state: if text(value, "state") == Some("done") {
            "succeeded"
        } else {
            "pending"
        }
        .into(),
        reads: strings(value.get("reads")),
        ports: value
            .get("ports")
            .or_else(|| {
                types
                    .get(text(value, "uses")?)
                    .and_then(|entry| entry.get("ports"))
            })
            .filter(|ports| ports.is_object())
            .cloned(),
        bindings: value.get("bindings").filter(|v| v.is_array()).cloned(),
        interface_bindings: value.get("interface_bindings").cloned(),
        needs: value.get("needs").map(|v| strings(Some(v))),
        judges: value.get("judges").cloned(),
        parameters: object(value.get("with")),
        outputs: json!({}),
        cache: None,
        error: text(value, "reason").map(str::to_string),
        duration_ms: None,
    })
}

/// Reads the browser response, checking availability of every recorded placed artifact.
pub(crate) fn read_run(root: &Path) -> io::Result<Snapshot> {
    let mut snapshot = read_inventory(root)?;
    for candidate in snapshot.candidates.values_mut() {
        candidate.artifact.available = candidate
            .paths
            .iter()
            .any(|path| verified_file(root, path, &candidate.artifact).is_ok());
        candidate.artifact.url = candidate
            .artifact
            .available
            .then(|| format!("/api/artifacts/{}", candidate.artifact.digest));
        snapshot.document.artifacts.push(candidate.artifact.clone());
    }
    if snapshot
        .document
        .artifacts
        .iter()
        .any(|artifact| !artifact.available)
    {
        snapshot.document.warnings.push("Some recorded files are unavailable in this run folder. This viewer does not open a project cache.".into());
    }
    snapshot.document.warnings.sort();
    snapshot.document.warnings.dedup();
    Ok(snapshot)
}

/// Loads records and artifact references without opening artifact bytes. An artifact request
/// uses this inventory, then validates only its selected digest through `open_artifact`.
pub(crate) fn read_inventory(root: &Path) -> io::Result<Snapshot> {
    let path = confined_file(root, "plan.json")?;
    let plan = parse_json(&std::fs::read_to_string(path)?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "plan.json is not valid JSON"))?;
    if text(&plan, "kind") != Some("fx-graph-v1")
        || plan.get("workflow").and_then(|w| text(w, "id")).is_none()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "folder has no supported FX plan",
        ));
    }
    let mut warnings = Vec::new();
    let events = match confined_file(root, "events.jsonl") {
        Ok(path) => {
            let bytes = std::fs::read(&path)?;
            if bytes.last().is_some_and(|b| *b != b'\n') {
                warnings.push("An unfinished event tail is ignored until it is complete.".into());
            }
            let events = grida_fx_runtime::events::read_events_tolerant(&path)?;
            let whole_lines = bytes
                .split_inclusive(|b| *b == b'\n')
                .filter(|line| {
                    line.last() == Some(&b'\n') && !line.iter().all(u8::is_ascii_whitespace)
                })
                .count();
            if events.len() < whole_lines {
                warnings.push(
                    "The event log contains an unreadable line; only its readable prefix is shown."
                        .into(),
                );
            }
            events
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    let steps = plan
        .get("steps")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let types = plan
        .get("types")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut nodes: Vec<Node> = plan
        .get("instances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|instance| text(instance, "state") != Some("absent"))
        .filter_map(|instance| node(instance, &steps, &types))
        .collect();
    let mut scopes = plan.get("scopes").cloned();
    let mut node_interfaces = None;
    let mut state = "planned".to_string();
    let mut stand_in = plan.get("stand_in") == Some(&Value::Bool(true));
    let mut charged = None;
    let mut outputs = json!({});
    let mut candidates = BTreeMap::new();
    for current in &nodes {
        collect_object(&current.parameters, None, &mut candidates);
    }
    for event in events {
        if text(&event, "kind") != Some("fx-run-events-v1") {
            warnings.push("An unsupported event kind was ignored.".into());
            continue;
        }
        if let (Some(expected), Some(recorded)) = (text(&plan, "plan"), text(&event, "plan"))
            && expected != recorded
        {
            warnings.push("An event for another plan was ignored.".into());
            continue;
        }
        match text(&event, "event") {
            Some("scopes_updated") => {
                scopes = Some(event.get("scopes").cloned().unwrap_or(Value::Null));
                node_interfaces = Some(
                    event
                        .get("node_interface_bindings")
                        .cloned()
                        .unwrap_or(Value::Null),
                );
            }
            Some("run_started") => {
                state = "unfinished".into();
                charged = None;
                outputs = json!({});
                stand_in |= event.get("stand_in") == Some(&Value::Bool(true));
            }
            Some("run_finished") => {
                state = if event.get("ok") == Some(&Value::Bool(true)) {
                    "succeeded"
                } else {
                    "failed"
                }
                .into();
                charged = event.get("charged_usd").and_then(Value::as_f64);
                outputs = object(event.get("outputs"));
                collect_object(&outputs, Some(Placement::Outputs), &mut candidates);
            }
            Some("run_cancelled") => {
                state = "cancelled".into();
                charged = event.get("charged_usd").and_then(Value::as_f64);
                outputs = json!({});
            }
            Some(name @ ("node_started" | "node_finished" | "node_failed" | "node_skipped")) => {
                let Some(id) = text(&event, "id") else {
                    continue;
                };
                let index = if let Some(index) = nodes.iter().position(|entry| entry.id == id) {
                    index
                } else if let Some(entry) = node(&event, &steps, &types) {
                    nodes.push(entry);
                    nodes.len() - 1
                } else {
                    continue;
                };
                let current = &mut nodes[index];
                // Runtime evidence is authoritative even when a dynamically-created node
                // failed or was blocked before dispatch. Absence preserves older records.
                if let Some(uses) = text(&event, "uses") {
                    current.uses = Some(uses.to_string());
                }
                if let Some(ports) = event.get("ports").filter(|v| v.is_object()) {
                    current.ports = Some(ports.clone());
                }
                if let Some(bindings) = event.get("bindings").filter(|v| v.is_array()) {
                    current.bindings = Some(bindings.clone());
                }
                if let Some(needs) = event.get("needs") {
                    current.needs = Some(strings(Some(needs)));
                }
                if let Some(judges) = event.get("judges") {
                    current.judges = Some(judges.clone());
                }
                if let Some(reads) = event.get("reads") {
                    current.reads = strings(Some(reads));
                }
                if let Some(parameters) = event.get("with") {
                    current.parameters = object(Some(parameters));
                    collect_object(&current.parameters, None, &mut candidates);
                }
                match name {
                    "node_started" => {
                        current.state = "running".into();
                        current.error = None;
                        current.cache = None;
                        current.outputs = json!({});
                        current.duration_ms = None;
                        if let Some(path) = text(&event, "path") {
                            current.path = path.into();
                        }
                        current.uses = text(&event, "uses")
                            .map(str::to_string)
                            .or(current.uses.take());
                        current.reads = strings(event.get("reads"));
                        current.parameters = object(event.get("with"));
                    }
                    "node_finished" => {
                        current.state = "succeeded".into();
                        current.outputs = object(event.get("outputs"));
                        current.cache = text(&event, "cache").map(str::to_string);
                        current.error = None;
                        let placed_step = step_folder(&current.path, &takes_of_id(&current.id));
                        collect_object(
                            &current.outputs,
                            Some(Placement::Step(&placed_step)),
                            &mut candidates,
                        );
                    }
                    _ => {
                        current.state = if name == "node_failed" {
                            "failed"
                        } else {
                            "skipped"
                        }
                        .into();
                        current.outputs = json!({});
                        current.cache = None;
                        current.error = text(&event, "error")
                            .filter(|s| !s.is_empty())
                            .or_else(|| text(&event, "reason"))
                            .map(str::to_string);
                    }
                }
                if name != "node_started" {
                    current.duration_ms = event.get("duration_ms").and_then(Value::as_u64);
                }
            }
            _ => {}
        }
    }
    if matches!(state.as_str(), "succeeded" | "failed" | "cancelled") {
        for current in &mut nodes {
            if current.state == "running" {
                current.state = "failed".into();
                current.error = Some("The run ended before this step finished.".into());
            } else if current.state == "pending" {
                current.state = "skipped".into();
            }
        }
    }
    if let Some(bindings) = &node_interfaces {
        for node in &mut nodes {
            // A complete snapshot supersedes earlier plan metadata, including empty arrays.
            node.interface_bindings = bindings.get(&node.id).cloned();
        }
    }
    scopes = super::scopes::project_run(
        scopes,
        node_interfaces.as_ref(),
        &plan,
        &mut nodes,
        &mut warnings,
    );
    warnings.sort();
    warnings.dedup();
    let workflow = plan.get("workflow").cloned().unwrap_or_else(|| json!({}));
    let document = RunDocument {
        kind: "fx-viewer-run-v1",
        workflow,
        run_name: root
            .file_name()
            .map_or_else(|| "Run".into(), |name| name.to_string_lossy().into_owned()),
        state,
        stand_in,
        charged_usd: charged,
        estimate: plan.get("estimate").cloned().unwrap_or(Value::Null),
        inputs: object(plan.get("inputs")),
        outputs,
        nodes,
        scopes,
        artifacts: Vec::new(),
        warnings,
    };
    Ok(Snapshot {
        document,
        candidates,
    })
}

enum Placement<'a> {
    Step(&'a str),
    Outputs,
}

fn collect_object(
    value: &Value,
    placement: Option<Placement<'_>>,
    inventory: &mut BTreeMap<String, Candidate>,
) {
    let Some(values) = value.as_object() else {
        return;
    };
    for (port, value) in values {
        let mut found = Vec::new();
        collect_files(value, None, &mut found);
        let one = found.len() == 1;
        for (index, (artifact, key)) in found.into_iter().enumerate() {
            let label = key.map_or_else(|| index.to_string(), |key| keyed_path(&key));
            let path = match placement {
                Some(Placement::Step(step)) => Some(step_file_path(
                    step,
                    port,
                    (!one).then_some(label.as_str()),
                    &artifact.kind,
                )),
                Some(Placement::Outputs) => {
                    let name = if value.get("file").is_some() {
                        port.clone()
                    } else {
                        format!("{port}/{label}")
                    };
                    Some(capped(
                        &format!("outputs/{}", named(&name, &artifact.kind)),
                        &artifact.kind,
                    ))
                }
                None => None,
            };
            let entry = inventory
                .entry(artifact.digest.clone())
                .or_insert_with(|| Candidate {
                    artifact,
                    paths: Vec::new(),
                });
            if let Some(path) = path
                && !entry.paths.contains(&path)
            {
                entry.paths.push(path);
            }
        }
    }
}

fn collect_files(
    value: &Value,
    item_key: Option<&str>,
    files: &mut Vec<(Artifact, Option<String>)>,
) {
    if let Some(file) = value.get("file").filter(|file| file.is_object()) {
        let (Some(digest), Some(kind), Some(size)) = (
            text(file, "digest"),
            text(file, "kind"),
            file.get("size").and_then(Value::as_u64),
        ) else {
            return;
        };
        if !is_digest(digest) {
            return;
        }
        files.push((
            Artifact {
                digest: digest.into(),
                kind: kind.into(),
                name: text(file, "name").unwrap_or(digest).into(),
                size,
                available: false,
                url: None,
            },
            text(file, "key")
                .filter(|key| !key.is_empty())
                .or(item_key)
                .map(str::to_string),
        ));
    } else if let Some(items) = value.get("list").and_then(Value::as_array) {
        for item in items {
            collect_files(item, None, files);
        }
    } else if let Some(items) = value.get("collection").and_then(Value::as_array) {
        for item in items {
            if let Some([key, value]) = item.as_array().map(Vec::as_slice) {
                collect_files(value, key.as_str(), files);
            }
        }
    }
}
