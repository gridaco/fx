//! Public CLI service lifecycle: independent native process, authenticated ownership and
//! explicit project discovery. No provider, node host or browser is involved.
#![cfg(unix)]

use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, TcpListener};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command
        .current_dir(root)
        .env_clear()
        .env("GRIDA_FX_DISABLE_DOTENV", "1")
        .env("GRIDA_FX_NETWORK", "off");
    command
}

fn execute(root: &Path, args: &[&str]) -> Output {
    command(root).args(args).output().unwrap()
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Service {
    root: tempfile::TempDir,
    foreground: Option<Child>,
}

impl Service {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        success(execute(root.path(), &["init", "--json"]));
        Self {
            root,
            foreground: None,
        }
    }
    fn path(&self) -> &Path {
        self.root.path()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = execute(self.path(), &["stop", "--json"]);
        if let Some(child) = &mut self.foreground {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn init_only_creates_config_and_preserves_an_existing_authored_file() {
    let root = tempfile::tempdir().unwrap();
    let initialized = success(execute(root.path(), &["init", "--json"]));
    assert_eq!(initialized["kind"], "fx-project-init-v1");
    assert_eq!(initialized["created"], true);
    assert_eq!(initialized["files_created"], json!(["fx.yaml"]));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    let source = "# Authored configuration\nfx: project/v1\nruns: history\n";
    std::fs::write(root.path().join("fx.yaml"), source).unwrap();
    let repeated = success(execute(root.path(), &["init", "--json"]));
    assert_eq!(repeated["created"], false);
    assert_eq!(
        std::fs::read_to_string(root.path().join("fx.yaml")).unwrap(),
        source
    );
    std::fs::write(root.path().join("fx.yaml"), "invalid: configuration\n").unwrap();
    assert!(!execute(root.path(), &["init", "--json"]).status.success());
    assert_eq!(
        std::fs::read_to_string(root.path().join("fx.yaml")).unwrap(),
        "invalid: configuration\n"
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn background_is_ready_idempotent_and_keeps_the_port_after_stop() {
    let service = Service::new();
    let port = port().to_string();
    let first = success(execute(
        service.path(),
        &["start", "--background", "--port", &port, "--json"],
    ));
    assert_eq!(first["state"], "running");
    assert_eq!(first["url"], format!("http://127.0.0.1:{port}/"));
    assert_eq!(
        success(execute(service.path(), &["status", "--json"])),
        first
    );
    assert_eq!(
        success(execute(
            service.path(),
            &["start", "--background", "--json"]
        )),
        first
    );
    let descriptor = service.path().join(".fx/service/instance.json");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&descriptor).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        success(execute(service.path(), &["stop", "--json"]))["state"],
        "stopped"
    );
    let stopped = execute(service.path(), &["status", "--json"]);
    assert_eq!(stopped.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&stopped.stdout).unwrap()["state"],
        "stopped"
    );
    assert_eq!(
        success(execute(service.path(), &["stop", "--json"]))["state"],
        "stopped"
    );
    let restarted = success(execute(
        service.path(),
        &["start", "--background", "--json"],
    ));
    assert_eq!(restarted["url"], first["url"]);
    assert_eq!(restarted["project_id"], first["project_id"]);
    assert_ne!(restarted["instance_id"], first["instance_id"]);
}

#[test]
fn parallel_background_starts_share_one_instance() {
    let service = Service::new();
    let port = port().to_string();
    // Cold-start contention is the interesting case: status probes briefly acquire the
    // same OS lock used by the new owner. Repeated groups must converge without a false
    // "unavailable" error or a child losing ownership to a transient probe.
    for _ in 0..4 {
        let children = (0..8)
            .map(|_| {
                command(service.path())
                    .args(["start", "--background", "--port", &port, "--json"])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let outputs = children
            .into_iter()
            .map(|child| child.wait_with_output().unwrap())
            .collect::<Vec<_>>();
        let states = outputs.into_iter().map(success).collect::<Vec<_>>();
        assert!(states.iter().all(|state| state == &states[0]));
        success(execute(service.path(), &["stop", "--json"]));
    }
}

#[test]
fn foreground_returns_ready_json_then_stays_until_authenticated_stop() {
    let mut service = Service::new();
    let port = port().to_string();
    let mut child = command(service.path())
        .args(["start", "--port", &port, "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        sender.send(line).unwrap();
    });
    service.foreground = Some(child);
    let line = receiver.recv_timeout(Duration::from_secs(15)).unwrap();
    let ready: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(ready["state"], "running");
    assert!(
        service
            .foreground
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_none()
    );
    assert_eq!(
        success(execute(service.path(), &["stop", "--json"]))["state"],
        "stopped"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = service.foreground.as_mut().unwrap().try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn occupied_port_is_refused_and_unrelated_listener_is_preserved() {
    let service = Service::new();
    let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = occupied.local_addr().unwrap().port().to_string();
    let output = execute(
        service.path(),
        &["start", "--background", "--port", &port, "--json"],
    );
    assert_eq!(output.status.code(), Some(2));
    let stopped = success(execute(service.path(), &["stop", "--json"]));
    assert_eq!(stopped["state"], "stopped");
    assert_eq!(occupied.local_addr().unwrap().port().to_string(), port);
    let log = execute(service.path(), &["logs", "--lines", "5"]);
    assert!(log.status.success());
    assert!(String::from_utf8_lossy(&log.stdout).contains("port"));
    assert_eq!(
        execute(service.path(), &["start", "--port", "0"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn a_wrong_credential_cannot_stop_a_running_service() {
    let service = Service::new();
    let port = port().to_string();
    let first = success(execute(
        service.path(),
        &["start", "--background", "--port", &port, "--json"],
    ));
    let path = service.path().join(".fx/service/instance.json");
    let original = std::fs::read(&path).unwrap();
    let mut changed: Value = serde_json::from_slice(&original).unwrap();
    changed["token"] = json!("0".repeat(64));
    std::fs::write(&path, changed.to_string()).unwrap();
    let refused = execute(service.path(), &["stop", "--json"]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&refused.stdout).unwrap()["state"],
        "unavailable"
    );
    std::fs::write(&path, original).unwrap();
    assert_eq!(
        success(execute(service.path(), &["status", "--json"])),
        first
    );
}

#[test]
fn service_flags_and_viewing_flags_are_parsed_before_workflow_inputs() {
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["plan", "missing.yaml", "--json", "--open"],
        vec!["plan", "missing.yaml", "--json", "--standalone"],
        vec!["inspect", "missing", "--json", "--open"],
        vec!["inspect", "missing", "--json", "--standalone"],
        vec!["run", "missing.yaml", "--standalone", "--no-view"],
    ] {
        let output = execute(root.path(), &args);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
        assert!(output.stdout.is_empty());
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn project_selection_and_stale_descriptors_never_stop_a_different_service() {
    let service = Service::new();
    let other = Service::new();
    let selected_port = port().to_string();
    let first = success(execute(
        service.path(),
        &["start", "--background", "--port", &selected_port, "--json"],
    ));
    let subdirectory = service.path().join("nested");
    std::fs::create_dir(&subdirectory).unwrap();
    assert_eq!(
        success(execute(&subdirectory, &["status", "--json"])),
        first
    );
    assert_eq!(
        success(execute(
            other.path(),
            &[
                "status",
                "--project",
                service.path().to_str().unwrap(),
                "--json"
            ]
        )),
        first
    );
    // A stale descriptor, even with another live service's PID and valid token, conveys no
    // process ownership. An unlocked project must report stopped without contacting it.
    let stale_dir = other.path().join(".fx/service");
    success(execute(other.path(), &["stop", "--json"]));
    std::fs::copy(
        service.path().join(".fx/service/instance.json"),
        stale_dir.join("instance.json"),
    )
    .unwrap();
    assert_eq!(
        success(execute(other.path(), &["stop", "--json"]))["state"],
        "stopped"
    );
    assert_eq!(
        success(execute(service.path(), &["status", "--json"])),
        first
    );
}

#[test]
fn parallel_different_ports_have_one_winner_and_an_explicit_conflict() {
    let service = Service::new();
    let first_port = port().to_string();
    let mut second_port = port().to_string();
    while second_port == first_port {
        second_port = port().to_string();
    }
    let first = command(service.path())
        .args(["start", "--background", "--port", &first_port, "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second = execute(
        service.path(),
        &["start", "--background", "--port", &second_port, "--json"],
    );
    let first = first.wait_with_output().unwrap();
    assert_ne!(first.status.success(), second.status.success());
    let (winner, expected, loser) = if first.status.success() {
        (first, first_port, second)
    } else {
        (second, second_port, first)
    };
    assert_eq!(
        success(winner)["url"],
        format!("http://127.0.0.1:{expected}/")
    );
    assert_eq!(loser.status.code(), Some(2));
}
