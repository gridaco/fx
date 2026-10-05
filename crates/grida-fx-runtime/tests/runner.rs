//! Whole runs: a temporary project is planned and run through `runner::run` with the real
//! `grida.fx` node host (`GRIDA_FX_PYTHON`, else the repository's `python/.venv`; each test skips
//! when neither exists), then the outcome, the run folder and its `events.jsonl` are checked.
//!
//! The tests hold one lock while they run: Ctrl-C is process-wide, and one test interrupts its
//! own process.
#![cfg(unix)]

use grida_fx_core::money::Usd;
use grida_fx_core::plan::make_plan;
use grida_fx_core::project::{PlanRequest, make_planner};
use grida_fx_providers::Adapters;
use grida_fx_runtime::engine::Engine;
use grida_fx_runtime::events::read_events_tolerant;
use grida_fx_runtime::folder::RunFolder;
use grida_fx_runtime::host::PythonHost;
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::plantime::PlanTime;
use grida_fx_runtime::runner::{RunError, RunOptions, RunOutcome, run};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// The interpreter for the real host: `GRIDA_FX_PYTHON`, else the repository's `python/.venv`.
fn real_python() -> Option<PathBuf> {
    if let Some(python) = std::env::var_os("GRIDA_FX_PYTHON").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(python));
    }
    let venv = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python/.venv/bin/python");
    venv.exists().then_some(venv)
}

const NODES: &str = r#"# Node types the runner tests use: small, local and deterministic.

import time

from grida.fx import Ctx, node


@node("shout", params={"text": str}, outputs={"text": "text"}, version=1)
def shout(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.params["text"].upper())}


@node("join", inputs={"parts": "text{}"}, outputs={"text": "text"}, version=1)
def join(ctx: Ctx) -> dict:
    parts = ctx.inputs["parts"].items()
    return {"text": ctx.out.text("|".join(f"{k}={v.read_bytes().decode()}" for k, v in parts))}


@node("refuse", params={"text": str}, outputs={"text": "text"}, version=1)
def refuse(ctx: Ctx) -> dict:
    ctx.fact("seen", ctx.params["text"])
    raise ctx.fail("refused on purpose")


@node("flaky", params={"text": str}, outputs={"text": "text"}, version=1, retry="engine")
def flaky(ctx: Ctx) -> dict:
    raise RuntimeError("kaboom " + ctx.params["text"])


@node("count", params={"n": int}, outputs={"items": "json"}, version=1)
def count(ctx: Ctx) -> dict:
    return {"items": ctx.out.json({"lines": [f"l{i}" for i in range(ctx.params["n"])]})}


@node("slow", params={"text": str}, outputs={"text": "text"}, version=1)
def slow(ctx: Ctx) -> dict:
    for _ in range(600):
        if ctx.cancelled:
            break
        time.sleep(0.05)
    return {"text": ctx.out.text(ctx.params["text"])}


@node(
    "verdict",
    inputs={"subject": "file"},
    params={"accept_take": int},
    outputs={},
    judge=True,
    version=1,
)
def verdict(ctx: Ctx) -> dict:
    accepted = ctx.instance.take >= ctx.params["accept_take"]
    ctx.fact("verdict", "accept" if accepted else "reject")
    return {}


@node("needs_tool", outputs={"text": "text"}, tools=["fx-runner-test-missing-tool>=1"], version=1)
def needs_tool(ctx: Ctx) -> dict:
    return {"text": ctx.out.text("never")}
"#;

const ROUTES: &str = "fx: routes/v1\nroutes:\n  - { capability: image.generate, route: img-a@acme, \
     price: { low_usd: 0.01, high_usd: 0.04 }, features: [alpha] }\n";

/// A temporary project with the test nodes, its workflows and the route table.
struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Project {
    fn new(workflows: &[(&str, &str)]) -> Project {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("acme");
        std::fs::create_dir_all(root.join("nodes")).unwrap();
        std::fs::create_dir_all(root.join("workflows")).unwrap();
        std::fs::write(
            root.join("fx.yaml"),
            "fx: project/v1\nroutes:\n  image.generate: img-a@acme\n",
        )
        .unwrap();
        std::fs::write(root.join("routes.yaml"), ROUTES).unwrap();
        std::fs::write(root.join("nodes/cases.py"), NODES).unwrap();
        std::fs::write(root.join("inputs.yaml"), "names: [ada, bo]\n").unwrap();
        for (id, text) in workflows {
            std::fs::write(root.join(format!("workflows/{id}.yaml")), text).unwrap();
        }
        Project { _dir: dir, root }
    }

    /// A copy of a conformance case's `in/` folder.
    fn conformance(case: &str) -> Project {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("acme");
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance")
            .join(case)
            .join("in");
        copy_tree(&source, &root);
        Project { _dir: dir, root }
    }

    fn events(&self, folder: &str) -> Vec<Value> {
        read_events_tolerant(&self.root.join(folder).join("events.jsonl")).unwrap()
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// How one test plans and runs.
struct Invocation<'a> {
    target: &'a str,
    inputs: &'a [&'a str],
    folder: &'a str,
    yes_up_to: Option<&'a str>,
    live: bool,
}

impl Default for Invocation<'_> {
    fn default() -> Self {
        Invocation {
            target: "case",
            inputs: &[],
            folder: "runs/one",
            yes_up_to: None,
            live: false,
        }
    }
}

/// Plans `invocation` in `project` and runs it; `None` when there is no Python to host nodes.
/// `during` runs on another thread while the run goes on.
fn run_in(
    project: &Project,
    invocation: Invocation<'_>,
    during: Option<Box<dyn FnOnce() + Send>>,
) -> Option<Result<RunOutcome, RunError>> {
    let Some(python) = real_python() else {
        eprintln!("skipped: neither GRIDA_FX_PYTHON nor python/.venv is set up");
        return None;
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let request = PlanRequest {
        target: invocation.target.to_string(),
        cwd: project.root.clone(),
        input_files: invocation.inputs.iter().map(|s| s.to_string()).collect(),
        rest: Vec::new(),
        arguments: Default::default(),
        routes: vec!["routes.yaml".to_string()],
        max_usd: None,
    };
    let mut host = PythonHost::new().with_python(python.clone());
    let mut planner = make_planner(&request, &mut host).unwrap();
    let engine = Arc::new(Engine::new(
        runtime.handle().clone(),
        HostSpec {
            python: python.clone(),
            label: python.display().to_string(),
            project_root: planner.home.root.clone(),
            sources: planner.home.document.sources.clone(),
        },
        &planner.project.cache_dir(),
        2,
        Adapters::new(),
        invocation.live,
    ));
    let mut plan_time = PlanTime::new(Arc::clone(&engine));
    let plan = make_plan(
        &mut planner,
        &mut host,
        Some(&mut plan_time),
        engine.store.as_ref(),
    )
    .unwrap();
    let options = RunOptions {
        folder: project.root.join(invocation.folder),
        label: invocation.folder.to_string(),
        yes_up_to: invocation.yes_up_to.map(|text| Usd::parse(text).unwrap()),
        takes_file: format!("workflows/{}.takes.yaml", invocation.target),
    };
    let helper = during.map(std::thread::spawn);
    let outcome = run(engine, &mut planner, &mut host, plan, options);
    if let Some(helper) = helper {
        helper.join().unwrap();
    }
    let _ = host.shutdown();
    runtime.shutdown_timeout(Duration::from_secs(10));
    Some(outcome)
}

fn names(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["event"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn named<'a>(events: &'a [Value], name: &str) -> Vec<&'a Value> {
    events.iter().filter(|e| e["event"] == name).collect()
}

fn invocations(events: &[Value]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for event in events {
        let id = event["invocation_id"].as_str().unwrap().to_string();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

const CASE: &str = "fx: workflow/v1
id: case
title: A local run
inputs:
  names: { type: list, items: { type: string } }
steps:
  loud:
    for_each: ${{ inputs.names }}
    key: ${{ item }}
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ item }}\" }
  joined:
    uses: ./nodes/cases.py#join
    with: { parts: \"${{ steps.loud.*.outputs.text }}\" }
outputs:
  all: ${{ steps.joined.outputs.text }}
  each: ${{ steps.loud.*.outputs.text }}
";

#[test]
fn a_local_run_records_and_places_everything() {
    let _serial = serial();
    let project = Project::new(&[("case", CASE)]);
    let Some(outcome) = run_in(
        &project,
        Invocation {
            inputs: &["inputs.yaml"],
            ..Invocation::default()
        },
        None,
    ) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(outcome.ok, "{outcome:?}");
    assert!(!outcome.incomplete && !outcome.cancelled);
    assert_eq!(outcome.stopped, None);
    assert_eq!(outcome.failed, []);
    assert_eq!(outcome.charged, Usd::ZERO);
    assert_eq!(outcome.folder, project.root.join("runs/one"));
    let output_names: Vec<&String> = outcome.outputs.keys().collect();
    assert_eq!(output_names, ["all", "each"]);

    let folder = project.root.join("runs/one");
    assert!(folder.join("plan.json").is_file());
    assert!(folder.join("run.lock").is_file());
    assert_eq!(
        std::fs::read_to_string(folder.join("outputs/all.txt")).unwrap(),
        "ada=ADA|bo=BO"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("outputs/each/ada.txt")).unwrap(),
        "ADA"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("files/loud__ada/text.txt")).unwrap(),
        "ADA"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("files/joined/text.txt")).unwrap(),
        "ada=ADA|bo=BO"
    );
    let plan: Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join("plan.json")).unwrap()).unwrap();
    assert_eq!(plan["takes_file"], json!("workflows/case.takes.yaml"));
    let text = std::fs::read_to_string(folder.join("plan.json")).unwrap();
    assert!(
        !text.contains(project.root.to_str().unwrap()),
        "no absolute path in plan.json"
    );

    let events = project.events("runs/one");
    let names = names(&events);
    assert_eq!(names.first().map(String::as_str), Some("run_started"));
    assert_eq!(names.last().map(String::as_str), Some("run_finished"));
    assert_eq!(names.iter().filter(|n| *n == "node_started").count(), 3);
    assert_eq!(names.iter().filter(|n| *n == "node_finished").count(), 3);
    let phases = named(&events, "phase_planned");
    assert_eq!(phases.len(), 1);
    assert_eq!(
        (
            &phases[0]["phase"],
            &phases[0]["steps"],
            &phases[0]["high_usd"]
        ),
        (&json!(1), &json!(3), &json!(0))
    );
    assert!(events.iter().all(|e| e["kind"] == "fx-run-events-v1"
        && e["invocation_id"] == events[0]["invocation_id"]
        && e["plan"] == events[0]["plan"]));
    let started = &events[0];
    assert_eq!(started["resumed"], json!(false));
    assert_eq!(started["workflow"], json!("case"));
    assert_eq!(started["estimate"], json!({"low_usd": 0, "high_usd": 0}));
    assert_eq!(started["plan"], plan["plan"]);
    // joined starts after both loud instances finished; it reads them.
    let joined_started = events
        .iter()
        .position(|e| e["event"] == "node_started" && e["id"] == "joined#1")
        .unwrap();
    let finished_before = events[..joined_started]
        .iter()
        .filter(|e| e["event"] == "node_finished")
        .count();
    assert_eq!(finished_before, 2);
    assert_eq!(
        events[joined_started]["reads"],
        json!(["loud['ada']#1", "loud['bo']#1"])
    );
    let finished = named(&events, "node_finished");
    assert!(finished.iter().all(|e| e["cache"] == "miss"));
    assert!(finished.iter().all(|e| e["facts"]["cost_usd"].is_null()));
    let end = events.last().unwrap();
    assert_eq!(end["ok"], json!(true));
    assert_eq!(end["failed"], json!([]));
    assert_eq!(end["stopped"], Value::Null);
    assert_eq!(end["outputs"]["all"]["file"]["kind"], json!("text/plain"));
    // Nothing private in the record.
    let record = std::fs::read_to_string(folder.join("events.jsonl")).unwrap();
    assert!(!record.contains(project.root.to_str().unwrap()));
}

#[test]
fn concurrency_one_runs_a_repeat_one_item_at_a_time() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1
id: case
title: One at a time
inputs:
  names: { type: list, items: { type: string } }
steps:
  loud:
    for_each: ${{ inputs.names }}
    key: ${{ item }}
    concurrency: 1
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ item }}\" }
",
    )]);
    let Some(outcome) = run_in(
        &project,
        Invocation {
            inputs: &["inputs.yaml"],
            ..Invocation::default()
        },
        None,
    ) else {
        return;
    };
    assert!(outcome.unwrap().ok);
    let events = project.events("runs/one");
    let nodes: Vec<(String, String)> = events
        .iter()
        .filter(|e| e["event"] == "node_started" || e["event"] == "node_finished")
        .map(|e| {
            (
                e["event"].as_str().unwrap().to_string(),
                e["id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = [
        ("node_started", "loud['ada']#1"),
        ("node_finished", "loud['ada']#1"),
        ("node_started", "loud['bo']#1"),
        ("node_finished", "loud['bo']#1"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(nodes, expected);
}

#[test]
fn a_need_on_a_judged_step_is_met_once_later_takes_are_absent() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1
id: case
title: A need on a judged step
steps:
  draw:
    uses: ./nodes/cases.py#shout
    with: { text: hi }
  check:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: \"${{ steps.draw.outputs.text }}\", accept_take: 1 }
    on_reject: { regenerate: { max: 3, then: fail } }
  after:
    uses: ./nodes/cases.py#shout
    needs: [draw]
    with: { text: done }
",
    )]);
    let Some(outcome) = run_in(&project, Invocation::default(), None) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(outcome.ok, "{outcome:?}");
    let events = project.events("runs/one");
    let finished: Vec<&str> = named(&events, "node_finished")
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(finished, ["draw#1", "check#1", "after#1"]);
    assert!(named(&events, "problem").is_empty());
}

#[test]
fn a_resumed_run_replays_what_finished() {
    let _serial = serial();
    let project = Project::new(&[("case", CASE)]);
    let invocation = || Invocation {
        inputs: &["inputs.yaml"],
        ..Invocation::default()
    };
    let Some(first) = run_in(&project, invocation(), None) else {
        return;
    };
    assert!(first.unwrap().ok);
    let second = run_in(&project, invocation(), None).unwrap().unwrap();
    assert!(second.ok, "{second:?}");
    let events = project.events("runs/one");
    let ids = invocations(&events);
    assert_eq!(ids.len(), 2);
    let later: Vec<&Value> = events
        .iter()
        .filter(|e| e["invocation_id"] == ids[1].as_str())
        .collect();
    let later_names: Vec<&str> = later.iter().map(|e| e["event"].as_str().unwrap()).collect();
    assert_eq!(later_names, ["run_started", "run_finished"]);
    assert_eq!(later[0]["resumed"], json!(true));
    assert_eq!(later[1]["outputs"], run_finished_outputs(&events, &ids[0]));

    // A step whose file left the store runs again; the others still replay.
    let ada = named(&events, "node_finished")
        .into_iter()
        .find(|e| e["id"] == "loud['ada']#1")
        .unwrap()["outputs"]["text"]["file"]["digest"]
        .as_str()
        .unwrap()
        .to_string();
    let stored = project
        .root
        .join(".fx/cache/files")
        .join(&ada[..2])
        .join(&ada);
    std::fs::remove_file(&stored).unwrap();
    let third = run_in(&project, invocation(), None).unwrap().unwrap();
    assert!(third.ok, "{third:?}");
    let events = project.events("runs/one");
    let ids = invocations(&events);
    let started: Vec<&str> = events
        .iter()
        .filter(|e| e["invocation_id"] == ids[2].as_str() && e["event"] == "node_started")
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(started, ["loud['ada']#1"]);
    assert!(stored.is_file());
}

fn run_finished_outputs(events: &[Value], invocation: &str) -> Value {
    events
        .iter()
        .find(|e| e["invocation_id"] == invocation && e["event"] == "run_finished")
        .unwrap()["outputs"]
        .clone()
}

#[test]
fn another_plan_or_a_held_lock_is_refused() {
    let _serial = serial();
    let project = Project::new(&[("case", CASE)]);
    std::fs::write(project.root.join("other.yaml"), "names: [cy]\n").unwrap();
    let Some(first) = run_in(
        &project,
        Invocation {
            inputs: &["inputs.yaml"],
            ..Invocation::default()
        },
        None,
    ) else {
        return;
    };
    assert!(first.unwrap().ok);
    let other = run_in(
        &project,
        Invocation {
            inputs: &["other.yaml"],
            ..Invocation::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(
        other,
        Err(RunError::Refused(
            "runs/one holds a run of another workflow or other inputs; choose a new folder".into()
        ))
    );
    let held = RunFolder::lock(&project.root.join("runs/two"), "runs/two").unwrap();
    let locked = run_in(
        &project,
        Invocation {
            inputs: &["inputs.yaml"],
            folder: "runs/two",
            ..Invocation::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(
        locked,
        Err(RunError::Refused(
            "another invocation is running runs/two".into()
        ))
    );
    drop(held);
    assert!(!project.root.join("runs/two/events.jsonl").exists());
}

#[test]
fn a_missing_tool_refuses_the_run_before_any_folder() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1\nid: case\ntitle: Tools\nsteps:\n  render:\n    uses: \
         ./nodes/cases.py#needs_tool\n",
    )]);
    let Some(outcome) = run_in(&project, Invocation::default(), None) else {
        return;
    };
    assert_eq!(
        outcome,
        Err(RunError::Refused(
            "a program the run needs is not installed:\nfx-runner-test-missing-tool (for \
             render): install it, or set GRIDA_FX_TOOL_FX_RUNNER_TEST_MISSING_TOOL"
                .into()
        ))
    );
    assert!(!project.root.join("runs/one").exists());
}

#[test]
fn a_failure_blocks_what_reads_it_and_the_rest_runs() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1
id: case
title: A failure and its readers
steps:
  bad:
    uses: ./nodes/cases.py#refuse
    with: { text: x }
  reader:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.bad.outputs.text }}\" }
  second:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.reader.outputs.text }}\" }
  fine:
    uses: ./nodes/cases.py#shout
    with: { text: ok }
outputs:
  last: ${{ steps.second.outputs.text }}
  good: ${{ steps.fine.outputs.text }}
",
    )]);
    let Some(outcome) = run_in(&project, Invocation::default(), None) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(!outcome.ok);
    assert!(!outcome.incomplete, "a failed output is not pending");
    let failed: Vec<&str> = outcome.failed.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(failed, ["bad#1", "reader#1", "second#1"]);
    assert_eq!(outcome.failed[0].1.as_deref(), Some("refused on purpose"));
    let events = project.events("runs/one");
    let bad = named(&events, "node_failed")[0];
    assert_eq!(bad["error"], json!("refused on purpose"));
    assert_eq!(bad["facts"]["seen"], json!("x"));
    let skipped = named(&events, "node_skipped");
    assert_eq!(skipped.len(), 2);
    assert!(
        skipped
            .iter()
            .all(|e| e["blocked"] == json!(true)
                && e["reason"] == json!("something it reads failed"))
    );
    let end = events.last().unwrap();
    assert_eq!(end["failed"], json!(["bad#1", "reader#1", "second#1"]));
    assert_eq!(
        end["outputs"]["last"],
        json!({"value": {"failed": "second#1"}})
    );
    assert!(project.root.join("runs/one/outputs/good.txt").is_file());
    assert!(!project.root.join("runs/one/files/bad").exists());
}

#[test]
fn retry_engine_runs_a_raising_body_six_times() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1\nid: case\ntitle: Retries\nsteps:\n  y:\n    uses: \
         ./nodes/cases.py#flaky\n    with: { text: y }\n",
    )]);
    let Some(outcome) = run_in(&project, Invocation::default(), None) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(!outcome.ok);
    assert_eq!(
        outcome.failed,
        [(
            "y#1".to_string(),
            Some("RuntimeError: kaboom y".to_string())
        )]
    );
    let events = project.events("runs/one");
    let retries = named(&events, "node_retry");
    let attempts: Vec<&Value> = retries.iter().map(|e| &e["attempt"]).collect();
    assert_eq!(
        attempts,
        [&json!(1), &json!(2), &json!(3), &json!(4), &json!(5)]
    );
    assert!(
        retries
            .iter()
            .all(|e| e["error"] == json!("RuntimeError: kaboom y"))
    );
    assert_eq!(named(&events, "node_failed").len(), 1);
}

#[test]
fn a_run_time_problem_stops_the_run() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1
id: case
title: Too many items
steps:
  list:
    uses: ./nodes/cases.py#count
    with: { n: 3 }
  each:
    for_each: ${{ steps.list.outputs.items.lines }}
    max: 2
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ item }}\" }
",
    )]);
    let Some(outcome) = run_in(&project, Invocation::default(), None) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(!outcome.ok && outcome.incomplete);
    let stopped = outcome.stopped.unwrap();
    assert_eq!(
        stopped,
        "a value the run produced breaks the workflow: each.for_each: 3 items exceed max: 2"
    );
    let events = project.events("runs/one");
    let problems = named(&events, "problem");
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0]["where"], json!("each.for_each"));
    assert_eq!(problems[0]["message"], json!("3 items exceed max: 2"));
    assert_eq!(named(&events, "node_started").len(), 1);
    assert_eq!(events.last().unwrap()["stopped"], json!(stopped));
}

#[test]
fn yes_up_to_stops_before_a_phase_that_would_pass_it() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1
id: case
title: A priced second phase
steps:
  list:
    uses: ./nodes/cases.py#count
    with: { n: 3 }
  draw:
    for_each: ${{ steps.list.outputs.items.lines }}
    max: 6
    uses: fx/image.generate@1
    with: { prompt: \"${{ item }}\" }
",
    )]);
    let Some(outcome) = run_in(
        &project,
        Invocation {
            yes_up_to: Some("0.1"),
            ..Invocation::default()
        },
        None,
    ) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert_eq!(
        outcome.stopped.as_deref(),
        Some(
            "phase 2 may cost up to $0.12, which takes the run past --yes-up-to 0.1; approve it \
             with a higher --yes-up-to"
        )
    );
    assert!(outcome.incomplete && !outcome.ok);
    assert_eq!(outcome.failed, []);
    let events = project.events("runs/one");
    let phases: Vec<(Value, Value)> = named(&events, "phase_planned")
        .iter()
        .map(|e| (e["phase"].clone(), e["high_usd"].clone()))
        .collect();
    assert_eq!(phases, [(json!(1), json!(0)), (json!(2), json!(0.12))]);
    let started: Vec<&str> = named(&events, "node_started")
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(started, ["list#1"]);
    // No stuck instance is reported for a run the gate stopped.
    assert!(named(&events, "problem").is_empty());
}

#[test]
fn a_recorded_call_replays_offline_and_then_the_result_cache_answers() {
    let _serial = serial();
    let project = Project::conformance("cache-replay");
    let Some(outcome) = run_in(&project, Invocation::default(), None) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(outcome.charged, Usd::ZERO);
    let image = &outcome.outputs["image"];
    let grida_fx_core::val::Val::File(file) = image else {
        panic!("{image:?}");
    };
    assert_eq!(
        file.digest,
        "f3945de0c1182a1b279816f51ef2e79938d04957c7bfde94cd4bf2eb4c2170b4"
    );
    assert!(project.root.join("runs/one/outputs/image.png").is_file());
    let events = project.events("runs/one");
    let calls = named(&events, "call");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["cached"], json!(true));
    assert_eq!(named(&events, "node_finished")[0]["cache"], json!("miss"));

    let second = run_in(
        &project,
        Invocation {
            folder: "runs/two",
            ..Invocation::default()
        },
        None,
    )
    .unwrap()
    .unwrap();
    assert!(second.ok);
    let events = project.events("runs/two");
    assert!(named(&events, "call").is_empty());
    assert_eq!(named(&events, "node_finished")[0]["cache"], json!("hit"));
}

#[test]
fn a_paid_call_offline_fails_only_its_step() {
    let _serial = serial();
    let project = Project::conformance("cache-replay");
    std::fs::remove_dir_all(project.root.join(".fx")).unwrap();
    let Some(outcome) = run_in(&project, Invocation::default(), None) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(!outcome.ok);
    assert_eq!(outcome.stopped, None);
    assert_eq!(outcome.failed.len(), 1);
    assert!(
        outcome.failed[0]
            .1
            .as_deref()
            .unwrap()
            .contains("is a paid call; run with --live"),
        "{outcome:?}"
    );
}

#[test]
fn a_live_run_without_a_ceiling_is_refused() {
    let _serial = serial();
    let project = Project::conformance("cache-replay");
    let Some(outcome) = run_in(
        &project,
        Invocation {
            live: true,
            ..Invocation::default()
        },
        None,
    ) else {
        return;
    };
    assert_eq!(
        outcome,
        Err(RunError::Refused(
            "a live run needs a ceiling: pass --max-usd, or set budget.max_usd in the workflow \
             or in fx.yaml"
                .into()
        ))
    );
    assert!(!project.root.join("runs/one").exists());
}

#[test]
fn ctrl_c_cancels_the_run() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1\nid: case\ntitle: Slow\nsteps:\n  wait:\n    uses: \
         ./nodes/cases.py#slow\n    with: { text: w }\n",
    )]);
    let events = project.root.join("runs/one/events.jsonl");
    let interrupt = Box::new(move || {
        // Once the step started, the runner listens for Ctrl-C.
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            let started = std::fs::read_to_string(&events)
                .map(|text| text.contains("\"node_started\""))
                .unwrap_or(false);
            if started {
                std::thread::sleep(Duration::from_millis(200));
                let pid = std::process::id().to_string();
                std::process::Command::new("kill")
                    .args(["-INT", &pid])
                    .status()
                    .unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the step never started");
    });
    let begin = Instant::now();
    let Some(outcome) = run_in(&project, Invocation::default(), Some(interrupt)) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(outcome.cancelled && !outcome.ok);
    assert!(outcome.outputs.is_empty());
    assert!(begin.elapsed() < Duration::from_secs(25));
    let events = project.events("runs/one");
    let names = names(&events);
    assert_eq!(names.last().map(String::as_str), Some("run_cancelled"));
    assert_eq!(events.last().unwrap()["reason"], json!("interrupted"));
    assert!(
        !names
            .iter()
            .any(|n| n == "run_finished" || n == "node_finished" || n == "node_failed")
    );
    assert!(!project.root.join("runs/one/outputs").exists());
}
