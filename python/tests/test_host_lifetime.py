"""How long a node host and what its code starts live: a fork never holds the protocol streams,
and a host whose engine is gone ends with its process group (``grida.fx.host``)."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import textwrap
import time
from pathlib import Path
from typing import BinaryIO

import pytest

from grida.fx._protocol import PROTOCOL, read_message, write_message

pytestmark = pytest.mark.skipif(
    not hasattr(os, "fork") or not hasattr(os, "killpg"), reason="needs fork and process groups"
)

ENGINE = {"name": "grida-fx", "version": "0.1.0"}
TIMEOUT = 30


def _alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _gone_within(pid: int, seconds: float) -> bool:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if not _alive(pid):
            return True
        time.sleep(0.02)
    return not _alive(pid)


def _wait_for(path: Path) -> str:
    deadline = time.monotonic() + TIMEOUT
    while time.monotonic() < deadline:
        if path.exists():
            text = path.read_text("utf-8")
            if text.endswith("\n"):
                return text
        time.sleep(0.02)
    raise AssertionError(f"{path.name} was never written")


def _project(tmp_path: Path, module: str) -> Path:
    root = tmp_path / "acme"
    (root / "nodes").mkdir(parents=True)
    (root / "fx.yaml").write_text("fx: project/v1\n", "utf-8")
    (root / "nodes" / "m.py").write_text(textwrap.dedent(module), "utf-8")
    return root


def _request(stream: BinaryIO, request_id: int, method: str, params: object = None) -> None:
    message: dict[str, object] = {"jsonrpc": "2.0", "id": request_id, "method": method}
    if params is not None:
        message["params"] = params
    write_message(stream, message)


def _initialize(root: Path) -> dict[str, object]:
    return {"protocol": PROTOCOL, "engine": ENGINE, "project_root": str(root), "sources": []}


FORKS_AT_IMPORT = """\
import os, time
child = os.fork()
if child == 0:
    time.sleep(30)
    os._exit(0)
with open("forked.txt", "w") as f:
    f.write("%d\\n" % child)
"""


def test_a_fork_does_not_keep_the_protocol_streams(tmp_path: Path) -> None:
    root = _project(tmp_path, FORKS_AT_IMPORT)
    host = subprocess.Popen(
        [sys.executable, "-P", "-m", "grida.fx.host"],
        cwd=root,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    assert host.stdin is not None and host.stdout is not None
    forked = 0
    try:
        _request(host.stdin, 1, "initialize", _initialize(root))
        assert json.loads(read_message(host.stdout) or b"")["id"] == 1
        _request(
            host.stdin, 2, "describe", {"targets": [{"path": "nodes/m.py"}], "builtins": False}
        )
        assert json.loads(read_message(host.stdout) or b"")["id"] == 2
        forked = int(_wait_for(root / "forked.txt"))
        assert _alive(forked)
        _request(host.stdin, 3, "shutdown")
        assert json.loads(read_message(host.stdout) or b"")["id"] == 3
        write_message(host.stdin, {"jsonrpc": "2.0", "method": "exit"})
        assert host.wait(timeout=TIMEOUT) == 0
        # The fork still runs, yet the host's output ends: the fork holds the null device.
        started = time.monotonic()
        assert host.stdout.read() == b""
        assert time.monotonic() - started < 5
        assert _alive(forked)
    finally:
        if forked and _alive(forked):
            os.kill(forked, signal.SIGKILL)
        if host.poll() is None:
            host.kill()
            host.wait()


HANGS_AT_IMPORT = """\
import os, subprocess, time
child = subprocess.Popen(["sleep", "60"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
with open("pids.txt", "w") as f:
    f.write("%d %d\\n" % (os.getpid(), child.pid))
while True:
    time.sleep(0.1)
"""

#: Starts a host as the engine does (a process group of its own), asks it to describe a module,
#: and then waits: it plays an engine that is killed while the host is busy.
ENGINE_STAND_IN = """\
import json, subprocess, sys, time
from grida.fx._protocol import write_message
root = sys.argv[1]
host = subprocess.Popen([sys.executable, "-P", "-m", "grida.fx.host"], cwd=root,
                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, process_group=0)
initialize = {"protocol": sys.argv[2], "engine": {"name": "grida-fx", "version": "0.1.0"},
              "project_root": root, "sources": []}
write_message(host.stdin, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                           "params": initialize})
write_message(host.stdin, {"jsonrpc": "2.0", "id": 2, "method": "describe",
                           "params": {"targets": [{"path": "nodes/m.py"}], "builtins": False}})
while True:
    time.sleep(1)
"""


def test_a_host_whose_engine_is_gone_ends_with_its_group(tmp_path: Path) -> None:
    root = _project(tmp_path, HANGS_AT_IMPORT)
    engine = subprocess.Popen(
        [sys.executable, "-c", ENGINE_STAND_IN, str(root), PROTOCOL],
        stderr=subprocess.DEVNULL,
    )
    pids: list[int] = []
    try:
        pids = [int(pid) for pid in _wait_for(root / "pids.txt").split()]
        assert all(_alive(pid) for pid in pids)
        # Killed: it can end nothing, and the host's reader is still busy importing the module.
        engine.kill()
        engine.wait(timeout=TIMEOUT)
        for pid in pids:
            assert _gone_within(pid, 10), f"{pid} outlived its engine"
    finally:
        if engine.poll() is None:
            engine.kill()
            engine.wait()
        for pid in pids:
            if _alive(pid):
                os.kill(pid, signal.SIGKILL)
