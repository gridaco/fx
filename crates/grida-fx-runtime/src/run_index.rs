//! The run index (spec/store.md §9, "Which runs a project holds"): every run folder a planning
//! project's runs used, so that listing and removal know every run of the project wherever it
//! lies, whatever `--no-view` said, and whatever the `runs` setting says now.
//!
//! ```text
//! <project root>/.fx/runs/
//!   .gitignore                  `*`
//!   lock                        held exclusively by `runs remove` while it removes ([`removals`])
//!   <sha256(folder)>.json       {"folder": "<folder>"}
//! ```
//!
//! - `<folder>` is the run folder's canonical path, relative to the project root (POSIX) when it
//!   lies inside the project, so a moved project keeps its index, and absolute otherwise.
//! - Written by the runner once it holds the folder's `run.lock`, before it writes `plan.json`
//!   or an event ([`RunIndex::record`]). Create-only: an entry that exists is left alone.
//! - Private local state, like `.fx/service`: it names local paths, so it is never part of a
//!   portable record and never leaves the machine. `.fx/runs` is `0700` and each entry `0600`;
//!   `.fx` itself may be a symbolic link (a project that keeps its local state elsewhere).
//! - Read without writing ([`entries`]); an entry goes when its run is removed ([`forget`]).

use crate::folder::temporary_name;
use grida_fx_core::value::sha256_hex;
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, ErrorKind, Write};
use std::path::{Component, Path, PathBuf};

/// The index folder under a project root.
pub const INDEX: &str = ".fx/runs";

/// An entry is a few hundred bytes; anything larger is not one.
const MAX_ENTRY_BYTES: u64 = 64 * 1024;

/// Where a run's folder is recorded: the planning project's run index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunIndex {
    /// The planning project's root (absolute, canonical).
    pub project_root: PathBuf,
}

impl RunIndex {
    /// Records `folder`, which exists (module doc).
    pub fn record(&self, folder: &Path) -> io::Result<()> {
        let recorded = recorded_form(&self.project_root, &folder.canonicalize()?)?;
        let index = self.project_root.join(INDEX);
        private_folder(&index)?;
        let path = index.join(entry_name(&recorded));
        if fs::symlink_metadata(&path).is_ok() {
            return Ok(());
        }
        let temporary = index.join(temporary_name());
        let written = (|| {
            let mut file = private_new_file(&temporary)?;
            file.write_all(serde_json::to_string(&json!({ "folder": recorded }))?.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            match fs::hard_link(&temporary, &path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error),
            }
        })();
        let _ = fs::remove_file(&temporary);
        written
    }
}

/// The recorded folders, resolved against the project root, in no particular order. A missing
/// index is empty; an entry that cannot be read is an error, so that a reader that must know
/// every run never takes it for absent.
pub fn entries(project_root: &Path) -> io::Result<Vec<PathBuf>> {
    let index = project_root.join(INDEX);
    let listing = match fs::read_dir(&index) {
        Ok(listing) => listing,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut folders = Vec::new();
    for item in listing {
        let item = item?;
        let name = item.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(digest) = name.strip_suffix(".json") else {
            continue;
        };
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            continue;
        }
        let path = item.path();
        let metadata = fs::symlink_metadata(&path)?;
        let unreadable = || {
            io::Error::new(
                ErrorKind::InvalidData,
                format!("{INDEX}/{name} is not a run index entry"),
            )
        };
        if !metadata.is_file() || metadata.len() > MAX_ENTRY_BYTES {
            return Err(unreadable());
        }
        let value: Value = serde_json::from_slice(&fs::read(&path)?).map_err(|_| unreadable())?;
        let recorded = value
            .get("folder")
            .and_then(Value::as_str)
            .filter(|recorded| entry_name(recorded) == name && well_formed(recorded))
            .ok_or_else(unreadable)?;
        folders.push(resolve(project_root, recorded));
    }
    Ok(folders)
}

/// Removes the entry of `folder` (canonical, or as [`entries`] gave it): whether there was one.
pub fn forget(project_root: &Path, folder: &Path) -> io::Result<bool> {
    let Ok(recorded) = recorded_form(project_root, folder) else {
        return Ok(false);
    };
    match fs::remove_file(project_root.join(INDEX).join(entry_name(&recorded))) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// The lock `runs remove` holds while it selects and removes runs, so two removals never decide
/// against each other's selection: taken exclusively, waiting (saying so once on stderr).
pub fn removals(project_root: &Path) -> io::Result<File> {
    let index = project_root.join(INDEX);
    private_folder(&index)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(index.join("lock"))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            eprintln!("waiting for another grida-fx runs remove to finish");
            file.lock()?;
        }
        Err(TryLockError::Error(error)) => return Err(error),
    }
    Ok(file)
}

/// How a canonical folder is recorded: relative (POSIX) inside the project, else absolute.
fn recorded_form(project_root: &Path, folder: &Path) -> io::Result<String> {
    let text = match folder.strip_prefix(project_root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
        Ok(relative) => {
            let parts: Option<Vec<&str>> = relative
                .components()
                .map(|c| c.as_os_str().to_str())
                .collect();
            parts.map(|parts| parts.join("/")).ok_or_else(not_utf8)?
        }
        Err(_) => folder.to_str().ok_or_else(not_utf8)?.to_string(),
    };
    Ok(text)
}

fn not_utf8() -> io::Error {
    io::Error::new(
        ErrorKind::InvalidInput,
        "the run folder's path is not UTF-8",
    )
}

/// An absolute path, `.`, or a relative POSIX path of plain names.
fn well_formed(recorded: &str) -> bool {
    let path = Path::new(recorded);
    recorded == "."
        || path.is_absolute()
        || (!recorded.contains('\\')
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_))))
}

fn resolve(project_root: &Path, recorded: &str) -> PathBuf {
    if recorded == "." {
        project_root.to_path_buf()
    } else if Path::new(recorded).is_absolute() {
        PathBuf::from(recorded)
    } else {
        recorded
            .split('/')
            .fold(project_root.to_path_buf(), |path, part| path.join(part))
    }
}

/// `<sha256 of the recorded form>.json`.
fn entry_name(recorded: &str) -> String {
    format!("{}.json", sha256_hex(recorded.as_bytes()))
}

/// Makes `.fx` (when missing) and `.fx/runs` private, with a `.gitignore` of `*`. `.fx/runs`
/// must be a real folder; `.fx` may be a symbolic link to one.
fn private_folder(index: &Path) -> io::Result<()> {
    let fx = index.parent().unwrap_or(index);
    match fs::metadata(fx) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("{} is not a folder", fx.display()),
            ));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => make_private(fx)?,
        Err(error) => return Err(error),
    }
    match fs::symlink_metadata(index) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("{} is not a folder", index.display()),
            ));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => make_private(index)?,
        Err(error) => return Err(error),
    }
    let ignore = index.join(".gitignore");
    if fs::symlink_metadata(&ignore).is_err() {
        let temporary = index.join(temporary_name());
        let mut file = private_new_file(&temporary)?;
        file.write_all(b"*\n")?;
        drop(file);
        let renamed = fs::rename(&temporary, &ignore);
        let _ = fs::remove_file(&temporary);
        renamed?;
    }
    Ok(())
}

fn make_private(folder: &Path) -> io::Result<()> {
    match fs::create_dir(folder) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(folder, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn private_new_file(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_every_run_once_relative_inside_the_project_and_forgets_it() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let index = RunIndex {
            project_root: root.join("project"),
        };
        let inside = root.join("project/runs/one");
        let outside = elsewhere.path().canonicalize().unwrap().join("two");
        fs::create_dir_all(&inside).unwrap();
        fs::create_dir_all(&outside).unwrap();
        index.record(&inside).unwrap();
        index.record(&inside).unwrap();
        index.record(&outside).unwrap();
        let mut found = entries(&index.project_root).unwrap();
        found.sort();
        let mut expected = vec![inside.clone(), outside.clone()];
        expected.sort();
        assert_eq!(found, expected);
        let recorded =
            fs::read_to_string(index.project_root.join(INDEX).join(entry_name("runs/one")))
                .unwrap();
        assert_eq!(recorded, "{\"folder\":\"runs/one\"}\n");
        assert_eq!(
            fs::read_to_string(index.project_root.join(".fx/runs/.gitignore")).unwrap(),
            "*\n"
        );
        // A moved project keeps what it recorded inside itself.
        fs::rename(root.join("project"), root.join("moved")).unwrap();
        let moved = root.join("moved");
        assert!(entries(&moved).unwrap().contains(&moved.join("runs/one")));
        assert!(forget(&moved, &moved.join("runs/one")).unwrap());
        assert!(!forget(&moved, &moved.join("runs/one")).unwrap());
        assert!(forget(&moved, &outside).unwrap());
        assert!(entries(&moved).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_fx_folder_is_followed() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let state = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(state.path(), root.join(".fx")).unwrap();
        fs::create_dir_all(root.join("runs/one")).unwrap();
        RunIndex {
            project_root: root.clone(),
        }
        .record(&root.join("runs/one"))
        .unwrap();
        assert_eq!(entries(&root).unwrap(), vec![root.join("runs/one")]);
        drop(removals(&root).unwrap());
    }

    #[test]
    fn an_unreadable_entry_is_an_error_not_an_absence() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fs::create_dir_all(root.join(INDEX)).unwrap();
        fs::write(root.join(INDEX).join(entry_name("x")), "{").unwrap();
        assert!(entries(&root).is_err());
        fs::write(
            root.join(INDEX).join(entry_name("x")),
            json!({"folder": "/elsewhere"}).to_string(),
        )
        .unwrap();
        assert!(entries(&root).is_err());
        fs::write(
            root.join(INDEX).join(entry_name("../x")),
            json!({"folder": "../x"}).to_string(),
        )
        .unwrap();
        fs::remove_file(root.join(INDEX).join(entry_name("x"))).unwrap();
        assert!(entries(&root).is_err());
    }
}
