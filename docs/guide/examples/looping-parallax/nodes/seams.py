"""Seam nodes for looping-parallax. Pseudo-code: the seam math is elided."""

from grida.fx import Ctx, node


@node("loops_already", inputs={"image": "image"}, outputs={}, judge=True, version=1)
def loops_already(ctx: Ctx) -> dict:
    err = wrap_seam_error(ctx.read.image("image"))  # compares the right edge with the left
    ctx.fact("seam_error", err)
    ctx.fact("verdict", "accept" if err < 0.02 else "reject")
    return {}


@node(
    "layer_repaint",
    inputs={"image": "image"},
    params={"opaque": bool},
    outputs={"image": "image/png"},
    calls={"image.edit": 1},
    resources=["prompts/seam.md"],
)
async def layer_repaint(ctx: Ctx) -> dict:
    src = ctx.read.image("image")
    shifted, mask = shift_seam_to_centre(src)  # move the wrap seam into the middle
    result = await ctx.image_edit(
        image=ctx.out.png(shifted),
        mask=ctx.out.png(mask),
        prompt=ctx.prompt("prompts/seam.md", opaque=ctx.params["opaque"]),
        background="opaque" if ctx.params["opaque"] else "transparent",
    )
    # Put the repainted band back; everything outside the mask must stay byte-identical.
    return {"image": ctx.out.png(unshift(result.pil(), src, mask))}


@node("seam_check", inputs={"image": "image"}, outputs={}, judge=True, version=1)
def seam_check(ctx: Ctx) -> dict:
    img = ctx.read.image("image")
    err = wrap_seam_error(img)
    drift = outside_mask_drift(img)
    ctx.fact("seam_error", err)
    ctx.fact("outside_drift", drift)
    ctx.fact("verdict", "accept" if err < 0.02 and drift == 0 else "reject")
    return {}
