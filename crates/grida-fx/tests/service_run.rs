//! The installed CLI path connects ordinary provider-free execution to a persistent service.
//! Gates prove lifetimes without timing assumptions, and a fake opener never launches a browser.
#![cfg(unix)]

use grida_fx_providers::testing::write_program;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
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
        write_program(
            &opener,
            &format!(
                r#"#!{}
import json
from pathlib import Path
import sys
from urllib.request import urlopen
root = Path(__file__).parent.parent
(root / "opener-invoked").touch()
url = sys.argv[1]
with urlopen(url, timeout=5) as response:
    exact_url = response.geturl()
with urlopen(exact_url + "api/view", timeout=5) as response:
    view = json.load(response)
temporary = root / "opened.tmp"
receipt = {{"url": url, "exact_url": exact_url, "kind": view["kind"]}}
receipt.update({{name: view[name] for name in ("steps", "takes_file") if name in view}})
temporary.write_text(json.dumps(receipt))
temporary.replace(root / "opened.json")
"#,
                project.python.display()
            ),
        );
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
        self.start_on(port)
    }
    fn start_on(&self, port: u16) -> Value {
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
        let inputs = format!("inputs-{token}.yaml");
        std::fs::write(self.root().join(&inputs), format!("token: {token}\n")).unwrap();
        let mut command = self.command();
        command.args([
            "run",
            "workflow.yaml",
            "--inputs",
            &inputs,
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
        assert!(output.contains("result    ok   booked $0.00"), "{output}");
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
fn http_get(url: &str) -> Vec<u8> {
    http_request(url, "GET", "", "")
}

fn http_request(url: &str, method: &str, headers: &str, body: &str) -> Vec<u8> {
    let (address, path) = url
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(35)))
        .unwrap();
    write!(
        stream,
        "{method} /{path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n{headers}Content-Length: {}\r\n\r\n{body}",
        body.len(),
    )
    .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    response
}

fn get(url: &str) -> Vec<u8> {
    let response = http_get(url);
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

fn exact_run_url(stable_url: &str) -> String {
    let response = http_get(stable_url);
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 307"), "{response}");
    let location = response
        .lines()
        .find_map(|line| line.strip_prefix("location: /"))
        .unwrap();
    let (base, _) = stable_url.split_once("/p/").unwrap();
    format!("{base}/{location}")
}

fn control_post(url: &str, operation: &str, origin: &str, body: Value) -> (u16, Value) {
    viewer_post(
        &format!("{url}api/control/{operation}"),
        operation,
        origin,
        body,
    )
}

fn viewer_post(url: &str, action: &str, origin: &str, body: Value) -> (u16, Value) {
    let response = http_request(
        url,
        "POST",
        &format!("Origin: {origin}\r\nContent-Type: application/json\r\nX-FX-Action: {action}\r\n"),
        &body.to_string(),
    );
    let response = String::from_utf8(response).unwrap();
    let (header, body) = response.split_once("\r\n\r\n").unwrap();
    let status = header
        .split_ascii_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();
    (
        status,
        serde_json::from_str(body).unwrap_or_else(|_| json!(body)),
    )
}

fn await_run_state(url: &str, state: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let view = get_json(&format!("{url}api/view"));
        if view["state"] == state {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {state}; got {}",
            view["state"]
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn await_exit(run: &mut Running) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while run.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "workflow runner did not stop");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn read_events(project: &Project, folder: &str) -> Vec<Value> {
    std::fs::read_to_string(project.root().join(folder).join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn start_control_fixture(project: &Project, folder: &str, live: bool) -> Running {
    let mut command = project.command();
    command.args(["run", "workflow.yaml", "--run", folder, "--max-usd", "0"]);
    if live {
        command.arg("--live");
    }
    Running::spawn(command)
}

#[test]
fn viewer_pauses_resumes_and_reruns_exact_attempts_through_the_project_service() {
    let Some(project) = Project::new() else {
        return;
    };
    let nodes = include_str!("../../../fixtures/control/nodes.py");
    let workflow = include_str!("../../../fixtures/control/workflow.yaml")
        .replace("./nodes.py#after", "./finish.py#finish");
    std::fs::write(project.root().join("nodes.py"), nodes).unwrap();
    std::fs::write(project.root().join("workflow.yaml"), workflow).unwrap();
    std::fs::write(
        project.root().join("finish.py"),
        r#"
from os import getenv
from time import monotonic, sleep
from pathlib import Path
from grida.fx import Ctx, node

@node("viewer_finish", inputs={"text": "text"}, outputs={"text": "text"}, version=1)
def finish(ctx: Ctx) -> dict:
    if getenv("ACME_VIEWER_TEST_KEY"):
        raise ctx.fail("viewer start inherited the test provider variable")
    home = Path(__file__).parent
    (home / "finish-entered").touch()
    deadline = monotonic() + 30
    while not (home / "finish-release").is_file():
        if ctx.cancelled or monotonic() >= deadline:
            raise ctx.fail("finish gate did not release")
        sleep(0.01)
    return {"text": ctx.inputs["text"]}
"#,
    )
    .unwrap();
    let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let service = json_success(
        project
            .command()
            .env("ACME_VIEWER_TEST_KEY", "fixture")
            .args([
                "start",
                "--background",
                "--port",
                &port.to_string(),
                "--json",
            ])
            .output()
            .unwrap(),
    );
    let origin = service["url"].as_str().unwrap().trim_end_matches('/');
    let folder = "runs/control";
    let mut run = start_control_fixture(&project, folder, false);
    let stable = run.url();
    let exact = exact_run_url(&stable);
    await_file(&project.root().join(".control-entered"));
    let control = get_json(&format!("{exact}api/control"));
    let first = control["invocation_id"].as_str().unwrap();
    assert_eq!(
        control["actions"],
        json!({"pause":true,"cancel":true,"resume":false,"rerun":false})
    );

    let (status, _) = control_post(
        &exact,
        "pause",
        "https://example.invalid",
        json!({"invocation_id":first}),
    );
    assert_eq!(status, 403);
    let (_, paused) = control_post(&exact, "pause", origin, json!({"invocation_id":first}));
    assert_eq!(paused["outcome"], "accepted", "{paused}");
    await_run_state(&exact, "pausing");
    std::fs::write(project.root().join(".control-release"), "").unwrap();
    await_exit(&mut run);
    await_run_state(&exact, "paused");
    let pause_events = read_events(&project, folder);
    assert!(
        pause_events
            .iter()
            .any(|event| event["event"] == "pause_requested" && event["source"] == "viewer")
    );
    assert!(!project.root().join("finish-entered").exists());

    // The service was launched before this variable became a provider credential.
    let configuration = project.root().join("fx.yaml");
    std::fs::OpenOptions::new()
        .append(true)
        .open(configuration)
        .unwrap()
        .write_all(
            br#"
providers:
  acme:
    credential: {variable: ACME_VIEWER_TEST_KEY, header: authorization, scheme: Bearer}
    base_url: {default: https://api.acme.test/v1}
    family: openai-compatible
"#,
        )
        .unwrap();

    let resume_body =
        json!({"invocation_id":first,"request_id":"bbd27793-3eb5-428d-aaf6-6f217a8f3d73"});
    let (_, resumed) = control_post(&exact, "resume", origin, resume_body);
    assert_eq!(resumed["outcome"], "accepted", "{resumed}");
    assert_eq!(
        format!("{origin}{}", resumed["url"].as_str().unwrap()),
        exact
    );
    await_file(&project.root().join("finish-entered"));
    let live = get_json(&format!("{exact}api/control"));
    let second = live["invocation_id"].as_str().unwrap();
    assert_ne!(second, first);
    let (_, stale) = control_post(&exact, "cancel", origin, json!({"invocation_id":first}));
    assert_eq!(stale["code"], "invocation_mismatch", "{stale}");
    project.stop();
    let restart = json_success(
        project
            .command()
            .args(["start", "--background", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(restart["url"], service["url"]);
    let resumed_after_restart = get_json(&format!("{exact}api/control"));
    assert_eq!(resumed_after_restart["invocation_id"], second);
    assert_eq!(resumed_after_restart["actions"]["cancel"], true);
    std::fs::write(project.root().join("finish-release"), "").unwrap();
    await_run_state(&exact, "succeeded");

    let rerun_body =
        json!({"invocation_id":second,"request_id":"01d42701-37d2-4bcc-afee-71e55f94451d"});
    let (_, rerun) = control_post(&exact, "rerun", origin, rerun_body.clone());
    assert_eq!(rerun["outcome"], "accepted", "{rerun}");
    let latest = format!("{origin}{}", rerun["url"].as_str().unwrap());
    assert_ne!(latest, exact);
    let (_, replay) = control_post(&exact, "rerun", origin, rerun_body);
    assert_eq!(replay["url"], rerun["url"], "{replay}");
    assert_eq!(replay["outcome"], "accepted", "{replay}");
    assert_eq!(exact_run_url(&stable), latest);
    await_run_state(&latest, "succeeded");
    let history = get_json(&format!("{exact}api/history"));
    assert_eq!(history["selected_attempt"], 1);
    assert_eq!(history["attempts"].as_array().unwrap().len(), 2);
}

#[test]
fn viewer_cancels_an_active_invocation_and_refuses_implicit_live_rerun() {
    let Some(project) = Project::new() else {
        return;
    };
    let service = project.start();
    let origin = service["url"].as_str().unwrap().trim_end_matches('/');
    // The fixture has only a local step. --live exercises the saved authorization policy.
    let folder = "runs/live";
    let mut run = start_control_fixture(&project, folder, true);
    let url = run.url();
    let exact = exact_run_url(&url);
    await_file(&project.root().join("entered-one"));
    let inspect = get_json(&format!("{exact}api/control"));
    let invocation = inspect["invocation_id"].as_str().unwrap();
    let (_, cancelled) = control_post(
        &exact,
        "cancel",
        origin,
        json!({"invocation_id":invocation}),
    );
    assert_eq!(cancelled["outcome"], "accepted", "{cancelled}");
    await_exit(&mut run);
    await_run_state(&exact, "cancelled");
    let events = read_events(&project, folder);
    assert!(
        events
            .iter()
            .any(|event| event["event"] == "cancel_requested" && event["source"] == "viewer")
    );
    assert!(!events.iter().any(|event| event["event"] == "call"));
    let (_, denied) = control_post(
        &exact,
        "rerun",
        origin,
        json!({"invocation_id":invocation,"request_id":"53104d6a-a2cf-4153-b95f-d2879d31b0a6"}),
    );
    assert_eq!(denied["outcome"], "refused", "{denied}");
    assert_eq!(denied["code"], "approval_required", "{denied}");
    assert!(denied["cli_command"].as_str().unwrap().contains("rerun"));
    assert!(
        !denied
            .to_string()
            .contains(project.root().to_str().unwrap())
    );
    assert_eq!(exact_run_url(&url), exact);
}

#[test]
fn viewer_rerun_continues_after_a_response_to_the_new_attempt() {
    let Some(project) = Project::new() else {
        return;
    };
    std::fs::write(
        project.root().join("workflow.yaml"),
        r#"fx: workflow/v1
id: review
title: Review
steps:
  review:
    uses: fx/await@1
    with:
      schema: {type: string}
      timeout_seconds: 60
outputs:
  answer: ${{ steps.review.outputs.answer }}
"#,
    )
    .unwrap();
    let service = project.start();
    let origin = service["url"].as_str().unwrap().trim_end_matches('/');
    let mut run = start_control_fixture(&project, "runs/review", false);
    let stable = run.url();
    let original = exact_run_url(&stable);
    await_exit(&mut run);
    let before = get_json(&format!("{original}api/requests"));
    let original_request = &before["requests"][0];
    let control = get_json(&format!("{original}api/control"));
    let (_, rerun) = control_post(
        &original,
        "rerun",
        origin,
        json!({
            "invocation_id":control["invocation_id"],
            "request_id":"cb588c1e-50e9-4a53-bac0-1fef6dba8901",
        }),
    );
    assert_eq!(rerun["outcome"], "accepted", "{rerun}");
    let exact = format!("{origin}{}", rerun["url"].as_str().unwrap());
    let deadline = Instant::now() + Duration::from_secs(15);
    let request = loop {
        let result = get_json(&format!("{exact}api/requests"));
        if let Some(request) = result["requests"].get(0) {
            break request.clone();
        }
        assert!(
            Instant::now() < deadline,
            "the new attempt did not reach its review"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_ne!(original_request["id"], request["id"]);
    assert!(request["deadline"].as_str() > original_request["deadline"].as_str());
    let (status, _) = viewer_post(
        &format!(
            "{exact}api/requests/{}/respond",
            request["id"].as_str().unwrap()
        ),
        "respond",
        origin,
        json!({"answer":"new result"}),
    );
    assert_eq!(status, 200);
    await_run_state(&exact, "succeeded");
    assert_eq!(get_json(&format!("{original}api/requests")), before);
}

#[test]
fn a_run_stays_inspectable_after_completion_and_service_restart() {
    let Some(project) = Project::new() else {
        return;
    };
    let service = project.start();
    let base = service["url"].as_str().unwrap();
    let mut run = project.run("one", false, true);
    let stable_url = run.url();
    assert!(stable_url.starts_with(&format!("{base}p/")) && stable_url.contains("/run/"));
    let url = exact_run_url(&stable_url);
    await_file(&project.root().join("entered-one"));
    assert_eq!(
        get_json(&format!("{url}api/snapshot"))["view"]["state"],
        "unfinished"
    );
    await_file(&project.root().join("opened.json"));
    let opened: Value =
        serde_json::from_slice(&std::fs::read(project.root().join("opened.json")).unwrap())
            .unwrap();
    assert_eq!(opened["url"], stable_url);
    assert_eq!(opened["exact_url"], url);
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
    assert_eq!(exact_run_url(&stable_url), url);
    assert_eq!(get_json(&format!("{url}api/view"))["state"], "succeeded");
    assert_eq!(get(&artifact_url), b"completed one");
    let inspection = project
        .command()
        .args(["inspect", "runs/one", "--open"])
        .output()
        .unwrap();
    assert!(inspection.status.success());
    assert!(String::from_utf8_lossy(&inspection.stdout).contains(&url));

    let rerun = project
        .command()
        .args(["rerun", "runs/one", "--no-view"])
        .output()
        .unwrap();
    assert!(
        rerun.status.success(),
        "{}",
        String::from_utf8_lossy(&rerun.stderr)
    );
    let latest = exact_run_url(&stable_url);
    assert_ne!(latest, url);
    let before = get_json(&format!("{url}api/history"));
    let after = get_json(&format!("{latest}api/history"));
    assert_eq!(before["selected_attempt"], 1);
    assert_eq!(after["selected_attempt"], 2);
    assert_eq!(before["attempts"], after["attempts"]);
    assert_eq!(
        after["attempts"][0]["url"],
        format!("/{}", latest.strip_prefix(base).unwrap())
    );
    assert_eq!(get(&artifact_url), b"completed one");
    let mut resume = project.run("one", false, false);
    assert_eq!(resume.url(), url);
    resume.finish();
    assert_eq!(exact_run_url(&stable_url), latest);
}

#[test]
fn stopping_the_service_during_execution_does_not_cancel_the_run() {
    let Some(project) = Project::new() else {
        return;
    };
    let service = project.start();
    let mut run = project.run("two", false, false);
    let url = exact_run_url(&run.url());
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
    let url = exact_run_url(&run.url());
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

#[test]
fn a_registered_plan_survives_a_new_service_address_without_replanning_or_opening() {
    let Some(project) = Project::new() else {
        return;
    };
    let graph = json_success(
        project
            .command()
            .args(["plan", "workflow.yaml", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(graph["kind"], "fx-graph-v1");
    assert!(!project.root().join(".fx/service").exists());
    let register = || {
        json_success(
            project
                .command()
                .args(["plan", "workflow.yaml", "--register", "--check", "--json"])
                .output()
                .unwrap(),
        )
    };
    let first = register();
    assert_eq!(first["plan_ok"], true);
    assert_eq!(first, register());
    assert!(!project.root().join("opener-invoked").exists());
    let route = first["route"].as_str().unwrap();
    let service = project.start();
    let base = service["url"].as_str().unwrap().trim_end_matches('/');
    let graph = get_json(&format!("{base}{route}api/view"));
    assert_eq!(graph["kind"], "fx-graph-v1");
    assert!(graph["steps"].as_object().unwrap().contains_key("wait"));
    let catalog = get_json(&format!("{base}/api/catalog"));
    let entry_id = &first["reference"]["entry_id"];
    let created = catalog["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| &entry["id"] == entry_id)
        .unwrap()["created_at"]
        .clone();
    let alternate_port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    project.stop();

    let path = project.root().join("workflow.yaml");
    let revised = std::fs::read_to_string(&path)
        .unwrap()
        .replace("title: Service test", "title: Revised workflow");
    std::fs::write(&path, revised).unwrap();
    assert_ne!(first["reference"], register()["reference"]);
    let source_reference = json!({
        "subject": {"kind": "plan", "reference": first["reference"]},
        "target": {"kind": "source", "source": "workflow.yaml"},
    });
    let resolved = json_success(
        project
            .command()
            .args([
                "files",
                "resolve",
                "--service-entry",
                "--project",
                ".",
                "--reference",
                &source_reference.to_string(),
                "--json",
            ])
            .output()
            .unwrap(),
    );
    assert!(
        resolved["local_path"]
            .as_str()
            .unwrap()
            .ends_with("workflow.yaml")
    );
    std::fs::remove_file(path).unwrap();
    let restarted = project.start_on(alternate_port);
    assert_ne!(restarted["url"], service["url"]);
    assert_ne!(restarted["instance_id"], service["instance_id"]);
    assert_eq!(restarted["project_id"], first["reference"]["project_id"]);
    let base = restarted["url"].as_str().unwrap().trim_end_matches('/');
    let restored = get_json(&format!("{base}{route}api/view"));
    assert_eq!(restored, graph);
    let catalog = get_json(&format!("{base}/api/catalog"));
    let entry = catalog["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| &entry["id"] == entry_id)
        .unwrap();
    assert_eq!(entry["created_at"], created);
    assert!(!project.root().join("opened.json").exists());
}

#[test]
fn inspect_registers_an_existing_run_without_executing_or_opening_it() {
    let Some(project) = Project::new() else {
        return;
    };
    let folder = "saved-run";
    project.release("one");
    let run = project
        .command()
        .args(["run", "workflow.yaml", "--run", folder, "--no-view"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    std::fs::remove_file(project.root().join("workflow.yaml")).unwrap();
    std::fs::remove_file(project.root().join("node.py")).unwrap();

    let summary = json_success(
        project
            .command()
            .args(["inspect", folder, "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(summary["run"]["state"], "succeeded");
    assert!(!project.root().join(".fx/service").exists());

    let register = || {
        json_success(
            project
                .command()
                .args(["inspect", folder, "--register", "--json"])
                .output()
                .unwrap(),
        )
    };
    let receipt = register();
    assert_eq!(receipt["kind"], "fx-run-registration-v1");
    assert_eq!(receipt["reference"]["kind"], "service_entry");
    assert_eq!(receipt["reference"]["entry_kind"], "run");
    let route = receipt["route"].as_str().unwrap();
    let reference = &receipt["reference"];
    assert_eq!(
        route,
        format!(
            "/p/{}/runs/{}/",
            reference["project_id"].as_str().unwrap(),
            reference["entry_id"].as_str().unwrap()
        )
    );
    assert_eq!(receipt, register());

    let text = project
        .command()
        .args(["inspect", folder, "--register"])
        .output()
        .unwrap();
    assert!(text.status.success());
    assert!(String::from_utf8_lossy(&text.stdout).lines().any(|line| {
        line.strip_prefix("registered")
            .is_some_and(|value| value.trim() == route)
    }));
    assert!(!project.root().join("opener-invoked").exists());
    let stopped = project
        .command()
        .args(["status", "--project-root", ".", "--json"])
        .output()
        .unwrap();
    assert_eq!(stopped.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&stopped.stdout).unwrap()["state"],
        "stopped"
    );

    let placed = summary["run"]["steps"][0]["files"][0]["path"]
        .as_str()
        .unwrap();
    std::fs::remove_file(project.root().join(folder).join(placed)).unwrap();
    let verified = project
        .command()
        .args(["inspect", folder, "--register", "--verify", "--json"])
        .output()
        .unwrap();
    assert_eq!(verified.status.code(), Some(1));
    let verified: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(verified["route"], route);
    assert_eq!(verified["verification"]["verified"], false);
    assert_eq!(
        verified["verification"]["problems"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(receipt, register());

    let service = project.start();
    assert_eq!(service["project_id"], reference["project_id"]);
    let base = service["url"].as_str().unwrap().trim_end_matches('/');
    let url = format!("{base}{route}");
    assert_eq!(get_json(&format!("{url}api/view"))["state"], "succeeded");
    assert!(!project.root().join("opener-invoked").exists());
    let opened = project
        .command()
        .args(["inspect", folder, "--open"])
        .output()
        .unwrap();
    assert!(opened.status.success());
    await_file(&project.root().join("opened.json"));
    let opened: Value =
        serde_json::from_slice(&std::fs::read(project.root().join("opened.json")).unwrap())
            .unwrap();
    assert_eq!(opened["url"], url);

    let alternate = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = alternate.local_addr().unwrap().port();
    project.stop();
    drop(alternate);
    let restarted = project.start_on(port);
    assert_ne!(restarted["url"], service["url"]);
    assert_eq!(restarted["project_id"], reference["project_id"]);
    let base = restarted["url"].as_str().unwrap().trim_end_matches('/');
    assert_eq!(
        get_json(&format!("{base}{route}api/view"))["state"],
        "succeeded"
    );
}
