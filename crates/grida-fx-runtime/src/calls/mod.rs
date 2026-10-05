//! Paid calls: the `capability` request (spec/protocol.md §6.1), shared by bodies, the engine's
//! own capability nodes (`executor::capability_node`) and the agent loop's turns (`agent`).
//!
//! [`call`] does, in this order:
//! 1. the declaration and the bound: the capability must be in the instance's resolved `calls`
//!    (`capability_undeclared`: `<type name> calls <capability> without declaring it`), and the
//!    run must have calls left (`over_bound`: `<type name> declared at most <n> <capability>
//!    calls`). Every call that gets past the declaration counts, cache hits included
//!    ([`CallCounter`]).
//! 2. the route: the instance's bound route for the capability (`no_route`: `no route serves
//!    <capability> for <step>`).
//! 3. the request: refused with `-32602` when it holds a reserved marker other than a file value
//!    where spec/protocol.md §3.2 puts one; every `{"file": d}` must name a file this run was
//!    handed ([`crate::engine::RunFiles`], `unknown_file`). The call key is
//!    `store::records::call_key` over the capability, the route's fingerprint, the request and the
//!    instance's take list.
//! 4. the call cache: a trusted call record answers (`cached: true`, `cost_usd` 0); a leftover
//!    job record with the key is removed (spec/store.md §5). Emits `call {cached: true, cost_usd:
//!    0}`.
//! 5. live: a run that is not live refuses with `not_live`: `<capability> on <route id> is a paid
//!    call; run with --live`.
//! 6. the adapter: none serving the route is `no_route`: `no adapter serves <capability> on <route
//!    id>`. This is checked after the cache and the live check, so recorded calls replay offline
//!    and without adapters (spec/protocol.md §6.1 step 4; spec/store.md §4).
//! 7. the job record: unreadable stops the run ([`CallError::Store`]); `submitting` is
//!    `job_unsettled`: `<capability> on <route id> (take <take>) was being submitted when a run
//!    stopped, and nobody can say whether the provider took it. Check the provider's dashboard,
//!    then run grida-fx jobs --forget <key> to submit it again`; `submitted` is collected
//!    (`retry::collect_job`; only a long-job adapter can collect, so a plain adapter meeting a
//!    `submitted` record is `job_unsettled` too); none or `settled` is a new submission.
//! 8. the hold: the route's high price for this request (`Route::cost` over the request's members
//!    as values), reserved per attempt by the retry owner under the run's ceiling and the
//!    instance's step budgets.
//! 9. the retry owner ([`retry`]); then the answer's files are stored, the call record published,
//!    the job record removed, and `call {cached: false, cost_usd}` emitted (`cost_usd` the
//!    attempt's reported cost, `null` when none, 0 for a collected job).
//!
//! Errors carry `capability`, `route` and `key` in `data` when known (spec/protocol.md §7);
//! `ceiling_exceeded` adds `needed_usd` and `remaining_usd`.
//!
//! **What a call was charged.** Every hold the retry owner settles for a call is added up (the
//! ledger's own figures): the answered attempt and any billed attempt before it. That total is
//! [`CallAnswer::charged`], and the run's [`CallCounter`] adds it whenever a call settled
//! anything or was answered without the cache, so the `cost_usd` fact is what the run's paid
//! calls cost (spec/protocol.md §5.3 step 5).
//!
//! The call runs on the calling task: whoever must not wait for it (a run that is being
//! cancelled) spawns it, and the call still completes and settles.

pub mod pacing;
pub mod retry;

use crate::engine::{RunFiles, Services};
use crate::events::Event;
use crate::ledger::{Hold, Refusal, Scopes};
use crate::store::records::{CallRecord, FileEntry, JobRecord, JobState, RouteEntry, call_key};
use grida_fx_core::money::Usd;
use grida_fx_core::routes::Route;
use grida_fx_core::val::{FileValue, Val};
use grida_fx_core::value::is_digest;
use grida_fx_protocol::{ErrorCode, RpcError};
use grida_fx_providers::{Adapter, CallRequest, RequestFile, RouteRef};
use indexmap::IndexMap;
use retry::{Attempts, Backoff, HoldBook, Outcome};
use serde_json::{Map, Value};
use std::sync::Mutex;

/// What the call path needs to know about the instance making the call.
#[derive(Debug, Clone)]
pub struct CallSite {
    pub instance_id: String,
    /// The declared step path, for messages.
    pub step: String,
    /// The type's name, for messages.
    pub type_name: String,
    pub takes: Vec<u32>,
    /// `{capability: bound}`, resolved for this instance.
    pub calls: IndexMap<String, u32>,
    /// Bound routes by capability.
    pub routes: IndexMap<String, Route>,
    /// `{capability: concurrency}` after the project's override (calls::pacing).
    pub limits: IndexMap<String, Option<u32>>,
    /// The step budgets the instance is inside (ledger::Scopes).
    pub scopes: crate::ledger::Scopes,
}

/// How many calls of each capability one body run has made (cache hits included).
#[derive(Debug, Default)]
pub struct CallCounter {
    made: Mutex<IndexMap<String, u32>>,
    /// The sum of what this run's uncached calls were charged; `None` until one was made.
    cost: Mutex<Option<Usd>>,
}

impl CallCounter {
    pub fn new() -> CallCounter {
        CallCounter::default()
    }

    /// The run's paid calls' cost, for the `cost_usd` fact and the result record: `None` when no
    /// uncached call was made.
    pub fn cost(&self) -> Option<Usd> {
        *self.cost.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Counts one call of `capability` when fewer than `bound` were made; `false` when none is
    /// left.
    fn take(&self, capability: &str, bound: u32) -> bool {
        let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
        let count = made.entry(capability.to_string()).or_insert(0);
        if *count >= bound {
            return false;
        }
        *count += 1;
        true
    }

    /// Adds what a call was charged (module doc).
    fn add(&self, charged: Usd) {
        let mut cost = self.cost.lock().unwrap_or_else(|e| e.into_inner());
        *cost = Some(cost.unwrap_or(Usd::ZERO) + charged);
    }
}

/// An answered call (the `capability` result, spec/protocol.md §6.1).
#[derive(Debug, Clone, PartialEq)]
pub struct CallAnswer {
    pub key: String,
    pub cached: bool,
    /// 0 on a hit and for a collected job; `None` when the provider reported no cost.
    pub cost: Option<Usd>,
    /// What the call was charged in all (module doc): every settled attempt; 0 on a hit and for
    /// a collected job.
    pub charged: Usd,
    /// The files, stored, with `location` set.
    pub files: IndexMap<String, FileValue>,
    pub data: Value,
}

/// Why a call failed (spec/protocol.md §7 codes).
#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// An RPC error to answer the host with (every code `-32010`…`-32024`, and `-32602`), its
    /// `data` already filled in.
    Rpc(RpcError),
    /// The run stopped: the request is answered `cancelled`.
    Cancelled,
    /// A fault that stops the run: the store failed (an unreadable job record, a file or record
    /// that cannot be written), or the call's own task ended without an answer.
    Store(String),
}

impl CallError {
    /// The error a host request is answered with.
    pub fn to_rpc(&self) -> RpcError {
        match self {
            CallError::Rpc(error) => error.clone(),
            CallError::Cancelled => RpcError::new(
                grida_fx_protocol::ErrorCode::Cancelled,
                "the run was stopped",
            ),
            CallError::Store(message) => {
                RpcError::new(grida_fx_protocol::ErrorCode::Internal, message.clone())
            }
        }
    }
}

/// Why a value a host sent was refused where file values are allowed ([`file_values`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ValueRefusal {
    /// A reserved marker other than a file value (`-32602`).
    Marker(String),
    /// A file value naming a file the run was not handed (`unknown_file`).
    UnknownFile(String),
}

impl ValueRefusal {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            ValueRefusal::Marker(_) => ErrorCode::InvalidParams,
            ValueRefusal::UnknownFile(_) => ErrorCode::UnknownFile,
        }
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            ValueRefusal::Marker(message) | ValueRefusal::UnknownFile(message) => message,
        }
    }

    pub(crate) fn to_rpc(&self) -> RpcError {
        RpcError::new(self.code(), self.message())
    }
}

/// The files a host value names, in order of first appearance (spec/protocol.md §3.2): every
/// `{"file": d}` must name a file this run was handed; any other reserved marker (spec/identity.md
/// §3) is refused. `where_` names the value in messages (`request`, `variables.who`).
pub(crate) fn file_values(
    value: &Value,
    where_: &str,
    files: &RunFiles,
) -> Result<Vec<FileValue>, ValueRefusal> {
    let mut found = IndexMap::new();
    walk_files(value, where_, files, &mut found)?;
    Ok(found.into_values().collect())
}

/// The digest of a file value: an object whose only member is `file` holding a digest.
pub(crate) fn file_value_digest(map: &Map<String, Value>) -> Option<&str> {
    if map.len() != 1 {
        return None;
    }
    map.get("file")
        .and_then(Value::as_str)
        .filter(|d| is_digest(d))
}

fn walk_files(
    value: &Value,
    where_: &str,
    files: &RunFiles,
    found: &mut IndexMap<String, FileValue>,
) -> Result<(), ValueRefusal> {
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk_files(item, &format!("{where_}[{i}]"), files, found)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            if let Some(digest) = file_value_digest(map) {
                let Some(file) = files.get(digest) else {
                    return Err(ValueRefusal::UnknownFile(format!(
                        "{where_} names the file {digest}, which this run was not handed"
                    )));
                };
                found.entry(digest.to_string()).or_insert(file);
                return Ok(());
            }
            if let Some((key, item)) = map.iter().next().filter(|_| map.len() == 1) {
                let reserved = match key.as_str() {
                    "missing" => item == &Value::Bool(true),
                    "failed" => item.is_string(),
                    "collection" => item.is_array(),
                    "pending" => true,
                    _ => false,
                };
                if reserved {
                    return Err(ValueRefusal::Marker(format!(
                        "{where_}: an object holding only `{key}` in this shape is reserved for FX's own values"
                    )));
                }
            }
            for (key, item) in map {
                walk_files(item, &format!("{where_}.{key}"), files, found)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// What is known about a call so far, for errors' `data`.
struct Known<'a> {
    capability: &'a str,
    route: Option<String>,
    key: Option<String>,
}

impl Known<'_> {
    fn data(&self) -> Map<String, Value> {
        let mut data = Map::new();
        data.insert("capability".into(), Value::from(self.capability));
        if let Some(route) = &self.route {
            data.insert("route".into(), Value::from(route.as_str()));
        }
        if let Some(key) = &self.key {
            data.insert("key".into(), Value::from(key.as_str()));
        }
        data
    }

    fn error(&self, code: ErrorCode, message: impl Into<String>) -> CallError {
        CallError::Rpc(RpcError::new(code, message).with_data(Value::Object(self.data())))
    }
}

/// Adds up what a call's holds were settled at, on top of the run's book.
struct Tally<'a> {
    book: &'a dyn HoldBook,
    charged: Mutex<Option<Usd>>,
}

impl Tally<'_> {
    fn charged(&self) -> Option<Usd> {
        *self.charged.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl HoldBook for Tally<'_> {
    fn reserve(&self, node_id: String, amount: Usd, scopes: &Scopes) -> Result<Hold, Refusal> {
        self.book.reserve(node_id, amount, scopes)
    }

    fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd {
        let charged = self.book.settle(hold, reported);
        let mut total = self.charged.lock().unwrap_or_else(|e| e.into_inner());
        *total = Some(total.unwrap_or(Usd::ZERO) + charged);
        charged
    }
}

/// The take list as messages show it: `1`, `2.1`.
fn take_text(takes: &[u32]) -> String {
    takes
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

/// Makes one paid call (module doc).
pub async fn call(
    services: &Services,
    site: &CallSite,
    counter: &CallCounter,
    files: &RunFiles,
    capability: &str,
    request: Value,
) -> Result<CallAnswer, CallError> {
    let mut known = Known {
        capability,
        route: site.routes.get(capability).map(Route::id),
        key: None,
    };

    // 1. The declaration and the bound.
    let Some(&bound) = site.calls.get(capability) else {
        return Err(known.error(
            ErrorCode::CapabilityUndeclared,
            format!("{} calls {capability} without declaring it", site.type_name),
        ));
    };
    if !counter.take(capability, bound) {
        return Err(known.error(
            ErrorCode::OverBound,
            format!(
                "{} declared at most {bound} {capability} calls",
                site.type_name
            ),
        ));
    }

    // 2. The route.
    let Some(route) = site.routes.get(capability) else {
        return Err(known.error(
            ErrorCode::NoRoute,
            format!("no route serves {capability} for {}", site.step),
        ));
    };
    let route_id = route.id();

    // 3. The request and its key.
    let Value::Object(members) = &request else {
        return Err(known.error(
            ErrorCode::InvalidParams,
            "a capability request is a JSON object",
        ));
    };
    let named = match file_values(&request, "request", files) {
        Ok(named) => named,
        Err(refusal) => return Err(known.error(refusal.code(), refusal.message())),
    };
    let fingerprint = route.fingerprint();
    let key = call_key(capability, &fingerprint, &request, &site.takes);
    known.key = Some(key.clone());
    let store = &services.engine.store;

    // 4. The call cache.
    if let Some(record) = store.load_call(&key) {
        // A leftover job record of an answered call is stale (spec/store.md §5); a failure to
        // remove it is harmless, since the call record answers first.
        let _ = store.remove_job(&key);
        let mut answered = IndexMap::new();
        for (name, entry) in &record.files {
            let file = store
                .file_value(entry)
                .map_err(|e| CallError::Store(e.to_string()))?;
            files.insert(&file);
            answered.insert(name.clone(), file);
        }
        emit_call(
            services,
            site,
            capability,
            &route_id,
            &key,
            true,
            Some(Usd::ZERO),
        );
        return Ok(CallAnswer {
            key,
            cached: true,
            cost: Some(Usd::ZERO),
            charged: Usd::ZERO,
            files: answered,
            data: record.data,
        });
    }

    // 5. Live.
    let ledger = match (services.live(), services.ledger.as_ref()) {
        (true, Some(ledger)) => ledger,
        _ => {
            return Err(known.error(
                ErrorCode::NotLive,
                format!("{capability} on {route_id} is a paid call; run with --live"),
            ));
        }
    };

    // 6. The adapter.
    let route_ref = RouteRef {
        capability: capability.to_string(),
        model: route.model.clone(),
        provider: route.provider.clone(),
        contract: route.contract.clone(),
    };
    let Some(adapter) = services.engine.adapters.serving(&route_ref).cloned() else {
        return Err(known.error(
            ErrorCode::NoRoute,
            format!("no adapter serves {capability} on {route_id}"),
        ));
    };

    // 7. The job record.
    let unsettled_text = format!(
        "{capability} on {route_id} (take {}) was being submitted when a run stopped, and nobody \
         can say whether the provider took it. Check the provider's dashboard, then run grida-fx \
         jobs --forget {key} to submit it again",
        take_text(&site.takes)
    );
    let collect = match store.load_job(&key) {
        Err(error) => return Err(CallError::Store(error.to_string())),
        Ok(None) => None,
        Ok(Some(record)) => match record.state {
            JobState::Settled => None,
            JobState::Submitting => {
                return Err(known.error(ErrorCode::JobUnsettled, unsettled_text));
            }
            JobState::Submitted => match (&adapter, record.handle) {
                (Adapter::Job(_), Some(handle)) => Some(handle),
                _ => return Err(known.error(ErrorCode::JobUnsettled, unsettled_text)),
            },
        },
    };

    // 8. The hold, and what every attempt shares.
    let with: IndexMap<String, Val> = members
        .iter()
        .map(|(name, value)| (name.clone(), Val::from_json(value)))
        .collect();
    let (_, hold) = route.cost(&with);
    let mut request_files = IndexMap::new();
    for file in &named {
        let path = store
            .file_path(&file.digest)
            .map_err(|e| CallError::Store(e.to_string()))?;
        request_files.insert(
            file.digest.clone(),
            RequestFile {
                digest: file.digest.clone(),
                kind: file.kind.clone(),
                size: file.size,
                path,
            },
        );
    }
    let route_entry = RouteEntry {
        id: route_id.clone(),
        fingerprint,
    };
    let job_template = JobRecord {
        key: key.clone(),
        capability: capability.to_string(),
        route: route_entry.clone(),
        request: request.clone(),
        take: site.takes.clone(),
        state: JobState::Submitting,
        handle: None,
    };
    let tally = Tally {
        book: &**ledger,
        charged: Mutex::new(None),
    };
    let hold_name = || services.next_hold(&site.instance_id);
    let attempts = Attempts {
        call: CallRequest {
            route: route_ref,
            request: request.clone(),
            files: request_files,
            take: site.takes.clone(),
            key: key.clone(),
            attempt: 1,
        },
        hold,
        scopes: &site.scopes,
        hold_name: &hold_name,
        // The site's limit (the project's override, else the route's); the route's when the
        // site names none for this capability.
        limit: site
            .limits
            .get(capability)
            .copied()
            .unwrap_or(route.concurrency),
        rpm: route.requests_per_minute,
        book: &tally,
        jobs: &**store,
        pacing: &*services.pacing,
        cancel: &services.cancel,
        backoff: Backoff::default(),
        job: matches!(adapter, Adapter::Job(_)).then_some(job_template),
    };

    // 9. The retry owner.
    let collected = collect.is_some();
    let outcome = match (&adapter, &collect) {
        (Adapter::Job(job), Some(handle)) => {
            retry::collect_job(job.as_ref(), &attempts, handle).await
        }
        (Adapter::Job(job), None) => retry::submit_job(job.as_ref(), &attempts).await,
        (Adapter::Request(plain), _) => retry::send_plain(plain.as_ref(), &attempts).await,
    };
    let charged = tally.charged();
    let ended = match outcome {
        Outcome::Answered { answer, .. } => Ok(answer),
        Outcome::Refused(reason) => Err(known.error(
            ErrorCode::CapabilityRefused,
            format!("{capability} on {route_id} was refused: {reason}"),
        )),
        Outcome::Ceiling(refusal) => {
            let mut data = known.data();
            data.insert("needed_usd".into(), refusal.needed.to_value());
            data.insert("remaining_usd".into(), refusal.remaining.to_value());
            Err(CallError::Rpc(
                RpcError::new(ErrorCode::CeilingExceeded, refusal.message)
                    .with_data(Value::Object(data)),
            ))
        }
        Outcome::Failed(reason) => Err(known.error(
            ErrorCode::CallFailed,
            format!("{capability} on {route_id} failed: {reason}"),
        )),
        Outcome::Unsettled(reason) => {
            let mut data = known.data();
            data.insert("reason".into(), Value::from(reason));
            Err(CallError::Rpc(
                RpcError::new(ErrorCode::JobUnsettled, unsettled_text)
                    .with_data(Value::Object(data)),
            ))
        }
        Outcome::Cancelled => Err(CallError::Cancelled),
        Outcome::Store(error) => Err(CallError::Store(error.to_string())),
    };
    let answer = match ended {
        Ok(answer) => answer,
        Err(error) => {
            if let Some(charged) = charged {
                counter.add(charged);
            }
            return Err(error);
        }
    };

    // The charge stands whatever happens to the answer next.
    let charged = charged.unwrap_or(Usd::ZERO);
    counter.add(charged);

    // Bytes first: the files, then the call record, then the job record goes (spec/store.md §6).
    let mut entries = IndexMap::new();
    let mut answered = IndexMap::new();
    for (name, file) in &answer.files {
        let stored = store
            .put_bytes(&file.bytes)
            .map_err(|e| CallError::Store(e.to_string()))?;
        let entry = FileEntry {
            digest: stored.digest,
            kind: file.kind.clone(),
            name: name.clone(),
            size: stored.size,
            key: None,
        };
        let value = store
            .file_value(&entry)
            .map_err(|e| CallError::Store(e.to_string()))?;
        files.insert(&value);
        answered.insert(name.clone(), value);
        entries.insert(name.clone(), entry);
    }
    let cost = if collected {
        Some(Usd::ZERO)
    } else {
        answer.cost
    };
    let record = CallRecord {
        key: key.clone(),
        capability: capability.to_string(),
        route: route_entry,
        request,
        take: site.takes.clone(),
        files: entries,
        data: answer.data.clone(),
        cost_usd: answer.cost,
    };
    store
        .save_call(&record)
        .map_err(|e| CallError::Store(e.to_string()))?;
    // A job record left behind is answered by the call record from now on (spec/store.md §5).
    let _ = store.remove_job(&key);
    emit_call(services, site, capability, &route_id, &key, false, cost);
    Ok(CallAnswer {
        key,
        cached: false,
        cost,
        charged,
        files: answered,
        data: answer.data,
    })
}

/// Emits `call` (spec/schemas/fx-run-events-v1); nothing while planning. A failed write does
/// not undo a call that is already recorded in the store.
fn emit_call(
    services: &Services,
    site: &CallSite,
    capability: &str,
    route_id: &str,
    key: &str,
    cached: bool,
    cost: Option<Usd>,
) {
    if let Some(events) = &services.events {
        let _ = events.emit(&Event::Call {
            id: site.instance_id.clone(),
            capability: capability.to_string(),
            route: route_id.to_string(),
            call: key.to_string(),
            cached,
            cost_usd: cost,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn digest(n: u8) -> String {
        format!("{:064x}", n)
    }

    fn handed(n: u8) -> FileValue {
        FileValue {
            digest: digest(n),
            kind: "image/png".into(),
            name: format!("f{n}.png"),
            size: 3,
            key: None,
            content: None,
            location: None,
        }
    }

    #[test]
    fn file_values_are_found_in_order_once() {
        let files = RunFiles::new();
        files.insert(&handed(1));
        files.insert(&handed(2));
        let request = json!({
            "prompt": "p",
            "context": [{"file": digest(2)}, {"file": digest(1)}],
            "again": {"deep": {"file": digest(2)}}
        });
        let found = file_values(&request, "request", &files).unwrap();
        let digests: Vec<_> = found.iter().map(|f| f.digest.clone()).collect();
        assert_eq!(digests, vec![digest(2), digest(1)]);
    }

    #[test]
    fn an_unknown_file_is_refused_with_its_place() {
        let files = RunFiles::new();
        let refusal =
            file_values(&json!({"a": [1, {"file": digest(9)}]}), "request", &files).unwrap_err();
        assert_eq!(refusal.code(), ErrorCode::UnknownFile);
        assert_eq!(
            refusal.message(),
            format!(
                "request.a[1] names the file {}, which this run was not handed",
                digest(9)
            )
        );
    }

    #[test]
    fn other_markers_are_refused_and_ordinary_lookalikes_pass() {
        let files = RunFiles::new();
        for (value, member) in [
            (json!({"x": {"missing": true}}), "missing"),
            (json!({"x": [{"failed": "a#1"}]}), "failed"),
            (json!({"x": {"collection": []}}), "collection"),
            (json!({"x": {"pending": null}}), "pending"),
        ] {
            let refusal = file_values(&value, "request", &files).unwrap_err();
            assert_eq!(refusal.code(), ErrorCode::InvalidParams, "{value}");
            assert!(
                refusal.message().contains(&format!("`{member}`")),
                "{}",
                refusal.message()
            );
        }
        let ordinary = json!({
            "a": {"file": "a.png"},
            "b": {"missing": false},
            "c": {"file": digest(1), "other": 1},
            "d": {"failed": 3}
        });
        assert_eq!(file_values(&ordinary, "request", &files).unwrap(), vec![]);
        let refusal = file_values(&json!({"x": {"missing": true}}), "request", &files).unwrap_err();
        assert_eq!(
            refusal.message(),
            "request.x: an object holding only `missing` in this shape is reserved for FX's own values"
        );
    }

    #[test]
    fn the_counter_counts_up_to_the_bound_and_adds_charges() {
        let counter = CallCounter::new();
        assert!(counter.take("image.generate", 2));
        assert!(counter.take("image.generate", 2));
        assert!(!counter.take("image.generate", 2));
        assert!(counter.take("agent.turn", 1));
        assert_eq!(counter.cost(), None);
        counter.add(Usd::ZERO);
        assert_eq!(counter.cost(), Some(Usd::ZERO));
        counter.add(Usd(20_000));
        counter.add(Usd(5));
        assert_eq!(counter.cost(), Some(Usd(20_005)));
    }

    #[test]
    fn takes_read_as_instance_ids_write_them() {
        assert_eq!(take_text(&[1]), "1");
        assert_eq!(take_text(&[2, 1]), "2.1");
    }
}
