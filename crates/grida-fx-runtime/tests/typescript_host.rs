//! The TypeScript node host as the engine starts it, and projects that hold modules of both
//! languages (spec/protocol.md §1; gridaco/fx#68 decision 7).
//!
//! The hosts here are fakes: a stdlib Python program that speaks the protocol as a host of either
//! language, started through a small shell script named as the interpreter or as Node, so the
//! engine's side (argv, environment, `initialize`, which host gets which module, which pool runs
//! which body) is checked without Node or the `grida` package. Every fake logs what it was asked
//! to `<language>.jsonl` beside the project. The tests skip when `python3` is not on `PATH`.
//!
//! Two tests run the real TypeScript host, one a node and one a project adapter: they skip unless
//! Node 24 or later is on `PATH` and the repository's `js/fx/dist/host-entry.js` has been built.
#![cfg(unix)]

use grida_fx_core::host::{HostFailure, Language, NodeHost};
use grida_fx_core::plan::make_plan;
use grida_fx_core::project::{PlanRequest, make_planner};
use grida_fx_core::val::Val;
use grida_fx_protocol::{BuildParams, DescribeParams, DescribeTarget, ModuleDescription};
use grida_fx_providers::Adapters;
use grida_fx_providers::testing::write_program;
use grida_fx_runtime::engine::Engine;
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::host::{LanguageHost, NodeHosts};
use grida_fx_runtime::plantime::PlanTime;
use grida_fx_runtime::runner::{RunOptions, RunOutcome, run};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

/// The fake host. `sys.argv[1]` is its language, `sys.argv[2]` the folder of its log; the rest is
/// what the engine passed.
const FAKE_HOST: &str = r#"
import json
import os
import sys

LANGUAGE = sys.argv[1]
LOG = os.path.join(sys.argv[2], LANGUAGE + ".jsonl")
ENGINE_ARGS = sys.argv[3:]
PROTOCOL_IN = sys.stdin.buffer
PROTOCOL_OUT = sys.stdout.buffer


def log(entry):
    with open(LOG, "a") as f:
        f.write(json.dumps(entry) + "\n")


def read():
    length = None
    while True:
        line = PROTOCOL_IN.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        name, _, value = line.decode("ascii").partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    return json.loads(PROTOCOL_IN.read(length).decode("utf-8"))


def write(message):
    body = json.dumps(message, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    PROTOCOL_OUT.write(b"Content-Length: " + str(len(body)).encode("ascii") + b"\r\n\r\n" + body)
    PROTOCOL_OUT.flush()


def answer(request, result):
    write({"jsonrpc": "2.0", "id": request["id"], "result": result})


def spec(name):
    return {
        "name": name,
        "description": None,
        "inputs": {},
        "params": {"text": {"type": "string"}},
        "outputs": {"text": "text"},
        "judge": False,
        "calls": {},
        "resources": [],
        "tools": [],
        "view": None,
        "version": None,
        "retry": "service",
    }


def describe(params):
    modules = []
    for target in params["targets"]:
        path = target["path"]
        if path.endswith("leak.ts"):
            # As Node names a file of the host's own package (gridaco/fx#68 review).
            package = os.path.dirname(os.path.dirname(ENGINE_ARGS[0]))
            modules.append({"path": path, "error": path + " failed to import: Error: not exported in "
                            + package + "/package.json imported from " + ENGINE_ARGS[0]})
            continue
        modules.append({
            "path": path,
            "types": [{"attribute": "shout", "spec": spec("shout")}],
            "closure": [{"label": path, "path": os.path.join(os.getcwd(), path)}],
        })
    return {"modules": modules, "builtins": []}


def run(params):
    text = LANGUAGE + ": " + params["params"]["text"]
    folder = os.path.join(params["work_dir"], "out")
    os.makedirs(folder, exist_ok=True)
    with open(os.path.join(folder, "text.txt"), "w") as f:
        f.write(text)
    return {"outputs": {"text": {"work_path": "out/text.txt", "kind": "text/plain"}}}


def main():
    shut_down = False
    while True:
        message = read()
        if message is None:
            sys.exit(0)
        method = message.get("method")
        params = message.get("params") or {}
        if method == "initialize":
            log({"method": method, "argv": ENGINE_ARGS, "cwd": os.getcwd(),
                 "safe_path": os.environ.get("PYTHONSAFEPATH"), "sources": params["sources"]})
            answer(message, {"protocol": params["protocol"],
                             "host": {"language": LANGUAGE, "version": "0", "sdk_version": "0.0.0-fake"}})
        elif method == "describe":
            log({"method": method, "targets": [t["path"] for t in params["targets"]],
                 "builtins": params["builtins"]})
            answer(message, describe(params))
        elif method == "build" and params["path"].endswith("leak.ts"):
            write({"jsonrpc": "2.0", "id": message["id"],
                   "error": {"code": -32004, "message": params["path"] + ": cannot load " + ENGINE_ARGS[0]}})
        elif method == "build":
            log({"method": method, "path": params["path"]})
            answer(message, {"document": {"fx": "workflow/v1", "id": "built", "title": "Built",
                                          "steps": {}},
                             "takes_anchor": params["path"]})
        elif method == "run":
            log({"method": method, "body": params["body"]})
            answer(message, run(params))
        elif method == "shutdown":
            shut_down = True
            answer(message, None)
        elif method == "exit":
            sys.exit(0 if shut_down else 1)
        elif "id" in message and method is not None:
            write({"jsonrpc": "2.0", "id": message["id"],
                   "error": {"code": -32601, "message": "no " + method}})


main()
"#;

fn have_python3() -> bool {
    let found = Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !found {
        eprintln!("skipped: python3 is not on PATH");
    }
    found
}

/// A project `acme` holding `nodes/a.py`, `nodes/b.ts` and `nodes/c.mjs`, with a fake
/// interpreter (`python-fake`) and a fake Node (`node-fake`) beside it, and a made-up
/// `@grida/fx` host program for the fake Node to be given.
struct Fixture {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    root: PathBuf,
    python: PathBuf,
    node: PathBuf,
    entry: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let root = base.join("acme");
        std::fs::create_dir_all(root.join("nodes")).unwrap();
        std::fs::create_dir_all(root.join("workflows")).unwrap();
        std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
        for module in ["nodes/a.py", "nodes/b.ts", "nodes/c.mjs"] {
            std::fs::write(root.join(module), "// made up\n").unwrap();
        }
        let host = base.join("fake_host.py");
        std::fs::write(&host, FAKE_HOST).unwrap();
        let fake = |name: &str, language: &str| {
            let program = base.join(name);
            write_program(
                &program,
                &format!(
                    "#!/bin/sh\nexec python3 '{}' {language} '{}' \"$@\"\n",
                    host.display(),
                    base.display()
                ),
            );
            program
        };
        let python = fake("python-fake", "python");
        let node = fake("node-fake", "typescript");
        let entry = base.join("sdk/dist/host-entry.js");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "").unwrap();
        Fixture {
            _dir: dir,
            dir: base,
            root,
            python,
            node,
            entry,
        }
    }

    fn python_host(&self) -> LanguageHost {
        LanguageHost::python().with_program(self.python.clone())
    }

    fn typescript_host(&self) -> LanguageHost {
        LanguageHost::typescript()
            .with_program(self.node.clone())
            .with_entry(self.entry.clone())
    }

    fn hosts(&self) -> NodeHosts {
        NodeHosts::of(self.python_host(), self.typescript_host())
    }

    fn python_spec(&self) -> HostSpec {
        HostSpec::python(&self.python, "python-fake", &self.root, Vec::new()).unchecked()
    }

    fn typescript_spec(&self) -> HostSpec {
        HostSpec::typescript(&self.node, &self.entry, "node-fake", &self.root, Vec::new())
            .unchecked()
    }

    /// What the host of `language` logged, in order.
    fn log(&self, language: &str) -> Vec<Value> {
        std::fs::read_to_string(self.dir.join(format!("{language}.jsonl")))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The `method` entries `language`'s host logged.
    fn asked(&self, language: &str, method: &str) -> Vec<Value> {
        self.log(language)
            .into_iter()
            .filter(|entry| entry["method"] == method)
            .collect()
    }
}

fn targets(paths: &[&str]) -> DescribeParams {
    DescribeParams {
        targets: paths
            .iter()
            .map(|path| DescribeTarget {
                path: path.to_string(),
                attribute: None,
            })
            .collect(),
        builtins: false,
    }
}

fn path_of(module: &ModuleDescription) -> &str {
    match module {
        ModuleDescription::Described { path, .. } | ModuleDescription::Failed { path, .. } => path,
    }
}

#[test]
fn the_typescript_host_is_node_running_its_host_program() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let mut host = fixture.typescript_host();
    let sources = ["acme_lib", "@acme/shapes", "AcmeLib", "acme-shapes"].map(String::from);
    host.open_project(&fixture.root, &sources).unwrap();
    assert_eq!(host.language(), Language::TypeScript);
    let described = host.describe(&targets(&["nodes/b.ts"])).unwrap();
    assert_eq!(path_of(&described.modules[0]), "nodes/b.ts");
    let info = host.info.clone().unwrap();
    assert_eq!(info.host.language, "typescript");
    let initialize = &fixture.asked("typescript", "initialize")[0];
    // `<node> <host-entry.js>`, and nothing Python's.
    assert_eq!(initialize["argv"], json!([fixture.entry]));
    if std::env::var_os("PYTHONSAFEPATH").is_none() {
        assert_eq!(initialize["safe_path"], Value::Null);
    }
    assert_eq!(
        std::fs::canonicalize(initialize["cwd"].as_str().unwrap()).unwrap(),
        fixture.root
    );
    // Only the source packages an npm name can name.
    assert_eq!(
        initialize["sources"],
        json!(["acme_lib", "@acme/shapes", "acme-shapes"])
    );
    let built = host
        .build(&BuildParams {
            path: "workflows/build.ts".into(),
            function: "build".into(),
            arguments: IndexMap::new(),
            cwd: fixture.root.display().to_string(),
        })
        .unwrap();
    assert_eq!(built.takes_anchor, "workflows/build.ts");
    host.shutdown().unwrap();
    assert!(fixture.log("python").is_empty(), "no Python host started");
}

#[test]
fn each_module_is_described_by_the_host_of_its_language_in_order() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let mut hosts = fixture.hosts();
    let sources = ["acme_lib", "@acme/shapes", "AcmeLib"].map(String::from);
    hosts.open_project(&fixture.root, &sources).unwrap();
    // Opening starts nothing.
    assert!(fixture.log("python").is_empty() && fixture.log("typescript").is_empty());
    let described = hosts
        .describe(&targets(&[
            "nodes/b.ts",
            "nodes/a.py",
            "nodes/d.cjs",
            "nodes/c.mjs",
            "nodes/e.py",
        ]))
        .unwrap();
    let paths: Vec<&str> = described.modules.iter().map(path_of).collect();
    assert_eq!(
        paths,
        [
            "nodes/b.ts",
            "nodes/a.py",
            "nodes/d.cjs",
            "nodes/c.mjs",
            "nodes/e.py"
        ]
    );
    assert_eq!(
        described.modules[2],
        ModuleDescription::Failed {
            path: "nodes/d.cjs".into(),
            attribute: None,
            error: "nodes/d.cjs: no node host for .cjs modules".into(),
        }
    );
    assert!(matches!(
        &described.modules[0],
        ModuleDescription::Described { types, .. } if types[0].attribute == "shout"
    ));
    // One describe per language, with only its own modules.
    assert_eq!(
        fixture.asked("python", "describe"),
        [json!({"method": "describe", "targets": ["nodes/a.py", "nodes/e.py"], "builtins": false})]
    );
    assert_eq!(
        fixture.asked("typescript", "describe"),
        [
            json!({"method": "describe", "targets": ["nodes/b.ts", "nodes/c.mjs"], "builtins": false})
        ]
    );
    // Each host got the source packages its language can name.
    assert_eq!(
        fixture.asked("python", "initialize")[0]["sources"],
        json!(["acme_lib", "AcmeLib"])
    );
    assert_eq!(
        fixture.asked("typescript", "initialize")[0]["sources"],
        json!(["acme_lib", "@acme/shapes"])
    );
    // A describe of one language asks only its host, and built-ins only Python's.
    hosts.describe(&targets(&["nodes/b.ts"])).unwrap();
    hosts
        .describe(&DescribeParams {
            targets: Vec::new(),
            builtins: true,
        })
        .unwrap();
    assert_eq!(fixture.asked("typescript", "describe").len(), 2);
    assert_eq!(
        fixture.asked("python", "describe")[1],
        json!({"method": "describe", "targets": [], "builtins": true})
    );
    // Builders go to the host of their file's language.
    let build = |path: &str| BuildParams {
        path: path.into(),
        function: "build".into(),
        arguments: IndexMap::new(),
        cwd: fixture.root.display().to_string(),
    };
    hosts.build(&build("workflows/build.mts")).unwrap();
    hosts.build(&build("workflows/build.py")).unwrap();
    assert_eq!(
        fixture.asked("typescript", "build"),
        [json!({"method": "build", "path": "workflows/build.mts"})]
    );
    assert_eq!(
        fixture.asked("python", "build"),
        [json!({"method": "build", "path": "workflows/build.py"})]
    );
    hosts.shutdown();
    assert_eq!(fixture.asked("python", "initialize").len(), 1);
    assert_eq!(fixture.asked("typescript", "initialize").len(), 1);
}

#[test]
fn a_language_without_a_host_leaves_the_other_working() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let mut hosts = NodeHosts::of(
        fixture.python_host(),
        LanguageHost::typescript().with_program(PathBuf::from("no-such-node-for-fx")),
    );
    hosts.open_project(&fixture.root, &[]).unwrap();
    let failure = hosts
        .describe(&targets(&["nodes/a.py", "nodes/b.ts"]))
        .unwrap_err();
    // No @grida/fx is found from the project (unless the environment names one), or the
    // missing Node cannot start: a sentence about the TypeScript host either way.
    let HostFailure::Unavailable(sentence) = failure else {
        panic!("{failure:?}")
    };
    assert!(sentence.contains("TypeScript node host"), "{sentence}");
    // The Python host answered its half, and still serves.
    let described = hosts.describe(&targets(&["nodes/a.py"])).unwrap();
    assert_eq!(path_of(&described.modules[0]), "nodes/a.py");
    assert_eq!(fixture.asked("python", "describe").len(), 2);
}

/// Plans `workflows/case.yaml` with `hosts` and runs it on `engine`; the outcome.
fn plan_and_run(fixture: &Fixture, hosts: &mut NodeHosts, engine: Engine) -> RunOutcome {
    let request = PlanRequest {
        target: "workflows/case.yaml".into(),
        cwd: fixture.root.clone(),
        ..PlanRequest::default()
    };
    let mut planner = make_planner(&request, hosts).unwrap();
    let engine = Arc::new(engine);
    let mut plan_time = PlanTime::new(Arc::clone(&engine));
    let plan = make_plan(
        &mut planner,
        hosts,
        Some(&mut plan_time),
        engine.store.as_ref(),
    )
    .unwrap();
    assert!(plan.ok(), "{:?}", plan.problems);
    let options = RunOptions {
        folder: fixture.root.join("runs/one"),
        label: "runs/one".into(),
        takes_file: "workflows/case.takes.yaml".into(),
        ..RunOptions::default()
    };
    let outcome = run(engine, &mut planner, hosts, plan, options).unwrap();
    hosts.shutdown();
    outcome
}

const MIXED: &str = "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  py:\n    uses: \
                     ./nodes/a.py#shout\n    with: { text: one }\n  ts:\n    uses: \
                     ./nodes/b.ts#shout\n    with: { text: two }\n  js:\n    uses: \
                     ./nodes/c.mjs#shout\n    with: { text: three }\noutputs:\n  py: ${{ \
                     steps.py.outputs.text }}\n  ts: ${{ steps.ts.outputs.text }}\n  js: ${{ \
                     steps.js.outputs.text }}\n";

/// The text of output `name`, read from the store.
fn output_text(engine_store: &Path, outcome: &RunOutcome, name: &str) -> String {
    let Some(Val::File(file)) = outcome.outputs.get(name) else {
        panic!("{name}: {:?}", outcome.outputs)
    };
    let store = grida_fx_runtime::store::Store::open(engine_store);
    std::fs::read_to_string(store.file_path(&file.digest).unwrap()).unwrap()
}

#[test]
fn a_mixed_workflow_runs_each_body_on_a_host_of_its_language() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    std::fs::write(fixture.root.join("workflows/case.yaml"), MIXED).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = fixture.root.join(".fx/cache");
    let engine = Engine::new(
        runtime.handle().clone(),
        fixture.python_spec(),
        &store,
        2,
        Adapters::new(),
        false,
    )
    .with_host(Language::TypeScript, Ok(fixture.typescript_spec()));
    let mut hosts = fixture.hosts();
    let outcome = plan_and_run(&fixture, &mut hosts, engine);
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(output_text(&store, &outcome, "py"), "python: one");
    assert_eq!(output_text(&store, &outcome, "ts"), "typescript: two");
    assert_eq!(output_text(&store, &outcome, "js"), "typescript: three");
    let bodies = |language: &str| -> Vec<Value> {
        let mut bodies: Vec<Value> = fixture
            .asked(language, "run")
            .into_iter()
            .map(|entry| entry["body"].clone())
            .collect();
        bodies.sort_by_key(|body| body["path"].as_str().unwrap_or_default().to_string());
        bodies
    };
    assert_eq!(
        bodies("python"),
        [json!({"path": "nodes/a.py", "attribute": "shout"})]
    );
    assert_eq!(
        bodies("typescript"),
        [
            json!({"path": "nodes/b.ts", "attribute": "shout"}),
            json!({"path": "nodes/c.mjs", "attribute": "shout"})
        ]
    );
    runtime.shutdown_timeout(Duration::from_secs(10));
}

#[test]
fn a_typescript_body_with_no_host_fails_alone() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    std::fs::write(fixture.root.join("workflows/case.yaml"), MIXED).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = fixture.root.join(".fx/cache");
    let refusal =
        "no @grida/fx for the TypeScript node host: run npm install @grida/fx in the project";
    let engine = Engine::new(
        runtime.handle().clone(),
        fixture.python_spec(),
        &store,
        2,
        Adapters::new(),
        false,
    )
    .with_host(Language::TypeScript, Err(refusal.into()));
    let mut hosts = fixture.hosts();
    let outcome = plan_and_run(&fixture, &mut hosts, engine);
    assert!(!outcome.ok);
    let mut failed = outcome.failed.clone();
    failed.sort();
    assert_eq!(
        failed,
        [
            ("js#1".to_string(), Some(refusal.to_string())),
            ("ts#1".to_string(), Some(refusal.to_string())),
        ]
    );
    assert_eq!(output_text(&store, &outcome, "py"), "python: one");
    assert!(fixture.asked("typescript", "run").is_empty());
    runtime.shutdown_timeout(Duration::from_secs(10));
}

#[test]
fn texts_never_name_a_grida_fx_outside_the_project() {
    if !have_python3() {
        return;
    }
    // The fixture's host program lies outside the project (`<base>/sdk`, as the SDK that started
    // the engine would): its folder is `@grida/fx` in what the planning host answers.
    let fixture = Fixture::new();
    let mut host = fixture.typescript_host();
    host.open_project(&fixture.root, &[]).unwrap();
    let described = host.describe(&targets(&["nodes/leak.ts"])).unwrap();
    let ModuleDescription::Failed { error, .. } = &described.modules[0] else {
        panic!("{described:?}")
    };
    assert_eq!(
        error,
        "nodes/leak.ts failed to import: Error: not exported in @grida/fx/package.json imported \
         from @grida/fx/dist/host-entry.js"
    );
    let built = host.build(&BuildParams {
        path: "workflows/leak.ts".into(),
        function: "build".into(),
        arguments: IndexMap::new(),
        cwd: fixture.root.display().to_string(),
    });
    let Err(HostFailure::Rpc(error)) = built else {
        panic!("{built:?}")
    };
    assert_eq!(
        error.message,
        "workflows/leak.ts: cannot load @grida/fx/dist/host-entry.js"
    );
    host.shutdown().unwrap();
}

/// Node 24 or later on `PATH`, and the repository's built TypeScript host program; `None`
/// (with the reason printed) when either is missing.
fn real_node_host() -> Option<(PathBuf, PathBuf)> {
    let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../js/fx/dist/host-entry.js");
    if !entry.is_file() {
        eprintln!("skipped: js/fx/dist/host-entry.js is not built");
        return None;
    }
    let version = Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    let major = version
        .as_deref()
        .and_then(|v| v.strip_prefix('v'))
        .and_then(|v| v.split('.').next())
        .and_then(|major| major.parse::<u32>().ok());
    match major {
        Some(major) if major >= 24 => Some((PathBuf::from("node"), entry.canonicalize().ok()?)),
        _ => {
            eprintln!("skipped: no Node 24 or later on PATH ({version:?})");
            None
        }
    }
}

const REAL_NODE: &str = r#"import { node } from "@grida/fx";
import { shout as loud } from "../lib/loud.ts";

export const shout = node(
  { name: "shout", params: { text: "string" }, outputs: { text: "text" } },
  (ctx) => ({ text: ctx.out.text(loud(ctx.params.text)) }),
);
"#;

#[test]
fn a_real_typescript_host_describes_and_runs_a_node() {
    let Some((node, entry)) = real_node_host() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap().join("acme");
    std::fs::create_dir_all(root.join("nodes")).unwrap();
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::create_dir_all(root.join("workflows")).unwrap();
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::write(root.join("package.json"), "{\"type\": \"module\"}\n").unwrap();
    std::fs::write(root.join("nodes/shout.ts"), REAL_NODE).unwrap();
    std::fs::write(
        root.join("lib/loud.ts"),
        "export function shout(text: string): string {\n  return text.toUpperCase();\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("workflows/case.yaml"),
        "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  s:\n    uses: ./nodes/shout.ts#shout\n    \
         with: { text: hello }\noutputs:\n  text: ${{ steps.s.outputs.text }}\n",
    )
    .unwrap();
    let typescript = || {
        LanguageHost::typescript()
            .with_program(node.clone())
            .with_entry(entry.clone())
    };
    // describe: the closure is the module and what it imports, labelled from the root.
    let mut host = typescript();
    host.open_project(&root, &[]).unwrap();
    let described = host.describe(&targets(&["nodes/shout.ts"])).unwrap();
    let ModuleDescription::Described { types, closure, .. } = &described.modules[0] else {
        panic!("{described:?}")
    };
    assert_eq!(types[0].attribute, "shout");
    let labels: Vec<&str> = closure.iter().map(|entry| entry.label.as_str()).collect();
    assert_eq!(labels, ["lib/loud.ts", "nodes/shout.ts"]);
    assert_eq!(host.info.as_ref().unwrap().host.language, "typescript");
    host.shutdown().unwrap();
    // run, on the engine's TypeScript pool.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = root.join(".fx/cache");
    let engine = Engine::new(
        runtime.handle().clone(),
        HostSpec::python("python3", "python3", &root, Vec::new()).unchecked(),
        &store,
        1,
        Adapters::new(),
        false,
    )
    .with_host(
        Language::TypeScript,
        Ok(HostSpec::typescript(&node, &entry, "node", &root, Vec::new()).unchecked()),
    );
    let fixture = Fixture {
        _dir: dir,
        dir: root.parent().unwrap().to_path_buf(),
        root: root.clone(),
        python: PathBuf::from("python3"),
        node: node.clone(),
        entry: entry.clone(),
    };
    let mut hosts = NodeHosts::of(LanguageHost::python(), typescript());
    let outcome = plan_and_run(&fixture, &mut hosts, engine);
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(output_text(&store, &outcome, "text"), "HELLO");
    runtime.shutdown_timeout(Duration::from_secs(10));
}

const REAL_ADAPTER: &str = r#"import { type AdapterCall, Answered, adapter, FetchError, type SendOutcome } from "@grida/fx";
import { JOB } from "./kites.ts";

async function send(call: AdapterCall): Promise<SendOutcome> {
  if (call.request.prompt === "refuse") {
    return { outcome: "refused", reason: "no kites today" };
  }
  let fetched = "sent";
  try {
    await call.fetch(`${call.baseUrl}/draw`, { method: "POST", json: {}, credential: true });
  } catch (error) {
    if (!(error instanceof FetchError)) {
      throw error;
    }
    fetched = `${error.phase}: ${error.reason}`;
  }
  return Answered.json({ fetched, key: "ACME_API_KEY" in process.env }, { costUsd: 0.01 });
}

export const acme = adapter({
  requests: ["structured.generate"],
  jobs: ["acme.render"],
  send,
  submit: (call) => ({ outcome: "accepted", handle: { job: `${JOB}${call.take}` } }),
  collect: async (call) => ({ outcome: "answered", data: { job: call.handle.job ?? null }, costUsd: 0.5 }),
  version: 2,
});
"#;

/// A project adapter (spec/adapters.md §9) of the real `@grida/fx`: the planning host loads it,
/// and an adapter host, started as a live run starts one, serves a send whose `fetch` the engine
/// refuses (the network is off), a refusal, and a long job submitted and collected, then ends
/// verified with the run. The twin of python_host.rs's
/// `the_real_python_host_serves_a_project_adapter`.
#[test]
fn the_real_typescript_host_serves_a_project_adapter() {
    use grida_fx_core::money::Usd;
    use grida_fx_core::providers::{
        BaseUrl, Credential, Declaration, Declarations, DeclaredAdapter, LoadedAdapter,
    };
    use grida_fx_protocol::{AdapterLoadParams, CallKind, ErrorCode, RpcError};
    use grida_fx_providers::adapter::{
        Adapter, Answer, CallRequest, Collected, RouteRef, Sent, Submitted,
    };
    use grida_fx_providers::clock::FakeClock;
    use grida_fx_providers::keys::{Environment, Keys, Secret};
    use grida_fx_providers::setup::Setup;
    use grida_fx_providers::transport::Offline;
    use grida_fx_runtime::adapter_host::{self, Launch, ProcessLaunch};

    const KEY: &str = "acme-made-up-key-90ad";
    let Some((node, entry)) = real_node_host() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap().join("acme");
    std::fs::create_dir_all(root.join("adapters")).unwrap();
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::write(root.join("package.json"), "{\"type\": \"module\"}\n").unwrap();
    std::fs::write(
        root.join("adapters/kites.ts"),
        "export const JOB = \"j-\";\n",
    )
    .unwrap();
    std::fs::write(root.join("adapters/acme.ts"), REAL_ADAPTER).unwrap();

    // Planning: the planning host loads the adapter, and goes on serving.
    let mut planning = LanguageHost::typescript()
        .with_program(node.clone())
        .with_entry(entry.clone());
    planning.open_project(&root, &[]).unwrap();
    let loaded = planning
        .load_adapter(&AdapterLoadParams {
            path: "adapters/acme.ts".into(),
            attribute: "acme".into(),
        })
        .unwrap();
    assert_eq!(loaded.adapter.version, Some(2));
    let kinds: Vec<_> = loaded.adapter.capabilities.clone().into_iter().collect();
    assert_eq!(
        kinds,
        [
            ("structured.generate".to_string(), CallKind::Request),
            ("acme.render".to_string(), CallKind::Job)
        ]
    );
    let labels: Vec<&str> = loaded.closure.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(labels, ["adapters/acme.ts", "adapters/kites.ts"]);
    let missing = planning
        .load_adapter(&AdapterLoadParams {
            path: "adapters/acme.ts".into(),
            attribute: "send".into(),
        })
        .unwrap_err();
    assert_eq!(
        missing,
        HostFailure::Rpc(RpcError::new(
            ErrorCode::LoadFailed,
            "adapters/acme.ts: send is not an export declared with adapter()"
        ))
    );
    let described = planning.describe(&targets(&["adapters/kites.ts"])).unwrap();
    assert!(
        matches!(&described.modules[0], ModuleDescription::Described { types, .. } if types.is_empty()),
        "{described:?}"
    );
    planning.shutdown().unwrap();

    // The live run: one adapter host for the declaration, with its own key.
    let declaration = Declaration {
        name: "acme".into(),
        label: "acme".into(),
        credential: Some(Credential {
            variable: "ACME_API_KEY".into(),
            header: "authorization".into(),
            scheme: "Bearer".into(),
        }),
        base_url: BaseUrl {
            variable: None,
            default: "https://api.acme.test/v1".into(),
        },
        download_hosts: Vec::new(),
        family: None,
        adapter: Some(DeclaredAdapter {
            path: "adapters/acme.ts".into(),
            attribute: "acme".into(),
            loaded: Some(LoadedAdapter {
                version: Some(2),
                capabilities: loaded.adapter.capabilities.clone(),
                source: grida_fx_core::registry::closure_digests(&loaded.closure).unwrap(),
                drift: None,
            }),
        }),
    };
    let declarations = Declarations::with(&[declaration]);
    let setup = Setup::new(
        Arc::new(Offline),
        Keys::from_environment(&Environment::declared(
            &[("ACME_API_KEY", KEY)],
            &declarations,
        )),
        Arc::new(FakeClock::new()),
    );
    let spec = HostSpec::typescript(&node, &entry, "node", &root, Vec::new())
        .unchecked()
        .with_environment(Vec::new(), vec![("ACME_API_KEY".into(), Secret::new(KEY))]);
    let launch: Arc<dyn Launch> = Arc::new(ProcessLaunch::new(Ok(spec)));
    let mut adapters = Adapters::new();
    adapter_host::register(&mut adapters, &setup, |_, _| Arc::clone(&launch));
    let route = |capability: &str| RouteRef {
        capability: capability.into(),
        model: "model-a".into(),
        provider: "acme".into(),
        contract: json!({"adapter": "adapters/acme.ts#acme", "adapter_behavior": 2}),
        limits: Default::default(),
    };
    let call = |capability: &str, prompt: &str| CallRequest {
        route: route(capability),
        request: json!({ "prompt": prompt }),
        files: IndexMap::new(),
        take: vec![1, 3],
        key: "k".repeat(64),
        attempt: 1,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let Some(Adapter::Request(sender)) = adapters.serving(&route("structured.generate")) else {
            panic!("a request adapter");
        };
        assert_eq!(
            sender.send(&call("structured.generate", "a kite")).await,
            Sent::Answered(Answer::new(
                json!({"json": {
                    "fetched": "refused: the network is off (GRIDA_FX_NETWORK=off)",
                    "key": true
                }}),
                Some(Usd(10_000))
            ))
        );
        assert_eq!(
            sender.send(&call("structured.generate", "refuse")).await,
            Sent::Refused {
                reason: "no kites today".into()
            }
        );
        let Some(Adapter::Job(job)) = adapters.serving(&route("acme.render")) else {
            panic!("a long job");
        };
        assert_eq!(
            job.submit(&call("acme.render", "a kite")).await,
            Submitted::Accepted {
                handle: json!({"job": "j-3"})
            }
        );
        assert_eq!(
            job.collect(&call("acme.render", "a kite"), &json!({"job": "j-3"}))
                .await,
            Collected::Answered(Answer::new(json!({"job": "j-3"}), Some(Usd(500_000))))
        );
        for runtime in adapters.runtimes() {
            assert!(runtime.shutdown().await, "the adapter host ends verified");
        }
    });
    runtime.shutdown_timeout(Duration::from_secs(10));
}
