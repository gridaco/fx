"""``grida.fx.plan/run`` and ``python -m grida.fx``, against a stand-in ``grida-fx``.

The stand-in is a Python script set as ``GRIDA_FX_BIN``: it records each invocation (its
arguments, working directory and the inputs files it was given) and answers with canned output
per verb; for ``run`` it writes a canned ``events.jsonl`` into the run folder and prints the
summary. So these tests pin what the SDK sends and how it reads what comes back, without an
engine.
"""

from __future__ import annotations

import asyncio
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import pytest

import grida.fx as fx
from grida.fx import _api
from grida.fx._api import RunFile, RunResult, StepResult

pytestmark = pytest.mark.skipif(os.name != "posix", reason="the stand-in binary is a script")

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
    out.write(json.dumps({"argv": args, "cwd": os.getcwd(), "inputs": inputs}) + "\n")
reply = config.get(args[0] if args else "", {})
folder = None
if args and args[0] == "run":
    folder = next((a.split("=", 1)[1] for a in args if a.startswith("--run=")), None)
    folder = folder or reply.get("folder")
    if "events" in reply:
        Path(folder).mkdir(parents=True, exist_ok=True)
        lines = "".join(json.dumps(event) + "\n" for event in reply["events"])
        (Path(folder) / "events.jsonl").write_text(lines + reply.get("torn", ""))
if "sleep" in reply:
    def interrupted(signum, frame):
        with log.open("a") as out:
            out.write(json.dumps({"interrupted": True}) + "\n")
        sys.exit(130)
    signal.signal(signal.SIGINT, interrupted)
    time.sleep(reply["sleep"])
sys.stdout.write(reply.get("stdout", "").replace("{folder}", folder or ""))
sys.stderr.write(reply.get("stderr", ""))
sys.exit(reply.get("status", 0))
"""

D1 = "1" * 64
D2 = "2" * 64
D3 = "3" * 64
D4 = "4" * 64

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
    """The stand-in binary, its canned replies and its log."""

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

    @property
    def calls(self) -> list[dict[str, Any]]:
        if not self.log_path.exists():
            return []
        lines = self.log_path.read_text(encoding="utf-8").splitlines()
        return [json.loads(line) for line in lines]


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
    assert packaged.parent == Path(fx.__file__).resolve().parent / "bin"
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
    ]
    assert run_call["inputs"] == {inputs_file: '{"poster": "inputs/poster.png"}\n'}
    assert run_call["cwd"] == os.path.realpath(project)
    assert result.run_dir == project / "runs" / "mine"
    assert result.events == EVENTS
    assert not list(project.glob(".grida-fx-inputs-*"))


def test_run_reads_the_folder_from_the_summary(fake: Fake, project: Path) -> None:
    fake.runs(folder="runs/gallery/2026-10-06-1")
    result = fx.run("gallery", cwd=project)
    assert fake.calls[-1]["argv"] == ["run", "gallery"]
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
        await asyncio.sleep(0.3)  # let the stand-in install its interrupt handler
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task

    asyncio.run(cancel())
    assert {"interrupted": True} in fake.calls


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
