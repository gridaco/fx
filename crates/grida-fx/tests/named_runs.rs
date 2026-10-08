//! Named runs keep logical history without changing plan or cache identity.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Project {
    root: tempfile::TempDir,
    python: PathBuf,
}

impl Project {
    fn new() -> Option<Self> {
        let python = std::env::var_os("GRIDA_FX_PYTHON")
            .map(PathBuf::from)
            .or_else(|| {
                let candidate =
                    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/.venv/bin/python");
                candidate.is_file().then_some(candidate)
            })?;
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("fx.yaml"), "fx: project/v1\n").unwrap();
        std::fs::write(
            root.path().join("workflow.yaml"),
            r#"fx: workflow/v1
id: greeting
title: Greeting
inputs:
  name: {type: string, default: Ada}
  fail: {type: boolean, default: false}
steps:
  greet:
    uses: ./node.py#greet
    with:
      name: ${{ inputs.name }}
      fail: ${{ inputs.fail }}
outputs:
  text: ${{ steps.greet.outputs.text }}
"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("node.py"),
            r#"from grida.fx import Ctx, node

@node("named_test_greet", params={"name": str, "fail": bool}, outputs={"text": "text"}, version=1)
def greet(ctx: Ctx) -> dict:
    if ctx.params["fail"]:
        raise ctx.fail("deliberate test failure")
    return {"text": ctx.out.text(f"Hello {ctx.params['name']}")}
"#,
        )
        .unwrap();
        Some(Self { root, python })
    }

    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_grida-fx"))
            .current_dir(self.root.path())
            .env_clear()
            .env("GRIDA_FX_PYTHON", &self.python)
            .env("GRIDA_FX_DISABLE_DOTENV", "1")
            .env("GRIDA_FX_NETWORK", "off")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .args(args)
            .output()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut all = vec!["run", "workflow.yaml", "--no-view", "--max-usd", "0"];
        all.extend_from_slice(args);
        self.command(&all)
    }

    fn inspect(&self, selector: &str) -> Value {
        let output = self.command(&["inspect", selector, "--json"]);
        success(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn events(&self, selector: &str) -> Vec<Value> {
        let document = self.inspect(selector);
        let folder = document["run"]["folder"].as_str().unwrap();
        std::fs::read_to_string(self.root.path().join(folder).join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn names_are_immutable_and_cache_and_plan_identity_are_unaffected() {
    let Some(project) = Project::new() else {
        return;
    };
    success(&project.run(&["--name", "baseline", "--", "--name", "Mira"]));
    let first = project.inspect("greeting/baseline");
    assert_eq!(first["run"]["name"], "baseline");
    let original = project.events("greeting/baseline");
    let original_start = original
        .iter()
        .find(|event| event["event"] == "run_started")
        .unwrap();
    assert_eq!(original_start["name"], "baseline");
    assert!(
        original_start["created_at"]
            .as_str()
            .unwrap()
            .ends_with("Z")
    );
    success(&project.run(&["--name", "copy", "--", "--name", "Mira"]));
    let copied = project.events("greeting/copy");
    assert_eq!(original[0]["plan"], copied[0]["plan"]);
    assert_eq!(
        copied
            .iter()
            .find(|event| event["event"] == "node_finished")
            .unwrap()["cache"],
        "hit"
    );
    assert_ne!(
        first["run"]["folder"],
        project.inspect("greeting/copy")["run"]["folder"]
    );
    assert_eq!(project.inspect("greeting")["run"]["name"], "copy");

    success(&project.run(&["--resume", "baseline", "--", "--name", "Mira"]));
    let resumed = project.events("greeting/baseline");
    let starts: Vec<_> = resumed
        .iter()
        .filter(|event| event["event"] == "run_started")
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0]["created_at"], starts[1]["created_at"]);
    assert_eq!(starts[1]["name"], "baseline");
    assert_eq!(starts[1]["resumed"], true);
    assert_eq!(
        project.inspect("greeting")["run"]["name"],
        "copy",
        "resume must not reorder creation history"
    );

    let folder = first["run"]["folder"].as_str().unwrap();
    success(&project.run(&["--run", folder, "--", "--name", "Mira"]));
    assert_eq!(
        project.inspect("greeting/baseline")["run"]["created_at"],
        first["run"]["created_at"]
    );
}

#[test]
fn names_are_case_sensitive_even_on_case_insensitive_disks() {
    let Some(project) = Project::new() else {
        return;
    };
    success(&project.run(&["--name", "Baseline"]));
    success(&project.run(&["--name", "baseline"]));
    let upper = project.inspect("greeting/Baseline");
    let lower = project.inspect("greeting/baseline");
    assert_ne!(upper["run"]["folder"], lower["run"]["folder"]);
    success(&project.run(&["--resume", "Baseline"]));
    success(&project.run(&["--resume", "baseline"]));
    assert_eq!(
        project
            .events("greeting/Baseline")
            .iter()
            .filter(|event| event["event"] == "run_started")
            .count(),
        2
    );
    assert_eq!(
        project
            .events("greeting/baseline")
            .iter()
            .filter(|event| event["event"] == "run_started")
            .count(),
        2
    );
}

#[test]
fn collision_missing_resume_and_changed_inputs_are_refused_without_new_invocations() {
    let Some(project) = Project::new() else {
        return;
    };
    let missing = project.run(&["--resume", "missing"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(!project.root.path().join("runs").exists());
    success(&project.run(&["--name", "baseline"]));
    let events = project.events("greeting/baseline");
    let collision = project.run(&["--name", "baseline"]);
    assert_eq!(collision.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&collision.stderr).contains("already exists"));
    assert_eq!(project.events("greeting/baseline"), events);
    let mismatch = project.run(&["--resume", "baseline", "--", "--name", "Different"]);
    assert_eq!(mismatch.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&mismatch.stdout).contains("other inputs"));
    assert_eq!(project.events("greeting/baseline"), events);
    let mut definition =
        std::fs::read_to_string(project.root.path().join("workflow.yaml")).unwrap();
    definition = definition.replace("title: Greeting", "title: Revised greeting");
    std::fs::write(project.root.path().join("workflow.yaml"), definition).unwrap();
    assert_eq!(
        project.run(&["--resume", "baseline"]).status.code(),
        Some(1)
    );
    assert_eq!(project.events("greeting/baseline"), events);
}

#[test]
fn selectors_refuse_ambiguous_sources_and_latest_includes_failed_runs() {
    let Some(project) = Project::new() else {
        return;
    };
    success(&project.run(&["--name", "success"]));
    let failed = project.run(&["--name", "failed", "--fail", "true"]);
    assert_eq!(failed.status.code(), Some(1));
    assert_eq!(project.inspect("greeting")["run"]["name"], "failed");
    assert_eq!(project.inspect("greeting")["run"]["state"], "failed");
    let explicit = project.inspect("greeting/success")["run"]["folder"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::copy(
        project.root.path().join("workflow.yaml"),
        project.root.path().join("other.yaml"),
    )
    .unwrap();
    success(&project.command(&["run", "other.yaml", "--name", "success", "--no-view"]));
    for selector in ["greeting", "greeting/success"] {
        let result = project.command(&["inspect", selector, "--json"]);
        assert_eq!(result.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&result.stderr).contains("multiple sources"));
    }
    assert_eq!(project.inspect(&explicit)["run"]["name"], "success");
}

#[test]
fn run_selectors_are_mutually_exclusive_and_names_are_validated_before_planning() {
    let Some(project) = Project::new() else {
        return;
    };
    for flags in [
        vec!["--name", "one", "--resume", "one"],
        vec!["--name", "one", "--run", "runs/one"],
        vec!["--resume", "one", "--run", "runs/one"],
        vec!["--name", "../escape"],
    ] {
        assert_eq!(project.run(&flags).status.code(), Some(2));
    }
    assert!(!project.root.path().join("runs").exists());
}
