"""Seam nodes for looping-parallax.

A layer repeats by being drawn again at its own right edge, so its seam is where its right edge
meets its left edge. ``loops_already`` measures that seam; ``layer_repaint`` has a model repaint a
band around it; ``seam_check`` judges the repaint.
"""

from PIL import Image, ImageChops, ImageStat

from grida.fx import Ctx, node

#: The largest step between two columns that still reads as seamless (0 is identical columns,
#: 1 the most different).
ACCEPT_ERROR = 0.02
#: How much of the width the repaint may change: a band this wide, centred on the seam.
BAND = 1 / 8


def step(picture: Image.Image, left: int, right: int) -> float:
    """The mean difference between two columns of a picture, over every channel (alpha
    included), from 0 to 1."""
    rgba = picture.convert("RGBA")
    one = rgba.crop((left, 0, left + 1, rgba.height))
    other = rgba.crop((right, 0, right + 1, rgba.height))
    means = ImageStat.Stat(ImageChops.difference(one, other)).mean
    return sum(means) / (len(means) * 255)


def seam_error(picture: Image.Image) -> float:
    """The step from the right edge column to the left edge column: the seam of a repeat."""
    return step(picture, picture.width - 1, 0)


def changed_pixels(one: Image.Image, other: Image.Image) -> int:
    """How many pixels of two pictures of one size differ, in any channel."""
    channels = ImageChops.difference(one, other).split()
    largest = channels[0]
    for channel in channels[1:]:
        largest = ImageChops.lighter(largest, channel)
    return largest.width * largest.height - largest.histogram()[0]


def reach(width: int) -> int:
    """How far the repainted band reaches on each side of the seam."""
    return max(1, round(width * BAND / 2))


def half(picture: Image.Image) -> int:
    """How far to turn a layer so that its seam lies in the middle."""
    return picture.width // 2


@node("loops_already", inputs={"image": "image"}, outputs={}, judge=True)
def loops_already(ctx: Ctx) -> dict:
    error = seam_error(ctx.read.image("image"))
    ctx.fact("seam_error", round(error, 4))
    ctx.fact("verdict", "accept" if error < ACCEPT_ERROR else "reject")
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
    source = ctx.read.image("image").convert("RGBA")
    turned = ImageChops.offset(source, half(source), 0)  # the seam is now in the middle
    start, end = half(source) - reach(source.width), half(source) + reach(source.width)
    # GPT Image's mask: fully transparent pixels mark what may change (spec/capabilities.md §3).
    mask = Image.new("RGBA", source.size, (0, 0, 0, 255))
    mask.paste((0, 0, 0, 0), (start, 0, end, source.height))
    opaque = ctx.params["opaque"]
    result = await ctx.image_edit(
        prompt=ctx.prompt("prompts/seam.md", opaque=opaque),
        background="opaque" if opaque else "transparent",
        image=ctx.out.png(turned),
        mask=ctx.out.png(mask),
    )
    # The model answers at a size of its own: bring it back, then keep only the band from it, so
    # everything outside the band stays the layer's own pixels.
    painted = result.pil().convert("RGBA").resize(source.size, Image.Resampling.LANCZOS)
    turned.paste(painted.crop((start, 0, end, source.height)), (start, 0))
    return {"image": ctx.out.png(ImageChops.offset(turned, -half(source), 0))}


@node("seam_check", inputs={"image": "image", "source": "image"}, outputs={}, judge=True)
def seam_check(ctx: Ctx) -> dict:
    """Judges a repaint at the three joins it makes: the seam itself, and the two places where the
    painted band, turned back to the edges (``reach`` columns at each), meets the layer's own
    pixels. A join may be no more abrupt than the layer already was there."""
    image = ctx.read.image("image").convert("RGBA")
    source = ctx.read.image("source").convert("RGBA")
    if image.size != source.size:
        raise ctx.fail(f"the repaint is {image.size}, the layer {source.size}")
    width, band = image.width, reach(image.width)
    error = seam_error(image)
    joins = [
        max(0.0, step(image, left, left + 1) - step(source, left, left + 1))
        for left in (band - 1, width - band - 1)
    ]
    kept = (band, 0, width - band, image.height)  # every column the band does not cover
    drift = changed_pixels(image.crop(kept), source.crop(kept))
    ctx.fact("seam_error", round(error, 4))
    ctx.fact("join_errors", [round(join, 4) for join in joins])
    ctx.fact("outside_drift", drift)
    seamless = error < ACCEPT_ERROR and all(join < ACCEPT_ERROR for join in joins)
    ctx.fact("verdict", "accept" if seamless and drift == 0 else "reject")
    return {}
