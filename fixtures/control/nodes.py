"""Provider-free cancellation and resume gates, copied into a fresh test project."""

import time
from pathlib import Path

from grida.fx import Ctx, node


@node("control_checkpoint", outputs={"text": "text"}, version=1)
def checkpoint(ctx: Ctx) -> dict:
    return {"text": ctx.out.text("saved checkpoint")}


@node(
    "control_hold",
    inputs={"text": "text"},
    params={"cancel_delay": int},
    outputs={"text": "text"},
    version=1,
)
def hold(ctx: Ctx) -> dict:
    root = Path(__file__).parent
    (root / ".control-entered").touch()
    deadline = time.monotonic() + 30
    while not (root / ".control-release").is_file():
        if ctx.cancelled:
            ctx.progress("Cancellation received; finishing local cleanup")
            time.sleep(ctx.params["cancel_delay"])
            raise ctx.fail("Control fixture cancelled")
        if time.monotonic() >= deadline:
            raise ctx.fail("Control fixture gate timed out")
        time.sleep(0.025)
    return {"text": ctx.inputs["text"]}


@node("control_after", inputs={"text": "text"}, outputs={"text": "text"}, version=1)
def after(ctx: Ctx) -> dict:
    return {"text": ctx.inputs["text"]}
