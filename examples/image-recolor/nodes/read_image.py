"""Normalize a supplied image to an RGBA PNG through the public FX SDK."""

from PIL import Image

from grida.fx import Ctx, node


@node("read_image", inputs={"image": "image"}, outputs={"image": "image/png"})
def read_image(ctx: Ctx) -> dict:
    with Image.open(ctx.inputs["image"].path) as opened:
        picture = opened.convert("RGBA")
    return {"image": ctx.out.png(picture)}
