//! One run invocation (spec/store.md §8; spec/protocol.md §5.3; spec/schemas/fx-run-events-v1).
//!
//! [`run`] is synchronous and runs on the command's thread, which owns the planner (the core is
//! not `Send`). Instances run as tokio tasks ([`dispatch::dispatch`]) on the engine's runtime and
//! report back over a channel the loop waits on with `blocking_recv`.
//!
//! Before anything is written, in order (each is [`RunError::Refused`], printed `refused:
//! <message>`, exit 1):
//! 1. the plan has problems: `the plan is refused:` and one `<where>: <message>` line each;
//! 2. a live run without a ceiling (docs/wg/overview.md, decision 5): `a live run needs a
//!    ceiling: pass --max-usd, or set budget.max_usd in the workflow or in fx.yaml`;
//! 3. a tool a live instance declares is not installed: `a program the run needs is not
//!    installed:` and one line per tool, sorted: `<name> (for <steps, sorted, ", ">): install it,
//!    or set GRIDA_FX_TOOL_<NAME>`;
//! 4. another invocation holds the folder's lock (`folder::RunFolder::lock`); 5. the folder holds
//!    another plan (`folder::check_plan`, read once the lock is held, so no invocation can write
//!    `plan.json` in between), or its `events.jsonl` holds an event of another plan:
//!    `<folder> holds a run of another workflow or other inputs; choose a new folder`; 6. the
//!    folder holds a run of the other mode, a stand-in run resumed without a stand-in or the
//!    reverse (`folder::check_mode`, right after `check_plan`; spec/store.md §8 "Stand-in
//!    runs").
//!
//! Then: read the earlier events (`events::read_events_noting`: a torn tail is cut off, with a
//! note on stderr, `note: <folder>/events.jsonl ended in a line an earlier invocation did not
//! finish writing; it was left out`); remove what killed invocations left behind
//! (`RunFolder::sweep`, `Store::sweep`); write `plan.json` once (`folder::plan_document`); replay
//! finished steps into `planner.results` ([`replay::replay`]); open the event log; build the
//! ledger (ceiling = the plan's) and replay it; emit `run_started {workflow, resumed, ceiling_usd,
//! charged_usd, estimate, stand_in?}` (`resumed`: there were earlier events; `estimate` the fresh
//! plan's; `stand_in: true` only when the engine has a stand-in, which `plan.json` records too).
//! A stand-in run's store, every lookup and every record included, is the stand-in store the
//! engine was opened on.
//! The ledger finds the step budgets of earlier holds in the expansion with the replayed results
//! (the plan's own expansion when that expansion fails), so run-time instances keep their scopes.
//!
//! The loop, until nothing runs and nothing can start:
//! - expand again with every result so far (`grida_fx_core::expand::expand`); run-time problems
//!   emit one `problem` each and stop the run: `a value the run produced breaks the workflow:
//!   <where>: <message>`; running tasks are cancelled (their attempts settle);
//! - settle instances that will never run ([`schedule::settle`]): blocked → failed result and
//!   `node_skipped {reason, blocked: true}`; failed by an assertion → `node_failed {error}`; a
//!   succeeded one a run-time assertion has since failed ([`schedule::overturned`]) → failed,
//!   `node_failed {error}`; then expand again, so what reads them settles in turn. A running
//!   instance whose assertion fails is stopped (its own cancellation, no `run_cancelled`) and
//!   gets `node_failed {error}` with the assertion's message once its dispatch reports;
//! - the phase gate ([`schedule::Gate`]): `phase_planned` per phase as it is reached; with
//!   `--yes-up-to`, a phase after the first whose worst case takes the run past it stops the run
//!   once nothing runs: `phase <p> may cost up to $<high, 2 places>, which takes the run past
//!   --yes-up-to <amount as JCS number>; approve it with a higher --yes-up-to`; while it holds,
//!   nothing new starts, and it is checked again after each completion;
//! - dispatch every ready instance ([`schedule::ready`]) in listing order; an instance whose job
//!   cannot be made (`executor::InstanceJob::from_instance`) fails at once with that sentence
//!   (`node_failed {error}`, no `node_started`);
//! - wait for one completion, a stop, or cancellation (Ctrl-C or SIGTERM:
//!   [`crate::engine::Cancel`]; nothing new starts after it). A completion's `stop`
//!   (`dispatch::Done::stop`) stops the run like a run-time problem: one `problem {where:
//!   <instance id>, message: <stop>}`, and `stopped` is that message. When the run stops, what
//!   still runs is cancelled and waited for; a dispatch that finished anyway keeps its result.
//!   In a stand-in run, the answerer going away while something runs
//!   ([`crate::stand_in::StandIn::lost`]) stops the run the same way: one `problem {where:
//!   stand_in, message: the stand-in answerer exited}`, and `stopped` is that message.
//!   An event that any part of the run could not write (`EventLog::fault`) is an engine fault.
//!
//! After the loop: expand once more; the declared outputs; `incomplete` when stopped, when an
//! output is still pending, or when a planned instance never ran (each named in a `problem`
//! event, `{where: <path>, message: never ran: it waits on <ids, ", ">}`, or `never ran` when it
//! waits on no step, only when the run was not stopped: a stopped run leaves work undone on
//! purpose); `ok` when this invocation failed nothing and is not incomplete; `failed` lists
//! what it failed in the expansion's listing order, never in completion order; place
//! `outputs/` (a file that cannot be placed is a `problem {where: outputs.<name>}` and stops the
//! run with `cannot place <folder>/<path>: <reason>`); wait until no paid call of the invocation
//! runs (`Services::calls_settled`: a step that ran past its timeout, or whose host exited, may
//! have left one in flight, which completes and settles); emit `run_finished`, whose
//! `charged_usd` therefore counts every attempt sent. Cancellation instead:
//! running tasks are told to stop (`$/cancel`, a host killed 5 seconds later), the calls in
//! flight end and settle their holds in full, then
//! `run_cancelled {reason: "interrupted", charged_usd}` is emitted, no outputs are placed, and
//! the outcome says `cancelled` (exit 130), with `ok: false`, `incomplete: true`, no `stopped`
//! and no outputs. An engine fault after the folder was locked (an
//! expansion that fails, an event that cannot be written) cancels what runs, waits for its paid
//! calls to settle, writes `problem {where: <workflow id>}` and `run_finished {ok: false,
//! incomplete: true, stopped: <fault>}` where it still can, and is returned as
//! [`RunError::Fatal`]. The host pool is shut down whatever happened.

pub mod dispatch;
pub mod replay;
pub mod schedule;

use crate::engine::{Cancel, Engine, Services};
use crate::events::{Event, EventLog};
use crate::executor::InstanceJob;
use crate::folder::RunFolder;
use crate::ledger::{Ledger, Scopes};
use crate::stand_in::ANSWERER_EXITED;
use crate::store::Store;
use dispatch::Done;
use grida_fx_core::Error;
use grida_fx_core::error::{ErrorKind, Problem, io_reason, unique};
use grida_fx_core::expand::{ExpandEnv, Expansion, Instance, ResultStatus};
use grida_fx_core::host::NodeHost;
use grida_fx_core::money::Usd;
use grida_fx_core::plan::Plan;
use grida_fx_core::project::Planner;
use grida_fx_core::val::Val;
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

/// The `where` of the `problem` a lost stand-in answerer writes.
const STAND_IN_WHERE: &str = "stand_in";

/// How to run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOptions {
    /// The run folder, absolute.
    pub folder: PathBuf,
    /// How messages name it (as typed, or relative to the working directory).
    pub label: String,
    /// `--yes-up-to`.
    pub yes_up_to: Option<Usd>,
    /// The workflow's takes file, project-relative (recorded in `plan.json`).
    pub takes_file: String,
}

/// How an invocation ended.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    pub folder: PathBuf,
    pub ok: bool,
    pub incomplete: bool,
    pub stopped: Option<String>,
    /// Charged in this folder over every invocation.
    pub charged: Usd,
    /// `(instance id, error)` for each instance this invocation failed: in the expansion's
    /// listing order when the run ended, else in the order they failed.
    pub failed: Vec<(String, Option<String>)>,
    /// The declared outputs that exist.
    pub outputs: IndexMap<String, Val>,
    /// Ctrl-C stopped it.
    pub cancelled: bool,
}

/// Why a run did not start, or broke.
#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    /// Printed `refused: <message>`; exit 1.
    Refused(String),
    /// `grida-fx: <message>`; exit 2.
    Fatal(Error),
}

/// Runs a plan (module doc). `host` is the planning host (re-expansion may describe again).
pub fn run(
    engine: Arc<Engine>,
    planner: &mut Planner,
    host: &mut dyn NodeHost,
    plan: Plan,
    options: RunOptions,
) -> Result<RunOutcome, RunError> {
    let outcome = invoke(&engine, planner, host, plan, &options);
    engine.handle.block_on(engine.hosts.shutdown());
    outcome
}

/// The refusals that come before anything is written (module doc, 1 to 3), with the process's
/// environment: a command checks them before it makes a new folder, so a refused run leaves
/// nothing behind.
pub fn refused(plan: &Plan, live: bool) -> Option<String> {
    let env = |name: &str| std::env::var(name).ok();
    refusal(plan, live, &env)
}

/// The refusals that come before anything is written (module doc, 1 to 3). `env` reads the
/// environment (tool overrides, `PATH`).
fn refusal(plan: &Plan, live: bool, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if !plan.ok() {
        let lines: Vec<String> = plan.problems.iter().map(Problem::to_string).collect();
        return Some(format!("the plan is refused:\n{}", lines.join("\n")));
    }
    if live && plan.ceiling.is_none() {
        return Some(
            "a live run needs a ceiling: pass --max-usd, or set budget.max_usd in the workflow or \
             in fx.yaml"
                .to_string(),
        );
    }
    let mut missing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for instance in plan.live() {
        for entry in &instance.ty.spec.tools {
            let name = crate::tools::tool_name(entry);
            if crate::tools::resolve_tool(name, env).is_none() {
                missing
                    .entry(name.to_string())
                    .or_default()
                    .insert(instance.step.clone());
            }
        }
    }
    if missing.is_empty() {
        return None;
    }
    let lines: Vec<String> = missing
        .iter()
        .map(|(name, steps)| {
            let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
            format!(
                "{name} (for {}): install it, or set {}",
                steps.join(", "),
                crate::tools::tool_env_var(name)
            )
        })
        .collect();
    Some(format!(
        "a program the run needs is not installed:\n{}",
        lines.join("\n")
    ))
}

/// `<label>/<name>`: a file of the run folder as messages name it.
fn in_folder(label: &str, name: &str) -> String {
    if label.is_empty() {
        name.to_string()
    } else if label.ends_with('/') {
        format!("{label}{name}")
    } else {
        format!("{label}/{name}")
    }
}

/// Places the stored file `digest` at `relative` in the folder; the error is a sentence that
/// names the place as the folder's label does.
fn place_file(
    store: &Store,
    folder: &RunFolder,
    relative: &str,
    digest: &str,
) -> Result<(), String> {
    let place = in_folder(&folder.label, relative);
    let source = store
        .file_path(digest)
        .map_err(|error| format!("cannot place {place}: {error}"))?;
    folder
        .place(relative, &source)
        .map_err(|error| format!("cannot place {place}: {}", io_reason(&error)))
}

/// Expands the workflow with every result so far.
fn expand(planner: &mut Planner, host: &mut dyn NodeHost) -> grida_fx_core::Result<Expansion> {
    grida_fx_core::expand::expand(ExpandEnv {
        workflow: &planner.workflow,
        inputs: &planner.inputs.values,
        registry: &mut planner.registry,
        host,
        routes: &planner.routes,
        takes: &planner.takes,
        results: &planner.results,
    })
}

/// The step budgets of an instance id in an expansion (empty when it lists no such instance).
fn scopes_of(expansion: &Expansion, id: &str) -> Scopes {
    expansion
        .instances
        .get(id)
        .and_then(|instance| instance.budget.clone())
        .into_iter()
        .collect()
}

/// The `node_started` event of an instance about to be dispatched.
fn node_started(instance: &Instance) -> Event {
    Event::NodeStarted {
        id: instance.id.clone(),
        path: instance.path.clone(),
        step: instance.step.clone(),
        take: instance.takes.clone(),
        identity: instance.identity.clone(),
        uses: instance.uses.clone(),
        reads: instance.inputs_from().into_iter().collect(),
        routes: instance
            .routes
            .iter()
            .map(|(capability, route)| (capability.clone(), route.id()))
            .collect(),
        with: instance
            .with
            .iter()
            .map(|(name, value)| (name.clone(), crate::events::encode(value)))
            .collect(),
    }
}

/// The `problem` message of an instance that never ran.
fn never_ran(waits: &[String]) -> String {
    if waits.is_empty() {
        "never ran".to_string()
    } else {
        format!("never ran: it waits on {}", waits.join(", "))
    }
}

/// The outputs that exist: neither pending nor missing.
fn existing_outputs(outputs: &IndexMap<String, Val>) -> IndexMap<String, Val> {
    outputs
        .iter()
        .filter(|(_, value)| !value.contains_pending() && !matches!(value, Val::Missing))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// One invocation, before the host pool is shut down.
fn invoke(
    engine: &Arc<Engine>,
    planner: &mut Planner,
    host: &mut dyn NodeHost,
    plan: Plan,
    options: &RunOptions,
) -> Result<RunOutcome, RunError> {
    let env = |name: &str| std::env::var(name).ok();
    if let Some(refused) = refusal(&plan, engine.live, &env) {
        return Err(RunError::Refused(refused));
    }
    let digest = grida_fx_core::plan::output::plan_digest(&plan, planner).ok_or_else(|| {
        RunError::Fatal(Error::internal(
            "the plan has no digest: an input holds a value only a run produces",
        ))
    })?;
    // Listening starts before anything is written, so an interruption is never missed.
    let cancel = Cancel::new();
    let (sender, receiver) = mpsc::unbounded_channel();
    let interrupted = Arc::new(AtomicBool::new(false));
    let watcher = {
        let cancel = cancel.clone();
        let sender = sender.clone();
        let interrupted = Arc::clone(&interrupted);
        engine.handle.spawn(async move {
            if interruption().await {
                interrupted.store(true, Ordering::SeqCst);
                cancel.cancel();
                let _ = sender.send(Message::Interrupted);
            }
        })
    };
    // A stand-in run stops when its answerer goes away while the run still runs.
    let answerer = engine.stand_in.as_ref().map(|stand_in| {
        let stand_in = Arc::clone(stand_in);
        let sender = sender.clone();
        engine.handle.spawn(async move {
            stand_in.lost().await;
            let _ = sender.send(Message::AnswererLost);
        })
    });
    let watchers = move || {
        watcher.abort();
        if let Some(answerer) = &answerer {
            answerer.abort();
        }
    };
    let started = prepare(engine, planner, &plan, options, &digest);
    let (folder, prior) = match started {
        Ok(started) => started,
        Err(error) => {
            watchers();
            return Err(error);
        }
    };
    let events_label = in_folder(&options.label, "events.jsonl");
    planner
        .results
        .extend(replay::replay(&prior, &engine.store));
    let invocation_id = crate::events::new_invocation_id();
    let log = match EventLog::open(&folder.events_path(), &invocation_id, &digest) {
        Ok(log) => Arc::new(log),
        Err(error) => {
            watchers();
            return Err(RunError::Fatal(Error::io(&events_label, &error)));
        }
    };
    let first = expand(planner, host);
    let ledger = Arc::new(Ledger::new(plan.ceiling, Some(Arc::clone(&log))));
    let scoped = first.as_ref().unwrap_or(&plan.expansion);
    ledger.replay(&prior, &|id: &str| scopes_of(scoped, id));
    let services = Arc::new(Services::run(
        Arc::clone(engine),
        Arc::clone(&ledger),
        Arc::clone(&log),
        invocation_id,
        cancel.clone(),
    ));
    let workflow = planner.workflow.workflow.id.clone();
    let mut scheduler = Scheduler {
        engine: Arc::clone(engine),
        planner,
        host,
        options,
        folder: Arc::new(folder),
        log,
        ledger,
        services,
        events_label,
        workflow,
        sender,
        receiver,
        running: IndexMap::new(),
        overturned: IndexMap::new(),
        groups: schedule::Groups::default(),
        gate: schedule::Gate::new(),
        failed: Vec::new(),
        tasks: Vec::new(),
        interrupted,
    };
    let estimate = plan.estimate();
    let started = Event::RunStarted {
        workflow: scheduler.workflow.clone(),
        resumed: !prior.is_empty(),
        ceiling_usd: plan.ceiling,
        charged_usd: scheduler.ledger.charged(),
        estimate_low: estimate.low,
        estimate_high: estimate.high,
        stand_in: engine.stand_in_run(),
    };
    let outcome = match scheduler.emit(&started) {
        Ok(()) => scheduler.conclude(first),
        Err(error) => Err(RunError::Fatal(error)),
    };
    watchers();
    scheduler.join();
    outcome
}

/// Resolves on Ctrl-C (SIGINT) or, where there is one, SIGTERM; `false` when neither can be
/// listened for.
async fn interruption() -> bool {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            return tokio::select! {
                interrupted = tokio::signal::ctrl_c() => interrupted.is_ok(),
                Some(()) = terminate.recv() => true,
            };
        }
    }
    tokio::signal::ctrl_c().await.is_ok()
}

/// The folder, locked and checked, and its earlier events, with `plan.json` written (module doc:
/// refusals 4 to 6, and what comes before the event log).
fn prepare(
    engine: &Arc<Engine>,
    planner: &Planner,
    plan: &Plan,
    options: &RunOptions,
    digest: &str,
) -> Result<(RunFolder, Vec<serde_json::Value>), RunError> {
    let folder = RunFolder::lock(&options.folder, &options.label)
        .map_err(|refused| RunError::Refused(refused.0))?;
    crate::folder::check_plan(&options.folder, &options.label, digest)
        .map_err(|refused| RunError::Refused(refused.0))?;
    crate::folder::check_mode(&options.folder, &options.label, engine.stand_in_run())
        .map_err(|refused| RunError::Refused(refused.0))?;
    let events_label = in_folder(&options.label, "events.jsonl");
    // The reader's sentences name `events.jsonl`; the folder says which one.
    let (prior, torn) =
        crate::events::read_events_noting(&folder.events_path()).map_err(|message| {
            RunError::Fatal(Error::new(
                ErrorKind::Io,
                format!("{}: {message}", options.label),
            ))
        })?;
    if torn {
        eprintln!(
            "note: {events_label} ended in a line an earlier invocation did not finish writing; \
             it was left out"
        );
    }
    let another = prior
        .iter()
        .any(|event| event.get("plan").and_then(serde_json::Value::as_str) != Some(digest));
    if another {
        return Err(RunError::Refused(format!(
            "{} holds a run of another workflow or other inputs; choose a new folder",
            options.label
        )));
    }
    folder.sweep();
    engine.store.sweep();
    let document = crate::folder::plan_document(
        plan,
        planner,
        digest,
        &options.takes_file,
        engine.stand_in_run(),
    );
    folder.write_plan_once(&document).map_err(|error| {
        RunError::Fatal(Error::io(&in_folder(&options.label, "plan.json"), &error))
    })?;
    Ok((folder, prior))
}

/// One running instance: the concurrency group it takes a place in, and its own cancellation
/// (a child of the run's).
struct Running {
    group: Option<String>,
    cancel: Cancel,
    /// The instance path, for the events the scheduler writes for it.
    path: String,
}

/// What reaches the loop.
#[derive(Debug)]
enum Message {
    Done(Box<Done>),
    Interrupted,
    /// A stand-in run's answerer is gone ([`crate::stand_in::StandIn::lost`]).
    AnswererLost,
}

/// The loop's state (module doc).
struct Scheduler<'a> {
    engine: Arc<Engine>,
    planner: &'a mut Planner,
    host: &'a mut dyn NodeHost,
    options: &'a RunOptions,
    folder: Arc<RunFolder>,
    log: Arc<EventLog>,
    ledger: Arc<Ledger>,
    services: Arc<Services>,
    /// `<folder>/events.jsonl` as messages name it.
    events_label: String,
    /// The workflow id.
    workflow: String,
    sender: mpsc::UnboundedSender<Message>,
    receiver: mpsc::UnboundedReceiver<Message>,
    /// Running instances by id.
    running: IndexMap<String, Running>,
    /// Running instances a run-time assertion failed and the run stopped: the assertion's
    /// message (module doc).
    overturned: IndexMap<String, String>,
    groups: schedule::Groups,
    gate: schedule::Gate,
    /// What this invocation failed, in the order it failed (in listing order once it ends).
    failed: Vec<(String, Option<String>)>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Set by Ctrl-C.
    interrupted: Arc<AtomicBool>,
}

impl Scheduler<'_> {
    /// Writes one event; a failure is an engine fault.
    fn emit(&self, event: &Event) -> Result<(), Error> {
        self.log
            .emit(event)
            .map_err(|error| Error::io(&self.events_label, &error))
    }

    fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    fn running_ids(&self) -> HashSet<String> {
        self.running.keys().cloned().collect()
    }

    /// The loop, then the end of the invocation (module doc).
    fn conclude(
        &mut self,
        first: grida_fx_core::Result<Expansion>,
    ) -> Result<RunOutcome, RunError> {
        let driven = first.and_then(|expansion| self.drive(expansion));
        let stopped = match driven {
            Ok(stopped) => stopped,
            Err(fault) => return Err(self.fault(fault)),
        };
        if stopped.is_some() || self.interrupted() {
            self.drain();
        }
        if self.interrupted() {
            self.settle_calls();
            let cancelled = Event::RunCancelled {
                reason: "interrupted".to_string(),
                charged_usd: self.ledger.charged(),
            };
            self.emit(&cancelled).map_err(RunError::Fatal)?;
            return Ok(RunOutcome {
                folder: self.options.folder.clone(),
                ok: false,
                incomplete: true,
                stopped: None,
                charged: self.ledger.charged(),
                failed: self.failed.clone(),
                outputs: IndexMap::new(),
                cancelled: true,
            });
        }
        self.close(stopped).map_err(|fault| self.fault(fault))
    }

    /// Runs the loop until nothing runs and nothing can start; the stop message, if the run
    /// stopped. Returns at once on Ctrl-C (the flag says so).
    fn drive(&mut self, first: Expansion) -> Result<Option<String>, Error> {
        let mut next = Some(first);
        loop {
            if self.interrupted() {
                return Ok(None);
            }
            let expansion = match next.take() {
                Some(expansion) => expansion,
                None => expand(self.planner, self.host)?,
            };
            if !expansion.problems.is_empty() {
                let problems = unique(expansion.problems);
                for problem in &problems {
                    self.emit(&Event::Problem {
                        where_: problem.where_.clone(),
                        message: problem.message.clone(),
                    })?;
                }
                return Ok(Some(format!(
                    "a value the run produced breaks the workflow: {}",
                    problems[0]
                )));
            }
            let running = self.running_ids();
            self.stop_overturned(&expansion);
            let mut settled = schedule::settle(&expansion, &self.planner.results, &running);
            settled.extend(schedule::overturned(
                &expansion,
                &self.planner.results,
                &running,
            ));
            if !settled.is_empty() {
                for settled in settled {
                    self.emit(&settled.event)?;
                    self.failed
                        .push((settled.id.clone(), settled.result.error.clone()));
                    self.planner.results.insert(settled.id, settled.result);
                }
                // What reads them settles once they are known.
                continue;
            }
            let check = self.gate.check(
                &expansion,
                &self.planner.results,
                self.ledger.charged(),
                self.ledger.held(),
                self.options.yes_up_to,
            );
            for planned in &check.planned {
                self.emit(planned)?;
            }
            let mut failed_at_once = false;
            // Nothing new starts once a person stopped the run.
            if check.stop.is_none() && !self.interrupted() {
                let ready =
                    schedule::ready(&expansion, &self.planner.results, &running, &self.groups);
                for id in ready {
                    if let Some(instance) = expansion.instances.get(&id) {
                        failed_at_once |= !self.start(instance)?;
                    }
                }
            }
            if self.running.is_empty() {
                if check.stop.is_some() {
                    return Ok(check.stop);
                }
                if failed_at_once {
                    continue;
                }
                return Ok(None);
            }
            let messages = self.wait();
            if messages.is_empty() {
                // Every sender is gone, so nothing running can report any more.
                return Err(Error::internal(
                    "the run lost track of the steps it started",
                ));
            }
            let mut stop = None;
            let mut lost = false;
            for message in messages {
                match message {
                    Message::Done(done) => {
                        if let Some(stopped) = self.finish(*done)? {
                            stop.get_or_insert(stopped);
                        }
                    }
                    Message::AnswererLost => lost = true,
                    Message::Interrupted => {}
                }
            }
            // A call that met the loss first has already stopped the run with it.
            if lost && stop.is_none() {
                self.emit(&Event::Problem {
                    where_: STAND_IN_WHERE.to_string(),
                    message: ANSWERER_EXITED.to_string(),
                })?;
                stop = Some(ANSWERER_EXITED.to_string());
            }
            self.check_log()?;
            if stop.is_some() {
                return Ok(stop);
            }
        }
    }

    /// An event some part of the run could not write is an engine fault (module doc).
    fn check_log(&self) -> Result<(), Error> {
        match self.log.fault() {
            Some(fault) => Err(Error::new(
                ErrorKind::Io,
                format!("{}: {fault}", self.events_label),
            )),
            None => Ok(()),
        }
    }

    /// Stops each running instance whose state a run-time assertion turned `failed` (module doc).
    fn stop_overturned(&mut self, expansion: &Expansion) {
        for (id, running) in &self.running {
            if self.overturned.contains_key(id) {
                continue;
            }
            if let Some(error) = expansion
                .instances
                .get(id)
                .and_then(schedule::assertion_failed)
            {
                self.overturned.insert(id.clone(), error);
                running.cancel.cancel();
            }
        }
    }

    /// Waits until no paid call of the invocation runs (module doc).
    fn settle_calls(&self) {
        self.engine.handle.block_on(self.services.calls_settled());
    }

    /// Dispatches an instance; `false` when its job could not be made and it failed at once.
    fn start(&mut self, instance: &Instance) -> Result<bool, Error> {
        let env = |name: &str| std::env::var(name).ok();
        let job = match InstanceJob::from_instance(instance, self.planner, &env) {
            Ok(job) => job,
            Err(error) => {
                let error = self.engine.scrub(&error);
                self.emit(&Event::NodeFailed {
                    id: instance.id.clone(),
                    path: instance.path.clone(),
                    error: Some(error.clone()),
                    code: None,
                    facts: None,
                    duration_ms: None,
                })?;
                self.planner
                    .results
                    .insert(instance.id.clone(), crate::executor::failed(&error));
                self.failed.push((instance.id.clone(), Some(error)));
                return Ok(false);
            }
        };
        let group = schedule::group_of(instance).map(|(group, _)| group.to_string());
        if let Some(group) = &group {
            self.groups.start(group);
        }
        // `Cancel::child` follows its parent from a task of the runtime.
        let cancel = {
            let _runtime = self.engine.handle.enter();
            self.services.cancel.child()
        };
        self.running.insert(
            instance.id.clone(),
            Running {
                group,
                cancel: cancel.clone(),
                path: instance.path.clone(),
            },
        );
        let started = node_started(instance);
        let services = Arc::clone(&self.services);
        let folder = Arc::clone(&self.folder);
        let sender = self.sender.clone();
        let id = instance.id.clone();
        let task = self.engine.handle.spawn(async move {
            let dispatched = tokio::spawn(dispatch::dispatch(
                services,
                Arc::new(job),
                started,
                folder,
                cancel,
            ));
            let done = match dispatched.await {
                Ok(done) => done,
                // A fault in the engine itself: the run stops with a terminal event.
                Err(_) => {
                    let stop = format!("the engine failed while running {id}");
                    Done {
                        id,
                        result: crate::executor::failed(&stop),
                        stop: Some(stop),
                        cancelled: false,
                    }
                }
            };
            let _ = sender.send(Message::Done(Box::new(done)));
        });
        self.tasks.push(task);
        Ok(true)
    }

    /// Waits for at least one message, then takes every one already there.
    fn wait(&mut self) -> Vec<Message> {
        let mut messages = Vec::new();
        if let Some(message) = self.receiver.blocking_recv() {
            messages.push(message);
        }
        while let Ok(message) = self.receiver.try_recv() {
            messages.push(message);
        }
        messages
    }

    /// Takes a completion in: its group place back, its result kept, its failure counted. Its
    /// stop, when it has one, is written as a `problem` and returned.
    fn finish(&mut self, done: Done) -> Result<Option<String>, Error> {
        let running = self.running.shift_remove(&done.id);
        let path = running
            .as_ref()
            .map_or_else(|| done.id.clone(), |running| running.path.clone());
        if let Some(group) = running.and_then(|running| running.group) {
            self.groups.finish(&group);
        }
        let overturned = self.overturned.shift_remove(&done.id);
        if done.cancelled
            && !self.services.cancel.is_cancelled()
            && let Some(error) = overturned
        {
            // Stopped by its own assertion: the failure is the assertion's.
            self.emit(&Event::NodeFailed {
                id: done.id.clone(),
                path,
                error: Some(error.clone()),
                code: None,
                facts: None,
                duration_ms: None,
            })?;
            self.failed.push((done.id.clone(), Some(error.clone())));
            self.planner
                .results
                .insert(done.id.clone(), crate::executor::failed(&error));
            return Ok(None);
        }
        if !done.cancelled {
            if done.result.status == ResultStatus::Failed {
                self.failed
                    .push((done.id.clone(), done.result.error.clone()));
            }
            self.planner.results.insert(done.id.clone(), done.result);
        }
        match done.stop {
            Some(stop) => {
                self.emit(&Event::Problem {
                    where_: done.id,
                    message: stop.clone(),
                })?;
                Ok(Some(stop))
            }
            None => Ok(None),
        }
    }

    /// Cancels what runs and waits for every dispatch to report, keeping what finished.
    fn drain(&mut self) {
        self.services.cancel.cancel();
        while !self.running.is_empty() {
            let messages = self.wait();
            if messages.is_empty() {
                break;
            }
            for message in messages {
                if let Message::Done(done) = message {
                    let _ = self.finish(*done);
                }
            }
        }
    }

    /// The end of an invocation that was not cancelled (module doc).
    fn close(&mut self, stopped: Option<String>) -> Result<RunOutcome, Error> {
        let expansion = expand(self.planner, self.host)?;
        in_listing_order(&mut self.failed, &expansion);
        let mut stopped = stopped;
        let mut incomplete =
            stopped.is_some() || expansion.outputs.values().any(Val::contains_pending);
        if stopped.is_none() {
            for (id, waits) in schedule::stuck(&expansion, &self.planner.results) {
                let path = expansion
                    .instances
                    .get(&id)
                    .map_or_else(|| id.clone(), |instance| instance.path.clone());
                self.emit(&Event::Problem {
                    where_: path,
                    message: never_ran(&waits),
                })?;
                incomplete = true;
            }
        }
        for (name, value) in &expansion.outputs {
            for (relative, digest) in crate::folder::output_files(name, value) {
                if let Err(sentence) =
                    place_file(&self.engine.store, &self.folder, &relative, &digest)
                {
                    self.emit(&Event::Problem {
                        where_: format!("outputs.{name}"),
                        message: sentence.clone(),
                    })?;
                    stopped.get_or_insert(sentence);
                    incomplete = true;
                }
            }
        }
        let ok = self.failed.is_empty() && !incomplete;
        let outputs = existing_outputs(&expansion.outputs);
        self.settle_calls();
        self.check_log()?;
        let charged = self.ledger.charged();
        self.emit(&Event::RunFinished {
            ok,
            incomplete,
            stopped: stopped.clone(),
            charged_usd: charged,
            failed: self.failed.iter().map(|(id, _)| id.clone()).collect(),
            outputs: outputs
                .iter()
                .map(|(name, value)| (name.clone(), crate::events::encode(value)))
                .collect(),
        })?;
        Ok(RunOutcome {
            folder: self.options.folder.clone(),
            ok,
            incomplete,
            stopped,
            charged,
            failed: self.failed.clone(),
            outputs,
            cancelled: false,
        })
    }

    /// An engine fault after the lock (module doc): what runs is cancelled and drained, and the
    /// fault is written where it still can be.
    fn fault(&mut self, fault: Error) -> RunError {
        self.drain();
        self.settle_calls();
        let _ = self.emit(&Event::Problem {
            where_: self.workflow.clone(),
            message: fault.message.clone(),
        });
        let _ = self.emit(&Event::RunFinished {
            ok: false,
            incomplete: true,
            stopped: Some(fault.message.clone()),
            charged_usd: self.ledger.charged(),
            failed: self.failed.iter().map(|(id, _)| id.clone()).collect(),
            outputs: IndexMap::new(),
        });
        RunError::Fatal(fault)
    }

    /// Waits for every task this invocation spawned, so none outlives it.
    fn join(&mut self) {
        for task in self.tasks.drain(..) {
            let _ = self.engine.handle.block_on(task);
        }
    }
}

/// Orders what an invocation failed as the expansion lists its instances, so `run_finished` and
/// the outcome do not depend on which of several concurrent steps ended first. An id the
/// expansion does not list keeps its place after the others.
fn in_listing_order(failed: &mut [(String, Option<String>)], expansion: &Expansion) {
    failed.sort_by_key(|(id, _)| {
        expansion
            .instances
            .get_index_of(id.as_str())
            .unwrap_or(usize::MAX)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::expand::State;
    use grida_fx_core::registry::{ResolvedType, TypeOrigin};
    use grida_fx_core::spec::{BodyKind, NodeSpec, Retry};
    use std::collections::HashMap;
    use std::rc::Rc;

    fn instance(id: &str, step: &str, tools: &[&str], state: State) -> Instance {
        let spec = NodeSpec {
            name: "render".into(),
            description: None,
            inputs: IndexMap::new(),
            params: IndexMap::new(),
            outputs: IndexMap::new(),
            judge: false,
            capability: None,
            calls: IndexMap::new(),
            resources: Vec::new(),
            tools: tools.iter().map(|t| t.to_string()).collect(),
            view: None,
            version: Some(1),
            retry: Retry::Service,
        };
        Instance {
            id: id.into(),
            path: step.into(),
            step: step.into(),
            takes: vec![1],
            uses: "./nodes/render.py#render".into(),
            ty: Rc::new(ResolvedType {
                uses: "./nodes/render.py#render".into(),
                identity: "source:0".into(),
                spec: Rc::new(spec),
                origin: TypeOrigin::Project {
                    path: "nodes/render.py".into(),
                    attribute: "render".into(),
                },
                body: BodyKind::Project,
                source: None,
                drift: None,
            }),
            with: IndexMap::new(),
            needs: Vec::new(),
            state,
            identity: None,
            routes: IndexMap::new(),
            prices: Vec::new(),
            phase: 1,
            key: None,
            judges: None,
            judge_policy: None,
            judged_by: Vec::new(),
            view: serde_json::Value::Bool(false),
            at_plan: false,
            budget: None,
            concurrency_group: None,
            concurrency: None,
            timeout_s: None,
            reason: None,
            reads: BTreeSet::new(),
        }
    }

    fn plan(instances: Vec<Instance>, problems: Vec<Problem>, ceiling: Option<Usd>) -> Plan {
        Plan {
            expansion: Expansion {
                instances: instances.into_iter().map(|i| (i.id.clone(), i)).collect(),
                ..Expansion::default()
            },
            problems,
            ceiling,
            cached: BTreeSet::new(),
        }
    }

    fn env_of(vars: HashMap<String, String>) -> impl Fn(&str) -> Option<String> {
        move |name| vars.get(name).cloned()
    }

    #[test]
    fn refusals_in_order() {
        let nothing = env_of(HashMap::new());
        let problems = vec![
            Problem::new("draw.with.prompt", "image.generate needs setting prompt"),
            Problem::new("join", "no step x"),
        ];
        let tooled = vec![instance("a#1", "a", &["blender>=4.2"], State::Planned)];
        assert_eq!(
            refusal(&plan(tooled.clone(), problems, None), true, &nothing).as_deref(),
            Some(
                "the plan is refused:\ndraw.with.prompt: image.generate needs setting \
                 prompt\njoin: no step x"
            )
        );
        assert_eq!(
            refusal(&plan(tooled.clone(), Vec::new(), None), true, &nothing).as_deref(),
            Some(
                "a live run needs a ceiling: pass --max-usd, or set budget.max_usd in the \
                 workflow or in fx.yaml"
            )
        );
        let ceiling = Some(Usd::parse("1").unwrap());
        assert!(
            refusal(&plan(tooled, Vec::new(), ceiling), true, &nothing)
                .unwrap()
                .starts_with("a program the run needs is not installed:")
        );
        assert_eq!(
            refusal(&plan(Vec::new(), Vec::new(), None), false, &nothing),
            None
        );
    }

    #[test]
    fn missing_tools_one_line_each_sorted_with_their_steps() {
        let dir = tempfile::tempdir().unwrap();
        let found = dir.path().join("magick");
        std::fs::write(&found, "").unwrap();
        let env = env_of(HashMap::from([(
            "GRIDA_FX_TOOL_MAGICK".to_string(),
            found.to_str().unwrap().to_string(),
        )]));
        let instances = vec![
            instance(
                "b#1",
                "scene['b'].render",
                &["blender>=4.2", "magick"],
                State::Planned,
            ),
            instance("a#1", "scene['a'].render", &["blender"], State::Maybe),
            instance("c#1", "audio-mix", &["sox-ng"], State::Planned),
            instance("d#1", "cut", &["ffmpeg"], State::Absent),
            instance("e#1", "zed", &["blender"], State::Planned),
        ];
        assert_eq!(
            refusal(&plan(instances, Vec::new(), None), false, &env).as_deref(),
            Some(
                "a program the run needs is not installed:\nblender (for scene['a'].render, \
                 scene['b'].render, zed): install it, or set GRIDA_FX_TOOL_BLENDER\nsox-ng (for \
                 audio-mix): install it, or set GRIDA_FX_TOOL_SOX_NG"
            )
        );
    }

    #[test]
    fn folder_labels() {
        assert_eq!(
            in_folder("runs/one", "events.jsonl"),
            "runs/one/events.jsonl"
        );
        assert_eq!(in_folder("runs/one/", "plan.json"), "runs/one/plan.json");
        assert_eq!(in_folder("", "plan.json"), "plan.json");
    }

    #[test]
    fn instances_that_never_ran() {
        assert_eq!(never_ran(&[]), "never ran");
        assert_eq!(
            never_ran(&["a#1".to_string(), "b['x']#2".to_string()]),
            "never ran: it waits on a#1, b['x']#2"
        );
    }

    #[test]
    fn existing_outputs_leave_out_pending_and_missing_values() {
        let pending = Val::Pending(Box::new(grida_fx_core::val::Pending::of("a#1", None)));
        let outputs: IndexMap<String, Val> = [
            ("one".to_string(), Val::Number(1.0)),
            ("later".to_string(), pending.clone()),
            (
                "some".to_string(),
                Val::List(vec![Val::Number(2.0), pending]),
            ),
            ("gone".to_string(), Val::Missing),
            ("bad".to_string(), Val::Failed("b#1".into())),
            ("none".to_string(), Val::Null),
        ]
        .into();
        let existing = existing_outputs(&outputs);
        let kept: Vec<&String> = existing.keys().collect();
        assert_eq!(kept, ["one", "bad", "none"]);
    }

    #[test]
    fn node_started_lists_reads_routes_and_with() {
        let mut i = instance("join#1", "join", &[], State::Planned);
        i.reads = ["b#1".to_string(), "join#1".to_string()].into();
        i.needs = vec!["c#1".into()];
        i.with.insert(
            "parts".into(),
            Val::Pending(Box::new(grida_fx_core::val::Pending::of("a#1", None))),
        );
        let Event::NodeStarted {
            id,
            take,
            reads,
            routes,
            with,
            ..
        } = node_started(&i)
        else {
            panic!();
        };
        assert_eq!(id, "join#1");
        assert_eq!(take, [1]);
        assert_eq!(reads, ["a#1", "b#1", "c#1"]);
        assert!(routes.is_empty());
        assert_eq!(with.keys().collect::<Vec<_>>(), ["parts"]);
    }

    #[test]
    fn budgets_by_instance_id() {
        let mut scoped = instance("scene['a'].draw#1", "scene['a'].draw", &[], State::Planned);
        scoped.budget = Some(("scene['a']".into(), Usd::parse("0.5").unwrap()));
        let e = Expansion {
            instances: [(scoped.id.clone(), scoped)].into_iter().collect(),
            ..Expansion::default()
        };
        assert_eq!(
            scopes_of(&e, "scene['a'].draw#1"),
            vec![("scene['a']".to_string(), Usd::parse("0.5").unwrap())]
        );
        assert!(scopes_of(&e, "other#1").is_empty());
    }

    #[test]
    fn failures_are_listed_in_listing_order() {
        let e = Expansion {
            instances: ["ok#1", "nope#1", "bang#1", "after#1"]
                .into_iter()
                .map(|id| {
                    let step = id.trim_end_matches("#1");
                    (id.to_string(), instance(id, step, &[], State::Planned))
                })
                .collect(),
            ..Expansion::default()
        };
        // In the order they happened to fail; an id the expansion does not list goes last.
        let mut failed: Vec<(String, Option<String>)> = ["gone#1", "bang#1", "after#1", "nope#1"]
            .into_iter()
            .map(|id| (id.to_string(), Some(format!("{id} failed"))))
            .collect();
        in_listing_order(&mut failed, &e);
        let ids: Vec<&str> = failed.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["nope#1", "bang#1", "after#1", "gone#1"]);
        assert_eq!(failed[0].1.as_deref(), Some("nope#1 failed"));
    }
}
