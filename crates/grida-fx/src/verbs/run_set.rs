//! The runs a planning project holds (spec/store.md §9, "Which runs a project holds"), for
//! `runs list` and `runs remove`. Nothing here takes a lock, repairs a log or creates a folder.
//!
//! - **The runs tree**, walked whole from the configured runs folder (a symbolic link there is
//!   followed): every directory below it, at any depth and with no cap, never through a symbolic
//!   link. A directory holding a regular `plan.json` is a run folder whatever its name; the walk
//!   goes on below it (a run may sit inside a `--run runs/<id>` folder) except into its `files`,
//!   `outputs` and `views`.
//! - **Placement**, by position below the tree: `allocated` (`<id>/<YYYY-MM-DD>-<n>`), `named`
//!   (`<id>/named-<sha256(source)>/<NAME>-<sha256(NAME)>`), each only when the recorded workflow
//!   (and name and source) agree with the position, else `explicit`; `external` for the folders
//!   outside the tree that the run index or the catalog know. Inside the tree the walk is
//!   authoritative: an index or catalog entry of a folder there that is gone is not a run.
//! - **Leftovers**: a directory at an allocated or named position that holds nothing but
//!   `run.lock` and temporary names is an *empty claim* (a claim, or a start that was refused or
//!   killed before it wrote its plan); one that holds more but no `plan.json` is a problem; a
//!   `.removing-<16 hex>` directory is a *tombstone* a removal left.
//! - **Problems** stop nothing here but are reported: a directory that cannot be read, a symbolic
//!   link or special file among the tree's directories, a `plan.json` of another kind, a run whose
//!   files cannot be read, an index or catalog entry that cannot be read, an external folder that
//!   cannot be reached. `runs list` reports them as skipped.
//!
//! State (`fx-run-list-v1`): `planned`, `unfinished`, `succeeded`, `cancelled` as `inspect`
//! reads them; inspect's `failed` split into `incomplete` (`ok: false, incomplete: true`, which a
//! later invocation may finish) and `failed`; `empty` and `removing` for the leftovers; `missing`
//! for an external folder that is gone.

use super::takes::{is_inside, picked_result};
use super::{event_name, text};
use grida_fx_core::docs::project::Project;
use grida_fx_core::docs::takes::read_takes;
use grida_fx_core::value::{is_digest, sha256_hex};
use grida_fx_runtime::events::{read_events_tolerant, run_metadata};
use grida_fx_runtime::folder::{holds_stand_in, safe_name};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The largest `plan.json` read (as `run_catalog` reads it).
const MAX_PLAN_BYTES: u64 = 16 * 1024 * 1024;

/// Where a run folder sits (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    Allocated,
    Named,
    Explicit,
    External,
}

impl Placement {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Placement::Allocated => "allocated",
            Placement::Named => "named",
            Placement::Explicit => "explicit",
            Placement::External => "external",
        }
    }
}

/// What a found folder holds.
#[derive(Debug, Clone)]
pub(crate) enum Holds {
    /// A run: its `plan.json` is an fx-graph-v1 document.
    Recorded(Box<Recorded>),
    /// No `plan.json` at a run's position.
    Empty,
    /// A removal's `.removing-<16 hex>` leftover.
    Tombstone,
    /// An external folder (from the index or the catalog) that is gone.
    Missing,
}

/// What a run's `plan.json` and log say.
#[derive(Debug, Clone)]
pub(crate) struct Recorded {
    pub workflow: String,
    pub source: String,
    pub name: Option<String>,
    pub created_at: String,
    pub state: &'static str,
    pub stopped: Option<String>,
    pub charged_usd: Option<Value>,
    pub stand_in: bool,
    /// The project-relative takes file `plan.json` names, when it names one inside the project.
    pub takes_file: Option<String>,
    pub events: Vec<Value>,
}

/// A folder the walk, the index or the catalog found.
#[derive(Debug, Clone)]
pub(crate) struct Found {
    /// Canonical.
    pub folder: PathBuf,
    pub placement: Placement,
    pub holds: Holds,
    /// The workflow id and run name its position gives (allocated and named positions).
    pub layout_id: Option<String>,
    pub layout_name: Option<String>,
}

impl Found {
    pub(crate) fn recorded(&self) -> Option<&Recorded> {
        match &self.holds {
            Holds::Recorded(recorded) => Some(recorded),
            _ => None,
        }
    }

    /// The list state (module doc).
    pub(crate) fn state(&self) -> &'static str {
        match &self.holds {
            Holds::Recorded(recorded) => recorded.state,
            Holds::Empty => "empty",
            Holds::Tombstone => "removing",
            Holds::Missing => "missing",
        }
    }

    pub(crate) fn workflow(&self) -> Option<&str> {
        self.recorded()
            .map(|recorded| recorded.workflow.as_str())
            .or(self.layout_id.as_deref())
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.recorded()
            .and_then(|recorded| recorded.name.as_deref())
            .or(self.layout_name.as_deref())
    }

    pub(crate) fn created_at(&self) -> Option<&str> {
        self.recorded().map(|recorded| recorded.created_at.as_str())
    }

    pub(crate) fn stand_in(&self) -> bool {
        self.recorded().is_some_and(|recorded| recorded.stand_in)
    }
}

/// Why something could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProblemKind {
    /// A directory of the tree that cannot be listed (the configured root included).
    Unreadable,
    /// A symbolic link, socket, FIFO or device among the tree's directories.
    NotAFolder,
    /// A `plan.json` that is not an fx-graph-v1 document.
    Foreign,
    /// A run whose `plan.json` or `events.jsonl` cannot be read.
    UnreadableRun,
    /// A folder at a run's place that holds files but no `plan.json`: searched for runs, never
    /// removed.
    NoPlan,
    /// A run index entry or catalog entry that cannot be read.
    UnreadableEntry,
    /// An external folder the index or the catalog names that cannot be reached.
    Unreachable,
}

impl ProblemKind {
    pub(crate) fn code(self) -> &'static str {
        match self {
            ProblemKind::Unreadable => "unreadable",
            ProblemKind::NotAFolder => "not_a_folder",
            ProblemKind::Foreign => "foreign_run",
            ProblemKind::UnreadableRun => "unreadable_run",
            ProblemKind::NoPlan => "no_plan",
            ProblemKind::UnreadableEntry => "unreadable_entry",
            ProblemKind::Unreachable => "unreachable_run",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Problem {
    pub path: PathBuf,
    pub kind: ProblemKind,
    pub message: String,
}

/// Every run folder of a project (module doc).
#[derive(Debug, Clone, Default)]
pub(crate) struct RunSet {
    pub found: Vec<Found>,
    pub problems: Vec<Problem>,
    /// The runs tree, canonical (`None` when it does not exist).
    pub runs_root: Option<PathBuf>,
}

impl RunSet {
    /// Walks `project`'s runs tree and reads its run index and catalog.
    pub(crate) fn collect(project: &Project) -> RunSet {
        let mut set = RunSet::default();
        let configured = project.runs_dir();
        match configured.canonicalize() {
            Ok(root) if root.is_dir() => {
                set.walk(&root);
                set.runs_root = Some(root);
            }
            Ok(root) => set.problem(&root, ProblemKind::Unreadable, "not a folder"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => set.problem(&configured, ProblemKind::Unreadable, &error.to_string()),
        }
        // Inside the tree the walk is authoritative, also when the tree is gone (deleted by
        // hand): its index and catalog entries then name no run.
        let tree = set
            .runs_root
            .clone()
            .unwrap_or_else(|| comparable(&configured));
        set.external(project, &tree);
        set.found.sort_by(|a, b| {
            let newest = b.created_at().cmp(&a.created_at());
            newest.then_with(|| a.folder.cmp(&b.folder))
        });
        set
    }

    /// The found folder whose canonical path is `folder`.
    pub(crate) fn at(&self, folder: &Path) -> Option<usize> {
        self.found.iter().position(|found| found.folder == folder)
    }

    fn problem(&mut self, path: &Path, kind: ProblemKind, message: &str) {
        self.problems.push(Problem {
            path: path.to_path_buf(),
            kind,
            message: message.to_string(),
        });
    }

    fn walk(&mut self, root: &Path) {
        // (folder, its parts below the root)
        let mut pending: Vec<(PathBuf, Vec<String>)> = vec![(root.to_path_buf(), Vec::new())];
        while let Some((folder, parts)) = pending.pop() {
            let mut is_run = false;
            if !parts.is_empty() {
                let name = parts.last().map(String::as_str).unwrap_or_default();
                if is_tombstone_name(name) {
                    self.found.push(Found {
                        folder: folder.clone(),
                        placement: if parts.len() == 2 {
                            Placement::Allocated
                        } else {
                            Placement::Explicit
                        },
                        holds: Holds::Tombstone,
                        layout_id: (parts.len() > 1).then(|| parts[0].clone()),
                        layout_name: None,
                    });
                    continue;
                }
                match plan_kind(&folder) {
                    PlanFile::Graph => {
                        is_run = true;
                        match read_recorded(&folder) {
                            Ok(recorded) => {
                                let (placement, layout_id, layout_name) =
                                    placed(&parts, Some(&recorded));
                                self.found.push(Found {
                                    folder: folder.clone(),
                                    placement,
                                    holds: Holds::Recorded(Box::new(recorded)),
                                    layout_id,
                                    layout_name,
                                });
                            }
                            Err(message) => {
                                self.problem(&folder, ProblemKind::UnreadableRun, &message);
                            }
                        }
                    }
                    PlanFile::Other => {
                        self.problem(&folder, ProblemKind::Foreign, "plan.json is not an FX plan");
                        continue;
                    }
                    PlanFile::Unreadable(message) => {
                        self.problem(&folder, ProblemKind::UnreadableRun, &message);
                        continue;
                    }
                    PlanFile::None => {
                        let (placement, layout_id, layout_name) = placed(&parts, None);
                        if placement != Placement::Explicit {
                            if holds_only_a_claim(&folder) {
                                self.found.push(Found {
                                    folder: folder.clone(),
                                    placement,
                                    holds: Holds::Empty,
                                    layout_id,
                                    layout_name,
                                });
                                continue;
                            }
                            // Something at a run's place that is not a run: never taken for
                            // a claim, and searched for runs below it.
                            self.problem(
                                &folder,
                                ProblemKind::NoPlan,
                                "it holds files but no plan.json",
                            );
                        }
                    }
                }
            }
            let listing = match fs::read_dir(&folder) {
                Ok(listing) => listing,
                Err(error) => {
                    self.problem(&folder, ProblemKind::Unreadable, &error.to_string());
                    continue;
                }
            };
            for item in listing {
                let item = match item {
                    Ok(item) => item,
                    Err(error) => {
                        self.problem(&folder, ProblemKind::Unreadable, &error.to_string());
                        continue;
                    }
                };
                let name = item.file_name();
                let Some(name) = name.to_str().map(str::to_string) else {
                    if !is_run {
                        self.problem(&item.path(), ProblemKind::NotAFolder, "not a UTF-8 name");
                    }
                    continue;
                };
                if is_run && matches!(name.as_str(), "files" | "outputs" | "views") {
                    continue;
                }
                let kind = match item.file_type() {
                    Ok(kind) => kind,
                    Err(error) => {
                        self.problem(&item.path(), ProblemKind::Unreadable, &error.to_string());
                        continue;
                    }
                };
                if kind.is_dir() {
                    let mut below = parts.clone();
                    below.push(name);
                    pending.push((item.path(), below));
                } else if !is_run && !kind.is_file() {
                    let what = if kind.is_symlink() {
                        "a symbolic link"
                    } else {
                        "not a regular file or folder"
                    };
                    self.problem(&item.path(), ProblemKind::NotAFolder, what);
                }
            }
        }
    }

    /// The run index's and the catalog's folders outside the tree.
    fn external(&mut self, project: &Project, tree: &Path) {
        let mut outside: BTreeSet<PathBuf> = BTreeSet::new();
        match grida_fx_runtime::run_index::entries(&project.root) {
            Ok(folders) => {
                outside.extend(folders);
            }
            Err(error) => self.problem(
                &project.root.join(grida_fx_runtime::run_index::INDEX),
                ProblemKind::UnreadableEntry,
                &error.to_string(),
            ),
        }
        match grida_fx_viewer::service::registered_runs(&project.root) {
            Ok((runs, unreadable)) => {
                outside.extend(runs.into_iter().map(|run| run.root));
                for name in unreadable {
                    self.problem(
                        &project.root.join(".fx/service/entries").join(&name),
                        ProblemKind::UnreadableEntry,
                        "a catalog entry that cannot be read",
                    );
                }
            }
            Err(error) => self.problem(
                &project.root.join(".fx/service"),
                ProblemKind::UnreadableEntry,
                &error.to_string(),
            ),
        }
        for folder in outside {
            if folder.starts_with(tree) {
                // The walk is authoritative inside the tree.
                continue;
            }
            if folder
                .file_name()
                .is_some_and(|name| is_tombstone_name(&name.to_string_lossy()))
                && folder.is_dir()
            {
                self.found.push(Found {
                    folder,
                    placement: Placement::External,
                    holds: Holds::Tombstone,
                    layout_id: None,
                    layout_name: None,
                });
                continue;
            }
            let holds = match fs::symlink_metadata(&folder) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Holds::Missing,
                Err(error) => {
                    self.problem(&folder, ProblemKind::Unreachable, &error.to_string());
                    continue;
                }
                Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                    self.problem(&folder, ProblemKind::Unreachable, "not a folder");
                    continue;
                }
                Ok(_) => match plan_kind(&folder) {
                    PlanFile::Graph => match read_recorded(&folder) {
                        Ok(recorded) => Holds::Recorded(Box::new(recorded)),
                        Err(message) => {
                            self.problem(&folder, ProblemKind::UnreadableRun, &message);
                            continue;
                        }
                    },
                    PlanFile::None if holds_only_a_claim(&folder) => Holds::Empty,
                    PlanFile::None => {
                        self.problem(
                            &folder,
                            ProblemKind::NoPlan,
                            "it holds files but no plan.json",
                        );
                        continue;
                    }
                    PlanFile::Other => {
                        self.problem(&folder, ProblemKind::Foreign, "plan.json is not an FX plan");
                        continue;
                    }
                    PlanFile::Unreadable(message) => {
                        self.problem(&folder, ProblemKind::UnreadableRun, &message);
                        continue;
                    }
                },
            };
            self.found.push(Found {
                folder,
                placement: Placement::External,
                holds,
                layout_id: None,
                layout_name: None,
            });
        }
    }

    /// The picks the found runs hold: for each `(takes file, step)` whose entry records a
    /// `result`, the runs (indexes into `found`) whose log produced it (spec/store.md §9,
    /// "Picks"). Stand-in runs never hold one.
    pub(crate) fn picks(&self, project: &Project) -> Picks {
        let mut picks = Picks::default();
        let mut files = BTreeMap::new();
        for (index, found) in self.found.iter().enumerate() {
            let Some(recorded) = found.recorded() else {
                continue;
            };
            if recorded.stand_in {
                continue;
            }
            let Some(relative) = &recorded.takes_file else {
                continue;
            };
            let takes = files.entry(relative.clone()).or_insert_with(|| {
                let path = relative
                    .split('/')
                    .fold(project.root.clone(), |path, part| path.join(part));
                read_takes(&path, relative).ok()
            });
            // A takes file that cannot be read may hold any pick: its runs are all kept.
            let Some(takes) = takes else {
                picks.unreadable.insert(index, relative.clone());
                continue;
            };
            for (step, choice) in takes.iter() {
                let Some(result) = &choice.result else {
                    continue;
                };
                if picked_result(&recorded.events, step, choice.take).as_ref() == Some(result) {
                    picks
                        .holders
                        .entry((relative.clone(), step.clone()))
                        .or_default()
                        .push(index);
                }
            }
        }
        picks
    }
}

/// A missing path as a canonical one would read: its deepest existing ancestor canonicalized,
/// with the rest appended (so it compares with paths recorded when it existed).
pub(crate) fn comparable(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut here = path.to_path_buf();
    loop {
        if let Ok(canonical) = here.canonicalize() {
            return missing
                .iter()
                .rev()
                .fold(canonical, |path: PathBuf, part: &std::ffi::OsString| {
                    path.join(part)
                });
        }
        match (here.file_name().map(ToOwned::to_owned), here.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name);
                here = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Whether a folder holds nothing but `run.lock`, temporary names and `.DS_Store`: a claim
/// (module doc).
pub(crate) fn holds_only_a_claim(folder: &Path) -> bool {
    fs::read_dir(folder).is_ok_and(|listing| {
        listing.flatten().all(|item| {
            let name = item.file_name().to_string_lossy().into_owned();
            item.file_type().is_ok_and(|kind| kind.is_file())
                && (name == "run.lock"
                    || name == ".DS_Store"
                    || grida_fx_runtime::folder::is_temporary_name(&name))
        })
    })
}

/// The picks the found runs hold ([`RunSet::picks`]).
#[derive(Debug, Default)]
pub(crate) struct Picks {
    /// For each `(takes file, step)` whose entry records a `result`, the runs holding it.
    pub holders: BTreeMap<(String, String), Vec<usize>>,
    /// The runs whose takes file cannot be read, with that file.
    pub unreadable: BTreeMap<usize, String>,
}

/// `.removing-<16 lowercase hex>`.
pub(crate) fn is_tombstone_name(name: &str) -> bool {
    name.strip_prefix(".removing-").is_some_and(|hex| {
        hex.len() == 16 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

enum PlanFile {
    None,
    Graph,
    Other,
    Unreadable(String),
}

/// What `<folder>/plan.json` is, reading only its kind.
fn plan_kind(folder: &Path) -> PlanFile {
    let path = folder.join("plan.json");
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return PlanFile::None,
        Err(error) => return PlanFile::Unreadable(format!("plan.json: {error}")),
        Ok(metadata) if !metadata.is_file() => {
            return PlanFile::Unreadable("plan.json is not a regular file".into());
        }
        Ok(metadata) if metadata.len() > MAX_PLAN_BYTES => {
            return PlanFile::Unreadable("plan.json is too large".into());
        }
        Ok(_) => {}
    }
    match fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(plan)
                if plan.get("kind").and_then(Value::as_str) == Some("fx-graph-v1")
                    && plan
                        .get("workflow")
                        .and_then(|w| w.get("id"))
                        .and_then(Value::as_str)
                        .is_some() =>
            {
                PlanFile::Graph
            }
            _ => PlanFile::Other,
        },
        Err(error) => PlanFile::Unreadable(format!("plan.json: {error}")),
    }
}

/// A run's `plan.json` and log (module doc). Never writes.
fn read_recorded(folder: &Path) -> Result<Recorded, String> {
    let plan_path = folder.join("plan.json");
    let bytes = fs::read(&plan_path).map_err(|error| format!("plan.json: {error}"))?;
    let plan: Value =
        serde_json::from_slice(&bytes).map_err(|_| "plan.json cannot be read".to_string())?;
    let modified = fs::metadata(&plan_path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let events_path = folder.join("events.jsonl");
    if fs::symlink_metadata(&events_path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err("events.jsonl is not a regular file".into());
    }
    let events =
        read_events_tolerant(&events_path).map_err(|error| format!("events.jsonl: {error}"))?;
    let (name, created_at) = run_metadata(&events, modified);
    let workflow = plan
        .get("workflow")
        .and_then(|w| w.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let source = plan
        .get("workflow")
        .and_then(|w| w.get("file"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut state = "planned";
    let mut stopped = None;
    let mut charged = None;
    let mut stand_in = holds_stand_in(&plan);
    for event in &events {
        match event_name(event) {
            Some("run_started") => {
                state = "unfinished";
                stopped = None;
                charged = None;
                stand_in |= event.get("stand_in") == Some(&Value::Bool(true));
            }
            Some("run_finished") => {
                let ok = event.get("ok") == Some(&Value::Bool(true));
                let incomplete = event.get("incomplete") == Some(&Value::Bool(true));
                state = match (ok, incomplete) {
                    (true, _) => "succeeded",
                    (false, true) => "incomplete",
                    (false, false) => "failed",
                };
                stopped = text(event, "stopped").map(str::to_string);
                charged = event.get("charged_usd").filter(|c| c.is_number()).cloned();
            }
            Some("run_cancelled") => {
                state = "cancelled";
                stopped = None;
                charged = event.get("charged_usd").filter(|c| c.is_number()).cloned();
            }
            _ => {}
        }
    }
    let takes_file = plan
        .get("takes_file")
        .and_then(Value::as_str)
        .filter(|relative| is_inside(relative))
        .map(str::to_string);
    Ok(Recorded {
        workflow,
        source,
        name,
        created_at,
        state,
        stopped,
        charged_usd: charged,
        stand_in,
        takes_file,
        events,
    })
}

/// The placement of a folder at `parts` below the tree, and the id and name its position gives.
fn placed(
    parts: &[String],
    recorded: Option<&Recorded>,
) -> (Placement, Option<String>, Option<String>) {
    let explicit = (Placement::Explicit, None, None);
    match parts {
        [id, run] if is_date_folder(run) => {
            if recorded.is_some_and(|recorded| &recorded.workflow != id) {
                return explicit;
            }
            (Placement::Allocated, Some(id.clone()), None)
        }
        [id, parent, run] => {
            let Some(source_digest) = parent.strip_prefix("named-").filter(|d| is_digest(d)) else {
                return explicit;
            };
            let Some((name, name_digest)) = run.rsplit_once('-') else {
                return explicit;
            };
            if super::run_catalog::validate_name(name).is_err()
                || sha256_hex(name.as_bytes()) != name_digest
            {
                return explicit;
            }
            if let Some(recorded) = recorded
                && (safe_name(&recorded.workflow) != *id
                    || sha256_hex(recorded.source.as_bytes()) != source_digest
                    || recorded
                        .name
                        .as_deref()
                        .is_some_and(|recorded| recorded != name))
            {
                return explicit;
            }
            (Placement::Named, Some(id.clone()), Some(name.to_string()))
        }
        _ => explicit,
    }
}

/// `YYYY-MM-DD-<n>`, `n` from 1 (`folder::new_folder`).
fn is_date_folder(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() < 12 {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    digits(0..4)
        && bytes[4] == b'-'
        && digits(5..7)
        && bytes[7] == b'-'
        && digits(8..10)
        && bytes[10] == b'-'
        && bytes[11] != b'0'
        && digits(11..bytes.len())
}

/// The found indexes a selection may not take away from a pick: for each pick whose every holder
/// is selected, the newest selected holder; and every run whose takes file cannot be read.
pub(crate) fn last_pick_holders(
    picks: &Picks,
    selected: &BTreeSet<usize>,
    set: &RunSet,
) -> BTreeMap<usize, Vec<(String, String)>> {
    let mut kept: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
    for (index, file) in &picks.unreadable {
        kept.entry(*index)
            .or_default()
            .push((file.clone(), String::new()));
    }
    for (pick, holders) in &picks.holders {
        if holders.iter().any(|holder| !selected.contains(holder)) {
            continue;
        }
        if let Some(newest) = holders
            .iter()
            .copied()
            .max_by(|a, b| set.found[*a].created_at().cmp(&set.found[*b].created_at()))
        {
            kept.entry(newest).or_default().push(pick.clone());
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_folders_and_tombstones_are_told_apart() {
        assert!(is_date_folder("2026-10-09-1"));
        assert!(is_date_folder("2026-10-09-12"));
        assert!(!is_date_folder("2026-10-09-0"));
        assert!(!is_date_folder("2026-10-09"));
        assert!(!is_date_folder("rekey"));
        assert!(is_tombstone_name(".removing-0123456789abcdef"));
        assert!(!is_tombstone_name(".removing-0123"));
        assert!(!is_tombstone_name("removing-0123456789abcdef"));
    }
}
