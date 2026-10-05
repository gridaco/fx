"""Node types of the run-failures case: one that works, one that fails on purpose, one that
breaks."""

from grida.fx import Ctx, node


@node("make", params={"text": str}, outputs={"text": "text"}, version=1)
def make(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.params["text"])}


@node("refuse", params={"text": str}, outputs={"text": "text"}, version=1)
def refuse(ctx: Ctx) -> dict:
    ctx.fact("seen", ctx.params["text"])
    raise ctx.fail("refused on purpose")


@node("boom", params={"text": str}, outputs={"text": "text"}, version=1)
def boom(ctx: Ctx) -> dict:
    raise RuntimeError("kaboom " + ctx.params["text"])
