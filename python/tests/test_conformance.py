"""Scheduling-independent comparison of recorded display snapshots."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

import pytest


def _runner() -> Any:
    source = Path(__file__).resolve().parents[2] / "conformance" / "run.py"
    spec = importlib.util.spec_from_file_location("fx_conformance_run", source)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


runner = _runner()


def event(name: str, invocation: str = "one", **fields: object) -> dict:
    return {
        "kind": "fx-run-events-v1",
        "event": name,
        "invocation_id": invocation,
        "offset_ms": 1,
        **fields,
    }


def snapshot(
    left: object = None,
    right: object = None,
    *,
    invocation: str = "one",
    instances: tuple[str, ...] = ("left", "right"),
    links: tuple[str, ...] = ("source",),
) -> dict:
    def value(name: str, data: object) -> dict:
        if data is None:
            return {"id": name, "state": "pending"}
        return {"id": name, "state": "known", "data": {"value": data}}

    return event(
        "scopes_updated",
        invocation,
        scopes=[],
        instances=list(instances),
        pending=[],
        value_views=[value("left", left), value("right", right)],
        value_links=list(links),
    )


def normal(*events: dict) -> bytes:
    raw = "\n".join(json.dumps(item) for item in events).encode()
    return runner.normalised_jsonl(raw, "events.jsonl")


def test_independent_completion_orders_and_update_counts_compare_equally() -> None:
    start = event("run_started")
    left = event("node_finished", id="left")
    right = event("node_finished", id="right")
    final = snapshot("a", "b")
    assert normal(start, snapshot(), left, snapshot("a"), right, final) == normal(
        start, snapshot(), right, snapshot(None, "b"), left, final
    )
    assert normal(start, snapshot(), left, snapshot("a"), right, final) == normal(
        start, snapshot(), right, left, final
    )


@pytest.mark.parametrize("changed", [snapshot("wrong", "b"), snapshot("a", "b", links=("other",))])
def test_changed_terminal_values_and_links_still_fail(changed: dict) -> None:
    assert normal(event("run_started"), snapshot(), snapshot("a", "b")) != normal(
        event("run_started"), snapshot(), changed
    )


def test_changed_initial_values_still_fail() -> None:
    assert normal(snapshot(), snapshot("a", "b")) != normal(snapshot("changed"), snapshot("a", "b"))


def test_structural_expansion_is_not_hidden_with_intermediate_values() -> None:
    ordinary = normal(event("run_started"), snapshot(), snapshot("a", "b"))
    expanded = normal(
        event("run_started"),
        snapshot(),
        snapshot("a", instances=("left", "right", "extra")),
        snapshot("a", "b"),
    )
    assert ordinary != expanded
    records = json.loads(expanded)
    structural = next(item for item in records if "extra" in item.get("instances", []))
    assert "value_views" not in structural and "value_links" not in structural


def test_each_resume_retains_its_own_endpoint_values_before_sorting() -> None:
    first = [event("run_started"), snapshot(), snapshot("a", "first")]
    resume = [
        event("run_started", "two"),
        snapshot("a", "first", invocation="two"),
        snapshot("a", "second", invocation="two"),
    ]
    original = normal(*first, *resume)
    assert original != normal(*first[:-1], snapshot("a", "changed"), *resume)
    records = json.loads(original)
    captures = [item["value_views"] for item in records if item.get("event") == "scopes_updated"]
    assert len(captures) == 4
    assert captures.count(snapshot("a", "first")["value_views"]) == 2


@pytest.mark.parametrize("field", ["with", "outputs", "facts"])
def test_nested_authored_data_and_legacy_snapshots_are_unchanged(field: str) -> None:
    opaque = {"event": "scopes_updated", "offset_ms": 9, "value_views": ["authored"]}
    records = json.loads(normal(event("node_started", **{field: opaque})))
    assert records[0][field] == opaque
    legacy = event("scopes_updated", scopes=[], instances=[])
    assert len(json.loads(normal(legacy, legacy))) == 2


def test_identical_initial_and_final_capture_does_not_depend_on_interim_count() -> None:
    assert normal(snapshot(), snapshot("transient"), snapshot()) == normal(snapshot())
