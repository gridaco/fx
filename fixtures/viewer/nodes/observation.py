"""A bounded, cancellable wait for real observation tests; no providers or external assets."""

import time
from pathlib import Path

from grida.fx import Ctx, node


@node(
    "viewer_observation_wait",
    inputs={"image": "image"},
    params={"seconds": int, "gate": bool, "fail_after_wait": bool},
    outputs={"image": "image/png"},
    version=1,
)
def wait(ctx: Ctx) -> dict:
    seconds = ctx.params["seconds"]
    if not 0 <= seconds <= 30:
        raise ctx.fail("observation fixture wait must be between zero and thirty seconds")
    gate = ctx.params["gate"]
    release = Path(__file__).resolve().parents[1] / ".observation-release"
    deadline = time.monotonic() + seconds
    ctx.progress("Waiting in the observation fixture", 0)
    while time.monotonic() < deadline:
        if ctx.cancelled:
            raise ctx.fail("Observation fixture cancelled")
        if gate and release.is_file():
            break
        time.sleep(0.025)
    else:
        if gate:
            raise ctx.fail("Observation fixture release gate timed out")
    if ctx.params["fail_after_wait"]:
        raise ctx.fail("Intentional observation fixture failure")
    ctx.fact("wait_seconds", seconds)
    ctx.fact("gated", gate)
    return {"image": ctx.inputs["image"]}
