"""Deterministic geometric images and a local judge for the viewer's takes fixture.

Pillow draws every pixel here; no external or generated artwork is used. Take one
places an orange circle away from the center. Later takes center a green circle.
"""

from PIL import Image, ImageDraw

from grida.fx import Ctx, node


@node("viewer_take_render", outputs={"image": "image/png"}, version=1)
def render(ctx: Ctx) -> dict:
    """Draw visibly different candidates using the engine's public take number."""
    take = ctx.instance.take
    picture = Image.new("RGB", (320, 200), (245, 243, 236))
    draw = ImageDraw.Draw(picture)
    draw.rounded_rectangle((12, 12, 307, 187), radius=16, outline=(192, 190, 181), width=2)
    center_x = 72 if take == 1 else 160
    color = (228, 127, 56) if take == 1 else (47, 151, 119)
    draw.ellipse((center_x - 42, 58, center_x + 42, 142), fill=color)
    for index in range(take):
        left = 140 + index * 16
        draw.rectangle((left, 160, left + 8, 168), fill=(72, 76, 83))
    ctx.fact("take", take)
    return {"image": ctx.out.png(picture)}


@node(
    "viewer_take_review",
    inputs={"image": "image/png"},
    outputs={"report": "json"},
    judge=True,
    version=1,
)
def review(ctx: Ctx) -> dict:
    """Accept a candidate only when its center contains the expected green circle."""
    picture = ctx.read.image("image").convert("RGB")
    accepted = picture.size == (320, 200) and picture.getpixel((160, 100)) == (47, 151, 119)
    verdict = "accept" if accepted else "reject"
    ctx.fact("verdict", verdict)
    ctx.fact("score", 1 if accepted else 0)
    return {
        "report": ctx.out.json(
            {
                "take": ctx.instance.take,
                "verdict": verdict,
                "check": "A green circle covers the image center.",
                "accepted": accepted,
            }
        )
    }


@node(
    "viewer_take_finish",
    inputs={"image": "image/png", "report": "json"},
    outputs={"image": "image/png", "report": "json"},
    version=1,
)
def finish(ctx: Ctx) -> dict:
    """Keep the selected image and its judge's report as the workflow outputs."""
    report = ctx.read.json("report")
    if report["verdict"] != "accept":
        raise ctx.fail("The downstream step requires an accepted candidate.")
    ctx.fact("selected_take", report["take"])
    return {
        "image": ctx.out.bytes(ctx.read.bytes("image"), "image/png"),
        "report": ctx.out.json(report),
    }
