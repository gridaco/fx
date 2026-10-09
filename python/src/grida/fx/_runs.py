"""A project's runs through the engine's public command line (spec/store.md §8 and §9).

The engine finds the project from ``cwd``, resolves every run, takes each run's lock and removes
it; these wrappers forward explicit targets and preserve its structured result. They select
nothing themselves: a caller filters :func:`list_runs` and names what to remove, so what is
removed is exactly what it chose. Results are unbounded, since a project may hold thousands of
runs.
"""

from __future__ import annotations

import asyncio
import json
import math
import os
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any

from grida.fx._api import _USAGE_OR_ERROR, FxError, _call, _Exit, _failure, _no_running_loop

_LIST_KIND = "fx-run-list-v1"
_REMOVAL_KIND = "fx-run-removal-v1"
_PLACEMENTS = {"allocated", "named", "explicit", "external"}
_STATES = {
    "planned",
    "unfinished",
    "succeeded",
    "failed",
    "incomplete",
    "cancelled",
    "empty",
    "removing",
    "missing",
}
_PREVIEW_OUTCOMES = {"would_remove", "would_forget", "refused"}
_APPLIED_OUTCOMES = {"removed", "forgotten", "refused", "partial"}
_FAILED_OUTCOMES = {"refused", "partial"}
_RUN_CODES = {"active", "holds_pick", "changed", "not_removable", "io"}
_REMOVAL_SKIPS = {"named", "explicit", "external", "holds_pick", "young", "holds_runs"}
_ERROR_CODES = {"invalid_target", "ambiguous_target", "not_a_run"}


@dataclass(frozen=True)
class RunEntry:
    """One run as ``fx-run-list-v1`` gives it; field names match the public wire contract.

    ``folder`` is as the engine printed it (relative to ``cwd``, or absolute); ``run_dir`` is that
    folder resolved once against the call's ``cwd``, and a removal accepts it as it is. Recorded
    ``unfinished`` is not evidence that a process is alive. ``charged_usd`` is ``None`` before
    the run ended; ``stopped`` says why an ``incomplete`` run stopped.
    """

    folder: str
    run_dir: Path
    workflow: str | None
    source: str | None
    name: str | None
    placement: str
    state: str
    created_at: str | None
    charged_usd: float | None
    stand_in: bool
    stopped: str | None

    def __post_init__(self) -> None:
        object.__setattr__(self, "run_dir", Path(self.run_dir).resolve())


@dataclass(frozen=True)
class RunRemovalEntry(RunEntry):
    """What removing one run did, or would do in a preview (``would_remove``, ``would_forget``).

    ``forgotten``: a missing external folder's run index and catalog entries were removed.
    ``refused`` (nothing of the run was removed) and ``partial`` (gone as a run, but its tombstone
    remains; removing it again finishes it) carry ``code`` and ``message``. ``freed_bytes``
    counts files whose every link was inside the folder; ``cache_bytes``, files still linked from
    elsewhere, which removing it does not free.
    """

    outcome: str
    code: str | None
    message: str | None
    freed_bytes: int
    cache_bytes: int


@dataclass(frozen=True)
class RunRemoval:
    """The engine's ``fx-run-removal-v1`` result: what a removal did, or would do when
    ``applied`` is false. ``skipped`` counts runs a selection passed over, by reason; explicit
    runs are never passed over, so a removal through this SDK normally reports none."""

    applied: bool
    runs: tuple[RunRemovalEntry, ...]
    skipped: Mapping[str, int]
    freed_bytes: int
    cache_bytes: int


class RunRemovalError(FxError):
    """A run the engine would not resolve (exit status 2): nothing was removed. ``code`` is
    ``invalid_target``, ``ambiguous_target`` or ``not_a_run``; ``run`` is the target it names.

    Usage errors and malformed responses raise ordinary :class:`FxError` instead, and a refused or
    partial run is reported in the :class:`RunRemoval`, not raised.
    """

    def __init__(self, code: str, message: str, run: str | None) -> None:
        super().__init__(message or f"runs remove refused: {code}", 2)
        self.code = code
        self.run = run


def _reject_constant(value: str) -> None:
    raise ValueError(f"non-JSON constant {value}")


def _base(cwd: str | Path | None) -> Path:
    return (Path(cwd) if cwd is not None else Path.cwd()).resolve()


def _workflow(workflow: str | None) -> list[str]:
    if workflow is None:
        return []
    if not isinstance(workflow, str):
        raise TypeError("workflow is a workflow id")
    if not workflow or "\0" in workflow:
        raise ValueError("workflow is a nonempty workflow id without NUL")
    return [f"--workflow={workflow}"]


def _targets(runs: Sequence[str | os.PathLike[str]]) -> list[str]:
    if isinstance(runs, str | bytes) or not isinstance(runs, Sequence):
        raise TypeError("runs is a sequence of run folders or WORKFLOW_ID/NAME, not one string")
    if not runs:
        raise ValueError("runs names at least one run")
    targets = []
    for run in runs:
        if not isinstance(run, str | os.PathLike):
            raise TypeError("each run is a run folder or WORKFLOW_ID/NAME")
        text = os.fspath(run)
        if not isinstance(text, str) or not text or "\0" in text:
            raise ValueError("each run is nonempty and contains no NUL")
        targets.append(text)
    return targets


def _text(value: Any) -> bool:
    return value is None or isinstance(value, str)


def _count(value: Any) -> bool:
    return type(value) is int and value >= 0


def _charge(value: Any) -> float | None:
    """A recorded charge in US dollars, read as :attr:`RunResult.cost` reads one: a float."""
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, int | float):
        raise ValueError("charged_usd is not an amount")
    try:
        amount = float(value)
    except OverflowError:
        raise ValueError("charged_usd is not an amount") from None
    if not math.isfinite(amount):
        raise ValueError("charged_usd is not an amount")
    return amount


def _entry(row: Any, base: Path) -> dict[str, Any]:
    """The :class:`RunEntry` fields of one wire row; compatible extensions are ignored."""
    if not isinstance(row, dict):
        raise ValueError("a run is not an object")
    folder = row.get("folder")
    if not isinstance(folder, str) or not folder or "\0" in folder:
        raise ValueError("invalid folder")
    required = {"workflow", "source", "name", "created_at", "charged_usd"}
    if not required <= row.keys():
        raise ValueError("missing fields")
    if not all(_text(row[name]) for name in ("workflow", "source", "name", "created_at")):
        raise ValueError("invalid run text field")
    if row.get("placement") not in _PLACEMENTS or row.get("state") not in _STATES:
        raise ValueError("unknown placement or state")
    if type(row.get("stand_in")) is not bool:
        raise ValueError("invalid stand_in")
    if "stopped" in row and not isinstance(row["stopped"], str):
        raise ValueError("invalid stopped")
    return {
        "folder": folder,
        "run_dir": base / folder,
        "workflow": row["workflow"],
        "source": row["source"],
        "name": row["name"],
        "placement": row["placement"],
        "state": row["state"],
        "created_at": row["created_at"],
        "charged_usd": _charge(row["charged_usd"]),
        "stand_in": row["stand_in"],
        "stopped": row.get("stopped"),
    }


def _listed(document: Any, base: Path, workflow: str | None) -> list[RunEntry]:
    if not isinstance(document, dict) or document.get("kind") != _LIST_KIND:
        raise ValueError("incompatible result")
    # Unreadable folders are not returned, so their entries and codes may grow freely.
    runs, skipped = document.get("runs"), document.get("skipped")
    if not isinstance(runs, list) or not isinstance(skipped, list):
        raise ValueError("missing runs or skipped")
    entries = [RunEntry(**_entry(row, base)) for row in runs]
    if workflow is not None and any(entry.workflow != workflow for entry in entries):
        raise ValueError("a run of another workflow")
    return entries


def _removal_entry(row: Any, base: Path, outcomes: set[str]) -> RunRemovalEntry:
    fields = _entry(row, base)
    outcome = row.get("outcome")
    if outcome not in outcomes:
        raise ValueError("unknown or mismatched outcome")
    if "code" in row and row["code"] not in _RUN_CODES:
        raise ValueError("unknown code")
    if "message" in row and not isinstance(row["message"], str):
        raise ValueError("invalid message")
    if outcome in _FAILED_OUTCOMES and not {"code", "message"} <= row.keys():
        raise ValueError("refusal lacks code or message")
    if not _count(row.get("freed_bytes")) or not _count(row.get("cache_bytes")):
        raise ValueError("invalid byte counts")
    return RunRemovalEntry(
        **fields,
        outcome=outcome,
        code=row.get("code"),
        message=row.get("message"),
        freed_bytes=row["freed_bytes"],
        cache_bytes=row["cache_bytes"],
    )


def _removal(document: Any, base: Path, targets: int, preview: bool, status: int) -> RunRemoval:
    if not isinstance(document, dict) or document.get("kind") != _REMOVAL_KIND:
        raise ValueError("incompatible result")
    applied = document.get("applied")
    if "error" in document or type(applied) is not bool or applied == preview:
        raise ValueError("an error document or a mismatched applied")
    runs, skipped = document.get("runs"), document.get("skipped")
    freed, cache = document.get("freed_bytes"), document.get("cache_bytes")
    if not isinstance(runs, list) or not isinstance(skipped, dict):
        raise ValueError("missing runs or skipped")
    if not _count(freed) or not _count(cache):
        raise ValueError("invalid byte counts")
    if any(reason not in _REMOVAL_SKIPS or not _count(n) or n < 1 for reason, n in skipped.items()):
        raise ValueError("invalid skipped")
    # Every target resolves to one run or fails the command; repeats of one run are merged.
    if not 1 <= len(runs) <= targets:
        raise ValueError("not one run per distinct target")
    outcomes = _PREVIEW_OUTCOMES if preview else _APPLIED_OUTCOMES
    entries = tuple(_removal_entry(row, base, outcomes) for row in runs)
    if status != int(any(entry.outcome in _FAILED_OUTCOMES for entry in entries)):
        raise ValueError("mismatched exit status")
    return RunRemoval(applied, entries, MappingProxyType(dict(skipped)), freed, cache)


def _refusal(done: _Exit) -> RunRemovalError | None:
    """The error document of exit status 2, or ``None`` when stdout holds none."""
    try:
        document = json.loads(done.stdout, parse_constant=_reject_constant)
    except ValueError:
        return None
    if (
        not isinstance(document, dict)
        or document.get("kind") != _REMOVAL_KIND
        or document.get("applied") is not False
        or not isinstance(document.get("error"), dict)
    ):
        return None
    error = document["error"]
    code, message = error.get("code"), error.get("message")
    if (
        not isinstance(code, str)
        or code not in _ERROR_CODES
        or not isinstance(message, str)
        or "run" not in error
        or not _text(error["run"])
    ):
        return None
    return RunRemovalError(code, message, error["run"])


def _unverified(done: _Exit) -> FxError:
    """An applied removal whose result cannot be read: some runs may already be gone."""
    what = (
        "grida-fx runs remove printed a result that could not be read"
        if done.status in (0, 1)
        else _failure("runs remove", done).message
    )
    error = FxError(f"{what}; the removal may already have taken effect", done.status)
    error.stdout = done.stdout  # type: ignore[attr-defined]
    return error


async def list_runs_async(
    *, workflow: str | None = None, cwd: str | Path | None = None
) -> list[RunEntry]:
    """The runs of the project found from ``cwd``, newest first, as ``runs list`` gives them.

    ``workflow`` keeps only that workflow id's runs. Folders the engine could not read are left
    out. Listing takes no lock and writes nothing.
    """
    options = _workflow(workflow)
    base = _base(cwd)
    done = await _call(["runs", "list", *options, "--json"], base)
    if done.status != 0:
        raise _failure("runs list", done)
    try:
        return _listed(json.loads(done.stdout, parse_constant=_reject_constant), base, workflow)
    except (ValueError, TypeError):
        raise _failure("runs list", done) from None


def list_runs(*, workflow: str | None = None, cwd: str | Path | None = None) -> list[RunEntry]:
    """:func:`list_runs_async`, for code outside an event loop."""
    _no_running_loop("list_runs")
    return asyncio.run(list_runs_async(workflow=workflow, cwd=cwd))


async def remove_runs_async(
    runs: Sequence[str | os.PathLike[str]],
    *,
    preview: bool = False,
    cwd: str | Path | None = None,
) -> RunRemoval:
    """Remove exactly the named runs, or with ``preview=True`` say what removing them would do.

    Each run is a run folder (relative to ``cwd``; a :attr:`RunEntry.run_dir` as it is) or
    ``WORKFLOW_ID/NAME`` of a named run. Every run is resolved before anything is removed, and
    one that is not a run raises :class:`RunRemovalError`. A run an invocation holds
    (``active``), or the last one holding a pick (``holds_pick``), is refused in the result
    without raising: read each entry's ``outcome``. When the result of an applied removal cannot
    be read, the :class:`FxError` says the removal may already have taken effect and keeps the
    engine's output as ``stdout``.
    """
    targets = _targets(runs)
    if type(preview) is not bool:
        raise TypeError("preview is True or False")
    base = _base(cwd)
    args = ["runs", "remove", *([] if preview else ["--yes"]), "--json", "--", *targets]
    done = await _call(args, base)
    if done.status == _USAGE_OR_ERROR:
        refused = _refusal(done)
        if refused is not None:
            raise refused
        raise _failure("runs remove", done)
    try:
        if done.status not in (0, 1):
            raise ValueError("unexpected status")
        document = json.loads(done.stdout, parse_constant=_reject_constant)
        return _removal(document, base, len(targets), preview, done.status)
    except (ValueError, TypeError):
        raise (_failure("runs remove", done) if preview else _unverified(done)) from None


def remove_runs(
    runs: Sequence[str | os.PathLike[str]],
    *,
    preview: bool = False,
    cwd: str | Path | None = None,
) -> RunRemoval:
    """:func:`remove_runs_async`, for code outside an event loop."""
    _no_running_loop("remove_runs")
    return asyncio.run(remove_runs_async(runs, preview=preview, cwd=cwd))
