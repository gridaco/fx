"""Pinned, read-only saved-run inspection and observation through the engine.

Inspection and observation are independent reads. Only an observation snapshot's cursor
attaches its captured prefix to subsequent event batches; these wrappers never derive state,
identity, source liveness or an execution result from the documents.
"""

from __future__ import annotations

import asyncio
import json
import math
import os
import re
from collections.abc import AsyncIterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from grida.fx._api import FxError, _call, _failure, _no_running_loop

_DIGEST = re.compile(r"[0-9a-f]{64}")


class RunObservationError(FxError):
    """A structured engine reader error, with its stable ``code`` and full ``document``."""

    def __init__(self, document: dict[str, Any]) -> None:
        super().__init__(document["message"], 2)
        self.code: str = document["code"]
        self.document = document


def _target(target: str | Path) -> str:
    if not isinstance(target, str | os.PathLike):
        raise TypeError("target is a run folder, workflow ID or WORKFLOW_ID/NAME selector")
    text = os.fspath(target)
    if not isinstance(text, str) or not text or "\0" in text or text.startswith("-"):
        raise ValueError("target is nonempty and cannot start with '-' or contain NUL")
    return text


def _options(after: str | None, limit: int) -> list[str]:
    if after is not None:
        if not isinstance(after, str):
            raise TypeError("after is a nonempty opaque cursor string")
        if not after or "\0" in after:
            raise ValueError("after is nonempty and contains no NUL")
    if isinstance(limit, bool) or not isinstance(limit, int):
        raise TypeError("limit is an integer from 1 to 1024")
    if not 1 <= limit <= 1024:
        raise ValueError("limit is an integer from 1 to 1024")
    return ([] if after is None else [f"--after={after}"]) + [f"--limit={limit}"]


def _text(value: Any) -> bool:
    return isinstance(value, str) and bool(value)


def _reject_constant(value: str) -> None:
    raise ValueError(f"non-JSON constant {value}")


def _event(event: Any) -> bool:
    return (
        isinstance(event, dict)
        and event.get("kind") == "fx-run-events-v1"
        and _text(event.get("event"))
        and _text(event.get("invocation_id"))
        and isinstance(event.get("plan"), str)
        and _DIGEST.fullmatch(event["plan"]) is not None
        and type(event.get("offset_ms")) is int
        and event["offset_ms"] >= 0
    )


async def _observe(
    run_dir: Path,
    *,
    snapshot: bool,
    options: list[str],
    after: str | None = None,
    limit: int = 256,
) -> dict[str, Any]:
    done = await _call(["observe", str(run_dir), *options], Path.cwd())
    try:
        document = json.loads(done.stdout, parse_constant=_reject_constant)
    except ValueError:
        raise FxError("grida-fx observe printed malformed JSON", done.status) from None
    if not isinstance(document, dict):
        raise FxError("grida-fx observe printed no JSON object", done.status)
    if done.status == 2 and document.get("kind") == "fx-run-observation-error-v1":
        if _text(document.get("code")) and _text(document.get("message")):
            raise RunObservationError(document)
        raise FxError("grida-fx observe printed a malformed reader error", done.status)
    if done.status != 0:
        raise _failure("observe", done)
    kind = "fx-run-snapshot-v1" if snapshot else "fx-run-event-batch-v1"
    events = document.get("events")
    valid = (
        document.get("kind") == kind
        and _text(document.get("cursor"))
        and "\0" not in document["cursor"]
        and isinstance(events, list)
        and all(_event(event) for event in events)
    )
    if snapshot:
        valid = (
            valid
            and isinstance(document.get("plan"), dict)
            and document["plan"].get("kind") == "fx-graph-v1"
        )
    else:
        valid = (
            valid
            and type(document.get("has_more")) is bool
            and len(events) <= limit
            and (bool(events) or not document["has_more"])
            and (
                after is None
                or (document["cursor"] != after if events else document["cursor"] == after)
            )
        )
    if not valid:
        raise FxError("grida-fx observe printed a malformed observation document", done.status)
    return document


@dataclass(frozen=True)
class RunRecord:
    """A selected folder and its engine inspection document, independent of a run result.

    ``run_dir`` is pinned when loaded; changing discovery or the inspection document does not
    retarget observation. ``snapshot`` and ``events`` preserve the engine's wire documents.
    """

    run_dir: Path
    inspection: dict[str, Any]

    def __post_init__(self) -> None:
        object.__setattr__(self, "run_dir", Path(self.run_dir).resolve())

    def snapshot(self) -> dict[str, Any]:
        """Capture a consistent saved-plan/event prefix and its opaque cursor."""
        _no_running_loop("RunRecord.snapshot")
        return asyncio.run(self.snapshot_async())

    async def snapshot_async(self) -> dict[str, Any]:
        return await _observe(self.run_dir, snapshot=True, options=["--snapshot"])

    def events(self, after: str | None = None, limit: int = 256) -> dict[str, Any]:
        """Read one bounded event batch, from the beginning unless ``after`` is supplied."""
        _no_running_loop("RunRecord.events")
        return asyncio.run(self.events_async(after=after, limit=limit))

    async def events_async(self, after: str | None = None, limit: int = 256) -> dict[str, Any]:
        return await _observe(
            self.run_dir, snapshot=False, options=_options(after, limit), after=after, limit=limit
        )

    async def follow(
        self, after: str | None = None, limit: int = 256, poll_interval: float = 1.0
    ) -> AsyncIterator[dict[str, Any]]:
        """Yield nonempty batches across invocations until the caller breaks or cancels.

        Requests follow consumer demand: drain ``has_more`` pages immediately, then sleep.
        No terminal event ends this iterator. Reader cancellation stops only its observer.
        """
        _options(after, limit)
        if isinstance(poll_interval, bool) or not isinstance(poll_interval, int | float):
            raise TypeError("poll_interval is a positive finite number")
        if not math.isfinite(poll_interval) or poll_interval <= 0:
            raise ValueError("poll_interval is a positive finite number")
        while True:
            batch = await self.events_async(after=after, limit=limit)
            # Capture immutable wire values before caller code can mutate the yielded dict.
            after, has_more = batch["cursor"], batch["has_more"]
            if batch["events"]:
                yield batch
            del batch
            if not has_more:
                await asyncio.sleep(poll_interval)


async def load_run_async(
    target: str | Path, *, verify: bool = False, cwd: str | Path | None = None
) -> RunRecord:
    """Inspect a folder or workflow/name selector once and pin its selected folder.

    ``--verify`` problems remain inspection evidence rather than an execution failure.
    Snapshot and event reads are separate from this inspection read.
    """
    text = _target(target)
    if type(verify) is not bool:
        raise TypeError("verify is True or False")
    base = (Path(cwd) if cwd is not None else Path.cwd()).resolve()
    done = await _call(["inspect", text, "--json", *(["--verify"] if verify else [])], base)
    if done.status != 0 and not (verify and done.status == 1):
        raise _failure("inspect", done)
    try:
        document = json.loads(done.stdout, parse_constant=_reject_constant)
    except ValueError:
        raise FxError("grida-fx inspect printed malformed JSON", done.status) from None
    if not isinstance(document, dict) or not isinstance(document.get("run"), dict):
        raise FxError("grida-fx inspect printed no run inspection object", done.status)
    if done.status == 1:
        verification = document.get("verification")
        if (
            not isinstance(verification, dict)
            or verification.get("verified") is not False
            or not isinstance(verification.get("problems"), list)
            or not verification["problems"]
            or not all(_text(problem) for problem in verification["problems"])
        ):
            raise FxError("grida-fx inspect printed no failed verification evidence", done.status)
    folder = document["run"].get("folder")
    if not _text(folder) or "\0" in folder:
        raise FxError("grida-fx inspect printed no selected run folder", done.status)
    return RunRecord(base / folder, document)


def load_run(
    target: str | Path, *, verify: bool = False, cwd: str | Path | None = None
) -> RunRecord:
    """:func:`load_run_async`, for code outside an event loop."""
    _no_running_loop("load_run")
    return asyncio.run(load_run_async(target, verify=verify, cwd=cwd))
