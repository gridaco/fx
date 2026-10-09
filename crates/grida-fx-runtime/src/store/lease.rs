//! The store's lock and its users (spec/store.md §6, "The lock", and §9, "Pruning a store").
//!
//! ```text
//! <store>/lock                   an OS file lock (`flock` on POSIX); its content means nothing
//! <store>/projects/<id>          one empty file per planning project that wrote the store,
//!                                id = sha256 of the project root's canonical path; its
//!                                modification time is that project's last use
//! <store>/projects/legacy        written when `projects/` was made in a store that already held
//!                                something: projects that used it before are unknown
//! ```
//!
//! - **Shared, from the first write.** A process holds the lock of a store shared from the first
//!   time it writes to it (a file, a record, a job record removed, a work claim, a sweep) until
//!   the process ends ([`Store::lease`], called by every writer in `super`). Reading takes no
//!   lock. While a prune holds the lock, a writer waits up to [`PRUNE_WAIT`] and then fails
//!   ("the cache is being pruned"), telling stderr once that it waits. Where the file system
//!   cannot lock, a writer goes on without the lock, saying so once.
//! - **Users.** When it takes the lock, a process whose planning project is known
//!   ([`set_user`]) marks it among the store's users: `projects/<id>` in the *project's* store
//!   (for a stand-in store, the store it lies in), made or touched.
//! - **Exclusive, for pruning.** [`Store::lock_exclusive`] takes the lock without waiting; held
//!   shared by anyone, it is refused. The process that holds it writes the store freely.

use super::Store;
use grida_fx_core::value::sha256_hex;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// How long a writer waits for a prune to finish.
pub const PRUNE_WAIT: Duration = Duration::from_secs(60);

/// The legacy marker (module doc).
pub const LEGACY: &str = "legacy";

/// The locks this process holds, by store root: `None` where the file system cannot lock.
static HELD: Mutex<BTreeMap<PathBuf, Option<File>>> = Mutex::new(BTreeMap::new());

/// The planning project of this process, and its store's root (module doc, "Users").
static USER: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();

fn held() -> MutexGuard<'static, BTreeMap<PathBuf, Option<File>>> {
    HELD.lock().unwrap_or_else(|e| e.into_inner())
}

/// Names `project_root` (canonical) as the planning project whose store is `store_root`; the
/// first call wins.
pub fn set_user(project_root: &Path, store_root: &Path) {
    let _ = USER.set((project_root.to_path_buf(), store_root.to_path_buf()));
}

/// A project's id among a store's users: the SHA-256 of its root's canonical path.
pub fn user_id(project_root: &Path) -> String {
    sha256_hex(project_root.as_os_str().as_encoded_bytes())
}

/// Why a lock was not taken.
#[derive(Debug)]
pub enum NotLocked {
    /// Someone holds it (shared or, for a writer, a prune past [`PRUNE_WAIT`]).
    Held,
    /// The file system cannot lock, or the lock file cannot be made.
    Unsupported(io::Error),
}

impl Store {
    /// Holds this store's lock shared until the process ends (module doc). Once per store and
    /// process; a store this process holds exclusively is already held.
    pub fn lease(&self) -> Result<(), NotLocked> {
        self.lease_waiting(PRUNE_WAIT)
    }

    /// [`Store::lease`], waiting at most `wait` for a prune. The process-wide table is not held
    /// while it waits, so other stores, and this one once leased, are not held up.
    pub fn lease_waiting(&self, wait: Duration) -> Result<(), NotLocked> {
        if held().contains_key(&self.root) {
            return Ok(());
        }
        let unsupported = |error: io::Error| {
            eprintln!("note: the cache cannot be locked ({error}); going on without its lock");
            if let Entry::Vacant(vacant) = held().entry(self.root.clone()) {
                vacant.insert(None);
            }
            self.mark_user();
            Ok(())
        };
        let file = match lock_file(&self.root) {
            Ok(file) => file,
            Err(error) => return unsupported(error),
        };
        let started = Instant::now();
        let mut told = false;
        loop {
            match file.try_lock_shared() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if started.elapsed() < wait => {
                    if !told {
                        eprintln!("waiting for grida-fx cache prune to finish");
                        told = true;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(TryLockError::WouldBlock) => return Err(NotLocked::Held),
                Err(TryLockError::Error(error)) => return unsupported(error),
            }
        }
        // Another thread of this process may have leased it meanwhile: one lock is enough.
        if let Entry::Vacant(vacant) = held().entry(self.root.clone()) {
            vacant.insert(Some(file));
        }
        self.mark_user();
        Ok(())
    }

    /// Takes this store's lock exclusively, without waiting (module doc).
    pub fn lock_exclusive(&self) -> Result<(), NotLocked> {
        let mut held = held();
        if let Some(Some(_)) = held.get(&self.root) {
            // Held shared by this process: a prune never writes first.
            return Err(NotLocked::Held);
        }
        let file = lock_file(&self.root).map_err(NotLocked::Unsupported)?;
        match file.try_lock() {
            Ok(()) => {
                held.insert(self.root.clone(), Some(file));
                Ok(())
            }
            Err(TryLockError::WouldBlock) => Err(NotLocked::Held),
            Err(TryLockError::Error(error)) => Err(NotLocked::Unsupported(error)),
        }
    }

    /// The store's users: each marker's name (a project id or [`LEGACY`]) and its last use.
    pub fn users(&self) -> io::Result<Vec<(String, SystemTime)>> {
        let listing = match fs::read_dir(self.root.join("projects")) {
            Ok(listing) => listing,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut users = Vec::new();
        for item in listing {
            let item = item?;
            let name = item.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let used = item
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            users.push((name, used));
        }
        users.sort();
        Ok(users)
    }

    /// Marks this process's planning project among the users of its store (module doc). Best
    /// effort: a marker that cannot be written makes a later prune see fewer users, never more,
    /// so it is reported.
    fn mark_user(&self) {
        let Some((project, store)) = USER.get() else {
            return;
        };
        if &self.root != store && self.root.parent() != Some(store.as_path()) {
            return;
        }
        let users = store.join("projects");
        if let Err(error) = make_users(store, &users) {
            eprintln!("note: the cache's projects/ cannot be made: {error}");
            return;
        }
        if let Err(error) = touch(&users.join(user_id(project))) {
            eprintln!("note: the cache's projects/ cannot be written: {error}");
        }
    }
}

/// Makes `projects/` when missing, holding [`LEGACY`] when the store already holds something.
/// It is made beside and renamed into place, so `projects/` never exists without the marker it
/// needs; one made meanwhile by another process is left as it is.
fn make_users(store: &Path, users: &Path) -> io::Result<()> {
    if fs::symlink_metadata(users).is_ok() {
        return Ok(());
    }
    let making = store.join(format!(".projects{}", crate::folder::temporary_name()));
    fs::create_dir(&making)?;
    let made = (|| {
        if holds_anything(store) {
            touch(&making.join(LEGACY))?;
        }
        fs::rename(&making, users)
    })();
    if made.is_err() {
        let _ = fs::remove_file(making.join(LEGACY));
        let _ = fs::remove_dir(&making);
        if fs::symlink_metadata(users).is_ok() {
            return Ok(());
        }
    }
    made
}

/// `<root>/lock`, made with its folder when missing.
fn lock_file(root: &Path) -> io::Result<File> {
    fs::create_dir_all(root)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("lock"))
}

/// Makes `path` or sets its modification time to now.
fn touch(path: &Path) -> io::Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.set_modified(SystemTime::now())
}

/// Whether a store, or the stand-in store in it, holds any file, result, call or job.
pub fn holds_anything(store: &Path) -> bool {
    [
        "files",
        "results",
        "calls",
        "jobs",
        "stand-in/files",
        "stand-in/results",
        "stand-in/calls",
    ]
    .iter()
    .any(|part| fs::read_dir(store.join(part)).is_ok_and(|mut entries| entries.next().is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_shared_by_one_process_cannot_be_held_exclusively_by_another_opener() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("cache"));
        store.lease().unwrap();
        store.lease().unwrap();
        assert!(matches!(store.lock_exclusive(), Err(NotLocked::Held)));
        // Another open file description (as another process would have) cannot take it either.
        let other = File::open(root.path().join("cache/lock")).unwrap();
        assert!(matches!(other.try_lock(), Err(TryLockError::WouldBlock)));
        other.try_lock_shared().unwrap();
    }

    #[test]
    fn a_writer_waits_for_a_prune_then_gives_up() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("cache"));
        let prune = lock_file(store.root()).unwrap();
        prune.try_lock().unwrap();
        assert!(matches!(
            store.lease_waiting(Duration::from_millis(300)),
            Err(NotLocked::Held)
        ));
        drop(prune);
        store.lease_waiting(Duration::from_millis(300)).unwrap();
    }

    #[test]
    fn an_exclusive_holder_writes_freely() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("cache"));
        store.lock_exclusive().unwrap();
        store.lease().unwrap();
        let other = File::open(root.path().join("cache/lock")).unwrap();
        assert!(matches!(
            other.try_lock_shared(),
            Err(TryLockError::WouldBlock)
        ));
    }
}
