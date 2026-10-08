"""Saved-run loading and observation preserve engine selection and wire evidence."""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import shutil
import sys
import time
from pathlib import Path
from typing import Any

import pytest
from test_api import Fake

import grida.fx as fx
from grida.fx import _api, _record

pytestmark = pytest.mark.skipif(os.name != "posix", reason="the fake binary is a script")

INSPECTION = {
    "run": {"folder": "runs/selected", "state": "unfinished", "future_summary": [1, 2]},
    "future_inspection": {"keep": True},
}


def event(name: str = "future_event", invocation: str = "invocation-1") -> dict[str, Any]:
    return {
        "kind": "fx-run-events-v1",
        "event": name,
        "invocation_id": invocation,
        "plan": "a" * 64,
        "offset_ms": 0,
        "future_payload": {"keep": [1, 2]},
    }


def batch(
    cursor: str = "opaque cursor", events: list[Any] | None = None, more: bool = False
) -> dict:
    return {
        "kind": "fx-run-event-batch-v1",
        "cursor": cursor,
        "events": [event()] if events is None else events,
        "has_more": more,
        "future_batch": {"keep": True},
    }


@pytest.fixture
def fake(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Fake:
    folder = tmp_path / "bin"
    folder.mkdir()
    engine = Fake(folder, monkeypatch)
    engine.reply(inspect={"stdout": json.dumps(INSPECTION)})
    return engine


@pytest.mark.parametrize("target", [Path("runs/source"), "case", "case/take-A", "case/latest"])
def test_load_run_forwards_selection_and_pins_inspected_folder(
    fake: Fake, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, target: str | Path
) -> None:
    record = fx.load_run(target, cwd=tmp_path)
    assert fake.invocations == [["inspect", str(target), "--json"]]
    assert record.run_dir == tmp_path.resolve() / "runs/selected"
    assert record.inspection == INSPECTION
    assert set(vars(record)) == {"run_dir", "inspection"}
    record.inspection["run"]["folder"] = "runs/changed"
    other = tmp_path / "other"
    other.mkdir()
    monkeypatch.chdir(other)
    fake.reply(inspect={"stdout": json.dumps({"run": {"folder": "runs/newest"}})})
    expected = batch()
    fake.reply(observe={"stdout": json.dumps(expected)})
    assert record.events() == expected
    assert fake.invocations[-1] == ["observe", str(record.run_dir), "--limit=256"]
    assert len(fake.invocations) == 2


def test_verify_failure_is_retained_inspection_evidence(fake: Fake, tmp_path: Path) -> None:
    document = {**INSPECTION, "verification": {"verified": False, "problems": ["file missing"]}}
    fake.reply(inspect={"stdout": json.dumps(document), "status": 1})
    record = fx.load_run("case/take-A", verify=True, cwd=tmp_path)
    assert record.inspection == document
    assert fake.invocations == [["inspect", "case/take-A", "--json", "--verify"]]
    with pytest.raises(fx.FxError):
        fx.load_run("case", cwd=tmp_path)


def test_direct_run_record_construction_pins_a_relative_folder(
    fake: Fake, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    record = fx.RunRecord(Path("runs/selected"), INSPECTION)
    monkeypatch.chdir(tmp_path.parent)
    assert record.run_dir == tmp_path.resolve() / "runs/selected"
    fake.reply(observe={"stdout": json.dumps(batch())})
    record.events()
    assert fake.invocations == [["observe", str(record.run_dir), "--limit=256"]]


@pytest.mark.parametrize(
    "verification",
    [
        None,
        {"verified": True, "problems": ["missing"]},
        {"verified": False, "problems": []},
        {"verified": False, "problems": [1]},
    ],
)
def test_exit_one_requires_failed_verification_evidence(
    fake: Fake, tmp_path: Path, verification: Any
) -> None:
    fake.reply(
        inspect={"stdout": json.dumps({**INSPECTION, "verification": verification}), "status": 1}
    )
    with pytest.raises(fx.FxError):
        fx.load_run("case", verify=True, cwd=tmp_path)


@pytest.mark.parametrize("target", ["", "a\0b", "--open", 1, b"runs/a"])
def test_bad_load_targets_start_nothing(fake: Fake, target: Any) -> None:
    with pytest.raises((ValueError, TypeError)):
        fx.load_run(target)
    assert fake.calls == []


def test_bad_verify_starts_nothing(fake: Fake) -> None:
    with pytest.raises(TypeError):
        fx.load_run("case", verify=1)
    assert fake.calls == []


@pytest.mark.parametrize(
    "document", [[], {}, {"run": []}, {"run": {"folder": ""}}, {"run": {"folder": "a\0b"}}]
)
def test_loader_rejects_missing_selected_folder(fake: Fake, document: Any) -> None:
    fake.reply(inspect={"stdout": json.dumps(document)})
    with pytest.raises(fx.FxError):
        fx.load_run("case")


def test_snapshot_and_events_preserve_unknown_wire_evidence(fake: Fake, tmp_path: Path) -> None:
    record = fx.load_run("case", cwd=tmp_path)
    snapshot = {
        "kind": "fx-run-snapshot-v1",
        "plan": {"kind": "fx-graph-v1", "future_graph": {"keep": True}},
        "events": [event()],
        "cursor": "opaque snapshot cursor",
        "future_snapshot": [1, 2],
    }
    fake.reply(observe={"stdout": json.dumps(snapshot)})
    assert record.snapshot() == snapshot
    assert fake.invocations[-1] == ["observe", str(record.run_dir), "--snapshot"]
    expected = batch("next opaque cursor")
    fake.reply(observe={"stdout": json.dumps(expected)})
    assert record.events(after=snapshot["cursor"], limit=1) == expected
    assert fake.invocations[-1] == [
        "observe",
        str(record.run_dir),
        "--after=opaque snapshot cursor",
        "--limit=1",
    ]


def test_async_loader_and_reads_inside_an_event_loop(fake: Fake, tmp_path: Path) -> None:
    async def use() -> None:
        record = await fx.load_run_async("case", cwd=tmp_path)
        expected = batch()
        fake.reply(observe={"stdout": json.dumps(expected)})
        assert await record.events_async() == expected
        snapshot = {
            "kind": "fx-run-snapshot-v1",
            "plan": {"kind": "fx-graph-v1"},
            "events": [],
            "cursor": "snapshot",
        }
        fake.reply(observe={"stdout": json.dumps(snapshot)})
        assert await record.snapshot_async() == snapshot
        for call in (lambda: fx.load_run("case"), record.events, record.snapshot):
            with pytest.raises(RuntimeError, match="_async"):
                call()

    asyncio.run(use())


@pytest.mark.parametrize(
    "options",
    [
        {"after": ""},
        {"after": "a\0b"},
        {"after": 1},
        {"limit": 0},
        {"limit": 1025},
        {"limit": True},
        {"limit": 1.0},
    ],
)
def test_invalid_observation_options_spawn_no_observer(fake: Fake, options: dict) -> None:
    record = fx.load_run("case")
    with pytest.raises((TypeError, ValueError)):
        record.events(**options)
    with pytest.raises((TypeError, ValueError)):
        asyncio.run(record.events_async(**options))
    assert len(fake.invocations) == 1


def test_structured_reader_errors_retain_code_and_document(fake: Fake) -> None:
    record = fx.load_run("case")
    document = {
        "kind": "fx-run-observation-error-v1",
        "code": "future_error",
        "message": "the reader refused",
        "future_error_detail": [1, 2],
    }
    fake.reply(observe={"stdout": json.dumps(document), "status": 2})
    with pytest.raises(fx.RunObservationError) as raised:
        record.events(after="old cursor")
    assert raised.value.code == "future_error"
    assert raised.value.document == document
    assert raised.value.status == 2


@pytest.mark.parametrize(
    ("document", "status"),
    [
        ([], 0),
        ({}, 0),
        ({**batch(), "kind": "fx-run-event-batch-v2"}, 0),
        ({**batch(), "cursor": ""}, 0),
        ({**batch(), "cursor": "a\0b"}, 0),
        ({**batch(), "has_more": 1}, 0),
        ({**batch(), "events": {}}, 0),
        (batch(events=[{**event(), "kind": "unknown"}]), 0),
        (batch(events=[{**event(), "plan": "bad"}]), 0),
        (batch(events=[{**event(), "offset_ms": True}]), 0),
        (batch(events=[{**event(), "offset_ms": -1}]), 0),
        (batch(events=[{**event(), "event": ""}]), 0),
        (batch(events=[{**event(), "invocation_id": ""}]), 0),
        ({"kind": "fx-run-observation-error-v1", "code": "x", "message": "m"}, 0),
        ({"kind": "fx-run-observation-error-v1", "code": "", "message": "m"}, 2),
        (batch(), 1),
    ],
)
def test_malformed_or_wrong_status_wire_documents_are_fx_errors(
    fake: Fake, document: Any, status: int
) -> None:
    record = fx.load_run("case")
    fake.reply(observe={"stdout": json.dumps(document), "status": status})
    with pytest.raises(fx.FxError) as raised:
        record.events()
    assert not isinstance(raised.value, fx.RunObservationError)


@pytest.mark.parametrize(
    "document",
    [
        batch(events=[], more=True),
        batch(events=[event(), event()]),
        batch("existing"),
        batch("changed", []),
    ],
)
def test_contradictory_or_unbounded_batches_are_rejected(fake: Fake, document: dict) -> None:
    record = fx.load_run("case")
    fake.reply(observe={"stdout": json.dumps(document)})
    with pytest.raises(fx.FxError):
        record.events(after="existing", limit=1)


def test_snapshot_requires_saved_graph_kind(fake: Fake) -> None:
    record = fx.load_run("case")
    fake.reply(
        observe={
            "stdout": json.dumps(
                {
                    "kind": "fx-run-snapshot-v1",
                    "cursor": "x",
                    "events": [],
                    "plan": {"kind": "other"},
                }
            )
        }
    )
    with pytest.raises(fx.FxError):
        record.snapshot()


def test_non_json_and_nonfinite_json_are_rejected(fake: Fake) -> None:
    record = fx.load_run("case")
    for text in ("not json", json.dumps({**batch(), "extra": float("nan")})):
        fake.reply(observe={"stdout": text})
        with pytest.raises(fx.FxError):
            record.events()


def test_follow_drains_pages_with_backpressure_and_continues_across_resume(
    fake: Fake, monkeypatch: pytest.MonkeyPatch
) -> None:
    record = fx.load_run("case")
    replies = iter(
        [
            batch("first", more=True),
            batch("terminal", [event("run_finished")]),
            batch("terminal", []),
            batch("resumed", [event("run_started", "invocation-2")]),
        ]
    )
    calls: list[list[str]] = []
    sleeps: list[float] = []

    async def call(args: list[str], cwd: Path) -> _api._Exit:
        calls.append(args)
        return _api._Exit(0, json.dumps(next(replies)), "")

    async def sleep(interval: float) -> None:
        sleeps.append(interval)

    monkeypatch.setattr(_record, "_call", call)
    monkeypatch.setattr(_record.asyncio, "sleep", sleep)

    async def use() -> None:
        stream = record.follow(after="attach", limit=1, poll_interval=0.25)
        first = await anext(stream)
        assert len(calls) == 1 and not sleeps
        first["cursor"], first["has_more"] = "caller mutation", False
        second = await anext(stream)
        assert second["events"][0]["event"] == "run_finished"
        assert len(calls) == 2 and not sleeps
        second["cursor"], second["has_more"] = "caller mutation", True
        third = await anext(stream)
        assert third["events"][0]["invocation_id"] == "invocation-2"
        assert sleeps == [0.25, 0.25]
        assert [args[-2] for args in calls] == [
            "--after=attach",
            "--after=first",
            "--after=terminal",
            "--after=terminal",
        ]
        await stream.aclose()
        assert len(calls) == 4

    asyncio.run(use())


@pytest.mark.parametrize("interval", [0, -1, float("nan"), float("inf"), True, "1"])
def test_bad_follow_interval_spawns_nothing(fake: Fake, interval: Any) -> None:
    record = fx.load_run("case")

    async def use() -> None:
        with pytest.raises((TypeError, ValueError)):
            await anext(record.follow(poll_interval=interval))

    asyncio.run(use())
    assert len(fake.invocations) == 1


def test_follow_propagates_reader_error_without_reattaching(fake: Fake) -> None:
    record = fx.load_run("case")
    fake.reply(
        observe={
            "stdout": json.dumps(
                {
                    "kind": "fx-run-observation-error-v1",
                    "code": "invalid_cursor",
                    "message": "prefix changed",
                }
            ),
            "status": 2,
        }
    )

    async def use() -> None:
        with pytest.raises(fx.RunObservationError, match="prefix changed"):
            await anext(record.follow(after="old cursor"))

    asyncio.run(use())
    assert len(fake.invocations) == 2


def test_cancelling_follow_idle_sleep_starts_no_more_observers(
    fake: Fake, monkeypatch: pytest.MonkeyPatch
) -> None:
    record = fx.load_run("case")

    async def use() -> None:
        observed = asyncio.Event()
        calls = []

        async def call(args: list[str], cwd: Path) -> _api._Exit:
            calls.append(args)
            observed.set()
            return _api._Exit(0, json.dumps(batch("empty", [])), "")

        monkeypatch.setattr(_record, "_call", call)
        stream = record.follow(poll_interval=30)
        task = asyncio.create_task(anext(stream))
        await observed.wait()
        await asyncio.sleep(0)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        await stream.aclose()
        assert len(calls) == 1

    asyncio.run(use())


def test_cancelling_follow_stops_only_its_spawned_observer(fake: Fake, tmp_path: Path) -> None:
    record = fx.load_run("case")
    ready = tmp_path / "observer-ready"
    fake.reply(observe={"sleep": 30, "ready_file": str(ready)})

    async def use() -> None:
        unrelated = await asyncio.create_subprocess_exec(
            sys.executable, "-c", "import time; time.sleep(30)"
        )
        stream = record.follow()
        task = asyncio.create_task(anext(stream))
        try:
            deadline = time.monotonic() + 10
            while not ready.exists():
                assert time.monotonic() < deadline, "the observer did not start"
                await asyncio.sleep(0.01)
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
            await stream.aclose()
            assert unrelated.returncode is None
            os.kill(unrelated.pid, 0)
        finally:
            if not task.done():
                task.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await task
            unrelated.kill()
            await unrelated.wait()

    asyncio.run(use())
    assert {"interrupted": True} in fake.calls
    assert [args[0] for args in fake.invocations] == ["inspect", "observe"]


def test_real_engine_named_collision_resume_and_pinned_loading(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    engine = _api._packaged_binary()
    if not engine.is_file():
        pytest.skip("needs the engine beside the SDK")
    monkeypatch.setenv("GRIDA_FX_BIN", str(engine))
    for key in (
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
    ):
        monkeypatch.delenv(key, raising=False)
    (tmp_path / "fx.yaml").write_text("fx: project/v1\n")
    workflow = tmp_path / "workflow.yaml"
    source = (
        "fx: workflow/v1\nid: case\ntitle: Saved run fixture\nsteps:\n"
        "  pick:\n    uses: fx/select@1\n    with: { first_of: [1] }\n"
    )
    workflow.write_text(source)
    first = fx.run("workflow.yaml", name="take-A", cwd=tmp_path)
    assert first.ok
    with pytest.raises(fx.FxError, match="already exists"):
        fx.run("workflow.yaml", name="take-A", cwd=tmp_path)
    resumed = fx.run("workflow.yaml", resume="take-A", cwd=tmp_path)
    assert resumed.ok and resumed.run_dir == first.run_dir
    assert len([e for e in resumed.events if e["event"] == "run_started"]) == 2
    record = fx.load_run("case/take-A", verify=True, cwd=tmp_path)
    assert record.run_dir == first.run_dir.resolve()
    assert record.inspection["verification"]["verified"]
    snapshot = record.snapshot()
    assert snapshot["kind"] == "fx-run-snapshot-v1"
    assert record.events(after=snapshot["cursor"])["events"] == []
    second = fx.run("workflow.yaml", name="take-B", cwd=tmp_path)
    assert fx.load_run("case", cwd=tmp_path).run_dir == second.run_dir.resolve()
    assert record.run_dir == first.run_dir.resolve()
    assert record.snapshot()["events"] == snapshot["events"]
    workflow.write_text(source.replace("first_of: [1]", "first_of: [2]"))
    with pytest.raises(fx.FxError, match="another workflow|other inputs"):
        fx.run("workflow.yaml", resume="take-A", cwd=tmp_path)


def test_real_engine_observation_cancellation_leaves_workflow_running_and_follows_resume(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    engine = _api._packaged_binary()
    if not engine.is_file():
        pytest.skip("needs the engine beside the SDK")
    monkeypatch.setenv("GRIDA_FX_BIN", str(engine))
    for key in (
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
    ):
        monkeypatch.delenv(key, raising=False)
    project = tmp_path / "project"
    shutil.copytree(Path(__file__).resolve().parents[2] / "fixtures/control", project)

    async def use() -> None:
        owner = asyncio.create_task(fx.run_async("workflow.yaml", name="watch", cwd=project))
        reader = None
        reading = None
        following = None
        resumed = None
        try:
            deadline = time.monotonic() + 10
            while not (project / ".control-entered").is_file():
                assert time.monotonic() < deadline, "the workflow did not reach its local gate"
                if owner.done():
                    pytest.fail(f"the workflow ended before observation: {owner.result()}")
                await asyncio.sleep(0.01)
            record = await fx.load_run_async("control-harness/watch", cwd=project)
            assert record.inspection["run"]["state"] == "unfinished"
            snapshot = await record.snapshot_async()
            assert any(e["event"] == "node_finished" for e in snapshot["events"])
            reader = record.follow(after=snapshot["cursor"], poll_interval=0.02)
            reading = asyncio.create_task(anext(reader))
            await asyncio.sleep(0.05)
            reading.cancel()
            with pytest.raises(asyncio.CancelledError):
                await reading
            await reader.aclose()
            assert not owner.done(), "cancelling a reader must not cancel the workflow"
            (project / ".control-release").touch()
            result = await asyncio.wait_for(owner, 10)
            assert result.ok
            following = record.follow(after=snapshot["cursor"], limit=1, poll_interval=0.01)
            while True:
                document = await asyncio.wait_for(anext(following), 10)
                if document["events"][0]["event"] == "run_finished":
                    terminal_invocation = document["events"][0]["invocation_id"]
                    break
            resumed = asyncio.create_task(
                fx.run_async("workflow.yaml", resume="watch", cwd=project)
            )
            document = await asyncio.wait_for(anext(following), 10)
            assert document["events"][0]["event"] == "run_started"
            assert document["events"][0]["invocation_id"] != terminal_invocation
            assert (await asyncio.wait_for(resumed, 10)).ok
            assert record.run_dir == result.run_dir.resolve()
        finally:
            if reading is not None and not reading.done():
                reading.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await reading
            for stream in (reader, following):
                if stream is not None:
                    await stream.aclose()
            (project / ".control-release").touch()
            for task in (owner, resumed):
                if task is not None and not task.done():
                    with contextlib.suppress(Exception):
                        await asyncio.wait_for(task, 10)

    asyncio.run(use())
