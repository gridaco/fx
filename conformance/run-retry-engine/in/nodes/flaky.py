"""A body that fails twice, then succeeds. It counts its runs in `count.txt` in the project
folder, which is the node host's working directory."""

from pathlib import Path

from grida.fx import Ctx, node

COUNTER = Path("count.txt")


@node("flaky", params={"text": str}, outputs={"text": "text"}, version=1, retry="engine")
def flaky(ctx: Ctx) -> dict:
    runs = int(COUNTER.read_text()) + 1 if COUNTER.exists() else 1
    COUNTER.write_text(str(runs))
    if runs < 3:
        raise RuntimeError(f"run {runs} fails")
    return {"text": ctx.out.text(f"{ctx.params['text']} after {runs} runs")}
