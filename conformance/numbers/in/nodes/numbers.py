"""Node types for the numbers case: a number setting, and text written as given."""

from grida.fx import Ctx, node


@node("hold", params={"n": float}, outputs={"text": "text"}, version=1)
def hold(ctx: Ctx) -> dict:
    return {"text": ctx.out.text("held")}


@node("say", params={"text": str}, outputs={"text": "text"}, version=1)
def say(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.params["text"])}
