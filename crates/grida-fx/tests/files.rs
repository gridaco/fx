//! File discovery/resolution uses recorded evidence and never loads author code.
use grida_fx_core::value::file_digest;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn command(cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command
        .current_dir(cwd)
        .env_clear()
        .env("GRIDA_FX_NETWORK", "off")
        .env("GRIDA_FX_DISABLE_DOTENV", "1");
    command
}

fn document(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn fixture() -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("fx.yaml"), "fx: project/v1\n").unwrap();
    fs::write(
        project.path().join("node.py"),
        "raise RuntimeError('never import this file for inspection')\n",
    )
    .unwrap();
    fs::write(
        project.path().join("workflow.yaml"),
        "fx: workflow/v1\nid: sample\n",
    )
    .unwrap();
    let source = fs::read(project.path().join("node.py")).unwrap();
    let bytes = b"synthetic recorded bytes\n";
    let digest = file_digest(bytes);
    let run = project.path().join("run");
    fs::create_dir(&run).unwrap();
    let graph = json!({
        "kind":"fx-graph-v1","workflow":{"id":"sample","title":"Sample","file":"workflow.yaml"},
        "instances":[{"id":"render#1","path":"render","step":"render","uses":"./node.py#render","state":"planned","with":{},"reads":[]}],
        "types":{"./node.py#render":{"identity":"node.py#render@1","source":{"files":{"node.py":file_digest(&source)},"resources":{}}}},
        "steps":{},"inputs":{},"estimate":{"low_usd":0,"high_usd":0},"pending":[],
    });
    fs::write(run.join("plan.json"), graph.to_string()).unwrap();
    let events = [
        json!({"event":"node_started","id":"render#1","path":"render","uses":"./node.py#render","with":{},"reads":[]}),
        json!({"event":"node_finished","id":"render#1","path":"render","outputs":{"document":{"file":{"digest":digest,"kind":"text/plain","name":"document.txt","size":bytes.len()}}},"cache":"miss","duration_ms":0}),
    ];
    let text: String = events
        .into_iter()
        .map(|mut event| {
            event["kind"] = json!("fx-run-events-v1");
            event["plan"] = json!("a".repeat(64));
            event["invocation_id"] = json!("synthetic-files-fixture");
            event["offset_ms"] = json!(0);
            format!("{event}\n")
        })
        .collect();
    fs::write(run.join("events.jsonl"), text).unwrap();
    let relative =
        grida_fx_runtime::folder::step_file_path("render", "document", None, "text/plain");
    let output = run.join(relative);
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    fs::write(output, bytes).unwrap();
    project
}

fn captured(target: Value) -> Value {
    json!({"subject":{"kind":"run","workflow":{"id":"sample"}},"target":target})
}

#[test]
fn native_cli_lists_portable_targets_and_resolves_verified_bytes() {
    let project = fixture();
    let output = command(project.path())
        .args(["files", "list", "run", "--instance", "render#1", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let inventory = document(&output);
    assert_eq!(inventory["kind"], "fx-files-v1");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(project.path().to_str().unwrap()));
    let target = inventory["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["role"] == "artifact")
        .unwrap()["target"]
        .clone();
    let reference = captured(target).to_string();
    let resolved = command(project.path())
        .args([
            "files",
            "resolve",
            "run",
            "--reference",
            &reference,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(resolved.status.success(), "{resolved:?}");
    let result = document(&resolved);
    let path = Path::new(result["local_path"].as_str().unwrap());
    assert!(path.is_absolute());
    assert_eq!(fs::read(path).unwrap(), b"synthetic recorded bytes\n");
    fs::write(path, "tampered").unwrap();
    let missing = command(project.path())
        .args([
            "files",
            "resolve",
            "run",
            "--reference",
            &reference,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(missing.status.success());
    assert_eq!(document(&missing)["local_path"], Value::Null);
}

#[test]
fn native_cli_source_resolution_requires_explicit_binding_and_accepts_changed_code() {
    let project = fixture();
    let list = command(project.path())
        .args(["files", "list", "run", "--json"])
        .output()
        .unwrap();
    let inventory = document(&list);
    let target = inventory["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["role"] == "node")
        .unwrap()["target"]
        .clone();
    let reference = captured(target).to_string();
    let unbound = command(project.path())
        .args([
            "files",
            "resolve",
            "run",
            "--reference",
            &reference,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(unbound.status.success());
    assert_eq!(document(&unbound)["local_path"], Value::Null);
    fs::write(
        project.path().join("node.py"),
        "raise RuntimeError('changed source must not run either')\n",
    )
    .unwrap();
    let found = command(project.path())
        .args([
            "files",
            "resolve",
            "run",
            "--reference",
            &reference,
            "--project",
            ".",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(found.status.success(), "{found:?}");
    let result = document(&found);
    assert_eq!(result["source_match"], "changed");
    assert!(result["local_path"].as_str().unwrap().ends_with("node.py"));
}

#[test]
fn native_cli_refusals_are_structured_and_do_not_echo_private_paths() {
    let project = fixture();
    for arguments in [
        vec!["files", "resolve", "run", "--reference", "{", "--json"],
        vec!["files", "list", "run", "--instance", "missing#1", "--json"],
    ] {
        let output = command(project.path()).args(arguments).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(document(&output)["kind"], "fx-file-error-v1");
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains(project.path().to_str().unwrap())
        );
    }
}
