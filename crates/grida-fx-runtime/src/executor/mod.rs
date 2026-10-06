//! Running one instance once: one attempt (spec/protocol.md §5.3; spec/store.md §4).
//!
//! The runner (or the plan-time runner) turns an expanded `Instance` into an [`InstanceJob`] on
//! the scheduler thread ([`InstanceJob::from_instance`]); the job is `Send` and runs as a task.
//! [`execute`] makes one attempt:
//!
//! 1. **Result cache.** With a known identity, a trusted result record for the identity and the
//!    job's read set (`store::read_set`) answers: outputs from the record (`RecordOutput::to_val`),
//!    the record's facts plus `cost_usd` (the record's), [`CacheUse::Hit`]. Nothing runs.
//! 2. **`fx/select@1`** ([`select`]): the first candidate of `first_of` (a list) that is not
//!    missing, null or failed: succeeded with output `value` and fact `chosen` (its index); none:
//!    skipped, `no candidate exists`.
//! 3. **A paid built-in** (`JobBody::Capability`): `capability_node::run`; no host.
//! 4. **No body** (`JobBody::None`): failed, `<uses> has no implementation yet`.
//! 5. **A body** (a project type, or a built-in of `grida.fx.std`): input and param files that are
//!    not in the store yet are adopted (`Store::adopt`); an empty work dir
//!    `<store>/work/<run id>/` is made; `run` params are staged ([`stage::run_params`]); a host is
//!    leased from the pool and `run` sent with a `requests::RunHandler` serving the run's requests;
//!    the reply is accepted ([`accept::accept`]); the work dir is removed.
//!    - a host `node_failure`: failed with its message, facts kept (reported with `fact`, then the
//!      error's `data.facts`), never retried;
//!    - a host `node_error`: failed with `<type>: <message>` (`data.exception`), or `<type>` alone
//!      when the message is empty or is the type's name (an exception with no message), facts
//!      kept, retryable under `retry: engine`;
//!    - an engine error the body let propagate (`ceiling_exceeded`, `call_failed`, …): failed with
//!      its message, not retried;
//!    - timed out: failed `ran past <n> seconds` (`n` in JCS form), not retried; from the
//!      deadline on, the run's requests are answered `cancelled` (a paid call in flight goes on
//!      and settles), and whatever the host answers then, other than a result, is the timeout;
//!    - the host exited: failed `the node host exited with status <n>`, `the node host was killed
//!      by signal <n> (<NAME>)`, or `the node host exited` when unknown (`host::exit_text`),
//!      retryable under `retry: engine`;
//!    - a reply FX cannot read: failed `the node host answered run with something FX cannot
//!      read: <what>` (`accept::unreadable_result`).
//! 6. **Judges** (§5.3 step 4): a judge whose `verdict` fact is not `accept` or `reject` fails with
//!    `<type name> is a judge and reported no verdict`.
//! 7. **Facts and the record** (§5.3 step 5): the engine sets the fact `cost_usd` (the run's paid
//!    calls, `null` when none) and, when the identity is known, publishes the result record (its
//!    output files first). A failed attempt writes no record.
//!
//! [`Attempt::stop`] carries a reason the whole run must stop for (an unreadable job record, the
//! store failing); the runner then stops like a run-time problem.
//!
//! Further rules:
//! - **Stopping.** When `cancel` fires the attempt ends failed with `the run was stopped`, at
//!   once: the host gets `$/cancel` (the pool kills it 5 seconds later) and a paid built-in's call
//!   goes on on a task of its own, so a call already in flight completes and settles. The
//!   dispatcher sees the token and emits no terminal event. A step's `timeout:` bounds a paid
//!   built-in the same way (`ran past <n> seconds`).
//! - **Faults.** A run request that met a fault of the engine's own (an unreadable job record, a
//!   store that cannot be read or written: [`requests::RunHandler::fault`]) stops the run even
//!   when the body caught the error; so do a store that cannot be written while the result is
//!   accepted, and a work dir that cannot be made. Other `internal` answers, such as a resource
//!   that cannot be read, fail only the request, and the node when the body lets them propagate.
//! - **Adoption.** A file that is not in the store and whose local copy cannot be read fails the
//!   attempt (`<name> is not in the store, and its file cannot be read: <reason>`); a local copy
//!   whose bytes no longer have the planned digest fails it too (`<name> changed after the run
//!   was planned`).
//! - **Facts.** Facts reported with `fact` survive every failure. A paid built-in's facts (from
//!   its answer's `data`) leave out `cost_usd` and values holding a reserved marker.
//! - **No private paths.** Error texts that reach a result have the store root and the project
//!   root replaced by relative labels ([`crate::engine::Engine::scrub`]).

pub mod accept;
pub mod capability_node;
pub mod requests;
pub mod stage;

use crate::calls::{CallCounter, CallError};
use crate::engine::{Cancel, RunFiles, Services};
use crate::host::pool::{RunReply, RunRequests};
use crate::ledger::Scopes;
use crate::store::records::{RecordOutput, ResultRecord};
use crate::store::{ReadSet, Store};
use accept::{NotAccepted, failure_facts};
use grida_fx_core::error::io_reason;
use grida_fx_core::expand::{Instance, NodeResult, ResultStatus};
use grida_fx_core::money::Usd;
use grida_fx_core::project::Planner;
use grida_fx_core::registry::TypeOrigin;
use grida_fx_core::routes::Route;
use grida_fx_core::spec::{BodyKind, NodeSpec, Retry};
use grida_fx_core::val::{FileValue, Val};
use grida_fx_protocol::{ErrorCode, RpcError, RunResult};
use indexmap::IndexMap;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Who runs a job's body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobBody {
    /// A project type: `{path, attribute}` for `run`'s `body`.
    Project { path: String, attribute: String },
    /// A built-in whose body is in `grida.fx.std`: `fx/<name>@<major>`.
    Std { builtin: String },
    /// A paid built-in: the engine makes its one call.
    Capability { capability: String },
    /// `fx/select@1`.
    Select,
    /// A built-in with no body yet.
    None,
}

/// One instance, ready to run off the scheduler thread.
#[derive(Debug, Clone)]
pub struct InstanceJob {
    pub id: String,
    pub path: String,
    pub step: String,
    pub key: Option<String>,
    pub takes: Vec<u32>,
    pub uses: String,
    pub type_identity: String,
    pub spec: Arc<NodeSpec>,
    pub body: JobBody,
    pub with: IndexMap<String, Val>,
    pub identity: Option<String>,
    pub read: ReadSet,
    /// `{capability: bound}` (`NodeSpec::capability_calls`; `{capability: 1}` for a paid built-in).
    pub calls: IndexMap<String, u32>,
    pub routes: IndexMap<String, Route>,
    /// `{capability: concurrency}`: the project's `routes.<capability>.concurrency`, else the
    /// route's.
    pub limits: IndexMap<String, Option<u32>>,
    pub scopes: Scopes,
    /// `{declared path: absolute path}` under the home project.
    pub resources: IndexMap<String, PathBuf>,
    /// `{tool name: executable or None}` (`tools::resolve_tool`).
    pub tools: IndexMap<String, Option<PathBuf>>,
    pub timeout_s: Option<f64>,
    pub retry: Retry,
    /// The takes file's `result` for this instance's path and take, when it names one (a pick).
    pub picked: Option<String>,
}

impl InstanceJob {
    /// The job of an expanded instance (module doc). `env` reads environment variables (tool
    /// overrides). Fails with a sentence when the instance cannot be run as planned.
    pub fn from_instance(
        instance: &Instance,
        planner: &Planner,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<InstanceJob, String> {
        let ty = &instance.ty;
        if instance
            .with
            .values()
            .any(|value| value.contains_pending() || value.contains_view())
        {
            let waiting: Vec<String> = instance.waiting_on().into_iter().collect();
            return Err(format!(
                "{} cannot run yet: it waits on {}",
                instance.id,
                if waiting.is_empty() {
                    "a value no step produces".to_string()
                } else {
                    waiting.join(", ")
                }
            ));
        }
        let spec = Arc::new((*ty.spec).clone());
        let body = match (ty.body, &ty.origin) {
            (BodyKind::Project, TypeOrigin::Project { path, attribute }) => JobBody::Project {
                path: path.clone(),
                attribute: attribute.clone(),
            },
            (BodyKind::Python, TypeOrigin::Builtin { name, major }) => JobBody::Std {
                builtin: format!("fx/{name}@{major}"),
            },
            (BodyKind::Capability, _) => match &spec.capability {
                Some(capability) => JobBody::Capability {
                    capability: capability.clone(),
                },
                None => return Err(format!("{} is a paid type with no capability", ty.uses)),
            },
            (BodyKind::Engine, _) => JobBody::Select,
            (BodyKind::None, _) => JobBody::None,
            (BodyKind::Project, TypeOrigin::Builtin { .. }) => {
                return Err(format!("{} is a built-in, not a project type", ty.uses));
            }
            (BodyKind::Python, TypeOrigin::Project { .. }) => {
                return Err(format!("{} is a project type, not a built-in", ty.uses));
            }
        };
        let limits = instance
            .routes
            .iter()
            .map(|(capability, route)| {
                let project = planner
                    .project
                    .document
                    .routes
                    .get(capability)
                    .and_then(|default| default.concurrency);
                (capability.clone(), project.or(route.concurrency))
            })
            .collect();
        let resources = spec
            .resources
            .iter()
            .map(|declared| (declared.clone(), planner.home.root.join(declared)))
            .collect();
        let tools = spec
            .tools
            .iter()
            .map(|entry| {
                let name = crate::tools::tool_name(entry);
                (name.to_string(), crate::tools::resolve_tool(name, env))
            })
            .collect();
        let picked = planner
            .takes
            .get(&instance.path)
            .filter(|choice| choice.take == instance.take())
            .and_then(|choice| choice.result.clone());
        Ok(InstanceJob {
            id: instance.id.clone(),
            path: instance.path.clone(),
            step: instance.step.clone(),
            key: instance.key.clone(),
            takes: instance.takes.clone(),
            uses: instance.uses.clone(),
            type_identity: ty.identity.clone(),
            calls: spec.capability_calls(&instance.with),
            spec,
            body,
            with: instance.with.clone(),
            identity: instance.identity.clone(),
            read: crate::store::read_set(&instance.with),
            routes: instance.routes.clone(),
            limits,
            scopes: instance.budget.clone().into_iter().collect(),
            resources,
            tools,
            timeout_s: instance.timeout_s,
            retry: ty.spec.retry,
            picked,
        })
    }

    /// The `calls::CallSite` of this job.
    pub fn call_site(&self) -> crate::calls::CallSite {
        crate::calls::CallSite {
            instance_id: self.id.clone(),
            step: self.step.clone(),
            type_name: self.spec.name.clone(),
            takes: self.takes.clone(),
            calls: self.calls.clone(),
            routes: self.routes.clone(),
            limits: self.limits.clone(),
            scopes: self.scopes.clone(),
        }
    }
}

/// Whether the result came from the result cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheUse {
    Hit,
    Miss,
}

/// One attempt's outcome (module doc).
#[derive(Debug, Clone, PartialEq)]
pub struct Attempt {
    pub result: NodeResult,
    pub cache: CacheUse,
    /// A `node_error` (or a host that exited): the dispatcher may run the body again under
    /// `retry: engine`.
    pub retryable: bool,
    /// The run must stop, for this reason.
    pub stop: Option<String>,
}

impl Attempt {
    /// An attempt that ran (or was refused): not from the cache, not retryable, no stop.
    fn ran(result: NodeResult) -> Attempt {
        Attempt {
            result,
            cache: CacheUse::Miss,
            retryable: false,
            stop: None,
        }
    }

    /// A failed attempt that stops the run for `reason`.
    fn stopping(reason: String, facts: IndexMap<String, Value>) -> Attempt {
        Attempt {
            result: failure(reason.clone(), facts),
            cache: CacheUse::Miss,
            retryable: false,
            stop: Some(reason),
        }
    }
}

/// The message of an attempt cut short by cancellation.
const STOPPED: &str = "the run was stopped";

/// Makes one attempt (module doc).
pub async fn execute(services: Arc<Services>, job: Arc<InstanceJob>, cancel: Cancel) -> Attempt {
    let store = Arc::clone(&services.engine.store);
    if let Some(hit) = from_cache(&store, &job) {
        return hit;
    }
    let mut attempt = match &job.body {
        JobBody::Select => Attempt::ran(select(&job)),
        JobBody::None => Attempt::ran(failed(&format!("{} has no implementation yet", job.uses))),
        JobBody::Capability { .. } => paid_builtin(&services, &job, &cancel).await,
        JobBody::Project { .. } | JobBody::Std { .. } => host_run(&services, &job, &cancel).await,
    };
    let engine = &services.engine;
    if let Some(error) = attempt.result.error.take() {
        attempt.result.error = Some(engine.scrub(&error));
    }
    if let Some(stop) = attempt.stop.take() {
        attempt.stop = Some(engine.scrub(&stop));
    }
    attempt
}

/// Step 1: a trusted result record answers, or `None`.
fn from_cache(store: &Store, job: &InstanceJob) -> Option<Attempt> {
    let identity = job.identity.as_deref()?;
    let record = store.load_result(identity, &job.read)?;
    let mut outputs = IndexMap::with_capacity(record.outputs.len());
    for (port, output) in &record.outputs {
        // A file that cannot be read back makes the record absent (spec/store.md §4).
        outputs.insert(port.clone(), output.to_val(store).ok()?);
    }
    let mut facts = record.facts.clone();
    facts.insert(
        "cost_usd".into(),
        record.cost_usd.map_or(Value::Null, Usd::to_value),
    );
    Some(Attempt {
        result: NodeResult {
            status: ResultStatus::Succeeded,
            outputs,
            facts,
            error: None,
        },
        cache: CacheUse::Hit,
        retryable: false,
        stop: None,
    })
}

/// Step 3: a paid built-in's one call (module doc, "Stopping").
async fn paid_builtin(
    services: &Arc<Services>,
    job: &Arc<InstanceJob>,
    cancel: &Cancel,
) -> Attempt {
    let store = &services.engine.store;
    let files = Arc::new(RunFiles::new());
    if let Err(unadopted) = adopt(store, job, Some(&files)) {
        return unadopted.attempt();
    }
    if cancel.is_cancelled() {
        return Attempt::ran(failed(STOPPED));
    }
    let counter = Arc::new(CallCounter::new());
    let task = {
        let (services, job, counter, files) = (
            Arc::clone(services),
            Arc::clone(job),
            Arc::clone(&counter),
            Arc::clone(&files),
        );
        // Counted as running before the task first runs, so a run that ends meanwhile waits for
        // it (`Services::calls_settled`).
        let running = services.track_call();
        services.engine.handle.clone().spawn(async move {
            let _running = running;
            capability_node::run(&services, &job, &counter, &files).await
        })
    };
    let joined = tokio::select! {
        joined = task => joined,
        _ = cancel.cancelled() => return Attempt::ran(failed(STOPPED)),
        _ = deadline(timeout_of(job)) => return Attempt::ran(failed(&ran_past(job))),
    };
    match joined {
        Ok(Ok(produced)) => {
            let facts = failure_facts(&produced.facts, None);
            finish(store, job, produced.outputs, facts, counter.cost())
        }
        Ok(Err(CallError::Rpc(error))) => Attempt::ran(failed(&error.message)),
        Ok(Err(CallError::Cancelled)) => Attempt::ran(failed(STOPPED)),
        Ok(Err(CallError::Store(message))) => Attempt::stopping(message, IndexMap::new()),
        Err(error) => Attempt::stopping(
            format!(
                "the engine failed while making the call of {}: {error}",
                job.id
            ),
            IndexMap::new(),
        ),
    }
}

/// Step 5: a body run by a node host (module doc).
async fn host_run(services: &Arc<Services>, job: &Arc<InstanceJob>, cancel: &Cancel) -> Attempt {
    let store = &services.engine.store;
    if let Err(unadopted) = adopt(store, job, None) {
        return unadopted.attempt();
    }
    if cancel.is_cancelled() {
        return Attempt::ran(failed(STOPPED));
    }
    let run_id = services.next_run_id();
    services.claim_work();
    let work_dir = store.work_root().join(&run_id);
    if let Err(error) = fresh_dir(&work_dir) {
        return Attempt::stopping(
            format!("the store's work/{run_id}: {}", io_reason(&error)),
            IndexMap::new(),
        );
    }
    let attempt = run_on_host(services, job, cancel, &run_id, &work_dir).await;
    remove_dir(&work_dir);
    attempt
}

/// Sends `run` to a leased host and maps its reply (module doc).
async fn run_on_host(
    services: &Arc<Services>,
    job: &Arc<InstanceJob>,
    cancel: &Cancel,
    run_id: &str,
    work_dir: &Path,
) -> Attempt {
    let store = &services.engine.store;
    let files = Arc::new(RunFiles::new());
    let params = match stage::run_params(job, run_id, work_dir, store, &files) {
        Ok(params) => params,
        Err(message) => return Attempt::ran(failed(&message)),
    };
    let lease = tokio::select! {
        lease = services.engine.hosts.lease() => lease,
        _ = cancel.cancelled() => return Attempt::ran(failed(STOPPED)),
    };
    let mut lease = match lease {
        Ok(lease) => lease,
        Err(message) => return Attempt::ran(failed(&message)),
    };
    // The run's requests end at its deadline as they do when it is stopped (module doc,
    // "Stopping"): the lease stops them (`RunRequests::stop` cancels this token) before it sends
    // `$/cancel`. From then on they are answered `cancelled`, and a paid call in flight goes on
    // on its own task and settles.
    let requests_cancel = cancel.child();
    let handler = Arc::new(requests::RunHandler::new(
        Arc::clone(services),
        Arc::clone(job),
        run_id.to_string(),
        work_dir.to_path_buf(),
        Arc::clone(&files),
        lease.host().connection().clone(),
        requests_cancel.clone(),
    ));
    let timeout = timeout_of(job);
    let reply = lease
        .run(
            &params,
            Arc::clone(&handler) as Arc<dyn RunRequests>,
            cancel,
            timeout,
        )
        .await;
    handler.close();
    drop(lease);
    // Past the deadline, whatever the host answered other than a result is the timeout.
    let timed_out = requests_cancel.is_cancelled() && !cancel.is_cancelled();
    let reply = match reply {
        RunReply::Result(value) => RunReply::Result(value),
        _ if timed_out => RunReply::TimedOut,
        other => other,
    };
    let reported = handler.facts();
    let marks = handler.marks();
    let mut attempt = match reply {
        RunReply::Result(value) => match serde_json::from_value::<RunResult>(value.clone()) {
            Ok(result) => {
                match accept::accept_or_stop(
                    job, &result, &reported, &marks, work_dir, store, &files,
                ) {
                    Ok(accepted) => finish(
                        store,
                        job,
                        accepted.outputs,
                        accepted.facts,
                        handler.counter.cost(),
                    ),
                    Err(NotAccepted::Refused(refused)) => {
                        Attempt::ran(failure(refused.message, refused.facts))
                    }
                    Err(NotAccepted::Store { message, facts }) => Attempt::stopping(message, facts),
                }
            }
            Err(error) => Attempt::ran(failure(
                format!(
                    "the node host answered run with something FX cannot read: {}",
                    accept::unreadable_result(&value, &error)
                ),
                failure_facts(&reported, None),
            )),
        },
        RunReply::Error(error) => host_error(error, &reported, cancel),
        RunReply::TimedOut => Attempt::ran(failure(ran_past(job), failure_facts(&reported, None))),
        RunReply::Cancelled => {
            Attempt::ran(failure(STOPPED.into(), failure_facts(&reported, None)))
        }
        RunReply::Exited(status) => {
            let message = format!("the node host {}", crate::host::exit_text(status));
            Attempt {
                retryable: true,
                ..Attempt::ran(failure(message, failure_facts(&reported, None)))
            }
        }
    };
    if attempt.stop.is_none() {
        attempt.stop = handler.fault();
    }
    attempt
}

/// A host's error answer to `run` (module doc, step 5).
fn host_error(error: RpcError, reported: &IndexMap<String, Value>, cancel: &Cancel) -> Attempt {
    let facts = failure_facts(reported, error.data.as_ref());
    match error.kind() {
        Some(ErrorCode::NodeError) => {
            let exception = error
                .data
                .as_ref()
                .and_then(|data| data.get("exception"))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("Error");
            // A message is never empty (spec/protocol.md §7), so a host stands the exception's
            // name in for one that has none: either way the failure is the name alone.
            let message = if error.message.is_empty() || error.message == exception {
                exception.to_string()
            } else {
                format!("{exception}: {}", error.message)
            };
            Attempt {
                retryable: true,
                ..Attempt::ran(failure(message, facts))
            }
        }
        Some(ErrorCode::Cancelled) if cancel.is_cancelled() => {
            Attempt::ran(failure(STOPPED.into(), facts))
        }
        // node_failure, load_failed, and every engine error the body let propagate.
        _ => Attempt::ran(failure(error.message, facts)),
    }
}

/// Steps 6 and 7: the judge's verdict, the `cost_usd` fact and the result record.
fn finish(
    store: &Store,
    job: &InstanceJob,
    outputs: IndexMap<String, Val>,
    facts: IndexMap<String, Value>,
    cost: Option<Usd>,
) -> Attempt {
    if job.spec.judge
        && !matches!(
            facts.get("verdict").and_then(Value::as_str),
            Some("accept" | "reject")
        )
    {
        return Attempt::ran(failure(
            format!("{} is a judge and reported no verdict", job.spec.name),
            facts,
        ));
    }
    if let Some(identity) = &job.identity {
        let mut recorded = IndexMap::with_capacity(outputs.len());
        for (port, value) in &outputs {
            match RecordOutput::from_val(value) {
                Ok(output) => {
                    recorded.insert(port.clone(), output);
                }
                Err(message) => return Attempt::ran(failure(message, facts)),
            }
        }
        let record = ResultRecord {
            identity: identity.clone(),
            type_identity: job.type_identity.clone(),
            outputs: recorded,
            facts: facts.clone(),
            read: job.read.clone(),
            cost_usd: cost,
        };
        if let Err(error) = store.save_result(&record) {
            return Attempt::stopping(error.to_string(), facts);
        }
    }
    let mut facts = facts;
    facts.insert("cost_usd".into(), cost.map_or(Value::Null, Usd::to_value));
    Attempt::ran(NodeResult {
        status: ResultStatus::Succeeded,
        outputs,
        facts,
        error: None,
    })
}

/// `fx/select@1` (module doc).
pub fn select(job: &InstanceJob) -> NodeResult {
    if let Some(Val::List(candidates)) = job.with.get("first_of") {
        for (index, candidate) in candidates.iter().enumerate() {
            if matches!(candidate, Val::Missing | Val::Null | Val::Failed(_)) {
                continue;
            }
            return NodeResult {
                status: ResultStatus::Succeeded,
                outputs: IndexMap::from([("value".to_string(), candidate.clone())]),
                facts: IndexMap::from([("chosen".to_string(), Value::from(index as u64))]),
                error: None,
            };
        }
    }
    NodeResult {
        status: ResultStatus::Skipped,
        outputs: IndexMap::new(),
        facts: IndexMap::new(),
        error: Some("no candidate exists".into()),
    }
}

/// A failed result with no outputs and no facts.
pub fn failed(error: &str) -> NodeResult {
    NodeResult {
        status: grida_fx_core::expand::ResultStatus::Failed,
        outputs: IndexMap::new(),
        facts: IndexMap::new(),
        error: Some(error.to_string()),
    }
}

/// A failed result keeping `facts`.
fn failure(error: String, facts: IndexMap<String, Value>) -> NodeResult {
    NodeResult {
        facts,
        ..failed(&error)
    }
}

/// `ran past <n> seconds`.
fn ran_past(job: &InstanceJob) -> String {
    format!(
        "ran past {} seconds",
        grida_fx_core::value::format_number(job.timeout_s.unwrap_or(0.0))
    )
}

/// The step's timeout as a duration; `None` without one (or for a value no duration holds).
fn timeout_of(job: &InstanceJob) -> Option<Duration> {
    job.timeout_s
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
}

/// Resolves after `timeout`, or never.
async fn deadline(timeout: Option<Duration>) {
    match timeout {
        Some(timeout) => tokio::time::sleep(timeout).await,
        None => std::future::pending::<()>().await,
    }
}

/// Why a file could not be adopted.
enum Unadopted {
    /// The attempt fails with this sentence.
    Failed(String),
    /// The store failed: the run stops.
    Store(String),
}

impl Unadopted {
    fn attempt(self) -> Attempt {
        match self {
            Unadopted::Failed(message) => Attempt::ran(failed(&message)),
            Unadopted::Store(message) => Attempt::stopping(message, IndexMap::new()),
        }
    }
}

/// Puts every file of the job's declared inputs and params in the store (module doc,
/// "Adoption"); with `files`, hands each one to the run.
fn adopt(store: &Store, job: &InstanceJob, files: Option<&RunFiles>) -> Result<(), Unadopted> {
    let names = job.spec.inputs.keys().chain(job.spec.params.keys());
    for value in names.filter_map(|name| job.with.get(name)) {
        let mut found = Vec::new();
        each_file(value, &mut found);
        for file in found {
            let stored = adopt_file(store, file)?;
            if let Some(files) = files {
                files.insert(&stored);
            }
        }
    }
    Ok(())
}

/// One file, in the store, with `location` at the store's copy.
fn adopt_file(store: &Store, file: &FileValue) -> Result<FileValue, Unadopted> {
    if store.has(&file.digest, file.size) {
        let mut stored = file.clone();
        stored.location = store.file_path(&file.digest).ok();
        return Ok(stored);
    }
    let readable = match &file.location {
        Some(location) => std::fs::File::open(location)
            .map(drop)
            .map_err(|error| io_reason(&error)),
        None => Err("it has no local copy".to_string()),
    };
    if let Err(reason) = readable {
        return Err(Unadopted::Failed(format!(
            "{} is not in the store, and its file cannot be read: {reason}",
            file.name
        )));
    }
    let stored = store.adopt(file).map_err(|error| match error {
        // The file the plan named, not the store: the attempt fails.
        crate::store::StoreError::Source(sentence) => Unadopted::Failed(sentence),
        other => Unadopted::Store(other.to_string()),
    })?;
    if stored.digest != file.digest {
        return Err(Unadopted::Failed(format!(
            "{} changed after the run was planned",
            file.name
        )));
    }
    Ok(stored)
}

/// Every file a value holds, in order: a file, a list's items, an object's members, a
/// collection's items.
fn each_file<'a>(value: &'a Val, found: &mut Vec<&'a FileValue>) {
    match value {
        Val::File(file) => found.push(file),
        Val::List(items) => items.iter().for_each(|item| each_file(item, found)),
        Val::Object(map) => map.values().for_each(|item| each_file(item, found)),
        Val::Collection(collection) => collection
            .items
            .iter()
            .for_each(|(_, item)| each_file(item, found)),
        _ => {}
    }
}

/// Makes `dir` an empty directory.
fn fresh_dir(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)
}

/// Removes a work dir and everything under it, best effort.
fn remove_dir(dir: &Path) {
    if std::fs::remove_dir_all(dir).is_err() {
        // A body may leave read-only folders behind: make everything writable and try again.
        make_writable(dir);
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn make_writable(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    let mut permissions = metadata.permissions();
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
    let _ = std::fs::set_permissions(path, permissions);
    if metadata.is_dir()
        && let Ok(entries) = std::fs::read_dir(path)
    {
        for entry in entries.flatten() {
            make_writable(&entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::val::Collection;
    use grida_fx_protocol::RpcError;
    use serde_json::json;

    fn job_with(with: Vec<(&str, Val)>) -> InstanceJob {
        InstanceJob {
            id: "pick#1".into(),
            path: "pick".into(),
            step: "pick".into(),
            key: None,
            takes: vec![1],
            uses: "fx/select@1".into(),
            type_identity: "fx/select@1.1".into(),
            spec: Arc::new(NodeSpec {
                name: "select".into(),
                description: None,
                inputs: IndexMap::new(),
                params: IndexMap::from([("first_of".to_string(), json!({"type": "array"}))]),
                outputs: IndexMap::new(),
                judge: false,
                capability: None,
                calls: IndexMap::new(),
                resources: Vec::new(),
                tools: Vec::new(),
                view: None,
                version: Some(1),
                retry: Retry::Service,
            }),
            body: JobBody::Select,
            with: with.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            identity: None,
            read: ReadSet::new(),
            calls: IndexMap::new(),
            routes: IndexMap::new(),
            limits: IndexMap::new(),
            scopes: Vec::new(),
            resources: IndexMap::new(),
            tools: IndexMap::new(),
            timeout_s: None,
            retry: Retry::Service,
            picked: None,
        }
    }

    #[test]
    fn select_takes_the_first_candidate_that_exists() {
        let first_of = Val::List(vec![
            Val::Missing,
            Val::Null,
            Val::Failed("a#1".into()),
            Val::Str("b".into()),
            Val::Str("c".into()),
        ]);
        let result = select(&job_with(vec![("first_of", first_of)]));
        assert_eq!(result.status, ResultStatus::Succeeded);
        assert_eq!(result.outputs["value"], Val::Str("b".into()));
        assert_eq!(result.facts["chosen"], json!(3));
        assert_eq!(result.error, None);

        // False, zero and empty values exist.
        let result = select(&job_with(vec![(
            "first_of",
            Val::List(vec![Val::Missing, Val::Bool(false)]),
        )]));
        assert_eq!(result.outputs["value"], Val::Bool(false));
        assert_eq!(result.facts["chosen"], json!(1));
    }

    #[test]
    fn select_without_a_candidate_is_skipped() {
        let none = [
            vec![],
            vec![("first_of", Val::List(vec![]))],
            vec![("first_of", Val::List(vec![Val::Missing, Val::Null]))],
            vec![("first_of", Val::Str("x".into()))],
            vec![(
                "first_of",
                Val::Collection(Box::new(Collection {
                    items: vec![("k".into(), Val::Str("x".into()))],
                    verdicts: IndexMap::new(),
                })),
            )],
        ];
        for with in none {
            let result = select(&job_with(with));
            assert_eq!(result.status, ResultStatus::Skipped);
            assert!(result.outputs.is_empty());
            assert!(result.facts.is_empty());
            assert_eq!(result.error.as_deref(), Some("no candidate exists"));
        }
    }

    #[test]
    fn host_errors_map_to_failures() {
        let cancel = Cancel::new();
        let reported = IndexMap::from([("seen".to_string(), json!(1))]);
        let error = RpcError::new(ErrorCode::NodeFailure, "no faces found")
            .with_data(json!({"facts": {"faces": 0, "cost_usd": 3}}));
        let attempt = host_error(error, &reported, &cancel);
        assert_eq!(attempt.result.error.as_deref(), Some("no faces found"));
        assert!(!attempt.retryable);
        assert_eq!(
            attempt.result.facts,
            IndexMap::from([
                ("seen".to_string(), json!(1)),
                ("faces".to_string(), json!(0))
            ])
        );

        let error = RpcError::new(ErrorCode::NodeError, "division by zero")
            .with_data(json!({"exception": "ZeroDivisionError", "traceback": "…"}));
        let attempt = host_error(error, &reported, &cancel);
        assert_eq!(
            attempt.result.error.as_deref(),
            Some("ZeroDivisionError: division by zero")
        );
        assert!(attempt.retryable);
        assert_eq!(attempt.stop, None);

        let error = RpcError::new(ErrorCode::NodeError, "boom");
        assert_eq!(
            host_error(error, &reported, &cancel)
                .result
                .error
                .as_deref(),
            Some("Error: boom")
        );

        // An exception with no message: the host sends its name, which is not repeated.
        let error = RpcError::new(ErrorCode::NodeError, "KeyError")
            .with_data(json!({"exception": "KeyError", "traceback": "…"}));
        let attempt = host_error(error, &reported, &cancel);
        assert_eq!(attempt.result.error.as_deref(), Some("KeyError"));
        assert!(attempt.retryable);

        for code in [
            ErrorCode::CeilingExceeded,
            ErrorCode::CallFailed,
            ErrorCode::LoadFailed,
            ErrorCode::Internal,
            ErrorCode::Cancelled,
        ] {
            let error = RpcError::new(code, "said so");
            let attempt = host_error(error, &reported, &cancel);
            assert_eq!(attempt.result.error.as_deref(), Some("said so"), "{code}");
            assert!(!attempt.retryable, "{code}");
            assert_eq!(attempt.result.facts, reported);
        }

        cancel.cancel();
        let attempt = host_error(
            RpcError::new(ErrorCode::Cancelled, "cancelled"),
            &reported,
            &cancel,
        );
        assert_eq!(attempt.result.error.as_deref(), Some(STOPPED));
    }

    #[test]
    fn files_are_found_in_order() {
        let file = |name: &str| FileValue {
            digest: "a".repeat(64),
            kind: "image/png".into(),
            name: name.into(),
            size: 1,
            key: None,
            content: None,
            location: None,
        };
        let value = Val::List(vec![
            Val::File(Box::new(file("a"))),
            Val::Object(IndexMap::from([(
                "x".to_string(),
                Val::File(Box::new(file("b"))),
            )])),
            Val::Collection(Box::new(Collection {
                items: vec![("k".into(), Val::File(Box::new(file("c"))))],
                verdicts: IndexMap::new(),
            })),
            Val::Str("not a file".into()),
        ]);
        let mut found = Vec::new();
        each_file(&value, &mut found);
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn work_dirs_are_made_empty_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work/plan-1");
        std::fs::create_dir_all(work.join("old")).unwrap();
        std::fs::write(work.join("old/x"), "x").unwrap();
        fresh_dir(&work).unwrap();
        assert_eq!(std::fs::read_dir(&work).unwrap().count(), 0);
        std::fs::create_dir_all(work.join("locked")).unwrap();
        std::fs::write(work.join("locked/y"), "y").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(work.join("locked"), std::fs::Permissions::from_mode(0o500))
                .unwrap();
        }
        remove_dir(&work);
        assert!(!work.exists());
    }

    #[test]
    fn timeouts_read_as_durations() {
        let mut job = job_with(vec![]);
        assert_eq!(timeout_of(&job), None);
        job.timeout_s = Some(1.5);
        assert_eq!(timeout_of(&job), Some(Duration::from_millis(1500)));
        assert_eq!(ran_past(&job), "ran past 1.5 seconds");
        job.timeout_s = Some(30.0);
        assert_eq!(ran_past(&job), "ran past 30 seconds");
        job.timeout_s = Some(-1.0);
        assert_eq!(timeout_of(&job), None);
    }
}
