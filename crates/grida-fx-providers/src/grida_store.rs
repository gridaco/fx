//! Read-only credential records from Grida's provider file protocol v1.
//!
//! The caller supplies the exact, trusted Grida home; this module reads no environment and
//! performs no discovery. Missing directories are absence, without creating them. An existing
//! provider directory uses the protocol's private SQLite lock and orphan cleanup, but credential
//! records, migration state and existing permissions are never changed. See the pinned protocol and
//! synthetic compatibility inputs in `tests/fixtures/grida-provider-v1/README.md`.

use crate::keys::Secret;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// A fixed, secret-free failure. Parser diagnostics, filesystem paths and underlying errors are
/// deliberately not retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    InvalidInput,
    InvalidStore,
    UnsupportedVersion,
    StorageFailed,
    StoreBusy,
    MigrationPending,
    UnsupportedPlatform,
}

impl StoreError {
    /// The provider-v1 failure code, suitable for a diagnostic without exposing custody.
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::InvalidStore => "invalid_store",
            Self::UnsupportedVersion => "unsupported_version",
            Self::StorageFailed => "storage_failed",
            Self::StoreBusy => "store_busy",
            Self::MigrationPending => "migration_pending",
            Self::UnsupportedPlatform => "unsupported_platform",
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guidance = match self {
            Self::InvalidInput => "Use an absolute, normalized GRIDA_HOME and a valid provider ID.",
            Self::InvalidStore => {
                "Check the credential file format with grida ai providers --help."
            }
            Self::UnsupportedVersion => {
                "Update Grida FX to a compatible version; do not edit the stored version."
            }
            Self::StorageFailed => {
                "Check Grida home access, ownership, private permissions and filesystem aliases."
            }
            Self::StoreBusy => "Retry after the other Grida client finishes.",
            Self::MigrationPending => {
                "Open updated Grida Desktop to finish provider migration, or supply an environment key."
            }
            Self::UnsupportedPlatform => {
                "Stored credentials require macOS or Linux; supply an environment key instead."
            }
        };
        write!(
            formatter,
            "Grida provider credentials: {}. {guidance}",
            self.code()
        )
    }
}

impl std::error::Error for StoreError {}

/// Read exactly the requested provider IDs after validating the entire store. IDs are opaque;
/// the caller owns the allowlist of providers eligible to receive these secrets. Returned values
/// retain their exact bytes and must remain outside records, browser documents and diagnostics.
pub fn read(home: &Path, providers: &[&str]) -> Result<BTreeMap<String, Secret>, StoreError> {
    read_with_cancellation(home, providers, None)
}

pub(crate) fn read_cancellable(
    home: &Path,
    providers: &[&str],
    cancelled: &AtomicBool,
) -> Result<BTreeMap<String, Secret>, StoreError> {
    read_with_cancellation(home, providers, Some(cancelled))
}

fn read_with_cancellation(
    home: &Path,
    providers: &[&str],
    cancelled: Option<&AtomicBool>,
) -> Result<BTreeMap<String, Secret>, StoreError> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        native::read(home, providers, cancelled)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (home, providers, cancelled);
        Err(StoreError::UnsupportedPlatform)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native {
    use super::{AtomicBool, Secret, StoreError};
    use rusqlite::{Connection, ErrorCode, OpenFlags};
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs::{self, File, OpenOptions};
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::{Component, Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex, OnceLock, TryLockError, Weak};
    use std::time::{Duration, Instant};

    const BYTE_LIMIT: usize = 1_048_576;
    const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
    const LOCK_POLL: Duration = Duration::from_millis(25);

    fn storage<T>(_: T) -> StoreError {
        StoreError::StorageFailed
    }

    fn valid_provider(id: &str) -> bool {
        (1..=64).contains(&id.len())
            && id.as_bytes()[0].is_ascii_lowercase()
            && id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
            })
    }

    fn location(home: &Path) -> Result<(), StoreError> {
        let value = home.to_str().ok_or(StoreError::InvalidInput)?;
        if !home.is_absolute()
            || home.parent().is_none()
            || value.bytes().any(|byte| byte.is_ascii_control())
            || value.contains("//")
            || value.ends_with('/')
            || value.split('/').any(|part| part == "." || part == "..")
        {
            return Err(StoreError::InvalidInput);
        }
        Ok(())
    }

    /// Validate each existing ancestor before reporting a missing component. No path is repaired
    /// or resolved into another credential authority, including aliases of the temporary root.
    fn directory(path: &Path) -> Result<bool, StoreError> {
        let effective_user = rustix::process::geteuid().as_raw();
        let mut current = PathBuf::from("/");
        let components = std::iter::once(None).chain(path.components().filter_map(|part| {
            if let Component::Normal(name) = part {
                Some(Some(name))
            } else {
                None
            }
        }));
        for component in components {
            if let Some(name) = component {
                current.push(name);
            }
            let metadata = match fs::symlink_metadata(&current) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(storage(error)),
            };
            if !metadata.is_dir() || fs::canonicalize(&current).map_err(storage)? != current {
                return Err(StoreError::StorageFailed);
            }
            let mode = metadata.mode() & 0o7777;
            if current == path {
                if metadata.uid() != effective_user || mode != 0o700 {
                    return Err(StoreError::StorageFailed);
                }
            } else if (metadata.uid() != effective_user && metadata.uid() != 0)
                || (mode & 0o022 != 0 && !(metadata.uid() == 0 && mode & 0o1000 != 0))
            {
                return Err(StoreError::StorageFailed);
            }
            acl(&current)?;
        }
        Ok(true)
    }

    fn private_file(path: &Path, lock: bool) -> Result<File, StoreError> {
        let options = || {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .write(lock)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            options
        };
        let file = if lock {
            match options().create_new(true).open(path) {
                Ok(file) => {
                    // An owner's umask may have removed write access. Set the mode on the
                    // new descriptor before validating it. Existing files are never repaired.
                    file.set_permissions(fs::Permissions::from_mode(0o600))
                        .map_err(storage)?;
                    file
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    options().open(path).map_err(storage)?
                }
                Err(error) => return Err(storage(error)),
            }
        } else {
            options().open(path).map_err(storage)?
        };
        let opened = file.metadata().map_err(storage)?;
        let named = fs::symlink_metadata(path).map_err(storage)?;
        let effective_user = rustix::process::geteuid().as_raw();
        for metadata in [&opened, &named] {
            if !metadata.is_file()
                || metadata.uid() != effective_user
                || metadata.mode() & 0o7777 != 0o600
                || metadata.nlink() != 1
            {
                return Err(StoreError::StorageFailed);
            }
        }
        if opened.dev() != named.dev() || opened.ino() != named.ino() {
            return Err(StoreError::StorageFailed);
        }
        acl(path)?;
        Ok(file)
    }

    fn bytes(path: &Path, oversized: StoreError) -> Result<Option<Vec<u8>>, StoreError> {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage(error)),
            Ok(_) => (),
        }
        let file = private_file(path, false)?;
        if file.metadata().map_err(storage)?.len() > BYTE_LIMIT as u64 {
            return Err(oversized);
        }
        let mut buffer = Vec::new();
        file.take((BYTE_LIMIT + 1) as u64)
            .read_to_end(&mut buffer)
            .map_err(storage)?;
        if buffer.len() > BYTE_LIMIT {
            return Err(oversized);
        }
        Ok(Some(buffer))
    }

    fn temporary(name: &str) -> bool {
        let Some(id) = name
            .strip_prefix("credentials.toml.")
            .and_then(|rest| rest.strip_suffix(".tmp"))
        else {
            return false;
        };
        let id = id.as_bytes();
        id.len() == 36
            && id[14] == b'4'
            && b"89ab".contains(&id[19])
            && id.iter().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    *byte == b'-'
                } else {
                    byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
                }
            })
    }

    fn cleanup(directory: &Path) -> Result<(), StoreError> {
        for entry in fs::read_dir(directory).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            if entry.file_name().to_str().is_some_and(temporary) {
                drop(private_file(&entry.path(), false)?);
                fs::remove_file(entry.path()).map_err(storage)?;
            }
        }
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(storage)
    }

    type ProcessLocks = Mutex<BTreeMap<PathBuf, Weak<Mutex<()>>>>;
    static PROCESS_LOCKS: OnceLock<ProcessLocks> = OnceLock::new();

    fn check_cancelled(cancelled: Option<&AtomicBool>) -> Result<(), StoreError> {
        if cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            // The waiting caller has already gone away, so this result is discarded.
            return Err(StoreError::StoreBusy);
        }
        Ok(())
    }

    fn wait(deadline: Instant) -> Result<(), StoreError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(StoreError::StoreBusy);
        }
        std::thread::sleep(LOCK_POLL.min(remaining));
        Ok(())
    }

    fn locked<T>(
        path: &Path,
        timeout: Duration,
        cancelled: Option<&AtomicBool>,
        operation: impl FnOnce() -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let deadline = Instant::now() + timeout;
        let mutex = {
            let mut locks = PROCESS_LOCKS
                .get_or_init(Default::default)
                .lock()
                .map_err(storage)?;
            if let Some(mutex) = locks.get(path).and_then(Weak::upgrade) {
                mutex
            } else {
                let mutex = Arc::new(Mutex::new(()));
                locks.insert(path.to_owned(), Arc::downgrade(&mutex));
                mutex
            }
        };
        let _guard = loop {
            check_cancelled(cancelled)?;
            match mutex.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::WouldBlock) => wait(deadline)?,
                Err(error) => return Err(storage(error)),
            }
        };
        if !directory(path)? {
            return Err(StoreError::StorageFailed);
        }
        let filename = path.join("profile.lock.sqlite");
        let file = private_file(&filename, true)?;
        if file.metadata().map_err(storage)?.len() != 0 {
            return Err(StoreError::StorageFailed);
        }
        drop(file);
        let journal = path.join("profile.lock.sqlite-journal");
        if let Err(error) = bytes(&journal, StoreError::StorageFailed) {
            // Another holder may have just removed its validated rollback journal.
            if !matches!(fs::symlink_metadata(&journal), Err(missing) if missing.kind() == std::io::ErrorKind::NotFound)
            {
                return Err(error);
            }
        }
        for suffix in ["-wal", "-shm"] {
            match fs::symlink_metadata(path.join(format!("profile.lock.sqlite{suffix}"))) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                _ => return Err(StoreError::StorageFailed),
            }
        }
        let connection = Connection::open_with_flags(
            &filename,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(storage)?;
        connection.load_extension_disable().map_err(storage)?;
        connection.busy_timeout(Duration::ZERO).map_err(storage)?;
        loop {
            check_cancelled(cancelled)?;
            if Instant::now() >= deadline {
                return Err(StoreError::StoreBusy);
            }
            match connection.execute_batch("BEGIN IMMEDIATE") {
                Ok(()) => break,
                Err(rusqlite::Error::SqliteFailure(error, _))
                    if error.code == ErrorCode::DatabaseBusy =>
                {
                    #[cfg(test)]
                    tests::signal_sqlite_busy();
                    wait(deadline)?;
                }
                Err(error) => return Err(storage(error)),
            }
        }
        let outcome = check_cancelled(cancelled).and_then(|()| operation());
        let released = connection.execute_batch("ROLLBACK").map_err(storage);
        let closed = connection.close().map_err(storage);
        released?;
        closed?;
        outcome
    }

    fn table<'a>(value: &'a toml::Value, fields: &[&str]) -> Result<&'a toml::Table, StoreError> {
        let table = value.as_table().ok_or(StoreError::InvalidStore)?;
        if table.len() != fields.len() || fields.iter().any(|field| !table.contains_key(*field)) {
            return Err(StoreError::InvalidStore);
        }
        Ok(table)
    }

    fn depth(value: &toml::Value, level: usize) -> bool {
        level <= 4
            && match value {
                toml::Value::Table(table) => table.values().all(|value| depth(value, level + 1)),
                toml::Value::Array(array) => array.iter().all(|value| depth(value, level + 1)),
                _ => true,
            }
    }

    fn document(bytes: &[u8], requested: &[&str]) -> Result<BTreeMap<String, Secret>, StoreError> {
        let text = std::str::from_utf8(bytes).map_err(|_| StoreError::InvalidStore)?;
        if text.starts_with('\u{feff}') {
            return Err(StoreError::InvalidStore);
        }
        let value: toml::Value = toml::from_str(text).map_err(|_| StoreError::InvalidStore)?;
        match value.get("version").and_then(toml::Value::as_integer) {
            Some(1) => (),
            Some(_) => return Err(StoreError::UnsupportedVersion),
            None => return Err(StoreError::InvalidStore),
        }
        if !depth(&value, 0) {
            return Err(StoreError::InvalidStore);
        }
        let root = table(&value, &["version", "migration", "providers"])?;
        let migration = table(&root["migration"], &["state", "removed"])?;
        let state = migration["state"]
            .as_str()
            .filter(|state| ["unstarted", "pending", "complete"].contains(state))
            .ok_or(StoreError::InvalidStore)?;
        let removed = migration["removed"]
            .as_array()
            .ok_or(StoreError::InvalidStore)?;
        let mut tombstones = BTreeSet::new();
        for value in removed {
            let id = value.as_str().ok_or(StoreError::InvalidStore)?;
            if !valid_provider(id) || !tombstones.insert(id) {
                return Err(StoreError::InvalidStore);
            }
        }
        let providers = root["providers"]
            .as_table()
            .ok_or(StoreError::InvalidStore)?;
        if providers.len() + tombstones.len() > 128
            || (state != "unstarted" && !tombstones.is_empty())
        {
            return Err(StoreError::InvalidStore);
        }
        let mut selected = BTreeMap::new();
        for (id, value) in providers {
            if !valid_provider(id) || tombstones.contains(id.as_str()) {
                return Err(StoreError::InvalidStore);
            }
            let entry = table(value, &["api_key"])?;
            let key = entry["api_key"]
                .as_str()
                .filter(|key| {
                    (1..=16_384).contains(&key.len()) && !key.chars().any(char::is_control)
                })
                .ok_or(StoreError::InvalidStore)?;
            if requested.contains(&id.as_str()) {
                selected.insert(id.clone(), Secret::new(key));
            }
        }
        if state == "pending" {
            return Err(StoreError::MigrationPending);
        }
        Ok(selected)
    }

    pub(super) fn read(
        home: &Path,
        providers: &[&str],
        cancelled: Option<&AtomicBool>,
    ) -> Result<BTreeMap<String, Secret>, StoreError> {
        read_with_timeout(home, providers, LOCK_TIMEOUT, cancelled)
    }

    fn read_with_timeout(
        home: &Path,
        providers: &[&str],
        timeout: Duration,
        cancelled: Option<&AtomicBool>,
    ) -> Result<BTreeMap<String, Secret>, StoreError> {
        check_cancelled(cancelled)?;
        location(home)?;
        if providers.iter().any(|provider| !valid_provider(provider)) {
            return Err(StoreError::InvalidInput);
        }
        if providers.is_empty() {
            return Ok(BTreeMap::new());
        }
        let provider_directory = home.join("providers");
        if !directory(&provider_directory)? {
            return Ok(BTreeMap::new());
        }
        locked(&provider_directory, timeout, cancelled, || {
            cleanup(&provider_directory)?;
            match bytes(
                &provider_directory.join("credentials.toml"),
                StoreError::InvalidStore,
            )? {
                Some(bytes) => document(&bytes, providers),
                None => Ok(BTreeMap::new()),
            }
        })
    }

    #[cfg(target_os = "linux")]
    fn acl(_: &Path) -> Result<(), StoreError> {
        // The group mask and required owner-only modes constrain Linux access ACLs.
        Ok(())
    }

    /// macOS mode bits do not bound extended ACL grants. Query the native ACL without launching
    /// a subprocess, treating all entries except explicit deny entries as unsafe.
    #[cfg(target_os = "macos")]
    #[allow(unsafe_code)] // Native ACL handles and borrowed entries are checked and freed below.
    fn acl(path: &Path) -> Result<(), StoreError> {
        use std::ffi::CString;
        use std::os::raw::{c_char, c_int, c_void};
        use std::os::unix::ffi::OsStrExt;

        unsafe extern "C" {
            fn acl_get_link_np(path: *const c_char, kind: c_int) -> *mut c_void;
            fn acl_get_entry(acl: *mut c_void, selector: c_int, entry: *mut *mut c_void) -> c_int;
            fn acl_get_tag_type(entry: *mut c_void, tag: *mut c_int) -> c_int;
            fn acl_free(acl: *mut c_void) -> c_int;
        }

        let path_text = CString::new(path.as_os_str().as_bytes()).map_err(storage)?;
        // SAFETY: path_text is a live NUL-terminated string; the returned ACL is owned here.
        let acl = unsafe { acl_get_link_np(path_text.as_ptr(), 0x100) }; // ACL_TYPE_EXTENDED
        if acl.is_null() {
            return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT)
                && fs::symlink_metadata(path).is_ok()
            {
                Ok(())
            } else {
                Err(StoreError::StorageFailed)
            };
        }
        let mut result = Ok(());
        let mut selector = 0; // ACL_FIRST_ENTRY
        loop {
            let mut entry = std::ptr::null_mut();
            // SAFETY: acl remains live until acl_free below; entry borrows from that ACL.
            let status = unsafe { acl_get_entry(acl, selector, &mut entry) };
            if status != 0 {
                // macOS reports end-of-list as -1 with EINVAL, including an empty ACL.
                if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINVAL) {
                    result = Err(StoreError::StorageFailed);
                }
                break;
            }
            selector = -1; // ACL_NEXT_ENTRY
            let mut tag = 0;
            // SAFETY: acl_get_entry returned this entry from the still-live ACL.
            if unsafe { acl_get_tag_type(entry, &mut tag) } != 0 || tag != 2 {
                // ACL_EXTENDED_DENY = 2; allow entries and unknown tags are refused.
                result = Err(StoreError::StorageFailed);
                break;
            }
        }
        // SAFETY: acl was returned by acl_get_link_np and is released exactly once.
        unsafe { acl_free(acl) };
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::cell::Cell;
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Child, Command, Stdio};
        use std::sync::mpsc;

        const HOLDER_HOME: &str = "GRIDA_FX_TEST_STORE_HOLDER";
        const HOLDER_READY: &str = "FX_STORE_LOCKED";

        thread_local! {
            static SQLITE_BUSY: Cell<Option<mpsc::Sender<()>>> = const { Cell::new(None) };
        }

        pub(super) fn signal_sqlite_busy() {
            if let Some(signal) = SQLITE_BUSY.take() {
                let _ = signal.send(());
            }
        }

        #[test]
        fn lock_holder_child() {
            let Some(home) = std::env::var_os(HOLDER_HOME) else {
                return;
            };
            let lock = PathBuf::from(home).join("providers/profile.lock.sqlite");
            let connection =
                Connection::open_with_flags(lock, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
            connection.execute_batch("BEGIN IMMEDIATE").unwrap();
            println!("{HOLDER_READY}");
            std::io::stdout().flush().unwrap();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).unwrap();
            connection.execute_batch("ROLLBACK").unwrap();
        }

        struct Holder(Child);

        impl Holder {
            fn start(home: &Path) -> Self {
                let child = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "grida_store::native::tests::lock_holder_child",
                        "--nocapture",
                        "--format",
                        "terse",
                        "--test-threads=1",
                    ])
                    .env_clear()
                    .env(HOLDER_HOME, home)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap();
                let mut holder = Self(child);
                let output = BufReader::new(holder.0.stdout.take().unwrap());
                let (signal, ready) = mpsc::channel();
                let reader = std::thread::spawn(move || {
                    if output
                        .lines()
                        .map_while(Result::ok)
                        .any(|line| line.trim() == HOLDER_READY)
                    {
                        let _ = signal.send(());
                    }
                });
                if let Err(error) = ready.recv_timeout(Duration::from_secs(5)) {
                    // Close the child's pipe before joining the output reader.
                    drop(holder);
                    reader.join().unwrap();
                    panic!("holder did not signal lock acquisition: {error}");
                }
                reader.join().unwrap();
                holder
            }
        }

        impl Drop for Holder {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        #[test]
        fn reader_reports_busy_until_other_process_dies() {
            let temporary = tempfile::tempdir().unwrap();
            let home = temporary.path().canonicalize().unwrap();
            let directory = home.join("providers");
            fs::create_dir(&directory).unwrap();
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
            let credential_file = directory.join("credentials.toml");
            let credential = b"version = 1\n[migration]\nstate = 'unstarted'\nremoved = []\n\
                               [providers.acme]\napi_key = 'fx-fixture-lock-key'\n";
            fs::write(&credential_file, credential).unwrap();
            fs::set_permissions(&credential_file, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(read(&home, &["acme"], None).unwrap().contains_key("acme"));
            let lock = directory.join("profile.lock.sqlite");
            let original = fs::metadata(&lock).unwrap();

            let mut holder = Holder::start(&home);
            assert_eq!(
                read_with_timeout(&home, &["acme"], Duration::from_millis(200), None).unwrap_err(),
                StoreError::StoreBusy
            );
            let cancelled = AtomicBool::new(false);
            std::thread::scope(|scope| {
                let (send, receive) = mpsc::channel();
                let (busy, ready) = mpsc::channel();
                let home = &home;
                let flag = &cancelled;
                let lock_timeout = Duration::from_secs(10);
                scope.spawn(move || {
                    SQLITE_BUSY.set(Some(busy));
                    let began = Instant::now();
                    let result = read_with_timeout(home, &["acme"], lock_timeout, Some(flag));
                    let _ = send.send((result, began.elapsed()));
                });
                ready
                    .recv_timeout(Duration::from_secs(5))
                    .expect("reader did not encounter the SQLite lock");
                cancelled.store(true, Ordering::Relaxed);
                let (result, elapsed) = receive
                    .recv_timeout(Duration::from_secs(2))
                    .expect("reader did not stop after cancellation");
                assert!(elapsed < lock_timeout, "reader reached its lock deadline");
                assert_eq!(result.unwrap_err(), StoreError::StoreBusy);
            });
            assert!(holder.0.try_wait().unwrap().is_none());
            holder.0.kill().unwrap();
            holder.0.wait().unwrap();

            let found = read_with_timeout(&home, &["acme"], Duration::from_secs(5), None).unwrap();
            assert_eq!(found["acme"].expose(), "fx-fixture-lock-key");
            let after = fs::metadata(lock).unwrap();
            assert_eq!(after.ino(), original.ino());
            assert_eq!(after.len(), 0);
            assert_eq!(fs::read(credential_file).unwrap(), credential);
        }

        #[test]
        fn same_process_contention_times_out_without_entering_caller_work() {
            let temporary = tempfile::tempdir().unwrap();
            let directory = temporary.path().canonicalize().unwrap().join("providers");
            fs::create_dir(&directory).unwrap();
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
            locked(&directory, Duration::from_secs(5), None, || {
                let start = Instant::now();
                let result: Result<(), StoreError> =
                    locked(&directory, Duration::from_millis(30), None, || {
                        panic!("a busy store must not enter caller work")
                    });
                assert_eq!(result, Err(StoreError::StoreBusy));
                assert!(start.elapsed() < Duration::from_secs(1));
                Ok(())
            })
            .unwrap();
            locked(&directory, Duration::from_secs(1), None, || Ok(())).unwrap();
        }
    }
}
