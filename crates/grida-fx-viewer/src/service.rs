//! A project-owned catalog and loopback service. The catalog is private local bookkeeping;
//! portable plans, events and artifacts remain unchanged and authoritative.

use super::*;
use axum::http::Uri;
use axum::routing::post;
use chrono::{DateTime, SecondsFormat, Utc};
use grida_fx_core::value::digest;
use grida_fx_runtime::waiting::{self as requests, RequestState};
use grida_fx_runtime::{attempts, run_index};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::sync::Mutex;
use tokio::sync::oneshot;
use tower::ServiceExt;

const MAX_SCAN_ENTRIES: usize = 2048;
const MAX_SCAN_DEPTH: usize = 4;
const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;

mod run_control;
pub use run_control::{LaunchAccepted, LaunchOperation, LaunchRefused, LaunchRequest, RunLauncher};

/// A public catalog entry contains presentation metadata and an opaque local URL, never paths.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub url: String,
    pub state: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<Workflow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_number: Option<u64>,
}

/// Recorded workflow identity, independent of a particular plan or run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    pub id: String,
    pub source: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct RunMetadata {
    created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workflow: Option<Workflow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Clone)]
pub struct Catalog {
    root: PathBuf,
    runs_dir: PathBuf,
    state_dir: PathBuf,
    project_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Record {
    Run {
        id: String,
        root: PathBuf,
        identity: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<RunMetadata>,
    },
    Plan {
        id: String,
        graph: Value,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_at: Option<String>,
    },
}

impl Record {
    fn id(&self) -> &str {
        match self {
            Self::Run { id, .. } | Self::Plan { id, .. } => id,
        }
    }

    fn same_registration(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Run {
                    id,
                    root,
                    identity,
                    title,
                    ..
                },
                Self::Run {
                    id: other_id,
                    root: other_root,
                    identity: other_identity,
                    title: other_title,
                    ..
                },
            ) => {
                id == other_id
                    && root == other_root
                    && identity == other_identity
                    && title == other_title
            }
            (
                Self::Plan {
                    id, graph, title, ..
                },
                Self::Plan {
                    id: other_id,
                    graph: other_graph,
                    title: other_title,
                    ..
                },
            ) => id == other_id && graph == other_graph && title == other_title,
            _ => false,
        }
    }
}

impl Catalog {
    /// Opens local state beneath the canonical project root. No workflow code is loaded.
    pub fn open(root: &Path, runs_dir: &Path) -> io::Result<Self> {
        let mut catalog = Self::locate(root)?;
        catalog.runs_dir = catalog.root.join(runs_dir);
        private_dir(&catalog.root.join(".fx"), false)?;
        private_dir(&catalog.state_dir, true)?;
        private_ignore(&catalog.state_dir)?;
        private_dir(&catalog.state_dir.join("entries"), true)?;
        Ok(catalog)
    }

    /// Binds an exact project directory without creating local service state.
    pub fn locate(root: &Path) -> io::Result<Self> {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(invalid("The project root is not a directory."));
        }
        let state_dir = root.join(".fx/service");
        let project_id = digest(&json!(["fx-local-project-v1", root]));
        Ok(Self {
            runs_dir: root.join("runs"),
            root,
            state_dir,
            project_id,
        })
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Reports whether confined service state already exists, without creating it.
    pub fn has_state(&self) -> io::Result<bool> {
        match self.check_state() {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Registers an explicitly selected existing run. This operation is available only to local
    /// callers, never as an HTTP endpoint accepting browser-provided filesystem paths.
    pub fn register_run(&self, run: &Path) -> io::Result<Entry> {
        self.register_matching_run(run, None)
    }

    /// Return the scoped route only if this exact run already has a catalog entry.
    pub fn registered_run_route(&self, run: &Path) -> io::Result<Option<String>> {
        Ok(self
            .registered_attempt(run)?
            .map(|(id, _)| self.exact_url(&id)))
    }

    /// Returns the stable run route only if the supplied attempt is already registered.
    /// Opening the route resolves the newest recorded attempt and redirects to its exact URL.
    pub fn registered_run_latest_route(&self, run: &Path) -> io::Result<Option<String>> {
        Ok(self
            .registered_attempt(run)?
            .map(|(_, attempt)| self.latest_url(&attempt.run_id)))
    }

    fn registered_attempt(&self, run: &Path) -> io::Result<Option<(String, attempts::Attempt)>> {
        let root = run.canonicalize()?;
        let details = run_details(&root)?;
        let id = local_run_id(&root, &details.identity);
        match self.record(&id) {
            Ok(Record::Run {
                root: recorded,
                identity: original,
                ..
            }) if recorded == root && original == details.identity => {
                Ok(Some((id, details.attempt)))
            }
            Ok(_) => Err(invalid("The registered run no longer matches.")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn exact_url(&self, id: &str) -> String {
        format!("/p/{}/runs/{id}/", self.project_id)
    }

    fn latest_url(&self, run_id: &str) -> String {
        format!("/p/{}/run/{run_id}/", self.project_id)
    }

    /// Registers a run only if it still matches the recorded evidence read by inspection.
    pub fn register_inspected_run(
        &self,
        run: &Path,
        graph: &Value,
        initial_event: &Value,
    ) -> io::Result<Entry> {
        let expected = recorded_run_identity(graph, initial_event.clone());
        let entry = self.register_matching_run(run, Some(&expected))?;
        if entry.state == "unavailable" {
            return Err(invalid("The inspected run folder is unavailable."));
        }
        Ok(entry)
    }

    fn register_matching_run(&self, run: &Path, expected: Option<&str>) -> io::Result<Entry> {
        self.check_state()?;
        let root = run.canonicalize()?;
        let details = run_details(&root)?;
        let identity = details.identity;
        if expected.is_some_and(|expected| expected != identity) {
            return Err(invalid("The inspected run folder has been replaced."));
        }
        let id = local_run_id(&root, &identity);
        let record = Record::Run {
            id,
            root,
            identity,
            title: details.title,
            metadata: Some(details.metadata),
        };
        Ok(self.entry(&self.persist(&record)?))
    }

    /// Saves an immutable materialized plan in private project state; starts no workflow.
    pub fn register_plan(&self, graph: &Value) -> io::Result<Entry> {
        self.check_state()?;
        plan_source(graph.clone())?;
        let id = digest(&json!(["fx-local-plan-v1", graph]));
        let title = graph_title(graph);
        let record = Record::Plan {
            id,
            graph: graph.clone(),
            title,
            created_at: Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
        };
        Ok(self.entry(&self.persist(&record)?))
    }

    /// Enumerates registered entries and discovers runs only within the configured run tree.
    /// Missing or replaced registrations remain visible as unavailable; their URLs never retarget.
    pub fn entries(&self) -> io::Result<Vec<Entry>> {
        self.check_state()?;
        self.discover();
        let mut entries = Vec::new();
        for item in fs::read_dir(self.state_dir.join("entries"))? {
            let item = item?;
            let Some(id) = item
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .map(str::to_owned)
            else {
                continue;
            };
            if let Ok(record) = self.record(&id) {
                entries.push(self.entry(&record));
            }
        }
        entries.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.id.cmp(&b.id)));
        Ok(entries)
    }

    fn check_state(&self) -> io::Result<()> {
        for path in [
            self.root.join(".fx"),
            self.state_dir.clone(),
            self.state_dir.join("entries"),
        ] {
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || path.canonicalize()? != path
            {
                return Err(invalid("The local service state directory is unavailable."));
            }
        }
        Ok(())
    }

    fn persist(&self, record: &Record) -> io::Result<Record> {
        let directory = self.state_dir.join("entries");
        let target = directory.join(format!("{}.json", record.id()));
        let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(invalid("The catalog entry is too large."));
        }
        // Discovery runs while the index is observed. An already registered immutable entry
        // needs no temporary file, fsync or mutation on each refresh.
        match fs::symlink_metadata(&target) {
            Ok(_) => {
                let existing = self.record(record.id())?;
                if existing.same_registration(record) {
                    return Ok(existing);
                }
                return Err(invalid(
                    "An incompatible local catalog entry already exists.",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut file = tempfile::NamedTempFile::new_in(&directory)?;
        private_file(file.as_file())?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        match file.persist_noclobber(&target) {
            Ok(_) => Ok(record.clone()),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                let existing = self.record(record.id())?;
                if existing.same_registration(record) {
                    Ok(existing)
                } else {
                    Err(invalid(
                        "An incompatible local catalog entry already exists.",
                    ))
                }
            }
            Err(error) => Err(error.error),
        }
    }

    fn record(&self, id: &str) -> io::Result<Record> {
        self.check_state()?;
        if !is_digest(id) {
            return Err(invalid("Unknown catalog entry."));
        }
        read_entry(&self.state_dir.join("entries"), id)
    }

    fn entry(&self, record: &Record) -> Entry {
        let (kind, title, state, metadata, attempt) = match record {
            Record::Run {
                root,
                identity,
                title,
                metadata,
                ..
            } => {
                let details = run_details(root)
                    .ok()
                    .filter(|run| run.identity == *identity);
                let state = details
                    .as_ref()
                    .ok_or_else(|| invalid("The registered run folder is unavailable."))
                    .and_then(|_| recorded_state(root))
                    .unwrap_or_else(|_| "unavailable".into());
                let attempt = details.as_ref().map(|run| run.attempt.clone());
                let metadata = metadata.clone().or_else(|| details.map(|run| run.metadata));
                ("run", title.clone(), state, metadata, attempt)
            }
            Record::Plan {
                title,
                graph,
                created_at,
                ..
            } => (
                "plan",
                title.clone(),
                "planned".into(),
                Some(RunMetadata {
                    created_at: created_at
                        .clone()
                        .unwrap_or_else(|| self.record_time(record.id())),
                    workflow: graph_workflow(graph),
                    name: None,
                }),
                None,
            ),
        };
        let metadata = metadata.unwrap_or_else(|| RunMetadata {
            created_at: self.record_time(record.id()),
            workflow: None,
            name: None,
        });
        let (run_id, attempt_number) = attempt
            .map(|identity| (identity.run_id, identity.attempt_number))
            .unzip();
        Entry {
            id: record.id().into(),
            kind: kind.into(),
            title,
            url: format!("/p/{}/{}s/{}/", self.project_id, kind, record.id()),
            state,
            created_at: normalize_time(&metadata.created_at)
                .unwrap_or_else(|| self.record_time(record.id())),
            workflow: metadata.workflow.filter(safe_workflow),
            name: metadata.name.filter(|name| safe_name(name)),
            run_id,
            attempt_number,
        }
    }

    fn record_time(&self, id: &str) -> String {
        let time = read::confined_file(&self.state_dir, &format!("entries/{id}.json"))
            .and_then(fs::metadata)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        timestamp(time)
    }

    pub(crate) fn source(&self, id: &str, kind: &str) -> io::Result<Source> {
        match self.record(id)? {
            Record::Run { root, identity, .. } if kind == "runs" => {
                checked_run(&root, &identity)?;
                Ok(Source::Run(root))
            }
            Record::Plan { graph, .. } if kind == "plans" => plan_source(graph),
            _ => Err(invalid("The requested catalog entry does not exist.")),
        }
    }

    fn history(&self, run_id: &str) -> io::Result<RunHistory> {
        self.check_state()?;
        let (registrations, _) = registered_runs(&self.root)?;
        let mut candidates: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
        for registered in registrations {
            candidates
                .entry(registered.root)
                .or_default()
                .insert(registered.identity);
        }

        // The project index records starts even when the invocation has no viewer, including
        // folders outside the configured run tree. HTTP never supplies a folder to register.
        let index = self.root.join(run_index::INDEX);
        match fs::symlink_metadata(&index) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(invalid("The local run index is unavailable.")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        for path in run_index::entries(&self.root)? {
            candidates.entry(path).or_default();
        }

        let mut history = RunHistory::default();
        let mut allocations = BTreeMap::new();
        for allocation in attempts::allocations(&self.root, run_id)? {
            let number = allocation.attempt_number;
            let path = allocation.path(&self.root);
            let folder = allocation.existing_folder(&self.root);
            let verified = matches!(folder, Ok(Some(_)));
            if verified {
                candidates.entry(path.clone()).or_default();
            }
            // Explicit removal deletes the index and catalog entries. A missing folder
            // still tracked in either must never make the stable link fall back silently.
            if allocation.started && (!matches!(folder, Ok(None)) || candidates.contains_key(&path))
            {
                history.unavailable.insert(number);
            }
            allocations.insert(number, (path, verified));
        }

        for (root, bound) in candidates {
            let Ok(first) = run_start(&root) else {
                continue;
            };
            let attempt = attempts::from_start(&first, &root);
            if attempt.run_id != run_id {
                continue;
            }
            let Ok(details) = run_details_from_start(&root, first, attempt) else {
                continue;
            };
            if !bound.is_empty() && !bound.contains(&details.identity) {
                continue;
            }
            let number = details.attempt.attempt_number;
            if let Some((path, verified)) = allocations.get(&number) {
                if *path != root {
                    return Err(invalid("Multiple folders claim the same run attempt."));
                }
                if !verified {
                    history.unavailable.insert(number);
                    continue;
                }
            }
            let attempt = HistoryAttempt {
                root,
                identity: details.identity,
            };
            if history.attempts.insert(number, attempt).is_some() {
                return Err(invalid("Multiple folders claim the same run attempt."));
            }
            history.unavailable.remove(&number);
        }
        Ok(history)
    }

    fn exact_attempt_url(&self, attempt: &HistoryAttempt) -> io::Result<String> {
        let id = local_run_id(&attempt.root, &attempt.identity);
        if self.record(&id).is_err() {
            return self
                .register_matching_run(&attempt.root, Some(&attempt.identity))
                .map(|entry| entry.url);
        }
        checked_run(&attempt.root, &attempt.identity)?;
        Ok(self.exact_url(&id))
    }

    fn latest_exact_url(&self, run_id: &str) -> io::Result<String> {
        let history = self.history(run_id)?;
        let (_, attempt) = history.latest()?;
        recorded_state(&attempt.root)?;
        self.exact_attempt_url(attempt)
    }

    fn history_document(&self, id: &str) -> io::Result<Value> {
        let (root, details) = self.exact_run(id)?;
        let selected = details.attempt;
        let history = self.history(&selected.run_id)?;
        if !history
            .attempts
            .get(&selected.attempt_number)
            .is_some_and(|attempt| attempt.root == root && attempt.identity == details.identity)
        {
            return Err(invalid("The selected run attempt has changed."));
        }
        let latest_number = history.latest().ok().map(|(number, _)| number);
        let mut attempts = Vec::new();
        let mut latest_available = false;
        for (&number, attempt) in history.attempts.iter().rev() {
            let url = self.exact_attempt_url(attempt)?;
            let state = recorded_state(&attempt.root).unwrap_or_else(|_| "unavailable".into());
            latest_available |= latest_number == Some(number) && state != "unavailable";
            attempts.push(json!({"attempt_number": number, "url": url, "state": state}));
        }
        Ok(
            json!({"kind": "fx-viewer-run-history-v1", "run_id": selected.run_id,
            "latest_url": self.latest_url(&selected.run_id), "latest_available": latest_available,
            "selected_attempt": selected.attempt_number,
            "attempts": attempts}),
        )
    }

    fn exact_run(&self, id: &str) -> io::Result<(PathBuf, RunDetails)> {
        let Record::Run { root, identity, .. } = self.record(id)? else {
            return Err(invalid("The selected catalog entry is not a run."));
        };
        let details = run_details(&root)?;
        if details.identity != identity {
            return Err(invalid("The registered run folder has been replaced."));
        }
        Ok((root, details))
    }

    fn discover(&self) {
        // Refuse a configured symlink tree; explicit registration can still select a canonical run.
        if fs::symlink_metadata(&self.runs_dir)
            .map_or(true, |m| !m.is_dir() || m.file_type().is_symlink())
        {
            return;
        }
        let Ok(root) = self.runs_dir.canonicalize() else {
            return;
        };
        let mut pending = vec![(root.clone(), 0)];
        let mut seen = 0;
        while let Some((directory, depth)) = pending.pop() {
            if seen >= MAX_SCAN_ENTRIES {
                break;
            }
            seen += 1;
            if directory.join("plan.json").is_file() {
                let _ = self.register_run(&directory);
                continue;
            }
            if depth >= MAX_SCAN_DEPTH {
                continue;
            }
            let Ok(children) = fs::read_dir(&directory) else {
                continue;
            };
            for child in children.flatten() {
                if seen >= MAX_SCAN_ENTRIES {
                    break;
                }
                seen += 1;
                if child
                    .file_type()
                    .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
                {
                    let path = child.path();
                    if path.starts_with(&root) {
                        pending.push((path, depth + 1));
                    }
                }
            }
        }
    }
}

fn recorded_state(root: &Path) -> io::Result<String> {
    read::read_recorded_inventory(root)
        .map(|snapshot| catalog_run_state(root, snapshot.document.state))
}

fn catalog_run_state(root: &Path, recorded: String) -> String {
    if matches!(recorded.as_str(), "unfinished" | "awaiting")
        && let Ok(requests) = requests::list(root)
        && !requests.is_empty()
    {
        // A detached response or elapsed deadline can change what needs attention
        // before a runner records the resolution in the event log.
        return if requests
            .iter()
            .any(|request| matches!(request.state, RequestState::Pending))
        {
            "awaiting".into()
        } else {
            "unfinished".into()
        };
    }
    recorded
}

/// A run the project's catalog registered, read without creating or changing local state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredRun {
    /// The canonical run folder the entry binds.
    pub root: PathBuf,
    /// The recorded identity the entry binds ([`run_identity`]).
    pub identity: String,
}

/// The run entries of `project_root`'s catalog, and the names of entries that cannot be read.
/// Reads `.fx/service/entries` only; a project without a catalog has none. Nothing is created,
/// discovered or registered (spec/service.md §4).
pub fn registered_runs(project_root: &Path) -> io::Result<(Vec<RegisteredRun>, Vec<String>)> {
    let Some(entries) = entries_dir(project_root)? else {
        return Ok((Vec::new(), Vec::new()));
    };
    let mut runs = Vec::new();
    let mut unreadable = Vec::new();
    for item in fs::read_dir(&entries)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json").filter(|id| is_digest(id)) else {
            continue;
        };
        match read_entry(&entries, id) {
            Ok(Record::Run { root, identity, .. }) => runs.push(RegisteredRun { root, identity }),
            Ok(Record::Plan { .. }) => {}
            Err(_) => unreadable.push(name),
        }
    }
    Ok((runs, unreadable))
}

/// Checks an existing private catalog binding without discovering or registering anything.
pub(crate) fn matches_run_entry(project: &Path, id: &str, root: &Path) -> io::Result<bool> {
    let Some(entries) = entries_dir(project)? else {
        return Ok(false);
    };
    match read_entry(&entries, id)? {
        Record::Run {
            root: bound,
            identity,
            ..
        } if bound == root => {
            checked_run(&bound, &identity)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Checks an immutable materialized plan's existing private registration.
pub(crate) fn matches_plan_entry(project: &Path, id: &str, graph: &Value) -> io::Result<bool> {
    let Some(entries) = entries_dir(project)? else {
        return Ok(false);
    };
    Ok(matches!(read_entry(&entries, id)?, Record::Plan { graph: bound, .. } if bound == *graph))
}

/// The identity a catalog entry binds for the run in `root` (spec/service.md §4): its plan and
/// first `run_started`, without creation time or name.
pub fn run_identity(root: &Path) -> io::Result<String> {
    Ok(run_details(&root.canonicalize()?)?.identity)
}

/// Removes `project_root`'s catalog run entries that bind `root` (canonical, as registered), and
/// only those with `identity` when one is given: how many went. A removed run's URL is then not
/// found (spec/service.md §4).
pub fn deregister_runs(
    project_root: &Path,
    root: &Path,
    identity: Option<&str>,
) -> io::Result<usize> {
    let Some(entries) = entries_dir(project_root)? else {
        return Ok(0);
    };
    let mut removed = 0;
    for item in fs::read_dir(&entries)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json").filter(|id| is_digest(id)) else {
            continue;
        };
        if let Ok(Record::Run {
            root: registered,
            identity: bound,
            ..
        }) = read_entry(&entries, id)
            && registered == root
            && identity.is_none_or(|identity| identity == bound)
        {
            match fs::remove_file(item.path()) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(removed)
}

/// `<project>/.fx/service/entries` when it exists as real folders; `None` when there is none.
/// `.fx` itself may be a symbolic link to the project's local state; `service` and `entries`
/// must be real folders.
fn entries_dir(project_root: &Path) -> io::Result<Option<PathBuf>> {
    let fx = project_root.canonicalize()?.join(".fx");
    let mut path = match fs::metadata(&fx) {
        Ok(metadata) if metadata.is_dir() => fx.canonicalize()?,
        Ok(_) => return Err(invalid("The local service state directory is unavailable.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    for part in ["service", "entries"] {
        path = path.join(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(invalid("The local service state directory is unavailable.")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    Ok(Some(path))
}

/// One entry, checked as [`Catalog::record`] checks it.
fn read_entry(entries: &Path, id: &str) -> io::Result<Record> {
    let (file, _) = read::open_confined_file(entries, &format!("{id}.json"))?;
    if file.metadata()?.len() > MAX_RECORD_BYTES {
        return Err(invalid("The catalog entry is too large."));
    }
    let record: Record =
        serde_json::from_reader(file).map_err(|_| invalid("The catalog entry cannot be read."))?;
    let expected = match &record {
        Record::Run { root, identity, .. } => local_run_id(root, identity),
        Record::Plan { graph, .. } => digest(&json!(["fx-local-plan-v1", graph])),
    };
    if record.id() != id || expected != id {
        return Err(invalid("The catalog entry identity differs."));
    }
    Ok(record)
}

fn graph_title(graph: &Value) -> String {
    graph["workflow"]["title"]
        .as_str()
        .or_else(|| graph["workflow"]["id"].as_str())
        .unwrap_or("Workflow")
        .to_owned()
}

struct RunDetails {
    identity: String,
    title: String,
    attempt: attempts::Attempt,
    metadata: RunMetadata,
}

struct HistoryAttempt {
    root: PathBuf,
    identity: String,
}

#[derive(Default)]
struct RunHistory {
    attempts: BTreeMap<u64, HistoryAttempt>,
    unavailable: BTreeSet<u64>,
}

impl RunHistory {
    fn latest(&self) -> io::Result<(u64, &HistoryAttempt)> {
        let (&number, attempt) = self
            .attempts
            .last_key_value()
            .ok_or_else(|| invalid("No recorded attempt is available."))?;
        if self.unavailable.last().is_some_and(|known| *known > number) {
            return Err(invalid("A newer run attempt is unavailable."));
        }
        Ok((number, attempt))
    }
}

fn run_start(root: &Path) -> io::Result<Value> {
    if !root.is_dir()
        || root.canonicalize()? != root
        || fs::symlink_metadata(root)?.file_type().is_symlink()
    {
        return Err(invalid("The registered run folder is unavailable."));
    }
    let mut first_line = Vec::new();
    let events = read::confined_file(root, "events.jsonl")?;
    let mut reader = BufReader::new(std::io::Read::take(File::open(events)?, MAX_RECORD_BYTES));
    reader.read_until(b'\n', &mut first_line)?;
    if first_line.last() != Some(&b'\n') {
        return Err(invalid("The run has no complete initial event."));
    }
    let first: Value = serde_json::from_slice(&first_line)
        .map_err(|_| invalid("The initial run event cannot be read."))?;
    if first["kind"] != "fx-run-events-v1" || first["event"] != "run_started" {
        return Err(invalid("The run has no initial run_started event."));
    }
    Ok(first)
}

fn run_details(root: &Path) -> io::Result<RunDetails> {
    let first = run_start(root)?;
    let attempt = attempts::from_start(&first, root);
    run_details_from_start(root, first, attempt)
}

fn run_details_from_start(
    root: &Path,
    first: Value,
    attempt: attempts::Attempt,
) -> io::Result<RunDetails> {
    let plan = File::open(read::confined_file(root, "plan.json")?)?;
    let plan_metadata = plan.metadata()?;
    if plan_metadata.len() > MAX_RECORD_BYTES {
        return Err(invalid("The recorded plan is too large."));
    }
    let graph: Value =
        serde_json::from_reader(plan).map_err(|_| invalid("The recorded plan cannot be read."))?;
    plan_source(graph.clone())?;
    if first.get("plan").is_some() && first["plan"] != graph["plan"] {
        return Err(invalid("The initial event belongs to another plan."));
    }
    let (name, created_at) = grida_fx_runtime::events::run_metadata(
        std::slice::from_ref(&first),
        plan_metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
    );
    let created_at = normalize_time(&created_at).unwrap_or(created_at);
    let name = name.filter(|name| safe_name(name));
    Ok(RunDetails {
        attempt,
        identity: recorded_run_identity(&graph, first),
        title: graph_title(&graph),
        metadata: RunMetadata {
            created_at,
            workflow: graph_workflow(&graph),
            name,
        },
    })
}

fn recorded_run_identity(graph: &Value, mut first: Value) -> String {
    if let Some(first) = first.as_object_mut() {
        first.remove("created_at");
        first.remove("name");
    }
    // Resumes retain the first invocation, including legacy events without invocation_id.
    digest(&json!([graph, first]))
}

fn local_run_id(root: &Path, identity: &str) -> String {
    digest(&json!(["fx-local-run-v1", root, identity]))
}

fn checked_run(root: &Path, identity: &str) -> io::Result<()> {
    if run_details(root)?.identity == identity {
        Ok(())
    } else {
        Err(invalid("The registered run folder has been replaced."))
    }
}

fn graph_workflow(graph: &Value) -> Option<Workflow> {
    let workflow = Workflow {
        id: graph["workflow"]["id"].as_str()?.to_owned(),
        source: graph["workflow"]["file"].as_str()?.to_owned(),
    };
    safe_workflow(&workflow).then_some(workflow)
}

fn safe_workflow(workflow: &Workflow) -> bool {
    let source = &workflow.source;
    !workflow.id.is_empty()
        && !workflow.id.chars().any(char::is_control)
        && !source.is_empty()
        && !source.chars().any(char::is_control)
        && !source.starts_with(['/', '\\'])
        && !source.contains('\\')
        && !source.contains("://")
        && !source
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
        && !(source.as_bytes().get(1) == Some(&b':') && source.as_bytes()[0].is_ascii_alphabetic())
}

pub(super) fn safe_name(name: &str) -> bool {
    name.len() <= 64
        && name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn timestamp(time: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn normalize_time(time: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(time).ok().map(|time| {
        time.with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    })
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn private_dir(path: &Path, enforce_private: bool) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(invalid("Service state must not use symbolic links."));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    return private_dir(path, enforce_private);
                }
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    if enforce_private {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let _ = enforce_private;
    Ok(())
}

fn private_file(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Runtime descriptors contain a local control credential and catalog records contain source
/// paths. Exclude this entire directory even in a newly initialized Git project.
fn private_ignore(directory: &Path) -> io::Result<()> {
    let target = directory.join(".gitignore");
    let verify = || {
        let path = read::confined_file(directory, ".gitignore")?;
        if fs::read_to_string(&path)?.trim() != "*" {
            return Err(invalid(
                "The local service exclusion file must contain '*'.",
            ));
        }
        private_file(&File::open(path)?)
    };
    match fs::symlink_metadata(&target) {
        Ok(_) => return verify(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    private_file(file.as_file())?;
    file.write_all(b"*\n")?;
    file.as_file().sync_all()?;
    match file.persist_noclobber(target) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => verify(),
        Err(error) => Err(error.error),
    }
}

#[derive(Clone)]
struct ServiceState {
    catalog: Catalog,
    authority: String,
    url: String,
    instance_id: String,
    control_token: String,
    run_launcher: Option<RunLauncher>,
    stop: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

/// The loopback listener could not be bound. [`bind`] wraps that error, keeping its kind, so the
/// owner can tell a refused port from unavailable local state and name the port.
#[derive(Debug)]
pub struct ListenFailed(pub io::Error);

impl std::fmt::Display for ListenFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for ListenFailed {}

/// Binds a persistent project service. The receiver resolves after an authenticated stop request;
/// the owner combines it with process signals and passes that future to Server::serve_until.
pub async fn bind(
    catalog: Catalog,
    port: u16,
    instance_id: String,
    control_token: String,
) -> io::Result<(Server, oneshot::Receiver<()>)> {
    bind_service(catalog, port, instance_id, control_token, None).await
}

/// Binds the project service with an engine-supplied launcher for resume and rerun actions.
pub async fn bind_with_run_launcher(
    catalog: Catalog,
    port: u16,
    instance_id: String,
    control_token: String,
    run_launcher: RunLauncher,
) -> io::Result<(Server, oneshot::Receiver<()>)> {
    bind_service(
        catalog,
        port,
        instance_id,
        control_token,
        Some(run_launcher),
    )
    .await
}

async fn bind_service(
    catalog: Catalog,
    port: u16,
    instance_id: String,
    control_token: String,
    run_launcher: Option<RunLauncher>,
) -> io::Result<(Server, oneshot::Receiver<()>)> {
    if instance_id.is_empty() || control_token.is_empty() {
        return Err(invalid(
            "Service identity and control token must be nonempty.",
        ));
    }
    catalog.check_state()?;
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .await
        .map_err(|error| io::Error::new(error.kind(), ListenFailed(error)))?;
    let authority = listener.local_addr()?.to_string();
    let url = format!("http://{authority}/");
    let (sender, receiver) = oneshot::channel();
    let state = ServiceState {
        catalog,
        authority,
        url: url.clone(),
        instance_id,
        control_token,
        run_launcher,
        stop: Arc::new(Mutex::new(Some(sender))),
    };
    Ok((
        Server {
            listener,
            app: service_router(state),
            url,
        },
        receiver,
    ))
}

fn service_router(state: ServiceState) -> Router {
    // The outer middleware protects the dashboard, lifecycle API and nested artifact requests.
    let guard = Arc::new(AppState {
        source: Source::Plan(Arc::new(Value::Null)),
        authority: state.authority.clone(),
        artifact_prefix: String::new(),
        project_root: state.catalog.root.clone(),
    });
    Router::new()
        .route("/api/service", get(health))
        .route("/api/service/stop", post(stop))
        .route("/api/view", get(index))
        .route("/api/catalog", get(index))
        .route("/api/viewer_config", get(service_viewer_config))
        .route("/api/{*path}", get(api_missing))
        .fallback(dispatch)
        .layer(middleware::from_fn_with_state(guard, local_request))
        .with_state(Arc::new(state))
}

fn authorized(state: &ServiceState, headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| token == state.control_token)
}

async fn health(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Service authentication is required.",
        );
    }
    observed_json(
        json!({"kind":"fx-service-status-v1", "state":"running", "protocol_version":1,
        "project_id":state.catalog.project_id(), "instance_id":state.instance_id, "pid":std::process::id(), "url":state.url,
        "run_history":true}),
    )
}

async fn stop(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Service authentication is required.",
        );
    }
    if let Some(sender) = state.stop.lock().expect("service stop lock").take() {
        let _ = sender.send(());
    }
    observed_json(json!({"kind":"fx-service-stop-v1", "state":"stopping"}))
}

async fn index(State(state): State<Arc<ServiceState>>) -> Response {
    let catalog = state.catalog.clone();
    match tokio::task::spawn_blocking(move || catalog.entries()).await {
        Ok(Ok(entries)) => observed_json(
            json!({"kind":"fx-service-index-v1", "project_id":state.catalog.project_id(),
            "title": state.catalog.root.file_name().map(|n|n.to_string_lossy()).unwrap_or_default(), "entries":entries}),
        ),
        _ => failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The project catalog cannot be read.",
        ),
    }
}

async fn service_viewer_config(State(state): State<Arc<ServiceState>>) -> Response {
    super::project_viewer_config(state.catalog.root.clone()).await
}

async fn latest_run(catalog: Catalog, id: String) -> Response {
    let mut response =
        match tokio::task::spawn_blocking(move || catalog.latest_exact_url(&id)).await {
            Ok(Ok(url)) => axum::response::Redirect::temporary(&url).into_response(),
            _ => failure(
                StatusCode::NOT_FOUND,
                "The latest attempt of this run is unavailable.",
            ),
        };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn exact_run_history(catalog: Catalog, id: String) -> Response {
    match tokio::task::spawn_blocking(move || catalog.history_document(&id)).await {
        Ok(Ok(history)) => observed_json(history),
        _ => failure(
            StatusCode::NOT_FOUND,
            "The selected run history is unavailable.",
        ),
    }
}

async fn dispatch(State(state): State<Arc<ServiceState>>, mut request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if path.starts_with("/p/") {
        let parts: Vec<_> = path.splitn(6, '/').collect();
        if parts.len() < 5
            || parts[2] != state.catalog.project_id()
            || !matches!(parts[3], "run" | "runs" | "plans")
            || !is_digest(parts[4])
        {
            return failure(StatusCode::NOT_FOUND, "Unknown project entry.");
        }
        let catalog = state.catalog.clone();
        let id = parts[4].to_owned();
        let kind = parts[3].to_owned();
        let suffix = parts.get(5).copied().unwrap_or_default();
        if kind == "runs" && (suffix == "api/control" || suffix.starts_with("api/control/")) {
            return run_control::dispatch(state, id, suffix, request).await;
        }
        if kind == "run" || kind == "runs" && suffix == "api/history" {
            if !matches!(*request.method(), Method::GET | Method::HEAD) {
                return failure(
                    StatusCode::METHOD_NOT_ALLOWED,
                    "This resource is read-only.",
                );
            }
            return match kind.as_str() {
                "run" if suffix.is_empty() => latest_run(catalog, id).await,
                "runs" => exact_run_history(catalog, id).await,
                _ => failure(StatusCode::NOT_FOUND, "Unknown project entry."),
            };
        }
        let source = match tokio::task::spawn_blocking(move || catalog.source(&id, &kind)).await {
            Ok(Ok(source)) => source,
            _ => {
                return failure(
                    StatusCode::NOT_FOUND,
                    "The selected project entry is unavailable.",
                );
            }
        };
        let prefix = format!("/p/{}/{}/{}", parts[2], parts[3], parts[4]);
        let relative = format!("/{suffix}");
        let query = request
            .uri()
            .query()
            .map(|v| format!("?{v}"))
            .unwrap_or_default();
        if parts.len() == 5 {
            return axum::response::Redirect::permanent(&format!("{prefix}/{query}"))
                .into_response();
        }
        let Ok(uri) = format!("{relative}{query}").parse::<Uri>() else {
            return failure(StatusCode::BAD_REQUEST, "Invalid project route.");
        };
        *request.uri_mut() = uri;
        return router(AppState {
            source,
            authority: state.authority.clone(),
            artifact_prefix: prefix,
            project_root: state.catalog.root.clone(),
        })
        .oneshot(request)
        .await
        .expect("router is infallible");
    }
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return failure(
            StatusCode::METHOD_NOT_ALLOWED,
            "This resource is read-only.",
        );
    }
    asset(request).await
}

#[cfg(test)]
mod tests;
