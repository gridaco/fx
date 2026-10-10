//! The installed CLI path connects ordinary provider-free execution to a persistent service.
//! Gates prove lifetimes without timing assumptions, and a fake opener never launches a browser.
#![cfg(unix)]

use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

fn python() -> Option<PathBuf> {
    std::env::var_os("GRIDA_FX_PYTHON")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/.venv/bin/python");
            path.is_file().then_some(path)
        })
}

struct Project {
    folder: tempfile::TempDir,
    python: PathBuf,
}
impl Project {
    fn new() -> Option<Self> {
        let Some(python) = python() else {
            eprintln!("skipped: no Python with the grida package");
            return None;
        };
        let folder = tempfile::tempdir().unwrap();
        let project = Self { folder, python };
        let result = project.command().args(["init", "--json"]).output().unwrap();
        assert!(result.status.success());
        std::fs::write(
            project.root().join("workflow.yaml"),
            r#"fx: workflow/v1
id: service-test
title: Service test
inputs:
  token: {type: string, default: one}
steps:
  wait:
    uses: ./node.py#wait
    with: {token: '${{ inputs.token }}'}
outputs:
  done: ${{ steps.wait.outputs.text }}
"#,
        )
        .unwrap();
        std::fs::write(
            project.root().join("node.py"),
            r#"import time
from pathlib import Path
from grida.fx import Ctx, node

@node("service_test_wait", params={"token": str}, outputs={"text": "text"}, version=1)
def wait(ctx: Ctx) -> dict:
    root = Path(__file__).parent
    token = ctx.params["token"]
    (root / f"entered-{token}").touch()
    deadline = time.monotonic() + 20
    while not (root / f"release-{token}").is_file():
        if ctx.cancelled or time.monotonic() >= deadline:
            raise ctx.fail("service test gate did not release")
        time.sleep(0.01)
    text = "x" * (16 * 1024 * 1024) if token == "large" else f"completed {token}"
    return {"text": ctx.out.text(text)}
"#,
        )
        .unwrap();
        let bin = project.root().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let opener = bin.join(if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        });
        std::fs::write(
            &opener,
            format!(
                r#"#!{}
import json
from pathlib import Path
import sys
from urllib.request import urlopen
root = Path(__file__).parent.parent
url = sys.argv[1]
with urlopen(url + "api/view", timeout=5) as response:
    view = json.load(response)
temporary = root / "opened.tmp"
receipt = {{"url": url, "kind": view["kind"]}}
receipt.update({{name: view[name] for name in ("steps", "takes_file") if name in view}})
temporary.write_text(json.dumps(receipt))
temporary.replace(root / "opened.json")
"#,
                project.python.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(opener, std::fs::Permissions::from_mode(0o755)).unwrap();
        Some(project)
    }
    fn root(&self) -> &Path {
        self.folder.path()
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
        command
            .current_dir(self.root())
            .env_clear()
            .env("GRIDA_FX_PYTHON", &self.python)
            .env("GRIDA_FX_DISABLE_DOTENV", "1")
            .env("GRIDA_FX_NETWORK", "off")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("PATH", self.root().join("bin"));
        command
    }
    fn start(&self) -> Value {
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        json_success(
            self.command()
                .args([
                    "start",
                    "--background",
                    "--port",
                    &port.to_string(),
                    "--json",
                ])
                .output()
                .unwrap(),
        )
    }
    fn stop(&self) {
        json_success(self.command().args(["stop", "--json"]).output().unwrap());
    }
    fn release(&self, token: &str) {
        std::fs::write(self.root().join(format!("release-{token}")), "").unwrap();
    }
    fn run(&self, token: &str, standalone: bool, open: bool) -> Running {
        let mut command = self.command();
        command.args([
            "run",
            "workflow.yaml",
            "--token",
            token,
            "--run",
            &format!("runs/{token}"),
            "--max-usd",
            "0",
        ]);
        if standalone {
            command.arg("--standalone");
        }
        if open {
            command.arg("--open");
        }
        Running::spawn(command)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = self.command().args(["stop", "--json"]).output();
    }
}

fn json_success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

struct Running {
    child: Child,
    output: Receiver<String>,
    errors: Receiver<String>,
}
impl Running {
    fn spawn(mut command: Command) -> Self {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = lines(child.stdout.take().unwrap());
        let errors = lines(child.stderr.take().unwrap());
        Self {
            child,
            output,
            errors,
        }
    }
    fn url(&self) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let line = self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| {
                    panic!(
                        "no run URL: {error}; {}",
                        self.errors.try_iter().collect::<Vec<_>>().join("\n")
                    )
                });
            if let Some(url) = line.strip_prefix("view      ") {
                return url.into();
            }
        }
    }
    fn finish(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "{}",
                    self.errors.try_iter().collect::<Vec<_>>().join("\n")
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "run did not finish after release"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = self.output.iter().collect::<Vec<_>>().join("\n");
        assert!(output.contains("result    ok   spent $0.00"), "{output}");
        output
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn lines(reader: impl Read + Send + 'static) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            let _ = sender.send(line.unwrap());
        }
    });
    receiver
}
fn await_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "{} was not created",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn get(url: &str) -> Vec<u8> {
    let (address, path) = url
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET /{path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    assert!(
        response.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    let boundary = response
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .unwrap()
        + 4;
    response[boundary..].to_vec()
}
fn get_json(url: &str) -> Value {
    serde_json::from_slice(&get(url)).unwrap()
}

#[test]
fn a_run_stays_inspectable_after_completion_and_service_restart() {
    let Some(project) = Project::new() else {
        return;
    };
    let service = project.start();
    let base = service["url"].as_str().unwrap();
    let mut run = project.run("one", false, true);
    let url = run.url();
    assert!(url.starts_with(&format!("{base}p/")) && url.contains("/runs/"));
    await_file(&project.root().join("entered-one"));
    assert_eq!(
        get_json(&format!("{url}api/snapshot"))["view"]["state"],
        "unfinished"
    );
    await_file(&project.root().join("opened.json"));
    let opened: Value =
        serde_json::from_slice(&std::fs::read(project.root().join("opened.json")).unwrap())
            .unwrap();
    assert_eq!(opened["url"], url);
    project.release("one");
    run.finish();
    let completed = get_json(&format!("{url}api/view"));
    assert_eq!(completed["state"], "succeeded");
    assert_eq!(completed["charged_usd"].as_f64(), Some(0.0));
    let artifact = completed["artifacts"].as_array().unwrap().first().unwrap();
    let artifact_url = format!(
        "{}{}",
        base.trim_end_matches('/'),
        artifact["url"].as_str().unwrap()
    );
    assert_eq!(get(&artifact_url), b"completed one");
    project.stop();
    let restarted = json_success(
        project
            .command()
            .args(["start", "--background", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(restarted["url"], service["url"]);
    assert_eq!(get_json(&format!("{url}api/view"))["state"], "succeeded");
    assert_eq!(get(&artifact_url), b"completed one");
    let inspection = project
        .command()
        .args(["inspect", "runs/one", "--open"])
        .output()
        .unwrap();
    assert!(inspection.status.success());
    assert!(String::from_utf8_lossy(&inspection.stdout).contains(&url));
}

#[test]
fn stopping_the_service_during_execution_does_not_cancel_the_run() {
    let Some(project) = Project::new() else {
        return;
    };
    let service = project.start();
    let mut run = project.run("two", false, false);
    let url = run.url();
    await_file(&project.root().join("entered-two"));
    project.stop();
    assert!(run.child.try_wait().unwrap().is_none());
    project.release("two");
    run.finish();
    let summary = json_success(
        project
            .command()
            .args(["inspect", "runs/two", "--verify", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(summary["run"]["state"], "succeeded");
    let restarted = json_success(
        project
            .command()
            .args(["start", "--background", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(restarted["url"], service["url"]);
    assert_eq!(get_json(&format!("{url}api/view"))["state"], "succeeded");
}

#[test]
fn absent_service_open_warns_but_execution_succeeds_and_registers_for_later() {
    let Some(project) = Project::new() else {
        return;
    };
    let mut run = project.run("three", false, true);
    await_file(&project.root().join("entered-three"));
    project.release("three");
    run.finish();
    let warnings = run.errors.iter().collect::<Vec<_>>().join("\n");
    assert!(
        warnings.contains("grida-fx start --background"),
        "{warnings}"
    );
    assert!(!project.root().join("opened.json").exists());
    let service = project.start();
    let index = get_json(&format!("{}api/catalog", service["url"].as_str().unwrap()));
    assert_eq!(index["entries"].as_array().unwrap().len(), 1);
    assert_eq!(index["entries"][0]["state"], "succeeded");
}

#[test]
fn opening_a_plan_registers_its_graph_without_executing_a_run_node() {
    let Some(project) = Project::new() else {
        return;
    };
    let service = project.start();
    let output = project
        .command()
        .args(["plan", "workflow.yaml", "--open", "--max-usd", "0"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    await_file(&project.root().join("opened.json"));
    let opened: Value =
        serde_json::from_slice(&std::fs::read(project.root().join("opened.json")).unwrap())
            .unwrap();
    assert_eq!(opened["kind"], "fx-graph-v1");
    assert!(opened["url"].as_str().unwrap().contains("/plans/"));
    // The registered plan carries what a run's plan.json gives its readers.
    assert_eq!(opened["takes_file"], "service-test.takes.yaml");
    assert_eq!(
        opened["steps"],
        serde_json::json!({"wait": {
            "title": null, "description": null, "uses": "./node.py#wait", "view": false, "order": 0,
        }})
    );
    assert!(!project.root().join("entered-one").exists());
    assert!(!project.root().join("runs").exists());
    let index = get_json(&format!("{}api/catalog", service["url"].as_str().unwrap()));
    assert_eq!(index["entries"].as_array().unwrap().len(), 1);
    assert_eq!(index["entries"][0]["kind"], "plan");
}

#[test]
fn standalone_viewer_lifetime_is_independent_of_the_existing_project_service() {
    let Some(project) = Project::new() else {
        return;
    };
    let service = project.start();
    let mut run = project.run("four", true, false);
    let url = run.url();
    assert_ne!(url, service["url"]);
    assert!(!url.contains("/p/"));
    await_file(&project.root().join("entered-four"));
    assert_eq!(
        get_json(&format!("{url}api/snapshot"))["view"]["state"],
        "unfinished"
    );
    project.stop();
    assert_eq!(get_json(&format!("{url}api/view"))["state"], "unfinished");
    project.release("four");
    run.finish();
    let address = url.strip_prefix("http://").unwrap().trim_end_matches('/');
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn a_slow_artifact_reader_cannot_keep_the_service_alive_after_stop() {
    let Some(project) = Project::new() else {
        return;
    };
    let service = project.start();
    let base = service["url"].as_str().unwrap();
    let mut run = project.run("large", false, false);
    let url = run.url();
    await_file(&project.root().join("entered-large"));
    project.release("large");
    run.finish();
    let completed = get_json(&format!("{url}api/view"));
    let artifact = completed["artifacts"].as_array().unwrap().first().unwrap();
    assert_eq!(artifact["size"], 16 * 1024 * 1024);
    let address = base.strip_prefix("http://").unwrap().trim_end_matches('/');
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n",
        artifact["url"].as_str().unwrap()
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        if header == "\r\n" {
            break;
        }
    }
    // Leave most bytes unread: the response exceeds socket buffers, so graceful shutdown
    // cannot finish until the server imposes its own bound. The client remains connected.
    let started = Instant::now();
    project.stop();
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(TcpStream::connect(address).is_err());
    let status = project
        .command()
        .args(["status", "--json"])
        .output()
        .unwrap();
    assert_eq!(status.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&status.stdout).unwrap()["state"],
        "stopped"
    );
    drop(reader);
    let restarted = json_success(
        project
            .command()
            .args(["start", "--background", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(restarted["url"], service["url"]);
    assert_eq!(get_json(&format!("{url}api/view"))["state"], "succeeded");
}
