"""A node type of the stand-in-errors case whose body makes a call its capability refuses."""

from grida.fx import Ctx, node


@node("asks_nothing", outputs={"image": "image/png"}, calls={"image.generate": 1}, version=1)
async def asks_nothing(ctx: Ctx) -> dict:
    result = await ctx.capability("image.generate")
    return {"image": result.image}
