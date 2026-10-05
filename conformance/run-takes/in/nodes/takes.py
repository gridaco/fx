"""A node type whose text names the take that made it, so takes differ."""

from grida.fx import Ctx, node


@node("make", params={"text": str}, outputs={"text": "text"}, version=1)
def make(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(f"take {ctx.instance.take}: {ctx.params['text']}")}
