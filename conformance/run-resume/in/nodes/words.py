"""Node types of the run-resume case: small, local and deterministic."""

from grida.fx import Ctx, node


@node("shout", params={"text": str}, outputs={"text": "text"}, version=1)
def shout(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.params["text"].upper())}


@node("twice", inputs={"text": "text"}, outputs={"text": "text"}, version=1)
def twice(ctx: Ctx) -> dict:
    text = ctx.read.text("text")
    return {"text": ctx.out.text(f"{text} {text}")}
