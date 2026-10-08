//! Browser opening is opt-in, follows readiness, and cannot fail execution.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

fn python_host() -> Option<PathBuf> {
    if let Some(python) = std::env::var_os("GRIDA_FX_PYTHON").filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(python));
    }
    let python = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/.venv/bin/python");
    python.is_file().then_some(python)
}

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command
        .current_dir(root)
        .env_clear()
        .env("GRIDA_FX_DISABLE_DOTENV", "1")
        .env("GRIDA_FX_NETWORK", "off")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    command
}

fn project(python: &Path, fail_opener: bool) -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::write(
        root.join("workflow.yaml"),
        "fx: workflow/v1\nid: browser-test\ntitle: Browser test\nsteps:\n  wait:\n    uses: ./node.py#wait\noutputs:\n  done: ${{ steps.wait.outputs.text }}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("node.py"),
        r#"import time
from pathlib import Path
from grida.fx import Ctx, node

@node("browser_test_wait", outputs={"text": "text"}, version=1)
def wait(ctx: Ctx) -> dict:
    root = Path(__file__).parent
    (root / "entered").touch()
    deadline = time.monotonic() + 15
    while not (root / "release").is_file():
        if ctx.cancelled or time.monotonic() >= deadline:
            raise ctx.fail("browser test gate did not release")
        time.sleep(0.01)
    return {"text": ctx.out.text("done")}
"#,
    )
    .unwrap();
    fake_opener(root, python, "api/snapshot", fail_opener);
    folder
}

fn fake_opener(root: &Path, python: &Path, endpoint: &str, fail_opener: bool) {
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let opener = bin.join(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    // This replaces the platform launcher, including in environments with a real browser.
    let source = format!(
        r#"#!{}
import json
from pathlib import Path
import sys
from urllib.request import urlopen

root = Path(__file__).parent.parent
(root / "invoked").touch()
url = sys.argv[1]
with urlopen(url + "{}", timeout=5) as response:
    snapshot = json.load(response)
receipt = {{"url": url, "kind": snapshot["kind"], "events": [event["event"] for event in snapshot.get("events", [])]}}
temporary = root / "opened.tmp"
temporary.write_text(json.dumps(receipt))
temporary.replace(root / "opened.json")
sys.exit({})
"#,
        python.display(),
        endpoint,
        u8::from(fail_opener)
    );
    std::fs::write(&opener, source).unwrap();
    std::fs::set_permissions(opener, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Running {
    child: Child,
    output: Receiver<String>,
    errors: Receiver<String>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn lines(reader: impl std::io::Read + Send + 'static) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            let _ = sender.send(line.unwrap());
        }
    });
    receiver
}

fn start(root: &Path, python: &Path, open: bool) -> Running {
    let mut command = command(root);
    command
        .env("GRIDA_FX_PYTHON", python)
        .env("PATH", root.join("bin"))
        .args([
            "run",
            "workflow.yaml",
            "--standalone",
            "--run",
            "run",
            "--max-usd",
            "0",
        ]);
    if open {
        command.arg("--open");
    }
    spawn(command)
}

fn spawn(mut command: Command) -> Running {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let output = lines(child.stdout.take().unwrap());
    let errors = lines(child.stderr.take().unwrap());
    Running {
        child,
        output,
        errors,
    }
}

fn await_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "{} was not written",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn run_opens_only_when_requested_and_launcher_failure_does_not_fail_execution() {
    let Some(python) = python_host() else {
        eprintln!("skipped: no Python with the grida package");
        return;
    };
    // A gated node keeps the server alive long enough to prove the launcher can read it.
    for (open, fail_opener) in [(false, false), (true, false), (true, true)] {
        let folder = project(&python, fail_opener);
        let root = folder.path();
        let mut running = start(root, &python, open);
        let deadline = Instant::now() + Duration::from_secs(10);
        let url = loop {
            let line = running
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| {
                    panic!(
                        "viewer URL was not printed: {error}; {}",
                        running.errors.try_iter().collect::<Vec<_>>().join("\n")
                    )
                });
            if let Some(url) = line.strip_prefix("view      ") {
                break url.to_string();
            }
        };
        assert!(url.starts_with("http://127.0.0.1:"));
        await_file(&root.join("entered"));
        let invocation = running.errors.recv_timeout(Duration::from_secs(5)).unwrap();
        let id = invocation.strip_prefix("invocation ").unwrap();
        assert_eq!(id.len(), 16);
        assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let control = running.errors.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            control.as_str(),
            "control   available" | "control   unavailable"
        ));
        if open {
            await_file(&root.join("opened.json"));
            let receipt: serde_json::Value =
                serde_json::from_slice(&std::fs::read(root.join("opened.json")).unwrap()).unwrap();
            assert_eq!(receipt["url"], url);
            assert_eq!(receipt["kind"], "fx-run-snapshot-v1");
            assert!(
                receipt["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|event| event == "run_started")
            );
            if fail_opener {
                let warning = running.errors.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(warning.contains("could not open a browser"), "{warning}");
            }
        }
        std::fs::write(root.join("release"), "").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = running.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "browser mode {open}/{fail_opener}: {status}"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "run did not finish after release"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = running.output.iter().collect::<Vec<_>>().join("\n");
        assert!(output.contains("result    ok   spent $0.00"), "{output}");
        if !open {
            assert!(!root.join("opened.json").exists());
        }
    }
}

#[test]
fn open_conflicts_with_no_view_before_planning_or_creating_a_run() {
    let folder = tempfile::tempdir().unwrap();
    let output = command(folder.path())
        .args(["run", "missing.yaml", "--open", "--no-view", "--run", "run"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("--open") && error.contains("--no-view"),
        "{error}"
    );
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
}

fn plan_project(python: &Path) -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(
        folder.path().join("plan.json"),
        serde_json::json!({
            "kind": "fx-graph-v1",
            "workflow": {"id": "browser-test", "title": "Browser test"},
            "instances": [], "steps": {}, "inputs": {}, "pending": [],
            "estimate": {"low_usd": 0, "high_usd": 0, "ceiling_usd": 0}
        })
        .to_string(),
    )
    .unwrap();
    fake_opener(folder.path(), python, "api/view", false);
    folder
}

fn graph(url: &str) -> serde_json::Value {
    let address = url.strip_prefix("http://").unwrap().trim_end_matches('/');
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET /api/view HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[test]
fn view_opens_only_when_requested_and_remains_a_foreground_server() {
    let Some(python) = python_host() else {
        eprintln!("skipped: no Python with the grida package");
        return;
    };
    for flag in [None, Some("--no-open"), Some("--open")] {
        let folder = plan_project(&python);
        let root = folder.path();
        let mut command = command(root);
        command
            .env("PATH", root.join("bin"))
            // A saved plan must never need an author-code host, even to open its browser.
            .env("GRIDA_FX_PYTHON", root.join("missing-python"))
            .args(["view", "--plan", "plan.json"]);
        if let Some(flag) = flag {
            command.arg(flag);
        }
        let mut viewer = spawn(command);
        let url = viewer
            .output
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|error| {
                panic!(
                    "viewer URL was not printed for {flag:?}: {error}; {}",
                    viewer.errors.try_iter().collect::<Vec<_>>().join("\n")
                )
            });
        assert!(url.starts_with("http://127.0.0.1:"), "{url}");
        assert_eq!(graph(&url)["workflow"]["id"], "browser-test");
        if flag == Some("--open") {
            await_file(&root.join("opened.json"));
            let receipt: serde_json::Value =
                serde_json::from_slice(&std::fs::read(root.join("opened.json")).unwrap()).unwrap();
            assert_eq!(receipt["url"], url);
            assert_eq!(receipt["kind"], "fx-graph-v1");
        } else {
            // Give an accidentally launched background opener time to expose its invocation.
            std::thread::sleep(Duration::from_millis(250));
            assert!(!root.join("invoked").exists());
        }
        assert!(viewer.child.try_wait().unwrap().is_none());
        assert_eq!(graph(&url)["kind"], "fx-graph-v1");
        let interrupted = Command::new("kill")
            .args(["-INT", &viewer.child.id().to_string()])
            .status()
            .unwrap();
        assert!(interrupted.success());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = viewer.child.try_wait().unwrap() {
                assert_eq!(status.code(), Some(130));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "viewer did not stop after SIGINT"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let address = url.strip_prefix("http://").unwrap().trim_end_matches('/');
        assert!(TcpStream::connect(address).is_err());
        if flag != Some("--open") {
            assert!(!root.join("invoked").exists());
        }
    }
}

#[test]
fn view_open_conflicts_with_no_open_before_reading_the_plan_or_serving() {
    let folder = tempfile::tempdir().unwrap();
    let output = command(folder.path())
        .args(["view", "--plan", "missing.json", "--open", "--no-open"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("--open") && error.contains("--no-open"),
        "{error}"
    );
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
}
