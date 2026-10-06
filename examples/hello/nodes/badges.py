"""The hello example's own nodes: draw one badge, and lay badges out on one sheet (Pillow)."""

from PIL import Image, ImageDraw

from grida.fx import Ctx, node

SIDE = 256  # a badge is drawn on a 256 x 256 transparent square; the workflow scales it down
INSET = 32  # the clear margin around the shape


def _rgba(color: str) -> tuple[int, int, int, int]:
    """``#rrggbb`` as an opaque RGBA colour."""
    return (int(color[1:3], 16), int(color[3:5], 16), int(color[5:7], 16), 255)


@node("badge", params={"shape": str, "color": str}, outputs={"image": "image/png"})
def badge(ctx: Ctx) -> dict:
    fill = _rgba(ctx.params["color"])
    low, high, middle = INSET, SIDE - 1 - INSET, SIDE // 2
    picture = Image.new("RGBA", (SIDE, SIDE), (0, 0, 0, 0))
    pen = ImageDraw.Draw(picture)
    shape = ctx.params["shape"]
    if shape == "circle":
        pen.ellipse((low, low, high, high), fill=fill)
    elif shape == "square":
        pen.rectangle((low, low, high, high), fill=fill)
    elif shape == "diamond":
        pen.polygon([(middle, low), (high, middle), (middle, high), (low, middle)], fill=fill)
    else:
        raise ctx.fail(f"no shape {shape!r}: circle, square or diamond")
    return {"image": ctx.out.png(picture)}


@node(
    "sheet",
    inputs={"badges": "image{}"},
    params={"gap": int},
    outputs={"image": "image/png"},
)
def sheet(ctx: Ctx) -> dict:
    gap = ctx.params["gap"]
    pictures = []
    for file in ctx.inputs["badges"].values():
        with Image.open(file.path) as picture:
            pictures.append(picture.convert("RGBA"))
    if not pictures:
        raise ctx.fail("no badges to lay out")
    width = sum(picture.width for picture in pictures) + gap * (len(pictures) + 1)
    height = max(picture.height for picture in pictures) + 2 * gap
    canvas = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    x = gap
    for picture in pictures:
        canvas.paste(picture, (x, gap))
        x += picture.width + gap
    ctx.fact("count", len(pictures))
    return {"image": ctx.out.png(canvas)}
