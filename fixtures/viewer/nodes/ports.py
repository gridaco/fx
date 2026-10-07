"""Free node bodies that make port identity observable without guessing from file bytes."""

from PIL import Image

from grida.fx import Ctx, node


@node(
    "viewer_split_ports",
    inputs={"image": "image"},
    outputs={"color": "image/png", "copy": "image/png", "mask": "image/png", "report": "json"},
    version=1,
)
def split(ctx: Ctx) -> dict:
    picture = ctx.read.image("image").convert("RGBA")
    ctx.fact("pixel_count", picture.width * picture.height)
    return {
        # Two distinct declared ports deliberately carry the same content and file reference.
        "color": ctx.inputs["image"],
        "copy": ctx.inputs["image"],
        "mask": ctx.out.png(picture.getchannel("A")),
        "report": ctx.out.json({"width": picture.width, "height": picture.height}),
    }


@node(
    "viewer_merge_ports",
    inputs={"image": "image", "mask": "image"},
    outputs={"image": "image/png"},
    version=1,
)
def merge(ctx: Ctx) -> dict:
    picture = ctx.read.image("image").convert("RGBA")
    mask = ctx.read.image("mask").convert("L")
    picture.putalpha(mask)
    return {"image": ctx.out.png(picture)}


@node(
    "viewer_compare_ports",
    inputs={"left": "image", "right": "image"},
    outputs={"report": "json"},
    version=1,
)
def compare(ctx: Ctx) -> dict:
    return {
        "report": ctx.out.json(
            {
                "same_digest": ctx.inputs["left"].digest == ctx.inputs["right"].digest,
                "same_bytes": ctx.read.bytes("left") == ctx.read.bytes("right"),
            }
        )
    }


@node(
    "viewer_gather_ports",
    inputs={"images": "image{}"},
    outputs={"image": "image/png", "keys": "json"},
    version=1,
)
def gather(ctx: Ctx) -> dict:
    pictures = []
    for file in ctx.inputs["images"].values():
        with Image.open(file.path) as opened:
            pictures.append(opened.convert("RGBA"))
    canvas = Image.new(
        "RGBA",
        (sum(picture.width for picture in pictures), max(picture.height for picture in pictures)),
        (0, 0, 0, 0),
    )
    left = 0
    for picture in pictures:
        canvas.paste(picture, (left, 0))
        left += picture.width
    return {"image": ctx.out.png(canvas), "keys": ctx.out.json(list(ctx.inputs["images"]))}


@node(
    "viewer_settings_ports",
    params={"width": int, "label": str, "pixels": int, "file_width": int, "literal": str},
    outputs={"report": "json"},
    version=1,
)
def settings(ctx: Ctx) -> dict:
    return {"report": ctx.out.json(dict(ctx.params))}
