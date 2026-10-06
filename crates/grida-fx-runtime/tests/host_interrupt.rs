//! Ending every node host at once, as an interrupted command does before it exits
//! (`host::end_every_host`). A test binary of its own: once called, no host starts in the process.
//!
//! The hosts are a stdlib-only fake started through a shell script named as the interpreter; the
//! test needs `python3` on `PATH` and skips without it.
#![cfg(unix)]

use grida_fx_runtime::host::connection::NoIncoming;
use grida_fx_runtime::host::end_every_host;
use grida_fx_runtime::host::process::{HostProcess, HostSpec};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

/// Answers `initialize`, starts a grandchild, then ignores everything (the end of its input too).
const HOST: &str = r#"
import json, os, subprocess, sys, time
def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        name, _, value = line.decode("ascii").partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(length))
message = read()
body = json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {
    "protocol": message["params"]["protocol"],
    "host": {"language": "python", "version": "3", "sdk_version": "0"}}}).encode()
child = subprocess.Popen(["sleep", "60"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
with open("pids-%d.txt" % os.getpid(), "w") as f:
    f.write("%d %d" % (os.getpid(), child.pid))
sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
sys.stdout.buffer.flush()
while read() is not None:
    pass
time.sleep(60)
"#;

fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_every_host_kills_their_groups_and_starts_no_more() {
    let found = Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !found {
        eprintln!("skipped: python3 is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("host.py"), HOST).unwrap();
    let python: PathBuf = dir.path().join("python-host");
    std::fs::write(
        &python,
        "#!/bin/sh\nexec python3 \"$(dirname \"$0\")/host.py\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
    let spec = HostSpec {
        python,
        label: "python-host".into(),
        project_root: dir.path().to_path_buf(),
        sources: Vec::new(),
    };
    let mut hosts = Vec::new();
    for _ in 0..2 {
        hosts.push(
            HostProcess::start(&spec, Arc::new(NoIncoming))
                .await
                .unwrap(),
        );
    }
    let mut grandchildren = Vec::new();
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("pids-")
        {
            let text = std::fs::read_to_string(path).unwrap();
            let (_, grandchild) = text.split_once(' ').unwrap();
            grandchildren.push(grandchild.to_string());
        }
    }
    assert_eq!(grandchildren.len(), 2);
    assert!(grandchildren.iter().all(|pid| alive(pid)));

    end_every_host();

    for host in &mut hosts {
        host.connection().closed().await;
        let mut status = None;
        for _ in 0..300 {
            status = host.exit_status();
            if status.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let status = status.expect("the host was not killed");
        assert_eq!(status.code(), None, "killed by a signal");
    }
    for pid in &grandchildren {
        let mut gone = false;
        for _ in 0..300 {
            if !alive(pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(gone, "{pid} survived");
    }
    // No host starts after it.
    let refused = HostProcess::start(&spec, Arc::new(NoIncoming))
        .await
        .err()
        .unwrap();
    assert_eq!(refused, "the command is stopping, so no node host starts");
    for host in hosts {
        assert!(host.kill().await.is_some());
    }
}
