"""Wait locally, then return the same input file without rewriting its bytes."""

import time

from grida.fx import Ctx, node


@node(
    "delay",
    inputs={"image": "image/png"},
    params={"seconds": int},
    outputs={"image": "image/png"},
)
def delay(ctx: Ctx) -> dict:
    seconds = ctx.params["seconds"]
    if not 0 <= seconds <= 30:
        raise ctx.fail("seconds must be between 0 and 30")
    time.sleep(seconds)
    ctx.fact("wait_seconds", seconds)
    return {"image": ctx.inputs["image"]}
