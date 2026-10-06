"""Errors a body sees (``spec/protocol.md`` section 7).

- :class:`NodeFailure`: the body fails its node on purpose (``raise ctx.fail(...)``); the host
  answers ``run`` with ``node_failure``, never retried.
- :class:`EngineError` and its subclasses: the engine answered one of the run's requests with an
  error (codes ``-32010`` to ``-32024``, and ``cancelled``). A body may catch them; uncaught, the
  host answers ``run`` with the same code and message, so the engine fails the node with them.
  :func:`engine_error` builds the subclass of a code.
"""

from __future__ import annotations

from typing import Any


class NodeFailure(Exception):
    """A body fails its node on purpose (``node_failure``, not retried)."""


class EngineError(Exception):
    """The engine refused a request of the run: ``code``, ``message``, and ``data`` when given."""

    code: int = -32099

    def __init__(self, message: str, data: Any = None, code: int | None = None) -> None:
        super().__init__(message)
        self.message = message
        self.data = data
        if code is not None:
            self.code = code


class Cancelled(EngineError):
    """The run was stopped (``cancelled``)."""

    code = -32002


class CapabilityError(EngineError):
    """A paid call that could not be made or answered."""


class CapabilityUndeclared(CapabilityError):
    code = -32010


class OverBound(CapabilityError):
    code = -32011


class NoRoute(CapabilityError):
    code = -32012


class NotLive(CapabilityError):
    code = -32013


class CeilingExceeded(CapabilityError):
    code = -32014


class CallRefused(CapabilityError):
    """``capability_refused``: the adapter refused before sending; settled at $0. A stand-in
    raises it to refuse the call it was asked (``spec/protocol.md`` section 5.7)."""

    code = -32015


class CallFailed(CapabilityError):
    """``call_failed``: every attempt failed. A stand-in raises it to fail the call it was
    asked."""

    code = -32016


class JobUnsettled(CapabilityError):
    code = -32017


class AgentUnfinished(EngineError):
    code = -32020


class UndeclaredResource(EngineError):
    code = -32021


class ExpressionError(EngineError):
    code = -32022


class OutsideWorkDir(EngineError):
    code = -32023


class UnknownFile(EngineError):
    code = -32024


#: The subclass of each engine code (``spec/protocol.md`` section 7).
_BY_CODE: dict[int, type[EngineError]] = {
    error.code: error
    for error in (
        Cancelled,
        CapabilityUndeclared,
        OverBound,
        NoRoute,
        NotLive,
        CeilingExceeded,
        CallRefused,
        CallFailed,
        JobUnsettled,
        AgentUnfinished,
        UndeclaredResource,
        ExpressionError,
        OutsideWorkDir,
        UnknownFile,
    )
}


def engine_error(code: int, message: str, data: Any = None) -> EngineError:
    """The :class:`EngineError` subclass of ``code`` (``EngineError`` itself for any other)."""
    kind = _BY_CODE.get(code)
    if kind is None:
        return EngineError(message, data, code)
    return kind(message, data)
