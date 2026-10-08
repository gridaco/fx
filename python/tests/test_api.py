"""``grida.fx.plan/run`` and ``python -m grida.fx``, against a fake ``grida-fx``.

The fake is a Python script set as ``GRIDA_FX_BIN``: it records each invocation (its arguments,
working directory and the inputs files it was given) and answers with canned output per verb; for
``run`` it writes a canned ``events.jsonl`` into the run folder and prints the summary. Given
``--stand-in=-``, it speaks to the stand-in on its standard input as the engine does: it sends
``initialize``, then a scripted series of ``stand_in.answer`` requests and ``$/cancel``
notifications, then ``shutdown`` and ``exit``, and logs every message it got back. So these tests
pin what the SDK sends and how it reads what comes back, without an engine.
"""

from __future__ import annotations

import asyncio
import base64
import collections
import contextlib
import gc
import hashlib
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time
import types
import warnings
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator

import grida.fx as fx
from grida.fx import _api
from grida.fx._api import RunFile, RunResult, StepResult

pytestmark = pytest.mark.skipif(os.name != "posix", reason="the fake binary is a script")

FAKE = r"""
import json, os, signal, sys, time
from pathlib import Path

args = sys.argv[1:]
config = json.loads(Path(os.environ["FAKE_FX_CONFIG"]).read_text())
log = Path(os.environ["FAKE_FX_LOG"])
inputs = {}
for arg in args:
    if arg.startswith("--inputs="):
        name = arg.split("=", 1)[1]
        if Path(name).is_file():
            inputs[name] = Path(name).read_text(encoding="utf-8")
with log.open("a") as out:
    python = os.environ.get("GRIDA_FX_PYTHON")
    sdk_python = os.environ.get("GRIDA_FX_SDK_PYTHON")
    out.write(json.dumps({"argv": args, "cwd": os.getcwd(), "inputs": inputs, "python": python,
                          "sdk_python": sdk_python}))
    out.write("\n")
reply = config.get(args[0] if args else "", {})
folder = None
if args and args[0] == "run":
    folder = next((a.split("=", 1)[1] for a in args if a.startswith("--run=")), None)
    folder = folder or reply.get("folder")
    if "events" in reply:
        Path(folder).mkdir(parents=True, exist_ok=True)
        lines = "".join(json.dumps(event) + "\n" for event in reply["events"])
        (Path(folder) / "events.jsonl").write_text(lines + reply.get("torn", ""))
if "--stand-in=-" in args:
    import itertools, socket, stat
    from grida.fx._protocol import PROTOCOL, read_message, write_message

    on_socket = stat.S_ISSOCK(os.fstat(0).st_mode)
    channel = socket.socket(fileno=os.dup(0))
    reader, writer = channel.makefile("rb"), channel.makefile("wb")
    ids = itertools.count(1)
    received = []

    def send(method, params=None, notify=False):
        message = {"jsonrpc": "2.0", "method": method}
        if not notify:
            message["id"] = next(ids)
        if params is not None:
            message["params"] = params
        write_message(writer, message)

    def take(count):
        for _ in range(count):
            body = read_message(reader)
            received.append(None if body is None else json.loads(body))

    engine = {"name": "grida-fx", "version": "0.0.0"}
    session = {"protocol": PROTOCOL, "engine": engine, "project_root": os.getcwd(), "sources": []}
    send("initialize", session)
    take(1)
    for step in reply.get("stand_in", []):
        if "ask" in step:
            send("stand_in.answer", step["ask"])
        elif "cancel" in step:
            send("$/cancel", {"id": step["cancel"]}, notify=True)
        elif "request" in step:
            send(step["request"], step.get("params"))
        elif "take" in step:
            take(step["take"])
        elif "sleep" in step:
            time.sleep(step["sleep"])
    send("shutdown")
    take(1)
    send("exit", notify=True)
    with log.open("a") as out:
        out.write(json.dumps({"on_socket": on_socket, "stand_in": received}) + "\n")
if "linger" in reply:
    # A process of its own session that keeps this one's stdout and stderr open, as a process a
    # node body started does.
    import subprocess
    child = subprocess.Popen(
        [sys.executable, "-c", f"import time; time.sleep({reply['linger']})"],
        stdin=subprocess.DEVNULL,
        start_new_session=True,
    )
    with log.open("a") as out:
        out.write(json.dumps({"lingering": child.pid}) + "\n")
if "sleep" in reply:
    def interrupted(signum, frame):
        with log.open("a") as out:
            out.write(json.dumps({"interrupted": True}) + "\n")
            if "cleanup_gate" in reply:
                out.write(json.dumps({"signal": signum,
                                      "source": os.environ.get("GRIDA_FX_CANCEL_SOURCE")}) + "\n")
        if "cleanup_gate" in reply:
            while not Path(reply["cleanup_gate"]).exists():
                time.sleep(0.01)
            with log.open("a") as out:
                out.write(json.dumps({"cleanup": "complete"}) + "\n")
        sys.exit(130)
    signal.signal(signal.SIGINT, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    if "ready_file" in reply:
        Path(reply["ready_file"]).write_text(str(os.getpid()))
    time.sleep(reply["sleep"])
sys.stdout.write(reply.get("stdout", "").replace("{folder}", folder or ""))
sys.stderr.write(reply.get("stderr", ""))
sys.exit(reply.get("status", 0))
"""

D1 = "1" * 64
D2 = "2" * 64
D3 = "3" * 64
D4 = "4" * 64

REPO = Path(__file__).resolve().parents[2]
SCHEMA = json.loads(
    (REPO / "spec" / "schemas" / "fx-node-protocol-v1.schema.json").read_text("utf-8")
)
EVENTS_SCHEMA = Draft202012Validator(
    json.loads((REPO / "spec" / "schemas" / "fx-run-events-v1.schema.json").read_text("utf-8"))
)


def _definition(name: str) -> Draft202012Validator:
    return Draft202012Validator({"$ref": f"#/$defs/{name}", "$defs": SCHEMA["$defs"]})


ASK_PARAMS = _definition("stand_in_answer_params")
ANSWER_RESULT = _definition("stand_in_answer_result")


def _valid(validator: Draft202012Validator, value: Any) -> None:
    errors = [f"{list(error.path)}: {error.message}" for error in validator.iter_errors(value)]
    assert errors == [], errors


GRAPH = {
    "kind": "fx-graph-v1",
    "workflow": {"id": "gallery", "title": "Gallery", "file": "workflows/gallery.yaml"},
    "types": {},
    "instances": [],
    "pending": [],
    "estimate": {"low_usd": 0.12, "high_usd": 0.5, "ceiling_usd": 2},
    "problems": [],
}
PRICE = {
    "phases": [
        {"phase": 1, "steps": 2, "calls": [1, 2], "low_usd": 0.12, "high_usd": 0.3, "then": []},
        {"phase": 2, "steps": 1, "calls": [0, 4], "low_usd": 0, "high_usd": 0.2, "then": []},
    ],
    "estimate": {"low_usd": 0.12, "high_usd": 0.5},
    "ceiling_usd": 2,
}
REFUSED_GRAPH = {
    **GRAPH,
    "problems": [
        {"where": "steps.draw.with.prompt", "message": "names nothing"},
        {"where": "inputs", "message": "poster is missing"},
    ],
}


def _envelope(event: str, **fields: Any) -> dict[str, Any]:
    return {
        "kind": "fx-run-events-v1",
        "event": event,
        "invocation_id": "inv-1",
        "plan": "f" * 64,
        "offset_ms": 0,
        **fields,
    }


def _file(digest: str, kind: str, name: str, key: str | None = None) -> dict[str, Any]:
    ref: dict[str, Any] = {"digest": digest, "kind": kind, "name": name, "size": 3}
    if key is not None:
        ref["key"] = key
    return {"file": ref}


EVENTS = [
    _envelope(
        "run_started",
        workflow="gallery",
        resumed=False,
        ceiling_usd=2,
        charged_usd=0,
        estimate={"low_usd": 0.12, "high_usd": 0.5},
    ),
    _envelope(
        "node_finished",
        id="entity['ada'].review#1",
        path="entity['ada'].review",
        cache="miss",
        outputs={},
        facts={"verdict": "reject", "cost_usd": None},
    ),
    _envelope(
        "node_finished",
        id="entity['ada'].review#2",
        path="entity['ada'].review",
        cache="hit",
        outputs={"notes": _file(D4, "json", "entity['ada'].review/notes")},
        facts={"verdict": "accept", "cost_usd": 0.04},
    ),
    _envelope("node_failed", id="poster#1", path="poster", error="the poster has no face"),
    _envelope(
        "run_finished",
        ok=False,
        incomplete=False,
        stopped=None,
        charged_usd=0.25,
        failed=["poster#1"],
        outputs={
            "plate": _file(D1, "image/png", "plate/image"),
            "images": {
                "collection": [
                    ["ada", _file(D2, "image/png", "draw/image", key="ada")],
                    ["../bo/x y", _file(D3, "image/jpeg", "draw/image", key="../bo/x y")],
                ]
            },
            "frames": {
                "list": [_file(D1, "image/png", "f/0"), _file(D2, "image/png", "f/1", key="k")]
            },
            "skipped": {"none": True},
            "count": {"value": 3},
        },
    ),
]


class Fake:
    """The fake binary, its canned replies and its log."""

    def __init__(self, folder: Path, monkeypatch: pytest.MonkeyPatch) -> None:
        self.folder = folder
        self.script = folder / "grida-fx"
        self.script.write_text(f"#!{sys.executable}\n{FAKE}", encoding="utf-8")
        self.script.chmod(0o755)
        self.config_path = folder / "config.json"
        self.log_path = folder / "log.jsonl"
        self.config: dict[str, Any] = {}
        self.reply(plan={"stdout": json.dumps(GRAPH)}, price={"stdout": json.dumps(PRICE)})
        monkeypatch.setenv("GRIDA_FX_BIN", str(self.script))
        monkeypatch.setenv("FAKE_FX_CONFIG", str(self.config_path))
        monkeypatch.setenv("FAKE_FX_LOG", str(self.log_path))

    def reply(self, **verbs: dict[str, Any]) -> None:
        self.config.update(verbs)
        self.config_path.write_text(json.dumps(self.config), encoding="utf-8")

    def runs(self, events: list[dict[str, Any]] = EVENTS, **reply: Any) -> None:
        summary = "the plan text\nrun       {folder}\nresult    failed   spent $0.25\n"
        self.reply(run={"events": events, "stdout": summary, "status": 1, **reply})

    def stands_in(self, script: list[dict[str, Any]], **reply: Any) -> None:
        """A stand-in run (into ``runs/gallery/r1`` unless ``--run`` names a folder) whose
        engine follows ``script``."""
        reply.setdefault("folder", "runs/gallery/r1")
        self.runs(events=STAND_IN_EVENTS, stand_in=script, **reply)

    @property
    def calls(self) -> list[dict[str, Any]]:
        if not self.log_path.exists():
            return []
        lines = self.log_path.read_text(encoding="utf-8").splitlines()
        return [json.loads(line) for line in lines]

    @property
    def invocations(self) -> list[list[str]]:
        return [call["argv"] for call in self.calls if "argv" in call]

    @property
    def stand_in(self) -> list[dict[str, Any]]:
        """What the last stand-in session got back, in order: the answer to ``initialize``, to
        each scripted request, and to ``shutdown``; each ``stand_in.answer`` result checked
        against its definition."""
        sessions = [call for call in self.calls if "stand_in" in call]
        assert sessions, "the fake engine served no stand-in"
        assert sessions[-1]["on_socket"], "the engine's standard input is the stand-in's socket"
        received = sessions[-1]["stand_in"]
        for message in received[1:-1]:
            if message is not None and "result" in message:
                _valid(ANSWER_RESULT, message["result"])
        return received


@pytest.fixture
def fake(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Fake:
    bin_folder = tmp_path / "bin"
    bin_folder.mkdir()
    return Fake(bin_folder, monkeypatch)


@pytest.fixture
def project(tmp_path: Path) -> Path:
    root = tmp_path / "project"
    (root / "workflows").mkdir(parents=True)
    (root / "fx.yaml").write_text("fx: project/v1\ncache: store/cache  # here\n", "utf-8")
    return root


# ------------------------------------------------------------------------------------------------
# The binary


def test_the_binary_is_named_by_the_environment_first(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    named = tmp_path / "named-fx"
    named.write_text("")
    packaged = tmp_path / "packaged" / "grida-fx"
    packaged.parent.mkdir()
    packaged.write_text("")
    on_path = tmp_path / "path"
    on_path.mkdir()
    (on_path / "grida-fx").write_text("#!/bin/sh\n")
    (on_path / "grida-fx").chmod(0o755)
    monkeypatch.setattr(_api, "_packaged_binary", lambda: packaged)
    monkeypatch.setenv("PATH", str(on_path))
    monkeypatch.setenv("GRIDA_FX_BIN", str(named))
    assert _api.binary() == named
    monkeypatch.delenv("GRIDA_FX_BIN")
    assert _api.binary() == packaged
    packaged.unlink()
    assert _api.binary() == on_path / "grida-fx"
    monkeypatch.setenv("PATH", str(tmp_path / "nowhere"))
    with pytest.raises(RuntimeError, match="GRIDA_FX_BIN"):
        _api.binary()


def test_a_named_binary_that_is_not_there_is_an_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("GRIDA_FX_BIN", str(tmp_path / "missing"))
    with pytest.raises(RuntimeError) as raised:
        _api.binary()
    assert str(raised.value) == f"GRIDA_FX_BIN is {tmp_path / 'missing'}, which is not a file"


def test_a_relative_binary_is_taken_from_the_current_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / "fx-bin").write_text("")
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("GRIDA_FX_BIN", "fx-bin")
    assert _api.binary() == tmp_path / "fx-bin"


def test_the_packaged_binary_sits_in_the_package() -> None:
    packaged = _api._packaged_binary()
    assert packaged.parent == Path(fx.__file__).resolve().parent / "_bin"
    assert packaged.name in ("grida-fx", "grida-fx.exe")


# ------------------------------------------------------------------------------------------------
# Planning


def test_plan_sends_the_target_and_its_options(fake: Fake, project: Path) -> None:
    planned = fx.plan(
        "workflows/gallery.yaml",
        inputs={
            "poster": Path("inputs/poster.png"),
            "count": 3,
            "note": "a\u2028b\x85c\ufeffd\x7fe",
        },
        input_files=["inputs/base.yaml", Path("inputs/more.yaml")],
        max_usd=2.5,
        cwd=project,
        routes=["routes.yaml"],
        arguments={"level": "docks"},
    )
    plan_call, price_call = fake.calls
    assert plan_call["cwd"] == os.path.realpath(project)
    inputs_file = plan_call["argv"][4].split("=", 1)[1]
    assert inputs_file.startswith(".grida-fx-inputs-") and inputs_file.endswith(".yaml")
    common = [
        "workflows/gallery.yaml",
        "--inputs=inputs/base.yaml",
        "--inputs=inputs/more.yaml",
        f"--inputs={inputs_file}",
        "--routes=routes.yaml",
        "--arg=level=docks",
        "--max-usd=2.5",
    ]
    assert plan_call["argv"] == ["plan", *common, "--json"]
    assert price_call["argv"] == ["price", *common]
    text = plan_call["inputs"][inputs_file]
    note = "a\\u2028b\\u0085c\\ufeffd\\u007fe"
    assert text == f'{{"poster": "inputs/poster.png", "count": 3, "note": "{note}"}}\n'
    assert json.loads(text) == {
        "poster": "inputs/poster.png",
        "count": 3,
        "note": "a\u2028b\x85c\ufeffd\x7fe",
    }
    assert not (project / inputs_file).exists(), "the inputs file is removed"
    assert planned.document == GRAPH
    assert planned.price == PRICE


def test_plan_with_nothing_but_a_target(fake: Fake, project: Path) -> None:
    fx.plan("gallery", cwd=project)
    assert [call["argv"] for call in fake.calls] == [
        ["plan", "gallery", "--json"],
        ["price", "gallery"],
    ]


def test_plan_runs_in_the_current_directory_by_default(
    fake: Fake, project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(project)
    fx.plan(project / "workflows" / "gallery.yaml")
    assert fake.calls[0]["cwd"] == os.path.realpath(project)
    assert fake.calls[0]["argv"][1] == str(project / "workflows" / "gallery.yaml")


def test_the_engine_falls_back_to_this_python_for_node_bodies(
    fake: Fake, project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("GRIDA_FX_PYTHON", raising=False)
    monkeypatch.delenv("GRIDA_FX_SDK_PYTHON", raising=False)
    fx.plan("gallery", cwd=project)
    fake.stands_in([])
    fx.run("gallery", cwd=project, stand_in=lambda call: fx.DECLINE)
    calls = [call for call in fake.calls if "argv" in call]
    # The SDK names itself as the last choice; it never sets GRIDA_FX_PYTHON, so a project's
    # .venv still wins over it.
    assert [call["sdk_python"] for call in calls] == [sys.executable] * 3
    assert [call["python"] for call in calls] == [None] * 3
    assert "GRIDA_FX_SDK_PYTHON" not in os.environ
    monkeypatch.setenv("GRIDA_FX_PYTHON", "/opt/other/python")
    fx.plan("gallery", cwd=project)
    assert fake.calls[-1]["python"] == "/opt/other/python"
    assert fake.calls[-1]["sdk_python"] == sys.executable


@pytest.mark.parametrize(
    ("amount", "text"), [(2, "2"), (2.0, "2.0"), (0.1, "0.1"), (1e-07, "0.0000001"), ("3", "3")]
)
def test_amounts_are_written_as_decimals(fake: Fake, project: Path, amount: Any, text: str) -> None:
    fx.plan("gallery", max_usd=amount, cwd=project)
    assert fake.calls[0]["argv"][-2] == f"--max-usd={text}"


def test_an_amount_is_a_number(project: Path) -> None:
    with pytest.raises(TypeError, match="max_usd is an amount of US dollars, not bool"):
        fx.plan("gallery", max_usd=True, cwd=project)


def test_inputs_must_be_json_values(fake: Fake, project: Path) -> None:
    with pytest.raises(ValueError):
        fx.plan("gallery", inputs={"n": float("nan")}, cwd=project)
    with pytest.raises(TypeError):
        fx.plan("gallery", inputs={"n": object()}, cwd=project)
    with pytest.raises(TypeError, match="keys in inputs are strings, not int"):
        fx.plan("gallery", inputs={"n": {1: "x"}}, cwd=project)
    assert fake.calls == []
    assert not list(project.glob(".grida-fx-inputs-*"))


def test_a_plan_reads_the_engines_documents(fake: Fake, project: Path) -> None:
    planned = fx.plan("gallery", cwd=project)
    assert planned.ok
    assert planned.problems == []
    assert planned.estimate() == (0.12, 0.5)
    assert planned.phases() == PRICE["phases"]
    assert repr(planned) == "<Plan gallery ok=True estimate=$0.12-$0.50>"


def test_a_plan_with_problems_is_not_ok(fake: Fake, project: Path) -> None:
    fake.reply(plan={"stdout": json.dumps(REFUSED_GRAPH)}, price={"stdout": "{}", "status": 1})
    planned = fx.plan("gallery", cwd=project)
    assert not planned.ok
    assert planned.problems == REFUSED_GRAPH["problems"]
    assert planned.estimate() == (0.12, 0.5)
    assert planned.phases() == []


def test_an_engine_error_raises_its_message(fake: Fake, project: Path) -> None:
    fake.reply(plan={"stderr": "grida-fx: no workflow nope here\n", "status": 2})
    with pytest.raises(fx.FxError) as raised:
        fx.plan("nope", cwd=project)
    assert str(raised.value) == "no workflow nope here"
    assert raised.value.status == 2
    assert len(fake.calls) == 1


def test_an_unexpected_exit_raises(fake: Fake, project: Path) -> None:
    fake.reply(plan={"stderr": "thread panicked", "status": 101})
    with pytest.raises(fx.FxError) as raised:
        fx.plan("gallery", cwd=project)
    assert str(raised.value) == "grida-fx plan exited with status 101: thread panicked"
    fake.reply(plan={"stdout": "not json"})
    with pytest.raises(fx.FxError, match="grida-fx plan exited with status 0: not json"):
        fx.plan("gallery", cwd=project)


def test_targets_that_cannot_be_planned(fake: Fake, project: Path) -> None:
    with pytest.raises(TypeError, match="cannot plan a Workflow object"):
        fx.plan(fx.Workflow("gallery", title="Gallery"), cwd=project)
    with pytest.raises(TypeError, match="cannot plan a Workflow object"):
        fx.run(fx.Workflow("gallery", title="Gallery"), cwd=project)
    with pytest.raises(TypeError, match="a target is a workflow file"):
        fx.plan(3, cwd=project)  # type: ignore[arg-type]
    with pytest.raises(ValueError, match="not made by grida.fx.plan"):
        fx.run(fx.Plan(GRAPH, PRICE))
    assert fake.calls == []


def test_a_plan_takes_no_new_planning_options(fake: Fake, project: Path) -> None:
    planned = fx.plan("gallery", cwd=project)
    with pytest.raises(TypeError) as raised:
        fx.run(planned, max_usd=5, inputs={"a": 1})
    assert "takes no inputs, max_usd" in str(raised.value)
    assert len(fake.calls) == 2


# ------------------------------------------------------------------------------------------------
# Running


def test_run_plans_then_runs_in_the_folder_given(fake: Fake, project: Path) -> None:
    fake.runs()
    result = fx.run(
        "gallery",
        inputs={"poster": "inputs/poster.png"},
        cwd=project,
        live=True,
        max_usd=2,
        yes_up_to=0.5,
        run_dir="runs/mine",
    )
    plan_call, price_call, run_call = fake.calls
    assert [plan_call["argv"][0], price_call["argv"][0]] == ["plan", "price"]
    inputs_file = run_call["argv"][2].split("=", 1)[1]
    assert run_call["argv"] == [
        "run",
        "gallery",
        f"--inputs={inputs_file}",
        "--max-usd=2",
        "--live",
        "--yes-up-to=0.5",
        "--run=runs/mine",
        "--no-view",
    ]
    assert run_call["inputs"] == {inputs_file: '{"poster": "inputs/poster.png"}\n'}
    assert run_call["cwd"] == os.path.realpath(project)
    assert result.run_dir == project / "runs" / "mine"
    assert result.events == EVENTS
    assert not list(project.glob(".grida-fx-inputs-*"))


def test_run_reads_the_folder_from_the_summary(fake: Fake, project: Path) -> None:
    fake.runs(folder="runs/gallery/2026-10-06-1")
    result = fx.run("gallery", cwd=project)
    assert fake.calls[-1]["argv"] == ["run", "gallery", "--no-view"]
    assert result.run_dir == project / "runs" / "gallery" / "2026-10-06-1"
    assert len(result.events) == len(EVENTS)


def test_a_run_result_reads_the_record(fake: Fake, project: Path) -> None:
    fake.runs()
    result = fx.run("gallery", cwd=project, run_dir="runs/mine")
    store = project.resolve() / "store" / "cache"
    assert not result.ok
    assert not result.incomplete
    assert result.cost == 0.25
    assert result.failed == ["poster#1"]
    outputs = result.outputs
    assert list(outputs) == ["plate", "images", "frames", "skipped", "count"]
    plate = outputs["plate"]
    assert isinstance(plate, RunFile)
    assert (plate.digest, plate.kind, plate.name, plate.size, plate.key) == (
        D1,
        "image/png",
        "plate/image",
        3,
        None,
    )
    assert plate.path == store / "files" / "11" / D1
    assert list(outputs["images"]) == ["ada", "../bo/x y"]
    assert outputs["images"]["ada"].path == store / "files" / "22" / D2
    assert outputs["images"]["ada"].key == "ada"
    assert [frame.digest for frame in outputs["frames"]] == [D1, D2]
    assert outputs["skipped"] is None
    assert outputs["count"] == 3
    steps = result.steps
    assert list(steps) == ["entity['ada'].review"]
    review = steps["entity['ada'].review"]
    assert isinstance(review, StepResult)
    assert review.id == "entity['ada'].review#2"
    assert review.facts == {"verdict": "accept", "cost_usd": 0.04}
    assert review.cache == "hit"
    assert review.outputs["notes"].path == store / "files" / "44" / D4


def test_a_run_that_ended_well(tmp_path: Path) -> None:
    events = [
        _envelope("run_started", workflow="w", resumed=False, ceiling_usd=None, charged_usd=0),
        _envelope(
            "run_finished",
            ok=True,
            incomplete=False,
            stopped=None,
            charged_usd=0,
            failed=[],
            outputs={},
        ),
    ]
    result = RunResult(tmp_path, events)
    assert (result.ok, result.incomplete, result.cost, result.failed) == (True, False, 0.0, [])
    assert result.outputs == {}
    assert result.steps == {}


def test_a_run_that_did_not_finish(tmp_path: Path) -> None:
    finished = _envelope(
        "run_finished",
        ok=True,
        incomplete=False,
        stopped=None,
        charged_usd=0.1,
        failed=[],
        outputs={"count": {"value": 1}},
    )
    cancelled = _envelope("run_cancelled", reason="interrupted", charged_usd=0.3)
    result = RunResult(tmp_path, [finished, cancelled])
    assert (result.ok, result.incomplete, result.cost, result.failed) == (False, True, 0.3, [])
    assert result.outputs == {}
    started = RunResult(tmp_path, [_envelope("run_started", workflow="w")])
    assert (started.ok, started.incomplete, started.cost) == (False, True, 0.0)


def test_the_store_without_a_project_file(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    folder = tmp_path / "bare"
    folder.mkdir()
    monkeypatch.chdir(folder)
    result = RunResult(folder, EVENTS)
    assert result.outputs["plate"].path == folder.resolve() / ".fx" / "cache" / "files" / "11" / D1


@pytest.mark.parametrize(
    ("text", "cache"),
    [
        ("fx: project/v1\n", ".fx/cache"),
        ("fx: project/v1\ncache: out/cache\n", "out/cache"),
        ('fx: project/v1\ncache: "out/my cache"  # quoted\n', "out/my cache"),
        ("fx: project/v1\ncache: 'it''s here'\n", "it's here"),
        ("fx: project/v1\nruns: runs\n  cache: nested\n", ".fx/cache"),
        ('{"fx": "project/v1", "cache": "json/cache"}', "json/cache"),
        ("\ufefffx: project/v1\ncache: ~\n", ".fx/cache"),
    ],
)
def test_the_store_is_the_projects_cache(tmp_path: Path, text: str, cache: str) -> None:
    (tmp_path / "fx.yaml").write_text(text, "utf-8")
    deeper = tmp_path / "a" / "b"
    deeper.mkdir(parents=True)
    assert _api._project_store(deeper) == tmp_path.resolve() / cache


def test_a_workflow_file_target_reads_the_store_of_its_own_project(
    fake: Fake, tmp_path: Path, project: Path
) -> None:
    # The engine plans a .yaml target in the project above the file, not the one above cwd.
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (elsewhere / "fx.yaml").write_text("fx: project/v1\ncache: other/cache\n", "utf-8")
    store = (project / "store" / "cache").resolve()

    def start(target: str) -> Path:
        request = _api._request(
            target,
            inputs=None,
            input_files=(),
            max_usd=None,
            cwd=elsewhere,
            routes=None,
            arguments=None,
        )
        return _api._project_store(_api._planning_start(request))

    (project / "workflows" / "gallery.yaml").write_text("fx: workflow/v1\n", "utf-8")
    assert start("../project/workflows/gallery.yaml") == store
    assert start(str(project / "workflows" / "gallery.yml")) == store
    # An id, a builder or another suffix: the project above cwd.
    other = (elsewhere / "other" / "cache").resolve()
    assert start("gallery") == other
    assert start("../project/build.py:build") == other
    assert start("../project/workflows/gallery.YAML") == other

    # And a run of that target reads its files there.
    fake.runs()
    result = fx.run("../project/workflows/gallery.yaml", cwd=elsewhere, run_dir="runs/one")
    assert result.outputs["plate"].path == store / "files" / "11" / D1


def test_a_torn_last_line_is_left_out(fake: Fake, project: Path) -> None:
    fake.runs(torn='{"kind": "fx-run-events-v1", "eve')
    result = fx.run("gallery", cwd=project, run_dir="runs/torn")
    assert result.events == EVENTS


def test_a_broken_line_inside_the_record_is_an_error(tmp_path: Path) -> None:
    (tmp_path / "events.jsonl").write_text('{"event": "run_started"}\nnot json\n{}\n', "utf-8")
    with pytest.raises(fx.FxError, match="line 2 of"):
        _api._read_events(tmp_path)


def test_a_refused_plan_runs_nothing(fake: Fake, project: Path) -> None:
    fake.reply(plan={"stdout": json.dumps(REFUSED_GRAPH)}, price={"stdout": "{}", "status": 1})
    fake.runs()
    with pytest.raises(fx.PlanRefused) as raised:
        fx.run("gallery", cwd=project, run_dir="runs/never")
    assert str(raised.value) == (
        "the plan is refused:\nsteps.draw.with.prompt: names nothing\ninputs: poster is missing"
    )
    assert not raised.value.plan.ok
    assert [call["argv"][0] for call in fake.calls] == ["plan", "price"]
    assert not (project / "runs").exists()


def test_a_plan_runs_as_it_was_planned(fake: Fake, project: Path) -> None:
    fake.runs()
    planned = fx.plan("gallery", cwd=project, max_usd=1, arguments={"n": "2"})
    result = fx.run(planned, live=True, run_dir="runs/p")
    calls = fake.calls
    assert [call["argv"][0] for call in calls] == ["plan", "price", "run"]
    assert calls[-1]["argv"] == [
        "run",
        "gallery",
        "--arg=n=2",
        "--max-usd=1",
        "--live",
        "--run=runs/p",
        "--no-view",
    ]
    assert calls[-1]["cwd"] == os.path.realpath(project)
    assert result.run_dir == project / "runs" / "p"


def test_a_refused_plan_object_does_not_run(fake: Fake, project: Path) -> None:
    fake.reply(plan={"stdout": json.dumps(REFUSED_GRAPH)}, price={"stdout": "{}", "status": 1})
    planned = fx.plan("gallery", cwd=project)
    with pytest.raises(fx.PlanRefused):
        fx.run(planned)
    assert len(fake.calls) == 2


def test_a_run_the_engine_refuses(fake: Fake, project: Path) -> None:
    refusal = "refused: another invocation is running runs/x"
    fake.reply(run={"stdout": f"plan text\n{refusal}\n", "status": 1})
    with pytest.raises(fx.FxError) as raised:
        fx.run("gallery", cwd=project, run_dir="runs/x")
    assert str(raised.value) == refusal


def test_a_plan_refused_by_the_run_itself(fake: Fake, project: Path) -> None:
    fake.reply(run={"stdout": "the plan, with its problems\n", "status": 1})
    calls_before = len(fake.calls)
    # The engine planned again and found problems: the SDK plans once more to say which.
    plans = iter([GRAPH, REFUSED_GRAPH])

    original = _api._plan

    async def plan_twice(request: Any) -> Any:
        planned = await original(request)
        planned.document = next(plans)
        return planned

    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(_api, "_plan", plan_twice)
        with pytest.raises(fx.PlanRefused):
            fx.run("gallery", cwd=project)
    assert [call["argv"][0] for call in fake.calls[calls_before:]] == [
        "plan",
        "price",
        "run",
        "plan",
        "price",
    ]


def test_a_run_engine_error_and_a_cancelled_run(fake: Fake, project: Path) -> None:
    fake.reply(run={"stderr": "grida-fx: the store at .fx/cache is not writable\n", "status": 2})
    with pytest.raises(fx.FxError) as raised:
        fx.run("gallery", cwd=project)
    assert str(raised.value) == "the store at .fx/cache is not writable"
    fake.reply(run={"status": 130})
    with pytest.raises(fx.FxError) as cancelled:
        fx.run("gallery", cwd=project)
    assert (str(cancelled.value), cancelled.value.status) == ("the run was cancelled", 130)


def test_run_refuses_a_running_event_loop(fake: Fake, project: Path) -> None:
    async def inside() -> None:
        with pytest.raises(RuntimeError, match="await grida.fx.run_async"):
            fx.run("gallery", cwd=project)
        with pytest.raises(RuntimeError, match="await grida.fx.plan_async"):
            fx.plan("gallery", cwd=project)

    asyncio.run(inside())
    assert fake.calls == []


def test_run_async_inside_an_event_loop(fake: Fake, project: Path) -> None:
    fake.runs()

    async def inside() -> RunResult:
        planned = await fx.plan_async("gallery", cwd=project)
        assert planned.ok
        return await fx.run_async("gallery", cwd=project, run_dir="runs/a")

    result = asyncio.run(inside())
    assert result.failed == ["poster#1"]


def test_cancelling_a_run_interrupts_the_engine(fake: Fake, project: Path) -> None:
    fake.reply(run={"sleep": 30})

    async def cancel() -> None:
        task = asyncio.ensure_future(fx.run_async("gallery", cwd=project))
        deadline = time.monotonic() + 20
        while not any(call.get("argv", [""])[0] == "run" for call in fake.calls):
            assert time.monotonic() < deadline, "the run never started"
            await asyncio.sleep(0.05)
        await asyncio.sleep(0.3)  # let the fake install its interrupt handler
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task

    asyncio.run(cancel())
    assert {"interrupted": True} in fake.calls


def test_repeated_run_task_cancellation_retains_cleanup_ownership(
    fake: Fake, project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    gate, ready = project / "cleanup", project / "ready"
    fake.reply(run={"sleep": 30, "cleanup_gate": str(gate), "ready_file": str(ready)})
    # A much shorter non-run deadline proves that run cancellation never uses that timer.
    monkeypatch.setattr(_api, "_STOP_GRACE_S", 0.03)

    async def scenario() -> None:
        task = asyncio.create_task(fx.run_async("gallery", cwd=project))
        try:
            async with asyncio.timeout(10):
                while not ready.exists():
                    await asyncio.sleep(0.01)
            task.cancel()
            async with asyncio.timeout(10):
                while {"interrupted": True} not in fake.calls:
                    await asyncio.sleep(0.01)
            task.cancel()
            await asyncio.sleep(0.1)
            assert not task.done(), "a cancelled run detached before engine cleanup"
            assert [call for call in fake.calls if "signal" in call] == [
                {"signal": signal.SIGTERM, "source": "sdk"}
            ]
        finally:
            gate.touch()
            with pytest.raises(asyncio.CancelledError):
                await asyncio.wait_for(task, 10)

    asyncio.run(scenario())
    assert {"cleanup": "complete"} in fake.calls


def test_run_cancellation_while_process_creation_is_pending_keeps_an_owner(
    project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    async def scenario() -> None:
        creation = asyncio.Event()
        released = asyncio.Event()
        observed: list[bool] = []

        async def owned(
            args: object, cwd: object, stdin: object, stop: asyncio.Event
        ) -> _api._Exit:
            creation.set()
            await released.wait()
            observed.append(stop.is_set())
            return _api._Exit(130, "", "")

        monkeypatch.setattr(_api, "_call_owned", owned)
        task = asyncio.create_task(_api._call(["run", "gallery"], project))
        await creation.wait()
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done()
        released.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert observed == [True]

    asyncio.run(scenario())


def test_cancelling_only_a_control_wait_does_not_signal_its_run(fake: Fake, project: Path) -> None:
    ready = project / "control-ready"
    # The independently owned runner is a separate process with its own interrupt evidence.
    runner_log = project / "runner-signals"
    runner = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import signal,sys,time; "
            "signal.signal(signal.SIGTERM, lambda *args: open(sys.argv[1], 'w').write('stop')); "
            "signal.signal(signal.SIGINT, lambda *args: open(sys.argv[1], 'w').write('stop')); "
            "time.sleep(30)",
            str(runner_log),
        ],
        start_new_session=True,
    )
    fake.reply(cancel={"sleep": 30, "ready_file": str(ready)})

    async def scenario() -> None:
        task = asyncio.create_task(fx.cancel_async("runs/a", cwd=project, wait=True))
        async with asyncio.timeout(10):
            while not ready.exists():
                await asyncio.sleep(0.01)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 10)

    try:
        asyncio.run(scenario())
        assert runner.poll() is None
        assert not runner_log.exists()
        assert {"interrupted": True} in fake.calls
    finally:
        runner.kill()
        runner.wait(timeout=10)


def _control_result(operation: str = "cancel", **fields: Any) -> dict[str, Any]:
    result = {
        "kind": "fx-run-control-v1",
        "operation": operation,
        "invocation_id": "inv-1",
        "outcome": "accepted" if operation == "cancel" else "inspected",
        "request_status": "accepted" if operation == "cancel" else "not_accepted",
        "recorded_state": "unfinished",
        "cleanup": "pending",
        "external_completion": "not_verified",
    }
    if operation == "inspect":
        result.update(availability="available", can_cancel=True)
    return {**result, **fields}


def test_control_wrappers_forward_exact_target_options_and_preserve_outcomes(
    fake: Fake, project: Path
) -> None:
    fake.reply(inspect={"stdout": json.dumps(_control_result("inspect"))})
    sampled = fx.inspect_control(Path("gallery/one"), cwd=project)
    assert isinstance(sampled, fx.RunControlResult)
    assert sampled.invocation_id == "inv-1" and sampled.can_cancel is True
    assert fake.invocations[-1] == ["inspect", "gallery/one", "--control", "--json"]
    fake.reply(cancel={"stdout": json.dumps(_control_result())})
    requested = fx.cancel("runs/one", cwd=project, invocation=sampled.invocation_id)
    assert requested.outcome == "accepted" and requested.cleanup == "pending"
    assert fake.invocations[-1] == [
        "cancel",
        "runs/one",
        "--source=sdk",
        "--invocation=inv-1",
        "--json",
    ]
    completed = _control_result(outcome="completed", recorded_state="failed", cleanup="complete")
    fake.reply(cancel={"stdout": json.dumps(completed)})
    result = fx.cancel("runs/one", cwd=project, wait=True, timeout="2m")
    assert result.outcome == "completed" and result.recorded_state == "failed"
    assert result.external_completion == "not_verified"
    assert fake.invocations[-1] == [
        "cancel",
        "runs/one",
        "--source=sdk",
        "--wait",
        "--timeout=2m",
        "--json",
    ]


def test_async_control_and_structured_errors(fake: Fake, project: Path) -> None:
    failure = _control_result(
        outcome="error", code="wait_timeout", message="local cleanup is still pending"
    )
    fake.reply(cancel={"stdout": json.dumps(failure), "status": 1})
    with pytest.raises(fx.RunControlError) as caught:
        asyncio.run(fx.cancel_async("runs/one", cwd=project, wait=True, timeout="1s"))
    assert caught.value.status == 1 and caught.value.code == "wait_timeout"
    assert caught.value.result.request_status == "accepted"
    assert caught.value.result.cleanup == "pending"
    inspected = _control_result("inspect", availability="unavailable", can_cancel=False)
    fake.reply(inspect={"stdout": json.dumps(inspected)})
    assert (
        asyncio.run(fx.inspect_control_async("runs/one", cwd=project)).availability == "unavailable"
    )


@pytest.mark.parametrize(
    ("fields", "status", "wait"),
    [
        ({"outcome": "future_success"}, 0, False),
        ({"outcome": "accepted"}, 2, False),
        ({"outcome": "completed"}, 0, False),
        ({"outcome": "accepted"}, 0, True),
        ({"outcome": "completed", "cleanup": "pending", "recorded_state": "cancelled"}, 0, True),
        ({"outcome": "completed", "cleanup": "complete", "recorded_state": "unfinished"}, 0, True),
        (
            {
                "outcome": "completed",
                "cleanup": "complete",
                "recorded_state": "cancelled",
                "invocation_id": None,
            },
            0,
            True,
        ),
        ({"outcome": "error", "code": "future_code", "message": "unknown"}, 0, False),
        ({"outcome": "error", "code": "wait_timeout", "message": "timed out"}, 2, True),
        ({"invocation_id": "x" * 257}, 0, False),
        ({"external_completion": "complete"}, 0, False),
    ],
)
def test_unknown_or_mismatched_control_responses_are_never_success(
    fake: Fake, project: Path, fields: dict[str, Any], status: int, wait: bool
) -> None:
    fake.reply(cancel={"stdout": json.dumps(_control_result(**fields)), "status": status})
    with pytest.raises(fx.FxError) as caught:
        fx.cancel("runs/one", cwd=project, wait=wait)
    assert not isinstance(caught.value, fx.RunControlError)


def test_unsupported_control_binary_and_compatible_extensions(fake: Fake, project: Path) -> None:
    fake.reply(cancel={"stderr": "grida-fx: unknown command 'cancel'\n", "status": 2})
    with pytest.raises(fx.FxError, match="unknown command") as caught:
        fx.cancel("runs/one", cwd=project)
    assert not isinstance(caught.value, fx.RunControlError)
    fake.reply(cancel={"stdout": json.dumps(_control_result(extension={"future": True}))})
    assert fx.cancel("runs/one", cwd=project).outcome == "accepted"


@pytest.mark.parametrize(
    "options",
    [
        {"timeout": "1s"},
        {"wait": True, "timeout": "0s"},
        {"wait": True, "timeout": "1.5s"},
        {"wait": "yes"},
        {"invocation": ""},
    ],
)
def test_invalid_control_options_are_refused_before_starting_an_engine(
    fake: Fake, project: Path, options: dict[str, Any]
) -> None:
    with pytest.raises((TypeError, ValueError)):
        fx.cancel("runs/one", cwd=project, **options)
    assert fake.invocations == []


def test_a_process_that_keeps_the_engines_pipes_does_not_hold_the_run(
    fake: Fake, project: Path
) -> None:
    fake.runs(linger=60)
    started = time.monotonic()
    try:
        result = fx.run("gallery", cwd=project, run_dir="runs/a")
        elapsed = time.monotonic() - started
    finally:
        for call in fake.calls:
            if "lingering" in call:
                with contextlib.suppress(ProcessLookupError):
                    os.kill(call["lingering"], signal.SIGKILL)
    assert any("lingering" in call for call in fake.calls)
    # The engine's summary was read, and the call ended with the engine, not with the process.
    assert result.failed == ["poster#1"]
    assert elapsed < 20


# ------------------------------------------------------------------------------------------------
# Stand-ins


K1 = "a" * 64
K2 = "b" * 64
K3 = "c" * 64
FINGERPRINT = "e" * 64
PNG = b"\x89PNG\r\n\x1a\nstand-in"


def _ask(
    key: str = K1,
    *,
    capability: str = "image.generate",
    request: dict[str, Any] | None = None,
    files: dict[str, Any] | None = None,
    take: tuple[int, ...] = (1,),
    instance: str = "draw#1",
    step: str = "draw",
) -> dict[str, Any]:
    """A scripted ``stand_in.answer``, checked against its definition."""
    params = {
        "capability": capability,
        "route": {"id": "img-a@acme", "fingerprint": FINGERPRINT},
        "request": {"prompt": "a lighthouse", "size": "64x64"} if request is None else request,
        "take": list(take),
        "key": key,
        "instance": {"id": instance, "path": instance.rsplit("#", 1)[0], "step": step},
        "files": files or {},
    }
    _valid(ASK_PARAMS, params)
    return {"ask": params}


def _key(number: int) -> str:
    return format(number, "064x")


def _ref(folder: Path, data: bytes, kind: str, name: str) -> dict[str, Any]:
    """A file ref, as the engine hands one over, of ``data`` written into ``folder``."""
    digest = hashlib.sha256(data).hexdigest()
    folder.mkdir(parents=True, exist_ok=True)
    (folder / digest).write_bytes(data)
    return {
        "digest": digest,
        "kind": kind,
        "size": len(data),
        "name": name,
        "path": str(folder / digest),
        "facts": {"bytes": len(data), "kind": kind},
    }


def _b64(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


CANCELLED = {"code": -32002, "message": "the engine cancelled stand_in.answer"}
STAND_IN_EVENTS = [
    _envelope(
        "run_started",
        workflow="gallery",
        resumed=False,
        ceiling_usd=None,
        charged_usd=0,
        estimate={"low_usd": 0, "high_usd": 0.04},
        stand_in=True,
    ),
    _envelope(
        "call",
        id="draw#1",
        capability="image.generate",
        route="img-a@acme",
        call=K1,
        cached=False,
        cost_usd=0,
        stand_in=True,
    ),
    _envelope(
        "node_finished",
        id="draw#1",
        path="draw",
        cache="miss",
        outputs={"image": _file(D1, "image/png", "draw/image")},
        facts={"cost_usd": 0},
    ),
    _envelope(
        "run_finished",
        ok=True,
        incomplete=False,
        stopped=None,
        charged_usd=0,
        failed=[],
        outputs={"plate": _file(D1, "image/png", "draw/image")},
    ),
]


def test_the_fixture_events_are_run_events() -> None:
    for event in [*EVENTS, *STAND_IN_EVENTS]:
        _valid(EVENTS_SCHEMA, event)


def test_a_stand_in_run_serves_the_engine_on_its_standard_input(fake: Fake, project: Path) -> None:
    fake.stands_in([_ask(), {"take": 1}])
    asked: list[fx.StandInCall] = []

    def answer(call: fx.StandInCall) -> fx.Answer:
        asked.append(call)
        return fx.Answer(files={"image": PNG})

    result = fx.run("gallery", cwd=project, max_usd=1, run_dir="runs/s", stand_in=answer)
    # No plan and no price first: the run plans.
    assert fake.invocations == [
        ["run", "gallery", "--max-usd=1", "--run=runs/s", "--stand-in=-", "--no-view"]
    ]
    initialized, answered, shut_down = fake.stand_in
    assert initialized["result"]["protocol"] == "fx-node-protocol-v1"
    assert initialized["result"]["host"]["language"] == "python"
    assert answered == {
        "jsonrpc": "2.0",
        "id": 2,
        "result": {"files": {"image": {"base64": _b64(PNG)}}, "data": None},
    }
    assert shut_down == {"jsonrpc": "2.0", "id": 3, "result": None}
    assert [call.key for call in asked] == [K1]
    # The result reads the stand-in store.
    assert result.stand_in
    assert result.ok
    store = project.resolve() / "store" / "cache" / "stand-in"
    assert result._store_root() == store
    assert result.outputs["plate"].path == store / "files" / "11" / D1
    assert result.steps["draw"].outputs["image"].path == store / "files" / "11" / D1


def test_a_stand_in_run_refuses_what_it_cannot_be_before_starting(
    fake: Fake, project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def answer(call: fx.StandInCall) -> fx.Answer:
        return fx.Answer()

    with pytest.raises(ValueError) as live:
        fx.run("gallery", cwd=project, live=True, stand_in=answer)
    assert str(live.value) == "a stand-in run is never live: pass live=False, or no stand_in"
    with pytest.raises(ValueError) as gated:
        fx.run("gallery", cwd=project, yes_up_to=1, stand_in=answer)
    assert str(gated.value) == "a stand-in run runs every phase: yes_up_to does not apply"
    with pytest.raises(TypeError) as not_callable:
        fx.run("gallery", cwd=project, stand_in=fx.Answer())  # type: ignore[arg-type]
    assert str(not_callable.value) == (
        "stand_in is a function of one grida.fx.StandInCall, not Answer"
    )
    monkeypatch.setattr(_api, "os", types.SimpleNamespace(name="nt"))
    with pytest.raises(NotImplementedError, match="stand-ins need a POSIX system"):
        _api._answerer(answer, live=False, yes_up_to=None)
    assert fake.calls == []


def test_a_plan_runs_with_a_stand_in(fake: Fake, project: Path) -> None:
    fake.stands_in([])
    planned = fx.plan("gallery", cwd=project)
    result = fx.run(planned, run_dir="runs/p", stand_in=lambda call: fx.DECLINE)
    assert fake.invocations[-1] == ["run", "gallery", "--run=runs/p", "--stand-in=-", "--no-view"]
    assert [argv[0] for argv in fake.invocations] == ["plan", "price", "run"]
    assert result.stand_in


def test_a_stand_in_run_whose_plan_has_problems(fake: Fake, project: Path) -> None:
    # The run refuses the plan (exit 1, no run line); the SDK plans then, to say why.
    fake.reply(
        plan={"stdout": json.dumps(REFUSED_GRAPH)},
        price={"stdout": "{}", "status": 1},
        run={"stdout": "the plan, with its problems\n", "status": 1},
    )
    with pytest.raises(fx.PlanRefused):
        fx.run("gallery", cwd=project, stand_in=lambda call: fx.DECLINE)
    assert [argv[0] for argv in fake.invocations] == ["run", "plan", "price"]


def test_a_stand_in_sees_the_call(fake: Fake, project: Path, tmp_path: Path) -> None:
    front = _ref(tmp_path / "store", b"front", "image/png", "front.png")
    back = _ref(tmp_path / "store", b"back", "image/png", "back.png")
    request = {
        "image": {"file": front["digest"]},
        "views": {"front": {"file": front["digest"]}, "back": {"file": back["digest"]}},
        "messages": [{"role": "user", "content": "look", "images": [{"file": back["digest"]}]}],
        "note": {"file": "a.png"},
        "size": "64x64",
    }
    ask = _ask(
        K1,
        capability="mesh.generate",
        request=request,
        files={front["digest"]: front, back["digest"]: back},
        take=(2, 3),
        instance="model['ada']#3",
        step="model",
    )
    fake.stands_in([ask, {"take": 1}])
    seen: list[fx.StandInCall] = []

    def answer(call: fx.StandInCall) -> Any:
        seen.append(call)
        return fx.DECLINE

    fx.run("gallery", cwd=project, stand_in=answer)
    assert fake.stand_in[1]["result"] == {"decline": True}
    (call,) = seen
    assert call.capability == "mesh.generate"
    assert (call.route.id, call.route.fingerprint) == ("img-a@acme", FINGERPRINT)
    assert call.key == K1
    assert (call.takes, call.take) == ((2, 3), 3)
    assert (call.instance.id, call.instance.path, call.instance.step) == (
        "model['ada']#3",
        "model['ada']",
        "model",
    )
    assert set(call.files) == {front["digest"], back["digest"]}
    assert all(isinstance(file, fx.InputFile) for file in call.files.values())
    # Every file value, at any depth, is the InputFile of its digest.
    assert call.request["image"] is call.files[front["digest"]]
    views = call.request["views"]
    assert (views["front"].read_bytes(), views["back"].read_bytes()) == (b"front", b"back")
    (image,) = call.request["messages"][0]["images"]
    assert (image.digest, image.kind, image.name) == (back["digest"], "image/png", "back.png")
    assert image.facts == {"bytes": 4, "kind": "image/png"}
    # {"file": "a.png"} is data, not a file value; params are as they came.
    assert call.request["note"] == {"file": "a.png"}
    assert call.request["size"] == "64x64"
    assert call.params["request"] == request
    assert repr(call) == "<StandInCall mesh.generate on img-a@acme for model['ada']#3 take 3>"


def test_stand_in_answers_go_out_as_the_protocol_says(
    fake: Fake, project: Path, tmp_path: Path
) -> None:
    source = _ref(tmp_path / "store", PNG, "image/png", "source.png")
    picture = tmp_path / "picture.WEBP"
    picture.write_bytes(b"webp bytes")
    script: list[Any] = [
        fx.Answer(files={"image": PNG}),
        fx.Answer(files={"image": fx.Output(kind="image/webp", data=b"webp")}),
        fx.Answer(files={"image": fx.InputFile(source)}),
        fx.Answer(files={"image": picture}),
        fx.Answer.json({"title": "Lighthouse"}),
        fx.Answer.turn(
            "thinking",
            [{"name": "look", "arguments": {"at": "sea"}}, {"id": "mine", "name": "look"}],
        ),
        fx.Answer.submit(caption="a lighthouse", score=3),
        fx.DECLINE,
        fx.CallRefused("no route takes a lighthouse"),
        fx.CallFailed("the picture never came"),
    ]

    def answer(call: fx.StandInCall) -> Any:
        scripted = script.pop(0)
        if isinstance(scripted, Exception):
            raise scripted
        return scripted

    asks = [_ask(_key(number)) for number in range(1, 11)]
    fake.stands_in([*asks, {"take": 10}])
    fx.run("gallery", cwd=project, stand_in=answer)
    received = fake.stand_in[1:-1]
    assert [message["id"] for message in received] == list(range(2, 12))
    assert [message.get("result", message.get("error")) for message in received] == [
        {"files": {"image": {"base64": _b64(PNG)}}, "data": None},
        {"files": {"image": {"base64": _b64(b"webp"), "kind": "image/webp"}}, "data": None},
        {"files": {"image": {"base64": _b64(PNG), "kind": "image/png"}}, "data": None},
        {"files": {"image": {"base64": _b64(b"webp bytes"), "kind": "image/webp"}}, "data": None},
        {"files": {}, "data": {"json": {"title": "Lighthouse"}}},
        {
            "files": {},
            "data": {
                "text": "thinking",
                "tool_calls": [
                    {"id": "call_1", "name": "look", "arguments": {"at": "sea"}},
                    {"id": "mine", "name": "look", "arguments": {}},
                ],
            },
        },
        {
            "files": {},
            "data": {
                "text": "",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "name": "submit",
                        "arguments": {"caption": "a lighthouse", "score": 3},
                    }
                ],
            },
        },
        {"decline": True},
        {"code": -32015, "message": "no route takes a lighthouse"},
        {"code": -32016, "message": "the picture never came"},
    ]
    assert script == []


def test_an_async_stand_in_keeps_its_state_in_this_process(fake: Fake, project: Path) -> None:
    counts: collections.Counter[str] = collections.Counter()
    asks = [_ask(K1), _ask(K2, capability="structured.generate"), _ask(K3)]
    fake.stands_in([*asks, {"take": 3}])

    async def answer(call: fx.StandInCall) -> fx.Answer:
        await asyncio.sleep(0)
        counts[call.capability] += 1
        if call.capability == "structured.generate":
            return fx.Answer.json({"n": counts[call.capability]})
        return fx.Answer(files={"image": PNG})

    fx.run("gallery", cwd=project, stand_in=answer)
    assert counts == {"image.generate": 2, "structured.generate": 1}
    assert fake.stand_in[2]["result"] == {"files": {}, "data": {"json": {"n": 1}}}


@pytest.mark.parametrize("kind", ["plain", "async"])
def test_stand_in_calls_are_answered_one_at_a_time_in_arrival_order(
    fake: Fake, project: Path, kind: str
) -> None:
    order: list[str] = []
    running = [0, 0]  # now, at most
    guard = threading.Lock()

    def enter(call: fx.StandInCall) -> None:
        with guard:
            order.append(call.key)
            running[0] += 1
            running[1] = max(running)

    def leave() -> fx.Answer:
        with guard:
            running[0] -= 1
        return fx.Answer(files={"image": PNG})

    def plain(call: fx.StandInCall) -> fx.Answer:
        enter(call)
        time.sleep(0.05)
        return leave()

    async def awaited(call: fx.StandInCall) -> fx.Answer:
        enter(call)
        await asyncio.sleep(0.05)
        return leave()

    fake.stands_in([_ask(K1), _ask(K2), _ask(K3), {"take": 3}])
    fx.run("gallery", cwd=project, stand_in=plain if kind == "plain" else awaited)
    assert order == [K1, K2, K3]
    assert running == [0, 1]
    assert [message["id"] for message in fake.stand_in[1:-1]] == [2, 3, 4]


def test_cancel_of_a_stand_in_call_waiting_its_turn(fake: Fake, project: Path) -> None:
    asked: list[str] = []

    def answer(call: fx.StandInCall) -> fx.Answer:
        asked.append(call.key)
        time.sleep(0.5)
        return fx.Answer(files={"image": PNG})

    # Ids: initialize 1, K1 2, K2 3.
    script = [_ask(K1), _ask(K2), {"cancel": 3}, {"take": 2}]
    fake.stands_in(script)
    fx.run("gallery", cwd=project, stand_in=answer)
    cancelled, answered = fake.stand_in[1:-1]
    assert cancelled == {
        "jsonrpc": "2.0",
        "id": 3,
        "error": CANCELLED,
    }
    assert answered["id"] == 2 and "result" in answered
    assert asked == [K1]


def test_cancel_of_a_running_stand_in_call(fake: Fake, project: Path) -> None:
    seen: list[str] = []

    async def awaited(call: fx.StandInCall) -> fx.Answer:
        try:
            await asyncio.sleep(30)
        except asyncio.CancelledError:
            seen.append("cancelled")
            raise
        return fx.Answer(files={"image": PNG})

    script = [_ask(K1), {"sleep": 0.2}, {"cancel": 2}, {"take": 1}]
    fake.stands_in(script)
    fx.run("gallery", cwd=project, stand_in=awaited)
    assert fake.stand_in[1]["error"] == CANCELLED
    assert seen == ["cancelled"]

    # A plain function already running finishes, and its answer goes out.
    def plain(call: fx.StandInCall) -> fx.Answer:
        time.sleep(0.6)
        seen.append("finished")
        return fx.Answer(files={"image": PNG})

    fx.run("gallery", cwd=project, stand_in=plain)
    assert fake.stand_in[1]["result"] == {"files": {"image": {"base64": _b64(PNG)}}, "data": None}
    assert seen == ["cancelled", "finished"]


def test_the_run_ends_with_the_engine_while_a_plain_stand_in_still_answers(
    fake: Fake, project: Path
) -> None:
    released = threading.Event()

    def blocked(call: fx.StandInCall) -> fx.Answer:
        released.wait(30)
        return fx.Answer(files={"image": PNG})

    # The second call waits for its turn behind the first; the engine exits without either.
    fake.stands_in([_ask(K1), _ask(K2), {"sleep": 0.3}])
    started = time.monotonic()
    try:
        result = fx.run("gallery", cwd=project, stand_in=blocked)
    finally:
        released.set()
    assert time.monotonic() - started < 5
    assert result.stand_in
    initialized, shut_down = fake.stand_in
    assert "result" in initialized
    assert shut_down["result"] is None


def test_the_run_ends_with_the_engine_while_a_coroutine_stand_in_still_answers(
    fake: Fake, project: Path
) -> None:
    async def asleep(call: fx.StandInCall) -> fx.Answer:
        await asyncio.sleep(30)
        return fx.Answer(files={"image": PNG})

    fake.stands_in([_ask(K1), _ask(K2), {"sleep": 0.3}])
    started = time.monotonic()
    result = fx.run("gallery", cwd=project, stand_in=asleep)
    assert time.monotonic() - started < 5
    assert result.stand_in
    assert len(fake.stand_in) == 2


def test_an_error_answer_holding_a_lone_surrogate_reaches_the_engine(
    fake: Fake, project: Path
) -> None:
    name = os.fsdecode(b"gull\xff.png")

    def refusing(call: fx.StandInCall) -> fx.Answer:
        raise fx.CallRefused("no file " + name)

    fake.stands_in([_ask(K1), {"take": 1}])
    fx.run("gallery", cwd=project, stand_in=refusing)
    assert fake.stand_in[1]["error"] == {
        "code": fx.CallRefused.code,
        "message": "no file gull\\udcff.png",
    }

    def faulting(call: fx.StandInCall) -> fx.Answer:
        raise RuntimeError("cannot read " + name)

    with pytest.raises(RuntimeError):
        fx.run("gallery", cwd=project, stand_in=faulting)
    assert fake.stand_in[1]["error"] == {
        "code": -32099,
        "message": "RuntimeError: cannot read gull\\udcff.png",
    }


def test_a_stand_in_run_without_an_engine_leaves_no_socket_open(
    project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("GRIDA_FX_BIN", str(project / "missing"))
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always", ResourceWarning)
        for _ in range(2):
            with pytest.raises(RuntimeError, match="GRIDA_FX_BIN"):
                fx.run("gallery", cwd=project, stand_in=lambda call: fx.DECLINE)
        gc.collect()
    unclosed = [w for w in caught if issubclass(w.category, ResourceWarning)]
    assert unclosed == [], [str(w.message) for w in unclosed]


def test_each_run_serves_its_own_stand_in(fake: Fake, project: Path) -> None:
    fake.stands_in([_ask(K1), {"take": 1}])
    first: list[str] = []
    second: list[str] = []
    fx.run("gallery", cwd=project, stand_in=lambda call: first.append(call.key) or fx.DECLINE)
    fx.run(
        "gallery",
        cwd=project,
        stand_in=lambda call: second.append(call.key) or fx.Answer(files={"image": PNG}),
    )
    assert (first, second) == ([K1], [K1])
    assert "result" in fake.stand_in[1] and fake.stand_in[1]["result"] != {"decline": True}


class _Stop(BaseException):
    """Not an Exception: what a stand-in raises is its fault all the same."""


@pytest.mark.parametrize(
    ("raised", "text"),
    [
        (RuntimeError("no picture for you"), "RuntimeError: no picture for you"),
        (_Stop(), "_Stop"),
    ],
)
def test_a_stand_in_that_raises_stops_the_run_and_run_raises_it(
    fake: Fake, project: Path, raised: BaseException, text: str
) -> None:
    script = [_ask(K1), {"take": 1}, _ask(K2), {"take": 1}]
    fake.stands_in(script)
    asked: list[str] = []

    def answer(call: fx.StandInCall) -> fx.Answer:
        asked.append(call.key)
        raise raised

    with pytest.raises(type(raised)) as caught:
        fx.run("gallery", cwd=project, run_dir="runs/s", stand_in=answer)
    assert caught.value is raised
    assert caught.value.__notes__ == [f"the stand-in run stopped in {project / 'runs' / 's'}"]
    failed, later = fake.stand_in[1:-1]
    assert failed["error"] == {"code": -32099, "message": text}
    # Later calls are answered without calling the stand-in again.
    assert later["error"] == {"code": -32099, "message": "the stand-in failed earlier"}
    assert asked == [K1]


@pytest.mark.parametrize(
    ("returned", "message"),
    [
        (None, "a stand-in returns an Answer or grida.fx.DECLINE, not None"),
        ({"files": {}}, "a stand-in returns an Answer or grida.fx.DECLINE, not dict"),
    ],
)
def test_a_stand_in_that_returns_something_else_is_a_fault(
    fake: Fake, project: Path, returned: Any, message: str
) -> None:
    fake.stands_in([_ask(K1), {"take": 1}])
    with pytest.raises(TypeError) as raised:
        fx.run("gallery", cwd=project, stand_in=lambda call: returned)
    assert str(raised.value) == message
    assert raised.value.__notes__ == [
        f"the stand-in run stopped in {project / 'runs' / 'gallery' / 'r1'}"
    ]
    assert fake.stand_in[1]["error"] == {"code": -32099, "message": f"TypeError: {message}"}


def test_the_stand_in_answers_only_its_own_methods(fake: Fake, project: Path) -> None:
    load = {"path": str(project / "stand_in.py"), "function": "answer"}
    initialize = {
        "protocol": "fx-node-protocol-v1",
        "engine": {"name": "grida-fx", "version": "0.0.0"},
        "project_root": str(project),
        "sources": [],
    }
    script = [
        {"request": "stand_in.load", "params": load},
        {"request": "initialize", "params": initialize},
        {"cancel": 99},
        {"take": 2},
    ]
    fake.stands_in(script)
    fx.run("gallery", cwd=project, stand_in=lambda call: fx.DECLINE)
    assert [message["error"] for message in fake.stand_in[1:-1]] == [
        {"code": -32601, "message": "grida.fx's stand-in has no method stand_in.load"},
        {"code": -32600, "message": "initialize was already answered"},
    ]


def test_run_result_failures(tmp_path: Path) -> None:
    started = {
        "workflow": "w",
        "ceiling_usd": None,
        "charged_usd": 0,
        "estimate": {"low_usd": 0, "high_usd": 0},
        "stand_in": True,
    }
    second = {"invocation_id": "inv-2"}
    events = [
        _envelope("run_started", resumed=False, **started),
        _envelope(
            "node_failed",
            id="draw#1",
            path="draw",
            error="image.generate on img-a@acme failed: the stand-in said no",
            code="call_failed",
            facts={"tries": 1},
        ),
        _envelope("node_failed", id="note#1", path="note", error="on purpose", code="node_failure"),
        _envelope(
            "run_finished",
            ok=False,
            incomplete=False,
            stopped=None,
            charged_usd=0,
            failed=["draw#1", "note#1"],
            outputs={},
        ),
        {**_envelope("run_started", resumed=True, **started), **second},
        {
            **_envelope(
                "node_failed",
                id="draw#1",
                path="draw",
                error="image.generate on img-a@acme failed: the image is 32x32, not 64x64",
                code="call_failed",
                facts={"cost_usd": 0},
            ),
            **second,
        },
        {
            **_envelope("node_failed", id="check#1", path="check", error="too dark", code=None),
            **second,
        },
        {
            **_envelope(
                "node_skipped",
                id="pack#1",
                path="pack",
                reason="something it reads failed",
                blocked=True,
            ),
            **second,
        },
        {
            **_envelope(
                "run_finished",
                ok=False,
                incomplete=True,
                stopped="the stand-in failed: RuntimeError: broken",
                charged_usd=0,
                failed=["check#1", "draw#1", "pack#1"],
                outputs={},
            ),
            **second,
        },
    ]
    for event in events:
        _valid(EVENTS_SCHEMA, event)
    result = RunResult(tmp_path, events)
    assert list(result.failures) == ["check#1", "draw#1", "pack#1"]
    assert result.failures["draw#1"] == fx.Failure(
        id="draw#1",
        path="draw",
        message="image.generate on img-a@acme failed: the image is 32x32, not 64x64",
        code="call_failed",
        facts={"cost_usd": 0},
        skipped=False,
    )
    assert result.failures["check#1"] == fx.Failure(
        id="check#1", path="check", message="too dark", code=None, facts={}, skipped=False
    )
    assert result.failures["pack#1"] == fx.Failure(
        id="pack#1",
        path="pack",
        message="something it reads failed",
        code=None,
        facts={},
        skipped=True,
    )
    assert result.stopped == "the stand-in failed: RuntimeError: broken"
    assert result.stand_in
    result._store = tmp_path / "cache"
    assert result._store_root() == tmp_path / "cache" / "stand-in"
    # Before a run_finished there are none.
    unfinished = RunResult(tmp_path, events[:3])
    assert (unfinished.failures, unfinished.stopped) == ({}, None)
    # A run without a stand-in, whose record predates `code`.
    plain = RunResult(tmp_path, EVENTS)
    assert not plain.stand_in
    assert plain.failures == {
        "poster#1": fx.Failure(
            id="poster#1",
            path="poster",
            message="the poster has no face",
            code=None,
            facts={},
            skipped=False,
        )
    }
    assert plain.stopped is None
    plain._store = tmp_path / "cache"
    assert plain._store_root() == tmp_path / "cache"


# ------------------------------------------------------------------------------------------------
# Delivering


def _stored(project: Path, digest: str, data: bytes) -> None:
    path = project / "store" / "cache" / "files" / digest[:2] / digest
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(0o444)


@pytest.fixture
def delivered(fake: Fake, project: Path) -> RunResult:
    fake.runs()
    for digest, data in [(D1, b"one"), (D2, b"two"), (D3, b"333")]:
        _stored(project, digest, data)
    return fx.run("gallery", cwd=project, run_dir="runs/d")


def test_deliver_copies_files_and_names_each_element(delivered: RunResult, tmp_path: Path) -> None:
    out = tmp_path / "out"
    missing = delivered.deliver(
        {
            "plate": "art/ground.png",
            "images": "icons/{key}.png",
            "frames": "frames/{key}.png",
            "skipped": "nothing.png",
            "count": "count.txt",
            "undeclared": "x.png",
        },
        root=out,
    )
    assert missing == ["skipped", "count", "undeclared"]
    assert (out / "art" / "ground.png").read_bytes() == b"one"
    assert (out / "icons" / "ada.png").read_bytes() == b"two"
    # A key leaves no folder: "../bo/x y" is "_/bo/x_y".
    assert (out / "icons" / "_" / "bo" / "x_y.png").read_bytes() == b"333"
    assert (out / "frames" / "0.png").read_bytes() == b"one"
    assert (out / "frames" / "k.png").read_bytes() == b"two"
    assert sorted(p.name for p in out.rglob("*") if p.is_file()) == [
        "0.png",
        "ada.png",
        "ground.png",
        "k.png",
        "x_y.png",
    ]
    assert os.access(out / "art" / "ground.png", os.W_OK)


def test_deliver_leaves_the_same_bytes_alone_and_replaces_others(
    delivered: RunResult, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    target = tmp_path / "ground.png"
    target.write_bytes(b"one")
    before = target.stat().st_ino, target.stat().st_mtime_ns
    assert delivered.deliver({"plate": "ground.png"}) == []
    assert (target.stat().st_ino, target.stat().st_mtime_ns) == before
    # A different file there is replaced by a new one, never written through: a hard link to it
    # keeps its bytes.
    target.unlink()
    target.write_bytes(b"old")
    os.link(target, tmp_path / "linked.png")
    assert delivered.deliver({"plate": "ground.png"}) == []
    assert target.read_bytes() == b"one"
    assert (tmp_path / "linked.png").read_bytes() == b"old"


def test_deliver_refuses_a_key_for_one_file_and_one_name_for_several(
    delivered: RunResult, tmp_path: Path
) -> None:
    out = tmp_path / "out"
    with pytest.raises(ValueError, match="plate is one file, so no {key}"):
        delivered.deliver({"images": "i/{key}.png", "plate": "p/{key}.png"}, root=out)
    with pytest.raises(ValueError, match="images holds 2 files: name each one with {key}"):
        delivered.deliver({"images": "i/all.png"}, root=out)
    assert not out.exists(), "nothing is copied when a target is wrong"


@pytest.mark.parametrize(
    ("key", "path"),
    [
        ("ada", "ada"),
        ("a/b", "a/b"),
        ("../x", "_/x"),
        ("a//b", "a/_/b"),
        ("./.", "_/_"),
        ("x y!", "x_y"),
        ("__a__", "a"),
        ("@@", "step"),
        ("é-1.v", "é-1.v"),
        ("", "_"),
    ],
)
def test_keys_as_paths(key: str, path: str) -> None:
    assert _api._key_path(key) == path


def test_a_run_file_reads_and_copies(delivered: RunResult, tmp_path: Path) -> None:
    plate = delivered.outputs["plate"]
    assert plate.read_bytes() == b"one"
    assert plate.copy_to(tmp_path / "a" / "b.png") == tmp_path / "a" / "b.png"
    assert (tmp_path / "a" / "b.png").read_bytes() == b"one"
    assert repr(plate) == "<RunFile plate/image image/png>"


# ------------------------------------------------------------------------------------------------
# python -m grida.fx


def _module(args: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "grida.fx", *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        timeout=60,
        env=os.environ.copy(),
    )


def test_the_module_becomes_the_binary(fake: Fake, project: Path) -> None:
    fake.reply(plan={"stdout": "plan text\n", "stderr": "a warning\n", "status": 1})
    done = _module(["plan", "gallery", "--check", "--name", "Ada Lovelace"], project)
    assert (done.returncode, done.stdout, done.stderr) == (1, "plan text\n", "a warning\n")
    assert fake.calls[-1]["argv"] == ["plan", "gallery", "--check", "--name", "Ada Lovelace"]
    assert fake.calls[-1]["cwd"] == os.path.realpath(project)


def test_the_module_names_its_own_python_for_node_bodies(
    fake: Fake, project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake.reply(plan={"stdout": "plan text\n"})
    monkeypatch.delenv("GRIDA_FX_PYTHON", raising=False)
    _module(["plan", "gallery"], project)
    assert fake.calls[-1]["python"] is None
    assert fake.calls[-1]["sdk_python"] == sys.executable
    monkeypatch.setenv("GRIDA_FX_PYTHON", "/opt/other/python")
    _module(["plan", "gallery"], project)
    assert fake.calls[-1]["python"] == "/opt/other/python"
    assert fake.calls[-1]["sdk_python"] == sys.executable


def test_the_module_without_a_binary(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("GRIDA_FX_BIN", str(tmp_path / "missing"))
    done = _module(["--help"], tmp_path)
    assert done.returncode == 2
    assert done.stderr == f"grida-fx: GRIDA_FX_BIN is {tmp_path / 'missing'}, which is not a file\n"


def test_the_module_with_a_binary_it_cannot_start(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    unrunnable = tmp_path / "grida-fx"
    unrunnable.write_text("not a program")
    unrunnable.chmod(0o644)
    monkeypatch.setenv("GRIDA_FX_BIN", str(unrunnable))
    done = _module(["plan", "x"], tmp_path)
    assert done.returncode == 2
    assert done.stderr.startswith(f"grida-fx: cannot start {unrunnable}: ")


def _installed(site: Path) -> Path:
    """A copy of this package in ``site``, as a wheel installs it: its packaged binary's place."""
    source = Path(fx.__file__).resolve().parents[1]
    shutil.copytree(source, site / "grida", ignore=shutil.ignore_patterns("__pycache__", "_bin"))
    packaged = site / _api._packaged_binary().relative_to(source.parent)
    packaged.parent.mkdir()
    return packaged


def _shell(path: Path, body: str) -> None:
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def test_the_module_runs_the_packaged_binary_before_the_one_on_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    packaged = _installed(tmp_path / "site")
    _shell(packaged, 'echo "packaged $# [$1] [$2]"; echo "to stderr" >&2; exit 3')
    on_path = tmp_path / "path"
    on_path.mkdir()
    _shell(on_path / "grida-fx", 'echo "on path"; exit 0')
    monkeypatch.delenv("GRIDA_FX_BIN", raising=False)
    monkeypatch.setenv("PYTHONPATH", str(tmp_path / "site"))
    monkeypatch.setenv("PATH", f"{on_path}{os.pathsep}{os.environ['PATH']}")
    done = _module(["--version", "two words"], tmp_path)
    assert (done.returncode, done.stdout, done.stderr) == (
        3,
        "packaged 2 [--version] [two words]\n",
        "to stderr\n",
    )
    packaged.unlink()
    done = _module(["nodes"], tmp_path)
    assert (done.returncode, done.stdout) == (0, "on path\n")


def test_the_module_with_no_binary_anywhere_names_the_variable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _installed(tmp_path / "site")
    monkeypatch.delenv("GRIDA_FX_BIN", raising=False)
    monkeypatch.setenv("PYTHONPATH", str(tmp_path / "site"))
    monkeypatch.setenv("PATH", str(tmp_path / "nowhere"))
    done = _module(["--version"], tmp_path)
    assert done.returncode == 2
    assert done.stdout == ""
    assert done.stderr == (
        "grida-fx: no grida-fx binary was found: this grida installation carries none, so set"
        " GRIDA_FX_BIN to its path, or put grida-fx on PATH\n"
    )
