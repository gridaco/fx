//! `grida-fx inspect <run | workflow id | workflow id/name> [--verify] [--json]` (spec/store.md §8).
//!
//! An existing folder or explicit path is inspected directly. Otherwise a workflow ID opens
//! its newest-created recorded run, including failures, and `workflow/name` opens its named
//! run. Distinct recorded sources sharing an ID are ambiguous and require an explicit path.
//! Creation time comes from the first `run_started`, falling back to the immutable plan file's
//! modification time for older records; resume does not reorder history. Discovery is bounded
//! to the configured runs tree and does not follow symlinks. A folder without a readable
//! fx-graph-v1 `plan.json` is `<run> is not a run folder` (exit 2). The log is read as `project`
//! reads it; a folder with no log has run nothing.
//! Ordinary inspection writes nothing. `--register` saves a catalog entry and reports its
//! route. Browser inspection may also register the run; standalone inspection serves it directly.
//!
//! The steps: each plan instance that is not absent, in plan order, then each instance only the
//! log names, in the order the log first names it. The last `node_*` event decides a step's state
//! (`node_finished` succeeded, `node_failed` failed, `node_skipped` skipped, `node_started`
//! running, or failed with `the run ended before this step finished` once the run ended); a
//! started step with an open wait for a route is `waiting` instead of running, for the route
//! of its earliest open wait (spec/store.md §8): a wait opens at a `call_waiting` and closes at
//! the `call_admitted` of the same `call`, or at the step's next `node_*` or `node_retry` event
//! or any `run_started`, which leave it behind; with no
//! event: `succeeded` for a plan state `done` (an `at: plan` step), else `skipped` once the run
//! ended (with the plan's reason), else `pending`. A failed or skipped step's error is the
//! event's `error`, else its `reason`. The run's state: `planned`, `unfinished` after
//! `run_started`, `succeeded`/`failed` after `run_finished` (by its `ok`), `cancelled` after
//! `run_cancelled`, or `paused` after `run_paused`. A verified draining owner after
//! `pause_requested` is `pausing`. `spent` records the last terminal invocation's `charged_usd`.
//! Failed managed attempts are read in event order, including failures from earlier invocations
//! and failures followed by a successful retry.
//! The text shows the latest recovery report for each call that is still pending recovery or
//! was recovered. JSON includes the full history.
//!
//! A succeeded step's files are where the run placed them (spec/store.md §8 "One file or
//! several": `files/<step>/<port><suffix>` for a port holding one file, else
//! `files/<step>/<port>/<label><suffix>`), from its `node_finished` outputs. `<step>` is the
//! step folder of the instance's takes, read from its id (`<step>#<takes>` unless they are `[1]`),
//! and each name is cut as the run cut it (`folder::step_file_path`).
//!
//! Text:
//! ```text
//! <workflow id>  ·  <folder name>[  ·  stand-in]
//! state     <run state>   <n> steps: <count state, …, sorted by state; or none>
//! booked    $<charged, 2 places>                   (from a terminal invocation)
//! failed    <id>: <error or "no reason recorded">  (each failed step)
//! attempt   <id> · attempt <n> [· send <n>] · <route> [· call <key>]
//! recovery  <id> · <status> · <original booked charge, when known>
//! differs   <id>: <relative path> is missing | differs from its recorded digest   (--verify)
//! verified  <n> files                              (--verify, nothing differs)
//! ```
//! `--json`: `{"run": {workflow, folder, state, charged_usd, steps: [{id, path, state, error?,
//! cache?, waiting_for?, files: [{path, digest, size}]}], call_attempt_failures?,
//! call_recoveries?, stand_in?},
//! "verification"?: {verified,
//! problems}}`, where `folder` is the folder as named, `charged_usd` is null before the run ended
//! and `waiting_for` is the route a `waiting` step waits for (when recorded). A
//! stand-in run (spec/store.md §8, "Stand-in runs"), whose `plan.json` or a `run_started` says
//! `stand_in: true`, has `  ·  stand-in` after the header's folder name and `stand_in: true` in
//! `run`. Exit 1 when
//! `--verify` found a problem, else 0 (a failed run included).

use crate::cli::InspectArgs;
use crate::print::{labelled, print_json, print_line, shown_path};
use crate::verbs::{event_name, read_log, read_plan, text, workflow_id};
use grida_fx_core::Error;
use grida_fx_core::docs::project::Project;
use grida_fx_core::money::Usd;
use grida_fx_runtime::folder::{
    holds_stand_in, keyed_path, step_file_path, step_folder, takes_of_id,
};
use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// Runs `grida-fx inspect`.
pub fn run(args: &InspectArgs) -> Result<u8, Error> {
    if args.requests {
        return super::waiting::inspect(args);
    }
    if args.control {
        return super::control::inspect(args);
    }
    let cwd = super::planning::working_directory()?;
    let (folder, label) = find_folder(&args.run, &cwd)?;
    let plan = read_plan(&folder, &label)?;
    let events = read_log(&folder, &label, false)?;
    let mut view = RunView::new(&plan, &events);
    if view.state == "unfinished" && grida_fx_runtime::observation::is_pausing(&folder, &events) {
        view.state = "pausing";
    }
    let requests = if args.verify || (args.json && !args.register) {
        super::waiting::requests(&folder)?
    } else {
        Vec::new()
    };
    // The finish event uses plain JSON for object outputs. The frozen choice retains
    // the file types needed to locate and verify any placements inside those objects.
    let selected: BTreeMap<_, _> = requests
        .iter()
        .filter_map(|request| {
            let grida_fx_runtime::waiting::RequestState::Answered { answer, .. } = &request.state
            else {
                return None;
            };
            Some((request.spec.step.as_str(), request.selected(answer)?))
        })
        .collect();
    for step in &mut view.steps {
        if step.state == "succeeded"
            && let Some(value) = selected.get(step.id.as_str())
        {
            let folder = step_folder(&step.path, &takes_of_id(&step.id));
            step.files = placed_port_files(&folder, "selected", value);
        }
    }
    let verification = args.verify.then(|| verify(&view, &folder));
    let receipt = args
        .register
        .then(|| {
            let project = Project::find(&cwd)?;
            super::service::register_run_receipt(&project, &folder, &plan, &events)
        })
        .transpose()?;
    if args.json {
        let document = if let Some(mut receipt) = receipt {
            if let Some(verification) = &verification {
                receipt["verification"] = verification.document();
            }
            receipt
        } else {
            let mut document = json_document(&view, &label, verification.as_ref());
            if !requests.is_empty() {
                document["run"]["requests"] = json!(requests);
            }
            document
        };
        print_json(&document);
    } else {
        let name = folder
            .components()
            .next_back()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_else(|| label.clone());
        for line in text_lines(&view, &name, verification.as_ref(), &events) {
            print_line(&line);
        }
        if let Some(receipt) = receipt {
            print_line(&labelled(
                "registered",
                receipt["route"].as_str().unwrap_or_default(),
            ));
        }
    }
    let status = u8::from(verification.is_some_and(|v| !v.problems.is_empty()));
    if args.open || args.standalone {
        let project = Project::find(&cwd)?;
        let declarations = project.declarations();
        if args.standalone || !project.has_file {
            super::view::serve(
                None,
                Some(&folder),
                0,
                args.open,
                &declarations,
                &project.root,
            )?;
        } else {
            super::view::report_service(
                super::service::register_run(&project, &folder),
                args.open,
                &declarations,
            );
        }
    }
    Ok(status)
}

/// The run folder `given` names, and how messages name it (module doc).
fn find_folder(given: &str, cwd: &Path) -> Result<(PathBuf, String), Error> {
    let folder = cwd.join(given);
    if folder.is_dir() {
        return Ok((folder, given.to_string()));
    }
    let parts: Vec<_> = Path::new(given)
        .components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect();
    let selector = match parts.as_slice() {
        [Component::Normal(id)] => id.to_str().map(|id| (id, None)),
        [Component::Normal(id), Component::Normal(name)] => id
            .to_str()
            .zip(name.to_str())
            .filter(|(_, name)| super::run_catalog::validate_name(name).is_ok())
            .map(|(id, name)| (id, Some(name))),
        _ => None,
    };
    if let Some((id, name)) = selector {
        let project = Project::find(cwd)?;
        if let Some(found) = super::run_catalog::resolve(&project.runs_dir(), id, name)? {
            let label = shown_path(&found, cwd);
            return Ok((found, label));
        }
        if name.is_none() {
            return Err(Error::usage(format!(
                "no run folder {given}, and no runs of a workflow {given} here"
            )));
        }
    }
    // An explicit path still reaches the normal recorded-run validation and error messages.
    Ok((folder, given.to_string()))
}

/// A file a succeeded step placed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Placed {
    /// Relative to the run folder, POSIX.
    path: String,
    digest: String,
    size: u64,
}

/// One step as the folder shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StepView {
    id: String,
    path: String,
    state: &'static str,
    error: Option<String>,
    diagnostic: Option<Value>,
    cache: Option<String>,
    /// The route a `waiting` step waits for.
    waiting_for: Option<String>,
    files: Vec<Placed>,
}

/// A run as its folder shows it (module doc).
#[derive(Debug, Clone, PartialEq)]
struct RunView {
    workflow: String,
    name: Option<String>,
    created_at: Option<String>,
    /// A stand-in run: its `plan.json` or a `run_started` says so.
    stand_in: bool,
    state: &'static str,
    /// The last `charged_usd` of a run that ended.
    charged: Option<Value>,
    steps: Vec<StepView>,
    call_attempt_failures: Vec<Value>,
    call_recoveries: Vec<Value>,
}

/// What the plan and the log say about one instance.
#[derive(Debug, Clone)]
struct Entry {
    path: String,
    plan_state: Option<String>,
    plan_reason: Option<String>,
    last: Option<Value>,
    /// The latest start's open waits for a route, in the order they began: each `call_waiting`'s
    /// call key (`None` when the event names none) and route.
    waits: IndexMap<Option<String>, Option<String>>,
}

impl RunView {
    fn new(plan: &Value, events: &[Value]) -> RunView {
        let mut entries: IndexMap<String, Entry> = IndexMap::new();
        let instances = plan.get("instances").and_then(Value::as_array);
        for instance in instances.into_iter().flatten() {
            let (Some(id), state) = (text(instance, "id"), text(instance, "state")) else {
                continue;
            };
            if state == Some("absent") {
                continue;
            }
            entries.insert(
                id.to_string(),
                Entry {
                    path: text(instance, "path").unwrap_or(id).to_string(),
                    plan_state: state.map(str::to_string),
                    plan_reason: text(instance, "reason").map(str::to_string),
                    last: None,
                    waits: IndexMap::new(),
                },
            );
        }
        let mut state = "planned";
        let mut charged = None;
        let mut stand_in = holds_stand_in(plan);
        let mut call_attempt_failures = Vec::new();
        let mut call_recoveries = Vec::new();
        for event in events {
            match event_name(event) {
                Some("run_started") => {
                    // A new invocation leaves every earlier wait behind.
                    for entry in entries.values_mut() {
                        entry.waits.clear();
                    }
                    state = "unfinished";
                    stand_in |= event.get("stand_in") == Some(&Value::Bool(true));
                }
                Some("run_finished") => {
                    let ok = event.get("ok").and_then(Value::as_bool) == Some(true);
                    state = if event["awaiting"]
                        .as_array()
                        .is_some_and(|ids| !ids.is_empty())
                    {
                        "awaiting"
                    } else if ok {
                        "succeeded"
                    } else {
                        "failed"
                    };
                    charged = event.get("charged_usd").cloned();
                }
                Some(name @ ("run_cancelled" | "run_paused")) => {
                    state = if name == "run_paused" {
                        "paused"
                    } else {
                        "cancelled"
                    };
                    charged = event.get("charged_usd").cloned();
                }
                Some(
                    name @ ("node_started" | "node_finished" | "node_failed" | "node_skipped"),
                ) => {
                    let Some(id) = text(event, "id") else {
                        continue;
                    };
                    let path = text(event, "path").unwrap_or(id);
                    let entry = entries.entry(id.to_string()).or_insert_with(|| Entry {
                        path: path.to_string(),
                        plan_state: None,
                        plan_reason: None,
                        last: None,
                        waits: IndexMap::new(),
                    });
                    if name == "node_started" {
                        entry.path = path.to_string();
                    }
                    entry.last = Some(event.clone());
                    entry.waits.clear();
                }
                Some("node_retry") => {
                    // A new attempt leaves its step's waits behind.
                    if let Some(entry) = text(event, "id").and_then(|id| entries.get_mut(id)) {
                        entry.waits.clear();
                    }
                }
                Some("waiting_request") => {
                    if let Some(entry) = text(event, "id").and_then(|id| entries.get_mut(id)) {
                        entry.last = Some(event.clone());
                    }
                }
                Some("call_attempt_failed") => call_attempt_failures.push(event.clone()),
                Some("call_recovery") => call_recoveries.push(event.clone()),
                Some(name @ ("call_waiting" | "call_admitted")) => {
                    // Only a step's start in progress waits for a route.
                    let Some(entry) =
                        text(event, "id")
                            .and_then(|id| entries.get_mut(id))
                            .filter(|entry| {
                                entry.last.as_ref().and_then(event_name) == Some("node_started")
                            })
                    else {
                        continue;
                    };
                    let call = text(event, "call").map(str::to_string);
                    if name == "call_waiting" {
                        let route = text(event, "route").map(str::to_string);
                        entry.waits.entry(call).or_insert(route);
                    } else {
                        entry.waits.shift_remove(&call);
                    }
                }
                _ => {}
            }
        }
        let ended = matches!(state, "succeeded" | "failed" | "cancelled");
        let steps = entries
            .into_iter()
            .map(|(id, entry)| step_view(id, entry, ended, state == "paused"))
            .collect();
        let first_start = events
            .iter()
            .find(|event| event_name(event) == Some("run_started"));
        RunView {
            name: first_start
                .and_then(|event| text(event, "name"))
                .map(str::to_string),
            created_at: first_start
                .and_then(|event| text(event, "created_at"))
                .map(str::to_string),
            workflow: workflow_id(plan).to_string(),
            stand_in,
            state,
            charged,
            steps,
            call_attempt_failures,
            call_recoveries,
        }
    }
}

/// One entry's state (module doc).
fn step_view(id: String, entry: Entry, ended: bool, paused: bool) -> StepView {
    let mut view = StepView {
        id,
        path: entry.path,
        state: "pending",
        error: None,
        diagnostic: None,
        cache: None,
        waiting_for: None,
        files: Vec::new(),
    };
    let error_of = |event: &Value| {
        text(event, "error")
            .filter(|e| !e.is_empty())
            .or_else(|| text(event, "reason"))
            .map(str::to_string)
    };
    match entry.last.as_ref().map(|e| (event_name(e), e)) {
        Some((Some("node_finished"), event)) => {
            view.state = "succeeded";
            view.cache = text(event, "cache").map(str::to_string);
            if let Some(outputs) = event.get("outputs").and_then(Value::as_object) {
                view.files = placed_files(&view.id, &view.path, outputs);
            }
        }
        Some((Some("node_failed"), event)) => {
            view.state = "failed";
            view.diagnostic = event.get("diagnostic").cloned();
            view.error = error_of(event);
        }
        Some((Some("node_skipped"), event)) => {
            view.state = "skipped";
            view.error = error_of(event);
        }
        Some((Some("waiting_request"), event))
            if !ended && text(event, "request_status") == Some("pending") =>
        {
            view.state = "awaiting";
        }
        Some((Some("node_started" | "waiting_request"), _)) if ended => {
            view.state = "failed";
            view.error = Some("the run ended before this step finished".into());
        }
        Some(_) if paused => {}
        Some(_) => match entry.waits.into_values().next() {
            Some(route) => {
                view.state = "waiting";
                view.waiting_for = route;
            }
            None => view.state = "running",
        },
        None if entry.plan_state.as_deref() == Some("done") => view.state = "succeeded",
        None if ended => {
            view.state = "skipped";
            view.error = entry.plan_reason;
        }
        None => {}
    }
    view
}

/// A file of an encoded value, with its label key when it has one.
struct EncodedFile {
    digest: String,
    kind: String,
    size: u64,
    key: Option<String>,
}

/// The files an encoded value holds, in order: a file; a list's items; a collection's items (an
/// item that is a file without a key of its own is labelled by the item's key).
fn encoded_files(value: &Value, item_key: Option<&str>, out: &mut Vec<EncodedFile>) {
    if let Some(file) = value.get("file") {
        let (Some(digest), Some(kind)) = (text(file, "digest"), text(file, "kind")) else {
            return;
        };
        let key = text(file, "key")
            .filter(|k| !k.is_empty())
            .or(item_key.filter(|k| !k.is_empty()))
            .map(str::to_string);
        out.push(EncodedFile {
            digest: digest.to_string(),
            kind: kind.to_string(),
            size: file.get("size").and_then(Value::as_u64).unwrap_or(0),
            key,
        });
    } else if let Some(items) = value.get("list").and_then(Value::as_array) {
        for item in items {
            encoded_files(item, None, out);
        }
    } else if let Some(members) = value.get("object").and_then(Value::as_object) {
        for item in members.values() {
            encoded_files(item, None, out);
        }
    } else if let Some(items) = value.get("collection").and_then(Value::as_array) {
        for pair in items {
            if let Some([key, item]) = pair.as_array().map(Vec::as_slice) {
                encoded_files(item, key.as_str(), out);
            }
        }
    }
}

/// Where a succeeded step's files were placed (module doc): the instance id `id` names its takes.
fn placed_files(id: &str, step_path: &str, outputs: &Map<String, Value>) -> Vec<Placed> {
    let step = step_folder(step_path, &takes_of_id(id));
    outputs
        .iter()
        .flat_map(|(port, value)| placed_port_files(&step, port, value))
        .collect()
}

fn placed_port_files(step: &str, port: &str, value: &Value) -> Vec<Placed> {
    let mut files = Vec::new();
    encoded_files(value, None, &mut files);
    let one = files.len() == 1;
    files
        .into_iter()
        .enumerate()
        .map(|(index, file)| {
            let label = (!one).then(|| {
                file.key
                    .map_or_else(|| index.to_string(), |key| keyed_path(&key))
            });
            Placed {
                path: step_file_path(step, port, label.as_deref(), &file.kind),
                digest: file.digest,
                size: file.size,
            }
        })
        .collect()
}

/// What `--verify` found.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Verification {
    files: usize,
    problems: Vec<String>,
}

impl Verification {
    fn document(&self) -> Value {
        json!({
            "verified": self.problems.is_empty(),
            "problems": self.problems,
        })
    }
}

/// Re-checks every placed file of every succeeded step against its recorded digest.
fn verify(view: &RunView, folder: &Path) -> Verification {
    let mut verification = Verification {
        files: 0,
        problems: Vec::new(),
    };
    for step in &view.steps {
        for file in &step.files {
            verification.files += 1;
            let at = file
                .path
                .split('/')
                .fold(folder.to_path_buf(), |path, part| path.join(part));
            let bytes = if at.is_file() {
                std::fs::read(&at).ok()
            } else {
                None
            };
            match bytes {
                None => verification
                    .problems
                    .push(format!("{}: {} is missing", step.id, file.path)),
                Some(bytes) if grida_fx_core::value::file_digest(&bytes) != file.digest => {
                    verification.problems.push(format!(
                        "{}: {} differs from its recorded digest",
                        step.id, file.path
                    ));
                }
                Some(_) => {}
            }
        }
    }
    verification
}

/// The text form (module doc).
fn text_lines(
    view: &RunView,
    folder_name: &str,
    verification: Option<&Verification>,
    events: &[Value],
) -> Vec<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for step in &view.steps {
        *counts.entry(step.state).or_default() += 1;
    }
    let counted = if counts.is_empty() {
        "none".to_string()
    } else {
        counts
            .iter()
            .map(|(state, count)| format!("{count} {state}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let stand_in = if view.stand_in {
        "  \u{b7}  stand-in"
    } else {
        ""
    };
    let folder_name = view.name.as_deref().unwrap_or(folder_name);
    let mut lines = vec![
        format!("{}  \u{b7}  {folder_name}{stand_in}", view.workflow),
        labelled(
            "state",
            &format!("{}   {} steps: {counted}", view.state, view.steps.len()),
        ),
    ];
    if let Some(spent) = view.charged.as_ref().and_then(|c| Usd::from_value(c).ok()) {
        lines.push(labelled("booked", &spent.dollars_2()));
    }
    for step in view.steps.iter().filter(|s| s.state == "failed") {
        let error = step.error.as_deref().unwrap_or("no reason recorded");
        lines.push(labelled("failed", &format!("{}: {error}", step.id)));
    }
    for event in &view.call_attempt_failures {
        append_call_attempt(&mut lines, event);
    }
    append_call_recoveries(&mut lines, events);
    if let Some(verification) = verification {
        if verification.problems.is_empty() {
            lines.push(labelled(
                "verified",
                &format!("{} files", verification.files),
            ));
        } else {
            for problem in &verification.problems {
                lines.push(labelled("differs", problem));
            }
        }
    }
    lines
}

/// Show the latest recorded recovery result and next action for each call.
pub(super) fn append_call_recoveries(lines: &mut Vec<String>, events: &[Value]) {
    let mut latest = IndexMap::new();
    for event in events {
        let Some(call) = text(event, "call") else {
            continue;
        };
        match event_name(event) {
            Some("call_recovery") => {
                latest.insert(call, event);
            }
            Some("call")
                if latest
                    .get(call)
                    .is_some_and(|event| text(event, "status") != Some("recovered")) =>
            {
                // A later answer for this key resolves the call, including after a replacement.
                latest.shift_remove(call);
            }
            _ => {}
        }
    }
    for (call, event) in latest {
        let Some(id) = text(event, "id") else {
            continue;
        };
        let status = text(event, "status");
        let description = match status {
            Some("available") => "completed provider task retained for collection",
            Some("legacy") => "provider task retained for a recovery check",
            Some("unavailable") => "original provider result unavailable",
            Some("recovered") => "collected original provider result · $0.00 additional",
            _ => continue,
        };
        let charge = event
            .get("original_charged_usd")
            .and_then(|value| Usd::from_value(value).ok())
            .map_or_else(
                || "original booking unknown".to_string(),
                |amount| format!("{} originally booked by FX", amount.dollars_2()),
            );
        lines.push(labelled(
            "recovery",
            &format!("{id} · {description} · {charge}"),
        ));
        match status {
            Some("available" | "legacy") => lines.push(labelled(
                "next",
                "resume with --live --recover to collect retained results",
            )),
            Some("unavailable") if grida_fx_core::value::is_digest(call) => {
                let command = crate::invoker::command();
                lines.push(labelled(
                    "next",
                    &format!("review the provider task, then use {command} jobs --forget {call} if you intend to authorize a new generation"),
                ));
            }
            _ => {}
        }
    }
}

/// Describe one recorded failure independently of the step's final state.
fn append_call_attempt(lines: &mut Vec<String>, event: &Value) {
    let (Some(id), Some(route), Some(attempt)) = (
        text(event, "id"),
        text(event, "route"),
        event.get("attempt").and_then(Value::as_u64),
    ) else {
        return;
    };
    let mut heading = format!("{id} · attempt {attempt}");
    if let Some(send) = event.get("send").and_then(Value::as_u64) {
        heading.push_str(&format!(" · send {send}"));
    }
    heading.push_str(&format!(" · {route}"));
    if let Some(call) = text(event, "call") {
        heading.push_str(&format!(" · call {call}"));
    }
    lines.push(labelled("attempt", &heading));

    if let Some(reason) = text(event, "reason") {
        lines.push(labelled("", reason));
    }
    if let Some(diagnostic) = event.get("diagnostic") {
        append_attempt_diagnostic(lines, diagnostic, text(event, "reason"));
    }
    let disposition = text(event, "outcome");
    let mut outcome = match disposition {
        Some("not_received") => vec!["request not received".into()],
        Some("refused") => vec!["call refused".into()],
        Some("failed") => vec!["call failed".into()],
        Some("unsettled") => vec!["submission outcome unknown".into()],
        Some("unreachable") => vec!["job outcome unavailable".into()],
        _ => Vec::new(),
    };
    if let Some(retryable) = event.get("retryable").and_then(Value::as_bool) {
        outcome.push(
            if retryable {
                "retry eligible"
            } else {
                "no retry"
            }
            .into(),
        );
    }
    if let Some(charge) = event
        .get("charged_usd")
        .and_then(|v| Usd::from_value(v).ok())
    {
        outcome.push(format!("{} booked by FX", charge.dollars_2()));
    }
    if disposition == Some("unsettled") {
        outcome.push("actual provider charge unknown".into());
    }
    if !outcome.is_empty() {
        lines.push(labelled("", &outcome.join(" · ")));
    }
    if disposition == Some("unsettled") {
        let command = crate::invoker::command();
        lines.push(labelled(
            "",
            &format!(
                "reconcile the original submission using {command} jobs and the provider's records"
            ),
        ));
    }
}

fn append_attempt_diagnostic(lines: &mut Vec<String>, diagnostic: &Value, reason: Option<&str>) {
    let mut facts = Vec::new();
    if let (Some(source), Some(phase)) = (text(diagnostic, "source"), text(diagnostic, "phase")) {
        facts.push(format!("{source}/{phase}"));
    }
    if let Some(status) = diagnostic.get("http_status").and_then(Value::as_u64) {
        facts.push(format!("HTTP {status}"));
    }
    for (key, label) in [
        ("code", "code"),
        ("field", "field"),
        ("request_id", "request"),
        ("task_id", "task"),
        ("thread_id", "thread"),
        ("turn_id", "turn"),
    ] {
        if let Some(value) = text(diagnostic, key) {
            facts.push(format!("{label} {value}"));
        }
    }
    if let Some(ephemeral) = diagnostic.get("ephemeral").and_then(Value::as_bool) {
        facts.push(
            if ephemeral {
                "ephemeral session"
            } else {
                "persistent session"
            }
            .into(),
        );
    }
    match text(diagnostic, "detail") {
        Some("unavailable") => facts.push("explanation unavailable".into()),
        Some("unreadable") => facts.push("explanation unreadable".into()),
        Some("withheld") => facts.push("explanation withheld".into()),
        _ => {}
    }
    if !facts.is_empty() {
        lines.push(labelled("", &facts.join(" · ")));
    }
    if let Some(message) =
        text(diagnostic, "message").filter(|m| reason.is_none_or(|r| !r.contains(*m)))
    {
        lines.push(labelled("", message));
    }
}

/// The `--json` document (module doc).
fn json_document(view: &RunView, folder: &str, verification: Option<&Verification>) -> Value {
    let steps: Vec<Value> = view
        .steps
        .iter()
        .map(|step| {
            let mut entry = Map::new();
            entry.insert("id".into(), json!(step.id));
            entry.insert("path".into(), json!(step.path));
            entry.insert("state".into(), json!(step.state));
            if let Some(error) = &step.error {
                entry.insert("error".into(), json!(error));
            }
            if let Some(diagnostic) = &step.diagnostic {
                entry.insert("diagnostic".into(), diagnostic.clone());
            }
            if let Some(cache) = &step.cache {
                entry.insert("cache".into(), json!(cache));
            }
            if let Some(route) = &step.waiting_for {
                entry.insert("waiting_for".into(), json!(route));
            }
            let files: Vec<Value> = step
                .files
                .iter()
                .map(|f| json!({"path": f.path, "digest": f.digest, "size": f.size}))
                .collect();
            entry.insert("files".into(), Value::Array(files));
            Value::Object(entry)
        })
        .collect();
    let mut document = json!({
        "run": {
            "workflow": view.workflow,
            "folder": folder,
            "state": view.state,
            "charged_usd": view.charged.clone().unwrap_or(Value::Null),
            "steps": steps,
        }
    });
    if let Some(name) = &view.name {
        document["run"]["name"] = json!(name);
    }
    if let Some(created_at) = &view.created_at {
        document["run"]["created_at"] = json!(created_at);
    }
    if view.stand_in {
        document["run"]["stand_in"] = Value::Bool(true);
    }
    if !view.call_attempt_failures.is_empty() {
        document["run"]["call_attempt_failures"] = json!(view.call_attempt_failures);
    }
    if !view.call_recoveries.is_empty() {
        document["run"]["call_recoveries"] = json!(view.call_recoveries);
    }
    if let Some(verification) = verification {
        document["verification"] = verification.document();
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str, fields: Value) -> Value {
        let mut object = json!({"kind": "fx-run-events-v1", "event": name});
        for (k, v) in fields.as_object().unwrap() {
            object[k] = v.clone();
        }
        object
    }

    fn plan() -> Value {
        json!({
            "kind": "fx-graph-v1",
            "workflow": {"id": "fails"},
            "instances": [
                {"id": "ok#1", "path": "ok", "state": "planned"},
                {"id": "nope#1", "path": "nope", "state": "planned"},
                {"id": "after#1", "path": "after", "state": "planned"},
                {"id": "gone#1", "path": "gone", "state": "absent"},
                {"id": "split#1", "path": "split", "state": "done"},
                {"id": "never#1", "path": "never", "state": "blocked",
                 "reason": "something it reads failed"},
                {"id": "slow#1", "path": "slow", "state": "planned"},
            ],
        })
    }

    fn finished_run() -> Vec<Value> {
        vec![
            event("run_started", json!({})),
            event("node_started", json!({"id": "ok#1", "path": "ok"})),
            event(
                "node_finished",
                json!({"id": "ok#1", "path": "ok", "cache": "miss",
                "outputs": {"text": {"file": {"digest": "1".repeat(64), "kind": "text/plain",
                                               "name": "ok/text", "size": 2}}}}),
            ),
            event("node_started", json!({"id": "nope#1", "path": "nope"})),
            event(
                "node_failed",
                json!({"id": "nope#1", "path": "nope",
                                        "error": "refused on purpose"}),
            ),
            event(
                "node_skipped",
                json!({"id": "after#1", "path": "after",
                                         "reason": "something it reads failed", "blocked": true}),
            ),
            event("node_started", json!({"id": "slow#1", "path": "slow"})),
            event(
                "node_started",
                json!({"id": "late['k']#1", "path": "late['k']"}),
            ),
            event(
                "node_failed",
                json!({"id": "late['k']#1", "path": "late['k']", "error": null}),
            ),
            event("run_finished", json!({"ok": false, "charged_usd": 0.125})),
        ]
    }

    #[test]
    fn inspect_keeps_call_failures_independent_of_the_final_step_state() {
        let failed = event(
            "call_attempt_failed",
            json!({"id": "ok#1", "capability": "image.generate", "route": "imagegen@codex",
              "call": "1".repeat(64), "attempt": 1, "send": 1, "outcome": "failed",
              "reason": "Codex completed the turn without an image", "charged_usd": 0,
              "retryable": false, "diagnostic": {"source": "adapter", "phase": "send",
                "thread_id": "thread-a", "turn_id": "turn-a", "ephemeral": true,
                "detail": "unavailable"}}),
        );
        let mut another_failure = failed.clone();
        another_failure["call"] = json!("2".repeat(64));
        let succeeded = event("node_finished", json!({"id": "ok#1", "cache": "miss"}));
        let events = [failed.clone(), another_failure.clone(), succeeded];
        let view = RunView::new(&plan(), &events);
        let lines = text_lines(&view, "runs/one", None, &events).join("\n");
        for call in ["1".repeat(64), "2".repeat(64)] {
            assert!(lines.contains(&format!(
                "attempt   ok#1 · attempt 1 · send 1 · imagegen@codex · call {call}"
            )));
        }
        assert!(lines.contains("thread thread-a · turn turn-a · ephemeral session"));
        assert!(lines.contains("explanation unavailable"));
        assert!(lines.contains("no retry · $0.00 booked by FX"));
        let summary = json_document(&view, "runs/one", None);
        assert_eq!(summary["run"]["steps"][0]["state"], "succeeded");
        assert_eq!(
            summary["run"]["call_attempt_failures"],
            json!([failed, another_failure])
        );
    }

    #[test]
    fn inspect_stops_advising_recovery_when_the_call_has_an_answer() {
        let call = "1".repeat(64);
        let unavailable = event(
            "call_recovery",
            json!({"id": "ok#1", "call": call, "status": "unavailable", "original_charged_usd": 0.5}),
        );
        let answered = event("call", json!({"call": call, "cost_usd": 0.5}));
        let mut recovered = unavailable.clone();
        recovered["status"] = json!("recovered");
        let free = event("call", json!({"call": call, "cost_usd": 0}));

        let events = [unavailable.clone()];
        let view = RunView::new(&plan(), &events);
        let lines = text_lines(&view, "runs/one", None, &events).join("\n");
        assert!(lines.contains("original provider result unavailable"));
        assert!(lines.contains(&format!("jobs --forget {call}")));

        let events = [unavailable.clone(), answered];
        let view = RunView::new(&plan(), &events);
        let lines = text_lines(&view, "runs/one", None, &events).join("\n");
        assert!(!lines.contains("recovery"));
        assert_eq!(
            json_document(&view, "runs/one", None)["run"]["call_recoveries"],
            json!([unavailable])
        );

        let events = [recovered, free];
        let view = RunView::new(&plan(), &events);
        let lines = text_lines(&view, "runs/one", None, &events).join("\n");
        assert!(lines.contains("collected original provider result · $0.00 additional"));
    }

    #[test]
    fn the_last_event_decides_each_steps_state() {
        let view = RunView::new(&plan(), &finished_run());
        assert_eq!(view.workflow, "fails");
        assert_eq!(view.state, "failed");
        let states: Vec<(&str, &str, Option<&str>)> = view
            .steps
            .iter()
            .map(|s| (s.id.as_str(), s.state, s.error.as_deref()))
            .collect();
        assert_eq!(
            states,
            [
                ("ok#1", "succeeded", None),
                ("nope#1", "failed", Some("refused on purpose")),
                ("after#1", "skipped", Some("something it reads failed")),
                ("split#1", "succeeded", None),
                ("never#1", "skipped", Some("something it reads failed")),
                (
                    "slow#1",
                    "failed",
                    Some("the run ended before this step finished")
                ),
                ("late['k']#1", "failed", None),
            ]
        );
        assert_eq!(view.steps[0].cache.as_deref(), Some("miss"));
    }

    #[test]
    fn a_run_that_has_not_ended_shows_running_and_pending_steps() {
        let events = vec![
            event("run_started", json!({})),
            event("node_started", json!({"id": "ok#1", "path": "ok"})),
        ];
        let view = RunView::new(&plan(), &events);
        assert_eq!(view.state, "unfinished");
        assert_eq!(view.charged, None);
        let states: Vec<&str> = view.steps.iter().map(|s| s.state).collect();
        assert_eq!(
            states,
            [
                "running",
                "pending",
                "pending",
                "succeeded",
                "pending",
                "pending"
            ]
        );
        let planned = RunView::new(&plan(), &[]);
        assert_eq!(planned.state, "planned");
        let mut cancelled = events.clone();
        cancelled.push(event(
            "run_cancelled",
            json!({"reason": "interrupted", "charged_usd": 0}),
        ));
        let view = RunView::new(&plan(), &cancelled);
        assert_eq!(view.state, "cancelled");
        assert_eq!(view.steps[0].state, "failed");
        assert_eq!(view.charged, Some(json!(0)));
    }

    #[test]
    fn a_step_waiting_for_its_route_is_counted_as_waiting() {
        let call = |name: &str, id: &str, key: char, route: &str| {
            event(
                name,
                json!({"id": id, "capability": "image.generate", "route": route,
                       "call": key.to_string().repeat(64)}),
            )
        };
        let events = vec![
            event("run_started", json!({})),
            event("node_started", json!({"id": "ok#1", "path": "ok"})),
            event("node_started", json!({"id": "nope#1", "path": "nope"})),
            event("node_started", json!({"id": "after#1", "path": "after"})),
            // Two calls of one body wait at once; one admitted, the step still waits.
            call("call_waiting", "ok#1", 'a', "img-a@acme"),
            call("call_waiting", "ok#1", 'b', "img-b@acme"),
            call("call_admitted", "ok#1", 'a', "img-a@acme"),
            // An admission of no open wait closes nothing; a new attempt closes them all.
            call("call_waiting", "nope#1", 'c', "img-a@acme"),
            call("call_admitted", "nope#1", 'd', "img-a@acme"),
            event(
                "node_retry",
                json!({"id": "nope#1", "attempt": 2, "error": "e"}),
            ),
            call("call_admitted", "nope#1", 'c', "img-a@acme"),
            // Waiting again after its admission, as a step with several calls does.
            call("call_waiting", "after#1", 'e', "img-a@acme"),
            call("call_admitted", "after#1", 'e', "img-a@acme"),
            call("call_waiting", "after#1", 'f', "img-a@acme"),
            // A step that has not started, or has ended, waits for nothing.
            call("call_waiting", "slow#1", 'g', "img-a@acme"),
        ];
        let states = |events: &[Value]| -> Vec<(String, &'static str, Option<String>)> {
            RunView::new(&plan(), events).steps[..3]
                .iter()
                .map(|s| (s.id.clone(), s.state, s.waiting_for.clone()))
                .collect()
        };
        let owned = |id: &str, state: &'static str, route: Option<&str>| {
            (id.to_string(), state, route.map(str::to_string))
        };
        assert_eq!(
            states(&events),
            [
                owned("ok#1", "waiting", Some("img-b@acme")),
                owned("nope#1", "running", None),
                owned("after#1", "waiting", Some("img-a@acme")),
            ]
        );
        let view = RunView::new(&plan(), &events);
        assert_eq!(view.steps[5].state, "pending");
        assert_eq!(
            text_lines(&view, "w1", None, &events)[1],
            "state     unfinished   6 steps: 2 pending, 1 running, 1 succeeded, 2 waiting"
        );
        let steps = json_document(&view, "w1", None)["run"]["steps"].clone();
        assert_eq!(steps[0]["state"], "waiting");
        assert_eq!(steps[0]["waiting_for"], "img-b@acme");
        assert!(steps[1].get("waiting_for").is_none());
        // The other call's admission ends the step's wait.
        let mut admitted = events.clone();
        admitted.push(call("call_admitted", "ok#1", 'b', "img-b@acme"));
        assert_eq!(states(&admitted)[0], owned("ok#1", "running", None));
        // A new invocation leaves the waits of the last one behind.
        let mut resumed = events.clone();
        resumed.push(event("run_started", json!({})));
        assert!(
            states(&resumed)
                .iter()
                .all(|(_, state, _)| *state == "running")
        );
        // A new start, and the run's end, leave no wait behind.
        let mut restarted = events.clone();
        restarted.push(event("node_started", json!({"id": "ok#1", "path": "ok"})));
        assert_eq!(RunView::new(&plan(), &restarted).steps[0].state, "running");
        restarted.push(event(
            "run_cancelled",
            json!({"reason": "interrupted", "charged_usd": 0}),
        ));
        let ended = RunView::new(&plan(), &restarted);
        assert!(ended.steps.iter().all(|s| s.state != "waiting"));
        assert_eq!(ended.steps[2].state, "failed");
    }

    #[test]
    fn the_text_summary() {
        let events = finished_run();
        let view = RunView::new(&plan(), &events);
        let lines = text_lines(&view, "f1", None, &events);
        assert_eq!(
            lines,
            [
                "fails  \u{b7}  f1",
                "state     failed   7 steps: 3 failed, 2 skipped, 2 succeeded",
                "booked    $0.12",
                "failed    nope#1: refused on purpose",
                "failed    slow#1: the run ended before this step finished",
                "failed    late['k']#1: no reason recorded",
            ]
        );
        let empty = RunView::new(&json!({"workflow": {"id": "case"}}), &[]);
        assert_eq!(
            text_lines(
                &empty,
                "p4",
                Some(&Verification {
                    files: 0,
                    problems: vec![]
                }),
                &[]
            ),
            [
                "case  \u{b7}  p4",
                "state     planned   0 steps: none",
                "verified  0 files",
            ]
        );
        let problems = Verification {
            files: 2,
            problems: vec!["ok#1: files/ok/text.txt is missing".into()],
        };
        assert_eq!(
            text_lines(&empty, "p4", Some(&problems), &[])
                .last()
                .unwrap(),
            "differs   ok#1: files/ok/text.txt is missing"
        );
    }

    #[test]
    fn the_json_document() {
        let view = RunView::new(&plan(), &finished_run());
        let document = json_document(
            &view,
            "runs/fails/f1",
            Some(&Verification {
                files: 1,
                problems: vec![],
            }),
        );
        assert_eq!(document["run"]["workflow"], "fails");
        assert_eq!(document["run"]["folder"], "runs/fails/f1");
        assert_eq!(document["run"]["state"], "failed");
        assert_eq!(document["run"]["charged_usd"], 0.125);
        let steps = document["run"]["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 7);
        assert_eq!(
            steps[1],
            json!({"id": "nope#1", "path": "nope", "state": "failed",
                                    "error": "refused on purpose", "files": []})
        );
        assert_eq!(steps[0]["cache"], "miss");
        assert_eq!(steps[0]["files"].as_array().unwrap().len(), 1);
        assert_eq!(
            document["verification"],
            json!({"verified": true, "problems": []})
        );
        let unverified = json_document(&RunView::new(&plan(), &[]), "x", None);
        assert!(unverified.get("verification").is_none());
        assert_eq!(unverified["run"]["charged_usd"], Value::Null);
    }

    #[test]
    fn encoded_files_carry_their_labels() {
        let file = |d: &str, key: Option<&str>| {
            let mut f = json!({"digest": d, "kind": "text/plain", "name": "x", "size": 1});
            if let Some(key) = key {
                f["key"] = json!(key);
            }
            json!({"file": f})
        };
        let value = json!({"collection": [
            ["first", file("a", None)],
            ["second", file("b", Some("own"))],
            ["third", {"list": [file("c", None)]}],
        ]});
        let mut files = Vec::new();
        encoded_files(&value, None, &mut files);
        let labels: Vec<(String, Option<String>)> =
            files.into_iter().map(|f| (f.digest, f.key)).collect();
        assert_eq!(
            labels,
            [
                ("a".to_string(), Some("first".to_string())),
                ("b".to_string(), Some("own".to_string())),
                ("c".to_string(), None),
            ]
        );
        let mut none = Vec::new();
        encoded_files(&json!({"none": true}), None, &mut none);
        encoded_files(&json!({"value": {"failed": "a#1"}}), None, &mut none);
        assert!(none.is_empty());
    }

    #[test]
    fn files_are_found_where_the_run_placed_them() {
        let outputs = json!({
            "text": {"file": {"digest": "1".repeat(64), "kind": "text/plain", "name": "t",
                              "size": 2}},
            "items": {"collection": [
                ["k0", {"file": {"digest": "2".repeat(64), "kind": "text/plain", "name": "i",
                                 "size": 2, "key": "k0"}}],
                ["a/../b", {"file": {"digest": "3".repeat(64), "kind": "text/plain",
                                     "name": "i", "size": 2, "key": "a/../b"}}],
            ]},
            "seq": {"list": [{"file": {"digest": "4".repeat(64), "kind": "json", "name": "s",
                                       "size": 2}}]},
        });
        let placed = placed_files(
            "entity['ada'].draw#1",
            "entity['ada'].draw",
            outputs.as_object().unwrap(),
        );
        let paths: Vec<&str> = placed.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "files/entity__ada__.draw/text.txt",
                "files/entity__ada__.draw/items/k0.txt",
                "files/entity__ada__.draw/items/a/_/b.txt",
                "files/entity__ada__.draw/seq.json",
            ]
        );
    }

    #[test]
    fn each_take_is_found_in_its_own_folder_and_long_names_as_cut() {
        let one = json!({"text": {"file": {"digest": "1".repeat(64), "kind": "text/plain",
                                           "name": "t", "size": 2}}});
        let outputs = one.as_object().unwrap();
        let path = |id: &str, step: &str| placed_files(id, step, outputs)[0].path.clone();
        assert_eq!(path("draw#1", "draw"), "files/draw/text.txt");
        assert_eq!(path("draw#2", "draw"), "files/draw#2/text.txt");
        assert_eq!(
            path("entity['ada'].draw#1.3", "entity['ada'].draw"),
            "files/entity__ada__.draw#1.3/text.txt"
        );
        // Where the run puts it, a name too long for a file system included.
        let long = "s".repeat(300);
        let found = path(&format!("{long}#2"), &long);
        let placed = grida_fx_runtime::folder::step_files(
            &long,
            &[2],
            &[(
                "text".to_string(),
                grida_fx_core::val::Val::File(Box::new(grida_fx_core::val::FileValue {
                    digest: "1".repeat(64),
                    kind: "text/plain".into(),
                    name: "t".into(),
                    size: 2,
                    key: None,
                    content: None,
                    location: None,
                })),
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(found, placed[0].0);
        assert!(
            found.split('/').all(|segment| segment.len() <= 255),
            "{found}"
        );
    }

    #[test]
    fn verify_finds_missing_and_changed_files() {
        let folder = tempfile::tempdir().unwrap();
        let good = b"ok".to_vec();
        let digest = grida_fx_core::value::file_digest(&good);
        let events = vec![
            event(
                "node_finished",
                json!({"id": "a#1", "path": "a", "outputs": {
                "text": {"file": {"digest": digest, "kind": "text/plain", "name": "a/text",
                                  "size": 2}}}}),
            ),
            event(
                "node_finished",
                json!({"id": "b#1", "path": "b", "outputs": {
                "text": {"file": {"digest": digest, "kind": "text/plain", "name": "b/text",
                                  "size": 2}}}}),
            ),
            event(
                "node_finished",
                json!({"id": "c#1", "path": "c", "outputs": {
                "text": {"file": {"digest": digest, "kind": "text/plain", "name": "c/text",
                                  "size": 2}}}}),
            ),
        ];
        for (step, bytes) in [("a", &b"ok"[..]), ("b", &b"no"[..])] {
            std::fs::create_dir_all(folder.path().join("files").join(step)).unwrap();
            std::fs::write(
                folder.path().join("files").join(step).join("text.txt"),
                bytes,
            )
            .unwrap();
        }
        let view = RunView::new(&json!({"workflow": {"id": "case"}}), &events);
        let verification = verify(&view, folder.path());
        assert_eq!(verification.files, 3);
        assert_eq!(
            verification.problems,
            [
                "b#1: files/b/text.txt differs from its recorded digest",
                "c#1: files/c/text.txt is missing",
            ]
        );
    }

    #[test]
    fn a_workflow_id_finds_its_newest_run() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
        let error = find_folder("case", &root).unwrap_err();
        assert_eq!(error.kind, grida_fx_core::ErrorKind::Usage);
        assert_eq!(
            error.message,
            "no run folder case, and no runs of a workflow case here"
        );
        for (name, age) in [
            ("2026-10-05-1", 30),
            ("2026-10-06-1", 10),
            ("2026-10-06-2", 20),
        ] {
            let folder = root.join("runs/case").join(name);
            std::fs::create_dir_all(&folder).unwrap();
            let plan = folder.join("plan.json");
            std::fs::write(
                &plan,
                json!({"kind":"fx-graph-v1","workflow":{"id":"case","file":"workflow.yaml"}})
                    .to_string(),
            )
            .unwrap();
            let when = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
            std::fs::File::options()
                .write(true)
                .open(&plan)
                .unwrap()
                .set_modified(when)
                .unwrap();
        }
        std::fs::create_dir_all(root.join("runs/case/empty")).unwrap();
        let (folder, label) = find_folder("case", &root).unwrap();
        assert_eq!(folder, root.join("runs/case/2026-10-06-1"));
        assert_eq!(label, "runs/case/2026-10-06-1");
        // From a folder inside the project, the newest run is named from there.
        let inner = root.join("elsewhere");
        std::fs::create_dir_all(&inner).unwrap();
        let (_, label) = find_folder("case", &inner).unwrap();
        assert_eq!(label, "../runs/case/2026-10-06-1");
        // A folder of that name here is taken as the run folder.
        let (folder, label) = find_folder("case", &root.join("runs")).unwrap();
        assert_eq!(folder, root.join("runs/case"));
        assert_eq!(label, "case");
        // A folder, or a path of several parts, is taken as it is.
        let (folder, label) = find_folder("runs/case/2026-10-05-1", &root).unwrap();
        assert_eq!(folder, root.join("runs/case/2026-10-05-1"));
        assert_eq!(label, "runs/case/2026-10-05-1");
        let (folder, _) = find_folder("a/b", &root).unwrap();
        assert_eq!(folder, root.join("a/b"));
        let (folder, _) = find_folder("runs", &root).unwrap();
        assert_eq!(folder, root.join("runs"));
    }
}
