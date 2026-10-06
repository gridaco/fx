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
//!   smallest free `n` from 1 ([`new_folder`]), claimed by making it: two invocations starting at
//!   once never get the same new folder.
//! - [`check_plan`]: a folder whose `plan.json` records another plan digest is refused before
//!   anything runs: `<folder> holds a run of another workflow or other inputs; choose a new
//!   folder` (`<folder>` as the user typed it, or relative to the working directory). The runner
//!   checks it once it holds the lock, so no other invocation can write `plan.json` in between.
//! - [`check_mode`]: right after it, a folder of the other mode is refused (spec/store.md §8,
//!   "Stand-in runs"): `<folder> holds a stand-in run; resume it with --stand-in, or choose a new
//!   folder`, or `<folder> holds a run without a stand-in; resume it without --stand-in, or
//!   choose a new folder`. A folder holds a stand-in run when its `plan.json` says `"stand_in":
//!   true` ([`holds_stand_in`]); a folder without `plan.json` holds nothing yet.
//! - [`RunFolder::lock`]: `run.lock` locked without waiting (`File::try_lock`); held until the
//!   value drops. Refused at once with `another invocation is running <folder>`.
//! - [`RunFolder::sweep`]: what a killed invocation left half placed (temporary names) is removed
//!   by the next one that holds the lock.
//! - [`plan_document`]: the graph document `grida-fx expand` prints, with `plan`, `steps` (every
//!   declared step by its path: `{title, description, uses, view}`), `inputs` (plain) and
//!   `view_origins` added, plus `takes_file` (the project-relative path of the workflow's takes
//!   file, which `reroll` and `pick` write; spec/store.md §8 records it), and `"stand_in": true`
//!   for a stand-in run only (not part of the plan digest). Written with
//!   `value::write_json` under a temporary name and renamed (`store::atomic_write`), once.
//! - Placing ([`RunFolder::place`]): a hard link to the store's copy, or a copy where linking
//!   fails, under a temporary name in the destination folder (`.<16 hex>.part`, short enough to
//!   fit wherever the final name fits), renamed over the final name; a name that already holds
//!   the same bytes is left alone (compared by size, then by digest).
//! - Names ([`safe_name`], [`keyed_path`], [`named`], [`step_folder`], [`capped`]): store.md §8
//!   "Names"; labels and the "one file or several" rule in [`step_files`] and [`output_files`].
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
        let (Some(parent), Some(_)) = (target.parent(), target.file_name()) else {
            return Err(invalid(relative));
        };
        std::fs::create_dir_all(parent)?;
        let temporary = parent.join(temporary_name());
        let _ = std::fs::remove_file(&temporary);
        let put = std::fs::hard_link(source, &temporary)
            .or_else(|_| copy_read_only(source, &temporary))
            .and_then(|()| std::fs::rename(&temporary, &target));
        if put.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        put
    }

    /// Removes what an earlier invocation left half placed: every file under the folder whose
    /// name is a temporary one ([`is_temporary_name`]). Only the invocation holding the lock
    /// places anything here, so none of them is being written. Best effort.
    pub fn sweep(&self) {
        let mut folders = vec![self.path.clone()];
        while let Some(folder) = folders.pop() {
            let Ok(entries) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    folders.push(entry.path());
                } else if is_temporary_name(&entry.file_name().to_string_lossy()) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
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

/// The most bytes one name may hold on the file systems FX writes to (`NAME_MAX`).
pub const NAME_BYTES: usize = 255;

/// A temporary name in the destination folder: `.<16 hex>.part`, hidden, never a digest, unique
/// in the process so two tasks placing at once do not share one, and short (22 bytes), so it fits
/// wherever the final name does.
fn temporary_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let seed = format!("{}:{n}:{nanos}", std::process::id());
    let digest = grida_fx_core::value::sha256_hex(seed.as_bytes());
    format!(".{}.part", &digest[..16])
}

/// Whether a name is a temporary one: `.<16 lowercase hex>.part`, or the longer
/// `.<name>.<pid>-<n>.part` earlier engines used.
pub fn is_temporary_name(name: &str) -> bool {
    let Some(inner) = name
        .strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(".part"))
    else {
        return false;
    };
    let hex = |text: &str| {
        text.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if inner.len() == 16 && hex(inner) {
        return true;
    }
    // `<name>.<pid>-<n>`.
    inner.rsplit_once('.').is_some_and(|(stem, tail)| {
        !stem.is_empty()
            && tail.split_once('-').is_some_and(|(pid, n)| {
                !pid.is_empty()
                    && !n.is_empty()
                    && pid.bytes().all(|b| b.is_ascii_digit())
                    && n.bytes().all(|b| b.is_ascii_digit())
            })
    })
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

/// Makes and returns a new folder `<runs>/<workflow id>/<YYYY-MM-DD>-<n>` (module doc): the
/// smallest `n` from 1 whose name holds nothing, claimed by making the folder (never one that
/// exists), so an invocation starting at the same moment takes the next `n`.
pub fn new_folder(runs: &Path, workflow_id: &str) -> std::io::Result<PathBuf> {
    let base = runs.join(workflow_id);
    std::fs::create_dir_all(&base)?;
    let date = local_date();
    let mut n: u64 = 1;
    loop {
        let candidate = base.join(format!("{date}-{n}"));
        // Anything of that name counts, a dangling link included.
        if std::fs::symlink_metadata(&candidate).is_err() {
            match std::fs::create_dir(&candidate) {
                Ok(()) => return Ok(candidate),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
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

/// Refuses a folder that holds a run of the other mode (module doc). `stand_in`: whether this
/// invocation answers its paid calls with a stand-in.
pub fn check_mode(path: &Path, label: &str, stand_in: bool) -> Result<(), FolderRefused> {
    let document = read_plan_file(path).map_err(|why| {
        FolderRefused(format!(
            "{label} holds a plan.json that cannot be read ({why}); choose a new folder"
        ))
    })?;
    let Some(document) = document else {
        return Ok(());
    };
    match (is_stand_in(&document), stand_in) {
        (true, false) => Err(FolderRefused(format!(
            "{label} holds a stand-in run; resume it with --stand-in, or choose a new folder"
        ))),
        (false, true) => Err(FolderRefused(format!(
            "{label} holds a run without a stand-in; resume it without --stand-in, or choose a \
             new folder"
        ))),
        _ => Ok(()),
    }
}

/// Whether a `plan.json` document records a stand-in run: `"stand_in": true`.
pub fn holds_stand_in(plan: &Value) -> bool {
    plan.as_object().is_some_and(is_stand_in)
}

fn is_stand_in(document: &Map<String, Value>) -> bool {
    document.get("stand_in") == Some(&Value::Bool(true))
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
/// `stand_in` adds `"stand_in": true`.
pub fn plan_document(
    plan: &Plan,
    planner: &Planner,
    digest: &str,
    takes_file: &str,
    stand_in: bool,
) -> Value {
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
    if stand_in {
        map.insert("stand_in".into(), Value::Bool(true));
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

/// A step's folder name under `files/` (spec/store.md §8 "Names"): [`safe_name`] of the instance
/// path, and, when its takes are not `[1]`, `#` and the takes joined by `.` (`draw#2`,
/// `entity__ada__.draw#1.3`), so two takes of one step never share a folder. Not capped yet
/// ([`capped`] does that with the rest of the path).
pub fn step_folder(path: &str, takes: &[u32]) -> String {
    let step = safe_name(path);
    if takes == [1] {
        return step;
    }
    let takes: Vec<String> = takes.iter().map(u32::to_string).collect();
    format!("{step}#{}", takes.join("."))
}

/// The takes of an instance id (`<path>#<t1>.<t2>…`, spec/identity.md §11); `[1]` when the id
/// carries none that read as numbers.
pub fn takes_of_id(id: &str) -> Vec<u32> {
    id.rsplit_once('#')
        .and_then(|(_, takes)| {
            takes
                .split('.')
                .map(|take| take.parse::<u32>().ok())
                .collect::<Option<Vec<u32>>>()
        })
        .filter(|takes| !takes.is_empty())
        .unwrap_or_else(|| vec![1])
}

/// Where one output file of a step goes (module doc): `files/<step folder>/<port><suffix>` for a
/// port's only file, else `files/<step folder>/<port>/<label><suffix>`; capped ([`capped`]).
pub fn step_file_path(step_folder: &str, port: &str, label: Option<&str>, kind: &str) -> String {
    let label = match label {
        None => port.to_string(),
        Some(label) => format!("{port}/{label}"),
    };
    capped(
        &format!("files/{step_folder}/{}", named(&label, kind)),
        kind,
    )
}

/// A relative path with every segment cut to fit one name (spec/store.md §8 "Names"): a segment
/// longer than [`NAME_BYTES`] keeps its first bytes (whole characters), then `~` and the first 16
/// hex characters of the SHA-256 of the whole segment, then, on the last segment, the kind's
/// suffix it ended with: `~` is never part of a converted name, so a cut name cannot be another
/// one's whole name.
pub fn capped(relative: &str, kind: &str) -> String {
    let suffix = suffix_of_kind(kind);
    let segments: Vec<&str> = relative.split('/').collect();
    let last = segments.len().saturating_sub(1);
    segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let kept = if index == last && !suffix.is_empty() {
                suffix
            } else {
                ""
            };
            cap_segment(segment, kept)
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// One segment cut to [`NAME_BYTES`] (see [`capped`]), keeping `suffix` when it ends with it.
fn cap_segment(segment: &str, suffix: &str) -> String {
    if segment.len() <= NAME_BYTES {
        return segment.to_string();
    }
    let suffix = if segment.ends_with(suffix) {
        suffix
    } else {
        ""
    };
    let stem = &segment[..segment.len() - suffix.len()];
    let digest = grida_fx_core::value::sha256_hex(segment.as_bytes());
    let room = NAME_BYTES - 1 - 16 - suffix.len();
    let mut cut = room.min(stem.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}~{}{suffix}", &stem[..cut], &digest[..16])
}

/// The files of a succeeded instance's outputs and where they go: `(relative path, digest)`.
/// `takes` are the instance's ([`step_folder`]).
pub fn step_files(
    path: &str,
    takes: &[u32],
    outputs: &indexmap::IndexMap<String, Val>,
) -> Vec<(String, String)> {
    let step = step_folder(path, takes);
    let mut placed = Vec::new();
    for (port, value) in outputs {
        let files = files_of(value);
        if let [(file, _)] = files.as_slice() {
            placed.push((
                step_file_path(&step, port, None, &file.kind),
                file.digest.clone(),
            ));
            continue;
        }
        for (index, (file, key)) in files.iter().enumerate() {
            let label = label_of(*key, index);
            placed.push((
                step_file_path(&step, port, Some(&label), &file.kind),
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
            capped(&format!("outputs/{}", named(name, &file.kind)), &file.kind),
            file.digest.clone(),
        )];
    }
    files_of(value)
        .into_iter()
        .enumerate()
        .map(|(index, (file, key))| {
            let label = label_of(key, index);
            (
                capped(
                    &format!("outputs/{}", named(&format!("{name}/{label}"), &file.kind)),
                    &file.kind,
                ),
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
    fn temporary_names_are_hidden_short_and_never_a_digest() {
        let a = temporary_name();
        let b = temporary_name();
        assert!(a.starts_with('.') && a.ends_with(".part"), "{a}");
        assert_eq!(a.len(), 22);
        assert_ne!(a, b);
        assert!(!is_digest(&a));
        assert!(is_temporary_name(&a));
    }

    #[test]
    fn temporary_names_are_told_from_others() {
        assert!(is_temporary_name(".0123456789abcdef.part"));
        assert!(is_temporary_name(".blob.11366-4.part"));
        assert!(is_temporary_name(".hero.png.2-0.part"));
        for name in [
            "hero.png",
            ".0123456789ABCDEF.part",
            ".0123456789abcde.part",
            ".blob.part",
            ".blob.x-1.part",
            ".blob.1-.part",
            "0123456789abcdef.part",
            ".blob.1-2.parts",
        ] {
            assert!(!is_temporary_name(name), "{name}");
        }
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
