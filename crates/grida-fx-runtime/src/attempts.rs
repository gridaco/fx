//! Project-local run history. The record owns its identity. The private allocation
//! journal retains numbers and request keys across interruptions and removals.

use crate::run_index::{self, INDEX};
use grida_fx_core::value::{digest, is_digest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

/// Stable address for one recorded execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub run_id: String,
    pub attempt_number: u64,
}

/// Allocates the identity of an independent run from its initial invocation.
pub fn first(invocation: &str) -> Attempt {
    Attempt {
        run_id: digest(&json!(["fx-run-v1", invocation])),
        attempt_number: 1,
    }
}

/// Older records already have a unique initial invocation, or, for earlier
/// records, an identity from the folder and initial record.
pub fn from_start(event: &Value, folder: &Path) -> Attempt {
    if let (Some(id), Some(number)) = (
        event["run_id"].as_str().filter(|id| is_digest(id)),
        event["attempt_number"]
            .as_u64()
            .filter(|number| *number > 0),
    ) {
        Attempt {
            run_id: id.to_owned(),
            attempt_number: number,
        }
    } else if let Some(invocation) = event["invocation_id"].as_str() {
        first(invocation)
    } else {
        let folder_digest = grida_fx_core::value::sha256_hex(folder.as_os_str().as_encoded_bytes());
        Attempt {
            run_id: digest(&json!(["fx-legacy-run-v1", folder_digest, event])),
            attempt_number: 1,
        }
    }
}

/// Reads just the initial event. Absent or unfinished starts yield no record.
pub fn read(folder: &Path) -> io::Result<Option<Attempt>> {
    let path = folder.join("events.jsonl");
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(io::Error::other("events.jsonl is not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut reader = BufReader::new(File::open(path)?.take(16 * 1024 * 1024));
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line)?;
    if line.last() != Some(&b'\n') {
        return Ok(None);
    }
    let event: Value = serde_json::from_slice(&line)?;
    if event["event"] != "run_started" {
        return Ok(None);
    }
    Ok(Some(from_start(&event, folder)))
}

/// An allocation survives alongside run removal, so numbers and request keys
/// once assigned never allocate an additional attempt.
#[derive(Debug, Serialize, Deserialize)]
pub struct Allocation {
    pub attempt_number: u64,
    pub source_attempt: u64,
    pub folder: String,
    reservation: String,
    #[serde(default)]
    pub started: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_digest: Option<String>,
}

/// Reads the private allocation journal without changing the project.
pub fn allocations(project: &Path, run_id: &str) -> io::Result<Vec<Allocation>> {
    let path = journal(project, run_id)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(io::Error::other("invalid run history journal")),
    }
    let records: Vec<Allocation> = serde_json::from_slice(&fs::read(path)?)?;
    let mut previous = 1;
    for entry in &records {
        if entry.attempt_number <= previous
            || entry.source_attempt == 0
            || entry.source_attempt >= entry.attempt_number
            || !run_index::well_formed(&entry.folder)
            || entry.folder.is_empty()
            || entry.folder == "."
            || !is_digest(&entry.reservation)
            || entry
                .request_digest
                .as_ref()
                .is_some_and(|digest| !is_digest(digest))
        {
            return Err(io::Error::other("invalid run history allocation"));
        }
        previous = entry.attempt_number;
    }
    Ok(records)
}

/// Publishes allocations under the caller's project run-index lock.
pub fn save(project: &Path, run_id: &str, records: &[Allocation]) -> io::Result<()> {
    let path = journal(project, run_id)?;
    let parent = path.parent().expect("journal has a parent");
    run_index::make_private(parent)?;
    let temporary = parent.join(crate::folder::temporary_name());
    let written = (|| {
        let mut file = run_index::private_new_file(&temporary)?;
        serde_json::to_writer(&mut file, records)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        sync_directory(parent)?;
        sync_directory(&project.join(INDEX))
    })();
    let _ = fs::remove_file(temporary);
    written
}

impl Allocation {
    /// Claim a folder before publishing its allocation in the project journal.
    ///
    /// An abandoned claim can be reused if it holds only the marker and
    /// temporary files. Once journaled, the marker must match on every retry.
    pub fn reserve(
        project: &Path,
        folder: &Path,
        attempt_number: u64,
        source_attempt: u64,
        request_digest: Option<String>,
    ) -> io::Result<Self> {
        let parent = folder
            .parent()
            .ok_or_else(|| io::Error::other("invalid attempt folder"))?;
        fs::create_dir_all(parent)?;
        match fs::create_dir(folder) {
            Ok(()) => {}
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists
                    && fs::symlink_metadata(folder)?.is_dir() => {}
            Err(error) => return Err(error),
        }
        let folder = folder.canonicalize()?;
        for entry in fs::read_dir(&folder)? {
            let name = entry?.file_name();
            if name != "run.lock" && !crate::folder::is_temporary_name(&name.to_string_lossy()) {
                return Err(io::Error::other(
                    "the attempt folder already contains files",
                ));
            }
        }
        let reservation = match reservation_marker(&folder)? {
            Some(reservation) => reservation,
            None => {
                let reservation = digest(&json!([
                    "fx-rerun-reservation-v1",
                    crate::events::new_invocation_id()
                ]));
                write_reservation_marker(&folder, &reservation)?;
                reservation
            }
        };
        sync_directory(&folder)?;
        sync_directory(folder.parent().expect("the attempt folder has a parent"))?;
        Ok(Self {
            attempt_number,
            source_attempt,
            folder: run_index::recorded_form(project, &folder)?,
            reservation,
            started: false,
            request_digest,
        })
    }

    pub fn path(&self, project: &Path) -> PathBuf {
        run_index::resolve(project, &self.folder)
    }

    /// A reservation path is written canonically. A replacement must never
    /// acquire the reserved attempt's identity or receive another dispatch.
    pub fn existing_folder(&self, project: &Path) -> io::Result<Option<PathBuf>> {
        let path = self.path(project);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() && path.canonicalize()? == path => {
                if reservation_marker(&path)?.as_deref() != Some(self.reservation.as_str()) {
                    return Err(io::Error::other(
                        "the allocated attempt folder was replaced",
                    ));
                }
                Ok(Some(path))
            }
            Ok(_) => Err(io::Error::other(
                "the allocated attempt folder was replaced",
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

const MARKER_PREFIX: &str = "fx-rerun-reservation-v1 ";

fn write_reservation_marker(folder: &Path, reservation: &str) -> io::Result<()> {
    let temporary = folder.join(crate::folder::temporary_name());
    let mut file = run_index::private_new_file(&temporary)?;
    let written = (|| {
        writeln!(file, "{MARKER_PREFIX}{reservation}")?;
        file.sync_all()?;
        fs::hard_link(&temporary, folder.join("run.lock"))
    })();
    drop(file);
    let _ = fs::remove_file(temporary);
    written
}

fn reservation_marker(folder: &Path) -> io::Result<Option<String>> {
    let path = folder.join("run.lock");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let malformed = || io::Error::other("the attempt folder has an invalid reservation marker");
    let length = (MARKER_PREFIX.len() + 65) as u64;
    if !metadata.is_file() || metadata.len() != length {
        return Err(malformed());
    }
    let mut marker = String::new();
    File::open(path)?
        .take(length + 1)
        .read_to_string(&mut marker)?;
    marker
        .strip_prefix(MARKER_PREFIX)
        .and_then(|value| value.strip_suffix('\n'))
        .filter(|value| is_digest(value))
        .map(|value| Some(value.to_owned()))
        .ok_or_else(malformed)
}

pub(crate) fn verify_reservation(
    project: &Path,
    attempt: &Attempt,
    folder: &Path,
) -> io::Result<()> {
    let entries = allocations(project, &attempt.run_id)?;
    let entry = entries
        .iter()
        .find(|entry| entry.attempt_number == attempt.attempt_number)
        .ok_or_else(|| io::Error::other("the attempt has no allocation"))?;
    if entry.existing_folder(project)?.as_deref() != Some(folder) {
        return Err(io::Error::other(
            "the allocated attempt folder was replaced or removed",
        ));
    }
    Ok(())
}

fn journal(project: &Path, run_id: &str) -> io::Result<PathBuf> {
    if !is_digest(run_id) {
        return Err(io::Error::other("invalid run ID"));
    }
    let index = project.join(INDEX);
    let history = index.join("history");
    for folder in [&index, &history] {
        match fs::symlink_metadata(folder) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(io::Error::other("invalid run history directory")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(history.join(format!("{run_id}.json")))
}

/// Makes acceptance durable outside the attempt folder before dispatch. The
/// caller holds the project run-index lock, even when recovering a lost receipt.
pub fn confirm_start(project: &Path, attempt: &Attempt, folder: &Path) -> io::Result<()> {
    let mut entries = allocations(project, &attempt.run_id)?;
    let entry = entries
        .iter_mut()
        .find(|entry| entry.attempt_number == attempt.attempt_number)
        .ok_or_else(|| io::Error::other("the attempt has no allocation"))?;
    if entry.existing_folder(project)?.as_deref() != Some(&folder.canonicalize()?) {
        return Err(io::Error::other(
            "the started attempt does not match its allocated folder",
        ));
    }
    if entry.started {
        return Ok(());
    }
    sync_start(folder)?;
    entry.started = true;
    save(project, &attempt.run_id, &entries)
}

/// A process can stop after emitting its initial event but before confirming
/// its reservation. Confirm that start before an explicit resume dispatches.
pub(crate) fn confirm_before_resume(
    project: &Path,
    attempt: &Attempt,
    folder: &Path,
) -> io::Result<()> {
    if attempt.attempt_number == 1 {
        return Ok(());
    }
    let folder = folder.canonicalize()?;
    let needs_confirmation = allocations(project, &attempt.run_id)?.iter().any(|entry| {
        entry.attempt_number == attempt.attempt_number
            && !entry.started
            && entry.path(project) == folder
    });
    if needs_confirmation {
        let _index = run_index::allocate_attempt(project)?;
        confirm_start(project, attempt, &folder)?;
    }
    Ok(())
}

/// Persist the initial event and its folder before a rerun can dispatch work.
fn sync_start(folder: &Path) -> io::Result<()> {
    File::open(folder.join("events.jsonl"))?.sync_all()?;
    sync_directory(folder)?;
    if let Some(parent) = folder.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
