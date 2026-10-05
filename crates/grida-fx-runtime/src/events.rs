//! The run record, `events.jsonl` (spec/store.md §8; spec/schemas/fx-run-events-v1.schema.json).
//!
//! Each line is `canon(event)` and one line feed, written whole and flushed before the run goes
//! on, oldest first, never rewritten. The envelope on every event: `kind:
//! "fx-run-events-v1"`, `event`, `invocation_id`, `plan` (the plan digest), `offset_ms`
//! (milliseconds since the [`EventLog`] was opened, rounded down). [`Event`] holds each event's
//! own fields with the schema's names; `to_fields` writes them (an `Option` that is `None` is
//! left out unless the schema requires the member, where it is `null`).
//!
//! Values are encoded as the schema's `encoded` def ([`encode`]): a file `{"file": {digest, kind,
//! name, size, key?}}` (`key` only when non-empty), a collection `{"collection": [[key, enc],
//! …]}`, a list `{"list": [enc, …]}`, null or missing `{"none": true}`, anything else `{"value":
//! plain(v)}` (a failed upstream result is `{"value": {"failed": id}}`). Money is a JSON number
//! of dollars.
//!
//! Reading: [`read_events`] (the runner, resuming) repairs a torn tail: when the file does not end
//! with a line feed it is truncated just after the last one; any other line that is not an I-JSON
//! object is an error naming its line number. [`read_events_tolerant`] (`project`, `inspect`)
//! stops at the first line it cannot read and never writes.

use crate::store::Store;
use crate::store::records::FileEntry;
use grida_fx_core::money::Usd;
use grida_fx_core::val::{Collection, Val};
use grida_fx_core::value::{canon, is_digest, parse_json, sha256_hex};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const EVENTS_KIND: &str = "fx-run-events-v1";

/// One event's own fields (module doc). Names and members follow fx-run-events-v1.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    RunStarted {
        workflow: String,
        resumed: bool,
        ceiling_usd: Option<Usd>,
        charged_usd: Usd,
        estimate_low: Usd,
        estimate_high: Usd,
    },
    PhasePlanned {
        phase: u32,
        steps: usize,
        high_usd: Usd,
    },
    NodeStarted {
        id: String,
        path: String,
        step: String,
        take: Vec<u32>,
        identity: Option<String>,
        uses: String,
        reads: Vec<String>,
        /// `{capability: route id}`.
        routes: IndexMap<String, String>,
        /// `{name: encoded}`.
        with: IndexMap<String, Value>,
    },
    NodeFinished {
        id: String,
        path: String,
        cache_hit: bool,
        outputs: IndexMap<String, Value>,
        facts: IndexMap<String, Value>,
        duration_ms: u64,
    },
    /// From a dispatch (`facts`, `duration_ms` set), or from a run-time assertion state (both
    /// `None`).
    NodeFailed {
        id: String,
        path: String,
        error: Option<String>,
        facts: Option<IndexMap<String, Value>>,
        duration_ms: Option<u64>,
    },
    /// A blocked instance (`reason`, `blocked: true`), or a select with no candidate (`error`,
    /// `facts`, `duration_ms`).
    NodeSkipped {
        id: String,
        path: String,
        reason: Option<String>,
        blocked: bool,
        error: Option<String>,
        facts: Option<IndexMap<String, Value>>,
        duration_ms: Option<u64>,
    },
    NodeRetry {
        id: String,
        attempt: u32,
        error: String,
    },
    Call {
        id: String,
        capability: String,
        route: String,
        call: String,
        cached: bool,
        cost_usd: Option<Usd>,
    },
    BudgetReserved {
        node_id: String,
        amount_usd: Usd,
    },
    BudgetSettled {
        node_id: String,
        charged_usd: Usd,
        reported: bool,
    },
    BudgetRefused {
        node_id: String,
        needed_usd: Usd,
        remaining_usd: Usd,
        ceiling_usd: Option<Usd>,
    },
    Problem {
        where_: String,
        message: String,
    },
    RunFinished {
        ok: bool,
        incomplete: bool,
        stopped: Option<String>,
        charged_usd: Usd,
        failed: Vec<String>,
        outputs: IndexMap<String, Value>,
    },
    RunCancelled {
        reason: String,
        charged_usd: Usd,
    },
}

impl Event {
    /// The `event` member: `run_started`, `node_finished`, ….
    pub fn name(&self) -> &'static str {
        match self {
            Event::RunStarted { .. } => "run_started",
            Event::PhasePlanned { .. } => "phase_planned",
            Event::NodeStarted { .. } => "node_started",
            Event::NodeFinished { .. } => "node_finished",
            Event::NodeFailed { .. } => "node_failed",
            Event::NodeSkipped { .. } => "node_skipped",
            Event::NodeRetry { .. } => "node_retry",
            Event::Call { .. } => "call",
            Event::BudgetReserved { .. } => "budget_reserved",
            Event::BudgetSettled { .. } => "budget_settled",
            Event::BudgetRefused { .. } => "budget_refused",
            Event::Problem { .. } => "problem",
            Event::RunFinished { .. } => "run_finished",
            Event::RunCancelled { .. } => "run_cancelled",
        }
    }

    /// The event's own members (module doc).
    pub fn to_fields(&self) -> Map<String, Value> {
        let mut fields = Fields::default();
        match self {
            Event::RunStarted {
                workflow,
                resumed,
                ceiling_usd,
                charged_usd,
                estimate_low,
                estimate_high,
            } => {
                fields.text("workflow", workflow);
                fields.put("resumed", Value::Bool(*resumed));
                fields.money_or_null("ceiling_usd", *ceiling_usd);
                fields.money("charged_usd", *charged_usd);
                let mut estimate = Map::new();
                estimate.insert("low_usd".into(), estimate_low.to_value());
                estimate.insert("high_usd".into(), estimate_high.to_value());
                fields.put("estimate", Value::Object(estimate));
            }
            Event::PhasePlanned {
                phase,
                steps,
                high_usd,
            } => {
                fields.put("phase", Value::from(*phase));
                fields.put("steps", Value::from(*steps as u64));
                fields.money("high_usd", *high_usd);
            }
            Event::NodeStarted {
                id,
                path,
                step,
                take,
                identity,
                uses,
                reads,
                routes,
                with,
            } => {
                fields.text("id", id);
                fields.text("path", path);
                fields.text("step", step);
                fields.put(
                    "take",
                    Value::Array(take.iter().map(|t| Value::from(*t)).collect()),
                );
                fields.text_or_null("identity", identity.as_deref());
                fields.text("uses", uses);
                fields.put(
                    "reads",
                    Value::Array(reads.iter().map(|r| Value::from(r.as_str())).collect()),
                );
                fields.put(
                    "routes",
                    Value::Object(
                        routes
                            .iter()
                            .map(|(capability, route)| {
                                (capability.clone(), Value::from(route.as_str()))
                            })
                            .collect(),
                    ),
                );
                fields.put("with", object_of(with));
            }
            Event::NodeFinished {
                id,
                path,
                cache_hit,
                outputs,
                facts,
                duration_ms,
            } => {
                fields.text("id", id);
                fields.text("path", path);
                fields.text("cache", if *cache_hit { "hit" } else { "miss" });
                fields.put("outputs", object_of(outputs));
                fields.put("facts", object_of(facts));
                fields.put("duration_ms", Value::from(*duration_ms));
            }
            Event::NodeFailed {
                id,
                path,
                error,
                facts,
                duration_ms,
            } => {
                fields.text("id", id);
                fields.text("path", path);
                // Both forms require `error`; the schema allows null.
                fields.text_or_null("error", error.as_deref());
                if let Some(facts) = facts {
                    fields.put("facts", object_of(facts));
                }
                if let Some(duration_ms) = duration_ms {
                    fields.put("duration_ms", Value::from(*duration_ms));
                }
            }
            Event::NodeSkipped {
                id,
                path,
                reason,
                blocked,
                error,
                facts,
                duration_ms,
            } => {
                fields.text("id", id);
                fields.text("path", path);
                if let Some(reason) = reason {
                    fields.text("reason", reason);
                }
                if *blocked {
                    fields.put("blocked", Value::Bool(true));
                    if let Some(error) = error {
                        fields.text("error", error);
                    }
                } else {
                    // The dispatch form (a select with no candidate) always names its error.
                    fields.text_or_null("error", error.as_deref());
                }
                if let Some(facts) = facts {
                    fields.put("facts", object_of(facts));
                }
                if let Some(duration_ms) = duration_ms {
                    fields.put("duration_ms", Value::from(*duration_ms));
                }
            }
            Event::NodeRetry { id, attempt, error } => {
                fields.text("id", id);
                fields.put("attempt", Value::from(*attempt));
                fields.text("error", error);
            }
            Event::Call {
                id,
                capability,
                route,
                call,
                cached,
                cost_usd,
            } => {
                fields.text("id", id);
                fields.text("capability", capability);
                fields.text("route", route);
                fields.text("call", call);
                fields.put("cached", Value::Bool(*cached));
                fields.money_or_null("cost_usd", *cost_usd);
            }
            Event::BudgetReserved {
                node_id,
                amount_usd,
            } => {
                fields.text("node_id", node_id);
                fields.money("amount_usd", *amount_usd);
            }
            Event::BudgetSettled {
                node_id,
                charged_usd,
                reported,
            } => {
                fields.text("node_id", node_id);
                fields.money("charged_usd", *charged_usd);
                fields.put("reported", Value::Bool(*reported));
            }
            Event::BudgetRefused {
                node_id,
                needed_usd,
                remaining_usd,
                ceiling_usd,
            } => {
                fields.text("node_id", node_id);
                fields.money("needed_usd", *needed_usd);
                fields.money("remaining_usd", *remaining_usd);
                fields.money_or_null("ceiling_usd", *ceiling_usd);
            }
            Event::Problem { where_, message } => {
                fields.text("where", where_);
                fields.text("message", message);
            }
            Event::RunFinished {
                ok,
                incomplete,
                stopped,
                charged_usd,
                failed,
                outputs,
            } => {
                fields.put("ok", Value::Bool(*ok));
                fields.put("incomplete", Value::Bool(*incomplete));
                fields.text_or_null("stopped", stopped.as_deref());
                fields.money("charged_usd", *charged_usd);
                fields.put(
                    "failed",
                    Value::Array(failed.iter().map(|id| Value::from(id.as_str())).collect()),
                );
                fields.put("outputs", object_of(outputs));
            }
            Event::RunCancelled {
                reason,
                charged_usd,
            } => {
                fields.text("reason", reason);
                fields.money("charged_usd", *charged_usd);
            }
        }
        fields.0
    }
}

/// An event's members as they are built.
#[derive(Default)]
struct Fields(Map<String, Value>);

impl Fields {
    fn put(&mut self, name: &str, value: Value) {
        self.0.insert(name.to_string(), value);
    }

    fn text(&mut self, name: &str, text: &str) {
        self.put(name, Value::from(text));
    }

    fn text_or_null(&mut self, name: &str, text: Option<&str>) {
        self.put(name, text.map_or(Value::Null, Value::from));
    }

    /// Money as a JSON number of dollars; a negative amount (never produced) is written as 0,
    /// because the schema allows none.
    fn money(&mut self, name: &str, amount: Usd) {
        self.put(name, Usd(amount.0.max(0)).to_value());
    }

    fn money_or_null(&mut self, name: &str, amount: Option<Usd>) {
        match amount {
            Some(amount) => self.money(name, amount),
            None => self.put(name, Value::Null),
        }
    }
}

fn object_of(members: &IndexMap<String, Value>) -> Value {
    Value::Object(
        members
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    )
}

/// An open `events.jsonl`, shared by every task of an invocation.
pub struct EventLog {
    file: Mutex<File>,
    invocation_id: String,
    plan: String,
    opened: Instant,
}

impl std::fmt::Debug for EventLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventLog")
            .field("invocation_id", &self.invocation_id)
            .field("plan", &self.plan)
            .finish_non_exhaustive()
    }
}

impl EventLog {
    /// Opens `path` for appending (created when missing, readable by its owner only).
    pub fn open(path: &Path, invocation_id: &str, plan: &str) -> std::io::Result<EventLog> {
        let mut options = OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        Ok(EventLog {
            file: Mutex::new(file),
            invocation_id: invocation_id.to_string(),
            plan: plan.to_string(),
            opened: Instant::now(),
        })
    }

    /// Writes one event with its envelope: one `canon` line, flushed.
    pub fn emit(&self, event: &Event) -> std::io::Result<()> {
        let line = self.line(event);
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        // One write of the whole line (O_APPEND), so concurrent tasks never interleave lines.
        file.write_all(line.as_bytes())?;
        file.flush()
    }

    /// The bytes [`EventLog::emit`] writes for `event`: `canon(envelope + fields)` and a line
    /// feed.
    fn line(&self, event: &Event) -> String {
        let mut record = Map::new();
        record.insert("kind".into(), Value::from(EVENTS_KIND));
        record.insert("event".into(), Value::from(event.name()));
        record.insert(
            "invocation_id".into(),
            Value::from(self.invocation_id.as_str()),
        );
        record.insert("plan".into(), Value::from(self.plan.as_str()));
        record.insert("offset_ms".into(), Value::from(self.offset_ms()));
        for (name, value) in event.to_fields() {
            record.entry(name).or_insert(value);
        }
        let mut line = canon(&Value::Object(record));
        line.push('\n');
        line
    }

    /// Milliseconds since the log was opened.
    pub fn offset_ms(&self) -> u64 {
        u64::try_from(self.opened.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    pub fn invocation_id(&self) -> &str {
        &self.invocation_id
    }

    pub fn plan(&self) -> &str {
        &self.plan
    }
}

/// A new invocation id: 16 lowercase hex characters, from the clock, the process id and a
/// counter (not a digest of anything; never part of an identity).
pub fn new_invocation_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = format!("{nanos}:{}:{count}", std::process::id());
    let mut id = sha256_hex(seed.as_bytes());
    id.truncate(16);
    id
}

/// Every event of a run folder's log, oldest first (module doc). A missing file is empty.
pub fn read_events(path: &Path) -> Result<Vec<Value>, String> {
    let mut data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read events.jsonl: {}", reason(&error))),
    };
    if data.last().is_some_and(|last| *last != b'\n') {
        // A torn tail: an invocation stopped in the middle of a line. Drop that line.
        let keep = data
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |at| at + 1);
        OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|file| file.set_len(keep as u64))
            .map_err(|error| format!("cannot repair events.jsonl: {}", reason(&error)))?;
        data.truncate(keep);
    }
    let mut events = Vec::new();
    for (index, line) in data.split(|b| *b == b'\n').enumerate() {
        if is_blank(line) {
            continue;
        }
        match parse_event(line) {
            Some(event) => events.push(event),
            None => return Err(format!("events.jsonl line {} is not an event", index + 1)),
        }
    }
    Ok(events)
}

/// As [`read_events`], but stops at the first unreadable line and never writes.
pub fn read_events_tolerant(path: &Path) -> std::io::Result<Vec<Value>> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    // A last line without its line feed is torn (or still being written): it is not read.
    let whole = data
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |at| at + 1);
    let mut events = Vec::new();
    for line in data[..whole].split(|b| *b == b'\n') {
        if is_blank(line) {
            continue;
        }
        match parse_event(line) {
            Some(event) => events.push(event),
            None => break,
        }
    }
    Ok(events)
}

/// A line of the log as an event: an I-JSON object, or `None`.
fn parse_event(line: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(line).ok()?;
    match parse_json(text) {
        Ok(event @ Value::Object(_)) => Some(event),
        _ => None,
    }
}

fn is_blank(line: &[u8]) -> bool {
    line.iter().all(u8::is_ascii_whitespace)
}

/// The reason of an I/O error, without the path it was about.
fn reason(error: &std::io::Error) -> String {
    match error.kind() {
        ErrorKind::NotFound => "no such file".to_string(),
        ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => error.to_string(),
    }
}

/// A value as the schema's `encoded` def (module doc).
pub fn encode(value: &Val) -> Value {
    match value {
        Val::File(file) => {
            let mut entry = Map::new();
            entry.insert("digest".into(), Value::from(file.digest.as_str()));
            entry.insert("kind".into(), Value::from(file.kind.as_str()));
            entry.insert("name".into(), Value::from(file.name.as_str()));
            entry.insert("size".into(), Value::from(file.size));
            if let Some(key) = file.key.as_deref().filter(|key| !key.is_empty()) {
                entry.insert("key".into(), Value::from(key));
            }
            single("file", Value::Object(entry))
        }
        Val::Collection(collection) => single(
            "collection",
            Value::Array(
                collection
                    .items
                    .iter()
                    .map(|(key, item)| Value::Array(vec![Value::from(key.as_str()), encode(item)]))
                    .collect(),
            ),
        ),
        Val::List(items) => single("list", Value::Array(items.iter().map(encode).collect())),
        Val::Null | Val::Missing => single("none", Value::Bool(true)),
        // A pending value never reaches an event; were it to, its shown form is the most a reader
        // can use.
        other => single("value", other.plain().unwrap_or_else(|| other.shown())),
    }
}

fn single(name: &str, value: Value) -> Value {
    let mut map = Map::new();
    map.insert(name.into(), value);
    Value::Object(map)
}

/// An encoded value back as a runtime value, files read through the store (resuming). `Err` when
/// a file is not present or does not decode. `{"none": true}` reads as null (a missing value is
/// written the same way), and `{"value": …}` as the JSON value it holds.
pub fn decode(value: &Value, store: &Store) -> Result<Val, String> {
    let not_encoded = || "an event holds a value that is not encoded".to_string();
    let map = value
        .as_object()
        .filter(|m| m.len() == 1)
        .ok_or_else(not_encoded)?;
    let (form, inner) = map.iter().next().ok_or_else(not_encoded)?;
    match form.as_str() {
        "file" => {
            let entry = file_entry(inner)?;
            if !store.has(&entry.digest, entry.size) {
                return Err(format!("the file {} is not in the store", entry.digest));
            }
            store
                .file_value(&entry)
                .map(|file| Val::File(Box::new(file)))
                .map_err(|error| error.to_string())
        }
        "collection" => {
            let pairs = inner.as_array().ok_or_else(not_encoded)?;
            let mut items = Vec::with_capacity(pairs.len());
            for pair in pairs {
                match pair.as_array().map(Vec::as_slice) {
                    Some([Value::String(key), item]) => {
                        items.push((key.clone(), decode(item, store)?))
                    }
                    _ => return Err(not_encoded()),
                }
            }
            Ok(Val::Collection(Box::new(Collection {
                items,
                verdicts: IndexMap::new(),
            })))
        }
        "list" => inner
            .as_array()
            .ok_or_else(not_encoded)?
            .iter()
            .map(|item| decode(item, store))
            .collect::<Result<Vec<_>, _>>()
            .map(Val::List),
        "none" if inner == &Value::Bool(true) => Ok(Val::Null),
        "value" => Ok(Val::from_json(inner)),
        _ => Err(not_encoded()),
    }
}

/// The `file` def of an encoded value (`{digest, kind, name, size, key?}`, nothing else).
fn file_entry(value: &Value) -> Result<FileEntry, String> {
    let refused = || "an event names a file it does not describe".to_string();
    let map = value.as_object().ok_or_else(refused)?;
    if map
        .keys()
        .any(|k| !matches!(k.as_str(), "digest" | "kind" | "name" | "size" | "key"))
    {
        return Err(refused());
    }
    let text = |name: &str| map.get(name).and_then(Value::as_str).map(str::to_string);
    let digest = text("digest")
        .filter(|d| is_digest(d))
        .ok_or_else(refused)?;
    let kind = text("kind").ok_or_else(refused)?;
    let name = text("name").ok_or_else(refused)?;
    let size = map
        .get("size")
        .and_then(Value::as_u64)
        .ok_or_else(refused)?;
    let key = match map.get("key") {
        None => None,
        Some(Value::String(key)) => Some(key.clone()).filter(|key| !key.is_empty()),
        Some(_) => return Err(refused()),
    };
    Ok(FileEntry {
        digest,
        kind,
        name,
        size,
        key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::val::FileValue;
    use serde_json::json;

    const DIGEST: &str = "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8";

    fn file(key: Option<&str>) -> Val {
        Val::File(Box::new(FileValue {
            digest: DIGEST.into(),
            kind: "text/plain".into(),
            name: "loud['ada']/text".into(),
            size: 8,
            key: key.map(str::to_string),
            content: None,
            location: None,
        }))
    }

    #[test]
    fn values_encode_as_the_schema_says() {
        assert_eq!(
            encode(&file(None)),
            json!({"file": {"digest": DIGEST, "kind": "text/plain", "name": "loud['ada']/text", "size": 8}})
        );
        assert_eq!(encode(&file(Some("")))["file"].get("key"), None);
        assert_eq!(encode(&file(Some("ada")))["file"]["key"], "ada");
        assert_eq!(encode(&Val::Null), json!({"none": true}));
        assert_eq!(encode(&Val::Missing), json!({"none": true}));
        assert_eq!(
            encode(&Val::Failed("after#1".into())),
            json!({"value": {"failed": "after#1"}})
        );
        assert_eq!(encode(&Val::Number(3.0)), json!({"value": 3}));
        assert_eq!(
            encode(&Val::List(vec![Val::Str("a".into()), file(None)]))["list"][0],
            json!({"value": "a"})
        );
        let collection = Val::Collection(Box::new(Collection {
            items: vec![
                ("ada".into(), file(Some("ada"))),
                ("bo".into(), Val::Bool(true)),
            ],
            verdicts: IndexMap::new(),
        }));
        let encoded = encode(&collection);
        assert_eq!(encoded["collection"][0][0], "ada");
        assert_eq!(encoded["collection"][1], json!(["bo", {"value": true}]));
        let object = Val::Object(IndexMap::from([("a".to_string(), file(None))]));
        assert_eq!(encode(&object), json!({"value": {"a": {"file": DIGEST}}}));
    }

    #[test]
    fn invocation_ids_are_sixteen_hex_and_distinct() {
        let a = new_invocation_id();
        let b = new_invocation_id();
        assert_eq!(a.len(), 16);
        assert!(
            a.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_ne!(a, b);
    }

    #[test]
    fn file_entries_are_read_strictly() {
        let entry = json!({"digest": DIGEST, "kind": "json", "name": "x", "size": 2, "key": "k"});
        let read = file_entry(&entry).unwrap();
        assert_eq!(read.key.as_deref(), Some("k"));
        assert_eq!(read.size, 2);
        for broken in [
            json!({"digest": "abc", "kind": "json", "name": "x", "size": 2}),
            json!({"digest": DIGEST, "kind": "json", "name": "x", "size": -1}),
            json!({"digest": DIGEST, "kind": "json", "name": "x"}),
            json!({"digest": DIGEST, "kind": "json", "name": "x", "size": 2, "extra": 1}),
        ] {
            assert!(file_entry(&broken).is_err(), "{broken}");
        }
    }
}
