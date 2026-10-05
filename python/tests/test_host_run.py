"""The host's ``run``, driven over pipes by a fake engine (spec/protocol.md sections 5.3–5.6
and 6): ``python -P -m grida.fx.host`` runs project bodies while the test answers the host's
requests and sends ``tool.invoke``, ``agent.check`` and ``$/cancel`` as the engine does."""

from __future__ import annotations

import base64
import hashlib
import itertools
import json
import os
import queue
import subprocess
import sys
import threading
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator

from grida.fx._protocol import PROTOCOL, encode_message, read_message
from grida.fx.host import SAFE_PATH_MARK

REPO = Path(__file__).resolve().parents[2]
SCHEMA = json.loads(
    (REPO / "spec" / "schemas" / "fx-node-protocol-v1.schema.json").read_text("utf-8")
)
MESSAGE = Draft202012Validator(SCHEMA)
TIMEOUT = 30


def _result_validator(name: str) -> Draft202012Validator:
    return Draft202012Validator({"$ref": f"#/$defs/{name}", "$defs": SCHEMA["$defs"]})


RESULTS = {
    "run": _result_validator("run_result"),
    "tool.invoke": _result_validator("tool_invoke_result"),
    "agent.check": _result_validator("agent_check_result"),
}


def _valid(validator: Draft202012Validator, value: Any) -> None:
    errors = [f"{list(error.path)}: {error.message}" for error in validator.iter_errors(value)]
    assert errors == [], errors


BODIES = '''\
import asyncio
import base64
import time
from pathlib import Path

from grida.fx import (
    CeilingExceeded,
    Tool,
    ToolInvocationError,
    ToolReply,
    ToolResult,
    node,
    tool,
)


@node(
    "writes",
    inputs={"source": "text"},
    outputs={
        "text": "text",
        "data": "json",
        "copy": "text",
        "file": "text/markdown",
        "items": "text[]",
        "keyed": "text{}",
        "skipped": "text?",
    },
)
def writes(ctx):
    print("printed by the body")
    ctx.fact("words", 2)
    ctx.annotate(shape="box", box=[0, 0, 0.5, 0.5], label="half")
    ctx.progress("writing", 0.5)
    note = ctx.out.path("note.md")
    note.write_text("# note\\n", "utf-8")
    return {
        "text": ctx.out.text("hello"),
        "data": ctx.out.json({"b": [1, 2], "a": "é"}),
        "copy": ctx.inputs["source"],
        "file": ctx.out.file(note),
        "items": [ctx.out.text("one"), ctx.inputs["source"]],
        "keyed": {"b": ctx.out.text("bee"), "a": ctx.out.text("ay")},
        "skipped": None,
    }


@node("edits", inputs={"image": "image"}, outputs={"image": "image"}, calls={"image.edit": 1})
async def edits(ctx):
    mask = ctx.out.bytes(b"mask", "image/png")
    result = await ctx.image_edit(image=ctx.inputs["image"], mask=mask, prompt="paint")
    ctx.fact("cached", result.cached)
    return {"image": result.image}


@node("fails", outputs={"x": "text"})
def fails(ctx):
    ctx.fact("kept", 1)
    ctx.annotate(label="why")
    raise ctx.fail("on purpose")


def helper(path):
    raise ValueError(f"cannot read {path}")


@node("crashes", outputs={"x": "text"})
def crashes(ctx):
    ctx.fact("before", True)
    helper(Path(__file__))


@node("async_crashes", outputs={"x": "text"})
async def async_crashes(ctx):
    await asyncio.sleep(0)
    raise KeyError("k")


@node("exits", outputs={"x": "text"})
def exits(ctx):
    raise SystemExit(3)


@node("says_nothing", outputs={"x": "text"})
def says_nothing(ctx):
    raise RuntimeError()


@node("ceiling", outputs={"image": "image"}, calls={"image.generate": 1})
async def ceiling(ctx):
    await ctx.image_generate(prompt="p")


@node("catches", outputs={"image": "image"}, calls={"image.generate": 1})
async def catches(ctx):
    try:
        await ctx.image_generate(prompt="p")
    except CeilingExceeded as error:
        raise ctx.fail(f"no money: {error}") from None


@node("not_outputs")
def not_outputs(ctx):
    return [1]


@node("bad_output", outputs={"x": "text"})
def bad_output(ctx):
    return {"x": "plain string"}


@node("dict_on_one", outputs={"x": "text"})
def dict_on_one(ctx):
    return {"x": {"a": ctx.out.text("a")}}


@node("waits", outputs={"x": "text?"})
def waits(ctx):
    ctx.progress("waiting")
    deadline = time.monotonic() + 20
    while not ctx.cancelled and time.monotonic() < deadline:
        time.sleep(0.01)
    ctx.fact("cancelled", ctx.cancelled)
    return {}


@node("waits_async", outputs={"x": "text?"})
async def waits_async(ctx):
    ctx.progress("waiting")
    deadline = time.monotonic() + 20
    while not ctx.cancelled and time.monotonic() < deadline:
        await asyncio.sleep(0.01)
    ctx.fact("cancelled", ctx.cancelled)
    return None


@node("leaves_a_call", calls={"image.generate": 1})
async def leaves_a_call(ctx):
    ctx.state["task"] = asyncio.create_task(ctx.image_generate(prompt="later"))
    await asyncio.sleep(0)
    return {}


@tool
def look(ctx, what: str, n: int = 1):
    """Look at something."""
    ctx.state.setdefault("looked", []).append(what)
    if what == "boom":
        raise RuntimeError("bad look")
    return {"saw": what, "n": n, "mesh": ctx.state["mesh"]}


@tool
def picture(ctx):
    """Shows the front."""
    path = ctx.work_path("views/front.png")
    path.write_bytes(b"front")
    return ToolReply("front view", images=[path])


@tool
async def slow(ctx):
    """Takes its time."""
    await asyncio.sleep(30)
    return "late"


@tool
def breaks(ctx):
    """Breaks the node."""
    raise ctx.fail("the room is broken")


def place(arguments):
    if arguments.get("x", 0) > 1:
        raise ToolInvocationError("x is outside the room")
    url = "data:image/png;base64," + base64.b64encode(b"placed").decode()
    return ToolResult("placed", images=(url,))


PLACE = Tool(
    name="place",
    description="Places a prop.",
    parameters={"type": "object", "properties": {"x": {"type": "number"}}},
    handler=place,
)


def check(value):
    if value["answer"] != "good":
        raise ValueError(f"{value['answer']} is not good enough")


@node("agent", outputs={"answer": "json"}, calls={"agent.turn": 5})
async def agent(ctx):
    ctx.state["mesh"] = "m.glb"
    runner = ctx.agent(system="sys", tools=[look, picture, slow, breaks, PLACE], recent_images=1)
    submitted = await runner.run("do it", max_steps=5, submit={"type": "object"}, check=check)
    ctx.fact("transcript_length", len(runner.transcript))
    ctx.fact("looked", ctx.state["looked"])
    return {"answer": ctx.out.json(submitted)}


@node("agent_fails", outputs={"answer": "json"}, calls={"agent.turn": 5})
async def agent_fails(ctx):
    ctx.fact("before", 1)
    await ctx.agent(system="sys", tools=[breaks]).run("do it", max_steps=2)


def bad_check(value):
    raise KeyError("unexpected")


@node("agent_check_breaks", outputs={"answer": "json"}, calls={"agent.turn": 5})
async def agent_check_breaks(ctx):
    await ctx.agent(system="sys").run(
        "do it", max_steps=2, submit={"type": "object"}, check=bad_check
    )
'''


class Refuse(Exception):
    """A handler's error answer."""

    def __init__(self, code: int, message: str, data: dict[str, Any] | None = None) -> None:
        super().__init__(message)
        self.error: dict[str, Any] = {"code": code, "message": message}
        if data is not None:
            self.error["data"] = data


#: A handler's way to leave a host request unanswered for now.
DEFER = object()


class Engine:
    """A fake engine: starts the host as the engine does, sends requests, and answers the
    host's requests with ``handlers`` while it waits for its own answers."""

    def __init__(self, root: Path, tmp: Path) -> None:
        self.root = root
        self.tmp = tmp
        self.store = tmp / "store"
        self.log = tmp / "host-stderr.log"
        self._stderr = open(self.log, "wb")  # noqa: SIM115
        env = dict(os.environ)
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
        threading.Thread(target=self._read, daemon=True).start()
        self.ids = itertools.count(1)
        self.methods: dict[int, str] = {}
        self.responses: dict[Any, dict[str, Any]] = {}
        self.requests: list[dict[str, Any]] = []
        self.notifications: list[dict[str, Any]] = []
        self.deferred: dict[Any, dict[str, Any]] = {}
        self.work_dirs: dict[str, Path] = {}
        self.handlers: dict[str, Callable[[dict[str, Any]], Any]] = {
            "file.put": self.file_put,
            "fact": lambda params: {},
            "annotate": lambda params: {},
        }

    # -- the wire ----------------------------------------------------------------------------

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

    def send(self, message: dict[str, Any]) -> None:
        assert self.process.stdin is not None
        body = encode_message(message)
        self.process.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        self.process.stdin.flush()

    def receive(self, timeout: float = TIMEOUT) -> dict[str, Any]:
        body = self.frames.get(timeout=timeout)
        assert isinstance(body, bytes), (body, self.stderr())
        message = json.loads(body)
        _valid(MESSAGE, message)
        return message

    def quiet(self, seconds: float = 0.5) -> None:
        """Asserts the host sends nothing for ``seconds``."""
        with pytest.raises(queue.Empty):
            message = self.receive(timeout=seconds)
            pytest.fail(f"the host sent {message}")

    # -- engine requests ---------------------------------------------------------------------

    def start(self, method: str, params: Any = None) -> int:
        request_id = next(self.ids)
        self.methods[request_id] = method
        message: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            message["params"] = params
        self.send(message)
        return request_id

    def finish(self, request_id: int) -> dict[str, Any]:
        """Waits for the answer to ``request_id``, serving the host's requests meanwhile."""
        while request_id not in self.responses:
            message = self.receive()
            if "method" not in message:
                self.responses[message["id"]] = message
            elif "id" in message:
                self.serve(message)
            else:
                self.notifications.append(message)
        answer = self.responses.pop(request_id)
        if "result" in answer and self.methods[request_id] in RESULTS:
            _valid(RESULTS[self.methods[request_id]], answer["result"])
        return answer

    def call(self, method: str, params: Any = None) -> dict[str, Any]:
        return self.finish(self.start(method, params))

    def notify(self, method: str, params: Any) -> None:
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    # -- host requests -----------------------------------------------------------------------

    def serve(self, message: dict[str, Any]) -> None:
        self.requests.append(message)
        handler = self.handlers.get(message["method"])
        assert handler is not None, f"no handler for {message}"
        try:
            outcome = handler(message["params"])
        except Refuse as refused:
            self.answer(message["id"], error=refused.error)
            return
        if outcome is DEFER:
            self.deferred[message["id"]] = message
            return
        self.answer(message["id"], outcome)

    def answer(
        self, request_id: Any, result: Any = None, error: dict[str, Any] | None = None
    ) -> None:
        message: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id}
        if error is not None:
            message["error"] = error
        else:
            message["result"] = result
        self.send(message)

    def ref(self, data: bytes, kind: str, name: str) -> dict[str, Any]:
        digest = hashlib.sha256(data).hexdigest()
        self.store.mkdir(parents=True, exist_ok=True)
        (self.store / digest).write_bytes(data)
        return {
            "digest": digest,
            "kind": kind,
            "size": len(data),
            "name": name,
            "path": str(self.store / digest),
            "facts": {"bytes": len(data), "kind": kind},
        }

    def file_put(self, params: dict[str, Any]) -> dict[str, Any]:
        if "base64" in params:
            data = base64.b64decode(params["base64"], validate=True)
            kind = params.get("kind", "file")
        elif "json" in params:
            data = json.dumps(params["json"], sort_keys=True, indent=1, ensure_ascii=False).encode()
            kind = params.get("kind", "json")
        else:
            data = (self.work_dirs[params["run_id"]] / params["work_path"]).read_bytes()
            kind = params["kind"]
        return self.ref(data, kind, params.get("name", "file"))

    # -- the session -------------------------------------------------------------------------

    def initialize(self) -> None:
        answer = self.call(
            "initialize",
            {
                "protocol": PROTOCOL,
                "engine": {"name": "grida-fx", "version": "0.1.0"},
                "project_root": str(self.root),
                "sources": [],
            },
        )
        assert "result" in answer, answer

    def run_params(self, run_id: str, attribute: str, **changes: Any) -> dict[str, Any]:
        work = self.tmp / "work" / run_id
        work.mkdir(parents=True)
        self.work_dirs[run_id] = work
        params: dict[str, Any] = {
            "run_id": run_id,
            "instance": {"id": f"{attribute}#1", "path": attribute, "step": attribute},
            "type": f"nodes/bodies.py#{attribute}",
            "body": {"path": "nodes/bodies.py", "attribute": attribute},
            "params": {},
            "param_files": {},
            "inputs": {},
            "work_dir": str(work),
            "resources": {},
            "tools": {},
            "calls": {},
            "timeout_s": None,
        }
        params["instance"].update({"key": None, "take": [1]})
        params.update(changes)
        return params

    def run(self, run_id: str, attribute: str, **changes: Any) -> dict[str, Any]:
        return self.call("run", self.run_params(run_id, attribute, **changes))

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
def project(tmp_path: Path) -> Path:
    root = tmp_path / "acme"
    (root / "nodes").mkdir(parents=True)
    (root / "nodes" / "bodies.py").write_text(BODIES, "utf-8")
    return root


@pytest.fixture
def engine(project: Path, tmp_path: Path) -> Iterator[Engine]:
    started = Engine(project, tmp_path)
    started.initialize()
    yield started
    started.stop()


def _methods(engine: Engine) -> list[str]:
    return [message["method"] for message in engine.requests]


# --- results ----------------------------------------------------------------------------------


def test_a_sync_body_writes_its_outputs(engine: Engine) -> None:
    source = engine.ref(b"source text", "text/plain", "source.txt")
    answer = engine.run("r1", "writes", inputs={"source": source})
    assert "result" in answer, answer
    assert _methods(engine) == ["fact", "annotate"] + ["file.put"] * 5
    fact, annotate, *puts = [message["params"] for message in engine.requests]
    assert fact == {"run_id": "r1", "name": "words", "value": 2}
    assert annotate == {
        "run_id": "r1",
        "mark": {"shape": "box", "label": "half", "box": [0, 0, 0.5, 0.5]},
    }
    assert engine.notifications == [
        {
            "jsonrpc": "2.0",
            "method": "progress",
            "params": {"run_id": "r1", "text": "writing", "fraction": 0.5},
        }
    ]
    assert [(put.get("name"), put.get("kind")) for put in puts] == [
        ("writes/text", "text/plain"),
        ("writes/data", "json"),
        ("writes/items[0]", "text/plain"),
        ("writes/keyed[b]", "text/plain"),
        ("writes/keyed[a]", "text/plain"),
    ]
    assert puts[1]["json"] == {"b": [1, 2], "a": "é"}

    def file(data: bytes, kind: str, name: str) -> dict[str, Any]:
        return {"file": engine.ref(data, kind, name)}

    # The answer carries outputs only: facts and marks went with fact and annotate.
    assert answer["result"] == {
        "outputs": {
            "text": file(b"hello", "text/plain", "writes/text"),
            "data": file(
                '{\n "a": "é",\n "b": [\n  1,\n  2\n ]\n}'.encode(), "json", "writes/data"
            ),
            "copy": {"file": source},
            "file": {"work_path": "out/note.md", "kind": "text/markdown"},
            "items": {"list": [file(b"one", "text/plain", "writes/items[0]"), {"file": source}]},
            "keyed": {
                "collection": [
                    ["b", file(b"bee", "text/plain", "writes/keyed[b]")],
                    ["a", file(b"ay", "text/plain", "writes/keyed[a]")],
                ]
            },
            "skipped": None,
        }
    }
    assert (engine.work_dirs["r1"] / "out" / "note.md").read_text("utf-8") == "# note\n"
    assert "printed by the body" in engine.stderr()


def test_an_async_body_calls_a_capability(engine: Engine) -> None:
    image = engine.ref(b"input image", "image/png", "in.png")
    made = engine.ref(b"edited image", "image/png", "out.png")
    calls: list[dict[str, Any]] = []

    def capability(params: dict[str, Any]) -> Any:
        calls.append(params)
        return {
            "key": "c" * 64,
            "cached": True,
            "cost_usd": 0,
            "files": {"image": made},
            "data": {},
        }

    engine.handlers["capability"] = capability
    answer = engine.run("r1", "edits", inputs={"image": image})
    assert answer["result"] == {"outputs": {"image": {"file": made}}}
    assert _methods(engine) == ["file.put", "capability", "fact"]
    assert engine.requests[0]["params"]["name"] == "request/file"
    assert calls == [
        {
            "run_id": "r1",
            "capability": "image.edit",
            "request": {
                "image": {"file": image["digest"]},
                "mask": {"file": hashlib.sha256(b"mask").hexdigest()},
                "prompt": "paint",
            },
        }
    ]
    assert engine.requests[2]["params"] == {"run_id": "r1", "name": "cached", "value": True}
    # Host requests are numbered from 1, per session.
    assert [message["id"] for message in engine.requests] == [1, 2, 3]


def test_a_run_is_answered_after_its_own_requests(engine: Engine) -> None:
    engine.handlers["capability"] = lambda params: DEFER
    run_id = engine.start("run", engine.run_params("r1", "leaves_a_call"))
    message = engine.receive()
    assert message["method"] == "capability"
    engine.serve(message)
    assert list(engine.deferred) == [message["id"]]
    # The body has returned, but its capability call is still pending.
    engine.quiet()
    answer = {"key": "c" * 64, "cached": False, "cost_usd": 0.04, "files": {}, "data": None}
    engine.answer(message["id"], answer)
    assert engine.finish(run_id)["result"] == {"outputs": {}}


# --- failures ---------------------------------------------------------------------------------


def test_node_failure_keeps_facts_and_marks(engine: Engine) -> None:
    answer = engine.run("r1", "fails")
    assert answer["error"] == {
        "code": -32000,
        "message": "on purpose",
        "data": {"facts": {"kept": 1}, "marks": [{"label": "why"}]},
    }


def test_node_error_carries_the_traceback(engine: Engine, project: Path) -> None:
    answer = engine.run("r1", "crashes")
    error = answer["error"]
    assert error["code"] == -32001
    # The message is the exception's own: the engine names the failure `<exception>: <message>`.
    assert error["message"] == "cannot read nodes/bodies.py"
    data = error["data"]
    assert data["exception"] == "ValueError"
    assert data["facts"] == {"before": True} and data["marks"] == []
    trace = data["traceback"]
    assert trace.startswith("Traceback (most recent call last):\n")
    assert str(project) not in trace and str(project.resolve()) not in trace
    lines = trace.splitlines()
    assert lines[1].startswith('  File "nodes/bodies.py"') and lines[1].endswith("in crashes")
    assert "in helper" in trace
    assert "grida/fx/host.py" not in trace and "asyncio" not in trace
    assert lines[-1] == "ValueError: cannot read nodes/bodies.py"

    error = engine.run("r2", "async_crashes")["error"]
    assert (error["code"], error["message"], error["data"]["exception"]) == (
        -32001,
        "'k'",
        "KeyError",
    )
    assert error["data"]["traceback"].splitlines()[1].endswith("in async_crashes")
    # SystemExit is the body's error; the host goes on.
    error = engine.run("r3", "exits")["error"]
    assert (error["code"], error["message"], error["data"]["exception"]) == (
        -32001,
        "3",
        "SystemExit",
    )
    assert engine.run("r4", "fails")["error"]["code"] == -32000
    # A message is never empty: an exception without one sends its type.
    error = engine.run("r5", "says_nothing")["error"]
    assert (error["code"], error["message"], error["data"]["exception"]) == (
        -32001,
        "RuntimeError",
        "RuntimeError",
    )


def test_an_engine_error_passes_through(engine: Engine) -> None:
    data = {"capability": "image.generate", "needed_usd": 0.04, "remaining_usd": 0.01}
    message = "run ceiling reached: ceiling#1 needs up to $0.04 and $0.01 is left"

    def refuse(params: dict[str, Any]) -> Any:
        raise Refuse(-32014, message, data)

    engine.handlers["capability"] = refuse
    assert engine.run("r1", "ceiling")["error"] == {
        "code": -32014,
        "message": message,
        "data": data,
    }
    # A body may catch it.
    error = engine.run("r2", "catches")["error"]
    assert error == {
        "code": -32000,
        "message": f"no money: {message}",
        "data": {"facts": {}, "marks": []},
    }


def test_local_refusals_of_outputs(engine: Engine) -> None:
    for attribute, message in [
        ("not_outputs", "not_outputs returned list, not its outputs"),
        ("bad_output", "output x is str; use ctx.out to make it"),
        ("dict_on_one", "output x is dict; use ctx.out to make it"),
    ]:
        error = engine.run(f"r-{attribute}", attribute)["error"]
        assert (error["code"], error["message"]) == (-32000, message)


def test_bodies_that_cannot_load(engine: Engine) -> None:
    error = engine.run("r1", "missing")["error"]
    assert error == {
        "code": -32004,
        "message": "nodes/bodies.py: missing is not declared with @node",
    }
    error = engine.run("r2", "helper")["error"]
    assert error["message"] == "nodes/bodies.py: helper is not declared with @node"
    params = engine.run_params("r3", "x", body={"path": "nodes/none.py", "attribute": "x"})
    assert engine.call("run", params)["error"] == {
        "code": -32004,
        "message": "no module file nodes/none.py",
    }
    for change in (
        {"body": {"path": "../x.py", "attribute": "x"}},
        {"body": {"builtin": "fx/a@1", "path": "x"}},
        {"work_dir": "relative"},
        {"instance": {"id": "x"}},
        {"run_id": ""},
        {"inputs": []},
        {"timeout_s": "1"},
    ):
        params = engine.run_params(f"r-{len(engine.work_dirs)}", "writes")
        params.update(change)
        assert engine.call("run", params)["error"]["code"] == -32602, change


def test_an_unknown_builtin(engine: Engine) -> None:
    params = engine.run_params("r1", "x", body={"builtin": "fx/nothing@1"})
    error = engine.call("run", params)["error"]
    assert error["code"] == -32004
    assert error["message"].endswith("has no body for fx/nothing@1")


# --- one run at a time, and cancelling it -----------------------------------------------------


@pytest.mark.parametrize("attribute", ["waits", "waits_async"])
def test_cancel_sets_ctx_cancelled(engine: Engine, attribute: str) -> None:
    run_id = engine.start("run", engine.run_params("r1", attribute))
    progress = engine.receive()
    assert progress["method"] == "progress"
    # One job at a time: work that comes while the run is pending is refused.
    second = engine.call("run", engine.run_params("r2", "writes"))
    assert second["error"] == {"code": -32600, "message": "run came while a run is pending"}
    describe = engine.call("describe", {"targets": [], "builtins": False})
    assert describe["error"] == {"code": -32600, "message": "describe came while a run is pending"}
    engine.notify("$/cancel", {"id": 999})  # names nothing pending: ignored
    engine.notify("$/cancel", {"id": run_id})
    answer = engine.finish(run_id)
    assert answer["result"] == {"outputs": {}}
    assert engine.requests[-1]["params"] == {"run_id": "r1", "name": "cancelled", "value": True}
    # The host takes the next run.
    assert engine.run("r3", "fails")["error"]["code"] == -32000


def test_requests_for_no_pending_run(engine: Engine) -> None:
    answer = engine.call(
        "tool.invoke",
        {"run_id": "r1", "agent_id": "agent-1", "call_id": None, "name": "look", "arguments": {}},
    )
    assert answer["error"] == {"code": -32602, "message": "no run r1 is pending on this host"}
    answer = engine.call("agent.check", {"run_id": "r1", "agent_id": "agent-1", "value": 1})
    assert answer["error"]["code"] == -32602


# --- agents -----------------------------------------------------------------------------------


def test_an_agent_run_serves_tools_and_checks(engine: Engine) -> None:
    seen: dict[str, Any] = {}

    def invoke(name: str, arguments: dict[str, Any], **changes: Any) -> dict[str, Any]:
        params = {
            "run_id": "r1",
            "agent_id": seen["agent_id"],
            "call_id": f"c-{name}",
            "name": name,
            "arguments": arguments,
        }
        params.update(changes)
        return engine.call("tool.invoke", params)

    def agent_run(params: dict[str, Any]) -> Any:
        seen["agent_id"] = params["agent_id"]
        seen["params"] = params
        seen["look"] = invoke("look", {"what": "x"})
        seen["boom"] = invoke("look", {"what": "boom"})
        seen["picture"] = invoke("picture", {})
        seen["far"] = invoke("place", {"x": 2})
        seen["near"] = invoke("place", {"x": 0.5})
        seen["unknown agent"] = invoke("look", {"what": "x"}, agent_id="agent-9")
        seen["other run"] = invoke("look", {"what": "x"}, run_id="r9")
        slow = engine.start(
            "tool.invoke",
            {
                "run_id": "r1",
                "agent_id": seen["agent_id"],
                "call_id": None,
                "name": "slow",
                "arguments": {},
            },
        )
        engine.notify("$/cancel", {"id": slow})
        seen["slow"] = engine.finish(slow)
        check = {"run_id": "r1", "agent_id": seen["agent_id"]}
        seen["bad"] = engine.call("agent.check", {**check, "value": {"answer": "bad"}})
        seen["good"] = engine.call("agent.check", {**check, "value": {"answer": "good"}})
        transcript = [
            {"role": "user", "content": "do it"},
            {"role": "assistant", "content": "", "tool_calls": []},
            {"role": "user", "content": "Finish by calling submit."},
        ]
        return {
            "submitted": {"answer": "good"},
            "transcript": transcript,
            "turns": 3,
            "cost_usd": 0,
        }

    engine.handlers["agent.run"] = agent_run
    answer = engine.run("r1", "agent")
    assert "result" in answer, answer
    params = seen["params"]
    assert params["agent_id"] == "agent-1"
    assert [tool["name"] for tool in params["tools"]] == [
        "look",
        "picture",
        "slow",
        "breaks",
        "place",
    ]
    assert params["tools"][0] == {
        "name": "look",
        "description": "Look at something.",
        "parameters": {
            "type": "object",
            "properties": {"what": {"type": "string"}, "n": {"type": "integer"}},
            "required": ["what"],
        },
    }
    assert params["tools"][4]["parameters"] == {
        "type": "object",
        "properties": {"x": {"type": "number"}},
    }
    assert (params["submit"], params["check"], params["max_steps"], params["recent_images"]) == (
        {"type": "object"},
        True,
        5,
        1,
    )
    assert "max_tokens" not in params and "images" not in params
    assert seen["look"]["result"] == {"content": {"saw": "x", "n": 1, "mesh": "m.glb"}}
    assert seen["boom"]["result"] == {"error": "RuntimeError: bad look"}
    assert seen["picture"]["result"] == {
        "content": "front view",
        "images": [{"file": hashlib.sha256(b"front").hexdigest()}],
    }
    assert seen["far"]["result"] == {"error": "ToolInvocationError: x is outside the room"}
    assert seen["near"]["result"] == {
        "content": "placed",
        "images": [{"file": hashlib.sha256(b"placed").hexdigest()}],
    }
    assert seen["unknown agent"]["error"] == {
        "code": -32602,
        "message": "run r1 has no agent agent-9 running",
    }
    assert seen["other run"]["error"] == {
        "code": -32602,
        "message": "no run r9 is pending on this host",
    }
    assert seen["slow"]["error"] == {"code": -32002, "message": "the engine cancelled tool.invoke"}
    assert seen["bad"]["result"] == {"refusal": "bad is not good enough"}
    assert seen["good"]["result"] == {"refusal": None}
    puts = [message["params"] for message in engine.requests if message["method"] == "file.put"]
    assert puts[0] == {
        "run_id": "r1",
        "work_path": "views/front.png",
        "kind": "image/png",
        "name": "front.png",
    }
    assert puts[1]["base64"] == base64.b64encode(b"placed").decode()
    facts = {
        message["params"]["name"]: message["params"]["value"]
        for message in engine.requests
        if message["method"] == "fact"
    }
    assert facts == {"transcript_length": 3, "looked": ["x", "boom"]}
    assert answer["result"]["outputs"]["answer"]["file"]["kind"] == "json"
    # Nothing is left pending: the cancelled tool's answer is never sent twice.
    engine.quiet(0.3)


def test_a_tool_that_fails_the_node(engine: Engine) -> None:
    def agent_run(params: dict[str, Any]) -> Any:
        answer = engine.call(
            "tool.invoke",
            {
                "run_id": "r1",
                "agent_id": params["agent_id"],
                "call_id": "c1",
                "name": "breaks",
                "arguments": {},
            },
        )
        assert answer["error"] == {"code": -32000, "message": "the room is broken"}
        raise Refuse(-32000, answer["error"]["message"])

    engine.handlers["agent.run"] = agent_run
    error = engine.run("r1", "agent_fails")["error"]
    assert error == {
        "code": -32000,
        "message": "the room is broken",
        "data": {"facts": {"before": 1}, "marks": []},
    }


def test_a_check_that_raises_is_a_node_error(engine: Engine) -> None:
    def agent_run(params: dict[str, Any]) -> Any:
        answer = engine.call(
            "agent.check", {"run_id": "r1", "agent_id": params["agent_id"], "value": {}}
        )
        error = answer["error"]
        assert (error["code"], error["message"]) == (-32001, "'unexpected'")
        assert error["data"]["exception"] == "KeyError"
        # The traceback starts at the check, not in the SDK or the event loop.
        first = error["data"]["traceback"].splitlines()[1]
        assert first.startswith('  File "nodes/bodies.py"') and first.endswith("in bad_check")
        raise Refuse(-32001, error["message"])

    engine.handlers["agent.run"] = agent_run
    error = engine.run("r1", "agent_check_breaks")["error"]
    assert (error["code"], error["message"]) == (-32001, "'unexpected'")
    assert error["data"]["exception"] == "KeyError"
    assert "in bad_check" in error["data"]["traceback"]


def test_shutdown_and_exit_after_runs(engine: Engine) -> None:
    assert "error" in engine.run("r1", "fails")
    assert engine.call("shutdown")["result"] is None
    engine.notify("exit", None)
    assert engine.process.wait(timeout=TIMEOUT) == 0


def test_project_modules_named_like_what_a_run_needs(project: Path, tmp_path: Path) -> None:
    """The body loop's machinery is imported before the project root is on sys.path, so a
    project module named like one of its modules never replaces it."""
    for name in ("queue", "selectors", "contextvars", "linecache", "tokenize", "signal"):
        (project / f"{name}.py").write_text(
            f"import sys\nsys.stderr.write('shadow {name} ran\\n')\nraise ImportError('{name}')\n",
            "utf-8",
        )
    engine = Engine(project, tmp_path)
    try:
        engine.initialize()
        source = engine.ref(b"source text", "text/plain", "source.txt")
        assert "result" in engine.run("r1", "writes", inputs={"source": source})
        assert engine.run("r2", "crashes")["error"]["code"] == -32001
        image = engine.ref(b"image", "image/png", "i.png")
        engine.handlers["capability"] = lambda params: {
            "key": "c" * 64,
            "cached": True,
            "cost_usd": 0,
            "files": {"image": image},
            "data": None,
        }
        assert "result" in engine.run("r3", "edits", inputs={"image": image})
        # A capability result without the family asked for: the SDK's CallFailed keeps its code.
        engine.handlers["capability"] = lambda params: {
            "key": "c" * 64,
            "cached": True,
            "cost_usd": 0,
            "files": {"text": source},
            "data": None,
        }
        assert engine.run("r4", "edits", inputs={"image": image})["error"] == {
            "code": -32016,
            "message": "the call returned no image",
        }
        assert "shadow" not in engine.stderr()
    finally:
        engine.stop()
