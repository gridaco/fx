"""Node types of the run-deliver case: small, local and deterministic."""

from grida.fx import Ctx, node


@node("make", params={"text": str}, outputs={"text": "text"}, version=1)
def make(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(f"made {ctx.params['text']}")}
