"""Grida FX: author nodes and workflows in Python, and drive the grida-fx engine.

- :func:`node` declares a node type over a body taking ``ctx``; its :class:`NodeSpec` becomes the
  protocol's type spec (``spec/protocol.md`` section 4) when the engine runs ``describe``.
- :class:`Workflow`, :class:`Group` and :class:`StepRef` build a workflow document in code
  (``fx: workflow/v1``), as a builder returns it to the engine's ``build``.
- :class:`Ctx` is what a body receives when the engine runs it (section 8); :func:`tool`,
  :class:`ToolReply`, :class:`Tool`, :class:`ToolResult` and :class:`ToolInvocationError` declare
  an agent's tools; :class:`NodeFailure` fails a node on purpose; the engine's refusals reach a
  body as :class:`EngineError` subclasses.
- :func:`plan`, :func:`plan_async`, :func:`run` and :func:`run_async` drive the ``grida-fx``
  binary (:mod:`grida.fx._api`); a :class:`RunResult` names each failed instance's
  :class:`Failure`.
- ``run(..., stand_in=answer)`` answers a run's paid calls offline with a function of one
  :class:`StandInCall` (spec/protocol.md section 5.7), which returns an :class:`Answer` or
  :data:`DECLINE`, or raises :class:`CallRefused` or :class:`CallFailed`
  (:mod:`grida.fx._stand_in`).
- :mod:`grida.fx.std` holds the Python bodies of the standard node types.

``python -P -m grida.fx.host`` is the node host the engine starts (:mod:`grida.fx.host`);
``python -m grida.fx <verb>`` runs the engine.
"""

from grida.fx._agent import Agent, Tool, ToolInvocationError, ToolReply, ToolResult, tool
from grida.fx._api import (
    Failure,
    FxError,
    Plan,
    PlanRefused,
    RunResult,
    plan,
    plan_async,
    run,
    run_async,
)
from grida.fx._builder import Group, StepRef, Workflow
from grida.fx._control import (
    RunControlError,
    RunControlResult,
    cancel,
    cancel_async,
    inspect_control,
    inspect_control_async,
)
from grida.fx._ctx import CallResult, Ctx, InputFile, Output
from grida.fx._errors import (
    CallFailed,
    CallRefused,
    CapabilityError,
    CeilingExceeded,
    EngineError,
    NodeFailure,
)
from grida.fx._record import RunObservationError, RunRecord, load_run, load_run_async
from grida.fx._spec import NodeSpec, PortSpec, SpecError, node, param, spec_of
from grida.fx._stand_in import DECLINE, Answer, StandInCall

__all__ = [
    "DECLINE",
    "Agent",
    "Answer",
    "CallFailed",
    "CallRefused",
    "CallResult",
    "CapabilityError",
    "CeilingExceeded",
    "Ctx",
    "EngineError",
    "Failure",
    "FxError",
    "Group",
    "InputFile",
    "NodeFailure",
    "NodeSpec",
    "Output",
    "Plan",
    "PlanRefused",
    "PortSpec",
    "RunResult",
    "RunRecord",
    "RunObservationError",
    "RunControlError",
    "RunControlResult",
    "SpecError",
    "StandInCall",
    "StepRef",
    "Tool",
    "ToolInvocationError",
    "ToolReply",
    "ToolResult",
    "Workflow",
    "cancel",
    "cancel_async",
    "inspect_control",
    "inspect_control_async",
    "load_run",
    "load_run_async",
    "node",
    "param",
    "plan",
    "plan_async",
    "run",
    "run_async",
    "spec_of",
    "tool",
]
