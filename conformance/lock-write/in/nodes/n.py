"""Two project node types: one named by its version, one by its source."""

from grida.fx import Ctx, node

from nodes import helper


@node("echo", params={"text": str}, outputs={"text": "text"}, resources=["prompts/r.md"])
def echo(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(helper.frame(ctx.prompt("prompts/r.md")))}


@node("pinned", params={"text": str}, outputs={"text": "text"}, version=2)
def pinned(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(helper.frame(ctx.params["text"]))}
