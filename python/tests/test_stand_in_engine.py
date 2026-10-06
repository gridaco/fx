"""Stand-in runs from Python against the real engine (``spec/protocol.md`` sections 5.7 and 8,
``spec/store.md`` section 8 "Stand-in runs").

``test_api.py`` pins what the SDK sends and reads against a fake binary; these run the
``grida-fx`` that ``tools/build_engine.py`` put beside the SDK, so the engine's own checks,
stores, records and exit statuses meet the SDK's answerer. They are skipped when no engine was
built. The project is a scratch one: two ``fx/image.generate@1`` steps and an ``fx/image.edit@1``
of the second, all on ``img-a@acme``, answered by closures in this process with PNGs drawn here.
Nothing reaches a provider: the network is off and no provider key is in the environment.
"""

from __future__ import annotations

import io
import json
import os
from pathlib import Path

import pytest
from PIL import Image

import grida.fx as fx
from grida.fx import DECLINE, Answer, CallRefused, Failure, InputFile, StandInCall, _api
from grida.fx._api import RunResult
from grida.fx._stand_in import Decline, StandIn

ENGINE = _api._packaged_binary()

pytestmark = pytest.mark.skipif(
    os.name != "posix" or not ENGINE.is_file(),
    reason="needs the engine beside the SDK (python3 tools/build_engine.py) on a POSIX system",
)

#: Every provider key FX reads (``spec/providers.md`` section 3), kept out of the runs.
KEYS = (
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "FAL_KEY",
    "TRIPO_API_KEY",
    "ELEVENLABS_API_KEY",
)

PROJECT = """\
fx: project/v1
routes:
  image.generate: img-a@acme
  image.edit: img-a@acme
"""

ROUTES = """\
fx: routes/v1
routes:
  - { capability: image.generate, route: img-a@acme, price: { low_usd: 0.01, high_usd: 0.04 } }
  - capability: image.edit
    route: img-a@acme
    price: { low_usd: 0.01, high_usd: 0.04 }
    features: [image_input]
"""

WORKFLOW = """\
fx: workflow/v1
id: harbour
title: Two pictures and an edit of one
steps:
  lantern:
    uses: fx/image.generate@1
    with: { prompt: a lantern on a quay, size: "64x64", background: opaque }
  gull:
    uses: fx/image.generate@1
    with: { prompt: a gull on a mast, size: "32x32", background: opaque }
  tint:
    uses: fx/image.edit@1
    with:
      prompt: make it blue
      image: ${{ steps.gull.outputs.image }}
      size: "32x32"
      background: opaque
outputs:
  lantern: ${{ steps.lantern.outputs.image }}
  gull: ${{ steps.tint.outputs.image }}
"""

#: One colour per prompt, so each answer is a file of its own.
COLOURS = {
    "a lantern on a quay": (200, 160, 40),
    "a gull on a mast": (230, 230, 230),
    "make it blue": (40, 90, 160),
}


@pytest.fixture(autouse=True)
def engine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(_api.BINARY_VARIABLE, str(ENGINE))
    monkeypatch.setenv("GRIDA_FX_NETWORK", "off")
    for key in KEYS:
        monkeypatch.delenv(key, raising=False)


@pytest.fixture
def project(tmp_path: Path) -> Path:
    root = tmp_path / "project"
    (root / "workflows").mkdir(parents=True)
    (root / "fx.yaml").write_text(PROJECT, "utf-8")
    (root / "routes.yaml").write_text(ROUTES, "utf-8")
    (root / "workflows" / "harbour.yaml").write_text(WORKFLOW, "utf-8")
    return root


def png(width: int, height: int, colour: tuple[int, int, int]) -> bytes:
    """An opaque PNG of one colour (RGB: no alpha channel)."""
    out = io.BytesIO()
    Image.new("RGB", (width, height), colour).save(out, "PNG")
    return out.getvalue()


def picture_for(call: StandInCall) -> bytes:
    """The picture a call asks for: its size, in its prompt's colour."""
    width, height = (int(side) for side in call.request["size"].split("x"))
    return png(width, height, COLOURS[call.request["prompt"]])


def picture(call: StandInCall) -> Answer:
    return Answer(files={"image": picture_for(call)})


def run(project: Path, folder: str, stand_in: StandIn | None = None) -> RunResult:
    return fx.run("harbour", cwd=project, routes=["routes.yaml"], run_dir=folder, stand_in=stand_in)


def stores(project: Path) -> tuple[Path, Path]:
    """The project's store and its stand-in store."""
    store = (project / ".fx" / "cache").resolve()
    return store, store / "stand-in"


def records(folder: Path) -> list[Path]:
    return sorted(folder.rglob("*.json")) if folder.is_dir() else []


def test_a_stand_in_answers_each_paid_call_once_and_its_answers_are_kept(project: Path) -> None:
    store, stand_ins = stores(project)
    asked: list[StandInCall] = []
    edited: list[bytes] = []

    def answer(call: StandInCall) -> Answer:
        asked.append(call)
        if call.capability == "image.edit":
            edited.append(call.request["image"].read_bytes())
        return picture(call)

    result = run(project, "runs/one", answer)
    assert result.ok, result.failures
    assert result.stand_in and result.cost == 0 and result.stopped is None
    assert result.failures == {}

    # Each paid call is asked once, with the canonical request and the call key.
    assert sorted((call.capability, call.instance.id) for call in asked) == [
        ("image.edit", "tint#1"),
        ("image.generate", "gull#1"),
        ("image.generate", "lantern#1"),
    ]
    by_step = {call.instance.step: call for call in asked}
    lantern = by_step["lantern"]
    assert lantern.route.id == "img-a@acme"
    assert lantern.request == {
        "prompt": "a lantern on a quay",
        "size": "64x64",
        "background": "opaque",
    }
    assert (lantern.takes, lantern.take, lantern.instance.path, lantern.files) == (
        (1,),
        1,
        "lantern",
        {},
    )
    calls = [event for event in result.events if event["event"] == "call"]
    assert sorted(event["call"] for event in calls) == sorted(call.key for call in asked)
    assert all(
        event["stand_in"] is True and event["cached"] is False and event["cost_usd"] == 0
        for event in calls
    )
    # Answered and counted at nothing, not a cache hit.
    assert {path: step.facts for path, step in result.steps.items()} == {
        "lantern": {"cost_usd": 0},
        "gull": {"cost_usd": 0},
        "tint": {"cost_usd": 0},
    }

    # The edit is handed the gull the stand-in drew, as an InputFile in the stand-in store.
    tint = by_step["tint"]
    image = tint.request["image"]
    assert isinstance(image, InputFile)
    assert list(tint.files) == [image.digest] and tint.files[image.digest] is image
    assert (image.kind, image.name) == ("image/png", "gull/image")
    assert image.path.resolve() == stand_ins / "files" / image.digest[:2] / image.digest
    assert edited == [picture_for(by_step["gull"])]

    # Everything went to the stand-in store; the project's own store holds nothing.
    assert sorted(path.name for path in store.iterdir()) == ["stand-in"]
    assert len(records(stand_ins / "calls")) == 3
    assert not (stand_ins / "jobs").exists()
    outputs = result.outputs
    assert outputs["lantern"].path.resolve().parent.parent == stand_ins / "files"
    assert outputs["lantern"].read_bytes() == picture_for(lantern)
    assert outputs["gull"].read_bytes() == picture_for(tint)
    plan = json.loads((project / "runs" / "one" / "plan.json").read_text("utf-8"))
    assert plan["stand_in"] is True
    started = [event for event in result.events if event["event"] == "run_started"]
    assert [event.get("stand_in") for event in started] == [True]

    # A second run, into a new folder and with another stand-in, a coroutine function this
    # time, is answered by the stand-in store: nothing is asked.
    again: list[StandInCall] = []

    async def answer_again(call: StandInCall) -> Answer:
        again.append(call)
        return picture(call)

    second = run(project, "runs/two", answer_again)
    assert second.ok and second.stand_in and second.cost == 0
    assert again == []
    assert {path: step.cache for path, step in second.steps.items()} == {
        "lantern": "hit",
        "gull": "hit",
        "tint": "hit",
    }
    assert [event for event in second.events if event["event"] == "call"] == []
    assert {name: file.digest for name, file in second.outputs.items()} == {
        name: file.digest for name, file in outputs.items()
    }
    assert len(records(stand_ins / "calls")) == 3
    assert sorted(path.name for path in store.iterdir()) == ["stand-in"]


def test_a_refused_call_fails_its_step_and_a_resume_with_another_stand_in_asks_only_it(
    project: Path,
) -> None:
    _, stand_ins = stores(project)
    first: list[str] = []

    def refuses_gulls(call: StandInCall) -> Answer:
        first.append(call.instance.id)
        if call.request["prompt"] == "a gull on a mast":
            raise CallRefused("no gulls today")
        return picture(call)

    result = run(project, "runs/one", refuses_gulls)
    assert not result.ok and not result.incomplete and result.stopped is None
    assert sorted(first) == ["gull#1", "lantern#1"]
    assert sorted(result.failed) == ["gull#1", "tint#1"]
    assert list(result.failures) == result.failed
    assert result.failures["gull#1"] == Failure(
        id="gull#1",
        path="gull",
        message="image.generate on img-a@acme was refused: no gulls today",
        code="capability_refused",
        facts={},
        skipped=False,
    )
    assert result.failures["tint#1"] == Failure(
        id="tint#1",
        path="tint",
        message="something it reads failed",
        code=None,
        facts={},
        skipped=True,
    )
    # A refusal is recorded nowhere: only the lantern's answer is.
    assert len(records(stand_ins / "calls")) == 1

    # The folder holds a stand-in run: resuming it without one is refused, and asks nothing.
    with pytest.raises(fx.FxError) as refused:
        run(project, "runs/one")
    assert str(refused.value) == (
        "refused: runs/one holds a stand-in run; resume it with --stand-in, or choose a new folder"
    )

    # Resumed with another stand-in, the folder asks only what failed: the lantern's answer is
    # replayed from the stand-in store.
    second: list[str] = []

    def answers_everything(call: StandInCall) -> Answer:
        second.append(call.instance.id)
        return picture(call)

    resumed = run(project, "runs/one", answers_everything)
    assert resumed.ok and resumed.failures == {} and resumed.failed == []
    assert sorted(second) == ["gull#1", "tint#1"]
    started = [event for event in resumed.events if event["event"] == "run_started"]
    assert [(event["resumed"], event.get("stand_in")) for event in started] == [
        (False, True),
        (True, True),
    ]
    assert resumed.outputs["gull"].read_bytes() == png(32, 32, COLOURS["make it blue"])
    assert len(records(stand_ins / "calls")) == 3


def test_a_declined_call_is_not_live(project: Path) -> None:
    asked: list[str] = []

    def declines_lanterns(call: StandInCall) -> Answer | Decline:
        asked.append(call.instance.id)
        if call.request["prompt"] == "a lantern on a quay":
            return DECLINE
        return picture(call)

    result = run(project, "runs/one", declines_lanterns)
    assert not result.ok
    assert sorted(asked) == ["gull#1", "lantern#1", "tint#1"]
    assert result.failed == ["lantern#1"]
    assert result.failures["lantern#1"] == Failure(
        id="lantern#1",
        path="lantern",
        message="image.generate on img-a@acme is a paid call the stand-in declined",
        code="not_live",
        facts={},
        skipped=False,
    )

    # A decline is recorded nowhere: resumed, the folder asks the lantern again, and only it.
    again = run(project, "runs/one", declines_lanterns)
    assert not again.ok and again.failed == ["lantern#1"]
    assert sorted(asked) == ["gull#1", "lantern#1", "lantern#1", "tint#1"]


def test_a_stand_in_that_raises_stops_the_run_and_run_raises_it(project: Path) -> None:
    def breaks_on_gulls(call: StandInCall) -> Answer:
        if call.request["prompt"] == "a gull on a mast":
            raise RuntimeError("the harness broke")
        return picture(call)

    with pytest.raises(RuntimeError) as raised:
        run(project, "runs/one", breaks_on_gulls)
    folder = project / "runs" / "one"
    assert str(raised.value) == "the harness broke"
    assert raised.value.__notes__ == [f"the stand-in run stopped in {folder}"]

    # The engine stopped the run, and its record says why.
    stopped = RunResult(folder, _api._read_events(folder))
    assert not stopped.ok and stopped.stand_in
    assert stopped.stopped == "the stand-in failed: RuntimeError: the harness broke"
    assert stopped.failures["gull#1"].code == "internal"
