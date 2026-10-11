//! Offline, disclosure-aware exports of one terminal saved run (spec/exports.md).
//! Neither source records nor artifacts are repaired, and no project or cache is opened.

use crate::{layout, read};
use grida_fx_core::value::{canon, sha256_hex, write_json};
use grida_fx_runtime::{folder::same_file, observation};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::Path;

mod references;

/// Deliberate categories of recorded author content to disclose.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExportOptions {
    pub include_values: bool,
    pub include_diagnostics: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OmittedArtifact {
    pub digest: String,
    pub reason: &'static str,
}

/// Counts exclude the manifest. `output` is local command metadata, not portable data.
#[derive(Debug, Serialize)]
pub struct ExportResult {
    pub kind: &'static str,
    pub manifest_sha256: String,
    pub output: String,
    pub files: u64,
    pub bytes: u64,
    pub omitted_artifacts: Vec<OmittedArtifact>,
}

/// Stable refusal codes; details never expose private absolute source paths.
#[derive(Debug, Clone, Serialize)]
pub struct ExportError {
    pub kind: &'static str,
    pub code: &'static str,
    pub message: &'static str,
}

impl ExportError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            kind: "fx-run-export-error-v1",
            code,
            message,
        }
    }
}
impl std::fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}
impl std::error::Error for ExportError {}

fn unavailable() -> ExportError {
    ExportError::new(
        "unavailable",
        "The run or its existing ownership lock is unavailable.",
    )
}
fn invalid() -> ExportError {
    ExportError::new(
        "invalid_record",
        "The saved run evidence is invalid or unsupported.",
    )
}
fn write_failed() -> ExportError {
    ExportError::new("write_failed", "The complete export could not be written.")
}
fn destination_exists() -> ExportError {
    ExportError::new(
        "destination_exists",
        "The export destination already exists.",
    )
}

/// Export one exact saved folder to a new directory; holds the run's existing lock throughout.
/// The caller's output label is returned verbatim, rather than revealing a canonical local path.
pub fn run_export(
    root: &Path,
    destination: &Path,
    output_label: &str,
    options: ExportOptions,
) -> Result<ExportResult, ExportError> {
    let root = root.canonicalize().map_err(|_| unavailable())?;
    if !root.is_dir() {
        return Err(unavailable());
    }
    let lock_path = read::confined_file(&root, "run.lock").map_err(|_| unavailable())?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|_| unavailable())?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(ExportError::new("busy", "The run is active."));
        }
        Err(TryLockError::Error(_)) => return Err(unavailable()),
    }
    if !same_file(&lock, &lock_path) {
        return Err(unavailable());
    }
    let plan_path = read::confined_file(&root, "plan.json").map_err(|_| invalid())?;
    let plan_bytes = std::fs::read(&plan_path).map_err(|_| invalid())?;
    let events_path = read::confined_file(&root, "events.jsonl").map_err(|_| invalid())?;
    let events_bytes = std::fs::read(&events_path).map_err(|_| invalid())?;
    if events_bytes.last() != Some(&b'\n') {
        return Err(ExportError::new(
            "incomplete_run",
            "The run has an incomplete event tail.",
        ));
    }
    let captured = observation::snapshot(&root).map_err(|_| invalid())?;
    let latest = captured
        .events
        .iter()
        .rposition(|event| event["event"] == "run_started");
    let terminal = captured.events.last().filter(|event| {
        matches!(
            event["event"].as_str(),
            Some("run_finished" | "run_cancelled")
        )
    });
    if latest.is_none()
        || terminal.is_none()
        || captured.events.len() <= latest.unwrap_or_default() + 1
    {
        return Err(ExportError::new(
            "incomplete_run",
            "The latest run invocation has no terminal record.",
        ));
    }
    if terminal.is_none_or(|event| {
        event["invocation_id"] != captured.events[latest.unwrap_or_default()]["invocation_id"]
    }) {
        return Err(invalid());
    }
    let mut snapshot = read::project_export_records(&root, &captured).map_err(|_| invalid())?;
    let mut layout = serde_json::to_value(layout::run_report(
        &captured.plan,
        &snapshot,
        captured.cursor.clone(),
    ))
    .map_err(|_| invalid())?;
    for key in ["cursor", "file", "revision"] {
        layout[key] = Value::Null;
    }
    if !options.include_diagnostics {
        layout["diagnostics"] = json!([]);
    }
    let inventory = snapshot.export_inventory();
    if std::fs::symlink_metadata(destination).is_ok() {
        return Err(destination_exists());
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = parent.canonicalize().map_err(|_| {
        ExportError::new(
            "destination_invalid",
            "The export destination parent must exist.",
        )
    })?;
    if parent.starts_with(&root) || destination.file_name().is_none() {
        return Err(ExportError::new(
            "destination_invalid",
            "The export destination must be outside the source run.",
        ));
    }
    let target = parent.join(destination.file_name().ok_or_else(write_failed)?);
    let temporary = tempfile::Builder::new()
        .prefix(".fx-export-")
        .tempdir_in(&parent)
        .map_err(|_| write_failed())?;
    let artifacts_dir = temporary.path().join("artifacts");
    std::fs::create_dir(&artifacts_dir).map_err(|_| write_failed())?;
    let mut artifacts = Vec::new();
    let mut omitted_artifacts = Vec::new();
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    for (mut artifact, placed) in inventory {
        artifact.name = basename(&artifact.name, &artifact.digest);
        let extension = media_extension(&artifact.kind);
        let omitted = if extension.is_none() {
            Some("policy_excluded")
        } else if !placed {
            Some("unplaced_input")
        } else {
            None
        };
        let relative = extension
            .filter(|_| omitted.is_none())
            .map(|extension| format!("artifacts/{}.{}", artifact.digest, extension));
        if let Some(relative) = &relative {
            let (mut source, _) = snapshot.open_artifact(&root, &artifact.digest).map_err(
                |_| {
                    ExportError::new(
                        "artifact_unavailable",
                        "Selected artifact bytes are missing, unsafe or differ from their record.",
                    )
                },
            )?;
            let mut copied =
                File::create(temporary.path().join(relative)).map_err(|_| write_failed())?;
            let mut hash = Sha256::new();
            let mut count = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let n = source.read(&mut buffer).map_err(|_| {
                    ExportError::new(
                        "artifact_unavailable",
                        "Selected artifact bytes could not be read.",
                    )
                })?;
                if n == 0 {
                    break;
                }
                copied.write_all(&buffer[..n]).map_err(|_| write_failed())?;
                hash.update(&buffer[..n]);
                count += n as u64;
            }
            if count != artifact.size || format!("{:x}", hash.finalize()) != artifact.digest {
                return Err(ExportError::new(
                    "artifact_unavailable",
                    "Selected artifact bytes changed while copying.",
                ));
            }
            copied.sync_all().map_err(|_| write_failed())?;
            files += 1;
            bytes += count;
        }
        if let Some(reason) = omitted {
            omitted_artifacts.push(OmittedArtifact {
                digest: artifact.digest.clone(),
                reason,
            });
        }
        artifact.available = relative.is_some();
        artifact.url = None;
        artifacts.push(json!({"digest":artifact.digest,"media_type":artifact.kind,"name":artifact.name,"size":artifact.size,"path":relative,"omitted_reason":omitted}));
        snapshot.document.artifacts.push(artifact);
    }
    // Cooperative writers are excluded by the held lock. Detect replacement or manual
    // modification too, rather than committing a mixture of two source records.
    if !same_file(&lock, &lock_path)
        || std::fs::read(read::confined_file(&root, "plan.json").map_err(|_| invalid())?)
            .map_err(|_| invalid())?
            != plan_bytes
        || std::fs::read(read::confined_file(&root, "events.jsonl").map_err(|_| invalid())?)
            .map_err(|_| invalid())?
            != events_bytes
    {
        return Err(invalid());
    }
    let view = project_view(
        &serde_json::to_value(snapshot.document).map_err(|_| invalid())?,
        options,
    );
    let manifest = json!({
        "kind":"fx-run-export-v1", "producer":{"version":env!("CARGO_PKG_VERSION")},
        "source":{"plan_sha256":sha256_hex(&plan_bytes),"events_sha256":sha256_hex(canon(&json!(captured.events)).as_bytes()),"event_count":captured.events.len()},
        "disclosure":{"values":options.include_values,"diagnostics":options.include_diagnostics,"artifacts":"media"},
        "view":view,"layout":layout,"artifacts":artifacts
    });
    let schema: Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/fx-run-export-v1.schema.json"
    ))
    .map_err(|_| invalid())?;
    jsonschema::validator_for(&schema)
        .map_err(|_| invalid())?
        .validate(&manifest)
        .map_err(|_| invalid())?;
    references::validate(&manifest)?;
    let encoded = write_json(&manifest);
    let manifest_path = temporary.path().join("manifest.json");
    let mut file = File::create(&manifest_path).map_err(|_| write_failed())?;
    file.write_all(encoded.as_bytes())
        .map_err(|_| write_failed())?;
    file.sync_all().map_err(|_| write_failed())?;
    place_directory(temporary.path(), &target).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            destination_exists()
        } else {
            write_failed()
        }
    })?;
    Ok(ExportResult {
        kind: "fx-run-export-result-v1",
        manifest_sha256: sha256_hex(encoded.as_bytes()),
        output: output_label.into(),
        files,
        bytes,
        omitted_artifacts,
    })
}

fn place_directory(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    {
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            source,
            rustix::fs::CWD,
            target,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)
    }
    #[cfg(windows)]
    {
        std::fs::rename(source, target)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        windows
    )))]
    {
        let _ = (source, target);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "exclusive directory rename is unavailable",
        ))
    }
}

fn media_extension(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "audio/wav" => "wav",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "model/gltf-binary" => "glb",
        _ => return None,
    })
}
fn basename(name: &str, fallback: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .filter(|value| {
            !value.is_empty()
                && *value != "."
                && *value != ".."
                && !value.chars().any(char::is_control)
        })
        .unwrap_or(fallback)
        .into()
}
fn fields(value: &Value, names: &[&str]) -> Value {
    Value::Object(
        names
            .iter()
            .filter_map(|name| value.get(name).map(|value| ((*name).into(), value.clone())))
            .collect(),
    )
}
fn list_project(value: Option<&Value>, names: &[&str]) -> Value {
    json!(
        value
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|value| fields(value, names))
            .collect::<Vec<_>>()
    )
}
fn project_view(view: &Value, options: ExportOptions) -> Value {
    let mut projected = fields(
        view,
        &[
            "kind",
            "run_name",
            "state",
            "stand_in",
            "charged_usd",
            "artifacts",
        ],
    );
    projected["workflow"] = fields(
        &view["workflow"],
        if options.include_values {
            &["id", "title", "description"]
        } else {
            &["id", "title"]
        },
    );
    projected["estimate"] = if view["estimate"].is_object() {
        fields(&view["estimate"], &["low_usd", "high_usd", "ceiling_usd"])
    } else {
        Value::Null
    };
    for name in ["inputs", "outputs"] {
        projected[name] = value_map(&view[name], options.include_values);
    }
    projected["warnings"] = if options.include_diagnostics {
        view["warnings"].clone()
    } else {
        json!([])
    };
    projected["nodes"] = json!(
        view["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|node| {
                let mut item = fields(
                    node,
                    &[
                        "id",
                        "path",
                        "step",
                        "take",
                        "key",
                        "title",
                        "uses",
                        "state",
                        "reads",
                        "needs",
                        "judges",
                        "cache",
                        "duration_ms",
                        "started_at",
                        "interrupted",
                    ],
                );
                for name in ["with", "outputs"] {
                    item[name] = value_map(&node[name], options.include_values);
                }
                item["error"] = if options.include_diagnostics {
                    node["error"].clone()
                } else {
                    Value::Null
                };
                if node.get("ports").is_some() {
                    let mut ports = fields(&node["ports"], &["inputs", "outputs"]);
                    ports["params"] = schema_map(&node["ports"]["params"], options.include_values);
                    item["ports"] = ports;
                }
                for name in ["bindings", "interface_bindings"] {
                    if let Some(value) = node.get(name) {
                        item[name] = list_project(
                            Some(value),
                            &["source", "source_port", "target_port", "source_kind"],
                        );
                    }
                }
                item
            })
            .collect::<Vec<_>>()
    );
    if let Some(scopes) = view.get("scopes") {
        projected["scopes"] = json!(scopes.as_array().into_iter().flatten().map(|scope| {
            let mut item = fields(scope, &["id","parent","kind","path","step","take","title","nodes","pending"]); item["source"] = Value::Null;
            item["ports"] = json!({"inputs":schema_map(&scope["ports"]["inputs"],options.include_values),"outputs":scope["ports"]["outputs"]});
            for name in ["input_bindings","output_bindings"] { item[name] = list_project(scope.get(name), &["source","source_port","target_port","source_kind"]); } item
        }).collect::<Vec<_>>());
    }
    if view.get("pending").is_some() {
        projected["pending"] = list_project(
            view.get("pending"),
            &["path", "step", "max", "phase", "high_usd", "waiting_on"],
        );
    }
    projected
}
fn value_map(value: &Value, include: bool) -> Value {
    Value::Object(
        value
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.clone(), export_value(value, include)))
            .collect(),
    )
}
fn export_value(value: &Value, include: bool) -> Value {
    if let Some(file) = value.get("file").filter(|file| file.is_object()) {
        let mut file = fields(file, &["digest", "kind", "name", "size", "key"]);
        file["name"] = json!(basename(
            file["name"].as_str().unwrap_or_default(),
            file["digest"].as_str().unwrap_or("artifact")
        ));
        return json!({"file":file});
    }
    if let Some(list) = value.get("list").and_then(Value::as_array) {
        return json!({"list":list.iter().map(|value| export_value(value,include)).collect::<Vec<_>>()});
    }
    if let Some(collection) = value.get("collection").and_then(Value::as_array) {
        return json!({"collection":collection.iter().filter_map(|entry| entry.as_array().filter(|entry|entry.len()==2)).map(|entry| json!([entry[0],export_value(&entry[1],include)])).collect::<Vec<_>>()});
    }
    if include {
        value.clone()
    } else {
        json!({"withheld":true})
    }
}
fn schema_map(value: &Value, include: bool) -> Value {
    Value::Object(
        value
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.clone(), schema(value, include)))
            .collect(),
    )
}
fn schema(value: &Value, include: bool) -> Value {
    if include {
        return value.clone();
    }
    let mut item = fields(
        value,
        &[
            "type",
            "required",
            "minimum",
            "maximum",
            "exclusiveMinimum",
            "exclusiveMaximum",
            "multipleOf",
            "minLength",
            "maxLength",
            "minItems",
            "maxItems",
            "uniqueItems",
            "minProperties",
            "maxProperties",
        ],
    );
    if let Some(format) = value
        .get("format")
        .and_then(Value::as_str)
        .filter(|format| {
            matches!(
                *format,
                "date-time"
                    | "date"
                    | "time"
                    | "duration"
                    | "email"
                    | "hostname"
                    | "ipv4"
                    | "ipv6"
                    | "uuid"
                    | "uri"
                    | "uri-reference"
            )
        })
    {
        item["format"] = json!(format);
    }
    if let Some(properties) = value.get("properties") {
        item["properties"] = schema_map(properties, false);
    }
    for name in ["items", "additionalProperties", "not"] {
        if let Some(nested) = value.get(name) {
            item[name] = if nested.is_boolean() {
                nested.clone()
            } else {
                schema(nested, false)
            };
        }
    }
    for name in ["allOf", "anyOf", "oneOf"] {
        if let Some(nested) = value.get(name).and_then(Value::as_array) {
            item[name] = json!(
                nested
                    .iter()
                    .map(|value| schema(value, false))
                    .collect::<Vec<_>>()
            );
        }
    }
    item
}

#[cfg(test)]
mod tests;
