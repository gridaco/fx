//! A rerun verifies its source without executing uncached plan-time steps.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn python_host() -> Option<PathBuf> {
    if let Some(python) = std::env::var_os("GRIDA_FX_PYTHON").filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(python));
    }
    let python = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/.venv/bin/python");
    if python.is_file() { Some(python) } else { None }
}

fn fx(project: &Path, python: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command.args(args).current_dir(project).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
        .env("GRIDA_FX_PYTHON", python)
        .env("GRIDA_FX_NETWORK", "off")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap()
}

#[test]
fn a_rejected_rerun_does_not_execute_a_changed_plan_time_step() {
    let Some(python) = python_host() else {
        eprintln!("skipped: no Python with the grida package");
        return;
    };
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::write(
        root.join("reel.yaml"),
        r#"fx: workflow/v1
id: reel
title: Short reel
steps:
  prepare:
    uses: ./nodes.py#prepare
    at: plan
  choose:
    uses: fx/select@1
    at: plan
    with:
      first_of:
        - ${{ steps.prepare.outputs.scenes }}
"#,
    )
    .unwrap();
    let node = root.join("nodes.py");
    std::fs::write(
        &node,
        r#"from grida.fx import Ctx, node

@node("prepare", outputs={"scenes": "json"}, version=1)
def prepare(ctx: Ctx) -> dict:
    return {"scenes": ctx.out.json({"count": 1})}
"#,
    )
    .unwrap();
    let first = fx(
        root,
        &python,
        &["run", "reel.yaml", "--name", "demo", "--no-view"],
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let repeat = ["rerun", "reel/demo", "--attempt", "1", "--no-view"];
    let cached = fx(root, &python, &repeat);
    assert!(
        cached.status.success(),
        "{}",
        String::from_utf8_lossy(&cached.stderr)
    );
    assert!(String::from_utf8_lossy(&cached.stdout).contains("attempt   2"));

    std::fs::write(
        &node,
        r#"from pathlib import Path
from grida.fx import Ctx, node

@node("prepare", outputs={"scenes": "json"}, version=2)
def prepare(ctx: Ctx) -> dict:
    Path(__file__).with_name("unexpected-render.txt").write_text("new node executed")
    return {"scenes": ctx.out.json({"count": 2})}
"#,
    )
    .unwrap();
    let refused = fx(root, &python, &repeat);
    assert_eq!(refused.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("no trusted cached at-plan result"),
        "{stderr}"
    );
    assert!(!root.join("unexpected-render.txt").exists());

    let history = fx(
        root,
        &python,
        &["inspect", "reel/demo", "--history", "--json"],
    );
    assert!(
        history.status.success(),
        "{}",
        String::from_utf8_lossy(&history.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&history.stdout).unwrap();
    assert_eq!(doc["attempts"].as_array().unwrap().len(), 2);
}
