//! Resolving a run and its numbered attempts in the planning project.

use crate::print::{print_json, print_line, shown_path};
use grida_fx_core::{Error, docs::project::Project, value::is_digest};
use grida_fx_runtime::{
    attempts::{self, Allocation, Attempt},
    run_index,
};
use serde_json::json;
use std::path::{Path, PathBuf};

pub fn request_id(text: &str) -> Result<String, String> {
    if text.is_empty()
        || text.len() > 128
        || !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("use 1–128 ASCII letters, digits, underscores or hyphens".into());
    }
    Ok(text.to_owned())
}

pub(super) fn recorded(folder: &Path) -> Result<Option<Attempt>, Error> {
    let folder = folder
        .canonicalize()
        .map_err(|error| Error::io("run folder", &error))?;
    attempts::read(&folder).map_err(|error| Error::io("run history", &error))
}

fn project_of(folder: &Path, cwd: &Path) -> Result<Option<PathBuf>, Error> {
    let here = Project::find(cwd)?.root;
    Ok(run_index::recording_project(folder, &here))
}

pub(super) fn journal(project: &Path, run_id: &str) -> Result<Vec<Allocation>, Error> {
    attempts::allocations(project, run_id).map_err(|e| Error::io("run history", &e))
}

fn records(
    folder: &Path,
    cwd: &Path,
    number: Option<u64>,
) -> Result<(String, Vec<(u64, PathBuf)>), Error> {
    let folder = folder
        .canonicalize()
        .map_err(|error| Error::io("run folder", &error))?;
    let selected =
        recorded(&folder)?.ok_or_else(|| Error::usage("the run has no recorded attempt"))?;
    let run_id = selected.run_id;
    let mut paths = Vec::new();
    if number.is_none_or(|number| number == selected.attempt_number) {
        paths.push((selected.attempt_number, folder.clone()));
        if number.is_some() {
            return Ok((run_id, paths));
        }
    }
    if let Some(project) = project_of(&folder, cwd)? {
        for entry in journal(&project, &run_id)? {
            if number.is_some_and(|number| number != entry.attempt_number) {
                continue;
            }
            if let Some(path) = allocated_record(&project, &run_id, &entry)? {
                let row = (entry.attempt_number, path);
                if !paths.contains(&row) {
                    paths.push(row);
                }
            }
        }
        for path in indexed(&project)? {
            if let Ok(path) = path.canonicalize()
                && let Ok(Some(identity)) = recorded(&path)
                && identity.run_id == run_id
                && number.is_none_or(|number| number == identity.attempt_number)
                && !paths.contains(&(identity.attempt_number, path.clone()))
            {
                paths.push((identity.attempt_number, path));
            }
        }
    }
    paths.sort_by_key(|(number, _)| *number);
    if paths.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(Error::usage(
            "multiple folders claim the same attempt; use an exact folder",
        ));
    }
    Ok((run_id, paths))
}

fn allocated_record(
    project: &Path,
    run_id: &str,
    entry: &Allocation,
) -> Result<Option<PathBuf>, Error> {
    let Some(path) = entry
        .existing_folder(project)
        .map_err(|error| Error::io("run history", &error))?
    else {
        return Ok(None);
    };
    match recorded(&path)? {
        Some(attempt)
            if attempt.run_id == run_id && attempt.attempt_number == entry.attempt_number =>
        {
            Ok(Some(path))
        }
        None if !entry.started => Ok(None),
        _ => Err(Error::usage(format!(
            "attempt {} no longer matches its recorded start",
            entry.attempt_number
        ))),
    }
}

fn indexed(project: &Path) -> Result<Vec<PathBuf>, Error> {
    run_index::entries(project).map_err(|e| Error::io("run index", &e))
}

pub(super) fn anchor_by_id(run_id: &str, cwd: &Path) -> Result<Option<PathBuf>, Error> {
    if !is_digest(run_id) {
        return Ok(None);
    }
    let project = Project::find(cwd)?.root;
    let mut matched = std::collections::BTreeMap::new();
    for folder in indexed(&project)? {
        if let Ok(Some(attempt)) = recorded(&folder)
            && attempt.run_id == run_id
            && matched.insert(attempt.attempt_number, folder).is_some()
        {
            return Err(Error::usage(
                "multiple folders claim the same attempt; use an exact folder",
            ));
        }
    }
    if let Some((_, anchor)) = matched.into_iter().next() {
        return Ok(Some(anchor));
    }
    // If every indexed record is gone or unreadable, check known allocations
    // before interpreting the caller's run ID as another kind of selector.
    for entry in journal(&project, run_id)? {
        if let Some(path) = allocated_record(&project, run_id, &entry)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

pub(super) fn by_id(run_id: &str, cwd: &Path) -> Result<Option<PathBuf>, Error> {
    anchor_by_id(run_id, cwd)?
        .map(|folder| select(folder, cwd, None))
        .transpose()
}

/// Resolve latest for a run-level selector or a particular attempt by number.
pub(super) fn select(folder: PathBuf, cwd: &Path, number: Option<u64>) -> Result<PathBuf, Error> {
    if number.is_none() && (!folder.is_dir() || recorded(&folder)?.is_none()) {
        return Ok(folder);
    }
    let (_, mut paths) = records(&folder, cwd, number)?;
    match number {
        Some(number) => paths
            .into_iter()
            .find(|(n, _)| *n == number)
            .map(|(_, path)| path)
            .ok_or_else(|| Error::usage(format!("attempt {number} is unavailable"))),
        None => Ok(paths.pop().expect("the source is present").1),
    }
}

pub(super) fn print(folder: &Path, cwd: &Path, json_output: bool) -> Result<u8, Error> {
    let (run_id, paths) = records(folder, cwd, None)?;
    if json_output {
        let rows: Vec<_> = paths
            .iter()
            .map(|(attempt_number, folder)| {
                let folder = folder
                    .to_str()
                    .ok_or_else(|| Error::usage("the attempt folder is not UTF-8"))?;
                Ok(json!({ "attempt_number": attempt_number, "folder": folder }))
            })
            .collect::<Result<_, Error>>()?;
        print_json(&json!({ "kind": "fx-run-history-v1", "run_id": run_id, "attempts": rows }));
    } else {
        print_line(&format!("run ID    {run_id}"));
        for (number, folder) in paths {
            print_line(&format!("attempt {number}    {}", folder.display()));
        }
    }
    Ok(0)
}

pub(super) fn pick(cwd: &Path) -> Result<String, Error> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Err(Error::usage(
            "supply a run ID, run name, or exact run folder",
        ));
    }
    let project = Project::find(cwd)?;
    let mut latest = std::collections::BTreeMap::new();
    for folder in indexed(&project.root)? {
        if let Ok(Some(attempt)) = recorded(&folder) {
            let entry = latest
                .entry(attempt.run_id)
                .or_insert_with(|| (0, PathBuf::new()));
            if entry.0 < attempt.attempt_number {
                *entry = (attempt.attempt_number, folder);
            }
        }
    }
    let mut folders: Vec<_> = latest.into_values().map(|(_, folder)| folder).collect();
    folders.sort_by_cached_key(|path| {
        std::cmp::Reverse(
            std::fs::metadata(path.join("plan.json"))
                .and_then(|meta| meta.modified())
                .ok(),
        )
    });
    let folders: Vec<_> = folders
        .into_iter()
        .filter(|folder| {
            let active = std::fs::File::open(folder.join("run.lock")).is_ok_and(|file| {
                matches!(
                    file.try_lock_shared(),
                    Err(std::fs::TryLockError::WouldBlock)
                )
            });
            !active
                && super::read_log(folder, "saved run", true).is_ok_and(|events| {
                    events
                        .iter()
                        .rfind(|event| event["event"] == "run_started")
                        .is_some_and(|event| {
                            !event["stand_in"].as_bool().unwrap_or(false)
                                && super::repeat::Repeat::parse(&event["repeat"]).is_some()
                        })
                })
        })
        .take(10)
        .collect();
    if folders.is_empty() {
        return Err(Error::usage(
            "there are no inactive runs with saved execution context here",
        ));
    }
    let mut output = std::io::stderr().lock();
    for (index, path) in folders.iter().enumerate() {
        writeln!(output, "  {}  {}", index + 1, shown_path(path, cwd))
            .map_err(|e| Error::io("run picker", &e))?;
    }
    write!(output, "Rerun which run? ")
        .and_then(|_| output.flush())
        .map_err(|e| Error::io("run picker", &e))?;
    let mut choice = String::new();
    std::io::stdin()
        .read_line(&mut choice)
        .map_err(|e| Error::io("run picker", &e))?;
    let chosen = choice
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|number| number.checked_sub(1))
        .and_then(|index| folders.get(index))
        .ok_or_else(|| Error::usage("select a run by its number"))?;
    chosen
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::usage("the selected run folder is not UTF-8"))
}

/// An accepted request is a reservation with a matching durable start event.
pub(super) fn accepted(
    project: &Path,
    run_id: &str,
    request_digest: Option<&str>,
    explicit_source: Option<u64>,
) -> Result<Option<PathBuf>, Error> {
    let Some(key) = request_digest else {
        return Ok(None);
    };
    let entries = journal(project, run_id)?;
    let Some(entry) = entries
        .iter()
        .find(|entry| entry.request_digest.as_deref() == Some(key))
    else {
        return Ok(None);
    };
    if explicit_source.is_some_and(|source| source != entry.source_attempt) {
        return Err(Error::usage(format!(
            "this request ID belongs to a rerun from attempt {}",
            entry.source_attempt
        )));
    }
    if entry
        .existing_folder(project)
        .map_err(|error| Error::io("run history", &error))?
        .is_none()
    {
        return Err(Error::usage("the previously allocated attempt was removed"));
    }
    let path = allocated_record(project, run_id, entry)?;
    if let Some(path) = &path
        && !entry.started
    {
        let identity = Attempt {
            run_id: run_id.into(),
            attempt_number: entry.attempt_number,
        };
        attempts::confirm_start(project, &identity, path)
            .map_err(|error| Error::io("run history", &error))?;
    }
    Ok(path)
}
