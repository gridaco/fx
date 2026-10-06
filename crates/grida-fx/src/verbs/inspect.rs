//! `grida-fx inspect <run | workflow id> [--verify] [--json]` (spec/store.md §8).
//!
//! The folder: `<run>` relative to the working directory when it is a folder or has more than one
//! path part; else the newest run of that workflow id under the runs folder of the project found
//! from the working directory (the folder whose `plan.json` was written last; the greater name
//! when two were written at once); none: `no run folder <run>, and no runs of a workflow <run>
//! here` (exit 2). A folder without a readable fx-graph-v1 `plan.json` is `<run> is not a run
//! folder` (exit 2). The log is read as `project` reads it; a folder with no log has run nothing.
//! Nothing is written.
//!
//! The steps: each plan instance that is not absent, in plan order, then each instance only the
//! log names, in the order the log first names it. The last `node_*` event decides a step's state
//! (`node_finished` succeeded, `node_failed` failed, `node_skipped` skipped, `node_started`
//! running, or failed with `the run ended before this step finished` once the run ended); with no
//! event: `succeeded` for a plan state `done` (an `at: plan` step), else `skipped` once the run
//! ended (with the plan's reason), else `pending`. A failed or skipped step's error is the
//! event's `error`, else its `reason`. The run's state: `planned`, `unfinished` after
//! `run_started`, `succeeded`/`failed` after `run_finished` (by its `ok`), `cancelled` after
//! `run_cancelled`, the last one winning; it has ended once succeeded, failed or cancelled.
//! `spent` is the `charged_usd` of the last `run_finished` or `run_cancelled`.
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
//! spent     $<charged, 2 places>                   (when the run finished or was cancelled)
//! failed    <id>: <error or "no reason recorded">  (each failed step)
//! differs   <id>: <relative path> is missing | differs from its recorded digest   (--verify)
//! verified  <n> files                              (--verify, nothing differs)
//! ```
//! `--json`: `{"run": {workflow, folder, state, charged_usd, steps: [{id, path, state, error?,
//! cache?, files: [{path, digest, size}]}], stand_in?}, "verification"?: {verified, problems}}`,
//! where `folder` is the folder as named and `charged_usd` is null before the run ended. A
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
    let cwd = super::planning::working_directory()?;
    let (folder, label) = find_folder(&args.run, &cwd)?;
    let plan = read_plan(&folder, &label)?;
    let events = read_log(&folder, &label, false)?;
    let view = RunView::new(&plan, &events);
    let verification = args.verify.then(|| verify(&view, &folder));
    if args.json {
        print_json(&json_document(&view, &label, verification.as_ref()));
    } else {
        let name = folder
            .components()
            .next_back()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_else(|| label.clone());
        for line in text_lines(&view, &name, verification.as_ref()) {
            print_line(&line);
        }
    }
    Ok(u8::from(
        verification.is_some_and(|v| !v.problems.is_empty()),
    ))
}

/// The run folder `given` names, and how messages name it (module doc).
fn find_folder(given: &str, cwd: &Path) -> Result<(PathBuf, String), Error> {
    let folder = cwd.join(given);
    let parts = Path::new(given)
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .count();
    if folder.is_dir() || parts > 1 {
        return Ok((folder, given.to_string()));
    }
    let project = Project::find(cwd)?;
    newest_run(&project.runs_dir().join(given))
        .map(|found| {
            let label = shown_path(&found, cwd);
            (found, label)
        })
        .ok_or_else(|| {
            Error::usage(format!(
                "no run folder {given}, and no runs of a workflow {given} here"
            ))
        })
}

/// The folder under `runs` whose `plan.json` was written last.
fn newest_run(runs: &Path) -> Option<PathBuf> {
    std::fs::read_dir(runs)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let written = std::fs::metadata(path.join("plan.json"))
                .ok()
                .filter(std::fs::Metadata::is_file)?
                .modified()
                .ok()?;
            path.is_dir().then(|| (written, entry.file_name(), path))
        })
        .max_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)))
        .map(|(_, _, path)| path)
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
    cache: Option<String>,
    files: Vec<Placed>,
}

/// A run as its folder shows it (module doc).
#[derive(Debug, Clone, PartialEq)]
struct RunView {
    workflow: String,
    /// A stand-in run: its `plan.json` or a `run_started` says so.
    stand_in: bool,
    state: &'static str,
    /// The last `charged_usd` of a run that ended.
    charged: Option<Value>,
    steps: Vec<StepView>,
}

/// What the plan and the log say about one instance.
#[derive(Debug, Clone)]
struct Entry {
    path: String,
    plan_state: Option<String>,
    plan_reason: Option<String>,
    last: Option<Value>,
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
                },
            );
        }
        let mut state = "planned";
        let mut charged = None;
        let mut stand_in = holds_stand_in(plan);
        for event in events {
            match event_name(event) {
                Some("run_started") => {
                    state = "unfinished";
                    stand_in |= event.get("stand_in") == Some(&Value::Bool(true));
                }
                Some("run_finished") => {
                    let ok = event.get("ok").and_then(Value::as_bool) == Some(true);
                    state = if ok { "succeeded" } else { "failed" };
                    charged = event.get("charged_usd").cloned();
                }
                Some("run_cancelled") => {
                    state = "cancelled";
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
                    });
                    if name == "node_started" {
                        entry.path = path.to_string();
                    }
                    entry.last = Some(event.clone());
                }
                _ => {}
            }
        }
        let ended = matches!(state, "succeeded" | "failed" | "cancelled");
        let steps = entries
            .into_iter()
            .map(|(id, entry)| step_view(id, entry, ended))
            .collect();
        RunView {
            workflow: workflow_id(plan).to_string(),
            stand_in,
            state,
            charged,
            steps,
        }
    }
}

/// One entry's state (module doc).
fn step_view(id: String, entry: Entry, ended: bool) -> StepView {
    let mut view = StepView {
        id,
        path: entry.path,
        state: "pending",
        error: None,
        cache: None,
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
            view.error = error_of(event);
        }
        Some((Some("node_skipped"), event)) => {
            view.state = "skipped";
            view.error = error_of(event);
        }
        Some((Some("node_started"), _)) if ended => {
            view.state = "failed";
            view.error = Some("the run ended before this step finished".into());
        }
        Some(_) => view.state = "running",
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
    let mut placed = Vec::new();
    for (port, value) in outputs {
        let mut files = Vec::new();
        encoded_files(value, None, &mut files);
        let one = files.len() == 1;
        for (index, file) in files.into_iter().enumerate() {
            let path = if one {
                step_file_path(&step, port, None, &file.kind)
            } else {
                let label = file
                    .key
                    .map_or_else(|| index.to_string(), |key| keyed_path(&key));
                step_file_path(&step, port, Some(&label), &file.kind)
            };
            placed.push(Placed {
                path,
                digest: file.digest,
                size: file.size,
            });
        }
    }
    placed
}

/// What `--verify` found.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Verification {
    files: usize,
    problems: Vec<String>,
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
    let mut lines = vec![
        format!("{}  \u{b7}  {folder_name}{stand_in}", view.workflow),
        labelled(
            "state",
            &format!("{}   {} steps: {counted}", view.state, view.steps.len()),
        ),
    ];
    if let Some(spent) = view.charged.as_ref().and_then(|c| Usd::from_value(c).ok()) {
        lines.push(labelled("spent", &spent.dollars_2()));
    }
    for step in view.steps.iter().filter(|s| s.state == "failed") {
        let error = step.error.as_deref().unwrap_or("no reason recorded");
        lines.push(labelled("failed", &format!("{}: {error}", step.id)));
    }
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
            if let Some(cache) = &step.cache {
                entry.insert("cache".into(), json!(cache));
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
    if view.stand_in {
        document["run"]["stand_in"] = Value::Bool(true);
    }
    if let Some(verification) = verification {
        document["verification"] = json!({
            "verified": verification.problems.is_empty(),
            "problems": verification.problems,
        });
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
    fn the_text_summary() {
        let view = RunView::new(&plan(), &finished_run());
        let lines = text_lines(&view, "f1", None);
        assert_eq!(
            lines,
            [
                "fails  \u{b7}  f1",
                "state     failed   7 steps: 3 failed, 2 skipped, 2 succeeded",
                "spent     $0.12",
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
                })
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
            text_lines(&empty, "p4", Some(&problems)).last().unwrap(),
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
            std::fs::write(&plan, "{}").unwrap();
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
