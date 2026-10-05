"""Grida FX: author nodes and workflows in Python, and drive the grida-fx engine.

Step 2 of milestone 1 provides the authoring surface the engine reads while planning:

- :func:`node` declares a node type over a body taking ``ctx``; its :class:`NodeSpec` becomes the
  protocol's type spec (``spec/protocol.md`` section 4) when the engine runs ``describe``.
- :class:`Workflow`, :class:`Group` and :class:`StepRef` build a workflow document in code
  (``fx: workflow/v1``), as a builder returns it to the engine's ``build``.
- :class:`Ctx`, :func:`tool`, :class:`ToolReply` and :class:`NodeFailure` are importable so node
  modules load; node bodies run in step 3.

``python -P -m grida.fx.host`` is the node host the engine starts (:mod:`grida.fx.host`).
"""

from grida.fx._builder import Group, StepRef, Workflow
from grida.fx._ctx import Ctx, NodeFailure, ToolReply, tool
from grida.fx._spec import NodeSpec, PortSpec, SpecError, node, param, spec_of

__all__ = [
    "Ctx",
    "Group",
    "NodeFailure",
    "NodeSpec",
    "PortSpec",
    "SpecError",
    "StepRef",
    "ToolReply",
    "Workflow",
    "node",
    "param",
    "spec_of",
    "tool",
]
