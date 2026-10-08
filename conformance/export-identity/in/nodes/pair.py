"""Two unversioned node types in one module: one source, two exports."""

from grida.fx import Ctx, node


@node("upper", params={"word": str}, outputs={"text": "text"})
def upper(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.params["word"].upper() + "\n")}


@node("lower", params={"word": str}, outputs={"text": "text"})
def lower(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.params["word"].lower() + "\n")}
