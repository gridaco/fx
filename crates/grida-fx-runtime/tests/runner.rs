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
use grida_fx_providers::fake::FakeAdapter;
use grida_fx_providers::{Adapter, Adapters, Answer, BoxFuture, CallRequest, RequestAdapter, Sent};
use grida_fx_runtime::engine::Engine;
use grida_fx_runtime::events::read_events_tolerant;
use grida_fx_runtime::folder::RunFolder;
use grida_fx_runtime::host::PythonHost;
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::plantime::PlanTime;
use grida_fx_runtime::runner::{RunError, RunOptions, RunOutcome, run};
use grida_fx_runtime::stand_in::{Answerer, AnswererError, Reply, StandIn, StandInFile};
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

import asyncio
import time

from grida.fx import Ctx, node


@node("shout", params={"text": str}, outputs={"text": "text"}, version=1)
def shout(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.params["text"].upper())}


@node("upper", inputs={"text": "text"}, outputs={"text": "text"}, version=1)
def upper(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.read.text("text").upper())}


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


@node("flag", params={"value": str, "delay": float}, outputs={"text": "text"}, version=1)
def flag(ctx: Ctx) -> dict:
    time.sleep(ctx.params["delay"])
    ctx.fact("flag", ctx.params["value"])
    return {"text": ctx.out.text(ctx.params["value"])}


@node(
    "wraps",
    params={"prompt": str},
    outputs={"image": "image"},
    calls={"image.generate": 1},
    version=1,
    retry="engine",
)
async def wraps(ctx: Ctx) -> dict:
    try:
        made = await ctx.image_generate(prompt=ctx.params["prompt"])
    except Exception as error:
        raise RuntimeError(f"generation failed: {error}")
    return {"image": made.files["image"]}


@node("paid_slowly", params={"prompt": str}, outputs={"image": "image"}, calls={"image.generate": 1}, version=1)
async def paid_slowly(ctx: Ctx) -> dict:
    made = await ctx.image_generate(prompt=ctx.params["prompt"])
    return {"image": made.files["image"]}


@node("asks_late", params={"prompt": str}, outputs={"image": "image"}, calls={"image.generate": 1}, version=1)
async def asks_late(ctx: Ctx) -> dict:
    while not ctx.cancelled:
        await asyncio.sleep(0.02)
    made = await ctx.image_generate(prompt=ctx.params["prompt"])
    return {"image": made.files["image"]}


@node("taken", params={"text": str}, outputs={"text": "text"}, version=1)
def taken(ctx: Ctx) -> dict:
    takes = ".".join(map(str, ctx.instance.takes))
    return {"text": ctx.out.text(f"take {takes}: {ctx.params['text']}")}
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
    /// `--max-usd`.
    max_usd: Option<&'a str>,
    /// The adapter serving `image.generate` on `acme`.
    adapter: Option<Adapter>,
    /// A stand-in run's answerer: the engine's store is then the stand-in store.
    stand_in: Option<Arc<dyn Answerer>>,
}

impl Default for Invocation<'_> {
    fn default() -> Self {
        Invocation {
            target: "case",
            inputs: &[],
            folder: "runs/one",
            yes_up_to: None,
            live: false,
            max_usd: None,
            adapter: None,
            stand_in: None,
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
        max_usd: invocation.max_usd.map(|text| Usd::parse(text).unwrap()),
        ..PlanRequest::default()
    };
    let mut host = PythonHost::new().with_python(python.clone());
    let mut planner = make_planner(&request, &mut host).unwrap();
    let mut adapters = Adapters::new();
    if let Some(adapter) = invocation.adapter {
        adapters.register("image.generate", "acme", adapter);
    }
    let store = match invocation.stand_in {
        Some(_) => planner.project.cache_dir().join("stand-in"),
        None => planner.project.cache_dir(),
    };
    let engine = Engine::new(
        runtime.handle().clone(),
        HostSpec {
            python: python.clone(),
            label: python.display().to_string(),
            project_root: planner.home.root.clone(),
            sources: planner.home.document.sources.clone(),
        },
        &store,
        2,
        adapters,
        invocation.live,
    );
    let engine = Arc::new(match invocation.stand_in {
        Some(answerer) => engine.with_stand_in(Arc::new(StandIn::new(answerer))),
        None => engine,
    });
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
        name: None,
        yes_up_to: invocation.yes_up_to.map(|text| Usd::parse(text).unwrap()),
        takes_file: format!("workflows/{}.takes.yaml", invocation.target),
        ..RunOptions::default()
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

#[test]
fn dynamic_import_scopes_are_recorded_before_failed_and_blocked_nodes() {
    let _serial = serial();
    let project = Project::new(&[
        (
            "case",
            r#"
fx: workflow/v1
id: case
title: Dynamic imported failures
steps:
  count: { uses: ./nodes/cases.py#count, with: { n: 1 } }
  imported:
    for_each: "${{ steps.count.outputs.items.lines }}"
    key: "${{ item }}"
    max: 1
    uses: ./workflows/inner.yaml
    with: { name: "${{ item }}" }
"#,
        ),
        (
            "inner",
            r#"
fx: workflow/v1
id: inner
title: Inner failure
inputs:
  name: { type: string }
steps:
  fail:
    uses: ./nodes/cases.py#refuse
    with: { text: "${{ inputs.name }}" }
  blocked:
    uses: ./nodes/cases.py#upper
    with: { text: "${{ steps.fail.outputs.text }}" }
outputs:
  result: "${{ steps.blocked.outputs.text }}"
"#,
        ),
    ]);
    let Some(outcome) = run_in(&project, Invocation::default(), None) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(!outcome.ok);
    assert_eq!(outcome.charged, Usd::ZERO);
    let events = project.events("runs/one");
    let scope_id = "scope:imported['l0']#";
    let first_scope = events
        .iter()
        .position(|event| {
            event["event"] == "scopes_updated"
                && event["scopes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["id"] == scope_id)
        })
        .unwrap();
    let failed = events
        .iter()
        .position(|event| event["event"] == "node_failed" && event["id"] == "imported['l0'].fail#1")
        .unwrap();
    let skipped = events
        .iter()
        .position(|event| {
            event["event"] == "node_skipped" && event["id"] == "imported['l0'].blocked#1"
        })
        .unwrap();
    assert!(first_scope < failed && first_scope < skipped);
    assert!(
        !named(&events, "node_started")
            .iter()
            .any(|event| event["id"] == "imported['l0'].blocked#1")
    );
    let snapshot = named(&events, "scopes_updated").last().copied().unwrap();
    assert_eq!(snapshot["scopes"][0]["id"], scope_id);
    assert_eq!(
        snapshot["scopes"][0]["nodes"],
        json!(["imported['l0'].fail#1", "imported['l0'].blocked#1"])
    );
    assert_eq!(
        snapshot["node_interface_bindings"]["imported['l0'].fail#1"],
        json!([{
            "source":scope_id,"source_port":"name","target_port":"text","source_kind":"scope_input"
        }])
    );
    let plan: Value =
        serde_json::from_slice(&std::fs::read(project.root.join("runs/one/plan.json")).unwrap())
            .unwrap();
    assert_eq!(plan["scopes"], json!([]));
    let schema: Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/fx-run-events-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for event in named(&events, "scopes_updated") {
        assert!(validator.is_valid(event), "{event}");
    }
}

#[test]
fn scope_snapshots_list_members_before_they_start_and_pending_repeats_until_they_expand() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1
id: case
title: Members before they start
steps:
  list:
    uses: ./nodes/cases.py#count
    with: { n: 2 }
  draw:
    uses: ./nodes/cases.py#shout
    with: { text: hi }
  check:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: \"${{ steps.draw.outputs.text }}\", accept_take: 1 }
    on_reject: { regenerate: { max: 3, then: fail } }
  loud:
    for_each: ${{ steps.list.outputs.items.lines }}
    max: 4
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ item }}\" }
",
    )]);
    let Some(outcome) = run_in(&project, Invocation::default(), None) else {
        return;
    };
    assert!(outcome.unwrap().ok);
    let events = project.events("runs/one");
    let snapshots: Vec<(usize, &Value)> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| event["event"] == "scopes_updated")
        .collect();
    let schema: Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/fx-run-events-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for (_, snapshot) in &snapshots {
        assert!(validator.is_valid(snapshot), "{snapshot}");
    }
    let ids = |snapshot: &Value| -> Vec<String> {
        snapshot["instances"]
            .as_array()
            .unwrap()
            .iter()
            .map(|instance| instance["id"].as_str().unwrap().to_string())
            .collect()
    };
    // Before anything runs: the plan's instances in its order, later takes that may run
    // included, and the repeat whose list `list` makes, with what it waits on.
    let plan: Value =
        serde_json::from_slice(&std::fs::read(project.root.join("runs/one/plan.json")).unwrap())
            .unwrap();
    let planned: Vec<Value> = plan["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| json!({"id": i["id"], "step": i["step"], "take": i["take"], "key": i["key"]}))
        .collect();
    let (first_at, first) = snapshots[0];
    let first_start = events
        .iter()
        .position(|event| event["event"] == "node_started")
        .unwrap();
    assert!(first_at < first_start);
    assert_eq!(first["instances"], Value::Array(planned));
    assert!(ids(first).contains(&"draw#3".to_string()));
    assert_eq!(
        first["pending"],
        json!([{"path": "loud", "step": "loud", "max": 4, "phase": 2, "high_usd": 0,
                "waiting_on": ["list#1"]}])
    );
    // Once the list exists, every item is listed with its key before the first of them starts,
    // and nothing is pending any more.
    let (expanded_at, expanded) = *snapshots
        .iter()
        .find(|(_, snapshot)| snapshot["pending"] == json!([]))
        .unwrap();
    let items: Vec<&Value> = expanded["instances"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|instance| instance["step"] == "loud")
        .collect();
    assert_eq!(
        items,
        [
            &json!({"id": "loud['0']#1", "step": "loud", "take": [1], "key": "0"}),
            &json!({"id": "loud['1']#1", "step": "loud", "take": [1], "key": "1"}),
        ]
    );
    let item_start = events
        .iter()
        .position(|event| event["event"] == "node_started" && event["step"] == "loud")
        .unwrap();
    assert!(expanded_at < item_start);
    // Once the first take is accepted the later ones are absent, so never members: the final
    // snapshot leaves them out, though their interface bindings are still recorded.
    let last = snapshots.last().unwrap().1;
    let last_ids = ids(last);
    for absent in ["draw#2", "draw#3", "check#2", "check#3"] {
        assert!(!last_ids.contains(&absent.to_string()), "{absent}");
        assert!(
            last["node_interface_bindings"].get(absent).is_some(),
            "{absent}"
        );
    }
    for present in ["list#1", "draw#1", "check#1", "loud['0']#1", "loud['1']#1"] {
        assert!(last_ids.contains(&present.to_string()), "{present}");
    }
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
    assert_eq!(
        later_names,
        ["run_started", "scopes_updated", "run_finished"]
    );
    let initial_display = events
        .iter()
        .rev()
        .find(|event| event["invocation_id"] == ids[0] && event["event"] == "scopes_updated")
        .unwrap();
    assert_eq!(later[1]["scopes"], initial_display["scopes"]);
    assert_eq!(
        later[1]["node_interface_bindings"],
        initial_display["node_interface_bindings"]
    );
    let execution: Vec<&Value> = later
        .into_iter()
        .filter(|event| event["event"] != "scopes_updated")
        .collect();
    assert_eq!(
        execution
            .iter()
            .map(|event| event["event"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["run_started", "run_finished"]
    );
    assert_eq!(execution[0]["resumed"], json!(true));
    assert_eq!(
        execution[1]["outputs"],
        run_finished_outputs(&events, &ids[0])
    );

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
    // Ctrl-C, and SIGTERM the same way.
    for signal in ["-INT", "-TERM"] {
        if !cancelled_by(signal) {
            return;
        }
    }
}

/// Runs a slow step and sends this process `signal` once it started; `false` when there is no
/// Python to host nodes.
fn cancelled_by(signal: &'static str) -> bool {
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
                    .args([signal, &pid])
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
        return false;
    };
    let outcome = outcome.unwrap();
    assert!(outcome.cancelled && !outcome.ok, "{signal}");
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
    true
}

// ------------------------------------------------------------------ paid calls and the end of a run

/// An adapter that answers $0.01 with a one-byte image after `delay`, counting its sends.
struct Slow {
    delay: Duration,
    sends: Arc<std::sync::atomic::AtomicU32>,
}

impl RequestAdapter for Slow {
    fn send<'a>(&'a self, _call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        self.sends.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            tokio::time::sleep(self.delay).await;
            Sent::Answered(Answer::new(json!({}), Some(Usd(10_000))).with_file(
                "image",
                "image/png",
                vec![1],
            ))
        })
    }
}

fn budget(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e["event"].as_str()? {
            name @ ("budget_reserved" | "budget_settled" | "node_failed" | "run_finished") => {
                Some(name.to_string())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_paid_call_left_by_a_timeout_settles_before_the_run_ends() {
    let _serial = serial();
    for uses in ["fx/image.generate@1", "./nodes/cases.py#paid_slowly"] {
        let project = Project::new(&[(
            "case",
            &format!(
                "fx: workflow/v1\nid: case\ntitle: A timeout\nsteps:\n  draw:\n    uses: \
                 {uses}\n    timeout: 0.5\n    with: {{ prompt: lantern }}\n"
            ),
        )]);
        let sends = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let slow = Slow {
            delay: Duration::from_millis(1500),
            sends: Arc::clone(&sends),
        };
        let Some(outcome) = run_in(
            &project,
            Invocation {
                live: true,
                max_usd: Some("1"),
                adapter: Some(Adapter::Request(Arc::new(slow))),
                ..Invocation::default()
            },
            None,
        ) else {
            return;
        };
        let outcome = outcome.unwrap();
        assert_eq!(
            outcome.failed,
            [(
                "draw#1".to_string(),
                Some("ran past 0.5 seconds".to_string())
            )],
            "{uses}"
        );
        // The call the step left went on, settled, and was counted before the run ended.
        assert_eq!(outcome.charged, Usd(10_000), "{uses}");
        let events = project.events("runs/one");
        assert_eq!(
            budget(&events),
            [
                "budget_reserved",
                "node_failed",
                "budget_settled",
                "run_finished"
            ],
            "{uses}"
        );
        assert_eq!(events.last().unwrap()["charged_usd"], json!(0.01));
        // Its answer is in the call cache: running again sends nothing and pays nothing more.
        let fake = FakeAdapter::plain(Vec::new());
        let again = run_in(
            &project,
            Invocation {
                live: true,
                max_usd: Some("1"),
                adapter: Some(fake.as_request()),
                ..Invocation::default()
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert!(again.ok, "{uses}: {again:?}");
        assert!(fake.log().is_empty());
        assert_eq!(again.charged, Usd(10_000));
        assert_eq!(sends.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

#[test]
fn a_body_past_its_deadline_can_make_no_paid_call() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1\nid: case\ntitle: Late\nsteps:\n  draw:\n    uses: \
         ./nodes/cases.py#asks_late\n    timeout: 0.5\n    with: { prompt: lantern }\n",
    )]);
    let fake = FakeAdapter::plain(Vec::new());
    let Some(outcome) = run_in(
        &project,
        Invocation {
            live: true,
            max_usd: Some("1"),
            adapter: Some(fake.as_request()),
            ..Invocation::default()
        },
        None,
    ) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert_eq!(
        outcome.failed,
        [(
            "draw#1".to_string(),
            Some("ran past 0.5 seconds".to_string())
        )]
    );
    assert!(fake.log().is_empty());
    let events = project.events("runs/one");
    assert!(named(&events, "budget_reserved").is_empty());
    assert_eq!(outcome.charged, Usd(0));
}

#[test]
fn reruns_of_a_body_never_send_a_call_that_failed_again() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1\nid: case\ntitle: Reruns\nsteps:\n  draw:\n    uses: \
         ./nodes/cases.py#wraps\n    with: { prompt: lantern }\n",
    )]);
    let failing = FakeAdapter::plain(
        (0..10)
            .map(|_| Sent::Failed {
                reason: "provider 400".into(),
                cost: None,
                retryable: false,
            })
            .collect(),
    );
    let Some(outcome) = run_in(
        &project,
        Invocation {
            live: true,
            max_usd: Some("1"),
            adapter: Some(failing.as_request()),
            ..Invocation::default()
        },
        None,
    ) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(!outcome.ok);
    let events = project.events("runs/one");
    // Six runs of the body, one request sent and paid for.
    assert_eq!(named(&events, "node_retry").len(), 5);
    assert_eq!(failing.log().len(), 1);
    assert_eq!(named(&events, "budget_reserved").len(), 1);
    assert_eq!(outcome.charged, Usd(40_000));
}

// ------------------------------------------------------------------ run-time assertions

/// A step asserting over another step's fact: `flagger` reports `flag: no` after `delay`
/// seconds, and `strict` runs `work`.
fn asserted(delay: &str, work: &str) -> String {
    format!(
        "fx: workflow/v1
id: case
title: A run-time assertion
steps:
  flagger:
    uses: ./nodes/cases.py#flag
    with: {{ value: \"no\", delay: {delay} }}
  strict:
    uses: ./nodes/cases.py#{work}
    with: {{ text: s }}
    assert:
      - check: ${{{{ steps.flagger.facts.flag == 'yes' }}}}
        message: \"flag is ${{{{ steps.flagger.facts.flag }}}}\"
  after:
    uses: ./nodes/cases.py#shout
    with: {{ text: \"${{{{ steps.strict.outputs.text }}}}\" }}
"
    )
}

#[test]
fn an_assertion_decided_after_its_step_ran_fails_the_step() {
    let _serial = serial();
    // `strict` either ends before `flagger` reports (`shout`) or is still running (`slow`).
    for (delay, work) in [("0.6", "shout"), ("0", "slow")] {
        let project = Project::new(&[("case", &asserted(delay, work))]);
        let Some(outcome) = run_in(&project, Invocation::default(), None) else {
            return;
        };
        let outcome = outcome.unwrap();
        assert!(!outcome.ok, "{work}");
        assert_eq!(
            outcome.failed.first(),
            Some(&("strict#1".to_string(), Some("flag is no".to_string()))),
            "{work}: {:?}",
            outcome.failed
        );
        let events = project.events("runs/one");
        let last = events
            .iter()
            .rev()
            .find(|e| e["id"] == json!("strict#1"))
            .unwrap();
        assert_eq!(last["event"], json!("node_failed"), "{work}");
        assert_eq!(last["error"], json!("flag is no"));
        // Resuming keeps the failure: the step's result is the assertion's.
        let again = run_in(&project, Invocation::default(), None)
            .unwrap()
            .unwrap();
        assert!(
            again
                .failed
                .iter()
                .any(|(id, error)| id == "strict#1" && error.as_deref() == Some("flag is no")),
            "{work}: {:?}",
            again.failed
        );
    }
}

#[test]
fn a_step_whose_assertion_skips_it_is_held_and_its_readers_never_run() {
    let _serial = serial();
    // `lenient` reads nothing through `with:`; only its assertion waits for `flagger`, which
    // reports late. Dispatched at once, `lenient` and `after` would both have run.
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1
id: case
title: A run-time assertion that skips its step
steps:
  flagger:
    uses: ./nodes/cases.py#flag
    with: { value: \"no\", delay: 0.4 }
  lenient:
    uses: ./nodes/cases.py#shout
    with: { text: l }
    assert:
      - check: ${{ steps.flagger.facts.flag == 'yes' }}
        message: \"flag is ${{ steps.flagger.facts.flag }}\"
        on_fail: skip
  after:
    uses: ./nodes/cases.py#upper
    with: { text: \"${{ steps.lenient.outputs.text }}\" }
",
    )]);
    let Some(outcome) = run_in(&project, Invocation::default(), None) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(outcome.ok, "{outcome:?}");
    let events = project.events("runs/one");
    let started: Vec<&str> = named(&events, "node_started")
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(started, ["flagger#1"], "{outcome:?}");
    for id in ["lenient#1", "after#1"] {
        assert!(
            !events
                .iter()
                .any(|e| e["id"] == json!(id) && e["event"] == json!("node_finished")),
            "{id} ran: {outcome:?}"
        );
    }
}

// ------------------------------------------------------------------ the record and the folder

#[test]
fn every_take_of_a_step_is_placed_where_inspect_finds_it() {
    let _serial = serial();
    let project = Project::new(&[(
        "case",
        "fx: workflow/v1
id: case
title: Takes
steps:
  draw:
    uses: ./nodes/cases.py#taken
    with: { text: lantern }
  check:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: \"${{ steps.draw.outputs.text }}\", accept_take: 2 }
    on_reject: { regenerate: { max: 3, then: fail } }
outputs:
  kept: ${{ steps.draw.outputs.text }}
",
    )]);
    let Some(outcome) = run_in(&project, Invocation::default(), None) else {
        return;
    };
    let outcome = outcome.unwrap();
    assert!(outcome.ok, "{outcome:?}");
    let events = project.events("runs/one");
    let finished: Vec<&Value> = named(&events, "node_finished")
        .into_iter()
        .filter(|e| e["path"] == json!("draw"))
        .collect();
    assert_eq!(finished.len(), 2);
    for event in finished {
        let id = event["id"].as_str().unwrap();
        let takes = grida_fx_runtime::folder::takes_of_id(id);
        let folder = grida_fx_runtime::folder::step_folder("draw", &takes);
        let digest = event["outputs"]["text"]["file"]["digest"].as_str().unwrap();
        let bytes = std::fs::read(
            project
                .root
                .join("runs/one/files")
                .join(&folder)
                .join("text.txt"),
        )
        .unwrap();
        assert_eq!(grida_fx_core::value::file_digest(&bytes), digest, "{id}");
    }
    assert!(
        project
            .root
            .join("runs/one/files/draw#2/text.txt")
            .is_file()
    );
}

#[test]
fn a_folder_with_another_plans_events_is_refused() {
    let _serial = serial();
    let project = Project::new(&[("case", CASE)]);
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
    // The record names another plan (as an invocation racing this one would have left it).
    let events = project.root.join("runs/one/events.jsonl");
    let text = std::fs::read_to_string(&events).unwrap();
    let digest = project.events("runs/one")[0]["plan"]
        .as_str()
        .unwrap()
        .to_string();
    let first_line = text
        .lines()
        .next()
        .unwrap()
        .replace(&digest, &"0".repeat(64));
    std::fs::write(&events, format!("{first_line}\n{text}")).unwrap();
    let refused = run_in(
        &project,
        Invocation {
            inputs: &["inputs.yaml"],
            ..Invocation::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(
        refused,
        Err(RunError::Refused(
            "runs/one holds a run of another workflow or other inputs; choose a new folder".into()
        ))
    );
}

#[test]
fn a_torn_last_line_is_left_out_and_the_run_goes_on() {
    let _serial = serial();
    let project = Project::new(&[("case", CASE)]);
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
    let events = project.root.join("runs/one/events.jsonl");
    let mut text = std::fs::read_to_string(&events).unwrap();
    let lines = text.lines().count();
    text.push_str("{\"kind\":\"fx-run-events-v1\",\"event\":\"node_sta");
    std::fs::write(&events, text).unwrap();
    let again = run_in(
        &project,
        Invocation {
            inputs: &["inputs.yaml"],
            ..Invocation::default()
        },
        None,
    )
    .unwrap()
    .unwrap();
    assert!(again.ok);
    let after = project.events("runs/one");
    assert!(after.len() > lines);
    let text = std::fs::read_to_string(&events).unwrap();
    assert!(
        text.lines()
            .all(|line| serde_json::from_str::<Value>(line).is_ok())
    );
}

/// A stand-in that answers every call with one small picture and counts what it was asked.
#[derive(Default)]
struct Pictures {
    asked: Mutex<Vec<String>>,
}

impl Answerer for Pictures {
    fn answer<'a>(
        &'a self,
        params: grida_fx_protocol::StandInAnswerParams,
        _cancel: &'a grida_fx_runtime::engine::Cancel,
    ) -> BoxFuture<'a, Result<Reply, AnswererError>> {
        Box::pin(async move {
            self.asked.lock().unwrap().push(params.key);
            let bytes = grida_fx_providers::testing::media::png(8, 8, None);
            Ok(Reply::Answer {
                files: [("image".to_string(), StandInFile { kind: None, bytes })]
                    .into_iter()
                    .collect(),
                data: Value::Null,
            })
        })
    }

    fn shutdown(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// Every file under `root` but the stand-in store, with its bytes.
fn store_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == root.join("stand-in") {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                folders.push(path);
            } else {
                found.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn a_stand_in_run_keeps_to_its_own_store_and_its_own_folders() {
    let _serial = serial();
    let project = Project::conformance("cache-replay");
    // A plain run replays the recorded call and records a result in the store.
    let Some(plain) = run_in(
        &project,
        Invocation {
            folder: "runs/plain",
            ..Invocation::default()
        },
        None,
    ) else {
        return;
    };
    assert!(plain.unwrap().ok);
    let store = project.root.join(".fx/cache");
    let before = store_files(&store);
    assert!(
        before
            .iter()
            .any(|(path, _)| path.starts_with(store.join("results")))
    );

    // A stand-in run reads none of it: the stand-in is asked, and what it answers is kept apart.
    let pictures = Arc::new(Pictures::default());
    let stand_in = || Invocation {
        stand_in: Some(pictures.clone() as Arc<dyn Answerer>),
        ..Invocation::default()
    };
    let outcome = run_in(&project, stand_in(), None).unwrap().unwrap();
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(outcome.charged, Usd::ZERO);
    assert_eq!(pictures.asked.lock().unwrap().len(), 1);
    assert_eq!(store_files(&store), before, "the store is untouched");
    assert!(store.join("stand-in/calls").is_dir());
    assert!(store.join("stand-in/results").is_dir());
    assert!(!store.join("stand-in/jobs").exists());

    // Marked in plan.json and run_started; the plan digest is the same in both modes.
    let plan = |folder: &str| -> Value {
        serde_json::from_slice(&std::fs::read(project.root.join(folder).join("plan.json")).unwrap())
            .unwrap()
    };
    assert_eq!(plan("runs/one")["stand_in"], json!(true));
    assert!(plan("runs/plain").get("stand_in").is_none());
    assert_eq!(plan("runs/one")["plan"], plan("runs/plain")["plan"]);
    let events = project.events("runs/one");
    assert_eq!(named(&events, "run_started")[0]["stand_in"], json!(true));
    assert!(
        named(&project.events("runs/plain"), "run_started")[0]
            .get("stand_in")
            .is_none()
    );
    let calls = named(&events, "call");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["stand_in"], json!(true));
    assert_eq!(calls[0]["cost_usd"], json!(0));
    assert!(named(&events, "budget_reserved").is_empty());
    assert_eq!(
        named(&events, "node_finished")[0]["facts"]["cost_usd"],
        json!(0)
    );

    // Resumed in its own mode, it replays from the stand-in store and asks nothing.
    let resumed = run_in(&project, stand_in(), None).unwrap().unwrap();
    assert!(resumed.ok);
    assert_eq!(pictures.asked.lock().unwrap().len(), 1);

    // Each folder refuses the other mode.
    let refused = run_in(&project, Invocation::default(), None).unwrap();
    assert_eq!(
        refused,
        Err(RunError::Refused(
            "runs/one holds a stand-in run; resume it with --stand-in, or choose a new folder"
                .into()
        ))
    );
    let refused = run_in(
        &project,
        Invocation {
            folder: "runs/plain",
            ..stand_in()
        },
        None,
    )
    .unwrap();
    assert_eq!(
        refused,
        Err(RunError::Refused(
            "runs/plain holds a run without a stand-in; resume it without --stand-in, or choose \
             a new folder"
                .into()
        ))
    );
    assert_eq!(store_files(&store), before);
}
