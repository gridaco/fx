"""A custom capability deliberately outside FX's standard vocabulary."""

from grida.fx import Ctx, node


@node(
    "native",
    inputs={"source": "file"},
    params={"settings": dict},
    outputs={"result": "json"},
    calls={"acme.native": 1},
    version=1,
)
async def native(ctx: Ctx) -> dict:
    answer = await ctx.capability(
        "acme.native", source=ctx.inputs["source"], **ctx.params["settings"]
    )
    return {"result": ctx.out.json(answer.json)}
