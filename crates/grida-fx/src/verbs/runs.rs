//! `grida-fx runs list` and `grida-fx runs remove` (spec/store.md §8 "Removing a run" and §9;
//! docs/guide/10-cleanup.md).
//!
//! Both read the project found from the working directory and its runs ([`super::run_set`]).
//!
//! `runs list [--workflow ID] [--json]` prints one line per run, newest first:
//! `<created, local>  <state>  <WORKFLOW_ID/NAME or folder>[  <spent>][  stand-in]`, and on
//! stderr `skipped   <path>: <why>` for what could not be read. `--json`:
//! `{"kind":"fx-run-list-v1","runs":[…],"skipped":[…]}`. Reads only; exit 0.
//!
//! `runs remove RUN…` or `runs remove (--workflow ID | --state STATE… | --before DATE)…`:
//!
//! - Every RUN is resolved before anything happens; one that is not a run fails the command (exit
//!   2, nothing removed). A RUN is a run folder (its own `plan.json` is an FX plan), an empty claim
//!   or a tombstone the listing shows, `WORKFLOW_ID/NAME` of a named run (found by its position,
//!   so an empty named claim is found as well), or a missing folder the run index or the catalog
//!   still names (then only forgotten). A folder that is or holds the project, its runs tree, its
//!   store or `.fx`, or that holds other runs, is not one.
//! - Filters match the dated runs (`allocated`) only, AND together, and say how many named,
//!   explicit and external runs they passed over; an empty claim younger than a minute is passed
//!   over too (a run may be starting in it).
//! - The last kept run holding a pick (spec/store.md §9, "Picks"), and any run whose takes file
//!   cannot be read, is refused (`holds_pick`) when named, and passed over by a filter.
//! - Without `--yes` it prints what it would remove and removes nothing. With it, the removal
//!   holds `.fx/runs/lock` from reading the runs to the end (one removal at a time), and each run
//!   is removed under its `run.lock` (refused `active` while an invocation holds it, `changed`
//!   when the folder no longer holds what was selected): its run index entry goes, it is renamed
//!   to a `.removing-<16 hex>` tombstone beside it (renamed back, `changed`, when a run started
//!   inside it), emptied (`plan.json` and `events.jsonl` first, `run.lock` last), and the
//!   tombstone removed; then its catalog entries go. A removal that cannot finish is `partial`
//!   and leaves the tombstone, which the listing shows and `runs remove` finishes.
//! - Exit 0 when every selected run was removed (or for a preview), 1 when one was refused or
//!   partial, 2 for usage and RUNs that are not runs.

use super::run_set::{
    Found, Holds, Placement, RunSet, comparable, holds_only_a_claim, is_tombstone_name,
    last_pick_holders,
};
use crate::cli::{RunsArgs, RunsListArgs, RunsRemoveArgs, RunsVerb};
use crate::print::{labelled, print_json, print_line, shown_path};
use grida_fx_core::Error;
use grida_fx_core::docs::project::Project;
use grida_fx_core::money::Usd;
use grida_fx_core::value::sha256_hex;
use grida_fx_runtime::folder::same_file;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions, TryLockError};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The result kinds (spec/schemas).
const LIST_KIND: &str = "fx-run-list-v1";
const REMOVAL_KIND: &str = "fx-run-removal-v1";

/// An empty claim younger than this may be a run that is starting: filters pass it over.
const YOUNG_CLAIM: Duration = Duration::from_secs(60);

/// `grida-fx runs`.
pub fn run(args: &RunsArgs) -> Result<u8, Error> {
    match &args.verb {
        RunsVerb::List(args) => list(args),
        RunsVerb::Remove(args) => remove(args),
    }
}

fn list(args: &RunsListArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let project = Project::find(&cwd)?;
    let set = RunSet::collect(&project);
    let shown: Vec<&Found> = set
        .found
        .iter()
        .filter(|found| {
            args.workflow
                .as_deref()
                .is_none_or(|id| found.workflow() == Some(id))
        })
        .collect();
    if args.json {
        print_json(&json!({
            "kind": LIST_KIND,
            "runs": shown.iter().map(|found| row(found, &cwd)).collect::<Vec<_>>(),
            "skipped": skipped(&set, &cwd),
        }));
        return Ok(0);
    }
    for found in shown {
        print_line(&list_line(found, &cwd));
    }
    for problem in &set.problems {
        eprintln!(
            "{}",
            labelled(
                "skipped",
                &format!("{}: {}", shown_path(&problem.path, &cwd), problem.message)
            )
        );
    }
    Ok(0)
}

/// A run as `fx-run-list-v1` and `fx-run-removal-v1` give it.
fn row(found: &Found, cwd: &Path) -> Value {
    let recorded = found.recorded();
    let mut row = Map::new();
    row.insert("folder".into(), json!(shown_path(&found.folder, cwd)));
    row.insert("workflow".into(), json!(found.workflow()));
    row.insert(
        "source".into(),
        json!(recorded.map(|recorded| recorded.source.as_str())),
    );
    row.insert("name".into(), json!(found.name()));
    row.insert("placement".into(), json!(found.placement.as_str()));
    row.insert("state".into(), json!(found.state()));
    row.insert("created_at".into(), json!(found.created_at()));
    row.insert(
        "charged_usd".into(),
        recorded
            .and_then(|recorded| recorded.charged_usd.clone())
            .unwrap_or(Value::Null),
    );
    row.insert("stand_in".into(), json!(found.stand_in()));
    if let Some(stopped) = recorded.and_then(|recorded| recorded.stopped.as_deref()) {
        row.insert("stopped".into(), json!(stopped));
    }
    Value::Object(row)
}

fn skipped(set: &RunSet, cwd: &Path) -> Vec<Value> {
    set.problems
        .iter()
        .map(|problem| {
            json!({
                "folder": shown_path(&problem.path, cwd),
                "code": problem.kind.code(),
                "message": problem.message,
            })
        })
        .collect()
}

/// How text output names a run: `WORKFLOW_ID/NAME` for a named one, else its folder.
fn label(found: &Found, cwd: &Path) -> String {
    match (found.placement, found.workflow(), found.name()) {
        (Placement::Named, Some(id), Some(name)) => format!("{id}/{name}"),
        _ => shown_path(&found.folder, cwd),
    }
}

fn list_line(found: &Found, cwd: &Path) -> String {
    let created = found
        .created_at()
        .and_then(local_minute)
        .unwrap_or_else(|| "-".repeat(16));
    let mut line = format!("{created}  {:<10}  {}", found.state(), label(found, cwd));
    if let Some(spent) = found
        .recorded()
        .and_then(|recorded| recorded.charged_usd.as_ref())
        .and_then(|charged| Usd::from_value(charged).ok())
    {
        line.push_str(&format!("  {}", spent.dollars_2()));
    }
    if found.stand_in() {
        line.push_str("  stand-in");
    }
    line
}

/// `YYYY-MM-DD HH:MM` in local time.
fn local_minute(created_at: &str) -> Option<String> {
    let time = chrono::DateTime::parse_from_rfc3339(created_at).ok()?;
    Some(
        time.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
    )
}

/// A RUN that is not one, or a usage problem: printed and exit 2, before anything is removed.
struct Invalid {
    code: &'static str,
    message: String,
    run: Option<String>,
}

impl Invalid {
    fn new(code: &'static str, run: &str, message: impl Into<String>) -> Invalid {
        Invalid {
            code,
            message: message.into(),
            run: Some(run.to_string()),
        }
    }
}

/// What is selected.
#[derive(Debug, Clone)]
enum Selected {
    /// A folder the run set found.
    Found(usize),
    /// A missing folder the run index or the catalog still names: only forgotten.
    Forget { folder: PathBuf, given: String },
}

/// How one selection ended.
struct Outcome {
    selected: Selected,
    outcome: &'static str,
    code: Option<&'static str>,
    message: Option<String>,
    freed: u64,
    cache: u64,
}

fn remove(args: &RunsRemoveArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let project = Project::find(&cwd)?;
    if args.runs.is_empty()
        && args.workflow.is_none()
        && args.state.is_empty()
        && args.before.is_none()
    {
        return Err(Error::usage(
            "runs remove takes run folders, WORKFLOW_ID/NAME, or --workflow, --state or --before",
        ));
    }
    let before = match &args.before {
        Some(date) => Some(midnight(date).ok_or_else(|| {
            Error::usage(format!("--before takes a date as YYYY-MM-DD, not {date}"))
        })?),
        None => None,
    };
    // One removal at a time selects and removes, so none decides against another's selection.
    let _removals = if args.yes {
        Some(
            grida_fx_runtime::run_index::removals(&project.root)
                .map_err(|error| Error::io(".fx/runs/lock", &error))?,
        )
    } else {
        None
    };
    let set = RunSet::collect(&project);
    let mut skipped = BTreeMap::new();
    let selection = if args.runs.is_empty() {
        Ok(filtered(&set, args, before, &mut skipped))
    } else {
        resolved(&set, &project, &cwd, &args.runs)
    };
    let selection = match selection {
        Ok(selection) => selection,
        Err(invalid) => return Ok(refuse_invalid(&invalid, args.json)),
    };
    // Picks: the last kept holder of each stays (spec/store.md §9, "Picks").
    let picks = set.picks(&project);
    let chosen: BTreeSet<usize> = selection
        .iter()
        .filter_map(|selected| match selected {
            Selected::Found(index) => Some(*index),
            Selected::Forget { .. } => None,
        })
        .collect();
    let holders = last_pick_holders(&picks, &chosen, &set);
    let mut outcomes = Vec::new();
    for selected in selection {
        if let Selected::Found(index) = &selected
            && let Some(held) = holders.get(index)
        {
            if args.runs.is_empty() {
                *skipped.entry("holds_pick").or_insert(0) += 1;
                continue;
            }
            let (file, step) = &held[0];
            let message = if step.is_empty() {
                format!(
                    "its takes file {file} cannot be read, so it may hold a pick; fix the file or \
                     keep the run"
                )
            } else {
                format!(
                    "it is the last run holding the pick of {step} in {file}; pick again or keep it"
                )
            };
            outcomes.push(Outcome {
                selected,
                outcome: "refused",
                code: Some("holds_pick"),
                message: Some(message),
                freed: 0,
                cache: 0,
            });
            continue;
        }
        outcomes.push(if args.yes {
            remove_one(&set, &project, selected)
        } else {
            preview_one(&set, selected)
        });
    }
    report(&set, &cwd, &outcomes, &skipped, args.yes, args.json);
    let failed = outcomes
        .iter()
        .any(|outcome| matches!(outcome.outcome, "refused" | "partial"));
    Ok(u8::from(failed))
}

/// The runs the filters select (module doc), counting what they passed over in `skipped`.
fn filtered(
    set: &RunSet,
    args: &RunsRemoveArgs,
    before: Option<chrono::DateTime<chrono::Utc>>,
    skipped: &mut BTreeMap<&'static str, usize>,
) -> Vec<Selected> {
    let now = SystemTime::now();
    let mut selected = Vec::new();
    for (index, found) in set.found.iter().enumerate() {
        let matches = args
            .workflow
            .as_deref()
            .is_none_or(|id| found.workflow() == Some(id))
            && (args.state.is_empty() || args.state.iter().any(|state| state == found.state()))
            && before.is_none_or(|before| created(found).is_some_and(|created| created < before));
        if !matches {
            continue;
        }
        match found.placement {
            Placement::Allocated => {}
            Placement::Named => {
                *skipped.entry("named").or_insert(0) += 1;
                continue;
            }
            Placement::Explicit => {
                *skipped.entry("explicit").or_insert(0) += 1;
                continue;
            }
            Placement::External => {
                *skipped.entry("external").or_insert(0) += 1;
                continue;
            }
        }
        let young = matches!(found.holds, Holds::Empty)
            && modified(&found.folder)
                .is_some_and(|time| now.duration_since(time).unwrap_or_default() < YOUNG_CLAIM);
        if young {
            *skipped.entry("young").or_insert(0) += 1;
            continue;
        }
        if holds_other_runs(set, index) {
            *skipped.entry("holds_runs").or_insert(0) += 1;
            continue;
        }
        selected.push(Selected::Found(index));
    }
    selected
}

/// When a run was made: its creation time, else (a leftover) its folder's modification time.
fn created(found: &Found) -> Option<chrono::DateTime<chrono::Utc>> {
    match found.created_at() {
        Some(created_at) => chrono::DateTime::parse_from_rfc3339(created_at)
            .ok()
            .map(|time| time.with_timezone(&chrono::Utc)),
        None => modified(&found.folder).map(chrono::DateTime::<chrono::Utc>::from),
    }
}

fn modified(folder: &Path) -> Option<SystemTime> {
    fs::symlink_metadata(folder)
        .and_then(|metadata| metadata.modified())
        .ok()
}

/// Local midnight at the start of `YYYY-MM-DD`.
fn midnight(date: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::TimeZone;
    let day = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    if day.format("%Y-%m-%d").to_string() != date {
        return None;
    }
    // The first moment of the day: midnight, or the end of a daylight-saving gap at midnight.
    let local = (0..=2).find_map(|hour| {
        chrono::Local
            .from_local_datetime(&day.and_hms_opt(hour, 0, 0)?)
            .earliest()
    })?;
    Some(local.with_timezone(&chrono::Utc))
}

/// Whether another found folder lies inside this one.
fn holds_other_runs(set: &RunSet, index: usize) -> bool {
    let folder = &set.found[index].folder;
    set.found.iter().enumerate().any(|(other, found)| {
        other != index && !matches!(found.holds, Holds::Missing) && found.folder.starts_with(folder)
    })
}

/// The RUNs, each resolved and checked (module doc).
fn resolved(
    set: &RunSet,
    project: &Project,
    cwd: &Path,
    runs: &[String],
) -> Result<Vec<Selected>, Invalid> {
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    for given in runs {
        let one = resolve_one(set, project, cwd, given)?;
        let key = match &one {
            Selected::Found(index) => set.found[*index].folder.clone(),
            Selected::Forget { folder, .. } => folder.clone(),
        };
        if seen.insert(key) {
            selected.push(one);
        }
    }
    Ok(selected)
}

fn resolve_one(
    set: &RunSet,
    project: &Project,
    cwd: &Path,
    given: &str,
) -> Result<Selected, Invalid> {
    if given.is_empty() {
        return Err(Invalid::new(
            "invalid_target",
            given,
            "an empty RUN selects nothing",
        ));
    }
    let path = cwd.join(given);
    let parts: Vec<_> = Path::new(given)
        .components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect();
    let named = match parts.as_slice() {
        [Component::Normal(id), Component::Normal(name)] if !given.starts_with("./") => id
            .to_str()
            .zip(name.to_str())
            .filter(|(_, name)| super::run_catalog::validate_name(name).is_ok()),
        _ => None,
    };
    let by_name = named.map(|(id, name)| {
        set.found
            .iter()
            .enumerate()
            .filter(|(_, found)| {
                found.placement == Placement::Named
                    && found.workflow() == Some(id)
                    && found.name() == Some(name)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
    });
    let metadata = fs::symlink_metadata(&path);
    if let Some(matches) = &by_name {
        if matches.len() > 1 {
            return Err(Invalid::new(
                "ambiguous_target",
                given,
                format!("{given} names runs of several workflow sources; give a run folder"),
            ));
        }
        if let [index] = matches.as_slice() {
            if metadata.is_ok()
                && path.canonicalize().ok().as_ref() != Some(&set.found[*index].folder)
            {
                return Err(Invalid::new(
                    "ambiguous_target",
                    given,
                    format!(
                        "{given} names both a folder and a named run; give the run folder as \
                         ./{given}"
                    ),
                ));
            }
            return checked(set, project, *index, given);
        }
    }
    let metadata = match metadata {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let folder = comparable(&path);
            let known = set
                .found
                .iter()
                .position(|found| matches!(found.holds, Holds::Missing) && found.folder == folder);
            return match known {
                Some(index) => Ok(Selected::Found(index)),
                None if registered_somewhere(project, &folder) => Ok(Selected::Forget {
                    folder,
                    given: given.to_string(),
                }),
                None if named.is_some() => Err(Invalid::new(
                    "invalid_target",
                    given,
                    format!("there is no run {given}"),
                )),
                None => Err(Invalid::new(
                    "invalid_target",
                    given,
                    format!("there is no run folder {given}"),
                )),
            };
        }
        Err(error) => {
            return Err(Invalid::new(
                "invalid_target",
                given,
                format!("{given}: {error}"),
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Invalid::new(
            "not_a_run",
            given,
            format!("{given} is not a run folder"),
        ));
    }
    let folder = path
        .canonicalize()
        .map_err(|error| Invalid::new("invalid_target", given, format!("{given}: {error}")))?;
    let Some(index) = set.at(&folder) else {
        if holds_a_kept_folder(project, &folder) {
            return Err(Invalid::new(
                "not_a_run",
                given,
                format!(
                    "{given} holds the project, its runs, its cache or .fx; it is not a run folder"
                ),
            ));
        }
        return Err(Invalid::new(
            "not_a_run",
            given,
            format!("{given} is not a run folder"),
        ));
    };
    checked(set, project, index, given)
}

/// Whether `folder` is or holds the project root, its runs folder, its store or `.fx`.
fn holds_a_kept_folder(project: &Project, folder: &Path) -> bool {
    let protected = [
        Some(project.root.clone()),
        project.runs_dir().canonicalize().ok(),
        project.cache_dir().canonicalize().ok(),
        project.root.join(".fx").canonicalize().ok(),
    ];
    protected
        .iter()
        .flatten()
        .any(|kept| kept.starts_with(folder))
}

/// The run at `index`, once checked as every selected run is (module doc).
fn checked(
    set: &RunSet,
    project: &Project,
    index: usize,
    given: &str,
) -> Result<Selected, Invalid> {
    if holds_a_kept_folder(project, &set.found[index].folder) {
        return Err(Invalid::new(
            "not_a_run",
            given,
            format!(
                "{given} holds the project, its runs, its cache or .fx; it is not a run folder"
            ),
        ));
    }
    if holds_other_runs(set, index) {
        return Err(Invalid::new(
            "not_a_run",
            given,
            format!("{given} holds other runs; remove them first"),
        ));
    }
    Ok(Selected::Found(index))
}

/// Whether the catalog still names `folder` (inside the tree, where the walk does not list it).
fn registered_somewhere(project: &Project, folder: &Path) -> bool {
    grida_fx_viewer::service::registered_runs(&project.root)
        .is_ok_and(|(runs, _)| runs.iter().any(|run| run.root == folder))
        || grida_fx_runtime::run_index::entries(&project.root)
            .is_ok_and(|folders| folders.iter().any(|indexed| indexed == folder))
}

fn refuse_invalid(invalid: &Invalid, json: bool) -> u8 {
    if json {
        print_json(&json!({
            "kind": REMOVAL_KIND,
            "applied": false,
            "error": {
                "code": invalid.code,
                "message": invalid.message,
                "run": invalid.run,
            },
        }));
    } else {
        crate::print::print_error(&Error::usage(invalid.message.clone()));
    }
    2
}

/// Without `--yes`: what would happen, probing the lock without taking it for longer than a look.
fn preview_one(set: &RunSet, selected: Selected) -> Outcome {
    let (freed, cache) = match &selected {
        Selected::Found(index) => bytes(&set.found[*index].folder),
        Selected::Forget { .. } => (0, 0),
    };
    let mut outcome = Outcome {
        selected,
        outcome: "would_remove",
        code: None,
        message: None,
        freed,
        cache,
    };
    if let Selected::Found(index) = &outcome.selected {
        let found = &set.found[*index];
        if matches!(found.holds, Holds::Missing) {
            outcome.outcome = "would_forget";
        } else if let Some(Err(TryLockError::WouldBlock)) = OpenOptions::new()
            .read(true)
            .write(true)
            .open(found.folder.join("run.lock"))
            .ok()
            .map(|file| file.try_lock())
        {
            outcome.outcome = "refused";
            outcome.code = Some("active");
            outcome.message = Some("an invocation is running it".into());
        } else if let Some(name) = foreign_entry(found) {
            outcome.outcome = "refused";
            outcome.code = Some("not_removable");
            outcome.message = Some(foreign_message(&name));
        }
    } else {
        outcome.outcome = "would_forget";
    }
    if outcome.outcome == "refused" {
        (outcome.freed, outcome.cache) = (0, 0);
    }
    outcome
}

/// The first top-level entry of a run folder or claim that FX did not write: anything but its
/// `plan.json`, `events.jsonl`, `run.lock`, `files/`, `outputs/`, `views/`, temporary names,
/// `.DS_Store` and folders holding no file. A tombstone's leftovers are all its own.
fn foreign_entry(found: &Found) -> Option<String> {
    if matches!(found.holds, Holds::Tombstone | Holds::Missing) {
        return None;
    }
    let listing = fs::read_dir(&found.folder).ok()?;
    for item in listing.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let ours = matches!(
            name.as_str(),
            "plan.json" | "events.jsonl" | "run.lock" | "files" | "outputs" | "views" | ".DS_Store"
        ) || grida_fx_runtime::folder::is_temporary_name(&name);
        let empty_folder =
            item.file_type().is_ok_and(|kind| kind.is_dir()) && !holds_a_file(&item.path());
        if !ours && !empty_folder {
            return Some(name);
        }
    }
    None
}

/// Whether a folder holds a file anywhere below it (a symbolic link counts).
fn holds_a_file(folder: &Path) -> bool {
    let mut pending = vec![folder.to_path_buf()];
    while let Some(here) = pending.pop() {
        let Ok(listing) = fs::read_dir(&here) else {
            return true;
        };
        for item in listing.flatten() {
            match item.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(item.path()),
                _ => return true,
            }
        }
    }
    false
}

fn foreign_message(name: &str) -> String {
    format!("it holds {name}, which FX did not write; move it out first")
}

/// With `--yes`: removes one run (spec/store.md §8, "Removing a run").
fn remove_one(set: &RunSet, project: &Project, selected: Selected) -> Outcome {
    let mut outcome = Outcome {
        selected: selected.clone(),
        outcome: "removed",
        code: None,
        message: None,
        freed: 0,
        cache: 0,
    };
    let found = match &selected {
        Selected::Forget { folder, .. } => {
            let _ = grida_fx_runtime::run_index::forget(&project.root, folder);
            forget(project, folder, None, None);
            outcome.outcome = "forgotten";
            return outcome;
        }
        Selected::Found(index) => &set.found[*index],
    };
    if matches!(found.holds, Holds::Missing) {
        let _ = grida_fx_runtime::run_index::forget(&project.root, &found.folder);
        forget(project, &found.folder, None, None);
        outcome.outcome = "forgotten";
        return outcome;
    }
    let refused = |outcome: &mut Outcome, code: &'static str, message: String| {
        outcome.outcome = "refused";
        outcome.code = Some(code);
        outcome.message = Some(message);
        (outcome.freed, outcome.cache) = (0, 0);
    };
    let folder = &found.folder;
    let lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(folder.join("run.lock"))
    {
        Ok(lock) => lock,
        Err(error) => {
            refused(&mut outcome, "not_removable", format!("run.lock: {error}"));
            return outcome;
        }
    };
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            refused(&mut outcome, "active", "an invocation is running it".into());
            return outcome;
        }
        Err(TryLockError::Error(error)) => {
            refused(&mut outcome, "not_removable", format!("run.lock: {error}"));
            return outcome;
        }
    }
    // Under its lock, the folder must still hold what was selected, and only what FX wrote.
    if !same_file(&lock, &folder.join("run.lock")) || !still_selected(found) {
        refused(
            &mut outcome,
            "changed",
            "it changed since it was selected; list the runs again".into(),
        );
        return outcome;
    }
    if let Some(name) = foreign_entry(found) {
        refused(&mut outcome, "not_removable", foreign_message(&name));
        return outcome;
    }
    let identity = grida_fx_viewer::service::run_identity(folder).ok();
    (outcome.freed, outcome.cache) = bytes(folder);
    // Forgotten before the folder goes, so a run that starts at its place meanwhile records
    // itself again; recorded again if the folder stays.
    let indexed = grida_fx_runtime::run_index::forget(&project.root, folder).unwrap_or(false);
    let restore = || {
        if indexed {
            let _ = grida_fx_runtime::run_index::RunIndex {
                project_root: project.root.clone(),
            }
            .record(folder);
        }
    };
    let tombstone = if matches!(found.holds, Holds::Tombstone) {
        if runs_inside(folder) {
            restore();
            refused(
                &mut outcome,
                "changed",
                "a run started inside it; list the runs again".into(),
            );
            return outcome;
        }
        folder.clone()
    } else {
        let Some(parent) = folder.parent() else {
            restore();
            refused(
                &mut outcome,
                "not_removable",
                "it has no parent folder".into(),
            );
            return outcome;
        };
        let tombstone = parent.join(tombstone_name());
        if let Err(error) = fs::rename(folder, &tombstone) {
            restore();
            refused(&mut outcome, "not_removable", error.to_string());
            return outcome;
        }
        // A run that started inside it since it was selected goes back with it. When its place
        // was taken meanwhile, it stays a tombstone, and nothing in it is removed.
        if runs_inside(&tombstone) {
            let message = match fs::rename(&tombstone, folder) {
                Ok(()) => {
                    restore();
                    "a run started inside it since it was selected; list the runs again".into()
                }
                Err(error) => {
                    record_tombstone(project, found, &tombstone);
                    format!(
                        "a run started inside it since it was selected, and its place was taken \
                         meanwhile ({error}); it was left as {}",
                        tombstone
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    )
                }
            };
            refused(&mut outcome, "changed", message);
            return outcome;
        }
        tombstone
    };
    let mut problems = empty_out(&tombstone);
    drop(lock);
    if let Err(error) = fs::remove_file(tombstone.join("run.lock"))
        && error.kind() != io::ErrorKind::NotFound
    {
        problems.push(format!("run.lock: {error}"));
    }
    if let Err(error) = fs::remove_dir(&tombstone)
        && error.kind() != io::ErrorKind::NotFound
    {
        problems.push(error.to_string());
    }
    forget(project, folder, identity.as_deref(), Some(&tombstone));
    if fs::symlink_metadata(&tombstone).is_ok() {
        record_tombstone(project, found, &tombstone);
        outcome.outcome = "partial";
        outcome.code = Some("io");
        let first = problems.first().cloned().unwrap_or_default();
        outcome.message = Some(format!(
            "{} was left behind ({first}); run grida-fx runs remove on it to finish",
            tombstone
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
    }
    outcome
}

/// Records a tombstone left outside the runs tree in the run index, so the list shows it and
/// `runs remove` can finish it (the walk finds those inside the tree).
fn record_tombstone(project: &Project, found: &Found, tombstone: &Path) {
    if found.placement == Placement::External {
        let _ = grida_fx_runtime::run_index::RunIndex {
            project_root: project.root.clone(),
        }
        .record(tombstone);
    }
}

/// Whether a found folder still holds what the listing saw: the same recorded run (workflow,
/// source and creation), or still only a claim.
fn still_selected(found: &Found) -> bool {
    match &found.holds {
        Holds::Recorded(recorded) => {
            let plan = fs::read(found.folder.join("plan.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            let Some(plan) = plan else {
                return false;
            };
            let workflow = plan.get("workflow");
            let text = |name: &str| {
                workflow
                    .and_then(|w| w.get(name))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let events =
                grida_fx_runtime::events::read_events_tolerant(&found.folder.join("events.jsonl"))
                    .unwrap_or_default();
            let modified = fs::metadata(found.folder.join("plan.json"))
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let (_, created_at) = grida_fx_runtime::events::run_metadata(&events, modified);
            text("id") == recorded.workflow
                && text("file") == recorded.source
                && created_at == recorded.created_at
        }
        Holds::Empty => holds_only_a_claim(&found.folder),
        Holds::Tombstone => true,
        Holds::Missing => false,
    }
}

/// Whether a run folder's tree holds another run, or a `run.lock` someone holds, below its top:
/// a run that started inside it.
fn runs_inside(folder: &Path) -> bool {
    let mut pending: Vec<PathBuf> = match fs::read_dir(folder) {
        Ok(listing) => listing
            .flatten()
            .filter(|item| {
                let name = item.file_name();
                item.file_type().is_ok_and(|kind| kind.is_dir())
                    && !matches!(name.to_str(), Some("files" | "outputs" | "views"))
            })
            .map(|item| item.path())
            .collect(),
        Err(_) => return false,
    };
    while let Some(here) = pending.pop() {
        if fs::symlink_metadata(here.join("plan.json")).is_ok() {
            return true;
        }
        if let Ok(file) = OpenOptions::new()
            .read(true)
            .write(true)
            .open(here.join("run.lock"))
            && matches!(file.try_lock(), Err(TryLockError::WouldBlock))
        {
            return true;
        }
        if let Ok(listing) = fs::read_dir(&here) {
            pending.extend(
                listing
                    .flatten()
                    .filter(|item| item.file_type().is_ok_and(|kind| kind.is_dir()))
                    .map(|item| item.path()),
            );
        }
    }
    false
}

/// Forgets a removed folder in the catalog: its entries (those binding `identity` when known),
/// and any entry a discovery made of its tombstone meanwhile.
fn forget(project: &Project, folder: &Path, identity: Option<&str>, tombstone: Option<&Path>) {
    let _ = grida_fx_viewer::service::deregister_runs(&project.root, folder, identity);
    if let Some(tombstone) = tombstone {
        let _ = grida_fx_viewer::service::deregister_runs(&project.root, tombstone, None);
    }
}

/// `.removing-<16 hex>`, unique in the process.
fn tombstone_name() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let seed = format!("{}:{n}:{nanos}", std::process::id());
    let name = format!(".removing-{}", &sha256_hex(seed.as_bytes())[..16]);
    debug_assert!(is_tombstone_name(&name));
    name
}

/// Removes everything in `folder` but its `run.lock`: `plan.json` and `events.jsonl` first, so a
/// half-emptied folder never reads as a run. Folders are made writable where needed; files never
/// are (they share their bytes with the store). The problems met.
fn empty_out(folder: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    for name in ["plan.json", "events.jsonl"] {
        if let Err(error) = fs::remove_file(folder.join(name))
            && error.kind() != io::ErrorKind::NotFound
        {
            problems.push(format!("{name}: {error}"));
        }
    }
    let listing = match fs::read_dir(folder) {
        Ok(listing) => listing,
        Err(error) => {
            problems.push(error.to_string());
            return problems;
        }
    };
    for item in listing.flatten() {
        if item.file_name() == "run.lock" {
            continue;
        }
        let path = item.path();
        let removed = match item.file_type() {
            Ok(kind) if kind.is_dir() => remove_folder(&path),
            _ => fs::remove_file(&path),
        };
        if let Err(error) = removed {
            problems.push(format!("{}: {error}", item.file_name().to_string_lossy()));
        }
    }
    problems
}

/// Removes a folder and what is in it, making read-only folders writable on a second try.
fn remove_folder(folder: &Path) -> io::Result<()> {
    if fs::remove_dir_all(folder).is_ok() {
        return Ok(());
    }
    let mut pending = vec![folder.to_path_buf()];
    while let Some(here) = pending.pop() {
        let Ok(metadata) = fs::symlink_metadata(&here) else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            if mode & 0o700 != 0o700 {
                fs::set_permissions(&here, fs::Permissions::from_mode(mode | 0o700))?;
            }
        }
        if let Ok(listing) = fs::read_dir(&here) {
            pending.extend(listing.flatten().map(|item| item.path()));
        }
    }
    fs::remove_dir_all(folder)
}

/// What removing `folder` frees now (files whose every link is inside it) and what stays held
/// elsewhere, by the cache or other runs (files linked from outside it). Each file counts once.
fn bytes(folder: &Path) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mut files: BTreeMap<(u64, u64), (u64, u64, u64)> = BTreeMap::new();
        let mut pending = vec![folder.to_path_buf()];
        while let Some(here) = pending.pop() {
            let Ok(listing) = fs::read_dir(&here) else {
                continue;
            };
            for item in listing.flatten() {
                let Ok(metadata) = fs::symlink_metadata(item.path()) else {
                    continue;
                };
                if metadata.is_dir() {
                    pending.push(item.path());
                } else if metadata.is_file() {
                    let entry = files.entry((metadata.dev(), metadata.ino())).or_insert((
                        metadata.nlink(),
                        0,
                        metadata.len(),
                    ));
                    entry.1 += 1;
                }
            }
        }
        let mut freed = 0;
        let mut held = 0;
        for (links, inside, size) in files.into_values() {
            if links <= inside {
                freed += size;
            } else {
                held += size;
            }
        }
        (freed, held)
    }
    #[cfg(not(unix))]
    {
        let _ = folder;
        (0, 0)
    }
}

fn report(
    set: &RunSet,
    cwd: &Path,
    outcomes: &[Outcome],
    skipped: &BTreeMap<&'static str, usize>,
    applied: bool,
    json: bool,
) {
    let freed: u64 = outcomes.iter().map(|outcome| outcome.freed).sum();
    let cache: u64 = outcomes.iter().map(|outcome| outcome.cache).sum();
    if json {
        let runs: Vec<Value> = outcomes
            .iter()
            .map(|outcome| {
                let mut row = match &outcome.selected {
                    Selected::Found(index) => row(&set.found[*index], cwd),
                    Selected::Forget { folder, .. } => json!({
                        "folder": shown_path(folder, cwd),
                        "workflow": null,
                        "source": null,
                        "name": null,
                        "placement": "external",
                        "state": "missing",
                        "created_at": null,
                        "charged_usd": null,
                        "stand_in": false,
                    }),
                };
                if let Some(row) = row.as_object_mut() {
                    row.insert("outcome".into(), json!(outcome.outcome));
                    if let Some(code) = outcome.code {
                        row.insert("code".into(), json!(code));
                    }
                    if let Some(message) = &outcome.message {
                        row.insert("message".into(), json!(message));
                    }
                    row.insert("freed_bytes".into(), json!(outcome.freed));
                    row.insert("cache_bytes".into(), json!(outcome.cache));
                }
                row
            })
            .collect();
        print_json(&json!({
            "kind": REMOVAL_KIND,
            "applied": applied,
            "runs": runs,
            "skipped": skipped,
            "freed_bytes": freed,
            "cache_bytes": cache,
        }));
        return;
    }
    for outcome in outcomes {
        let name = match &outcome.selected {
            Selected::Found(index) => label(&set.found[*index], cwd),
            Selected::Forget { given, .. } => given.clone(),
        };
        let state = match &outcome.selected {
            Selected::Found(index) => set.found[*index].state(),
            Selected::Forget { .. } => "missing",
        };
        let line = match (outcome.outcome, &outcome.message) {
            ("refused" | "partial", Some(message)) => {
                format!("{:<13} {name}: {message}", outcome.outcome)
            }
            (verb, _) => format!("{:<13} {name}  ({state})", verb.replace('_', " ")),
        };
        print_line(&line);
    }
    let passed: Vec<String> = [
        ("named", "named"),
        ("explicit", "placed with --run"),
        ("external", "outside the runs folder"),
        ("holds_pick", "holding the last copy of a pick"),
        ("young", "just claimed"),
        ("holds_runs", "holding other runs"),
    ]
    .iter()
    .filter_map(|(key, what)| skipped.get(key).map(|count| format!("{count} {what}")))
    .collect();
    if !passed.is_empty() {
        print_line(&labelled(
            "passed",
            &format!(
                "{} (filters select dated runs only; name a run to remove it)",
                passed.join(", ")
            ),
        ));
    }
    if outcomes.is_empty() {
        print_line("nothing to remove");
        return;
    }
    let verb = if applied { "freed" } else { "frees" };
    let shared = if cache > 0 {
        format!(
            "; {} more stays in the cache, which shares it",
            human(cache)
        )
    } else {
        String::new()
    };
    print_line(&labelled(verb, &format!("{}{shared}", human(freed))));
    if !applied {
        print_line(&labelled("next", "add --yes to remove them"));
    }
}

/// Bytes in B, kB, MB or GB (powers of 1000), one decimal above bytes.
pub(super) fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["kB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_dates_read_as_people_write_them() {
        assert_eq!(human(999), "999 B");
        assert_eq!(human(1_500), "1.5 kB");
        assert_eq!(human(2_340_000_000), "2.3 GB");
        assert!(midnight("2026-10-09").is_some());
        for date in ["2026-1-9", "2026-13-01", "yesterday", "2026-10-09T00:00"] {
            assert!(midnight(date).is_none(), "{date}");
        }
    }

    /// A run folder of `case` created at `created_at`.
    fn recorded_run(folder: &Path, created_at: &str) {
        fs::create_dir_all(folder).unwrap();
        fs::write(
            folder.join("plan.json"),
            r#"{"kind": "fx-graph-v1", "workflow": {"id": "case", "file": "case.yaml"}}"#,
        )
        .unwrap();
        fs::write(
            folder.join("events.jsonl"),
            format!("{{\"event\":\"run_started\",\"created_at\":\"{created_at}\"}}\n"),
        )
        .unwrap();
    }

    #[test]
    fn a_folder_that_changed_since_it_was_listed_is_told_apart() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().canonicalize().unwrap();
        fs::write(project.join("fx.yaml"), "fx: project/v1\n").unwrap();
        let run = project.join("runs/case/2026-10-01-1");
        recorded_run(&run, "2026-10-01T00:00:00.000Z");
        let claim = project.join("runs/case/2026-10-02-1");
        fs::create_dir_all(&claim).unwrap();
        let set = RunSet::collect(&Project::find(&project).unwrap());
        let found = |folder: &Path| &set.found[set.at(folder).unwrap()];
        assert!(still_selected(found(&run)) && still_selected(found(&claim)));
        // Another run took the place, and the claim became a run.
        recorded_run(&run, "2026-10-01T00:00:01.000Z");
        recorded_run(&claim, "2026-10-02T00:00:00.000Z");
        assert!(!still_selected(found(&run)) && !still_selected(found(&claim)));
    }

    #[test]
    fn a_run_started_inside_a_folder_is_found_below_its_top() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("runs/case");
        recorded_run(&folder, "2026-10-01T00:00:00.000Z");
        fs::create_dir_all(folder.join("files/draw")).unwrap();
        assert!(!runs_inside(&folder));
        // Placed files never hold a run; a folder beside them can.
        fs::write(folder.join("files/draw/plan.json"), "{}").unwrap();
        assert!(!runs_inside(&folder));
        let inner = folder.join("2026-10-01-1");
        fs::create_dir_all(&inner).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(inner.join("run.lock"))
            .unwrap();
        lock.try_lock().unwrap();
        assert!(runs_inside(&folder), "a held run.lock is a run starting");
        drop(lock);
        assert!(!runs_inside(&folder));
        recorded_run(&inner, "2026-10-01T00:00:00.000Z");
        assert!(runs_inside(&folder));
    }
}
