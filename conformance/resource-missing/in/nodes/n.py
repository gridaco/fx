"""Two project node types; the second declares a resource the project does not have."""

from grida.fx import Ctx, node


@node("echo", params={"text": str}, outputs={"text": "text"}, resources=["prompts/r.md"])
def echo(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.prompt("prompts/r.md"))}


@node("lost", params={"text": str}, outputs={"text": "text"}, resources=["prompts/missing.md"])
def lost(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.prompt("prompts/missing.md"))}
