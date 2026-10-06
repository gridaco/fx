"""The game's own node type: a ground plate, drawn to tile, from the level's material and light."""

from grida.fx import Ctx, node


@node(
    "ground_plate",
    params={"material": str, "span_m": float, "light": str},
    outputs={"image": "image/png"},
    calls={"image.generate": 1},
    resources=["prompts/plate.md"],
    version=1,
)
async def ground_plate(ctx: Ctx) -> dict:
    result = await ctx.image_generate(
        prompt=ctx.prompt("prompts/plate.md", **ctx.params), size="1024x1024", background="opaque"
    )
    return {"image": result.image}
