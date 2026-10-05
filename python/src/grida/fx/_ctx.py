"""The node body surface (``spec/protocol.md`` section 8): importable placeholders in step 2.

Node modules import ``Ctx``, ``tool``, ``ToolReply`` and ``NodeFailure`` at module level, so they
must exist for ``describe`` to load a module. Bodies run in step 3, when these are rehosted over
the protocol; until then a body that touches ``Ctx`` raises ``NotImplementedError``.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any


class NodeFailure(Exception):
    """A body fails its node on purpose (``node_failure``, not retried)."""


class Ctx:
    """What a body receives (step 3)."""

    def __getattr__(self, name: str) -> Any:
        raise NotImplementedError(f"Ctx.{name} runs with the runner (step 3)")


@dataclass
class ToolReply:
    """A tool's answer to the agent: content, and pictures."""

    content: Any = None
    images: list[Any] = field(default_factory=list)


def tool(function: Callable[..., Any] | None = None, *, name: str | None = None) -> Any:
    """Declare an agent tool over ``function(ctx, **arguments)`` (step 3 serves it)."""

    def wrap(inner: Callable[..., Any]) -> Callable[..., Any]:
        inner.fx_tool = {"name": name or inner.__name__}  # type: ignore[attr-defined]
        return inner

    return wrap(function) if function is not None else wrap
