"""One unversioned node type, given the same word in two places."""

from grida.fx import Ctx, node


@node("stamp", params={"word": str}, outputs={"text": "text"})
def stamp(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.params["word"].upper() + "\n")}
