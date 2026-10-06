//! The Python node host client against real processes.
//!
//! Most tests run a fake host: a stdlib-only Python program written to a temporary folder and
//! started through a small shell script that `PythonHost::with_python` names, so the engine
//! still runs `<python> -P -m grida.fx.host` and the script passes on a mode. They need `python3`
//! on `PATH` and skip without it. The last test runs the real `grida.fx.host` through
//! `GRIDA_FX_PYTHON` or the repository's `python/.venv`, and skips when neither exists.
#![cfg(unix)]

use grida_fx_core::ENGINE_VERSION;
use grida_fx_core::host::{HostFailure, NodeHost};
use grida_fx_protocol::{
    BuildParams, DescribeParams, DescribeTarget, ErrorCode, ModuleDescription, RetryMode,
};
use grida_fx_runtime::host::{PythonHost, SAFE_PATH_MARK};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The fake host. `sys.argv[1]` is the mode; the rest is what the engine passed.
const FAKE_HOST: &str = r#"
import json
import os
import platform
import signal
import sys
import time

MODE = sys.argv[1]
ENGINE_ARGS = sys.argv[2:]
PROTOCOL_IN = sys.stdin.buffer
PROTOCOL_OUT = sys.stdout.buffer


def log(event):
    with open("host-log.txt", "a") as f:
        f.write(event + "\n")


def read():
    length = None
    while True:
        line = PROTOCOL_IN.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        name, _, value = line.decode("ascii").partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    return json.loads(PROTOCOL_IN.read(length).decode("utf-8"))


def write(message):
    body = json.dumps(message, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    PROTOCOL_OUT.write(b"Content-Length: " + str(len(body)).encode("ascii") + b"\r\n\r\n" + body)
    PROTOCOL_OUT.flush()


def answer(request, result):
    write({"jsonrpc": "2.0", "id": request["id"], "result": result})


def fail(request, code, message, data=None):
    error = {"code": code, "message": message}
    if data is not None:
        error["data"] = data
    write({"jsonrpc": "2.0", "id": request["id"], "error": error})


def spec(name):
    return {
        "name": name,
        "description": "Echoes its input.",
        "inputs": {"image": "image"},
        "params": {"times": {"type": "integer", "default": 1}},
        "outputs": {"text": "text"},
        "judge": False,
        "calls": {},
        "resources": [],
        "tools": ["ffprobe"],
        "view": None,
        "version": 1,
        "retry": "service",
    }


def describe(params):
    modules = []
    for target in params["targets"]:
        path = target["path"]
        attribute = target.get("attribute")
        if path == "nodes/missing.py":
            entry = {"path": path, "error": path + " failed to import: ModuleNotFoundError: No module named 'missing'"}
            if attribute is not None:
                entry["attribute"] = attribute
            modules.append(entry)
            continue
        modules.append({
            "path": path,
            "types": [{"attribute": attribute or "echo", "spec": spec(attribute or "echo")}],
            "closure": [{"label": path, "path": os.path.join(os.getcwd(), path)}],
        })
    return {"modules": modules, "builtins": []}


def request_engine():
    """Asks the engine for something while its describe is pending; returns its answer."""
    write({"jsonrpc": "2.0", "id": 1, "method": "capability",
           "params": {"run_id": "r1", "capability": "image.generate", "request": {}}})
    write({"jsonrpc": "2.0", "method": "progress", "params": {"run_id": "r1", "text": "busy"}})
    while True:
        message = read()
        if message is None:
            sys.exit(5)
        if message.get("id") == 1 and "method" not in message:
            log("response")
            return message


def main():
    if MODE == "exit-early":
        sys.exit(3)
    with open("host-pid.txt", "w") as f:
        f.write(str(os.getpid()))
    shut_down = False
    while True:
        message = read()
        if message is None:
            log("eof")
            sys.exit(0)
        method = message.get("method")
        log(method or "response")
        if method == "initialize":
            params = message["params"]
            with open("initialize.json", "w") as f:
                json.dump({"params": params, "argv": ENGINE_ARGS, "cwd": os.getcwd(),
                           "safe_path": os.environ.get("PYTHONSAFEPATH")}, f)
            if MODE == "mismatch-error":
                fail(message, -32003, "the engine speaks another protocol",
                     {"engine_protocol": params["protocol"], "host_protocol": "fx-node-protocol-v0"})
                continue
            protocol = "fx-node-protocol-v2" if MODE == "mismatch-result" else params["protocol"]
            sdk = "9.9.9" if MODE == "mismatch-result" else "0.0.0-fake"
            answer(message, {"protocol": protocol,
                             "host": {"language": "python", "version": platform.python_version(),
                                      "sdk_version": sdk}})
        elif method == "describe":
            if MODE == "exit-in-describe":
                sys.exit(4)
            if MODE == "killed-in-describe":
                os.kill(os.getpid(), signal.SIGTERM)
            if MODE == "fork-in-describe":
                child = os.fork()
                if child == 0:
                    time.sleep(30)
                    os._exit(0)
                with open("forked.txt", "w") as f:
                    f.write(str(child))
                os._exit(4)
            if MODE == "garbage":
                PROTOCOL_OUT.write(b"Content-Length: 9\r\n\r\nnot json!")
                PROTOCOL_OUT.flush()
                continue
            if MODE == "bad-result":
                answer(message, {"modules": "none"})
                continue
            if MODE == "request-in-describe":
                reply = request_engine()
                error = reply["error"]
                answer(message, {"modules": [{"path": "nodes/echo.py",
                                              "error": "engine answered %d: %s" % (error["code"], error["message"])}],
                                 "builtins": []})
                continue
            answer(message, describe(message["params"]))
        elif method == "build":
            params = message["params"]
            if params["function"] == "boom":
                fail(message, -32005, "boom: RuntimeError: no levels")
                continue
            answer(message, {"document": {"fx": "workflow/v1", "id": "demo", "title": "Demo",
                                          "seen": {"arguments": params["arguments"], "cwd": params["cwd"]}},
                             "takes_anchor": params["path"]})
        elif method == "shutdown":
            shut_down = True
            answer(message, None)
        elif method == "exit":
            if MODE == "hang-on-exit":
                time.sleep(30)
            log("status %d" % (0 if shut_down else 1))
            sys.exit(0 if shut_down else 1)
        elif "id" in message and method is not None:
            fail(message, -32601, "no " + method)


main()
"#;

fn have_python3() -> bool {
    let found = Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !found {
        eprintln!("skipped: python3 is not on PATH");
    }
    found
}

/// A project folder and a fake interpreter running the fake host in `mode`.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    python: PathBuf,
}

impl Fixture {
    fn new(mode: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("acme");
        std::fs::create_dir_all(root.join("nodes")).unwrap();
        std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
        let host = dir.path().join("fake_host.py");
        std::fs::write(&host, FAKE_HOST).unwrap();
        let python = dir.path().join(format!("python-{mode}"));
        std::fs::write(
            &python,
            format!(
                "#!/bin/sh\nexec python3 '{}' {mode} \"$@\"\n",
                host.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        Fixture {
            _dir: dir,
            root,
            python,
        }
    }

    fn host(&self) -> PythonHost {
        PythonHost::new().with_python(self.python.clone())
    }

    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("host-log.txt"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn initialize(&self) -> Value {
        let text = std::fs::read_to_string(self.root.join("initialize.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn pid(&self) -> String {
        std::fs::read_to_string(self.root.join("host-pid.txt")).unwrap()
    }
}

fn describe_params(targets: &[(&str, Option<&str>)]) -> DescribeParams {
    DescribeParams {
        targets: targets
            .iter()
            .map(|(path, attribute)| DescribeTarget {
                path: path.to_string(),
                attribute: attribute.map(str::to_string),
            })
            .collect(),
        builtins: false,
    }
}

fn build_params(function: &str, cwd: &Path) -> BuildParams {
    BuildParams {
        path: "workflows/demo.py".into(),
        function: function.into(),
        arguments: IndexMap::from([("count".to_string(), "3".to_string())]),
        cwd: cwd.to_str().unwrap().into(),
    }
}

fn unavailable(failure: HostFailure) -> String {
    match failure {
        HostFailure::Unavailable(message) => message,
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

fn canonical(path: impl AsRef<Path>) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

#[test]
fn initialize_describe_build_and_shutdown() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("ok");
    let mut host = fixture.host();
    host.open_project(&fixture.root, &["acme_lib".to_string()])
        .unwrap();
    // Lazy: opening a project starts nothing.
    assert!(!fixture.root.join("initialize.json").exists());
    assert!(host.info.is_none());

    let described = host
        .describe(&describe_params(&[
            ("nodes/echo.py", Some("echo")),
            ("nodes/missing.py", None),
        ]))
        .unwrap();
    let ModuleDescription::Described {
        path,
        types,
        closure,
    } = &described.modules[0]
    else {
        panic!("{described:?}");
    };
    assert_eq!(path, "nodes/echo.py");
    assert_eq!(types[0].attribute, "echo");
    assert_eq!(types[0].spec.version, Some(1));
    assert_eq!(types[0].spec.retry, RetryMode::Service);
    assert_eq!(types[0].spec.tools, ["ffprobe"]);
    assert_eq!(
        types[0].spec.description.as_deref(),
        Some("Echoes its input.")
    );
    assert_eq!(closure[0].label, "nodes/echo.py");
    assert_eq!(
        described.modules[1],
        ModuleDescription::Failed {
            path: "nodes/missing.py".into(),
            attribute: None,
            error:
                "nodes/missing.py failed to import: ModuleNotFoundError: No module named 'missing'"
                    .into(),
        }
    );

    let info = host.info.clone().unwrap();
    assert_eq!(info.protocol, "fx-node-protocol-v1");
    assert_eq!(info.host.language, "python");
    assert_eq!(info.host.sdk_version, "0.0.0-fake");
    assert_eq!(host.host_info().unwrap(), info);

    let initialize = fixture.initialize();
    assert_eq!(initialize["argv"], json!(["-P", "-m", "grida.fx.host"]));
    if std::env::var_os("PYTHONSAFEPATH").is_none_or(|value| value.is_empty()) {
        assert_eq!(initialize["safe_path"], json!(SAFE_PATH_MARK));
    }
    assert_eq!(
        canonical(initialize["cwd"].as_str().unwrap()),
        canonical(&fixture.root)
    );
    let params = &initialize["params"];
    assert_eq!(params["protocol"], json!("fx-node-protocol-v1"));
    assert_eq!(
        params["engine"],
        json!({"name": "grida-fx", "version": ENGINE_VERSION})
    );
    assert_eq!(params["sources"], json!(["acme_lib"]));
    let project_root = PathBuf::from(params["project_root"].as_str().unwrap());
    assert!(project_root.is_absolute());
    assert_eq!(canonical(project_root), canonical(&fixture.root));

    let built = host.build(&build_params("build", &fixture.root)).unwrap();
    assert_eq!(built.document["fx"], json!("workflow/v1"));
    assert_eq!(built.document["seen"]["arguments"], json!({"count": "3"}));
    assert_eq!(built.takes_anchor, "workflows/demo.py");

    match host.build(&build_params("boom", &fixture.root)) {
        Err(HostFailure::Rpc(error)) => {
            assert_eq!(error.kind(), Some(ErrorCode::BuildFailed));
            assert_eq!(error.message, "boom: RuntimeError: no levels");
        }
        other => panic!("{other:?}"),
    }
    // An error answer leaves the host serving.
    host.describe(&describe_params(&[("nodes/echo.py", None)]))
        .unwrap();

    host.shutdown().unwrap();
    assert_eq!(
        fixture.log(),
        [
            "initialize",
            "describe",
            "build",
            "build",
            "describe",
            "shutdown",
            "exit",
            "status 0"
        ]
    );
    // Idempotent.
    host.shutdown().unwrap();
    assert_eq!(fixture.log().len(), 8);
}

#[test]
fn dropping_the_host_shuts_it_down() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("ok");
    {
        let mut host = fixture.host();
        host.open_project(&fixture.root, &[]).unwrap();
        host.describe(&describe_params(&[("nodes/echo.py", None)]))
            .unwrap();
    }
    assert_eq!(
        fixture.log(),
        ["initialize", "describe", "shutdown", "exit", "status 0"]
    );
}

#[test]
fn another_project_restarts_the_host() {
    if !have_python3() {
        return;
    }
    let first = Fixture::new("ok");
    let second = Fixture::new("ok");
    let mut host = first.host();
    host.open_project(&first.root, &[]).unwrap();
    host.describe(&describe_params(&[("nodes/echo.py", None)]))
        .unwrap();
    // The same project: the host keeps running.
    host.open_project(&first.root, &[]).unwrap();
    host.describe(&describe_params(&[("nodes/echo.py", None)]))
        .unwrap();
    assert_eq!(first.log(), ["initialize", "describe", "describe"]);
    // Other sources are another session.
    host.open_project(&first.root, &["lib".to_string()])
        .unwrap();
    assert_eq!(
        first.log(),
        [
            "initialize",
            "describe",
            "describe",
            "shutdown",
            "exit",
            "status 0"
        ]
    );
    host.open_project(&second.root, &[]).unwrap();
    host.describe(&describe_params(&[("nodes/echo.py", None)]))
        .unwrap();
    assert_eq!(second.log(), ["initialize", "describe"]);
    assert_eq!(
        first.log().len(),
        6,
        "the first project's host never restarted"
    );
    drop(host);
    assert_eq!(
        second.log(),
        ["initialize", "describe", "shutdown", "exit", "status 0"]
    );
}

#[test]
fn a_host_speaking_a_newer_protocol() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("mismatch-result");
    let mut host = fixture.host();
    host.open_project(&fixture.root, &[]).unwrap();
    let message = unavailable(
        host.describe(&describe_params(&[("nodes/echo.py", None)]))
            .unwrap_err(),
    );
    assert_eq!(
        message,
        format!(
            "the project's grida 9.9.9 speaks fx-node-protocol-v2 and grida-fx {ENGINE_VERSION} speaks fx-node-protocol-v1: upgrade grida-fx"
        )
    );
    assert!(host.info.is_none());
    // The host was ended: its input closed, it exited, and no describe reached it.
    assert_eq!(fixture.log(), ["initialize", "eof"]);
    // The failure stands for the project.
    assert_eq!(
        unavailable(
            host.build(&build_params("build", &fixture.root))
                .unwrap_err()
        ),
        message
    );
    assert_eq!(fixture.log(), ["initialize", "eof"]);
}

#[test]
fn a_host_refusing_the_protocol() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("mismatch-error");
    let mut host = fixture.host();
    host.open_project(&fixture.root, &[]).unwrap();
    let message = unavailable(
        host.describe(&describe_params(&[("nodes/echo.py", None)]))
            .unwrap_err(),
    );
    assert_eq!(
        message,
        format!(
            "the project's grida speaks fx-node-protocol-v0 and grida-fx {ENGINE_VERSION} speaks fx-node-protocol-v1: upgrade grida"
        )
    );
    assert_eq!(fixture.log(), ["initialize", "eof"]);
}

#[test]
fn a_host_that_exits_before_initialize() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("exit-early");
    let mut host = fixture.host();
    host.open_project(&fixture.root, &[]).unwrap();
    let message = unavailable(
        host.describe(&describe_params(&[("nodes/echo.py", None)]))
            .unwrap_err(),
    );
    assert_eq!(
        message,
        format!(
            "the Python node host ({}) exited with status 3 before it answered initialize; set GRIDA_FX_PYTHON to a Python that has the grida package",
            fixture.python.display()
        )
    );
}

#[test]
fn a_host_that_exits_during_describe() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("exit-in-describe");
    let mut host = fixture.host();
    host.open_project(&fixture.root, &[]).unwrap();
    let failure = host
        .describe(&describe_params(&[("nodes/echo.py", None)]))
        .unwrap_err();
    assert_eq!(
        unavailable(failure.clone()),
        "the node host exited with status 4 while answering describe"
    );
    // Not started again for this project.
    assert_eq!(
        host.describe(&describe_params(&[("nodes/echo.py", None)]))
            .unwrap_err(),
        failure
    );
    assert_eq!(fixture.log(), ["initialize", "describe"]);
}

#[test]
fn a_host_killed_by_a_signal_says_which() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("killed-in-describe");
    let mut host = fixture.host();
    host.open_project(&fixture.root, &[]).unwrap();
    let failure = host
        .describe(&describe_params(&[("nodes/echo.py", None)]))
        .unwrap_err();
    assert_eq!(
        unavailable(failure),
        "the node host was killed by signal 15 (SIGTERM) while answering describe"
    );
}

#[test]
fn a_host_whose_fork_keeps_its_pipes_is_seen_to_exit() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("fork-in-describe");
    let mut host = fixture.host();
    host.open_project(&fixture.root, &[]).unwrap();
    let start = Instant::now();
    let failure = host
        .describe(&describe_params(&[("nodes/echo.py", None)]))
        .unwrap_err();
    let took = start.elapsed();
    assert_eq!(
        unavailable(failure),
        "the node host exited with status 4 while answering describe"
    );
    assert!(took < Duration::from_secs(5), "{took:?}");
    let forked = std::fs::read_to_string(fixture.root.join("forked.txt")).unwrap();
    let mut gone = false;
    for _ in 0..300 {
        gone = !Command::new("kill")
            .args(["-0", forked.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        if gone {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(gone, "the forked child {forked} survived");
}

#[test]
fn a_host_request_during_describe_is_refused() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("request-in-describe");
    let mut host = fixture.host();
    host.open_project(&fixture.root, &[]).unwrap();
    let described = host
        .describe(&describe_params(&[("nodes/echo.py", None)]))
        .unwrap();
    assert_eq!(
        described.modules[0],
        ModuleDescription::Failed {
            path: "nodes/echo.py".into(),
            attribute: None,
            error: "engine answered -32601: the engine serves no capability here".into(),
        }
    );
    // The engine's answer reached the host as a response; the host stays usable.
    assert_eq!(fixture.log(), ["initialize", "describe", "response"]);
    host.shutdown().unwrap();
    assert_eq!(fixture.log()[3..], ["shutdown", "exit", "status 0"]);
}

#[test]
fn a_host_that_breaks_the_protocol() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("garbage");
    let mut host = fixture.host();
    host.open_project(&fixture.root, &[]).unwrap();
    let message = unavailable(
        host.describe(&describe_params(&[("nodes/echo.py", None)]))
            .unwrap_err(),
    );
    assert!(
        message.starts_with(
            "the node host broke the protocol while answering describe: the node host sent a message that is not I-JSON"
        ),
        "{message}"
    );
    // Its input was closed and it exited.
    assert_eq!(fixture.log(), ["initialize", "describe", "eof"]);
}

#[test]
fn a_result_fx_cannot_read() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("bad-result");
    let mut host = fixture.host();
    host.open_project(&fixture.root, &[]).unwrap();
    let message = unavailable(
        host.describe(&describe_params(&[("nodes/echo.py", None)]))
            .unwrap_err(),
    );
    assert!(
        message.starts_with("the Python node host answered describe with a result FX cannot read"),
        "{message}"
    );
    // The session still worked, so the host was shut down politely.
    assert_eq!(
        fixture.log(),
        ["initialize", "describe", "shutdown", "exit", "status 0"]
    );
}

#[test]
fn a_host_that_ignores_exit_is_killed() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new("hang-on-exit");
    let mut host = fixture.host();
    host.open_project(&fixture.root, &[]).unwrap();
    host.describe(&describe_params(&[("nodes/echo.py", None)]))
        .unwrap();
    let pid = fixture.pid();
    let start = Instant::now();
    host.shutdown().unwrap();
    let took = start.elapsed();
    assert!(took >= Duration::from_secs(5), "waited only {took:?}");
    assert!(took < Duration::from_secs(15), "waited {took:?}");
    assert_eq!(
        fixture.log(),
        ["initialize", "describe", "shutdown", "exit"]
    );
    let alive = Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive, "the host {pid} is still running");
}

#[test]
fn a_host_on_the_engines_runtime() {
    if !have_python3() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let fixture = Fixture::new("ok");
    let mut host = fixture.host().with_handle(runtime.handle().clone());
    host.open_project(&fixture.root, &[]).unwrap();
    host.describe(&describe_params(&[("nodes/echo.py", None)]))
        .unwrap();
    host.build(&build_params("build", &fixture.root)).unwrap();
    drop(host);
    assert_eq!(
        fixture.log(),
        [
            "initialize",
            "describe",
            "build",
            "shutdown",
            "exit",
            "status 0"
        ]
    );
}

/// Called where blocking would panic (inside a runtime), the host blocks on a helper thread.
#[test]
fn a_host_called_inside_a_runtime_does_not_panic() {
    if !have_python3() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let fixture = Fixture::new("exit-in-describe");
    runtime.block_on(async {
        let mut host = fixture.host();
        host.open_project(&fixture.root, &[]).unwrap();
        assert_eq!(
            unavailable(
                host.describe(&describe_params(&[("nodes/echo.py", None)]))
                    .unwrap_err()
            ),
            "the node host exited with status 4 while answering describe"
        );
    });
    let fixture = Fixture::new("ok");
    runtime.block_on(async {
        let mut host = fixture.host();
        host.open_project(&fixture.root, &[]).unwrap();
        host.describe(&describe_params(&[("nodes/echo.py", None)]))
            .unwrap();
    });
    assert_eq!(
        fixture.log(),
        ["initialize", "describe", "shutdown", "exit", "status 0"]
    );
}

/// The interpreter for the real host: `GRIDA_FX_PYTHON`, else the repository's
/// `python/.venv`.
fn real_python() -> Option<PathBuf> {
    if let Some(python) = std::env::var_os("GRIDA_FX_PYTHON").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(python));
    }
    let venv = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python/.venv/bin/python");
    venv.exists().then_some(venv)
}

#[test]
fn the_real_python_host() {
    let Some(python) = real_python() else {
        eprintln!("skipped: neither GRIDA_FX_PYTHON nor python/.venv is set up");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("acme");
    std::fs::create_dir_all(root.join("nodes")).unwrap();
    std::fs::create_dir_all(root.join("workflows")).unwrap();
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::write(
        root.join("nodes/echo.py"),
        "from grida.fx import node\n\n\n\
         @node(\"echo\", outputs={\"text\": \"text\"}, version=1)\n\
         def echo(ctx):\n    \"\"\"Echoes.\"\"\"\n    print(\"user code prints to stdout\")\n",
    )
    .unwrap();
    std::fs::write(
        root.join("workflows/demo.py"),
        "from grida.fx import Workflow\n\nprint(\"a builder module prints too\")\n\n\n\
         def build(count):\n    return Workflow(\"demo\", title=\"Demo \" + count)\n",
    )
    .unwrap();

    let mut host = PythonHost::new().with_python(python);
    host.open_project(&root, &[]).unwrap();
    let described = host
        .describe(&describe_params(&[("nodes/echo.py", Some("echo"))]))
        .unwrap();
    let ModuleDescription::Described { types, closure, .. } = &described.modules[0] else {
        panic!("{described:?}");
    };
    assert_eq!(types[0].spec.name, "echo");
    assert_eq!(types[0].spec.version, Some(1));
    assert_eq!(types[0].spec.description.as_deref(), Some("Echoes."));
    assert_eq!(closure[0].label, "nodes/echo.py");
    let info = host.info.clone().unwrap();
    assert_eq!(info.protocol, "fx-node-protocol-v1");
    assert_eq!(info.host.language, "python");

    let built = host.build(&build_params("build", &root)).unwrap();
    assert_eq!(built.document["fx"], json!("workflow/v1"));
    assert_eq!(built.document["id"], json!("demo"));
    assert_eq!(built.document["title"], json!("Demo 3"));
    assert_eq!(built.takes_anchor, "workflows/demo.py");
    host.shutdown().unwrap();
}

/// Project modules named like modules the host imports itself (`json.py`, `typing.py`, a
/// `grida/` package, …) do not replace them: the project root is not on `sys.path` until
/// `initialize` (protocol.md §1).
#[test]
fn the_real_python_host_ignores_root_modules_until_initialize() {
    let Some(python) = real_python() else {
        eprintln!("skipped: neither GRIDA_FX_PYTHON nor python/.venv is set up");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("acme");
    std::fs::create_dir_all(root.join("nodes")).unwrap();
    std::fs::create_dir_all(root.join("workflows")).unwrap();
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    for name in [
        "json",
        "typing",
        "types",
        "re",
        "platform",
        "traceback",
        "hashlib",
    ] {
        std::fs::write(
            root.join(format!("{name}.py")),
            format!("print(\"the project's {name} ran\")\n"),
        )
        .unwrap();
    }
    for package in ["grida", "email"] {
        std::fs::create_dir_all(root.join(package)).unwrap();
        std::fs::write(
            root.join(package).join("__init__.py"),
            format!("raise ImportError(\"the project's {package}\")\n"),
        )
        .unwrap();
    }
    std::fs::write(
        root.join("nodes/echo.py"),
        "from grida.fx import node\n\n\n\
         @node(\"echo\", outputs={\"text\": \"text\"})\n\
         def echo(ctx):\n    import json\n\n    return json\n",
    )
    .unwrap();
    std::fs::write(
        root.join("workflows/demo.py"),
        "from grida.fx import Workflow\n\n\n\
         def build(count):\n    return Workflow(\"demo\", title=\"Demo \" + count)\n",
    )
    .unwrap();

    let mut host = PythonHost::new().with_python(python);
    host.open_project(&root, &[]).unwrap();
    let described = host
        .describe(&describe_params(&[("nodes/echo.py", None)]))
        .unwrap();
    let ModuleDescription::Described { types, closure, .. } = &described.modules[0] else {
        panic!("{described:?}");
    };
    assert_eq!(types[0].spec.name, "echo");
    // The node's own `import json` is the project's.
    let labels: Vec<&str> = closure.iter().map(|entry| entry.label.as_str()).collect();
    assert_eq!(labels, ["json.py", "nodes/echo.py"]);
    let built = host.build(&build_params("build", &root)).unwrap();
    assert_eq!(built.document["title"], json!("Demo 3"));
    host.shutdown().unwrap();
}
