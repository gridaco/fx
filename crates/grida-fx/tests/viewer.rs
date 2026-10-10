//! The viewer is an independent foreground process serving one offline plan or run.

use serde_json::json;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct Viewer {
    child: Child,
    url: String,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn command(run: &Path) -> Command {
    let mut command = base_command(run.parent().unwrap());
    command.args(["view", "--run"]).arg(run).arg("--no-open");
    command
}

fn base_command(cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command
        .current_dir(cwd)
        .env_clear()
        .env("GRIDA_FX_DISABLE_DOTENV", "1")
        .env("GRIDA_FX_NETWORK", "off")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
}

fn run_folder() -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    let run = folder.path().join("run");
    std::fs::create_dir(&run).unwrap();
    let plan = json!({
        "kind": "fx-graph-v1",
        "workflow": {"id": "example", "title": "Example"},
        "instances": [], "steps": {}, "inputs": {}, "pending": [],
        "estimate": {"low_usd": 0, "high_usd": 0}
    });
    std::fs::write(run.join("plan.json"), plan.to_string()).unwrap();
    std::fs::write(run.join("events.jsonl"), "").unwrap();
    folder
}

fn start(run: &Path) -> Viewer {
    start_command(command(run))
}

fn start_command(mut command: Command) -> Viewer {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let output = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(output).lines();
        let _ = sender.send(lines.next().unwrap().unwrap());
        for _ in lines {}
    });
    let mut viewer = Viewer {
        child,
        url: String::new(),
    };
    viewer.url = receiver.recv_timeout(Duration::from_secs(15)).unwrap();
    assert!(viewer.url.starts_with("http://127.0.0.1:"));
    assert!(viewer.child.try_wait().unwrap().is_none());
    viewer
}

fn graph(viewer: &Viewer) -> serde_json::Value {
    let response = get(&viewer.url, "/api/view");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

fn plan_project() -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("fx.yaml"),
        "fx: project/v1\nroutes:\n  image.generate: img-a@acme\n  image.edit: edit-a@acme\nroute_tables: [routes.yaml]\n").unwrap();
    std::fs::write(folder.path().join("routes.yaml"),
        "fx: routes/v1\nroutes:\n  - { capability: image.generate, route: img-a@acme, price: { low_usd: 0.01, high_usd: 0.04 } }\n  - { capability: image.edit, route: edit-a@acme, price: { low_usd: 0.02, high_usd: 0.05 } }\n").unwrap();
    std::fs::write(folder.path().join("workflow.yaml"),
        "fx: workflow/v1\nid: preview\ntitle: Preview\ninputs:\n  caption: { type: string }\nsteps:\n  draw:\n    uses: fx/image.generate@1\n    with: { prompt: '${{ inputs.caption }}' }\n  refine:\n    uses: fx/image.edit@1\n    with: { image: '${{ steps.draw.outputs.image }}', prompt: Make the edges softer. }\noutputs:\n  image: ${{ steps.refine.outputs.image }}\n").unwrap();
    folder
}

#[test]
fn source_file_and_id_use_the_existing_offline_plan_without_a_run() {
    let folder = plan_project();
    let root = folder.path();
    let mut planned = base_command(root);
    let output = planned
        .args([
            "plan",
            "workflow.yaml",
            "--json",
            "--caption",
            "A copper lantern.",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(printed.get("steps").is_none() && printed.get("takes_file").is_none());
    // The served plan is the printed graph with the members a run's plan.json gives readers:
    // every declared step with its declaration order, and the takes file.
    let mut expected = printed.clone();
    expected["steps"] = json!({
        "draw": {"title": null, "description": null, "uses": "fx/image.generate@1", "view": false, "order": 0},
        "refine": {"title": null, "description": null, "uses": "fx/image.edit@1", "view": false, "order": 1},
    });
    expected["takes_file"] = json!("preview.takes.yaml");
    for target in ["workflow.yaml", "preview"] {
        let mut command = base_command(root);
        command.args([
            "view",
            target,
            "--caption",
            "A copper lantern.",
            "--no-open",
        ]);
        let viewer = start_command(command);
        assert_eq!(graph(&viewer), expected);
        assert!(get(&viewer.url, "/api/run").starts_with("HTTP/1.1 404"));
        assert!(!root.join("runs").exists());
    }
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    let viewer = start_command({
        let mut command = base_command(root);
        command.args([
            "view",
            "preview",
            "--caption",
            "A copper lantern.",
            "--routes",
            "routes.yaml",
            "--no-open",
        ]);
        command
    });
    assert!(!graph(&viewer)["problems"].as_array().unwrap().is_empty());
    assert!(!root.join("runs").exists());
}

#[test]
fn saved_plan_is_a_snapshot_and_rejects_planning_or_conflicting_options() {
    let folder = plan_project();
    let root = folder.path();
    let output = base_command(root)
        .args(["expand", "workflow.yaml", "--caption", "A copper lantern."])
        .output()
        .unwrap();
    assert!(output.status.success());
    std::fs::write(root.join("graph.json"), &output.stdout).unwrap();
    let expected: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let viewer = start_command({
        let mut command = base_command(root);
        command.args(["view", "--plan", "graph.json", "--no-open"]);
        command
    });
    assert_eq!(graph(&viewer), expected);
    std::fs::write(root.join("graph.json"), "invalid now").unwrap();
    assert_eq!(graph(&viewer), expected);
    for args in [
        vec!["view"],
        vec!["view", "workflow.yaml", "--plan", "graph.json"],
        vec!["view", "--run", "missing", "--plan", "graph.json"],
        vec!["view", "--plan", "graph.json", "--inputs", "inputs.yaml"],
        vec!["view", "--plan", "graph.json", "--arg", "theme=dusk"],
        vec!["view", "--plan", "graph.json", "--caption", "ignored"],
        vec!["view", "--run", "missing", "--unknown-flag"],
    ] {
        let output = base_command(root).args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

fn python_host() -> Option<PathBuf> {
    if let Some(python) = std::env::var_os("GRIDA_FX_PYTHON").filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(python));
    }
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let venv = repository.join("python/.venv/bin/python");
    venv.is_file().then_some(venv)
}

#[test]
fn python_builder_materializes_the_same_graph_once_before_serving() {
    let Some(python) = python_host() else {
        eprintln!("skipped: no Python with the grida package");
        return;
    };
    let folder = plan_project();
    let root = folder.path();
    std::fs::write(root.join("assets.py"), r#"from pathlib import Path
from grida.fx import Workflow

def build(theme):
    marker = Path("build-count.txt")
    count = int(marker.read_text()) if marker.exists() else 0
    marker.write_text(str(count + 1))
    workflow = Workflow("preview", title="Preview")
    draw = workflow.step("draw", uses="fx/image.generate@1", with_={"prompt": theme})
    refine = workflow.step("refine", uses="fx/image.edit@1", with_={"image": draw.outputs.image, "prompt": "Make the edges softer."})
    workflow.outputs(image=refine.outputs.image)
    return workflow
"#).unwrap();
    let mut planned = base_command(root);
    let output = planned
        .env("GRIDA_FX_PYTHON", &python)
        .args(["expand", "assets.py:build", "--arg", "theme=dusk"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut expected: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // A builder's steps are declared by the document it returns; its takes file lies by its
    // takes anchor.
    expected["steps"] = json!({
        "draw": {"title": null, "description": null, "uses": "fx/image.generate@1", "view": false, "order": 0},
        "refine": {"title": null, "description": null, "uses": "fx/image.edit@1", "view": false, "order": 1},
    });
    expected["takes_file"] = json!("preview.takes.yaml");
    std::fs::remove_file(root.join("build-count.txt")).unwrap();
    let mut command = base_command(root);
    command.env("GRIDA_FX_PYTHON", python).args([
        "view",
        "assets.py:build",
        "--arg",
        "theme=dusk",
        "--no-open",
    ]);
    let viewer = start_command(command);
    assert_eq!(graph(&viewer), expected);
    assert_eq!(graph(&viewer), expected);
    assert_eq!(
        std::fs::read_to_string(root.join("build-count.txt")).unwrap(),
        "1"
    );
    assert!(!root.join("runs").exists());
}

fn get(url: &str, path: &str) -> String {
    let address = url.strip_prefix("http://").unwrap().trim_end_matches('/');
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn independent_viewers_serve_the_embedded_client_and_do_not_write_the_run() {
    let folder = run_folder();
    let run = folder.path().join("run");
    let original = std::fs::read(run.join("plan.json")).unwrap();
    let first = start(&run);
    let second = start(&run);
    assert_ne!(first.url, second.url);
    let page = get(&first.url, "/");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("<script"), "{page}");
    let data = get(&first.url, "/api/run");
    assert!(data.starts_with("HTTP/1.1 200"), "{data}");
    assert!(data.contains("fx-viewer-run-v1"), "{data}");
    drop(first);
    let second_data = get(&second.url, "/api/run");
    assert!(second_data.starts_with("HTTP/1.1 200"), "{second_data}");
    assert_eq!(std::fs::read(run.join("plan.json")).unwrap(), original);
    assert_eq!(std::fs::read(run.join("events.jsonl")).unwrap(), b"");
    assert_eq!(std::fs::read_dir(&run).unwrap().count(), 2);
}

#[test]
fn a_missing_run_or_an_occupied_port_is_an_error() {
    let folder = run_folder();
    let run = folder.path().join("run");
    let missing = command(&folder.path().join("absent")).output().unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
    let first = start(&run);
    let port = first.url.trim_end_matches('/').rsplit(':').next().unwrap();
    let busy = command(&run).args(["--port", port]).output().unwrap();
    assert_eq!(busy.status.code(), Some(2));
    assert!(busy.stdout.is_empty());
    assert!(get(&first.url, "/api/run").starts_with("HTTP/1.1 200"));
}

#[cfg(unix)]
#[test]
fn an_interrupt_stops_only_the_viewer_and_exits_130() {
    let folder = run_folder();
    let run = folder.path().join("run");
    let mut viewer = start(&run);
    let sent = Command::new("kill")
        .args(["-INT", &viewer.child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = viewer.child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(130));
            break;
        }
        assert!(std::time::Instant::now() < deadline, "viewer did not stop");
        std::thread::sleep(Duration::from_millis(20));
    }
    let address = viewer
        .url
        .strip_prefix("http://")
        .unwrap()
        .trim_end_matches('/');
    assert!(TcpStream::connect(address).is_err());
}
