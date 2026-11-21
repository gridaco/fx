"""Discover and answer a workflow's run-scoped external requests."""

from __future__ import annotations

import asyncio
import json
import re
import tempfile
from pathlib import Path
from typing import Any

from grida.fx._api import FxError, _call, _Exit, _failure, _no_running_loop
from grida.fx._control import _target
from grida.fx._protocol import encode_message


class AnswerError(FxError):
    """A refused submission. A conflict or expiry carries the recorded request."""

    def __init__(self, receipt: dict[str, Any], status: int) -> None:
        super().__init__(receipt["message"], status)
        self.code: str = receipt["code"]
        self.request: dict[str, Any] | None = receipt.get("request")


def _read_reply(verb: str, done: _Exit) -> dict[str, Any]:
    try:
        body = json.loads(done.stdout)
    except ValueError:
        raise _failure(verb, done) from None
    if not isinstance(body, dict):
        raise _failure(verb, done)
    return body


def _is_request(value: Any) -> bool:
    if not (
        isinstance(value, dict)
        and isinstance(value.get("id"), str)
        and re.fullmatch(r"[0-9a-f]{64}", value["id"])
        and isinstance(value.get("step"), str)
        and isinstance(value.get("opened_at"), str)
        and isinstance(value.get("schema"), bool | dict)
        and "show" in value
        and value.get("on_timeout") in ("fail", "continue")
        and all(
            key not in value or isinstance(value[key], str)
            for key in ("message", "choice_field", "deadline")
        )
        and ("choices" not in value or isinstance(value["choices"], dict))
        and (
            "timeout_seconds" not in value
            or (type(value["timeout_seconds"]) is int and 0 < value["timeout_seconds"] < 2**53)
        )
    ):
        return False
    state = value.get("state")
    if not isinstance(state, dict):
        return False
    if state.get("status") == "pending":
        return True
    if not isinstance(state.get("at"), str):
        return False
    return state.get("status") == "expired" or (
        state.get("status") == "answered"
        and "answer" in state
        and state.get("channel") in ("cli", "sdk", "viewer")
    )


async def _request_inspection(
    run: str | Path, cwd: str | Path | None, *options: str
) -> dict[str, Any]:
    args = ["inspect", "--requests", "--json", *options, "--", _target(run)]
    done = await _call(args, Path(cwd or ".").absolute())
    if done.status != 0:
        raise _failure("inspect", done)
    result = _read_reply("inspect", done)
    if not isinstance(result.get("requests"), list) or not all(
        _is_request(r) for r in result["requests"]
    ):
        raise _failure("inspect", done)
    if options and ("viewer_url" not in result or not isinstance(result["viewer_url"], str | None)):
        raise _failure("inspect", done)
    return result


async def inspect_requests_async(
    run: str | Path, *, cwd: str | Path | None = None, register: bool = False
) -> dict[str, Any]:
    """Inspect requests and discover the local viewer URL. Registration is explicit."""
    return await _request_inspection(run, cwd, "--register" if register else "--viewer-url")


def inspect_requests(
    run: str | Path, *, cwd: str | Path | None = None, register: bool = False
) -> dict[str, Any]:
    """Inspect requests and their run's viewer URL synchronously."""
    _no_running_loop("inspect_requests")
    return asyncio.run(inspect_requests_async(run, cwd=cwd, register=register))


async def list_requests_async(
    run: str | Path, *, cwd: str | Path | None = None
) -> list[dict[str, Any]]:
    """List pending and resolved requests in an exact run folder or workflow/name."""
    return (await _request_inspection(run, cwd))["requests"]


def list_requests(run: str | Path, *, cwd: str | Path | None = None) -> list[dict[str, Any]]:
    """List requests synchronously."""
    _no_running_loop("list_requests")
    return asyncio.run(list_requests_async(run, cwd=cwd))


async def respond_async(
    run: str | Path, request_id: str, answer: Any, *, cwd: str | Path | None = None
) -> dict[str, Any]:
    """Submit a JSON value and return the durably answered request."""
    args = ["respond", "--source=sdk", "--json=-", "--", _target(run), request_id]
    with tempfile.TemporaryFile() as stream:
        stream.write(encode_message(answer))
        stream.seek(0)
        done = await _call(args, Path(cwd or ".").absolute(), stream)
    receipt = _read_reply("respond", done)
    request = receipt.get("request")
    if "request" in receipt and (not _is_request(request) or request["id"] != request_id):
        raise _failure("respond", done)
    status = request["state"]["status"] if request is not None else None
    if done.status == 0 and status == "answered":
        return request
    code = receipt.get("code")
    if done.status in (1, 2) and isinstance(code, str) and isinstance(receipt.get("message"), str):
        if (code == "answer_conflict" and status != "answered") or (
            code == "expired" and status != "expired"
        ):
            raise _failure("respond", done)
        raise AnswerError(receipt, done.status)
    raise _failure("respond", done)


def respond(
    run: str | Path, request_id: str, answer: Any, *, cwd: str | Path | None = None
) -> dict[str, Any]:
    """Submit an answer synchronously."""
    _no_running_loop("respond")
    return asyncio.run(respond_async(run, request_id, answer, cwd=cwd))
