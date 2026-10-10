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
//! Writing ([`EventLog::emit`]): a line is written whole or not at all. Each line goes out in one
//! write; when the write fails (a full disk can take part of a line), the file is cut back to the
//! length it had before the line, so no later line ever follows a fragment. A log whose cut fails
//! is broken: every later write fails without writing. A line its reader could not read back (a
//! value nested deeper than `value::MAX_DEPTH`) is refused before anything is written. Every
//! failure is kept ([`EventLog::fault`], the first one): the runner stops the run as an engine
//! fault whoever wrote the event, so a run never goes on with a record that lacks a line.
//!
//! Reading: [`read_events`] (the runner, resuming) repairs a torn tail: when the file does not end
//! with a line feed it is truncated just after the last one (an invocation was killed in the
//! middle of a line, or its log broke there; [`read_events_noting`] says when it did); any other
//! line that is not an I-JSON object is an error naming its line number.
//! [`read_events_tolerant`] (`project`, `inspect`) leaves out a torn tail, stops at the first
//! line it cannot read and never writes.
//!
//! Encoding keeps every event readable: a list nested so deep that its per-level encoding would
//! pass `value::MAX_DEPTH` is written once as `{"value": plain}` when it holds no file, which
//! [`decode`] reads back as the same list.

use crate::store::Store;
use crate::store::records::FileEntry;
use grida_fx_core::money::Usd;
use grida_fx_core::val::{Collection, Val};
use grida_fx_core::value::{MAX_DEPTH, canon, is_digest, parse_json, sha256_hex};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const EVENTS_KIND: &str = "fx-run-events-v1";

/// Display-only metadata for an instance, including one that settles before dispatch.
/// It contains no execution values and is never replayed into the scheduler.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeDisplay {
    pub uses: String,
    pub reads: Vec<String>,
    pub ports: Value,
    pub bindings: Vec<grida_fx_core::expand::wiring::Binding>,
    pub needs: Vec<String>,
    pub judges: Option<String>,
}

/// One event's own fields (module doc). Names and members follow fx-run-events-v1.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    CancelRequested {
        source: crate::run_control::CancelSource,
    },
    /// The display snapshot of the current expansion (spec/store.md §8): its scopes, each
    /// instance's interface bindings, its instances other than absent ones (`{id, step, take,
    /// key}`, in expansion order) and its pending repeats.
    ScopesUpdated {
        scopes: Value,
        node_interface_bindings: Value,
        instances: Value,
        pending: Value,
    },
    RunStarted {
        workflow: String,
        /// Optional human name, immutable across this run's invocations.
        name: Option<String>,
        /// Original creation time, UTC RFC3339 milliseconds.
        created_at: String,
        resumed: bool,
        ceiling_usd: Option<Usd>,
        charged_usd: Usd,
        estimate_low: Usd,
        estimate_high: Usd,
        /// A stand-in run (spec/protocol.md §5.7): `stand_in: true`, written only when true.
        stand_in: bool,
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
        /// Display declarations and references; never replayed into execution values.
        ports: Value,
        bindings: Vec<grida_fx_core::expand::wiring::Binding>,
        needs: Vec<String>,
        judges: Option<String>,
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
    /// `None`). `code` is the spec/protocol.md §7 name of the error that failed the node, `None`
    /// (written `null`) when no error code did: a timeout, an assertion.
    NodeFailed {
        id: String,
        path: String,
        error: Option<String>,
        code: Option<String>,
        facts: Option<IndexMap<String, Value>>,
        duration_ms: Option<u64>,
        display: Option<NodeDisplay>,
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
        display: Option<NodeDisplay>,
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
        /// A stand-in answered the call (spec/protocol.md §6.1, "Stand-in answers"): `stand_in:
        /// true`, written only when true.
        stand_in: bool,
    },
    BudgetReserved {
        node_id: String,
        amount_usd: Usd,
        /// The key of the call the hold is for, written before anything is sent, so a paid
        /// answer is named in the log even when its `call` event never got written
        /// (spec/store.md §9, "Pruning"). Absent for a hold of no call.
        call: Option<String>,
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
    /// Reuse a dispatch's recorded declarations in its terminal event without re-expanding it.
    pub(crate) fn node_display(&self) -> Option<NodeDisplay> {
        if let Event::NodeStarted {
            uses,
            reads,
            ports,
            bindings,
            needs,
            judges,
            ..
        } = self
        {
            Some(NodeDisplay {
                uses: uses.clone(),
                reads: reads.clone(),
                ports: ports.clone(),
                bindings: bindings.clone(),
                needs: needs.clone(),
                judges: judges.clone(),
            })
        } else {
            None
        }
    }

    /// The `event` member: `run_started`, `node_finished`, ….
    pub fn name(&self) -> &'static str {
        match self {
            Event::CancelRequested { .. } => "cancel_requested",
            Event::ScopesUpdated { .. } => "scopes_updated",
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
            Event::CancelRequested { source } => {
                fields.text("source", source.as_str());
            }
            Event::ScopesUpdated {
                scopes,
                node_interface_bindings,
                instances,
                pending,
            } => {
                fields.put("scopes", scopes.clone());
                fields.put("node_interface_bindings", node_interface_bindings.clone());
                fields.put("instances", instances.clone());
                fields.put("pending", pending.clone());
            }
            Event::RunStarted {
                workflow,
                name,
                created_at,
                resumed,
                ceiling_usd,
                charged_usd,
                estimate_low,
                estimate_high,
                stand_in,
            } => {
                fields.text("workflow", workflow);
                if let Some(name) = name {
                    fields.text("name", name);
                }
                fields.text("created_at", created_at);
                fields.put("resumed", Value::Bool(*resumed));
                fields.money_or_null("ceiling_usd", *ceiling_usd);
                fields.money("charged_usd", *charged_usd);
                let mut estimate = Map::new();
                estimate.insert("low_usd".into(), estimate_low.to_value());
                estimate.insert("high_usd".into(), estimate_high.to_value());
                fields.put("estimate", Value::Object(estimate));
                if *stand_in {
                    fields.put("stand_in", Value::Bool(true));
                }
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
                ports,
                bindings,
                needs,
                judges,
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
                fields.put("ports", ports.clone());
                fields.put("bindings", serde_json::json!(bindings));
                fields.put("needs", serde_json::json!(needs));
                fields.text_or_null("judges", judges.as_deref());
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
                code,
                facts,
                duration_ms,
                display,
            } => {
                fields.text("id", id);
                fields.text("path", path);
                // Both forms require `error`; the schema allows null.
                fields.text_or_null("error", error.as_deref());
                fields.text_or_null("code", code.as_deref());
                if let Some(facts) = facts {
                    fields.put("facts", object_of(facts));
                }
                if let Some(duration_ms) = duration_ms {
                    fields.put("duration_ms", Value::from(*duration_ms));
                }
                if let Some(display) = display {
                    fields.display(display);
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
                display,
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
                if let Some(display) = display {
                    fields.display(display);
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
                stand_in,
            } => {
                fields.text("id", id);
                fields.text("capability", capability);
                fields.text("route", route);
                fields.text("call", call);
                fields.put("cached", Value::Bool(*cached));
                fields.money_or_null("cost_usd", *cost_usd);
                if *stand_in {
                    fields.put("stand_in", Value::Bool(true));
                }
            }
            Event::BudgetReserved {
                node_id,
                amount_usd,
                call,
            } => {
                fields.text("node_id", node_id);
                fields.money("amount_usd", *amount_usd);
                if let Some(call) = call {
                    fields.text("call", call);
                }
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
    fn display(&mut self, display: &NodeDisplay) {
        self.text("uses", &display.uses);
        self.put("reads", serde_json::json!(display.reads));
        self.put("ports", display.ports.clone());
        self.put("bindings", serde_json::json!(display.bindings));
        self.put("needs", serde_json::json!(display.needs));
        self.text_or_null("judges", display.judges.as_deref());
    }

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
    writer: Mutex<Writer>,
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
        let length = file.metadata()?.len();
        Ok(EventLog {
            writer: Mutex::new(Writer {
                file: Box::new(file),
                length,
                broken: None,
                fault: None,
            }),
            invocation_id: invocation_id.to_string(),
            plan: plan.to_string(),
            opened: Instant::now(),
        })
    }

    /// Writes one event with its envelope: one `canon` line, flushed, whole or not at all
    /// (module doc).
    pub fn emit(&self, event: &Event) -> std::io::Result<()> {
        self.emit_record(event).map(|_| ())
    }

    /// Writes an event and returns the exact flushed record, including its envelope.
    pub fn emit_record(&self, event: &Event) -> std::io::Result<Value> {
        let record = self.record(event);
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(broken) = &writer.broken {
            return Err(std::io::Error::other(broken.clone()));
        }
        if nesting(&record) > MAX_DEPTH {
            let refused = format!(
                "the {} event holds a value nested deeper than {MAX_DEPTH} levels, which the \
                 record cannot hold",
                event.name()
            );
            writer.fault.get_or_insert_with(|| refused.clone());
            return Err(std::io::Error::new(ErrorKind::InvalidData, refused));
        }
        let mut line = canon(&record);
        line.push('\n');
        writer.append(line.as_bytes(), event.name())?;
        Ok(record)
    }

    /// The first event this log could not write, if any (module doc).
    pub fn fault(&self) -> Option<String> {
        let writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.fault.clone()
    }

    /// The record [`EventLog::emit`] writes for `event`: the envelope and the event's fields.
    fn record(&self, event: &Event) -> Value {
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
        Value::Object(record)
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

/// Where a log's lines go: its file (or a stand-in in tests).
trait Sink: Write + Send {
    /// Cuts the sink back to `length` bytes.
    fn set_len(&mut self, length: u64) -> std::io::Result<()>;
}

impl Sink for File {
    fn set_len(&mut self, length: u64) -> std::io::Result<()> {
        File::set_len(self, length)
    }
}

/// The open file of a log and what has been written to it.
struct Writer {
    file: Box<dyn Sink>,
    /// The length of the file after the last line written whole.
    length: u64,
    /// Why nothing can be written any more: a failed write that could not be cut back.
    broken: Option<String>,
    /// The first write that failed (module doc).
    fault: Option<String>,
}

impl Writer {
    /// Appends one line in one write (O_APPEND, so lines never interleave); on failure, cuts the
    /// file back to its length before the line, or marks the log broken when that fails too.
    fn append(&mut self, line: &[u8], name: &str) -> std::io::Result<()> {
        let written = self.file.write_all(line).and_then(|()| self.file.flush());
        match written {
            Ok(()) => {
                self.length += line.len() as u64;
                Ok(())
            }
            Err(error) => {
                let failure = format!("the {name} event could not be written: {}", reason(&error));
                if let Err(cut) = self.file.set_len(self.length) {
                    self.broken = Some(format!(
                        "{failure}, and the part written could not be removed: {}",
                        reason(&cut)
                    ));
                }
                self.fault.get_or_insert(failure);
                Err(error)
            }
        }
    }
}

/// The deepest a node's value (a fact, a mark) may nest to be held anywhere in an event: an
/// event holds a value at most three levels down (`with.<name>.value`), and [`encode`] keeps
/// deeper lists to one level.
pub const VALUE_DEPTH: usize = MAX_DEPTH - 3;

/// Refuses a value nested deeper than [`VALUE_DEPTH`] (`what` names it), which the run's record
/// could not hold.
pub fn check_depth(value: &Value, what: &str) -> Result<(), String> {
    if nesting(value) > VALUE_DEPTH {
        return Err(format!(
            "{what} is nested deeper than {VALUE_DEPTH} levels, which the run's record cannot hold"
        ));
    }
    Ok(())
}

/// How deeply a value nests, counted as `value::parse_json` counts it: a scalar or an empty
/// container at the top is 0, each container around a value adds 1.
pub fn nesting(value: &Value) -> usize {
    // A walk with its own stack: the value may be deeper than a recursion should go.
    let mut deepest = 0;
    let mut stack: Vec<(&Value, usize)> = vec![(value, 0)];
    while let Some((value, depth)) = stack.pop() {
        deepest = deepest.max(depth);
        match value {
            Value::Array(items) => stack.extend(items.iter().map(|item| (item, depth + 1))),
            Value::Object(members) => stack.extend(members.values().map(|item| (item, depth + 1))),
            _ => {}
        }
    }
    deepest
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

/// A wall-clock time in the portable run metadata format. It never enters an identity.
pub fn created_at(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The first start owns a run's name and creation time. Older records use the original
/// plan file's modification time; later invocations cannot rename or reorder the run.
pub fn run_metadata(events: &[Value], legacy_created: SystemTime) -> (Option<String>, String) {
    let first = events
        .iter()
        .find(|event| event.get("event").and_then(Value::as_str) == Some("run_started"));
    let name = first
        .and_then(|event| event.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let timestamp = first
        .and_then(|event| event.get("created_at"))
        .and_then(Value::as_str)
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|time| {
            time.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        })
        .unwrap_or_else(|| created_at(legacy_created));
    (name, timestamp)
}

/// Every event of a run folder's log, oldest first (module doc). A missing file is empty.
pub fn read_events(path: &Path) -> Result<Vec<Value>, String> {
    read_events_noting(path).map(|(events, _)| events)
}

/// [`read_events`], and whether a torn tail was cut off: the bytes after the last line feed,
/// which an invocation began to write and never finished.
pub fn read_events_noting(path: &Path) -> Result<(Vec<Value>, bool), String> {
    let mut data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok((Vec::new(), false)),
        Err(error) => return Err(format!("cannot read events.jsonl: {}", reason(&error))),
    };
    let torn = data.last().is_some_and(|last| *last != b'\n');
    if torn {
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
    Ok((events, torn))
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

/// The deepest a value may nest for [`encode`] to encode its lists level by level: an event
/// holds an encoded value at most three levels down (`with.<name>`, `outputs.<name>`), and each
/// encoded list level takes two (an object and its array).
const ENCODED_LEVELS: usize = (MAX_DEPTH - 3) / 2;

/// A value as the schema's `encoded` def (module doc).
pub fn encode(value: &Val) -> Value {
    if let Val::List(_) = value
        && json_nesting(value).is_some_and(|levels| levels > ENCODED_LEVELS)
    {
        // Encoded once: level by level, the event could not be read back (module doc).
        return single("value", value.shown());
    }
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

/// How deeply a value that is plain JSON nests (a scalar is 0); `None` when it holds anything
/// [`decode`] would not read back from `{"value": plain}` as itself (a file, a collection, a
/// missing, failed or pending value, a view).
fn json_nesting(value: &Val) -> Option<usize> {
    let mut deepest = 0;
    let mut stack: Vec<(&Val, usize)> = vec![(value, 0)];
    while let Some((value, depth)) = stack.pop() {
        deepest = deepest.max(depth);
        match value {
            Val::List(items) => stack.extend(items.iter().map(|item| (item, depth + 1))),
            Val::Object(members) => stack.extend(members.values().map(|item| (item, depth + 1))),
            Val::Null | Val::Bool(_) | Val::Number(_) | Val::Str(_) => {}
            _ => return None,
        }
    }
    Some(deepest)
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
    use std::sync::Arc;

    /// A file that takes at most `room` more bytes: a write that does not fit takes what fits
    /// and then fails, as a full disk does. `cut` says whether cutting back works.
    struct Full {
        bytes: Arc<Mutex<Vec<u8>>>,
        room: usize,
        cut: bool,
    }

    impl Write for Full {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.room == 0 {
                return Err(std::io::Error::other("No space left on device"));
            }
            let n = buf.len().min(self.room);
            self.room -= n;
            self.bytes.lock().unwrap().extend_from_slice(&buf[..n]);
            Ok(n)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Sink for Full {
        fn set_len(&mut self, length: u64) -> std::io::Result<()> {
            if !self.cut {
                return Err(std::io::Error::other("Input/output error"));
            }
            let mut bytes = self.bytes.lock().unwrap();
            let freed = bytes.len() - length as usize;
            bytes.truncate(length as usize);
            self.room += freed;
            Ok(())
        }
    }

    fn log_on(room: usize, cut: bool) -> (EventLog, Arc<Mutex<Vec<u8>>>) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let sink = Full {
            bytes: Arc::clone(&bytes),
            room,
            cut,
        };
        let log = EventLog {
            writer: Mutex::new(Writer {
                file: Box::new(sink),
                length: 0,
                broken: None,
                fault: None,
            }),
            invocation_id: "0123456789abcdef".into(),
            plan: "a".repeat(64),
            opened: Instant::now(),
        };
        (log, bytes)
    }

    fn problem(message: &str) -> Event {
        Event::Problem {
            where_: "case".into(),
            message: message.into(),
        }
    }

    #[test]
    fn cancellation_persistence_failure_closes_admission_without_acceptance() {
        let (log, bytes) = log_on(0, true);
        let cancel = crate::engine::Cancel::new();
        let control = crate::run_control::RunControl::new(Arc::new(log), cancel.clone());
        assert_eq!(
            control.request_cancel(crate::run_control::CancelSource::Cli),
            crate::run_control::CancelAcceptance::RecordError,
        );
        assert!(control.admit().is_none());
        assert!(cancel.is_cancelled());
        assert!(!control.cancellation_requested());
        assert!(bytes.lock().unwrap().is_empty());
    }

    #[test]
    fn a_line_that_does_not_fit_leaves_nothing_behind() {
        let (log, bytes) = log_on(500, true);
        log.emit(&problem("first")).unwrap();
        let whole = bytes.lock().unwrap().len();
        assert!(whole < 250, "{whole}");
        // Part of this line fits; the part is cut back off.
        let error = log.emit(&problem(&"z".repeat(1000))).unwrap_err();
        assert!(error.to_string().contains("No space"), "{error}");
        assert_eq!(bytes.lock().unwrap().len(), whole);
        assert!(
            log.fault()
                .unwrap()
                .contains("the problem event could not be written")
        );
        // Later lines follow whole lines only.
        log.emit(&problem("later")).unwrap();
        let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(
            lines
                .iter()
                .all(|line| parse_event(line.as_bytes()).is_some())
        );
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn a_log_that_cannot_be_cut_back_writes_nothing_more() {
        let (log, bytes) = log_on(300, false);
        log.emit(&problem("first")).unwrap();
        assert!(log.emit(&problem(&"z".repeat(1000))).is_err());
        let torn = bytes.lock().unwrap().len();
        let error = log.emit(&problem("later")).unwrap_err();
        assert!(
            error.to_string().contains("could not be removed"),
            "{error}"
        );
        assert_eq!(
            bytes.lock().unwrap().len(),
            torn,
            "nothing follows the fragment"
        );
        assert!(log.fault().is_some());
    }

    #[test]
    fn an_event_its_reader_could_not_read_is_never_written() {
        let (log, bytes) = log_on(usize::MAX, true);
        let mut deep = json!(1);
        for _ in 0..MAX_DEPTH {
            deep = json!([deep]);
        }
        let event = Event::NodeFinished {
            id: "a#1".into(),
            path: "a".into(),
            cache_hit: false,
            outputs: IndexMap::new(),
            facts: IndexMap::from([("x".to_string(), deep)]),
            duration_ms: 1,
        };
        let error = log.emit(&event).unwrap_err();
        assert!(
            error.to_string().contains("nested deeper than 512"),
            "{error}"
        );
        assert!(bytes.lock().unwrap().is_empty());
        assert!(log.fault().is_some());
    }

    #[test]
    fn deep_lists_are_encoded_once_and_read_back() {
        let mut deep = Val::Str("x".into());
        for _ in 0..300 {
            deep = Val::List(vec![deep, Val::Null]);
        }
        let encoded = encode(&deep);
        assert!(encoded.get("value").is_some());
        let mut event = Map::new();
        event.insert("with".into(), json!({"v": encoded.clone()}));
        assert!(nesting(&Value::Object(event)) <= MAX_DEPTH);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        assert_eq!(decode(&encoded, &store).unwrap(), deep);
        // Shallow lists keep the schema's list form.
        let shallow = Val::List(vec![Val::List(vec![Val::Number(1.0)])]);
        assert_eq!(
            encode(&shallow),
            json!({"list": [{"list": [{"value": 1}]}]})
        );
        assert_eq!(nesting(&json!(1)), 0);
        assert_eq!(nesting(&json!([[], {"a": [1]}])), 3);
    }

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
