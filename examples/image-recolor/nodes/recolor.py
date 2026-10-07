"""Rotate image channels with a deterministic local Pillow operation."""

from PIL import Image

from grida.fx import Ctx, node


@node("recolor", inputs={"image": "image/png"}, outputs={"image": "image/png"})
def recolor(ctx: Ctx) -> dict:
    with Image.open(ctx.inputs["image"].path) as opened:
        red, green, blue, alpha = opened.convert("RGBA").split()
    picture = Image.merge("RGBA", (blue, red, green, alpha))
    return {"image": ctx.out.png(picture)}
