"""Listing and removing a project's runs forward explicit targets and validate the engine's result.

The canned documents are checked against ``spec/schemas``; the last tests drive the real engine
over hand-written run folders.
"""

from __future__ import annotations

import asyncio
import json
import os
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator
from test_api import Fake

import grida.fx as fx
from grida.fx import _api

pytestmark = pytest.mark.skipif(os.name != "posix", reason="the fake binary is a script")

SCHEMAS = Path(__file__).resolve().parents[2] / "spec" / "schemas"
LIST_SCHEMA = Draft202012Validator(
    json.loads((SCHEMAS / "fx-run-list-v1.schema.json").read_text("utf-8"))
)
REMOVAL_SCHEMA = Draft202012Validator(
    json.loads((SCHEMAS / "fx-run-removal-v1.schema.json").read_text("utf-8"))
)


def row(folder: str = "runs/case/2026-10-01-1", **fields: Any) -> dict[str, Any]:
    return {
        "folder": folder,
        "workflow": "case",
        "source": "workflows/case.yaml",
        "name": None,
        "placement": "allocated",
        "state": "succeeded",
        "created_at": "2026-10-01T00:00:00.000Z",
        "charged_usd": 0.25,
        "stand_in": False,
        **fields,
    }


def listing(*runs: dict[str, Any], **fields: Any) -> dict[str, Any]:
    return {"kind": "fx-run-list-v1", "runs": list(runs), "skipped": [], **fields}


def outcome(entry: dict[str, Any], outcome: str = "removed", **fields: Any) -> dict[str, Any]:
    return {**entry, "outcome": outcome, "freed_bytes": 120, "cache_bytes": 30, **fields}


def removal(*runs: dict[str, Any], applied: bool = True, **fields: Any) -> dict[str, Any]:
    entries = list(runs) or [outcome(row(), "removed" if applied else "would_remove")]
    return {
        "kind": "fx-run-removal-v1",
        "applied": applied,
        "runs": entries,
        "skipped": {},
        "freed_bytes": sum(entry["freed_bytes"] for entry in entries),
        "cache_bytes": sum(entry["cache_bytes"] for entry in entries),
        **fields,
    }


def valid(validator: Draft202012Validator, document: dict[str, Any]) -> str:
    errors = [f"{list(error.path)}: {error.message}" for error in validator.iter_errors(document)]
    assert errors == [], errors
    return json.dumps(document)


@pytest.fixture
def fake(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Fake:
    folder = tmp_path / "bin"
    folder.mkdir()
    return Fake(folder, monkeypatch)


@pytest.fixture
def project(tmp_path: Path) -> Path:
    root = tmp_path / "project"
    root.mkdir()
    (root / "fx.yaml").write_text("fx: project/v1\n", "utf-8")
    return root


# ------------------------------------------------------------------------------------------------
# Listing


def test_list_runs_forwards_the_filter_and_reads_every_field(
    fake: Fake, project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    stopped = row(
        "runs/case/2026-10-01-2",
        state="incomplete",
        charged_usd=1,
        stand_in=True,
        stopped="the ceiling was reached",
    )
    named = row("runs/case/named-x/take-A-y", placement="named", name="take-A", charged_usd=None)
    missing = row(
        "/elsewhere/run",
        workflow=None,
        source=None,
        placement="external",
        state="missing",
        created_at=None,
        charged_usd=None,
    )
    fake.reply(runs={"stdout": valid(LIST_SCHEMA, listing(stopped, named, missing))})
    entries = fx.list_runs(cwd=project)
    assert fake.invocations == [["runs", "list", "--json"]]
    assert fake.calls[-1]["cwd"] == str(project.resolve())
    assert [type(entry) for entry in entries] == [fx.RunEntry] * 3
    first = entries[0]
    assert first == fx.RunEntry(
        folder="runs/case/2026-10-01-2",
        run_dir=project.resolve() / "runs/case/2026-10-01-2",
        workflow="case",
        source="workflows/case.yaml",
        name=None,
        placement="allocated",
        state="incomplete",
        created_at="2026-10-01T00:00:00.000Z",
        charged_usd=1.0,
        stand_in=True,
        stopped="the ceiling was reached",
    )
    assert type(first.charged_usd) is float
    assert entries[1].name == "take-A" and entries[1].stopped is None
    assert entries[1].charged_usd is None
    assert entries[2].run_dir == Path("/elsewhere/run").resolve()
    assert entries[2].workflow is None and entries[2].state == "missing"
    # The folder is pinned when listed: changing directory does not move it.
    monkeypatch.chdir(fake.folder)
    assert first.run_dir == project.resolve() / "runs/case/2026-10-01-2"
    fake.reply(runs={"stdout": valid(LIST_SCHEMA, listing(row()))})
    assert [entry.folder for entry in fx.list_runs(workflow="case", cwd=project)] == [
        "runs/case/2026-10-01-1"
    ]
    assert fake.invocations[-1] == ["runs", "list", "--workflow=case", "--json"]


def test_list_runs_async_and_compatible_extensions(fake: Fake, project: Path) -> None:
    document = listing(
        row(future_field={"keep": True}),
        skipped=[
            {"folder": "runs/odd", "code": "unreadable_run", "message": "x", "future": 1},
        ],
        future_list={"keep": True},
    )
    fake.reply(runs={"stdout": valid(LIST_SCHEMA, document)})
    entries = asyncio.run(fx.list_runs_async(cwd=project))
    assert [entry.folder for entry in entries] == ["runs/case/2026-10-01-1"]
    assert not hasattr(entries[0], "future_field")
    # Unreadable folders are not returned, so a future engine's entries never break listing.
    document["skipped"] = [{"folder": "runs/odd", "code": "future_code"}, "future"]
    fake.reply(runs={"stdout": json.dumps(document)})
    assert len(fx.list_runs(cwd=project)) == 1

    async def inside() -> None:
        with pytest.raises(RuntimeError, match="list_runs_async"):
            fx.list_runs(cwd=project)

    asyncio.run(inside())


def test_a_list_of_thousands_of_runs_is_not_capped(fake: Fake, project: Path) -> None:
    runs = [row(f"runs/case/2026-10-01-{index}") for index in range(1, 2001)]
    stdout = valid(LIST_SCHEMA, listing(*runs))
    assert len(stdout.encode("utf-8")) > 16 * 1024 * 10
    fake.reply(runs={"stdout": stdout})
    entries = fx.list_runs(cwd=project)
    assert len(entries) == 2000 and entries[-1].folder == "runs/case/2026-10-01-2000"


@pytest.mark.parametrize(
    ("stdout", "status"),
    [
        (json.dumps(listing(row(state="future_state"))), 0),
        (json.dumps(listing(row(placement="future_placement"))), 0),
        (json.dumps(listing(row(state=["succeeded"]))), 0),
        (json.dumps(listing(row(stand_in="no"))), 0),
        (json.dumps(listing(row(charged_usd="0.25"))), 0),
        (json.dumps(listing(row(charged_usd=True))), 0),
        (json.dumps(listing(row(stopped=None))), 0),
        (json.dumps(listing(row(folder=""))), 0),
        (json.dumps(listing(row(workflow=3))), 0),
        (json.dumps(listing({key: v for key, v in row().items() if key != "created_at"})), 0),
        (json.dumps(listing(row(), kind="fx-run-list-v2")), 0),
        (json.dumps(listing(row(), skipped={})), 0),
        (json.dumps({"kind": "fx-run-list-v1", "runs": []}), 0),
        (json.dumps(listing(row())), 1),
        ('{"kind": "fx-run-list-v1", "runs": [], "skipped": [], "x": NaN}', 0),
        ("not json", 0),
    ],
)
def test_malformed_or_mismatched_lists_are_fx_errors(
    fake: Fake, project: Path, stdout: str, status: int
) -> None:
    fake.reply(runs={"stdout": stdout, "status": status})
    with pytest.raises(fx.FxError):
        fx.list_runs(cwd=project)


def test_a_list_with_another_workflows_run_is_mismatched(fake: Fake, project: Path) -> None:
    fake.reply(runs={"stdout": valid(LIST_SCHEMA, listing(row(workflow="other")))})
    with pytest.raises(fx.FxError):
        fx.list_runs(workflow="case", cwd=project)


def test_a_list_usage_error_is_the_engines_message(fake: Fake, project: Path) -> None:
    fake.reply(runs={"stderr": "grida-fx: no fx.yaml at or above here\n", "status": 2})
    with pytest.raises(fx.FxError, match="^no fx.yaml at or above here$") as caught:
        fx.list_runs(cwd=project)
    assert caught.value.status == 2


# ------------------------------------------------------------------------------------------------
# Removing


def test_remove_runs_names_explicit_targets_after_the_options(fake: Fake, project: Path) -> None:
    removed = outcome(row(), "removed")
    forgotten = outcome(
        row("/elsewhere/run", placement="external", state="missing", charged_usd=None),
        "forgotten",
        freed_bytes=0,
        cache_bytes=0,
    )
    fake.reply(runs={"stdout": valid(REMOVAL_SCHEMA, removal(removed, forgotten))})
    targets = ["runs/case/2026-10-01-1", Path("/elsewhere/run"), "-starts-with-a-dash"]
    result = fx.remove_runs(targets, cwd=project)
    assert fake.invocations == [
        [
            "runs",
            "remove",
            "--yes",
            "--json",
            "--",
            "runs/case/2026-10-01-1",
            "/elsewhere/run",
            "-starts-with-a-dash",
        ]
    ]
    assert fake.calls[-1]["cwd"] == str(project.resolve())
    assert isinstance(result, fx.RunRemoval) and result.applied
    assert result.freed_bytes == 120 and result.cache_bytes == 30
    assert dict(result.skipped) == {}
    first, second = result.runs
    assert isinstance(first, fx.RunRemovalEntry) and isinstance(first, fx.RunEntry)
    assert first.outcome == "removed" and first.code is None and first.message is None
    assert first.run_dir == project.resolve() / "runs/case/2026-10-01-1"
    assert first.charged_usd == 0.25 and first.freed_bytes == 120
    assert second.outcome == "forgotten" and second.state == "missing"
    with pytest.raises(TypeError):
        result.skipped["named"] = 1  # type: ignore[index]


def test_a_preview_removes_nothing_and_says_what_it_would(fake: Fake, project: Path) -> None:
    would = outcome(row(), "would_remove")
    fake.reply(runs={"stdout": valid(REMOVAL_SCHEMA, removal(would, applied=False))})
    result = asyncio.run(fx.remove_runs_async(("case/take-A",), preview=True, cwd=project))
    assert fake.invocations == [["runs", "remove", "--json", "--", "case/take-A"]]
    assert not result.applied and [run.outcome for run in result.runs] == ["would_remove"]

    async def inside() -> None:
        with pytest.raises(RuntimeError, match="remove_runs_async"):
            fx.remove_runs(["case/take-A"], cwd=project)

    asyncio.run(inside())
    assert len(fake.invocations) == 1


@pytest.mark.parametrize("preview", [False, True])
def test_refused_and_partial_runs_are_results_not_errors(
    fake: Fake, project: Path, preview: bool
) -> None:
    held = outcome(
        row(),
        "refused",
        code="active",
        message="an invocation is running it",
        freed_bytes=0,
        cache_bytes=0,
    )
    other = outcome(row("runs/case/2026-10-01-2"), "would_remove" if preview else "partial")
    if not preview:
        other.update(code="io", message="the tombstone could not be removed")
    document = removal(held, other, applied=not preview, skipped={"holds_pick": 2})
    fake.reply(runs={"stdout": valid(REMOVAL_SCHEMA, document), "status": 1})
    targets = ["runs/case/2026-10-01-1", "runs/case/2026-10-01-2"]
    result = fx.remove_runs(targets, preview=preview, cwd=project)
    assert result.applied is not preview
    assert result.runs[0].outcome == "refused" and result.runs[0].code == "active"
    assert result.runs[0].message == "an invocation is running it"
    assert dict(result.skipped) == {"holds_pick": 2}
    if not preview:
        assert result.runs[1].outcome == "partial" and result.runs[1].code == "io"


@pytest.mark.parametrize("code", ["invalid_target", "ambiguous_target", "not_a_run"])
def test_a_target_that_is_not_a_run_raises_with_its_code(
    fake: Fake, project: Path, code: str
) -> None:
    document = {
        "kind": "fx-run-removal-v1",
        "applied": False,
        "error": {"code": code, "message": "there is no run case/x", "run": "case/x"},
        "future": True,
    }
    fake.reply(runs={"stdout": valid(REMOVAL_SCHEMA, document), "status": 2})
    with pytest.raises(fx.RunRemovalError) as caught:
        fx.remove_runs(["case/x"], cwd=project)
    assert caught.value.code == code and caught.value.run == "case/x"
    assert caught.value.message == "there is no run case/x" and caught.value.status == 2
    assert str(caught.value) == "there is no run case/x"
    document["error"]["run"] = None
    fake.reply(runs={"stdout": valid(REMOVAL_SCHEMA, document), "status": 2})
    with pytest.raises(fx.RunRemovalError) as caught:
        fx.remove_runs(["case/x"], preview=True, cwd=project)
    assert caught.value.run is None


@pytest.mark.parametrize(
    "reply",
    [
        {"stderr": "grida-fx: runs remove takes run folders\n"},
        {"stderr": "grida-fx: runs remove takes run folders\n", "stdout": "garbage"},
        {"stdout": json.dumps(removal())},
        {
            "stdout": json.dumps(
                {
                    "kind": "fx-run-removal-v1",
                    "applied": False,
                    "error": {"code": "future_code", "message": "m", "run": None},
                }
            )
        },
        {
            "stdout": json.dumps(
                {
                    "kind": "fx-run-removal-v1",
                    "applied": True,
                    "error": {"code": "not_a_run", "message": "m", "run": None},
                }
            )
        },
        {
            "stdout": json.dumps(
                {"kind": "fx-run-removal-v1", "applied": False, "error": {"code": "not_a_run"}}
            )
        },
    ],
)
def test_other_exit_two_outputs_are_plain_fx_errors_and_removed_nothing(
    fake: Fake, project: Path, reply: dict[str, Any]
) -> None:
    fake.reply(runs={**reply, "status": 2})
    with pytest.raises(fx.FxError) as caught:
        fx.remove_runs(["runs/a"], cwd=project)
    assert not isinstance(caught.value, fx.RunRemovalError)
    assert caught.value.status == 2
    assert "may already have taken effect" not in caught.value.message


def _with_run(**fields: Any) -> Callable[[dict[str, Any]], dict[str, Any]]:
    def change(document: dict[str, Any]) -> dict[str, Any]:
        document["runs"][0].update(fields)
        return document

    return change


def _without_run_field(name: str) -> Callable[[dict[str, Any]], dict[str, Any]]:
    def change(document: dict[str, Any]) -> dict[str, Any]:
        del document["runs"][0][name]
        return document

    return change


def _other_modes_outcome(document: dict[str, Any]) -> dict[str, Any]:
    document["runs"][0]["outcome"] = "would_remove" if document["applied"] else "removed"
    return document


def _flipped(document: dict[str, Any]) -> dict[str, Any]:
    document["applied"] = not document["applied"]
    return document


def _set(**fields: Any) -> Callable[[dict[str, Any]], dict[str, Any]]:
    return lambda document: {**document, **fields}


REFUSED = {"outcome": "refused", "code": "active", "message": "an invocation is running it"}

MALFORMED_REMOVALS: list[tuple[Callable[[dict[str, Any]], Any], int]] = [
    (_with_run(outcome="future_outcome"), 0),
    (_other_modes_outcome, 0),
    (_flipped, 0),
    (lambda document: document, 1),
    (_with_run(**REFUSED), 0),
    (_with_run(outcome="refused", code="active"), 1),
    (_with_run(outcome="refused", message="m"), 1),
    (_with_run(**{**REFUSED, "code": "future_code"}), 1),
    (_with_run(**{**REFUSED, "message": None}), 1),
    (_set(skipped={"future_reason": 1}), 0),
    (_set(skipped={"named": 0}), 0),
    (_set(skipped=[]), 0),
    (_with_run(placement="future_placement"), 0),
    (_with_run(state="future_state"), 0),
    (_with_run(freed_bytes=-1), 0),
    (_with_run(cache_bytes=True), 0),
    (_without_run_field("freed_bytes"), 0),
    (_without_run_field("workflow"), 0),
    (_with_run(charged_usd="0.25"), 0),
    (_with_run(stand_in=None), 0),
    (_with_run(folder="a\0b"), 0),
    (_set(freed_bytes=1.5), 0),
    (_set(cache_bytes=-3), 0),
    (_set(runs=[]), 0),
    (lambda document: {**document, "runs": document["runs"] * 3}, 0),
    (_set(kind="fx-run-removal-v2"), 0),
    (_set(error={"code": "not_a_run", "message": "m", "run": None}), 0),
    (lambda document: [document], 0),
    (lambda document: "not json", 0),
    (lambda document: json.dumps(document)[:-1] + ', "x": NaN}', 0),
    (lambda document: document, 3),
    (lambda document: document, 101),
]


@pytest.mark.parametrize(("change", "status"), MALFORMED_REMOVALS)
def test_a_malformed_preview_never_counts_as_success(
    fake: Fake, project: Path, change: Callable[[dict[str, Any]], Any], status: int
) -> None:
    document = change(removal(outcome(row(), "would_remove"), applied=False))
    stdout = document if isinstance(document, str) else json.dumps(document)
    fake.reply(runs={"stdout": stdout, "status": status})
    with pytest.raises(fx.FxError) as caught:
        fx.remove_runs(["runs/a", "runs/b"], preview=True, cwd=project)
    assert not isinstance(caught.value, fx.RunRemovalError)
    assert "may already have taken effect" not in caught.value.message


@pytest.mark.parametrize(("change", "status"), MALFORMED_REMOVALS)
def test_an_unreadable_applied_removal_may_have_taken_effect(
    fake: Fake, project: Path, change: Callable[[dict[str, Any]], Any], status: int
) -> None:
    document = change(removal())
    stdout = document if isinstance(document, str) else json.dumps(document)
    fake.reply(runs={"stdout": stdout, "status": status})
    with pytest.raises(fx.FxError, match="the removal may already have taken effect") as caught:
        fx.remove_runs(["runs/a", "runs/b"], cwd=project)
    assert not isinstance(caught.value, fx.RunRemovalError)
    assert caught.value.status == status
    assert caught.value.stdout == stdout  # type: ignore[attr-defined]


@pytest.mark.parametrize(
    ("runs", "options"),
    [
        ("runs/a", {}),
        (Path("runs/a"), {}),
        (b"runs/a", {}),
        ([], {}),
        ((), {}),
        ([""], {}),
        (["runs/a", "a\0b"], {}),
        ([1], {}),
        ([None], {}),
        ([b"runs/a"], {}),
        ({"runs/a"}, {}),
        (["runs/a"], {"preview": "yes"}),
        (["runs/a"], {"preview": 1}),
    ],
)
def test_invalid_removals_start_no_engine(
    fake: Fake, project: Path, runs: Any, options: dict[str, Any]
) -> None:
    with pytest.raises((TypeError, ValueError)):
        fx.remove_runs(runs, cwd=project, **options)
    with pytest.raises((TypeError, ValueError)):
        asyncio.run(fx.remove_runs_async(runs, cwd=project, **options))
    assert fake.invocations == []


@pytest.mark.parametrize("workflow", ["", "case\0", 3, Path("case")])
def test_invalid_list_filters_start_no_engine(fake: Fake, project: Path, workflow: Any) -> None:
    with pytest.raises((TypeError, ValueError)):
        fx.list_runs(workflow=workflow, cwd=project)
    assert fake.invocations == []


# ------------------------------------------------------------------------------------------------
# The real engine


def _hand_written_run(project: Path, folder: str, created_at: str) -> None:
    run = project / folder
    run.mkdir(parents=True)
    plan = {"kind": "fx-graph-v1", "workflow": {"id": "case", "file": "workflows/case.yaml"}}
    (run / "plan.json").write_text(json.dumps(plan), "utf-8")
    events = [
        {"kind": "fx-run-events-v1", "event": "run_started", "created_at": created_at},
        {"event": "run_finished", "ok": True, "incomplete": False, "charged_usd": 0},
    ]
    (run / "events.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events), "utf-8")


def test_real_engine_lists_previews_removes_and_lists_again(
    project: Path, monkeypatch: pytest.MonkeyPatch
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
    _hand_written_run(project, "runs/case/2026-10-01-1", "2026-10-01T00:00:00.000Z")
    _hand_written_run(project, "runs/case/2026-10-01-2", "2026-10-01T00:00:01.000Z")
    listed = fx.list_runs(cwd=project)
    assert [entry.folder for entry in listed] == [
        "runs/case/2026-10-01-2",
        "runs/case/2026-10-01-1",
    ]
    assert all(entry.state == "succeeded" and entry.placement == "allocated" for entry in listed)
    assert listed[1].run_dir == (project / "runs/case/2026-10-01-1").resolve()
    assert listed[1].charged_usd == 0.0 and listed[1].workflow == "case"
    assert fx.list_runs(workflow="other", cwd=project) == []
    oldest = listed[1]
    preview = fx.remove_runs([oldest.run_dir], preview=True, cwd=project)
    assert not preview.applied
    assert [(run.folder, run.outcome) for run in preview.runs] == [(oldest.folder, "would_remove")]
    assert preview.freed_bytes > 0 and oldest.run_dir.is_dir()
    with pytest.raises(fx.RunRemovalError) as caught:
        fx.remove_runs([oldest.run_dir, "case/no-such-run"], cwd=project)
    assert caught.value.code == "invalid_target" and caught.value.run == "case/no-such-run"
    assert oldest.run_dir.is_dir()
    removed = fx.remove_runs([oldest.run_dir, oldest.folder], cwd=project)
    assert removed.applied and [run.outcome for run in removed.runs] == ["removed"]
    assert removed.freed_bytes == preview.freed_bytes
    assert not oldest.run_dir.exists()
    assert [entry.folder for entry in fx.list_runs(cwd=project)] == ["runs/case/2026-10-01-2"]
