"""Exact-target run control through the engine's public command line (spec/control.md).

The engine resolves the invocation, authenticates its owner and verifies cleanup. These
wrappers forward options and preserve its structured result; they compute no identity.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from grida.fx._api import FxError, _call, _failure, _no_running_loop

_MAX_RESULT_BYTES = 16 * 1024
_OUTCOMES = {
    "inspected",
    "accepted",
    "already_requested",
    "already_terminal",
    "finishing",
    "completed",
    "error",
}
_ERROR_STATUS = {
    "unavailable": 1,
    "wait_timeout": 1,
    "owner_lost": 1,
    "acknowledgment_unknown": 1,
    "completion_unverified": 1,
    "invalid_target": 2,
    "ambiguous_target": 2,
    "invocation_mismatch": 2,
    "unsupported_version": 2,
    "unauthorized": 2,
    "record_error": 2,
}


@dataclass(frozen=True)
class RunControlResult:
    """The engine's fx-run-control-v1 outcome; field names match the public wire contract.

    ``accepted`` is cancellation intent. Only ``cleanup == "complete"`` certifies local
    cleanup; ``external_completion`` always remains ``not_verified``.
    """

    kind: str
    operation: str
    invocation_id: str | None
    outcome: str
    request_status: str
    recorded_state: str
    cleanup: str
    external_completion: str
    code: str | None = None
    message: str | None = None
    availability: str | None = None
    can_cancel: bool | None = None


class RunControlError(FxError):
    """A structured control failure, retaining its stable ``code`` and complete ``result``.

    Unsupported binaries and malformed responses raise ordinary :class:`FxError` instead.
    """

    def __init__(self, result: RunControlResult, status: int) -> None:
        super().__init__(result.message or f"run control failed: {result.code}", status)
        self.result = result
        self.code = result.code


def _target(run: str | Path) -> str:
    if not isinstance(run, str | os.PathLike):
        raise TypeError("run is an explicit run folder or exact WORKFLOW_ID/NAME")
    text = os.fspath(run)
    if not isinstance(text, str) or not text or text.startswith("-") or "\0" in text:
        raise ValueError("run is a nonempty target that cannot start with '-' or contain NUL")
    return text


def _cancel_options(invocation: str | None, wait: bool, timeout: str | None) -> list[str]:
    if type(wait) is not bool:
        raise TypeError("wait is True or False")
    options = ["--source=sdk"]
    if invocation is not None:
        if not isinstance(invocation, str) or not invocation or "\0" in invocation:
            raise TypeError("invocation is a nonempty invocation ID")
        options.append(f"--invocation={invocation}")
    if wait:
        options.append("--wait")
    if timeout is not None:
        if not wait:
            raise ValueError("timeout requires wait=True")
        if not isinstance(timeout, str) or not re.fullmatch(r"[0-9]+[smh]", timeout):
            raise ValueError("timeout is a positive integer followed by s, m or h")
        if int(timeout[:-1]) == 0:
            raise ValueError("timeout is a positive integer followed by s, m or h")
        options.append(f"--timeout={timeout}")
    return options


def _result(document: Any, operation: str, wait: bool, status: int) -> RunControlResult:
    if not isinstance(document, dict):
        raise ValueError("not an object")
    required = {
        "kind",
        "operation",
        "invocation_id",
        "outcome",
        "request_status",
        "recorded_state",
        "cleanup",
        "external_completion",
    }
    optional = {"code", "message", "availability", "can_cancel"}
    if not required <= document.keys():
        raise ValueError("missing fields")
    for name, value in document.items():
        if name not in required | optional:
            continue  # Compatible extensions never change the validated outcome.
        if name == "can_cancel":
            if type(value) is not bool:
                raise ValueError("can_cancel is not boolean")
        elif name == "invocation_id" and value is None:
            continue
        elif (
            not isinstance(value, str)
            or not value
            or len(value) > (256 if name == "invocation_id" else 4096)
        ):
            raise ValueError(f"invalid {name}")
    if document["kind"] != "fx-run-control-v1" or document["operation"] != operation:
        raise ValueError("incompatible result")
    enums = {
        "outcome": _OUTCOMES,
        "request_status": {"accepted", "not_accepted", "unknown"},
        "recorded_state": {"planned", "unfinished", "succeeded", "failed", "cancelled", "unknown"},
        "cleanup": {"pending", "complete", "unknown"},
        "external_completion": {"not_verified"},
    }
    if any(document[name] not in values for name, values in enums.items()):
        raise ValueError("unknown result enum")
    if operation == "inspect":
        if document.get("availability") not in {
            "available",
            "unavailable",
            "unsupported",
            "unknown",
        }:
            raise ValueError("missing or unknown availability")
        if type(document.get("can_cancel")) is not bool:
            raise ValueError("missing can_cancel")
    elif "availability" in document or "can_cancel" in document:
        raise ValueError("inspection fields on cancellation")
    outcome = document["outcome"]
    if outcome == "completed" and (
        document["invocation_id"] is None
        or document["cleanup"] != "complete"
        or document["recorded_state"] not in {"succeeded", "failed", "cancelled"}
    ):
        raise ValueError("completion lacks invocation, cleanup or terminal evidence")
    if outcome == "error":
        if _ERROR_STATUS.get(document.get("code")) != status or "message" not in document:
            raise ValueError("unknown code or mismatched error status")
    else:
        allowed = (
            {"inspected"}
            if operation == "inspect"
            else (
                {"completed"}
                if wait
                else {"accepted", "already_requested", "already_terminal", "finishing"}
            )
        )
        if status != 0 or outcome not in allowed or "code" in document or "message" in document:
            raise ValueError("mismatched success outcome or status")
    return RunControlResult(
        **{name: value for name, value in document.items() if name in required | optional}
    )


async def _control(
    operation: str,
    run: str | Path,
    *,
    cwd: str | Path | None,
    options: list[str],
    wait: bool = False,
) -> RunControlResult:
    verb = "inspect" if operation == "inspect" else "cancel"
    args = [verb, _target(run), *options, "--json"]
    done = await _call(args, Path.cwd() if cwd is None else Path(cwd).absolute())
    try:
        if len(done.stdout.encode("utf-8")) > _MAX_RESULT_BYTES:
            raise ValueError("oversized result")
        result = _result(json.loads(done.stdout), operation, wait, done.status)
    except (ValueError, TypeError):
        raise _failure(verb, done) from None
    if result.outcome == "error":
        raise RunControlError(result, done.status)
    return result


async def inspect_control_async(
    run: str | Path, *, cwd: str | Path | None = None
) -> RunControlResult:
    """Inspect one exact target's recorded invocation and authenticated control availability.

    A bare workflow ID is refused by the engine; no latest-run selection is implied.
    """
    return await _control("inspect", run, cwd=cwd, options=["--control"])


def inspect_control(run: str | Path, *, cwd: str | Path | None = None) -> RunControlResult:
    """:func:`inspect_control_async`, for code outside an event loop."""
    _no_running_loop("inspect_control")
    return asyncio.run(inspect_control_async(run, cwd=cwd))


async def cancel_async(
    run: str | Path,
    *,
    invocation: str | None = None,
    wait: bool = False,
    timeout: str | None = None,
    cwd: str | Path | None = None,
) -> RunControlResult:
    """Request cancellation of one exact target, optionally guarded by ``invocation``.

    ``wait=True`` verifies local completion, with a default 30-second timeout. ``timeout``
    accepts e.g. ``"5s"`` or ``"2m"`` and requires ``wait=True``. A timeout or cancellation
    of this client task only ends its wait; an accepted run cancellation remains effective.
    """
    return await _control(
        "cancel", run, cwd=cwd, options=_cancel_options(invocation, wait, timeout), wait=wait
    )


def cancel(
    run: str | Path,
    *,
    invocation: str | None = None,
    wait: bool = False,
    timeout: str | None = None,
    cwd: str | Path | None = None,
) -> RunControlResult:
    """:func:`cancel_async`, for code outside an event loop."""
    _no_running_loop("cancel")
    return asyncio.run(
        cancel_async(run, invocation=invocation, wait=wait, timeout=timeout, cwd=cwd)
    )
