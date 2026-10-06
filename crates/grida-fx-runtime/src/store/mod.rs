//! The store (spec/store.md): file bytes by digest, result records by step identity, call records
//! by call key, and job records of long provider jobs.
//!
//! ```text
//! <cache>/files/<d[:2]>/<d>          bytes, written read-only, never rewritten
//! <cache>/results/<i[:2]>/<i>.json   fx-result-record-v1
//! <cache>/calls/<k[:2]>/<k>.json     fx-call-record-v1
//! <cache>/jobs/<k>.json              fx-job-record-v1
//! <cache>/work/                      the engine's scratch (work dirs); not part of the contract
//! ```
//!
//! Rules (store.md §1–§7):
//! - every name is a digest: [`Store::file_path`] and every record path refuse anything that is
//!   not 64 lowercase hex (`not a digest: <text>`);
//! - [`atomic_write`]: a temporary name in the destination folder (never a digest: `.<n>.part`),
//!   write, flush, fsync, optionally read-only, rename over the final name; on any error the
//!   temporary file is removed;
//! - bytes first: a record is written only after every file it names is present; a job record is
//!   removed only after its call record is published;
//! - a present file is never written again (`put_*` checks presence by size first, store.md §2);
//! - records are `canon(record)` (spec/identity.md §2), money as JSON numbers of dollars
//!   (`Usd::to_value`);
//! - trust (§4): [`Store::load_result`] and [`Store::load_call`] return `None` for anything that
//!   fails a check (absent, never an error, never partly used); [`Store::load_job`] returns an
//!   error for a record that exists and cannot be read, because it may stand for a paid
//!   submission. Error texts never hold an absolute path: records are named `jobs/<key>.json`;
//! - nothing here holds a secret or a path in a record (§7).
//!
//! What a killed invocation leaves behind ([`Store::sweep`], at the start of every run): a
//! temporary file it was writing (`.<16 hex>.part` beside a record or a file) is removed once it
//! is an hour old, since one that is being written is touched far more often; a work dir is
//! removed once its invocation no longer runs. An invocation claims its work dirs
//! (`work/<invocation id>-<n>/`) with `work/<invocation id>.lock` ([`Store::claim_work`]), locked
//! for as long as it runs (it is locked before it gets its name, so no sweep sees it unlocked
//! while its invocation runs); the lock ends with the process, so a lock that can be taken
//! belongs to an invocation that is gone. A work dir with no lock file at all (an engine that claimed none)
//! goes once it is an hour old.
//!
//! The read set ([`read_set`]) maps `<with name>/<index>` to the digest of each file the
//! instance's with-values hold, in order (store.md §3).
//!
//! Beyond the letter of store.md, and on purpose:
//! - a call record is trusted only when its key recomputes from its own members (store.md §3
//!   says it MUST); one that does not is absent;
//! - `save_*` refuse a record the matching `load_*` would not read back (a call record whose key
//!   does not recompute included), so a caller's mistake shows at once instead of as a cache
//!   that silently never answers;
//! - a job record under a name other than its own `key` is unreadable;
//! - text content is read for files of at most 1 000 000 bytes, as workflow input files are, and
//!   JSON content obeys the reserved-marker rule (spec/identity.md §3), as JSON input files do.

pub mod records;

use grida_fx_core::error::io_reason;
use grida_fx_core::expand::Instance;
use grida_fx_core::host::ResultCache;
use grida_fx_core::kinds;
use grida_fx_core::text::decode_text;
use grida_fx_core::val::{FileContent, FileValue, Val};
use grida_fx_core::value::{self, is_digest};
use indexmap::IndexMap;
use records::{CallRecord, FileEntry, JobRecord, ResultRecord};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A read set: `<name>/<index>` → file digest, sorted by key.
pub type ReadSet = BTreeMap<String, String>;

/// The largest text file whose content [`Store::file_value`] reads (as for workflow input files).
const TEXT_LIMIT: u64 = 1_000_000;

/// How much of a file is read at a time while hashing or copying.
const CHUNK: usize = 64 * 1024;

/// Why the store could not do something. Texts name store paths relative to the store root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// A name that is not a digest.
    NotADigest(String),
    /// Reading or writing failed: `what` is a store-relative path, `reason` the OS's.
    Io { what: String, reason: String },
    /// A job record exists and cannot be read (spec/store.md §4): the run stops.
    UnreadableJob { key: String, reason: String },
    /// The file to be stored could not be read, or changed while it was read: the fault of
    /// whoever handed it in (a body's output or `file.put`), not the store's. A sentence that
    /// names the file as the caller did.
    Source(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::NotADigest(text) => write!(f, "not a digest: {text}"),
            StoreError::Io { what, reason } => write!(f, "the store's {what}: {reason}"),
            StoreError::Source(sentence) => write!(f, "{sentence}"),
            StoreError::UnreadableJob { key, reason } => {
                write!(f, "the job record jobs/{key}.json is unreadable: {reason}")
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl From<StoreError> for grida_fx_core::Error {
    fn from(error: StoreError) -> Self {
        grida_fx_core::Error::new(grida_fx_core::ErrorKind::Io, error.to_string())
    }
}

/// A file just stored: its digest and size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub digest: String,
    pub size: u64,
}

/// One project's store.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The store at `root` (absolute; usually `Project::cache_dir`). Touches nothing.
    pub fn open(root: &Path) -> Store {
        Store {
            root: root.to_path_buf(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/work`: where the engine makes each run's work dir. Not part of the contract.
    pub fn work_root(&self) -> PathBuf {
        self.root.join("work")
    }

    /// `files/<d[:2]>/<d>`; refuses a name that is not a digest.
    pub fn file_path(&self, digest: &str) -> Result<PathBuf, StoreError> {
        let relative = file_relative(digest)?;
        Ok(self.root.join(relative))
    }

    /// Whether the file is present: it exists with this size (spec/store.md §2).
    pub fn has(&self, digest: &str, size: u64) -> bool {
        self.file_path(digest)
            .ok()
            .and_then(|path| fs::metadata(path).ok())
            .is_some_and(|meta| meta.is_file() && meta.len() == size)
    }

    /// Rehashes a present file: `Ok(true)` when its bytes have this digest. An absent file is
    /// `Ok(false)`.
    pub fn verify(&self, digest: &str) -> Result<bool, StoreError> {
        let path = self.file_path(digest)?;
        let failed = |error: &io::Error| io_error(file_relative_unchecked(digest), error);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(failed(&error)),
        };
        if !file.metadata().map_err(|e| failed(&e))?.is_file() {
            return Ok(false);
        }
        let (found, _) = hash_reader(&mut file).map_err(|e| failed(&e))?;
        Ok(found == digest)
    }

    /// Stores bytes under their digest (no rewrite when present).
    pub fn put_bytes(&self, bytes: &[u8]) -> Result<Stored, StoreError> {
        let digest = value::file_digest(bytes);
        let size = bytes.len() as u64;
        if !self.has(&digest, size) {
            let path = self.file_path(&digest)?;
            atomic_write(&path, bytes, true)
                .map_err(|e| io_error(file_relative_unchecked(&digest), &e))?;
        }
        Ok(Stored { digest, size })
    }

    /// Stores a file's bytes by streaming them (hash while copying into a temporary name, then
    /// rename under the digest). `label` names the source in errors, as the user would.
    ///
    /// The source is hashed first, so a present file is neither copied nor rewritten; otherwise
    /// it is copied into a temporary name in `files/<d[:2]>/` and hashed again on the way, and a
    /// source that changed in between is refused.
    /// A source that cannot be read or that changes while it is copied is
    /// [`StoreError::Source`]; a store that cannot be written is [`StoreError::Io`].
    pub fn put_file(&self, source: &Path, label: &str) -> Result<Stored, StoreError> {
        let unreadable = |error: &io::Error| {
            StoreError::Source(format!("cannot read {label}: {}", io_reason(error)))
        };
        let (digest, size) = File::open(source)
            .and_then(|mut file| hash_reader(&mut file))
            .map_err(|e| unreadable(&e))?;
        if self.has(&digest, size) {
            return Ok(Stored { digest, size });
        }
        let path = self.file_path(&digest)?;
        let relative = file_relative_unchecked(&digest);
        let failed = |error: &io::Error| io_error(relative.clone(), error);
        let folder = parent_of(&path);
        fs::create_dir_all(folder).map_err(|e| failed(&e))?;
        let mut source_file = File::open(source).map_err(|e| unreadable(&e))?;
        let mut temp = TempFile::create(folder).map_err(|e| failed(&e))?;
        let mut hasher = Sha256::new();
        let mut copied = 0u64;
        let mut buffer = vec![0u8; CHUNK];
        loop {
            let n = match source_file.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(unreadable(&error)),
            };
            hasher.update(&buffer[..n]);
            copied += n as u64;
            temp.write_all(&buffer[..n]).map_err(|e| failed(&e))?;
        }
        if hex(&hasher.finalize()) != digest || copied != size {
            return Err(StoreError::Source(format!(
                "{label} changed while it was being stored"
            )));
        }
        temp.publish(&path, true).map_err(|e| failed(&e))?;
        Ok(Stored { digest, size })
    }

    /// Stores a workflow input or project file that is not in the store yet (its `location`),
    /// and returns it with `location` pointing at the store copy.
    pub fn adopt(&self, file: &FileValue) -> Result<FileValue, StoreError> {
        let path = self.file_path(&file.digest)?;
        if !self.has(&file.digest, file.size) {
            let Some(location) = &file.location else {
                return Err(StoreError::Source(format!(
                    "{} has no local copy",
                    file.name
                )));
            };
            let stored = self.put_file(location, &file.name)?;
            if stored.digest != file.digest || stored.size != file.size {
                return Err(StoreError::Source(format!(
                    "{} changed since it was planned",
                    file.name
                )));
            }
        }
        Ok(FileValue {
            location: Some(path),
            ..file.clone()
        })
    }

    /// A stored file as a runtime value: `location` at the store copy; `content` read and decoded
    /// for JSON kinds (`kinds::is_json`, parsed with the strict reader) and text kinds
    /// (`text::decode_text`), as expansion reads step outputs.
    pub fn file_value(&self, entry: &FileEntry) -> Result<FileValue, StoreError> {
        let path = self.file_path(&entry.digest)?;
        let relative = file_relative_unchecked(&entry.digest);
        let read_text = || -> Result<String, StoreError> {
            let bytes = fs::read(&path).map_err(|e| io_error(relative.clone(), &e))?;
            decode_text(&bytes, &entry.name).map_err(|e| StoreError::Io {
                what: relative.clone(),
                reason: e.message,
            })
        };
        let content = if kinds::is_json(&entry.kind) {
            let text = read_text()?;
            let json = value::parse_json(&text).map_err(|refused| StoreError::Io {
                what: relative.clone(),
                reason: format!("{}: {}", entry.name, refused.message),
            })?;
            value::check_markers(&json, &entry.name).map_err(|refused| StoreError::Io {
                what: relative.clone(),
                reason: refused.message,
            })?;
            Some(FileContent::Json(json))
        } else if kinds::is_text(&entry.kind) && entry.size <= TEXT_LIMIT {
            Some(FileContent::Text(read_text()?))
        } else {
            None
        };
        Ok(FileValue {
            digest: entry.digest.clone(),
            kind: entry.kind.clone(),
            name: entry.name.clone(),
            size: entry.size,
            key: entry.key.clone(),
            content,
            location: Some(path),
        })
    }

    /// The trusted result record of `identity` for this read set, or `None` (spec/store.md §4).
    pub fn load_result(&self, identity: &str, read: &ReadSet) -> Option<ResultRecord> {
        let relative = record_relative(Records::Results, identity).ok()?;
        let value = self.read_json(&relative)?;
        let record = ResultRecord::from_value(&value).ok()?;
        let trusted = record.identity == identity
            && &record.read == read
            && record
                .outputs
                .values()
                .flat_map(|output| output.files())
                .all(|file| self.has(&file.digest, file.size));
        trusted.then_some(record)
    }

    /// Publishes a result record (its output files must be present).
    pub fn save_result(&self, record: &ResultRecord) -> Result<(), StoreError> {
        let relative = record_relative(Records::Results, &record.identity)?;
        let value = record.to_value();
        ResultRecord::from_value(&value).map_err(|reason| not_a_record(&relative, reason))?;
        let files = record.outputs.values().flat_map(|output| output.files());
        self.check_present(&relative, files)?;
        self.write_record(&relative, &value, true)
    }

    /// The trusted call record of `key`, or `None` (spec/store.md §4). Its key must also
    /// recompute from its own members (store.md §3).
    pub fn load_call(&self, key: &str) -> Option<CallRecord> {
        let relative = record_relative(Records::Calls, key).ok()?;
        let value = self.read_json(&relative)?;
        let record = CallRecord::from_value(&value).ok()?;
        let trusted = record.key == key
            && record.computed_key() == key
            && record
                .files
                .values()
                .all(|file| self.has(&file.digest, file.size));
        trusted.then_some(record)
    }

    /// Publishes a call record (its files must be present, and its key must recompute).
    pub fn save_call(&self, record: &CallRecord) -> Result<(), StoreError> {
        let relative = record_relative(Records::Calls, &record.key)?;
        let value = record.to_value();
        CallRecord::from_value(&value).map_err(|reason| not_a_record(&relative, reason))?;
        if record.computed_key() != record.key {
            return Err(StoreError::Io {
                what: relative,
                reason: "its key is not the call key of its capability, route, request and take"
                    .into(),
            });
        }
        self.check_present(&relative, record.files.values())?;
        self.write_record(&relative, &value, true)
    }

    /// The job record of `key`: `Ok(None)` when there is none, an error when it cannot be read.
    pub fn load_job(&self, key: &str) -> Result<Option<JobRecord>, StoreError> {
        let relative = record_relative(Records::Jobs, key)?;
        let unreadable = |reason: String| StoreError::UnreadableJob {
            key: key.to_string(),
            reason,
        };
        let bytes = match fs::read(self.root.join(&relative)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unreadable(io_reason(&error))),
        };
        let text =
            std::str::from_utf8(&bytes).map_err(|_| unreadable("it is not UTF-8 text".into()))?;
        let value = value::parse_json(text).map_err(|refused| unreadable(refused.message))?;
        let record = JobRecord::from_value(&value).map_err(unreadable)?;
        if record.key != key {
            return Err(unreadable(format!("it holds the job {}", record.key)));
        }
        Ok(Some(record))
    }

    /// Writes a job record (atomically, replacing the last state).
    pub fn save_job(&self, record: &JobRecord) -> Result<(), StoreError> {
        let relative = record_relative(Records::Jobs, &record.key)?;
        let value = record.to_value();
        JobRecord::from_value(&value).map_err(|reason| not_a_record(&relative, reason))?;
        self.write_record(&relative, &value, false)
    }

    /// Removes a job record; absent is fine.
    pub fn remove_job(&self, key: &str) -> Result<(), StoreError> {
        let relative = record_relative(Records::Jobs, key)?;
        match fs::remove_file(self.root.join(&relative)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(relative, &error)),
        }
    }

    /// Every job record, sorted by key; the first unreadable one is an error (`grida-fx jobs`).
    /// Names under `jobs/` that are not `<digest>.json` are not records and are skipped.
    pub fn jobs(&self) -> Result<Vec<JobRecord>, StoreError> {
        let entries = match fs::read_dir(self.root.join("jobs")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_error("jobs", &error)),
        };
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| io_error("jobs", &e))?;
            let name = entry.file_name();
            let Some(key) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
                continue;
            };
            if is_digest(key) {
                keys.push(key.to_string());
            }
        }
        keys.sort();
        let mut records = Vec::new();
        for key in keys {
            if let Some(record) = self.load_job(&key)? {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// A record's JSON value, or `None` when it is missing or not I-JSON.
    fn read_json(&self, relative: &str) -> Option<Value> {
        let bytes = fs::read(self.root.join(relative)).ok()?;
        let text = std::str::from_utf8(&bytes).ok()?;
        value::parse_json(text).ok()
    }

    /// Bytes first (store.md §6): every file a record names must be present.
    fn check_present<'a>(
        &self,
        relative: &str,
        files: impl IntoIterator<Item = &'a FileEntry>,
    ) -> Result<(), StoreError> {
        for file in files {
            if !self.has(&file.digest, file.size) {
                return Err(StoreError::Io {
                    what: relative.to_string(),
                    reason: format!(
                        "it names {}, which is not in the store",
                        file_relative_unchecked(&file.digest)
                    ),
                });
            }
        }
        Ok(())
    }

    /// Writes `canon(value)` at the store-relative `relative`, atomically.
    fn write_record(
        &self,
        relative: &str,
        value: &Value,
        read_only: bool,
    ) -> Result<(), StoreError> {
        let bytes = value::canon(value);
        atomic_write(&self.root.join(relative), bytes.as_bytes(), read_only)
            .map_err(|e| io_error(relative, &e))
    }
}

/// The result cache planning reads (`plan.cached`): a trusted result record exists for the
/// instance's identity and its read set (spec/store.md §4).
impl ResultCache for Store {
    fn has_result(&self, instance: &Instance) -> bool {
        instance.identity.as_deref().is_some_and(|identity| {
            self.load_result(identity, &read_set(&instance.with))
                .is_some()
        })
    }
}

/// Writes `bytes` to `path` atomically (module doc). Used by the store and the run folder.
pub fn atomic_write(path: &Path, bytes: &[u8], read_only: bool) -> std::io::Result<()> {
    let folder = parent_of(path);
    fs::create_dir_all(folder)?;
    let mut temp = TempFile::create(folder)?;
    temp.write_all(bytes)?;
    temp.publish(path, read_only)
}

/// The read set of an instance's with-values (spec/store.md §3): for each name in order, every
/// file its value holds (a file; a list's items; a collection's items; an object's members, in
/// order), numbered from 0.
pub fn read_set(with: &IndexMap<String, Val>) -> ReadSet {
    let mut set = ReadSet::new();
    for (name, value) in with {
        for (index, file) in files_in(value).into_iter().enumerate() {
            set.insert(format!("{name}/{index}"), file.digest.clone());
        }
    }
    set
}

/// Every file a value holds, in order (lists, collections, objects; spec/store.md §8 "One file or
/// several").
pub fn files_in(value: &Val) -> Vec<&FileValue> {
    let mut files = Vec::new();
    collect_files(value, &mut files);
    files
}

fn collect_files<'a>(value: &'a Val, files: &mut Vec<&'a FileValue>) {
    match value {
        Val::File(file) => files.push(file),
        Val::List(items) => {
            for item in items {
                collect_files(item, files);
            }
        }
        Val::Collection(collection) => {
            for (_, item) in &collection.items {
                collect_files(item, files);
            }
        }
        Val::Object(members) => {
            for item in members.values() {
                collect_files(item, files);
            }
        }
        _ => {}
    }
}

// ------------------------------------------------------------------------ paths

/// The record folders (store.md §1).
#[derive(Debug, Clone, Copy)]
enum Records {
    Results,
    Calls,
    Jobs,
}

/// `files/<d[:2]>/<d>`, refusing a name that is not a digest.
fn file_relative(digest: &str) -> Result<String, StoreError> {
    if !is_digest(digest) {
        return Err(StoreError::NotADigest(digest.to_string()));
    }
    Ok(file_relative_unchecked(digest))
}

/// `files/<d[:2]>/<d>` of a name already checked (or only shown in a message).
fn file_relative_unchecked(digest: &str) -> String {
    let fan = digest.get(..2).unwrap_or(digest);
    format!("files/{fan}/{digest}")
}

/// `results/<i[:2]>/<i>.json`, `calls/<k[:2]>/<k>.json` or `jobs/<k>.json`, refusing a name that
/// is not a digest.
fn record_relative(folder: Records, name: &str) -> Result<String, StoreError> {
    if !is_digest(name) {
        return Err(StoreError::NotADigest(name.to_string()));
    }
    let fan = &name[..2];
    Ok(match folder {
        Records::Results => format!("results/{fan}/{name}.json"),
        Records::Calls => format!("calls/{fan}/{name}.json"),
        Records::Jobs => format!("jobs/{name}.json"),
    })
}

/// The folder a file is written in (`.` for a bare name).
fn parent_of(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn io_error(what: impl Into<String>, error: &io::Error) -> StoreError {
    StoreError::Io {
        what: what.into(),
        reason: io_reason(error),
    }
}

fn not_a_record(relative: &str, reason: String) -> StoreError {
    StoreError::Io {
        what: relative.to_string(),
        reason: format!("not a valid record: {reason}"),
    }
}

// ------------------------------------------------------------------- hashing

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// The SHA-256 (hex) and length of everything a reader yields, streamed.
fn hash_reader(reader: &mut impl Read) -> io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; CHUNK];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buffer[..n]);
                size += n as u64;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok((hex(&hasher.finalize()), size))
}

// ------------------------------------------------------------- temporary files

/// A temporary name in a folder: `.<16 hex>.part`, from the clock, the process, the thread and a
/// counter. It is never a digest, so nothing reads it as a store entry (store.md §6).
fn temp_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seed = format!(
        "{nanos}:{}:{count}:{:?}",
        std::process::id(),
        std::thread::current().id()
    );
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    let digest = hex(&hasher.finalize());
    format!(".{}.part", &digest[..16])
}

/// How old a temporary file or an unclaimed work dir must be before [`Store::sweep`] removes it.
const STALE: Duration = Duration::from_secs(60 * 60);

/// The work claims this process holds, by lock file: each file stays open (and locked) until
/// [`Store::release_work`].
static CLAIMS: std::sync::Mutex<BTreeMap<PathBuf, File>> = std::sync::Mutex::new(BTreeMap::new());

fn claims() -> std::sync::MutexGuard<'static, BTreeMap<PathBuf, File>> {
    CLAIMS.lock().unwrap_or_else(|e| e.into_inner())
}

impl Store {
    /// Claims the work dirs of `invocation_id` (module doc): `work/<invocation id>.lock`, locked
    /// until [`Store::release_work`] (or the process ends). Claiming again is a no-op.
    pub fn claim_work(&self, invocation_id: &str) -> io::Result<()> {
        let work = self.work_root();
        let path = work.join(format!("{invocation_id}.lock"));
        let mut held = claims();
        if held.contains_key(&path) {
            return Ok(());
        }
        fs::create_dir_all(&work)?;
        // Locked under a name of its own, then renamed into place: a sweep never meets a claim
        // that is not locked yet.
        let claiming = work.join(format!(".{invocation_id}.claim"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&claiming)?;
        let locked = file
            .try_lock()
            .map_err(|error| match error {
                fs::TryLockError::WouldBlock => io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("work/{invocation_id}.lock is held by another invocation"),
                ),
                fs::TryLockError::Error(error) => error,
            })
            .and_then(|()| fs::rename(&claiming, &path));
        if let Err(error) = locked {
            let _ = fs::remove_file(&claiming);
            return Err(error);
        }
        held.insert(path, file);
        Ok(())
    }

    /// Ends the claim of `invocation_id` (its work dirs are gone by then): the lock file is
    /// removed and unlocked. A claim never made is a no-op.
    pub fn release_work(&self, invocation_id: &str) {
        let path = self.work_root().join(format!("{invocation_id}.lock"));
        if let Some(file) = claims().remove(&path) {
            let _ = fs::remove_file(&path);
            drop(file);
        }
    }

    /// Removes what killed invocations left behind (module doc). Best effort: what cannot be
    /// read or removed stays.
    pub fn sweep(&self) {
        self.sweep_temporaries(STALE);
        self.sweep_work(STALE);
    }

    /// Removes temporary files older than `stale` beside the store's files and records.
    fn sweep_temporaries(&self, stale: Duration) {
        let mut folders = vec![self.root.join("jobs")];
        for top in ["files", "results", "calls"] {
            if let Ok(fans) = fs::read_dir(self.root.join(top)) {
                folders.extend(
                    fans.flatten()
                        .filter(|fan| fan.file_type().is_ok_and(|kind| kind.is_dir()))
                        .map(|fan| fan.path()),
                );
            }
        }
        for folder in folders {
            let Ok(entries) = fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') && name.ends_with(".part") && older(&entry.path(), stale) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }

    /// Removes the work dirs of invocations that no longer run (module doc).
    fn sweep_work(&self, stale: Duration) {
        let work = self.work_root();
        let Ok(entries) = fs::read_dir(&work) else {
            return;
        };
        let mut claims = Vec::new();
        let mut dirs = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
            if is_file && name.starts_with('.') && name.ends_with(".claim") {
                // A claim its invocation never finished making (one being made is left alone).
                if older(&entry.path(), Duration::from_secs(60))
                    && unlocked(&entry.path()).is_some()
                {
                    let _ = fs::remove_file(entry.path());
                }
                continue;
            }
            match name.strip_suffix(".lock") {
                Some(invocation) if is_file => claims.push(invocation.to_string()),
                _ if entry.file_type().is_ok_and(|kind| kind.is_dir()) => dirs.push(name),
                _ => {}
            }
        }
        let owner = |dir: &str| {
            dir.rsplit_once('-')
                .map(|(invocation, _)| invocation.to_string())
        };
        for invocation in &claims {
            let path = work.join(format!("{invocation}.lock"));
            // Held while the dirs and the file are removed; `None` while its invocation runs.
            let Some(_held) = unlocked(&path) else {
                continue;
            };
            for dir in dirs
                .iter()
                .filter(|dir| owner(dir).as_ref() == Some(invocation))
            {
                remove_tree(&work.join(dir));
            }
            let _ = fs::remove_file(&path);
        }
        for dir in &dirs {
            let claimed = owner(dir).is_some_and(|invocation| claims.contains(&invocation));
            let path = work.join(dir);
            if !claimed && path.exists() && older(&path, stale) {
                remove_tree(&path);
            }
        }
    }
}

/// The file at `path`, locked, when no one else holds its lock (`None` when someone does, or it
/// cannot be opened).
fn unlocked(path: &Path) -> Option<File> {
    let file = OpenOptions::new().read(true).write(true).open(path).ok()?;
    file.try_lock().ok()?;
    Some(file)
}

/// Whether a file was last changed more than `age` ago (unknown times are not old).
fn older(path: &Path, age: Duration) -> bool {
    fs::symlink_metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed > age)
}

/// Removes a folder and everything under it, making read-only folders writable first when a
/// first try fails. Best effort.
pub fn remove_tree(dir: &Path) {
    if fs::remove_dir_all(dir).is_ok() {
        return;
    }
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(meta) = fs::symlink_metadata(&folder) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let mut permissions = meta.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o700);
        }
        #[cfg(not(unix))]
        {
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
        }
        let _ = fs::set_permissions(&folder, permissions);
        if let Ok(entries) = fs::read_dir(&folder) {
            folders.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    let _ = fs::remove_dir_all(dir);
}

/// A file being written under a temporary name; removed when dropped unless published.
struct TempFile {
    path: PathBuf,
    file: Option<File>,
}

impl TempFile {
    /// Creates a new temporary file in `folder` (never one that exists).
    fn create(folder: &Path) -> io::Result<TempFile> {
        for _ in 0..16 {
            let path = folder.join(temp_name());
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(TempFile {
                        path,
                        file: Some(file),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no free temporary name",
        ))
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.write_all(bytes),
            None => Err(io::Error::other("the temporary file is closed")),
        }
    }

    /// Flushes and syncs the bytes, makes the file read-only when asked, and renames it over
    /// `destination`. On any error the temporary file is removed (on drop).
    fn publish(mut self, destination: &Path, read_only: bool) -> io::Result<()> {
        let Some(mut file) = self.file.take() else {
            return Err(io::Error::other("the temporary file is closed"));
        };
        file.flush()?;
        file.sync_all()?;
        if read_only {
            let mut permissions = file.metadata()?.permissions();
            permissions.set_readonly(true);
            file.set_permissions(permissions)?;
        }
        drop(file);
        fs::rename(&self.path, destination)?;
        // Published: nothing is left to remove.
        self.path = PathBuf::new();
        Ok(())
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        // Close before removing, so removal also works where open files cannot be removed.
        drop(self.file.take());
        if !self.path.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_names_are_never_digests_and_differ() {
        let a = temp_name();
        let b = temp_name();
        assert_ne!(a, b);
        for name in [&a, &b] {
            assert!(name.starts_with('.') && name.ends_with(".part"), "{name}");
            assert_eq!(name.len(), 1 + 16 + 5);
            assert!(!is_digest(name));
            assert!(!is_digest(
                name.trim_start_matches('.').trim_end_matches(".part")
            ));
        }
    }

    #[test]
    fn hex_matches_the_core_digest() {
        let mut reader: &[u8] = b"one\ntwo\n";
        let (digest, size) = hash_reader(&mut reader).unwrap();
        assert_eq!(digest, value::file_digest(b"one\ntwo\n"));
        assert_eq!(size, 8);
    }

    #[test]
    fn a_failed_publish_removes_the_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        // A non-empty folder at the final name: the rename fails.
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inside"), b"x").unwrap();
        assert!(atomic_write(&target, b"bytes", true).is_err());
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["target".to_string()]);
    }

    #[test]
    fn a_dropped_temporary_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let mut temp = TempFile::create(dir.path()).unwrap();
        temp.write_all(b"half").unwrap();
        let path = temp.path.clone();
        assert!(path.exists());
        drop(temp);
        assert!(!path.exists());
    }

    #[test]
    fn record_paths_fan_out_except_jobs() {
        let d = "ab".repeat(32);
        assert_eq!(
            record_relative(Records::Results, &d).unwrap(),
            format!("results/ab/{d}.json")
        );
        assert_eq!(
            record_relative(Records::Calls, &d).unwrap(),
            format!("calls/ab/{d}.json")
        );
        assert_eq!(
            record_relative(Records::Jobs, &d).unwrap(),
            format!("jobs/{d}.json")
        );
        assert_eq!(
            record_relative(Records::Jobs, "../x"),
            Err(StoreError::NotADigest("../x".into()))
        );
    }
}
