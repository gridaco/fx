"""A body that sleeps past its step's timeout without looking at `ctx.cancelled`. It counts its
runs in `naps.txt` in the project folder, the node host's working directory, before sleeping."""

import time
from pathlib import Path

from grida.fx import Ctx, node

NAPS = Path("naps.txt")


@node("slow", params={"seconds": float}, outputs={"text": "text"}, version=1, retry="engine")
def slow(ctx: Ctx) -> dict:
    naps = int(NAPS.read_text()) + 1 if NAPS.exists() else 1
    NAPS.write_text(str(naps))
    time.sleep(ctx.params["seconds"])
    return {"text": ctx.out.text("awake")}
