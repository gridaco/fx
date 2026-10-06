"""The Python node host, driven frame by frame as the engine drives it (spec/protocol.md)."""

from __future__ import annotations

import asyncio
import importlib.metadata
import io
import itertools
import json
import os
import platform
import queue
import shutil
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator

from grida.fx._errors import Cancelled, CeilingExceeded, EngineError
from grida.fx._protocol import (
    PROTOCOL,
    ProtocolError,
    check_value,
    encode_message,
    parse_message,
    read_message,
    write_message,
)
from grida.fx._session import Session
from grida.fx.host import SAFE_PATH_MARK, Host

REPO = Path(__file__).resolve().parents[2]
SCHEMA = json.loads(
    (REPO / "spec" / "schemas" / "fx-node-protocol-v1.schema.json").read_text("utf-8")
)
MESSAGE = Draft202012Validator(SCHEMA)
LOCAL_IDENTITY = REPO / "conformance" / "local-identity" / "in"
ENGINE = {"name": "grida-fx", "version": "0.1.0"}
TIMEOUT = 30


def _result_validator(name: str) -> Draft202012Validator:
    return Draft202012Validator({"$ref": f"#/$defs/{name}", "$defs": SCHEMA["$defs"]})


INITIALIZE_RESULT = _result_validator("initialize_result")
DESCRIBE_RESULT = _result_validator("describe_result")
BUILD_RESULT = _result_validator("build_result")
SHUTDOWN_RESULT = _result_validator("shutdown_result")


def _valid(validator: Draft202012Validator, value: Any) -> None:
    errors = [f"{list(error.path)}: {error.message}" for error in validator.iter_errors(value)]
    assert errors == [], errors


def _frame(body: bytes) -> bytes:
    return b"Content-Length: " + str(len(body)).encode() + b"\r\n\r\n" + body


# --- the project the host serves --------------------------------------------------------------

NOISY = """\
import os
import subprocess
import sys

from grida.fx import node

print("printed by print")
sys.stdout.write("written to sys.stdout\\n")
sys.stdout.flush()
os.write(1, b"written to fd 1\\n")
sys.__stdout__.write("written to sys.__stdout__\\n")
sys.__stdout__.flush()
subprocess.run([sys.executable, "-c", "print('printed by a child')"], check=True)
STDIN = sys.stdin.read()


@node("noisy", outputs={"text": "text"})
def noisy(ctx):
    return {}
"""

COUNTED = """\
from pathlib import Path

from grida.fx import node

with open(Path(__file__).parent.parent / "loads.txt", "a") as log:
    log.write("loaded\\n")


@node("counted", outputs={"text": "text"}, version=1)
def counted(ctx):
    return {}
"""

FAILS_ONCE = """\
from pathlib import Path

with open(Path(__file__).parent.parent / "failures.txt", "a") as log:
    log.write("failed\\n")
raise RuntimeError("cannot start")
"""

BUILDERS = """\
import os
from pathlib import Path

from grida.fx import Workflow

from builders.lib import make


def build(level):
    wf = Workflow("level-art", title=f"Level {level}")
    text = Path("level.txt").read_text("utf-8").strip()
    draw = wf.step("draw", uses="./nodes/n.py#echo", with_={"text": text})
    wf.outputs(text=draw.outputs.text)
    return wf


def elsewhere():
    return make()


def outside():
    import outside_lib

    return outside_lib.make()


def made_by_exec():
    namespace = {"Workflow": Workflow}
    exec("wf = Workflow('ex', title='Ex')", namespace)
    return namespace["wf"]


def where():
    wf = Workflow("where", title=os.getcwd())
    wf.step("a", uses="fx/files.copy@1")
    return wf


def boom():
    raise ValueError("no level")


def crash():
    raise RuntimeError("broken builder")


def missing_file():
    return (Path(__file__).parent / "missing.txt").read_text("utf-8")


def exits():
    raise SystemExit(3)


def interrupted():
    raise KeyboardInterrupt


def cancelled():
    import asyncio

    raise asyncio.CancelledError()


def safe_path():
    return Workflow("env", title=os.environ.get("PYTHONSAFEPATH", "unset"))


def plain():
    return {"fx": "workflow/v1"}


def not_json():
    wf = Workflow("nan", title="NaN")
    wf.step("a", uses="x", with_={"v": float("nan")})
    return wf


NOT_A_FUNCTION = 3
"""


@pytest.fixture
def project(tmp_path: Path) -> Path:
    root = tmp_path / "acme"
    shutil.copytree(LOCAL_IDENTITY, root)
    files = {
        "nodes/broken.py": "import not_a_module_xyz\n",
        "nodes/syntax.py": "x = 1\ndef (:\n",
        "nodes/bad_spec.py": (
            "from grida.fx import node\n\n\n@node('Bad')\ndef bad(ctx):\n    return {}\n"
        ),
        "nodes/noisy.py": NOISY,
        "nodes/counted.py": COUNTED,
        "nodes/fails_once.py": FAILS_ONCE,
        "nodes/many.py": (
            "from grida.fx import node\n\nfrom nodes.n import echo as alias\n\n\n"
            "@node('zeta')\ndef zeta(ctx):\n    return {}\n\n\n"
            "@node('alpha')\ndef alpha(ctx):\n    '''The first letter.'''\n    return {}\n"
        ),
        "builders/b.py": BUILDERS,
        "builders/lib.py": (
            "from grida.fx import Workflow\n\n\ndef make():\n"
            "    wf = Workflow('from-lib', title='Made in lib')\n"
            "    wf.step('a', uses='fx/files.copy@1')\n    return wf\n"
        ),
        "builders/broken.py": "raise ImportError('builder dependencies missing')\n",
        # Reads its rows at import, relative to where grida-fx runs (section 5.2).
        "builders/rows.py": (
            "from pathlib import Path\n\nfrom grida.fx import Workflow\n\n"
            "ROWS = Path('level.txt').read_text('utf-8').split()\n\n\n"
            "def build():\n    wf = Workflow('rows', title='Rows')\n    for row in ROWS:\n"
            "        wf.step(row, uses='fx/files.copy@1')\n    return wf\n"
        ),
        "nodes/interrupted.py": "raise KeyboardInterrupt\n",
        "nodes/cancelled.py": "import asyncio\n\nraise asyncio.CancelledError()\n",
        # A worker thread started at import that never ends and is not a daemon.
        "nodes/threaded.py": (
            "import threading\nimport time\n\nfrom grida.fx import node\n\n"
            "threading.Thread(target=lambda: time.sleep(3600)).start()\n\n\n"
            "@node('threaded')\ndef threaded(ctx):\n    return {}\n"
        ),
    }
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, "utf-8")
    return root


@pytest.fixture
def site(tmp_path: Path) -> Path:
    """Installed packages the project can import: a source package and a plain module."""
    folder = tmp_path / "site"
    for name, text in {
        "acme_lib/__init__.py": "from .colors import PALETTE\n",
        "acme_lib/colors.py": "PALETTE = ['red']\n",
        "outside_lib.py": (
            "from grida.fx import Workflow\n\n\ndef make():\n"
            "    wf = Workflow('outside', title='Outside')\n"
            "    wf.step('a', uses='fx/files.copy@1')\n    return wf\n"
        ),
    }.items():
        path = folder / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, "utf-8")
    return folder


# --- a host process ---------------------------------------------------------------------------


class HostProcess:
    """``python -P -m grida.fx.host`` started as the engine starts it (section 1), with pipes,
    read on a thread so a hang fails the test."""

    def __init__(self, root: Path, log: Path, env: dict[str, str] | None = None) -> None:
        self.root = root
        self.log = log
        self._stderr = open(log, "wb")  # noqa: SIM115
        env = dict(os.environ if env is None else env)
        if not env.get("PYTHONSAFEPATH"):
            env["PYTHONSAFEPATH"] = SAFE_PATH_MARK
        self.process = subprocess.Popen(
            [sys.executable, "-P", "-m", "grida.fx.host"],
            cwd=root,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._stderr,
            env=env,
        )
        self.frames: queue.Queue[bytes | None | Exception] = queue.Queue()
        self.ids = itertools.count(1)
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self) -> None:
        assert self.process.stdout is not None
        while True:
            try:
                body = read_message(self.process.stdout)
            except Exception as error:
                self.frames.put(error)
                return
            self.frames.put(body)
            if body is None:
                return

    def send_raw(self, data: bytes) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(data)
        self.process.stdin.flush()

    def send(self, message: dict[str, Any]) -> None:
        self.send_raw(_frame(json.dumps(message).encode("utf-8")))

    def receive(self) -> dict[str, Any]:
        body = self.frames.get(timeout=TIMEOUT)
        assert isinstance(body, bytes), body
        message = json.loads(body)
        _valid(MESSAGE, message)
        return message

    def request(self, method: str, params: Any = None, *, with_params: bool = True) -> Any:
        request_id = next(self.ids)
        message: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if with_params and params is not None:
            message["params"] = params
        self.send(message)
        answer = self.receive()
        assert answer["id"] == request_id
        return answer

    def notify(self, method: str) -> None:
        self.send({"jsonrpc": "2.0", "method": method})

    def initialize(self, sources: list[str] | None = None) -> dict[str, Any]:
        answer = self.request(
            "initialize",
            {
                "protocol": PROTOCOL,
                "engine": ENGINE,
                "project_root": str(self.root),
                "sources": sources or [],
            },
        )
        assert "result" in answer, answer
        return answer["result"]

    def describe(self, *targets: dict[str, str], builtins: bool = False) -> dict[str, Any]:
        answer = self.request("describe", {"targets": list(targets), "builtins": builtins})
        assert "result" in answer, answer
        _valid(DESCRIBE_RESULT, answer["result"])
        return answer["result"]

    def build(self, function: str, cwd: Path, path: str = "builders/b.py", **arguments: str) -> Any:
        return self.request(
            "build", {"path": path, "function": function, "arguments": arguments, "cwd": str(cwd)}
        )

    def stdout_ends(self) -> None:
        assert self.frames.get(timeout=TIMEOUT) is None

    def wait(self) -> int:
        return self.process.wait(timeout=TIMEOUT)

    def close_stdin(self) -> None:
        assert self.process.stdin is not None
        self.process.stdin.close()

    def stderr(self) -> str:
        self._stderr.flush()
        return self.log.read_text("utf-8", errors="replace")

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=TIMEOUT)
        for stream in (self.process.stdin, self.process.stdout):
            if stream is not None:
                try:
                    stream.close()
                except OSError:
                    pass
        self._stderr.close()


@pytest.fixture
def start(tmp_path: Path) -> Iterator[Callable[..., HostProcess]]:
    started: list[HostProcess] = []

    def launch(root: Path, env: dict[str, str] | None = None) -> HostProcess:
        host = HostProcess(root, tmp_path / f"stderr-{len(started)}.log", env)
        started.append(host)
        return host

    yield launch
    for host in started:
        host.stop()


def _with_pythonpath(folder: Path) -> dict[str, str]:
    env = dict(os.environ)
    env["PYTHONPATH"] = os.pathsep.join(
        part for part in (str(folder), env.get("PYTHONPATH", "")) if part
    )
    return env


# --- the session ------------------------------------------------------------------------------


def test_initialize(project: Path, start: Callable[..., HostProcess]) -> None:
    host = start(project)
    result = host.initialize()
    _valid(INITIALIZE_RESULT, result)
    assert result == {
        "protocol": "fx-node-protocol-v1",
        "host": {
            "language": "python",
            "version": platform.python_version(),
            "sdk_version": importlib.metadata.version("grida"),
        },
    }
    answer = host.request("shutdown")
    _valid(SHUTDOWN_RESULT, answer["result"])
    assert answer == {"jsonrpc": "2.0", "id": 2, "result": None}
    host.notify("exit")
    assert host.wait() == 0
    host.stdout_ends()


def test_the_first_frame_of_the_protocol_example_is_answered(
    project: Path, start: Callable[..., HostProcess]
) -> None:
    host = start(project)
    body = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocol": PROTOCOL,
                "engine": ENGINE,
                "project_root": str(project),
                "sources": [],
            },
        },
        separators=(",", ":"),
    ).encode()
    # Other header fields are ignored.
    host.send_raw(b"Content-Length: " + str(len(body)).encode() + b"\r\nX-Other: 1\r\n\r\n" + body)
    assert host.receive()["result"]["protocol"] == PROTOCOL


def test_a_protocol_mismatch(project: Path, start: Callable[..., HostProcess]) -> None:
    host = start(project)
    answer = host.request(
        "initialize",
        {
            "protocol": "fx-node-protocol-v2",
            "engine": {"name": "grida-fx", "version": "0.2.0"},
            "project_root": str(project),
            "sources": [],
        },
    )
    error = answer["error"]
    assert error["code"] == -32003
    assert error["data"] == {
        "engine_protocol": "fx-node-protocol-v2",
        "host_protocol": "fx-node-protocol-v1",
    }
    assert "fx-node-protocol-v2" in error["message"]
    assert "fx-node-protocol-v1" in error["message"]
    # The session is not initialized: work is still refused.
    answer = host.request("describe", {"targets": [], "builtins": False})
    assert answer["error"]["code"] == -32600
    host.notify("exit")
    assert host.wait() == 1


def test_requests_before_initialize_are_invalid(
    project: Path, start: Callable[..., HostProcess]
) -> None:
    host = start(project)
    for method, params in [
        ("describe", {"targets": [], "builtins": False}),
        ("shutdown", None),
        ("no.such.method", {}),
    ]:
        answer = host.request(method, params)
        assert answer["error"]["code"] == -32600, method
        assert answer["error"]["message"] == f"{method} came before initialize"
    host.initialize()
    answer = host.request("initialize", {"protocol": PROTOCOL})
    assert answer["error"] == {"code": -32600, "message": "initialize was already answered"}


def test_an_unknown_method(project: Path, start: Callable[..., HostProcess]) -> None:
    host = start(project)
    host.initialize()
    for method in ["no.such.method", "capability", "file.put"]:
        answer = host.request(method, {})
        assert answer["error"]["code"] == -32601
        assert answer["error"]["message"] == f"the Python node host has no method {method}"
    # The runner's methods are served (test_host_run.py): their params are checked here.
    assert host.request("run", {})["error"] == {
        "code": -32602,
        "message": "run's run_id is a non-empty string",
    }
    for method in ["tool.invoke", "agent.check"]:
        answer = host.request(method, {"run_id": "r1", "agent_id": "a"})
        assert answer["error"] == {"code": -32602, "message": "no run r1 is pending on this host"}
    # Unknown notifications, and a $/cancel that names nothing pending, are ignored.
    host.notify("$/cancel")
    host.notify("progress")
    host.send({"jsonrpc": "2.0", "method": "$/cancel", "params": {"id": 1}})
    assert host.request("shutdown")["result"] is None


def test_malformed_messages(project: Path, start: Callable[..., HostProcess]) -> None:
    host = start(project)
    host.initialize()
    for body in [
        b'{"jsonrpc": "2.0", "id": 9, "method": "describe", "params": {"x": NaN}}',
        b'{"jsonrpc": "2.0", "id": 9, "method": "shutdown", "id": 9}',
        b'{"jsonrpc": "2.0", "id": 9007199254740993, "method": "shutdown"}',
        b'{"jsonrpc": "2.0", "id": 9, "method": "\\ud800"}',
        b'{"jsonrpc": "2.0", "id": 9, "method": "shutdown", "params": 1e400}',
        b"\xff",
        b"[1,",
    ]:
        host.send_raw(_frame(body))
        answer = host.receive()
        assert answer["id"] is None, body
        assert answer["error"]["code"] == -32700, body
    for message, code in [
        ([1, 2], -32600),
        ({"jsonrpc": "1.0", "id": 3, "method": "shutdown"}, -32600),
        ({"jsonrpc": "2.0", "id": True, "method": "shutdown"}, -32600),
        ({"jsonrpc": "2.0", "id": None, "method": "shutdown"}, -32600),
        ({"jsonrpc": "2.0", "id": 4, "method": 5}, -32600),
        ({"jsonrpc": "2.0", "id": 5, "method": "describe", "params": [1]}, -32602),
        ({"jsonrpc": "2.0", "id": 6, "method": "describe", "params": {"targets": []}}, -32602),
        (
            {
                "jsonrpc": "2.0",
                "id": 7,
                "method": "describe",
                "params": {"targets": [{"path": "../x.py"}], "builtins": False},
            },
            -32602,
        ),
        (
            {
                "jsonrpc": "2.0",
                "id": 8,
                "method": "describe",
                "params": {"targets": [{"path": "/abs/x.py"}], "builtins": False},
            },
            -32602,
        ),
        ({"jsonrpc": "2.0", "id": "s", "method": "shutdown", "params": {"now": True}}, -32602),
    ]:
        host.send(message)  # type: ignore[arg-type]
        answer = host.receive()
        assert answer["error"]["code"] == code, message
    # A response the host never asked for is ignored; the session goes on.
    host.send({"jsonrpc": "2.0", "id": 1, "result": {}})
    assert host.request("shutdown")["result"] is None
    host.notify("exit")
    assert host.wait() == 0


def test_a_broken_frame_ends_the_session(project: Path, start: Callable[..., HostProcess]) -> None:
    host = start(project)
    host.send_raw(b"Content-Length: many\r\n\r\n{}")
    answer = host.receive()
    assert answer["error"]["code"] == -32700
    assert host.wait() == 1


def test_exit_without_shutdown(project: Path, start: Callable[..., HostProcess]) -> None:
    host = start(project)
    host.initialize()
    host.notify("exit")
    assert host.wait() == 1
    host.stdout_ends()


def test_end_of_input_ends_the_host(project: Path, start: Callable[..., HostProcess]) -> None:
    host = start(project)
    host.initialize()
    host.close_stdin()
    assert host.wait() == 1
    host.stdout_ends()
    after_shutdown = start(project)
    after_shutdown.initialize()
    after_shutdown.request("shutdown")
    after_shutdown.close_stdin()
    assert after_shutdown.wait() == 0
    # Work after shutdown is refused.
    third = start(project)
    third.initialize()
    third.request("shutdown")
    answer = third.request("describe", {"targets": [], "builtins": False})
    assert answer["error"] == {"code": -32600, "message": "describe came after shutdown"}
    third.close_stdin()
    assert third.wait() == 0


def _ends_promptly(host: HostProcess) -> int:
    started = time.monotonic()
    status = host.process.wait(timeout=TIMEOUT)
    assert time.monotonic() - started < 5, "the host waited for a user thread"
    return status


def test_a_thread_user_code_left_running_does_not_keep_the_host(
    project: Path, start: Callable[..., HostProcess]
) -> None:
    host = start(project)
    host.initialize()
    (module,) = host.describe({"path": "nodes/threaded.py"})["modules"]
    assert [entry["attribute"] for entry in module["types"]] == ["threaded"]
    assert host.request("shutdown")["result"] is None
    host.notify("exit")
    assert _ends_promptly(host) == 0
    host.stdout_ends()
    # End of input without shutdown ends it too.
    second = start(project)
    second.initialize()
    second.describe({"path": "nodes/threaded.py"})
    second.close_stdin()
    assert _ends_promptly(second) == 1


#: Root modules named like what the host imports itself: before initialize the project root is
#: not on sys.path (``-P``), so none of them replaces the host's own.
SHADOWS = [
    "ast",
    "base64",
    "calendar",
    "copy",
    "csv",
    "dataclasses",
    "datetime",
    "decimal",
    "enum",
    "hashlib",
    "inspect",
    "json",
    "platform",
    "quopri",
    "random",
    "re",
    "socket",
    "string",
    "traceback",
    "types",
    "typing",
    "zipfile",
]


def test_project_modules_named_like_the_hosts_imports(
    project: Path, work: Path, start: Callable[..., HostProcess]
) -> None:
    for name in SHADOWS:
        (project / f"{name}.py").write_text(
            f"import sys\nprint('{name} ran')\nsys.stderr.write('shadow {name} ran\\n')\n",
            "utf-8",
        )
    for package in ["email", "grida"]:
        (project / package).mkdir()
        (project / package / "__init__.py").write_text(
            f"print('{package} ran')\nraise ImportError('the project {package}')\n", "utf-8"
        )
    (project / "nodes" / "uses_json.py").write_text(
        "from grida.fx import node\n\n\n@node('reads')\ndef reads(ctx):\n"
        "    import json\n\n    return json\n",
        "utf-8",
    )
    host = start(project)
    result = host.initialize()
    assert result["host"]["sdk_version"] == importlib.metadata.version("grida")
    described = host.describe({"path": "nodes/n.py"}, {"path": "nodes/uses_json.py"})
    n, uses_json = described["modules"]
    assert [entry["attribute"] for entry in n["types"]] == ["echo", "pinned"]
    # The project's json.py is the one the node imports.
    assert [entry["label"] for entry in uses_json["closure"]] == [
        "json.py",
        "nodes/uses_json.py",
    ]
    assert host.build("build", work, level="one")["result"]["document"]["id"] == "level-art"
    assert host.build("boom", work)["error"]["code"] == -32005
    assert host.describe({"path": "nodes/none.py"})["modules"][0]["error"] == (
        "no module file nodes/none.py"
    )
    assert host.request("shutdown")["result"] is None
    host.notify("exit")
    assert host.wait() == 0
    host.stdout_ends()
    assert " ran" not in host.stderr()


def test_the_engines_safe_path_mark_is_not_passed_on(
    project: Path, work: Path, start: Callable[..., HostProcess]
) -> None:
    host = start(project)
    host.initialize()
    assert host.build("safe_path", work)["result"]["document"]["title"] == "unset"
    # A value the user set is theirs.
    env = dict(os.environ, PYTHONSAFEPATH="1")
    mine = start(project, env)
    mine.initialize()
    assert mine.build("safe_path", work)["result"]["document"]["title"] == "1"


# --- describe ---------------------------------------------------------------------------------


def test_describe_a_good_module(project: Path, start: Callable[..., HostProcess]) -> None:
    host = start(project)
    host.initialize()
    result = host.describe({"path": "nodes/n.py"})
    assert result["builtins"] == []
    (module,) = result["modules"]
    assert module["path"] == "nodes/n.py"
    assert [entry["attribute"] for entry in module["types"]] == ["echo", "pinned"]
    echo = module["types"][0]["spec"]
    assert echo == {
        "name": "echo",
        "inputs": {},
        "params": {"text": {"type": "string"}},
        "outputs": {"text": "text"},
        "judge": False,
        "calls": {},
        "resources": ["prompts/r.md"],
        "tools": [],
        "view": None,
        "version": None,
        "retry": "service",
    }
    assert module["types"][1]["spec"]["version"] == 2
    assert module["closure"] == [
        {"label": "nodes/helper.py", "path": str((project / "nodes" / "helper.py").resolve())},
        {"label": "nodes/n.py", "path": str((project / "nodes" / "n.py").resolve())},
    ]


def test_describe_one_attribute(project: Path, start: Callable[..., HostProcess]) -> None:
    host = start(project)
    host.initialize()
    result = host.describe(
        {"path": "nodes/n.py", "attribute": "pinned"},
        {"path": "nodes/n.py", "attribute": "missing"},
        {"path": "nodes/n.py", "attribute": "helper"},
        {"path": "nodes/many.py"},
        {"path": "nodes/many.py", "attribute": "alias"},
    )
    pinned, missing, helper, many, alias = result["modules"]
    assert [entry["attribute"] for entry in pinned["types"]] == ["pinned"]
    assert missing == {
        "path": "nodes/n.py",
        "attribute": "missing",
        "error": "nodes/n.py: missing is not declared with @node",
    }
    assert helper["error"] == "nodes/n.py: helper is not declared with @node"
    # Module order, aliases of imported types included; the docstring is the description.
    assert [entry["attribute"] for entry in many["types"]] == ["alias", "zeta", "alpha"]
    assert many["types"][2]["spec"]["description"] == "The first letter."
    assert [entry["label"] for entry in many["closure"]] == [
        "nodes/helper.py",
        "nodes/many.py",
        "nodes/n.py",
    ]
    assert alias["types"][0]["spec"]["name"] == "echo"


def test_describe_modules_that_fail(project: Path, start: Callable[..., HostProcess]) -> None:
    host = start(project)
    host.initialize()
    (project / "nodes" / "link.py").symlink_to(project.parent / "elsewhere.py")
    (project.parent / "elsewhere.py").write_text("x = 1\n", "utf-8")
    result = host.describe(
        {"path": "nodes/broken.py"},
        {"path": "nodes/broken.py", "attribute": "anything"},
        {"path": "nodes/syntax.py"},
        {"path": "nodes/bad_spec.py"},
        {"path": "nodes/none.py"},
        {"path": "nodes/link.py"},
        {"path": "prompts/r.md"},
    )
    errors = [(entry.get("attribute"), entry["error"]) for entry in result["modules"]]
    assert errors == [
        (
            None,
            "nodes/broken.py failed to import: ModuleNotFoundError: "
            "No module named 'not_a_module_xyz'",
        ),
        (
            "anything",
            "nodes/broken.py failed to import: ModuleNotFoundError: "
            "No module named 'not_a_module_xyz'",
        ),
        (
            None,
            "nodes/syntax.py failed to import: SyntaxError: "
            "invalid syntax (nodes/syntax.py, line 2)",
        ),
        (
            None,
            "nodes/bad_spec.py failed to import: SpecError: node type name 'Bad' must be "
            "lower_snake words joined by .",
        ),
        (None, "no module file nodes/none.py"),
        (None, "nodes/link.py is outside the project"),
        (None, "cannot import prompts/r.md"),
    ]
    for entry in result["modules"]:
        assert str(project) not in entry["error"]
        assert "types" not in entry


def test_whatever_a_module_raises_at_import_is_its_error(
    project: Path, start: Callable[..., HostProcess]
) -> None:
    host = start(project)
    host.initialize()
    result = host.describe(
        {"path": "nodes/interrupted.py"}, {"path": "nodes/cancelled.py"}, {"path": "nodes/n.py"}
    )
    interrupted, cancelled, n = result["modules"]
    assert interrupted == {
        "path": "nodes/interrupted.py",
        "error": "nodes/interrupted.py failed to import: KeyboardInterrupt",
    }
    assert cancelled == {
        "path": "nodes/cancelled.py",
        "error": "nodes/cancelled.py failed to import: CancelledError",
    }
    assert [entry["attribute"] for entry in n["types"]] == ["echo", "pinned"]
    # The host keeps serving.
    assert host.request("shutdown")["result"] is None
    host.notify("exit")
    assert host.wait() == 0


def test_each_module_loads_once_per_session(
    project: Path, start: Callable[..., HostProcess]
) -> None:
    host = start(project)
    host.initialize()
    first = host.describe({"path": "nodes/counted.py"}, {"path": "nodes/fails_once.py"})
    second = host.describe(
        {"path": "nodes/counted.py", "attribute": "counted"}, {"path": "nodes/fails_once.py"}
    )
    assert first["modules"][0]["types"] == second["modules"][0]["types"]
    assert first["modules"][1]["error"] == second["modules"][1]["error"]
    assert first["modules"][1]["error"] == (
        "nodes/fails_once.py failed to import: RuntimeError: cannot start"
    )
    assert (project / "loads.txt").read_text("utf-8") == "loaded\n"
    assert (project / "failures.txt").read_text("utf-8") == "failed\n"
    # A new session loads again.
    again = start(project)
    again.initialize()
    again.describe({"path": "nodes/counted.py"})
    assert (project / "loads.txt").read_text("utf-8") == "loaded\nloaded\n"


def test_what_a_module_prints_stays_off_the_protocol(
    project: Path, start: Callable[..., HostProcess]
) -> None:
    host = start(project)
    host.initialize()
    result = host.describe({"path": "nodes/noisy.py"})
    assert [entry["attribute"] for entry in result["modules"][0]["types"]] == ["noisy"]
    # The module read stdin and got nothing: the protocol input is not its stdin.
    assert host.request("shutdown")["result"] is None
    host.notify("exit")
    assert host.wait() == 0
    host.stdout_ends()
    log = host.stderr()
    for line in [
        "printed by print",
        "written to sys.stdout",
        "written to fd 1",
        "written to sys.__stdout__",
        "printed by a child",
    ]:
        assert line in log


def test_a_source_package_enters_the_closure(
    project: Path, site: Path, start: Callable[..., HostProcess]
) -> None:
    (project / "nodes" / "uses_lib.py").write_text(
        "from grida.fx import node\nfrom acme_lib import colors\n\n\n"
        "@node('paint')\ndef paint(ctx):\n    return {}\n",
        "utf-8",
    )
    host = start(project, _with_pythonpath(site))
    host.initialize(sources=["acme_lib"])
    (module,) = host.describe({"path": "nodes/uses_lib.py"})["modules"]
    assert module["closure"] == [
        {"label": "acme_lib/__init__.py", "path": str((site / "acme_lib/__init__.py").resolve())},
        {"label": "acme_lib/colors.py", "path": str((site / "acme_lib/colors.py").resolve())},
        {"label": "nodes/uses_lib.py", "path": str((project / "nodes/uses_lib.py").resolve())},
    ]


def test_builtins_are_declared_by_the_engine(
    project: Path, start: Callable[..., HostProcess]
) -> None:
    host = start(project)
    host.initialize()
    assert host.describe(builtins=True) == {"modules": [], "builtins": []}


# --- build ------------------------------------------------------------------------------------


@pytest.fixture
def work(tmp_path: Path) -> Path:
    folder = tmp_path / "work"
    folder.mkdir()
    (folder / "level.txt").write_text("kitewharf\n", "utf-8")
    return folder


def test_build(project: Path, work: Path, start: Callable[..., HostProcess]) -> None:
    host = start(project)
    host.initialize()
    answer = host.build("build", work, level="one")
    result = answer["result"]
    _valid(BUILD_RESULT, result)
    assert result == {
        "document": {
            "fx": "workflow/v1",
            "id": "level-art",
            "title": "Level one",
            "steps": {"draw": {"uses": "./nodes/n.py#echo", "with": {"text": "kitewharf"}}},
            "outputs": {"text": "${{ steps.draw.outputs.text }}"},
        },
        "takes_anchor": "builders/b.py",
    }


def test_build_runs_in_cwd_and_restores_it(
    project: Path, work: Path, start: Callable[..., HostProcess]
) -> None:
    (project / "nodes" / "cwd_probe.py").write_text(
        "import os\n\nfrom grida.fx import node\n\n\ndef probe(ctx):\n    return {}\n\n\n"
        "probe.__doc__ = os.getcwd()\nprobe = node('cwd_probe')(probe)\n",
        "utf-8",
    )
    host = start(project)
    host.initialize()
    title = host.build("where", work)["result"]["document"]["title"]
    assert os.path.realpath(title) == os.path.realpath(work)
    # A relative read resolves against cwd, not the project root.
    answer = host.build("build", project, level="x")
    assert answer["error"] == {
        "code": -32005,
        "message": "build: FileNotFoundError: [Errno 2] No such file or directory: 'level.txt'",
    }
    # The builder's working directory does not leak past the call.
    (probe,) = host.describe({"path": "nodes/cwd_probe.py"})["modules"]
    description = probe["types"][0]["spec"]["description"]
    assert os.path.realpath(description) == os.path.realpath(project)


CWD_PROBE = (
    "import os\n\nfrom grida.fx import node\n\n\ndef probe(ctx):\n    return {}\n\n\n"
    "probe.__doc__ = os.getcwd()\nprobe = node('cwd_probe')(probe)\n"
)


def _host_cwd(host: HostProcess) -> str:
    """The host's working directory, as a module loaded now sees it."""
    (probe,) = host.describe({"path": "nodes/cwd_probe.py"})["modules"]
    return os.path.realpath(probe["types"][0]["spec"]["description"])


def test_a_builder_module_loads_in_cwd(
    project: Path, work: Path, start: Callable[..., HostProcess]
) -> None:
    (project / "nodes" / "cwd_probe.py").write_text(CWD_PROBE, "utf-8")
    (work / "level.txt").write_text("alpha beta\n", "utf-8")
    # A file of the same name in the project root is not the one read.
    (project / "level.txt").write_text("gamma\n", "utf-8")
    host = start(project)
    host.initialize()
    document = host.build("build", work, "builders/rows.py")["result"]["document"]
    assert list(document["steps"]) == ["alpha", "beta"]
    # Without the file in cwd the module fails to load, and the working directory is restored.
    (project / "level.txt").unlink()
    other = start(project)
    other.initialize()
    answer = other.build("build", work.parent, "builders/rows.py")
    assert answer["error"] == {
        "code": -32004,
        "message": "builders/rows.py failed to import: FileNotFoundError: [Errno 2] "
        "No such file or directory: 'level.txt'",
    }
    assert _host_cwd(other) == os.path.realpath(project)


def test_takes_anchors(
    project: Path, work: Path, site: Path, start: Callable[..., HostProcess]
) -> None:
    host = start(project, _with_pythonpath(site))
    host.initialize()
    # The module that called Workflow(...), when it is in the project.
    assert host.build("elsewhere", work)["result"]["takes_anchor"] == "builders/lib.py"
    # Otherwise the builder file.
    assert host.build("outside", work)["result"]["takes_anchor"] == "builders/b.py"
    assert host.build("made_by_exec", work)["result"]["takes_anchor"] == "builders/b.py"


@pytest.mark.parametrize(
    ("function", "path", "arguments", "code", "message"),
    [
        ("boom", "builders/b.py", {}, -32005, "boom: ValueError: no level"),
        ("crash", "builders/b.py", {}, -32005, "crash: RuntimeError: broken builder"),
        ("exits", "builders/b.py", {}, -32005, "exits: SystemExit: 3"),
        ("interrupted", "builders/b.py", {}, -32005, "interrupted: KeyboardInterrupt"),
        ("cancelled", "builders/b.py", {}, -32005, "cancelled: CancelledError"),
        (
            "build",
            "builders/b.py",
            {},
            -32005,
            "build: TypeError: build() missing 1 required positional argument: 'level'",
        ),
        (
            "boom",
            "builders/b.py",
            {"extra": "x"},
            -32005,
            "boom: TypeError: boom() got an unexpected keyword argument 'extra'",
        ),
        ("plain", "builders/b.py", {}, -32005, "plain returned dict, not a grida.fx.Workflow"),
        (
            "not_json",
            "builders/b.py",
            {},
            -32005,
            "not_json: ValueError: at /steps/a/with/v: nan is not a JSON number",
        ),
        ("nope", "builders/b.py", {}, -32004, "builders/b.py has no function nope"),
        (
            "NOT_A_FUNCTION",
            "builders/b.py",
            {},
            -32004,
            "builders/b.py has no function NOT_A_FUNCTION",
        ),
        ("build", "builders/none.py", {}, -32004, "no builder file builders/none.py"),
        (
            "build",
            "builders/broken.py",
            {},
            -32004,
            "builders/broken.py failed to import: ImportError: builder dependencies missing",
        ),
    ],
)
def test_build_failures(
    project: Path,
    work: Path,
    start: Callable[..., HostProcess],
    function: str,
    path: str,
    arguments: dict[str, str],
    code: int,
    message: str,
) -> None:
    host = start(project)
    host.initialize()
    answer = host.build(function, work, path, **arguments)
    assert answer["error"] == {"code": code, "message": message}
    # The host keeps serving.
    title = host.build("where", work)["result"]["document"]["title"]
    assert os.path.realpath(title) == os.path.realpath(work)


def test_a_builder_error_names_project_files_relatively(
    project: Path, start: Callable[..., HostProcess]
) -> None:
    host = start(project)
    host.initialize()
    answer = host.build("missing_file", project)
    assert answer["error"] == {
        "code": -32005,
        "message": "missing_file: FileNotFoundError: [Errno 2] No such file or directory: "
        "'builders/missing.txt'",
    }


def test_build_params_are_checked(
    project: Path, work: Path, start: Callable[..., HostProcess]
) -> None:
    host = start(project)
    host.initialize()
    for params in [
        {"path": "builders/b.py", "function": "build", "arguments": {}, "cwd": "relative"},
        {"path": "builders/b.py", "function": "a-b", "arguments": {}, "cwd": str(work)},
        {"path": "builders/b.py", "function": "build", "arguments": {"a": 1}, "cwd": str(work)},
        {"path": "./builders/b.py", "function": "build", "arguments": {}, "cwd": str(work)},
        {"path": "builders/b.py", "function": "where", "arguments": {}, "cwd": str(work / "no")},
    ]:
        answer = host.request("build", params)
        assert answer["error"]["code"] == -32602, params


# --- in-process: the session object and the transport ----------------------------------------


def _session(*messages: dict[str, Any] | bytes) -> tuple[int, list[dict[str, Any]]]:
    data = b"".join(
        message if isinstance(message, bytes) else _frame(encode_message(message))
        for message in messages
    )
    writer = io.BytesIO()
    path = list(sys.path)
    try:
        status = Host(io.BytesIO(data), writer).serve()
    finally:
        sys.path[:] = path
    out = io.BytesIO(writer.getvalue())
    answers = []
    while (body := read_message(out)) is not None:
        answers.append(json.loads(body))
    return status, answers


def _initialize(root: Path, request_id: int = 1) -> dict[str, Any]:
    params = {"protocol": PROTOCOL, "engine": ENGINE, "project_root": str(root), "sources": []}
    return {"jsonrpc": "2.0", "id": request_id, "method": "initialize", "params": params}


def test_the_host_object_serves_a_session(project: Path) -> None:
    status, answers = _session(
        _initialize(project),
        {"jsonrpc": "2.0", "id": "two", "method": "shutdown"},
        {"jsonrpc": "2.0", "method": "exit"},
        _initialize(project, 3),  # never read: the session has ended
    )
    assert status == 0
    assert [answer["id"] for answer in answers] == [1, "two"]
    assert answers[1]["result"] is None


def test_initialize_puts_the_project_root_first(tmp_path: Path) -> None:
    root = tmp_path / "root"
    root.mkdir()
    host = Host(io.BytesIO(), io.BytesIO())
    path = list(sys.path)
    try:
        sys.path.append(str(root.resolve()))
        host.initialize(
            {"protocol": PROTOCOL, "engine": ENGINE, "project_root": str(root), "sources": ["a"]}
        )
        assert sys.path[0] == str(root.resolve())
        assert sys.path.count(str(root.resolve())) == 1
        assert host.sources == ["a"]
    finally:
        sys.path[:] = path


def test_initialize_refuses_a_bad_project_root(tmp_path: Path) -> None:
    status, answers = _session(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocol": PROTOCOL,
                "engine": ENGINE,
                "project_root": str(tmp_path / "missing"),
                "sources": [],
            },
        },
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "initialize",
            "params": {
                "protocol": PROTOCOL,
                "engine": ENGINE,
                "project_root": "rel",
                "sources": [],
            },
        },
        {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "initialize",
            "params": {
                "protocol": PROTOCOL,
                "engine": ENGINE,
                "project_root": str(tmp_path),
                "sources": ["not-a-name"],
            },
        },
        {"jsonrpc": "2.0", "id": 4, "method": "initialize", "params": {"engine": ENGINE}},
    )
    assert status == 1
    assert [answer["error"]["code"] for answer in answers] == [-32602] * 4


def test_read_message() -> None:
    stream = io.BytesIO(
        _frame(b'{"a":1}') + b"content-length:  2 \nX: y\r\n\r\n[]" + _frame('"é"'.encode())
    )
    assert read_message(stream) == b'{"a":1}'
    assert read_message(stream) == b"[]"
    assert read_message(stream) == '"é"'.encode()
    assert read_message(stream) is None


@pytest.mark.parametrize(
    "data",
    [
        b"Content-Length: 5\r\n\r\n{}",
        b"Content-Length: 2\r\n",
        b"Content-Length",
        b"X-Only: 1\r\n\r\n{}",
        b"Content-Length: -2\r\n\r\n",
        b"Content-Length: 1_0\r\n\r\n",
        b"Content-Length: 2\r\nContent-Length: 3\r\n\r\n{}",
        b"no colon here\r\n\r\n",
        b"Content-Length: " + b"9" * 10000 + b"\r\n\r\n",
    ],
)
def test_read_message_refuses_broken_frames(data: bytes) -> None:
    with pytest.raises(ProtocolError):
        read_message(io.BytesIO(data))


def test_write_message() -> None:
    stream = io.BytesIO()
    write_message(stream, {"jsonrpc": "2.0", "id": 1, "result": {"text": "é", "n": 0.5}})
    body = '{"jsonrpc":"2.0","id":1,"result":{"text":"é","n":0.5}}'.encode()
    assert stream.getvalue() == b"Content-Length: " + str(len(body)).encode() + b"\r\n\r\n" + body
    for value in [float("nan"), float("inf"), {1: "a"}, "\ud800", 2**53 + 1, {"a": {1, 2}}]:
        with pytest.raises(ValueError):
            write_message(io.BytesIO(), {"result": value})


@pytest.mark.parametrize(
    "body",
    [
        b"NaN",
        b"[Infinity]",
        b'{"a": -Infinity}',
        b"1e400",
        b"9007199254740993",
        b"1152921504606846976",
        b"1" + b"0" * 5000,
        b'"\\ud800"',
        b'{"\\udc00": 1}',
        b'{"a": 1, "\\u0061": 2}',
        b"\xef\xbb\xbf{}",
        b"\xff",
        b"[1,]",
        b"[" * 100000,
    ],
)
def test_parse_message_refuses_what_is_not_i_json(body: bytes) -> None:
    with pytest.raises(ProtocolError):
        parse_message(body)


def test_parse_message_accepts_the_edges() -> None:
    assert parse_message(b"[9007199254740992, -9007199254740992, 10000000000000000, -0]") == [
        2**53,
        -(2**53),
        10**16,
        0,
    ]
    assert parse_message(b'"\\ud83d\\ude00"') == "\U0001f600"
    assert parse_message(b'{"a": 1.5e3}') == {"a": 1500.0}


def test_check_value_names_where() -> None:
    check_value({"a": [1, "b", None, True, 0.5, {"c": (1, 2)}]})
    with pytest.raises(ValueError, match="^at /a/1/b~1c: nan is not a JSON number$"):
        check_value({"a": [0, {"b/c": float("nan")}]})
    with pytest.raises(ValueError, match="a set is not a JSON value"):
        check_value({"a": {1}})
    looped: dict[str, Any] = {}
    looped["self"] = looped
    with pytest.raises(ValueError, match="^at /self: the value holds itself$"):
        check_value(looped)
    shared = [1]
    check_value({"a": shared, "b": [shared, (shared,)]})


def _nested(levels: int) -> list[Any]:
    value: list[Any] = []
    for _ in range(levels - 1):
        value = [value]
    return value


def test_check_value_refuses_what_the_engine_cannot_read() -> None:
    # The engine's reader takes a value inside at most 512 lists and objects of its message.
    check_value(_nested(513))  # the innermost list, empty, sits inside 512
    check_value([_nested(512)])
    with pytest.raises(
        ValueError, match="^at /0/0/0/0/…: the value nests deeper than the 512 levels"
    ):
        check_value(_nested(514))
    with pytest.raises(ValueError, match="the value nests deeper than the 512 levels"):
        check_value([[1]], depth=511)
    check_value([[]], depth=511)
    # A request's params sit one level inside the message.
    check_value({"value": _nested(511)}, depth=1)
    with pytest.raises(ValueError, match="nests deeper"):
        check_value({"value": _nested(512)}, depth=1)


def test_a_request_the_engine_cannot_read_is_refused_before_it_is_sent() -> None:
    written = io.BytesIO()
    session = Session(io.BytesIO(), written)
    with pytest.raises(ValueError, match="^fact: at /value/0/0/0/…: the value nests deeper"):
        session.request("r1", "fact", {"name": "x", "value": _nested(600)})
    assert written.getvalue() == b""
    assert session.pending("r1") == 0
    # The deepest value that fits is sent, and the engine's reader takes it.
    session.request("r1", "fact", {"name": "x", "value": _nested(511)})
    body = written.getvalue().split(b"\r\n\r\n", 1)[1]
    assert parse_message(body)["params"]["name"] == "x"


def test_build_in_process_restores_the_working_directory(
    project: Path, work: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(project)
    path = list(sys.path)
    host = Host(io.BytesIO(), io.BytesIO())
    try:
        host.initialize(
            {"protocol": PROTOCOL, "engine": ENGINE, "project_root": str(project), "sources": []}
        )
        result = host.build(
            {"path": "builders/lib.py", "function": "make", "arguments": {}, "cwd": str(work)}
        )
    finally:
        sys.path[:] = path
        sys.modules.pop("builders", None)
        sys.modules.pop("builders.lib", None)
    assert os.path.realpath(os.getcwd()) == os.path.realpath(project)
    assert result["takes_anchor"] == "builders/lib.py"
    assert result["document"]["id"] == "from-lib"


# --- the session's requests to the engine (grida.fx._session) ---------------------------------


class _Pipe(io.RawIOBase):
    """A writer whose frames a test reads back."""

    def __init__(self) -> None:
        self.data = bytearray()
        self.lock = threading.Lock()

    def writable(self) -> bool:
        return True

    def write(self, data: Any) -> int:
        with self.lock:
            self.data += bytes(data)
        return len(data)

    def messages(self) -> list[dict[str, Any]]:
        with self.lock:
            stream = io.BytesIO(bytes(self.data))
        found = []
        while (body := read_message(stream)) is not None:
            found.append(json.loads(body))
        return found


def _new_session() -> tuple[Session, _Pipe]:
    pipe = _Pipe()
    return Session(io.BytesIO(), pipe), pipe  # type: ignore[arg-type]


def test_host_requests_are_numbered_and_resolved_by_their_answers() -> None:
    session, pipe = _new_session()
    first = session.request("r1", "fact", {"name": "a", "value": 1})
    second = session.request("r1", "file.put", {"json": {"x": [1]}})
    assert [message["id"] for message in pipe.messages()] == [1, 2]
    assert pipe.messages()[0] == {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "fact",
        "params": {"run_id": "r1", "name": "a", "value": 1},
    }
    for message in pipe.messages():
        _valid(MESSAGE, message)
    assert session.pending("r1") == 2
    # Answers resolve futures in any order; the reader thread sets them itself.
    assert session.deliver({"jsonrpc": "2.0", "id": 2, "result": {"digest": "d"}})
    assert second.result(timeout=1) == {"digest": "d"}
    assert not first.done()
    assert session.deliver(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32014, "message": "no money", "data": {"remaining_usd": 0}},
        }
    )
    with pytest.raises(CeilingExceeded, match="^no money$") as raised:
        first.result(timeout=1)
    assert raised.value.data == {"remaining_usd": 0}
    assert session.pending("r1") == 0
    # Answers to nothing pending are not delivered.
    for request_id in (1, 3, True, None, [1], "1"):
        assert not session.deliver({"jsonrpc": "2.0", "id": request_id, "result": {}})


def test_malformed_answers_are_engine_errors() -> None:
    session, _ = _new_session()
    bad_error = session.request("r1", "fact", {"name": "a", "value": 1})
    neither = session.request("r1", "fact", {"name": "b", "value": 1})
    session.deliver({"jsonrpc": "2.0", "id": 1, "error": {"code": "x"}})
    session.deliver({"jsonrpc": "2.0", "id": 2})
    with pytest.raises(EngineError, match="fact with a malformed error") as raised:
        bad_error.result(timeout=1)
    assert raised.value.code == -32099
    with pytest.raises(EngineError, match="neither a result nor an error"):
        neither.result(timeout=1)


def test_a_request_that_is_not_i_json_is_not_sent() -> None:
    session, pipe = _new_session()
    with pytest.raises(ValueError, match=r"^fact: at /value: nan is not a JSON number$"):
        session.request("r1", "fact", {"name": "a", "value": float("nan")})
    assert pipe.messages() == [] and session.pending("r1") == 0
    assert session.request("r1", "fact", {"name": "a", "value": 1}) is not None
    assert pipe.messages()[0]["id"] == 1


def test_a_channel_blocks_until_the_reader_delivers() -> None:
    session, pipe = _new_session()
    channel = session.channel("r7")
    results: list[Any] = []
    worker = threading.Thread(target=lambda: results.append(channel.request("fact", {"x": 1})))
    worker.start()
    deadline = time.monotonic() + TIMEOUT
    while not pipe.messages() and time.monotonic() < deadline:
        time.sleep(0.01)
    assert worker.is_alive()
    session.deliver({"jsonrpc": "2.0", "id": 1, "result": {}})
    worker.join(TIMEOUT)
    assert results == [{}]
    channel.notify("progress", {"text": "half", "fraction": 0.5})
    assert pipe.messages()[-1] == {
        "jsonrpc": "2.0",
        "method": "progress",
        "params": {"run_id": "r7", "text": "half", "fraction": 0.5},
    }

    async def awaited() -> Any:
        pending = asyncio.ensure_future(channel.request_async("fact", {"y": 2}))
        await asyncio.sleep(0)
        session.deliver({"jsonrpc": "2.0", "id": 2, "result": {"ok": True}})
        return await pending

    assert asyncio.run(awaited()) == {"ok": True}


def test_settle_waits_for_answers_even_when_the_waiter_gave_up() -> None:
    session, _ = _new_session()
    channel = session.channel("r1")

    async def scenario() -> list[str]:
        order: list[str] = []
        waiter = asyncio.ensure_future(channel.request_async("capability", {"capability": "x"}))
        await asyncio.sleep(0)
        waiter.cancel()  # the body stopped waiting; the request is still pending
        settled = asyncio.ensure_future(session.settle("r1"))
        await asyncio.sleep(0.05)
        assert not settled.done()
        order.append("answered")
        session.deliver({"jsonrpc": "2.0", "id": 1, "result": {}})  # a late answer is harmless
        await asyncio.wait_for(settled, TIMEOUT)
        order.append("settled")
        await asyncio.wait_for(session.settle("r2"), TIMEOUT)  # nothing pending for r2
        return order

    assert asyncio.run(scenario()) == ["answered", "settled"]


def test_closing_the_session_fails_what_is_pending() -> None:
    session, pipe = _new_session()
    pending = session.request("r1", "fact", {"name": "a", "value": 1})
    session.close()
    with pytest.raises(Cancelled, match="the engine ended the session"):
        pending.result(timeout=1)
    later = session.request("r1", "fact", {"name": "b", "value": 1})
    with pytest.raises(Cancelled):
        later.result(timeout=1)
    assert len(pipe.messages()) == 1
    assert session.pending("r1") == 0


def test_the_body_loop_runs_on_a_thread_of_its_own() -> None:
    session, _ = _new_session()
    loop = session.loop()
    assert session.loop() is loop
    names = asyncio.run_coroutine_threadsafe(_thread_name(), loop).result(TIMEOUT)
    assert names == "grida.fx body loop"


async def _thread_name() -> str:
    return threading.current_thread().name


# --- in-process: bodies of built-ins, and their outputs ----------------------------------------


class _Recorder:
    """A channel that answers ``file.put`` with a ref named after its params."""

    def __init__(self) -> None:
        self.puts: list[dict[str, Any]] = []

    async def request_async(self, method: str, params: dict[str, Any]) -> Any:
        assert method == "file.put"
        self.puts.append(params)
        return {"digest": "f" * 64, "kind": params["kind"], "size": 1, "name": params["name"]}


def test_a_builtin_body_comes_from_grida_fx_std(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from grida.fx import std
    from grida.fx._ctx import Ctx, InputFile
    from grida.fx.host import _Refusal

    def files_copy(ctx: Any) -> dict[str, Any]:
        return {}

    monkeypatch.setattr(
        std, "body_of", lambda builtin: files_copy if builtin == "fx/files.copy@1" else None
    )
    host = Host(io.BytesIO(), io.BytesIO())
    assert host._body({"builtin": "fx/files.copy@1"}) == (files_copy, None, "files.copy")
    with pytest.raises(_Refusal) as refused:
        host._body({"builtin": "fx/files.copy@2"})
    assert refused.value.code == -32004
    assert refused.value.message.endswith("has no body for fx/files.copy@2")
    # A built-in's ports are the engine's: a dict on any of them is a keyed collection, and the
    # engine checks the shape.
    channel = _Recorder()
    run = {
        "instance": {"id": "pack#1", "path": "pack", "step": "pack", "key": None, "take": [1]},
        "work_dir": str(tmp_path),
    }
    ctx = Ctx(run, channel)  # type: ignore[arg-type]
    ref = {"digest": "a" * 64, "kind": "json", "size": 2, "name": "m.json", "path": "/s/a"}
    outputs = asyncio.run(
        host._outputs(
            ctx,
            None,
            "package",
            {"files": {"b/x.json": InputFile(ref), "a.txt": ctx.out.text("a")}, "none": None},
        )
    )
    assert outputs == {
        "files": {
            "collection": [
                ["b/x.json", {"file": ref}],
                [
                    "a.txt",
                    {
                        "file": {
                            "digest": "f" * 64,
                            "kind": "text/plain",
                            "size": 1,
                            "name": "pack/files[a.txt]",
                        }
                    },
                ],
            ]
        },
        "none": None,
    }
    assert channel.puts == [{"base64": "YQ==", "kind": "text/plain", "name": "pack/files[a.txt]"}]


def test_a_failure_keeps_only_facts_and_marks_the_engine_can_read(tmp_path: Path) -> None:
    from grida.fx._ctx import Ctx
    from grida.fx.host import _kept

    run = {
        "instance": {"id": "a#1", "path": "a", "step": "a", "key": None, "take": [1]},
        "work_dir": str(tmp_path),
    }
    ctx = Ctx(run, _Recorder())  # type: ignore[arg-type]
    ctx.facts.update({"score": 1, "cost_usd": 3})
    ctx.marks.append({"label": "x"})
    assert _kept(ctx) == {"facts": {"score": 1}, "marks": [{"label": "x"}]}
    ctx.facts["bad"] = float("nan")  # changed by hand: left out
    assert _kept(ctx) == {"marks": [{"label": "x"}]}


def _conformance_projects() -> list[Path]:
    return sorted(path.parent for path in (REPO / "conformance").glob("*/in/nodes"))


@pytest.mark.parametrize("source", _conformance_projects(), ids=lambda path: path.parent.name)
def test_every_conformance_project_describes(
    source: Path, tmp_path: Path, start: Callable[..., HostProcess]
) -> None:
    root = tmp_path / source.parent.name
    shutil.copytree(source, root)
    host = start(root)
    host.initialize()
    modules = sorted(path.relative_to(root).as_posix() for path in root.glob("nodes/**/*.py"))
    result = host.describe(*({"path": module} for module in modules))
    assert any(entry.get("types") for entry in result["modules"])
    for entry in result["modules"]:
        assert "error" not in entry, entry
        labels = [item["label"] for item in entry["closure"]]
        assert entry["path"] in labels
        assert labels == sorted(labels)
