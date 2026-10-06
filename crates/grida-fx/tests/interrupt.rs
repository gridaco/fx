//! An interrupted `grida-fx` (SIGINT, SIGTERM) ends the node hosts it started and what their
//! code started, whatever it was doing; a run is cancelled the way Ctrl-C cancels it.
//!
//! The projects have Python node modules, so the tests skip without a Python that has the `grida`
//! package (`GRIDA_FX_PYTHON`, else `python/.venv/bin/python`).
#![cfg(unix)]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The Python that hosts node bodies, when one is there.
fn python_host() -> Option<PathBuf> {
    if let Some(python) = std::env::var_os("GRIDA_FX_PYTHON").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(python));
    }
    let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/.venv/bin/python");
    venv.is_file().then_some(venv)
}

/// A project whose workflow runs `./nodes/n.py#waits`, the module being `module`.
fn project(module: &str) -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::create_dir_all(root.join("nodes")).unwrap();
    std::fs::create_dir_all(root.join("workflows")).unwrap();
    std::fs::write(root.join("nodes/n.py"), module).unwrap();
    std::fs::write(
        root.join("workflows/case.yaml"),
        "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  s:\n    uses: ./nodes/n.py#waits\n",
    )
    .unwrap();
    folder
}

/// Starts `grida-fx run case --run runs/one` in `root`, leading a process group of its own as a
/// terminal's foreground job does. The node code writes `<host pid> <child pid>` to `pids.txt`.
fn start(root: &Path, python: &Path) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command
        .args(["run", "case", "--run", "runs/one"])
        .current_dir(root)
        .env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
        .env("HOME", root)
        .env("NO_COLOR", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("GRIDA_FX_PYTHON", python)
        .env("FX_TEST_PIDS", root.join("pids.txt"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    command.spawn().unwrap()
}

/// Waits for the node code to say which processes it is (up to 60 seconds).
fn pids(root: &Path) -> Vec<String> {
    let path = root.join("pids.txt");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        if let Ok(text) = std::fs::read_to_string(&path)
            && text.ends_with('\n')
        {
            return text.split_whitespace().map(str::to_string).collect();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the node code never started");
}

fn signal(target: &str, name: &str) {
    let sent = Command::new("kill")
        .args(["-s", name, "--", target])
        .status()
        .unwrap();
    assert!(sent.success());
}

fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Waits up to `limit` for the command to exit; its status code.
fn exits_within(child: &mut Child, limit: Duration) -> Option<i32> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("grida-fx was still running {limit:?} after the signal");
}

/// Asserts that every process is gone within 3 seconds.
fn all_gone(pids: &[String]) {
    for pid in pids {
        let start = Instant::now();
        while alive(pid) && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(pid), "process {pid} outlived grida-fx");
    }
}

/// A module that starts a child and then never finishes importing.
const HANGS_AT_IMPORT: &str = r#"
import os, subprocess, time
child = subprocess.Popen(["sleep", "60"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
with open(os.environ["FX_TEST_PIDS"], "w") as f:
    f.write("%d %d\n" % (os.getpid(), child.pid))
while True:
    time.sleep(0.1)
"#;

/// A body that starts a child and waits to be cancelled.
const WAITS: &str = r#"
import os, subprocess, time
from grida.fx import Ctx, node

@node("waits", outputs={"text": "text"}, version=1)
def waits(ctx: Ctx) -> dict:
    child = subprocess.Popen(["sleep", "60"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
    with open(os.environ["FX_TEST_PIDS"], "w") as f:
        f.write("%d %d\n" % (os.getpid(), child.pid))
    while not ctx.cancelled:
        time.sleep(0.05)
    raise ctx.fail("stopped on cancel")
"#;

#[test]
fn an_interrupt_while_planning_ends_the_hosts_at_once() {
    let Some(python) = python_host() else {
        eprintln!("skipped: no Python with the grida package");
        return;
    };
    for (target_group, name) in [(true, "INT"), (false, "TERM")] {
        let folder = project(HANGS_AT_IMPORT);
        let mut command = start(folder.path(), &python);
        let pids = pids(folder.path());
        let id = command.id().to_string();
        // A terminal sends Ctrl-C to the foreground group; a SIGTERM usually names the process.
        let target = if target_group { format!("-{id}") } else { id };
        let sent = Instant::now();
        signal(&target, name);
        assert_eq!(
            exits_within(&mut command, Duration::from_secs(10)),
            Some(130),
            "SIG{name}"
        );
        assert!(sent.elapsed() < Duration::from_secs(3), "SIG{name}");
        all_gone(&pids);
    }
}

#[test]
fn an_interrupted_run_is_cancelled_and_ends_what_its_bodies_started() {
    let Some(python) = python_host() else {
        eprintln!("skipped: no Python with the grida package");
        return;
    };
    for name in ["TERM", "INT"] {
        let folder = project(WAITS);
        let mut command = start(folder.path(), &python);
        let pids = pids(folder.path());
        signal(&command.id().to_string(), name);
        assert_eq!(
            exits_within(&mut command, Duration::from_secs(20)),
            Some(130),
            "SIG{name}"
        );
        all_gone(&pids);
        let events = std::fs::read_to_string(folder.path().join("runs/one/events.jsonl")).unwrap();
        assert!(
            events.contains("\"event\":\"run_cancelled\""),
            "SIG{name}: {events}"
        );
        assert!(!events.contains("\"event\":\"run_finished\""), "SIG{name}");
    }
}
