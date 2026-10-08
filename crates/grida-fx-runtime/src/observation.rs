//! Read-only run observation (spec/observation.md).
//!
//! A snapshot and its cursor describe one captured complete event prefix. Following the
//! cursor returns only subsequently appended events. Cursors are opaque and local to the
//! selected run folder: they contain no paths and cannot be used as execution identities.
//! They verify the acknowledged bytes with SHA-256, so the stateless reader streams
//! that prefix to verify it on each follow request, without reparsing or retaining it.
//! Returned batches are bounded; snapshots intentionally contain the whole readable record.

use crate::events::EVENTS_KIND;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use grida_fx_core::docs::{Schema, validate};
use grida_fx_core::value::{canon, is_digest, parse_json, sha256_hex};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const DEFAULT_LIMIT: usize = 256;
pub const MAX_LIMIT: usize = 1024;
pub const SNAPSHOT_KIND: &str = "fx-run-snapshot-v1";
pub const BATCH_KIND: &str = "fx-run-event-batch-v1";
const CURSOR_VERSION: &str = "fx1";
const MAX_CURSOR_BYTES: usize = 2048;

/// A graph/run plan and exactly the events acknowledged by `cursor`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunSnapshot {
    pub kind: &'static str,
    pub cursor: String,
    pub plan: Value,
    pub events: Vec<Value>,
}

/// Events strictly after the requested cursor, in record order. `has_more` says another
/// complete event was available when this request captured the log's length.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventBatch {
    pub kind: &'static str,
    pub cursor: String,
    pub events: Vec<Value>,
    pub has_more: bool,
}

/// A machine-readable observation failure. Messages omit private paths and I/O details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObservationError {
    pub kind: &'static str,
    pub code: &'static str,
    pub message: String,
}

impl ObservationError {
    fn new(code: &'static str, message: &str) -> Self {
        Self {
            kind: "fx-run-observation-error-v1",
            code,
            message: message.to_string(),
        }
    }

    /// Recommended status for an HTTP adapter; command adapters use a nonzero exit.
    pub fn status_code(&self) -> u16 {
        match self.code {
            "invalid_limit" | "unsupported_version" => 400,
            "invalid_cursor" | "run_changed" => 409,
            _ => 422,
        }
    }
}

impl std::fmt::Display for ObservationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ObservationError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    scope: String,
    offset: u64,
    prefix: String,
}

impl Cursor {
    fn encode(&self) -> String {
        // All fields are scalars; serializing them cannot fail.
        let data = serde_json::to_vec(self).expect("cursor scalars serialize");
        format!("{CURSOR_VERSION}.{}", URL_SAFE_NO_PAD.encode(data))
    }

    fn decode(text: &str) -> Result<Self, ObservationError> {
        if text.len() > MAX_CURSOR_BYTES {
            return Err(invalid_cursor());
        }
        let (version, data) = text.split_once('.').ok_or_else(invalid_cursor)?;
        if version != CURSOR_VERSION {
            return Err(ObservationError::new(
                "unsupported_version",
                "The observation cursor version is unsupported; request a fresh snapshot.",
            ));
        }
        let bytes = URL_SAFE_NO_PAD.decode(data).map_err(|_| invalid_cursor())?;
        let cursor: Self = serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())?;
        if !is_digest(&cursor.scope) || !is_digest(&cursor.prefix) {
            return Err(invalid_cursor());
        }
        Ok(cursor)
    }
}

fn invalid_cursor() -> ObservationError {
    ObservationError::new("invalid_cursor", "The observation cursor is invalid.")
}

fn changed() -> ObservationError {
    ObservationError::new(
        "run_changed",
        "The selected run or acknowledged event prefix changed; request a fresh snapshot.",
    )
}

fn unavailable() -> ObservationError {
    ObservationError::new("unavailable", "The run record is unavailable.")
}

/// Capture the plan and the complete event prefix visible at the start of the read.
/// A missing event log is an empty prefix; no file is created or repaired.
pub fn snapshot(root: &Path) -> Result<RunSnapshot, ObservationError> {
    let record = Record::open(root)?;
    let mut prefix = Prefix::new(record.scope.clone());
    let events = read_events(
        record.log,
        None,
        &mut prefix,
        usize::MAX,
        &record.plan_digest,
    )?
    .0;
    Ok(RunSnapshot {
        kind: SNAPSHOT_KIND,
        cursor: prefix.cursor(),
        plan: record.plan,
        events,
    })
}

/// Read a bounded batch. `None` starts before the first event. Following a cursor validates
/// that its folder, plan and acknowledged bytes still match before returning new events.
pub fn batch(
    root: &Path,
    after: Option<&str>,
    limit: usize,
) -> Result<EventBatch, ObservationError> {
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(ObservationError::new(
            "invalid_limit",
            "The observation batch limit must be between 1 and 1024.",
        ));
    }
    let cursor = after.map(Cursor::decode).transpose()?;
    let record = Record::open(root)?;
    let mut prefix = Prefix::new(record.scope.clone());
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.scope != record.scope)
    {
        return Err(changed());
    }
    let (events, has_more) = read_events(
        record.log,
        cursor.as_ref(),
        &mut prefix,
        limit,
        &record.plan_digest,
    )?;
    Ok(EventBatch {
        kind: BATCH_KIND,
        cursor: prefix.cursor(),
        events,
        has_more,
    })
}

struct Record {
    plan: Value,
    plan_digest: String,
    scope: String,
    log: Option<File>,
}

impl Record {
    fn open(root: &Path) -> Result<Self, ObservationError> {
        let root = root.canonicalize().map_err(|_| unavailable())?;
        let root_metadata = root.metadata().map_err(|_| unavailable())?;
        if !root_metadata.is_dir() {
            return Err(unavailable());
        }
        let plan_path = confined_file(&root, "plan.json")?.ok_or_else(unavailable)?;
        let plan_text = std::fs::read_to_string(plan_path).map_err(|_| unavailable())?;
        let plan = parse_json(&plan_text).map_err(|_| {
            ObservationError::new(
                "invalid_record",
                "The run plan is not a readable JSON object.",
            )
        })?;
        if !plan.is_object() {
            return Err(ObservationError::new(
                "invalid_record",
                "The run plan is not a readable JSON object.",
            ));
        }
        let kind = plan.get("kind").and_then(Value::as_str).ok_or_else(|| {
            ObservationError::new(
                "invalid_record",
                "The run plan has no readable document kind.",
            )
        })?;
        if kind != "fx-graph-v1" {
            return Err(ObservationError::new(
                "unsupported_version",
                "The run plan version is unsupported.",
            ));
        }
        validate(Schema::Graph, &plan, "plan").map_err(|_| {
            ObservationError::new(
                "invalid_record",
                "The run plan does not conform to fx-graph-v1.",
            )
        })?;
        let plan_digest = plan
            .get("plan")
            .and_then(Value::as_str)
            .filter(|digest| is_digest(digest))
            .ok_or_else(|| {
                ObservationError::new("invalid_record", "The run plan has no valid plan digest.")
            })?
            .to_string();
        let scope = scope_digest(&root, &root_metadata, &plan);
        let log = confined_file(&root, "events.jsonl")?
            .map(|path| File::open(path).map_err(|_| unavailable()))
            .transpose()?;
        Ok(Self {
            plan,
            plan_digest,
            scope,
            log,
        })
    }
}

fn confined_file(root: &Path, name: &str) -> Result<Option<PathBuf>, ObservationError> {
    let path = root.join(name);
    match path.canonicalize() {
        Ok(path) if path.starts_with(root) && path.is_file() => Ok(Some(path)),
        Ok(_) => Err(unavailable()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => Err(unavailable()),
    }
}

fn scope_digest(root: &Path, metadata: &Metadata, plan: &Value) -> String {
    let mut hash = Sha256::new();
    hash.update(b"fx-observation-scope-v1\0");
    // Filesystem identity separates empty logs without persisting a path or global run ID.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = root;
        hash.update(metadata.dev().to_le_bytes());
        hash.update(metadata.ino().to_le_bytes());
    }
    #[cfg(not(unix))]
    {
        // The fallback is a one-way binding only; no path bytes appear in any document.
        let _ = metadata;
        hash.update(root.as_os_str().as_encoded_bytes());
    }
    hash.update(canon(plan).as_bytes());
    digest(&hash)
}

struct Prefix {
    scope: String,
    offset: u64,
    hash: Sha256,
}

impl Prefix {
    fn new(scope: String) -> Self {
        Self {
            scope,
            offset: 0,
            hash: Sha256::new(),
        }
    }

    fn acknowledge(&mut self, bytes: &[u8]) {
        self.hash.update(bytes);
        self.offset += bytes.len() as u64;
    }

    fn cursor(&self) -> String {
        Cursor {
            scope: self.scope.clone(),
            offset: self.offset,
            prefix: digest(&self.hash),
        }
        .encode()
    }
}

fn digest(hash: &Sha256) -> String {
    hash.clone()
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_events(
    log: Option<File>,
    cursor: Option<&Cursor>,
    prefix: &mut Prefix,
    limit: usize,
    plan_digest: &str,
) -> Result<(Vec<Value>, bool), ObservationError> {
    let Some(mut log) = log else {
        if cursor.is_some_and(|cursor| cursor.offset != 0 || cursor.prefix != sha256_hex(b"")) {
            return Err(changed());
        }
        return Ok((Vec::new(), false));
    };
    // Capturing length gives this request a finite frontier even while a writer appends.
    let frontier = log.metadata().map_err(|_| unavailable())?.len();
    if let Some(cursor) = cursor {
        if cursor.offset > frontier {
            return Err(changed());
        }
        let mut buffer = [0_u8; 64 * 1024];
        let mut remaining = cursor.offset;
        let mut last = None;
        while remaining > 0 {
            let want = remaining.min(buffer.len() as u64) as usize;
            let read = log.read(&mut buffer[..want]).map_err(|_| unavailable())?;
            if read == 0 {
                return Err(changed());
            }
            prefix.acknowledge(&buffer[..read]);
            remaining -= read as u64;
            last = buffer.get(read - 1).copied();
        }
        if cursor.prefix != digest(&prefix.hash) || last.is_some_and(|last| last != b'\n') {
            return Err(changed());
        }
    }
    log.seek(SeekFrom::Start(prefix.offset))
        .map_err(|_| unavailable())?;
    let remaining = frontier - prefix.offset;
    let mut reader = BufReader::new(log.take(remaining));
    let mut events = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|_| unavailable())?;
        if read == 0 || line.last() != Some(&b'\n') {
            return Ok((events, false));
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            return Err(invalid_record());
        }
        let event = parse_observed_event(&line, plan_digest)?;
        if events.len() == limit {
            return Ok((events, true));
        }
        prefix.acknowledge(&line);
        events.push(event);
    }
}

fn parse_observed_event(line: &[u8], plan_digest: &str) -> Result<Value, ObservationError> {
    let text = std::str::from_utf8(line).map_err(|_| invalid_record())?;
    let event = parse_json(text).map_err(|_| invalid_record())?;
    if !event.is_object()
        || event
            .get("event")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(invalid_record());
    }
    let kind = event
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(invalid_record)?;
    if kind != EVENTS_KIND {
        return Err(ObservationError::new(
            "unsupported_version",
            "The run event version is unsupported.",
        ));
    }
    if event
        .get("invocation_id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
        || event.get("offset_ms").and_then(Value::as_u64).is_none()
        || event.get("plan").and_then(Value::as_str) != Some(plan_digest)
    {
        return Err(invalid_record());
    }
    Ok(event)
}

fn invalid_record() -> ObservationError {
    ObservationError::new(
        "invalid_record",
        "The run contains an unreadable complete event.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn record() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("plan.json"), plan("fixture").to_string()).unwrap();
        root
    }

    fn plan(id: &str) -> Value {
        json!({"kind": "fx-graph-v1", "plan": "a".repeat(64), "workflow": {"id": id, "title": id}, "instances": [], "pending": [], "estimate": {"low_usd": 0, "high_usd": 0, "ceiling_usd": null}})
    }

    fn event(name: &str, invocation: &str) -> Value {
        json!({"kind": EVENTS_KIND, "event": name, "invocation_id": invocation, "plan": "a".repeat(64), "offset_ms": 0})
    }

    fn append(root: &Path, event: &Value) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("events.jsonl"))
            .unwrap();
        writeln!(file, "{}", canon(event)).unwrap();
    }

    #[test]
    fn snapshot_follow_attaches_without_missing_or_repeating_events() {
        let root = record();
        append(root.path(), &event("run_started", "one"));
        let before = snapshot(root.path()).unwrap();
        append(root.path(), &event("node_started", "one"));
        let after = batch(root.path(), Some(&before.cursor), DEFAULT_LIMIT).unwrap();
        assert_eq!(before.events.len(), 1);
        assert_eq!(after.events.len(), 1);
        assert_eq!(after.events[0]["event"], "node_started");
        assert!(
            batch(root.path(), Some(&after.cursor), DEFAULT_LIMIT)
                .unwrap()
                .events
                .is_empty()
        );
        let replay = batch(root.path(), Some(&before.cursor), DEFAULT_LIMIT).unwrap();
        assert_eq!(after, replay);
    }

    #[test]
    fn pagination_preserves_record_order_and_reports_more() {
        let root = record();
        for name in [
            "run_started",
            "node_started",
            "node_finished",
            "run_finished",
        ] {
            append(root.path(), &event(name, "one"));
        }
        let first = batch(root.path(), None, 2).unwrap();
        assert_eq!(first.events.len(), 2);
        assert!(first.has_more);
        let second = batch(root.path(), Some(&first.cursor), 2).unwrap();
        assert_eq!(second.events.len(), 2);
        assert!(!second.has_more);
        let all = snapshot(root.path()).unwrap();
        assert_eq!(
            first
                .events
                .into_iter()
                .chain(second.events)
                .collect::<Vec<_>>(),
            all.events
        );
        assert_eq!(second.cursor, all.cursor);
    }

    #[test]
    fn resumed_invocations_remain_observable_after_a_terminal_event() {
        let root = record();
        append(root.path(), &event("run_started", "one"));
        append(root.path(), &event("run_finished", "one"));
        let finished = snapshot(root.path()).unwrap();
        append(root.path(), &event("run_started", "two"));
        let next = batch(root.path(), Some(&finished.cursor), 1).unwrap();
        assert_eq!(next.events[0]["invocation_id"], "two");
        assert!(!next.has_more);
    }

    #[test]
    fn a_torn_tail_is_not_acknowledged_or_repaired() {
        let root = record();
        append(root.path(), &event("run_started", "one"));
        let path = root.path().join("events.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let pending = canon(&event("node_started", "one"));
        write!(file, "{}", &pending[..pending.len() - 1]).unwrap();
        let length = file.metadata().unwrap().len();
        let partial = snapshot(root.path()).unwrap();
        assert_eq!(partial.events.len(), 1);
        assert_eq!(file.metadata().unwrap().len(), length);
        writeln!(file, "}}").unwrap();
        assert_eq!(
            batch(root.path(), Some(&partial.cursor), 1).unwrap().events[0]["event"],
            "node_started"
        );
    }

    #[test]
    fn empty_logs_have_a_usable_cursor_and_are_scoped_to_the_folder() {
        let root = record();
        let empty = snapshot(root.path()).unwrap();
        assert!(empty.events.is_empty());
        let other = record();
        assert_eq!(
            batch(other.path(), Some(&empty.cursor), 1)
                .unwrap_err()
                .code,
            "run_changed"
        );
        append(root.path(), &event("run_started", "one"));
        assert_eq!(
            batch(root.path(), Some(&empty.cursor), 1)
                .unwrap()
                .events
                .len(),
            1
        );
    }

    #[test]
    fn truncation_replacement_and_prefix_edits_require_a_fresh_snapshot() {
        let root = record();
        let original = event("run_started", "one");
        append(root.path(), &original);
        let old = snapshot(root.path()).unwrap();
        let path = root.path().join("events.jsonl");
        std::fs::write(&path, "").unwrap();
        assert_eq!(
            batch(root.path(), Some(&old.cursor), 1).unwrap_err().code,
            "run_changed"
        );
        append(root.path(), &event("run_started", "two"));
        assert_eq!(
            batch(root.path(), Some(&old.cursor), 1).unwrap_err().code,
            "run_changed"
        );
        std::fs::write(&path, format!("{}\n", canon(&original))).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let at = bytes.windows(3).position(|bytes| bytes == b"one").unwrap();
        bytes[at] = b'x';
        std::fs::write(&path, bytes).unwrap();
        append(root.path(), &event("node_started", "one"));
        assert_eq!(
            batch(root.path(), Some(&old.cursor), 1).unwrap_err().code,
            "run_changed"
        );
    }

    #[test]
    fn changed_plans_are_not_accepted_with_an_old_cursor() {
        let root = record();
        let old = snapshot(root.path()).unwrap();
        std::fs::write(root.path().join("plan.json"), plan("different").to_string()).unwrap();
        assert_eq!(
            batch(root.path(), Some(&old.cursor), 1).unwrap_err().code,
            "run_changed"
        );
    }

    #[test]
    fn future_events_and_fields_are_preserved_and_a_crash_does_not_invent_completion() {
        let root = record();
        let mut value = event("future_event", "one");
        value["future_field"] = json!({"opaque": true});
        append(root.path(), &value);
        let snapshot = snapshot(root.path()).unwrap();
        assert_eq!(snapshot.events, vec![value]);
        assert!(
            batch(root.path(), Some(&snapshot.cursor), 1)
                .unwrap()
                .events
                .is_empty()
        );
    }

    #[test]
    fn malformed_complete_lines_and_unsupported_record_versions_are_errors() {
        let root = record();
        std::fs::write(root.path().join("events.jsonl"), "{bad}\n").unwrap();
        assert_eq!(snapshot(root.path()).unwrap_err().code, "invalid_record");
        std::fs::write(
            root.path().join("events.jsonl"),
            "{\"kind\":\"fx-run-events-v2\",\"event\":\"run_started\"}\n",
        )
        .unwrap();
        assert_eq!(
            snapshot(root.path()).unwrap_err().code,
            "unsupported_version"
        );
    }

    #[test]
    fn complete_blank_lines_never_advance_a_cursor_without_an_event() {
        let root = record();
        append(root.path(), &event("run_started", "one"));
        let acknowledged = snapshot(root.path()).unwrap();
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(root.path().join("events.jsonl"))
            .unwrap();
        writeln!(log, "   ").unwrap();
        let error = batch(root.path(), Some(&acknowledged.cursor), 1).unwrap_err();
        assert_eq!(error.code, "invalid_record");
        assert!(serde_json::to_value(error).unwrap().get("cursor").is_none());
        assert_eq!(snapshot(root.path()).unwrap_err().code, "invalid_record");
        // The legacy tolerant reader retains its separate salvage behavior.
        assert_eq!(
            crate::events::read_events_tolerant(&root.path().join("events.jsonl"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn event_envelopes_require_types_and_the_selected_plan() {
        let root = record();
        for field in ["kind", "invocation_id", "plan", "offset_ms", "event"] {
            let mut value = event("future_event", "one");
            value.as_object_mut().unwrap().remove(field);
            std::fs::write(
                root.path().join("events.jsonl"),
                format!("{}\n", canon(&value)),
            )
            .unwrap();
            assert_eq!(
                snapshot(root.path()).unwrap_err().code,
                "invalid_record",
                "{field}"
            );
        }
        for (field, replacement) in [
            ("kind", json!(1)),
            ("invocation_id", json!(1)),
            ("invocation_id", json!("")),
            ("plan", json!("b".repeat(64))),
            ("offset_ms", json!(-1)),
            ("offset_ms", json!(0.5)),
            ("event", json!(true)),
            ("event", json!("")),
        ] {
            let mut value = event("future_event", "one");
            value[field] = replacement;
            std::fs::write(
                root.path().join("events.jsonl"),
                format!("{}\n", canon(&value)),
            )
            .unwrap();
            assert_eq!(
                snapshot(root.path()).unwrap_err().code,
                "invalid_record",
                "{field}"
            );
        }
    }

    #[test]
    fn invalid_requests_have_structured_path_free_errors() {
        let root = record();
        for limit in [0, MAX_LIMIT + 1] {
            let error = batch(root.path(), None, limit).unwrap_err();
            assert_eq!(error.code, "invalid_limit");
            assert_eq!(error.status_code(), 400);
            assert!(
                !serde_json::to_string(&error)
                    .unwrap()
                    .contains(root.path().to_str().unwrap())
            );
        }
        assert_eq!(
            batch(root.path(), Some("bad"), 1).unwrap_err().code,
            "invalid_cursor"
        );
        assert_eq!(
            batch(root.path(), Some("fx2.e30"), 1).unwrap_err().code,
            "unsupported_version"
        );
        assert_eq!(
            batch(root.path(), Some("fx1.e30"), 1).unwrap_err().code,
            "invalid_cursor"
        );
    }

    #[cfg(unix)]
    #[test]
    fn observation_never_follows_record_files_outside_the_run() {
        let root = record();
        let external = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(external.path(), root.path().join("events.jsonl")).unwrap();
        assert_eq!(snapshot(root.path()).unwrap_err().code, "unavailable");
    }
}
