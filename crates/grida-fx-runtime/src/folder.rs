//! Run folders (spec/store.md §8).
//!
//! ```text
//! <run folder>/
//!   plan.json         fx-graph-v1: the plan the run started from, written once
//!   events.jsonl      fx-run-events-v1 (crate::events)
//!   run.lock          an exclusive OS lock held by the invocation running the folder
//!   files/<step>/<port><suffix>      each succeeded instance's output files
//!   outputs/<name><suffix>           the workflow's declared outputs
//! ```
//!
//! - Where: `--run <folder>` relative to the working directory, else
//!   `<runs>/<workflow id>/<YYYY-MM-DD>-<n>` with the local date when the command starts and the
//!   smallest free `n` from 1 ([`new_folder`]).
//! - [`check_plan`]: a folder whose `plan.json` records another plan digest is refused before
//!   anything runs: `<folder> holds a run of another workflow or other inputs; choose a new
//!   folder` (`<folder>` as the user typed it, or relative to the working directory).
//! - [`RunFolder::lock`]: `run.lock` locked without waiting (`File::try_lock`); held until the
//!   value drops. Refused at once with `another invocation is running <folder>`.
//! - [`plan_document`]: the graph document `grida-fx expand` prints, with `plan`, `steps` (every
//!   declared step by its path: `{title, description, uses, view}`), `inputs` (plain) and
//!   `view_origins` added, plus `takes_file` (the project-relative path of the workflow's takes
//!   file, which `reroll` and `pick` write; spec/store.md §8 records it). Written with
//!   `value::write_json` under a temporary name and renamed (`store::atomic_write`), once.
//! - Placing ([`RunFolder::place`]): a hard link to the store's copy, or a copy where linking
//!   fails, under a temporary name in the destination folder, renamed over the final name; a name
//!   that already holds the same bytes is left alone (compared by size, then by digest).
//! - Names ([`safe_name`], [`keyed_path`], [`named`]): store.md §8 "Names"; labels and the "one
//!   file or several" rule in [`step_files`] and [`output_files`].
//!
//! Messages name the folder by its label and files by their place in it, never by an absolute
//! path.

use grida_fx_core::docs::workflow::Steps;
use grida_fx_core::kinds::suffix_of_kind;
use grida_fx_core::plan::Plan;
use grida_fx_core::project::Planner;
use grida_fx_core::val::{FileValue, Val};
use grida_fx_core::value::{is_digest, parse_json, write_json};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A run folder the invocation holds the lock of.
#[derive(Debug)]
pub struct RunFolder {
    pub path: PathBuf,
    /// How messages name it: as typed, or relative to the working directory.
    pub label: String,
    /// Held, never read: the OS lock lasts while the file is open, and ends with the process.
    #[allow(dead_code)]
    lock: File,
}

/// Why a folder cannot be used: printed as `refused: <message>` (exit 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRefused(pub String);

impl RunFolder {
    /// Makes the folder (and its parents) when missing and takes its lock.
    pub fn lock(path: &Path, label: &str) -> Result<RunFolder, FolderRefused> {
        std::fs::create_dir_all(path)
            .map_err(|e| FolderRefused(format!("cannot make {label}: {}", reason(&e))))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.join("run.lock"))
            .map_err(|e| FolderRefused(format!("cannot open {label}/run.lock: {}", reason(&e))))?;
        match lock.try_lock() {
            Ok(()) => Ok(RunFolder {
                path: path.to_path_buf(),
                label: label.to_string(),
                lock,
            }),
            Err(TryLockError::WouldBlock) => Err(FolderRefused(format!(
                "another invocation is running {label}"
            ))),
            Err(TryLockError::Error(e)) => Err(FolderRefused(format!(
                "cannot lock {label}/run.lock: {}",
                reason(&e)
            ))),
        }
    }

    pub fn plan_path(&self) -> PathBuf {
        self.path.join("plan.json")
    }

    pub fn events_path(&self) -> PathBuf {
        self.path.join("events.jsonl")
    }

    /// Writes `plan.json` when the folder has none (module doc).
    pub fn write_plan_once(&self, document: &Value) -> std::io::Result<()> {
        let path = self.plan_path();
        if std::fs::symlink_metadata(&path).is_ok() {
            return Ok(());
        }
        crate::store::atomic_write(&path, write_json(document).as_bytes(), false)
    }

    /// Places the store file `source` at `relative` (POSIX, under the folder).
    pub fn place(&self, relative: &str, source: &Path) -> std::io::Result<()> {
        let target = self.inside(relative)?;
        let source_meta = std::fs::metadata(source)?;
        if let Ok(target_meta) = std::fs::metadata(&target)
            && target_meta.is_file()
            && same_bytes(source, &source_meta, &target, &target_meta)?
        {
            return Ok(());
        }
        let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
            return Err(invalid(relative));
        };
        std::fs::create_dir_all(parent)?;
        let temporary = parent.join(temporary_name(&name.to_string_lossy()));
        let _ = std::fs::remove_file(&temporary);
        let put = std::fs::hard_link(source, &temporary)
            .or_else(|_| copy_read_only(source, &temporary))
            .and_then(|()| std::fs::rename(&temporary, &target));
        if put.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        put
    }

    /// `relative` as a path under the folder; refused when it is empty, absolute or could leave
    /// the folder.
    fn inside(&self, relative: &str) -> std::io::Result<PathBuf> {
        let mut path = self.path.clone();
        for segment in relative.split('/') {
            if matches!(segment, "" | "." | "..") || segment.contains('\\') {
                return Err(invalid(relative));
            }
            path.push(segment);
        }
        Ok(path)
    }
}

fn invalid(relative: &str) -> std::io::Error {
    std::io::Error::new(
        ErrorKind::InvalidInput,
        format!("{relative} is not a place inside a run folder"),
    )
}

/// A temporary name in the destination folder: hidden, never a digest, unique in the process so
/// two tasks placing at once do not share one.
fn temporary_name(name: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(".{name}.{}-{n}.part", std::process::id())
}

/// A copy where a link cannot be made (another file system): read-only, like the store's bytes.
fn copy_read_only(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::copy(source, target)?;
    let mut permissions = std::fs::metadata(target)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(target, permissions)
}

/// Whether `target` already holds `source`'s bytes: the same file (a link), or the same size and
/// the same digest. The store names its files by their digest, so the source is hashed only when
/// its name is not one.
fn same_bytes(
    source: &Path,
    source_meta: &std::fs::Metadata,
    target: &Path,
    target_meta: &std::fs::Metadata,
) -> std::io::Result<bool> {
    if source_meta.len() != target_meta.len() {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if source_meta.dev() == target_meta.dev() && source_meta.ino() == target_meta.ino() {
            return Ok(true);
        }
    }
    let named = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| is_digest(name));
    let source_digest = match named {
        Some(digest) => digest,
        None => digest_of(source)?,
    };
    Ok(digest_of(target)? == source_digest)
}

/// The file digest of a file, read as a stream.
fn digest_of(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// `<runs>/<workflow id>/<YYYY-MM-DD>-<n>` (module doc); touches nothing.
pub fn new_folder(runs: &Path, workflow_id: &str) -> PathBuf {
    let base = runs.join(workflow_id);
    let date = local_date();
    let mut n: u64 = 1;
    loop {
        let candidate = base.join(format!("{date}-{n}"));
        // Anything of that name counts, a dangling link included.
        if std::fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
        n += 1;
    }
}

/// Today's local date, `YYYY-MM-DD`.
pub fn local_date() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Refuses a folder whose `plan.json` records another plan digest (module doc). A folder with no
/// `plan.json` passes; an unreadable one is refused with a sentence.
pub fn check_plan(path: &Path, label: &str, digest: &str) -> Result<(), FolderRefused> {
    let document = read_plan_file(path).map_err(|why| {
        FolderRefused(format!(
            "{label} holds a plan.json that cannot be read ({why}); choose a new folder"
        ))
    })?;
    match document {
        None => Ok(()),
        Some(document) if document.get("plan").and_then(Value::as_str) == Some(digest) => Ok(()),
        Some(_) => Err(FolderRefused(format!(
            "{label} holds a run of another workflow or other inputs; choose a new folder"
        ))),
    }
}

/// The `plan` member of a folder's `plan.json`, when it has one.
pub fn recorded_plan(path: &Path) -> Result<Option<Value>, String> {
    let document = read_plan_file(path).map_err(|why| format!("plan.json: {why}"))?;
    Ok(document.and_then(|document| document.get("plan").cloned()))
}

/// A folder's `plan.json` as a JSON object; `None` when there is none. The error is a reason
/// that names no path.
fn read_plan_file(folder: &Path) -> Result<Option<Map<String, Value>>, String> {
    let bytes = match std::fs::read(folder.join("plan.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(reason(&error)),
    };
    let text = String::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_string())?;
    match parse_json(&text) {
        Ok(Value::Object(document)) => Ok(Some(document)),
        Ok(_) => Err("it is not a JSON object".to_string()),
        Err(refused) => Err(refused.message),
    }
}

/// The `plan.json` document (module doc). `takes_file` is project-relative; empty leaves it out.
pub fn plan_document(plan: &Plan, planner: &Planner, digest: &str, takes_file: &str) -> Value {
    let mut document = grida_fx_core::plan::output::graph_document(plan, planner);
    let Value::Object(map) = &mut document else {
        return document;
    };
    map.insert("plan".into(), Value::from(digest));
    let mut steps = Map::new();
    step_documents(&planner.workflow.workflow.steps, "", &mut steps);
    map.insert("steps".into(), Value::Object(steps));
    let inputs: Map<String, Value> = planner
        .inputs
        .given
        .iter()
        .map(|(name, value)| (name.clone(), value.plain().unwrap_or_else(|| value.shown())))
        .collect();
    map.insert("inputs".into(), Value::Object(inputs));
    map.insert(
        "view_origins".into(),
        Value::Array(
            planner
                .project
                .document
                .view_origins
                .iter()
                .map(|origin| Value::from(origin.as_str()))
                .collect(),
        ),
    );
    if !takes_file.is_empty() {
        map.insert("takes_file".into(), Value::from(takes_file));
    }
    document
}

/// Each declared step by its path (a group's steps as `<group>.<step>`): `{title, description,
/// uses, view}`. A used workflow's own steps are not listed.
fn step_documents(steps: &Steps, prefix: &str, found: &mut Map<String, Value>) {
    for (name, step) in steps.iter() {
        let path = format!("{prefix}{name}");
        let text = |text: &Option<String>| text.as_deref().map_or(Value::Null, Value::from);
        let mut document = Map::new();
        document.insert("title".into(), text(&step.title));
        document.insert("description".into(), text(&step.description));
        document.insert("uses".into(), text(&step.uses));
        document.insert("view".into(), step.view.to_value());
        found.insert(path.clone(), Value::Object(document));
        if let Some(inner) = &step.steps {
            step_documents(inner, &format!("{path}."), found);
        }
    }
}

/// `<step>`: every character that is not a letter or digit (Unicode general category L or N),
/// `.`, `_` or `-` becomes `_`; leading and trailing `_` removed; empty becomes `step`.
pub fn safe_name(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| {
            if is_letter_or_number(c) || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = replaced.trim_matches('_');
    if trimmed.is_empty() {
        "step".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Whether `c` is in Unicode general category L (letters) or N (numbers). Not
/// `char::is_alphanumeric`, which also takes the marks and symbols that are Alphabetic, such as
/// the vowel sign U+093F or the circled letter U+24B6.
fn is_letter_or_number(c: char) -> bool {
    use unicode_general_category::{GeneralCategory as G, get_general_category};
    matches!(
        get_general_category(c),
        G::UppercaseLetter
            | G::LowercaseLetter
            | G::TitlecaseLetter
            | G::ModifierLetter
            | G::OtherLetter
            | G::DecimalNumber
            | G::LetterNumber
            | G::OtherNumber
    )
}

/// A key as a path: split at `/`, `""`/`.`/`..` segments become `_`, others [`safe_name`].
pub fn keyed_path(key: &str) -> String {
    key.split('/')
        .map(|segment| match segment {
            "" | "." | ".." => "_".to_string(),
            other => safe_name(other),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// `label` plus the first suffix spec/identity.md §4 lists for `kind` (`annotations` takes
/// `.json`), unless it already ends with it.
pub fn named(label: &str, kind: &str) -> String {
    let suffix = suffix_of_kind(kind);
    if suffix.is_empty() || label.ends_with(suffix) {
        label.to_string()
    } else {
        format!("{label}{suffix}")
    }
}

/// The files of a succeeded instance's outputs and where they go: `(relative path, digest)`.
pub fn step_files(path: &str, outputs: &indexmap::IndexMap<String, Val>) -> Vec<(String, String)> {
    let step = safe_name(path);
    let mut placed = Vec::new();
    for (port, value) in outputs {
        let files = files_of(value);
        if let [(file, _)] = files.as_slice() {
            placed.push((
                format!("files/{step}/{}", named(port, &file.kind)),
                file.digest.clone(),
            ));
            continue;
        }
        for (index, (file, key)) in files.iter().enumerate() {
            let label = label_of(*key, index);
            placed.push((
                format!(
                    "files/{step}/{}",
                    named(&format!("{port}/{label}"), &file.kind)
                ),
                file.digest.clone(),
            ));
        }
    }
    placed
}

/// The files of one declared workflow output and where they go: `(relative path, digest)`.
pub fn output_files(name: &str, value: &Val) -> Vec<(String, String)> {
    if let Val::File(file) = value {
        return vec![(
            format!("outputs/{}", named(name, &file.kind)),
            file.digest.clone(),
        )];
    }
    files_of(value)
        .into_iter()
        .enumerate()
        .map(|(index, (file, key))| {
            let label = label_of(key, index);
            (
                format!("outputs/{}", named(&format!("{name}/{label}"), &file.kind)),
                file.digest.clone(),
            )
        })
        .collect()
}

/// A file's label: its key as a path when it has a non-empty one, else its position from 0.
fn label_of(key: Option<&str>, index: usize) -> String {
    match key {
        Some(key) => keyed_path(key),
        None => index.to_string(),
    }
}

/// Every file a value holds, in order (store.md §8 "One file or several": a list's items, a keyed
/// collection's items in collection order, an object's members in order), each with its key: the
/// file's own, else the key of the collection item it is.
fn files_of(value: &Val) -> Vec<(&FileValue, Option<&str>)> {
    fn collect<'a>(
        value: &'a Val,
        item_key: Option<&'a str>,
        out: &mut Vec<(&'a FileValue, Option<&'a str>)>,
    ) {
        match value {
            Val::File(file) => {
                let key = file
                    .key
                    .as_deref()
                    .filter(|key| !key.is_empty())
                    .or(item_key.filter(|key| !key.is_empty()));
                out.push((file, key));
            }
            Val::List(items) => items.iter().for_each(|item| collect(item, None, out)),
            Val::Collection(collection) => collection
                .items
                .iter()
                .for_each(|(key, item)| collect(item, Some(key), out)),
            Val::Object(members) => members.values().for_each(|item| collect(item, None, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    collect(value, None, &mut out);
    out
}

/// The reason of an I/O error, without the path it was about.
fn reason(error: &std::io::Error) -> String {
    match error.kind() {
        ErrorKind::NotFound => "no such file or folder".to_string(),
        ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_names_are_hidden_and_never_a_digest() {
        let a = temporary_name("hero.png");
        let b = temporary_name("hero.png");
        assert!(a.starts_with(".hero.png.") && a.ends_with(".part"));
        assert_ne!(a, b);
        assert!(!is_digest(&a));
    }

    #[test]
    fn digests_of_files_are_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x");
        std::fs::write(&path, b"one\ntwo\n").unwrap();
        assert_eq!(
            digest_of(&path).unwrap(),
            "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8"
        );
    }
}
