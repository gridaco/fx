//! Named run allocation and recorded-workflow lookup, independent of a viewer service.

use grida_fx_core::Error;
use grida_fx_core::value::sha256_hex;
use grida_fx_runtime::events::run_metadata;
use grida_fx_runtime::folder::safe_name;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const MAX_ENTRIES: usize = 2048;
const MAX_DEPTH: usize = 4;

pub(super) fn validate_name(name: &str) -> Result<(), Error> {
    if name.len() > 64
        || !name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(Error::usage(
            "run names use 1–64 ASCII letters, digits, underscores or hyphens, beginning with a letter or digit",
        ));
    }
    Ok(())
}

fn named_parent(runs: &Path, id: &str, source: &str) -> PathBuf {
    runs.join(safe_name(id))
        .join(format!("named-{}", sha256_hex(source.as_bytes())))
}

/// A readable prefix plus a digest preserves case-sensitive names on case-insensitive disks.
fn named_folder(runs: &Path, id: &str, source: &str, name: &str) -> PathBuf {
    named_parent(runs, id, source).join(format!("{name}-{}", sha256_hex(name.as_bytes())))
}

/// Claim a name atomically. A surviving empty claim is a collision as well.
pub(super) fn create_named(
    runs: &Path,
    id: &str,
    source: &str,
    name: &str,
) -> Result<PathBuf, Error> {
    validate_name(name)?;
    let parent = named_parent(runs, id, source);
    ensure_directory(runs)?;
    ensure_directory(&runs.join(safe_name(id)))?;
    ensure_directory(&parent)?;
    let folder = named_folder(runs, id, source, name);
    std::fs::create_dir(&folder).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists && !folder.join("plan.json").exists() {
            Error::usage(format!(
                "run {id}/{name} was claimed but holds no run; remove the claim with grida-fx \
                 runs remove {id}/{name}, or choose a new name"
            ))
        } else if error.kind() == std::io::ErrorKind::AlreadyExists {
            Error::usage(format!(
                "run {id}/{name} already exists; use --resume {name} or choose a new name"
            ))
        } else {
            Error::io(&format!("run {id}/{name}"), &error)
        }
    })?;
    Ok(folder)
}

/// A resume never creates missing state, and checks the exact recorded logical workflow.
pub(super) fn resume_named(
    runs: &Path,
    id: &str,
    source: &str,
    name: &str,
) -> Result<PathBuf, Error> {
    validate_name(name)?;
    let folder = named_folder(runs, id, source, name);
    let parent = named_parent(runs, id, source);
    for path in [runs, &runs.join(safe_name(id)), &parent, &folder] {
        if !std::fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        {
            return Err(Error::usage(format!(
                "no named run {id}/{name}; create it with --name {name}"
            )));
        }
    }
    read_recorded(&folder)
        .filter(|run| run.id == id && run.source == source && run.name.as_deref() == Some(name))
        .map(|run| run.folder)
        .ok_or_else(|| {
            Error::usage(format!(
                "no named run {id}/{name}; create it with --name {name}"
            ))
        })
}

/// Only regular directories can form the allocation namespace. Configured roots may be
/// outside the project, but symbolic links under those roots cannot redirect a claim.
fn ensure_directory(path: &Path) -> Result<(), Error> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(Error::usage(
            "named run folders cannot traverse symbolic links or non-directories",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                ensure_directory(parent)?;
            }
            match std::fs::create_dir(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    ensure_directory(path)
                }
                Err(error) => Err(Error::io("named run folders", &error)),
            }
        }
        Err(error) => Err(Error::io("named run folders", &error)),
    }
}

struct RecordedRun {
    folder: PathBuf,
    id: String,
    source: String,
    name: Option<String>,
    created_at: String,
}

/// The newest-created run, including failures; different recorded sources cannot silently
/// share an ID lookup. Names are unique only within a source, so name lookup also checks it.
pub(super) fn resolve(runs: &Path, id: &str, name: Option<&str>) -> Result<Option<PathBuf>, Error> {
    resolve_bounded(runs, id, name, None)
}

pub(super) fn resolve_control(
    runs: &Path,
    id: &str,
    name: &str,
    wait: Option<(Instant, Duration)>,
) -> Result<Option<PathBuf>, Error> {
    resolve_bounded(runs, id, Some(name), wait)
}

fn resolve_bounded(
    runs: &Path,
    id: &str,
    name: Option<&str>,
    wait: Option<(Instant, Duration)>,
) -> Result<Option<PathBuf>, Error> {
    let candidates: Vec<_> = scan(runs, wait)?
        .into_iter()
        .filter(|run| run.id == id)
        .collect();
    let sources: BTreeSet<_> = candidates.iter().map(|run| &run.source).collect();
    if sources.len() > 1 {
        return Err(Error::usage(format!(
            "workflow {id} has runs from multiple sources; inspect an explicit run folder"
        )));
    }
    let mut matches: Vec<_> = candidates
        .into_iter()
        .filter(|run| name.is_none_or(|name| run.name.as_deref() == Some(name)))
        .collect();
    if name.is_some() && matches.len() > 1 {
        return Err(Error::usage(format!(
            "named run {id}/{} is ambiguous; inspect an explicit run folder",
            name.unwrap_or_default()
        )));
    }
    matches.sort_by(|a, b| (&a.created_at, &a.folder).cmp(&(&b.created_at, &b.folder)));
    Ok(matches.pop().map(|run| run.folder))
}

/// Bounded discovery stays within the configured run tree and never follows symlinks.
fn scan(runs: &Path, wait: Option<(Instant, Duration)>) -> Result<Vec<RecordedRun>, Error> {
    let mut stack = vec![(runs.to_path_buf(), 0)];
    let mut visited = 0;
    let mut recorded = Vec::new();
    while let Some((folder, depth)) = stack.pop() {
        if wait.is_some_and(|(started, timeout)| started.elapsed() >= timeout) {
            return Err(Error::usage(
                "the control wait deadline expired during run discovery",
            ));
        }
        let Ok(metadata) = std::fs::symlink_metadata(&folder) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        if let Some(run) = read_recorded(&folder) {
            recorded.push(run);
            continue;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_ENTRIES {
                return Err(Error::usage(
                    "run lookup exceeds 2048 entries; inspect an explicit run folder",
                ));
            }
            if entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
            {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    Ok(recorded)
}

fn read_recorded(folder: &Path) -> Option<RecordedRun> {
    let path = folder.join("plan.json");
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    if metadata.len() > 16 * 1024 * 1024 {
        return None;
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 16 * 1024 * 1024 {
        return None;
    }
    let graph: Value = serde_json::from_slice(&bytes).ok()?;
    if graph.get("kind")?.as_str()? != "fx-graph-v1" {
        return None;
    }
    let workflow = graph.get("workflow")?;
    let id = workflow.get("id")?.as_str()?.to_string();
    let source = workflow
        .get("file")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let events_path = folder.join("events.jsonl");
    if std::fs::symlink_metadata(&events_path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return None;
    }
    // Lookup needs only the immutable first run_started metadata. Bound this read as well
    // as tree traversal; a client must never load an arbitrarily large run to select it.
    let events = match std::fs::File::open(&events_path) {
        Ok(file) => {
            if file.metadata().ok()?.len() > 64 * 1024 * 1024 {
                return None;
            }
            let mut bytes = Vec::new();
            file.take(64 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            if bytes.len() > 64 * 1024 * 1024 {
                return None;
            }
            let mut events = Vec::new();
            for line in bytes.split_inclusive(|byte| *byte == b'\n') {
                if line.last() != Some(&b'\n') {
                    break;
                }
                let Ok(event) = serde_json::from_slice::<Value>(line) else {
                    break;
                };
                if event.get("event").and_then(Value::as_str) == Some("run_started") {
                    events.push(event);
                    break;
                }
            }
            events
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return None,
    };
    let (name, created_at) = run_metadata(
        &events,
        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
    );
    Some(RecordedRun {
        folder: folder.to_path_buf(),
        id,
        source,
        name,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Barrier};

    fn record(
        runs: &Path,
        folder: &str,
        source: &str,
        name: Option<&str>,
        created_at: &str,
        ok: bool,
    ) -> PathBuf {
        let folder = runs.join(folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("plan.json"),
            json!({"kind":"fx-graph-v1","workflow":{"id":"case","file":source}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            folder.join("events.jsonl"),
            format!(
                "{}\n{}\n",
                json!({"event":"run_started","name":name,"created_at":created_at}),
                json!({"event":"run_finished","ok":ok})
            ),
        )
        .unwrap();
        folder
    }

    #[test]
    fn creation_is_atomic_and_source_scoped() {
        let root = tempfile::tempdir().unwrap();
        let runs = root.path().join("runs");
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let runs = runs.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    create_named(&runs, "case", "workflow.yaml", "baseline")
                })
            })
            .collect();
        let successes = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(Result::is_ok)
            .count();
        assert_eq!(successes, 1);
        assert!(create_named(&runs, "case", "other.yaml", "baseline").is_ok());
        let mixed_case = create_named(&runs, "case", "workflow.yaml", "Baseline").unwrap();
        assert_ne!(
            mixed_case,
            named_folder(&runs, "case", "workflow.yaml", "baseline")
        );
        for name in ["", "..", "a/b", "a b", "_a", "é", &"a".repeat(65)] {
            assert!(validate_name(name).is_err());
        }
        assert!(validate_name("Character_1-rig").is_ok());
    }

    #[test]
    fn lookup_uses_creation_even_for_failed_latest_and_ignores_resume_time() {
        let root = tempfile::tempdir().unwrap();
        let old = record(
            root.path(),
            "case/old",
            "workflow.yaml",
            Some("old"),
            "2026-10-08T01:00:00.000Z",
            true,
        );
        let latest = record(
            root.path(),
            "case/latest",
            "workflow.yaml",
            Some("latest"),
            "2026-10-08T02:00:00.000Z",
            false,
        );
        use std::io::Write;
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(old.join("events.jsonl"))
                .unwrap(),
            "{}",
            json!({"event":"run_started","name":"renamed","created_at":"2026-10-08T03:00:00.000Z"})
        )
        .unwrap();
        assert_eq!(resolve(root.path(), "case", None).unwrap(), Some(latest));
        assert_eq!(
            resolve(root.path(), "case", Some("old")).unwrap(),
            Some(old)
        );
        assert!(
            resolve(root.path(), "case", Some("renamed"))
                .unwrap()
                .is_none()
        );
        record(
            root.path(),
            "other/run",
            "other.yaml",
            None,
            "2026-10-08T04:00:00.000Z",
            true,
        );
        assert!(resolve(root.path(), "case", None).is_err());
        assert!(resolve(root.path(), "case", Some("old")).is_err());
    }

    #[test]
    fn newest_lookup_compares_normalized_creation_times() {
        let root = tempfile::tempdir().unwrap();
        record(
            root.path(),
            "case/older",
            "workflow.yaml",
            None,
            "2026-10-08T01:00:00Z",
            true,
        );
        let newest = record(
            root.path(),
            "case/newer",
            "workflow.yaml",
            None,
            "2026-10-08T10:00:00.1+09:00",
            true,
        );
        assert_eq!(resolve(root.path(), "case", None).unwrap(), Some(newest));
    }

    #[test]
    fn resume_requires_recorded_exact_name_source_and_does_not_create() {
        let root = tempfile::tempdir().unwrap();
        assert!(resume_named(root.path(), "case", "workflow.yaml", "baseline").is_err());
        let claimed = create_named(root.path(), "case", "workflow.yaml", "baseline").unwrap();
        assert!(resume_named(root.path(), "case", "workflow.yaml", "baseline").is_err());
        let relative = claimed.strip_prefix(root.path()).unwrap().to_str().unwrap();
        record(
            root.path(),
            relative,
            "workflow.yaml",
            Some("baseline"),
            "2026-10-08T00:00:00.000Z",
            true,
        );
        assert_eq!(
            resume_named(root.path(), "case", "workflow.yaml", "baseline").unwrap(),
            claimed
        );
        for index in 0..MAX_ENTRIES + 1 {
            std::fs::create_dir(root.path().join(format!("unrelated-{index}"))).unwrap();
        }
        assert_eq!(
            resume_named(root.path(), "case", "workflow.yaml", "baseline").unwrap(),
            claimed
        );
        assert!(resolve(root.path(), "case", None).is_err());
        assert!(resume_named(root.path(), "case", "other.yaml", "baseline").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn allocation_and_discovery_refuse_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let runs = root.path().join("runs");
        std::os::unix::fs::symlink(outside.path(), &runs).unwrap();
        assert!(create_named(&runs, "case", "workflow.yaml", "baseline").is_err());
        assert!(resolve(&runs, "case", None).unwrap().is_none());
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }
}
