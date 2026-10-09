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
//!
//!    Then the key's flight in the invocation ([`flights`]): a key that already ended for good in
//!    this invocation answers with that error and sends nothing; a key in flight is not called
//!    again, and its caller gets the leader's outcome as a cache hit would give it (`cached:
//!    true`, `cost_usd` 0, the `call` event too), or its error. Otherwise the caller leads, on to
//!    step 4.
//! 4. the call cache: a trusted call record answers (`cached: true`, `cost_usd` 0); a leftover
//!    job record with the key is removed (spec/store.md §5). Emits `call {cached: true, cost_usd:
//!    0}`.
//! 5. live: a run that is not live refuses with `not_live`: `<capability> on <route id> is a paid
//!    call; run with --live`.
//! 6. the adapter: none serving the route is `no_route`: `no adapter serves <capability> on <route
//!    id>`. This is checked after the cache and the live check, so recorded calls replay offline
//!    and without adapters (spec/protocol.md §6.1 step 4; spec/store.md §4).
//! 7. the job record: unreadable stops the run ([`CallError::Fault`]); `submitting` is
//!    `job_unsettled`: `<capability> on <route id> (take <take>) was being submitted when a run
//!    stopped, and nobody can say whether the provider took it. Check the provider's dashboard,
//!    then run grida-fx jobs --forget <key> to submit it again`, with the record's `note` (why
//!    the submit's outcome is unknown, spec/store.md §5) as `reason` in `data` when it has one;
//!    `submitted` is collected (`retry::collect_job`; only a long-job adapter can collect, so a
//!    plain adapter meeting a `submitted` record is `job_unsettled` too); none or `settled` is a
//!    new submission. A submit of this run whose outcome is unknown is `job_unsettled` with the
//!    same message and the adapter's redacted reason as `reason` in `data`.
//! 8. the hold: the route's high price for this request (`Route::cost` over the request's members
//!    as values), reserved per attempt by the retry owner under the run's ceiling and the
//!    instance's step budgets.
//! 9. the retry owner ([`retry`]), with the engine's own check of the capability's answers
//!    ([`engine_check`]: an `agent.turn` reply must be `{"text", "tool_calls"}`, spec/protocol.md
//!    §6.2); then the answer's files are stored, the call record published, the job record
//!    removed, and `call {cached: false, cost_usd}` emitted (`cost_usd` the attempt's reported
//!    cost, `null` when none, 0 for a collected job). The answer handed back is the published
//!    record read back as a cache hit reads it (its `data` canonical, its files in the record's
//!    order), so a live call and its replay give the caller the same value.
//!
//! **Stand-in answers** (spec/protocol.md §6.1 "Stand-in answers"). In a run whose engine has a
//! stand-in ([`crate::stand_in`]), step 4's cache is the stand-in store (the engine's store), and
//! a miss goes to the stand-in in place of steps 5 to 9: nothing is reserved, held, settled,
//! paced or retried, and no job record is read or written.
//! 1. a request a capability of spec/capabilities.md refuses is `capability_refused`,
//!    `<capability> on <route id> was refused: <reason>`, and the stand-in is not asked;
//! 2. `stand_in.answer {capability, route, request, take, key, instance: {id, path, step}, files}`
//!    (`files`: a file ref for each file the request names, `executor::stage::file_ref`), waited
//!    for until the calling instance is cancelled ([`CallSite::cancel`]: `cancelled`, nothing
//!    recorded). A decline is `not_live`, `<capability> on <route id> is a paid call the stand-in
//!    declined`, which does not end the key; the stand-in's `capability_refused` and `call_failed`
//!    are the call's, `<capability> on <route id> was refused: <message>` and `… failed:
//!    <message>`; a fault of the stand-in is [`CallError::Fault`] with its sentence, which stops
//!    the run;
//! 3. the answer's shape and the route's check (`StandInChecks::answer`), the round trip of its
//!    data through canonical JSON, and [`engine_check`]: the first refusal is `call_failed`,
//!    `<capability> on <route id> failed: <reason>`;
//! 4. its files and its call record (`cost_usd` 0) are stored, the run's counter adds 0, and
//!    `call {cached: false, cost_usd: 0, stand_in: true}` is emitted; the caller gets the record as
//!    a replay reads it.
//!
//! A leader whose instance was cancelled lands `cancelled`, which ends nothing: each of its
//! followers leads in its place and asks again.
//!
//! A reservation or a settlement the run's log could not record ([`crate::ledger`]) is a fault
//! ([`CallError::Fault`]) that stops the run; an answer that was paid for is still published
//! first, so a resumed run replays it.
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
//! cancelled, a step that ran past its timeout) spawns it, and the call still completes and
//! settles. Every call counts as running until it returns ([`flights::Flights::track`]), and
//! [`Services::calls_settled`] waits until none is: a run waits for it before it ends.

pub mod flights;
pub mod pacing;
pub mod retry;

use crate::agent::AGENT_TURN;
use crate::engine::{Cancel, RunFiles, Services};
use crate::events::Event;
use crate::executor::stage::file_ref;
use crate::ledger::{Hold, Ledger, NotReserved, Scopes};
use crate::stand_in::{AnswererError, Reply, StandIn};
use crate::store::Store;
use crate::store::records::{CallRecord, FileEntry, JobRecord, JobState, RouteEntry, call_key};
use flights::{Joined, Landed, Tracked};
use grida_fx_core::money::Usd;
use grida_fx_core::routes::Route;
use grida_fx_core::val::{FileValue, Val};
use grida_fx_core::value::is_digest;
use grida_fx_protocol::{ErrorCode, RpcError, StandInAnswerParams, StandInInstance, StandInRoute};
use grida_fx_providers::{Adapter, Answer, CallRequest, RequestFile, RouteRef};
use indexmap::IndexMap;
use retry::{AnswerCheck, Attempts, Backoff, HoldBook, Outcome};
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex};

impl Services {
    /// How many paid calls of this invocation are running now (followers of a call in flight
    /// included).
    pub fn calls_running(&self) -> usize {
        self.pacing.flights().running()
    }

    /// Resolves once no paid call of this invocation is running: every hold its calls opened is
    /// settled, every answer they were paid is stored, and their events are written. A run awaits
    /// it before `run_finished` and `run_cancelled` (after cancelling the invocation when it
    /// stops, so calls in flight end at once and settle their holds in full).
    pub async fn calls_settled(&self) {
        self.pacing.flights().settled().await;
    }

    /// Counts a call as running before it is spawned onto a task of its own, so a wait that
    /// starts before the task first runs still sees it ([`flights::Flights::track`]).
    pub fn track_call(&self) -> Tracked {
        self.pacing.flights().track()
    }
}

/// What the call path needs to know about the instance making the call.
#[derive(Debug, Clone)]
pub struct CallSite {
    pub instance_id: String,
    /// The instance's step path with its repeat keys (a stand-in's `instance.path`).
    pub path: String,
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
    /// The calling instance's cancellation: its timeout, or a stopped run. A stand-in's answer
    /// is waited for only until it fires (module doc, "Stand-in answers").
    pub cancel: Cancel,
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
    /// that cannot be written), the call's own task ended without an answer, or a stand-in
    /// failed (spec/protocol.md §5.7: the message is the run's `stopped` sentence).
    Fault(String),
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
            CallError::Fault(message) => {
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

/// Adds up what a call's holds were settled at, on top of the run's ledger, and keeps the first
/// reservation or settlement of the call that the run's log could not record.
struct Tally<'a> {
    ledger: &'a Ledger,
    charged: Mutex<Option<Usd>>,
    unrecorded: Mutex<Option<String>>,
}

impl Tally<'_> {
    fn charged(&self) -> Option<Usd> {
        *self.charged.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What this call could not record, if anything.
    fn own_fault(&self) -> Option<String> {
        self.unrecorded
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Keeps why a reservation could not be recorded.
    fn reserved(&self, reserved: Result<Hold, NotReserved>) -> Result<Hold, NotReserved> {
        if let Err(NotReserved::Unrecorded(reason)) = &reserved {
            self.keep(reason);
        }
        reserved
    }

    fn keep(&self, reason: &str) {
        self.unrecorded
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(|| reason.to_string());
    }
}

impl HoldBook for Tally<'_> {
    fn reserve(&self, node_id: String, amount: Usd, scopes: &Scopes) -> Result<Hold, NotReserved> {
        self.reserved(self.ledger.reserve_recorded(node_id, amount, scopes))
    }

    fn reserve_call(
        &self,
        node_id: String,
        amount: Usd,
        scopes: &Scopes,
        call: &str,
    ) -> Result<Hold, NotReserved> {
        self.reserved(self.ledger.reserve_recorded_for(
            node_id,
            amount,
            scopes,
            Some(call.to_string()),
        ))
    }

    fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd {
        let charged = match self.ledger.settle_recorded(hold, reported) {
            Ok(charged) => charged,
            Err(unrecorded) => {
                self.keep(&unrecorded.reason);
                unrecorded.charged
            }
        };
        let mut total = self.charged.lock().unwrap_or_else(|e| e.into_inner());
        *total = Some(total.unwrap_or(Usd::ZERO) + charged);
        charged
    }

    fn unrecorded(&self) -> Option<String> {
        self.own_fault().or_else(|| self.ledger.fault())
    }
}

/// The engine's own check of a capability's answers (module doc, step 9), if it has one.
pub fn engine_check(capability: &str) -> Option<&'static AnswerCheck<'static>> {
    (capability == AGENT_TURN).then_some(&turn_reply as &AnswerCheck<'static>)
}

/// An `agent.turn` answer is `{"text", "tool_calls"}` (spec/protocol.md §6.2).
fn turn_reply(answer: &Answer) -> Result<(), String> {
    crate::agent::check_reply(&answer.data)
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
    let flights = Arc::clone(services.pacing.flights());
    let _running = flights.track();
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
    if !request.is_object() {
        return Err(known.error(
            ErrorCode::InvalidParams,
            "a capability request is a JSON object",
        ));
    }
    let named = match file_values(&request, "request", files) {
        Ok(named) => named,
        Err(refusal) => return Err(known.error(refusal.code(), refusal.message())),
    };
    let key = call_key(capability, &route.fingerprint(), &request, &site.takes);
    known.key = Some(key.clone());

    // 3, then: the key's flight in the invocation.
    loop {
        match flights.join(&key, &site.instance_id) {
            Joined::Ended(error) => return Err(CallError::Rpc(error)),
            Joined::Follow(mut landed) => {
                let outcome: Option<Landed> = match landed.wait_for(Option::is_some).await {
                    Ok(outcome) => outcome.clone(),
                    // The leader went away without an outcome: lead in its place.
                    Err(_) => continue,
                };
                let Some(outcome) = outcome else { continue };
                match &*outcome {
                    Ok(answer) => {
                        return Ok(followed(
                            services, site, files, capability, &route_id, answer,
                        ));
                    }
                    // A ceiling refused the leader's instance, not necessarily this one: ask
                    // again (a rerun of that instance is answered by the ended key).
                    Err(CallError::Rpc(error))
                        if error.kind() == Some(ErrorCode::CeilingExceeded) =>
                    {
                        continue;
                    }
                    // The leader's instance was cancelled, not this one: lead in its place.
                    Err(CallError::Cancelled) => continue,
                    Err(error) => return Err(error.clone()),
                }
            }
            Joined::Lead(leader) => {
                let lead = Lead {
                    services,
                    site,
                    counter,
                    files,
                    capability,
                    route,
                    named: &named,
                    key: &key,
                };
                let outcome = lead.call(request).await;
                leader.land(&outcome);
                return outcome;
            }
        }
    }
}

/// The answer a follower of a call in flight gets: the leader's, as a cache hit gives it
/// (module doc, step 3).
fn followed(
    services: &Services,
    site: &CallSite,
    files: &RunFiles,
    capability: &str,
    route_id: &str,
    answer: &CallAnswer,
) -> CallAnswer {
    for file in answer.files.values() {
        files.insert(file);
    }
    emit_call(
        services,
        site,
        capability,
        route_id,
        &answer.key,
        true,
        Some(Usd::ZERO),
        false,
    );
    CallAnswer {
        key: answer.key.clone(),
        cached: true,
        cost: Some(Usd::ZERO),
        charged: Usd::ZERO,
        files: answer.files.clone(),
        data: answer.data.clone(),
    }
}

/// What the leader of a key's flight makes its call with (steps 4 to 9).
struct Lead<'a> {
    services: &'a Services,
    site: &'a CallSite,
    counter: &'a CallCounter,
    files: &'a RunFiles,
    capability: &'a str,
    route: &'a Route,
    named: &'a [FileValue],
    key: &'a str,
}

/// A record's files and data as the caller gets them, its files handed to the run.
fn from_record(
    store: &Store,
    files: &RunFiles,
    record: &CallRecord,
) -> Result<(IndexMap<String, FileValue>, Value), CallError> {
    let mut answered = IndexMap::new();
    for (name, entry) in &record.files {
        let file = store
            .file_value(entry)
            .map_err(|e| CallError::Fault(e.to_string()))?;
        files.insert(&file);
        answered.insert(name.clone(), file);
    }
    Ok((answered, record.data.clone()))
}

/// A record as the store gives it back: written in canonical form and read again.
fn replayed(record: &CallRecord) -> Result<CallRecord, CallError> {
    grida_fx_core::value::parse_json(&grida_fx_core::value::canon(&record.to_value()))
        .map_err(|refused| refused.message)
        .and_then(|value| CallRecord::from_value(&value))
        .map_err(|reason| CallError::Fault(format!("a call record cannot be read back: {reason}")))
}

impl Lead<'_> {
    /// Steps 4 to 9 (module doc).
    async fn call(&self, request: Value) -> Result<CallAnswer, CallError> {
        let Lead {
            services,
            site,
            counter,
            files,
            capability,
            route,
            named,
            key,
        } = *self;
        let key = key.to_string();
        let route_id = route.id();
        let known = Known {
            capability,
            route: Some(route_id.clone()),
            key: Some(key.clone()),
        };
        let store = &services.engine.store;

        // 4. The call cache.
        if let Some(record) = store.load_call(&key) {
            let (answered, data) = from_record(store, files, &record)?;
            let named = emit_call(
                services,
                site,
                capability,
                &route_id,
                &key,
                true,
                Some(Usd::ZERO),
                false,
            );
            // A leftover job record of an answered call is stale (spec/store.md §5); a failure to
            // remove it is harmless, since the call record answers first. It goes only once the
            // log names the call: until then it is what keeps the answer from a prune (§9).
            if named {
                let _ = store.remove_job(&key);
            }
            return Ok(CallAnswer {
                key,
                cached: true,
                cost: Some(Usd::ZERO),
                charged: Usd::ZERO,
                files: answered,
                data,
            });
        }

        let route_ref = RouteRef {
            capability: capability.to_string(),
            model: route.model.clone(),
            provider: route.provider.clone(),
            contract: route.contract.clone(),
        };

        // 4, in a stand-in run: the stand-in answers, refuses or fails the call; a decline goes
        // on to the live check, which a stand-in run never passes.
        if let Some(stand_in) = services
            .engine
            .stand_in
            .as_ref()
            .filter(|_| services.ledger.is_some())
        {
            if let Some(ended) = self.stand_in(stand_in, &request, &route_ref, &known).await {
                return ended;
            }
            return Err(known.error(
                ErrorCode::NotLive,
                format!("{capability} on {route_id} is a paid call the stand-in declined"),
            ));
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
        let Some(adapter) = services.engine.adapters.serving(&route_ref).cloned() else {
            return Err(known.error(
                ErrorCode::NoRoute,
                format!("no adapter serves {capability} on {route_id}"),
            ));
        };

        // 7. The job record.
        let unsettled_text = format!(
            "{capability} on {route_id} (take {}) was being submitted when a run stopped, and \
             nobody can say whether the provider took it. Check the provider's dashboard, then \
             run grida-fx jobs --forget {key} to submit it again",
            take_text(&site.takes)
        );
        let collect = match store.load_job(&key) {
            Err(error) => return Err(CallError::Fault(error.to_string())),
            Ok(None) => None,
            Ok(Some(record)) => match record.state {
                JobState::Settled => None,
                JobState::Submitting => {
                    let mut data = known.data();
                    if let Some(note) = record.note {
                        data.insert("reason".into(), Value::from(note));
                    }
                    return Err(CallError::Rpc(
                        RpcError::new(ErrorCode::JobUnsettled, unsettled_text)
                            .with_data(Value::Object(data)),
                    ));
                }
                JobState::Submitted => match (&adapter, record.handle) {
                    (Adapter::Job(_), Some(handle)) => Some(handle),
                    _ => return Err(known.error(ErrorCode::JobUnsettled, unsettled_text)),
                },
            },
        };

        // 8. The hold, and what every attempt shares.
        let Value::Object(members) = &request else {
            return Err(known.error(
                ErrorCode::InvalidParams,
                "a capability request is a JSON object",
            ));
        };
        let with: IndexMap<String, Val> = members
            .iter()
            .map(|(name, value)| (name.clone(), Val::from_json(value)))
            .collect();
        let (_, hold) = route.cost(&with);
        let mut request_files = IndexMap::new();
        for file in named {
            let path = store
                .file_path(&file.digest)
                .map_err(|e| CallError::Fault(e.to_string()))?;
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
            fingerprint: route.fingerprint(),
        };
        let job_template = JobRecord {
            key: key.clone(),
            capability: capability.to_string(),
            route: route_entry.clone(),
            request: request.clone(),
            take: site.takes.clone(),
            state: JobState::Submitting,
            handle: None,
            note: None,
        };
        let tally = Tally {
            ledger,
            charged: Mutex::new(None),
            unrecorded: Mutex::new(None),
        };
        let hold_name = || services.next_hold(&site.instance_id);
        let attempts = Attempts {
            control: Some(&services.control),
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
            check: engine_check(capability),
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
        // What the log could not record stops the run, whatever the outcome.
        let unrecorded = tally.own_fault();
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
            Outcome::Store(error) => Err(CallError::Fault(error.to_string())),
            Outcome::Unrecorded(reason) => Err(CallError::Fault(reason)),
        };
        let answer = match ended {
            Ok(answer) => answer,
            Err(error) => {
                if let Some(charged) = charged {
                    counter.add(charged);
                }
                return Err(match unrecorded {
                    Some(reason) => CallError::Fault(reason),
                    None => error,
                });
            }
        };

        // The charge stands whatever happens to the answer next.
        let charged = charged.unwrap_or(Usd::ZERO);
        counter.add(charged);

        // Bytes first: the files, then the call record, then the job record goes (spec/store.md
        // §6).
        let cost = if collected {
            Some(Usd::ZERO)
        } else {
            answer.cost
        };
        let reported = answer.cost;
        let record = self.publish(request, route_entry, answer, reported)?;
        if let Some(reason) = unrecorded {
            return Err(CallError::Fault(reason));
        }
        // The caller gets what a replay of the record gives (module doc, step 9).
        let (answered, data) = from_record(store, files, &replayed(&record)?)?;
        let named = emit_call(
            services, site, capability, &route_id, &key, false, cost, false,
        );
        // A job record left behind is answered by the call record from now on (spec/store.md
        // §5). It goes once the log names the call, so a collected answer is never named by
        // neither (§9, "Pruning a store"); a later run that trusts the record removes it.
        if named {
            let _ = store.remove_job(&key);
        }
        Ok(CallAnswer {
            key,
            cached: false,
            cost,
            charged,
            files: answered,
            data,
        })
    }
}

impl Lead<'_> {
    /// Stores an answer's files and then its call record (spec/store.md §6), with `cost_usd`
    /// `reported`; the record as written.
    fn publish(
        &self,
        request: Value,
        route: RouteEntry,
        answer: Answer,
        reported: Option<Usd>,
    ) -> Result<CallRecord, CallError> {
        let store = &self.services.engine.store;
        let mut entries = IndexMap::new();
        for (name, file) in &answer.files {
            let stored = store
                .put_bytes(&file.bytes)
                .map_err(|e| CallError::Fault(e.to_string()))?;
            entries.insert(
                name.clone(),
                FileEntry {
                    digest: stored.digest,
                    kind: file.kind.clone(),
                    name: name.clone(),
                    size: stored.size,
                    key: None,
                },
            );
        }
        let record = CallRecord {
            key: self.key.to_string(),
            capability: self.capability.to_string(),
            route,
            request,
            take: self.site.takes.clone(),
            files: entries,
            data: answer.data,
            cost_usd: reported,
        };
        store
            .save_call(&record)
            .map_err(|e| CallError::Fault(e.to_string()))?;
        Ok(record)
    }

    /// Step 4 in a stand-in run (module doc, "Stand-in answers"): how the call ended, or `None`
    /// when the stand-in declined it.
    async fn stand_in(
        &self,
        stand_in: &StandIn,
        request: &Value,
        route_ref: &RouteRef,
        known: &Known<'_>,
    ) -> Option<Result<CallAnswer, CallError>> {
        let Lead {
            services,
            site,
            counter,
            files,
            capability,
            route,
            named,
            key,
        } = *self;
        let route_id = route.id();
        let store = &services.engine.store;
        let refused = |reason: &str| {
            known.error(
                ErrorCode::CapabilityRefused,
                format!("{capability} on {route_id} was refused: {reason}"),
            )
        };
        let Some(_admission) = services.control.admit() else {
            return Some(Err(CallError::Cancelled));
        };
        let failed = |reason: &str| {
            known.error(
                ErrorCode::CallFailed,
                format!("{capability} on {route_id} failed: {reason}"),
            )
        };

        // 1. The request, against its capability; the stand-in is not asked when it is refused.
        if let Err(reason) = stand_in.checks().request(capability, request) {
            return Some(Err(refused(&reason)));
        }

        // 2. Ask, with a file ref for every file the request names.
        let mut references = IndexMap::new();
        let mut request_files = IndexMap::new();
        for file in named {
            let reference = match file_ref(store, file, files) {
                Ok(reference) => reference,
                Err(reason) => return Some(Err(CallError::Fault(reason))),
            };
            request_files.insert(
                file.digest.clone(),
                RequestFile {
                    digest: file.digest.clone(),
                    kind: file.kind.clone(),
                    size: file.size,
                    path: std::path::PathBuf::from(&reference.path),
                },
            );
            references.insert(file.digest.clone(), reference);
        }
        let params = StandInAnswerParams {
            capability: capability.to_string(),
            route: StandInRoute {
                id: route_id.clone(),
                fingerprint: route.fingerprint(),
            },
            request: request
                .as_object()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            take: site.takes.iter().map(|take| u64::from(*take)).collect(),
            key: key.to_string(),
            instance: StandInInstance {
                id: site.instance_id.clone(),
                path: site.path.clone(),
                step: site.step.clone(),
            },
            files: references,
        };
        let (sent, data) = match stand_in.answerer().answer(params, &site.cancel).await {
            Ok(Reply::Answer { files, data }) => (files, data),
            Ok(Reply::Decline) => return None,
            Err(AnswererError::Refused(message)) => return Some(Err(refused(&message))),
            Err(AnswererError::Failed(message)) => return Some(Err(failed(&message))),
            Err(AnswererError::Fault(sentence)) => return Some(Err(CallError::Fault(sentence))),
            Err(AnswererError::Cancelled) => return Some(Err(CallError::Cancelled)),
        };

        // 3. The checks a provider's answer passes, in order; the first refusal fails the call.
        let call = CallRequest {
            route: route_ref.clone(),
            request: request.clone(),
            files: request_files,
            take: site.takes.clone(),
            key: key.to_string(),
            attempt: 1,
        };
        let mut answer = match stand_in.checks().answer(&call, sent, data) {
            Ok(answer) => answer,
            Err(reason) => return Some(Err(failed(&reason))),
        };
        answer.data = match retry::canonical_data(&answer.data) {
            Ok(data) => data,
            Err(reason) => return Some(Err(failed(&reason))),
        };
        if let Some(check) = engine_check(capability)
            && let Err(reason) = check(&answer)
        {
            return Some(Err(failed(&reason)));
        }

        // 4. Stored in the stand-in store, paid at nothing, and handed back as a replay reads it.
        let route_entry = RouteEntry {
            id: route_id.clone(),
            fingerprint: route.fingerprint(),
        };
        let ended = self
            .publish(request.clone(), route_entry, answer, Some(Usd::ZERO))
            .and_then(|record| replayed(&record))
            .and_then(|record| from_record(store, files, &record));
        let (answered, data) = match ended {
            Ok(answered) => answered,
            Err(error) => return Some(Err(error)),
        };
        counter.add(Usd::ZERO);
        emit_call(
            services,
            site,
            capability,
            &route_id,
            key,
            false,
            Some(Usd::ZERO),
            true,
        );
        Some(Ok(CallAnswer {
            key: key.to_string(),
            cached: false,
            cost: Some(Usd::ZERO),
            charged: Usd::ZERO,
            files: answered,
            data,
        }))
    }
}

/// Emits `call` (spec/schemas/fx-run-events-v1); nothing while planning. A failed write does
/// not undo a call that is already recorded in the store. `stand_in`: a stand-in answered it.
/// Whether the log holds the event now (true while planning, which keeps no log).
#[allow(clippy::too_many_arguments)]
fn emit_call(
    services: &Services,
    site: &CallSite,
    capability: &str,
    route_id: &str,
    key: &str,
    cached: bool,
    cost: Option<Usd>,
    stand_in: bool,
) -> bool {
    match &services.events {
        Some(events) => events
            .emit(&Event::Call {
                id: site.instance_id.clone(),
                capability: capability.to_string(),
                route: route_id.to_string(),
                call: key.to_string(),
                cached,
                cost_usd: cost,
                stand_in,
            })
            .is_ok(),
        None => true,
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
