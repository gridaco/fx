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
        "kind":"fx-graph-v1","plan":"a".repeat(64),"workflow":{"id":"sample","title":"Sample","file":"workflow.yaml"},
        "instances":[{
            "id":"render#1", "path":"render", "step":"render", "take":[1],
            "uses":"./node.py#render", "type":"node.py#render@1", "state":"planned",
            "with":{}, "routes":{}, "identity":null, "phase":1, "key":null,
            "judges":null, "judged_by":[], "waiting_on":[], "needs":[], "reads":[],
            "price":{"low_usd":0,"high_usd":0},
        }],
        "types":{"./node.py#render":{"identity":"node.py#render@1","source":{"files":{"node.py":file_digest(&source)},"resources":{}}}},
        "steps":{},"inputs":{},"estimate":{"low_usd":0,"high_usd":0,"ceiling_usd":null},"pending":[],
    });
    fs::write(run.join("plan.json"), graph.to_string()).unwrap();
    let events = [
        json!({"event":"run_started"}),
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
        vec![
            "files",
            "resolve",
            "--project",
            ".",
            "--reference",
            r#"{"subject":{"kind":"run","workflow":{"id":"sample"}},"target":{"kind":"source","source":"workflow.yaml"}}"#,
            "--json",
        ],
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

#[test]
fn a_registered_run_resolves_only_in_its_exact_project_even_while_stopped() {
    let project = fixture();
    let catalog =
        grida_fx_viewer::service::Catalog::open(project.path(), Path::new("runs")).unwrap();
    let run = project.path().join("run");
    let entry = catalog.register_run(&run).unwrap();
    let inventory = grida_fx_viewer::files::list(&run, None).unwrap();
    let target = inventory["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["role"] == "artifact")
        .unwrap()["target"]
        .clone();
    let mut reference = captured(target);
    reference["subject"]["reference"] = json!({
        "kind": "service_entry",
        "project_id": catalog.project_id(),
        "entry_kind": "run",
        "entry_id": entry.id,
    });
    let resolve = |root: &Path, reference: &Value| {
        command(project.path())
            .args(["files", "resolve", "--project"])
            .arg(root)
            .args(["--reference", &reference.to_string(), "--json"])
            .output()
            .unwrap()
    };
    let found = resolve(project.path(), &reference);
    assert!(found.status.success(), "{found:?}");
    let result = document(&found);
    let compatibility = command(project.path())
        .args([
            "files",
            "resolve",
            "--service-entry",
            "--project",
            ".",
            "--reference",
            &reference.to_string(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(compatibility.status.success(), "{compatibility:?}");
    assert_eq!(document(&compatibility), result);
    let missing_binding = command(project.path())
        .args(["files", "resolve", "--reference", &reference.to_string()])
        .output()
        .unwrap();
    assert_eq!(missing_binding.status.code(), Some(2));
    let local_path = result["local_path"].as_str().unwrap();
    assert_eq!(fs::read(local_path).unwrap(), b"synthetic recorded bytes\n");
    fs::remove_file(local_path).unwrap();
    let missing = resolve(project.path(), &reference);
    assert!(missing.status.success(), "{missing:?}");
    assert!(document(&missing)["local_path"].is_null());

    let descendant = project.path().join("descendant");
    fs::create_dir(&descendant).unwrap();
    let wrong_project = resolve(&descendant, &reference);
    assert_eq!(wrong_project.status.code(), Some(2));
    assert_eq!(document(&wrong_project)["code"], "stale_subject");
    assert!(!descendant.join(".fx").exists());

    let mut undeclared = reference.clone();
    undeclared["target"]["artifact"]["digest"] = json!("b".repeat(64));
    let unknown = resolve(project.path(), &undeclared);
    assert_eq!(unknown.status.code(), Some(2));
    assert_eq!(document(&unknown)["code"], "not_found");

    let mut wrong_kind = reference.clone();
    wrong_kind["subject"]["reference"]["entry_kind"] = json!("plan");
    let mismatch = resolve(project.path(), &wrong_kind);
    assert_eq!(mismatch.status.code(), Some(2));
    assert_eq!(document(&mismatch)["code"], "invalid_request");

    let events = run.join("events.jsonl");
    let replacement = fs::read_to_string(&events).unwrap().replacen(
        "synthetic-files-fixture",
        "replacement-invocation",
        1,
    );
    fs::write(events, replacement).unwrap();
    let stale = resolve(project.path(), &reference);
    assert_eq!(stale.status.code(), Some(2));
    assert_eq!(document(&stale)["code"], "stale_subject");
}
