"""Free value-card fixtures; Pillow draws every image without external resources."""

from PIL import Image, ImageDraw, ImageOps

from grida.fx import Ctx, node, param


def reference_picture() -> Image.Image:
    """The same original image is used as an optional input and a local fallback."""
    picture = Image.new("RGB", (240, 180), "#f4e9cd")
    pen = ImageDraw.Draw(picture)
    pen.rounded_rectangle((30, 24, 210, 156), radius=24, fill="#354b70")
    pen.ellipse((48, 44, 116, 112), fill="#f2a65a")
    pen.polygon([(152, 44), (194, 126), (110, 126)], fill="#74c4b4")
    return picture


@node(
    "viewer_value_reference",
    inputs={"reference": "image?"},
    params={
        "prompt": param(
            str,
            description=(
                "The effective image brief, including the shared shape. "
                "This fixture draws locally without calling a model."
            ),
        ),
        "style": param(
            dict,
            description="Shared shape, seed, palette and transparency settings for this image.",
        ),
        "size": param(int, default=256, minimum=64, maximum=512),
        "background": param(str, default="warm paper"),
    },
    outputs={"image": "image/png", "summary": "json"},
    version=1,
)
def reference(ctx: Ctx) -> dict:
    picture = (
        ctx.read.image("reference").convert("RGB")
        if "reference" in ctx.inputs
        else reference_picture()
    )
    picture.thumbnail((ctx.params["size"], ctx.params["size"]))
    # Keep the result distinct from the input: input previews must stand on their own.
    picture = ImageOps.mirror(picture)
    return {
        "image": ctx.out.png(picture),
        "summary": ctx.out.json(
            {
                "prompt": ctx.params["prompt"],
                "style": ctx.params["style"],
                "size": ctx.params["size"],
                "background": ctx.params["background"],
                "optional": None,
                "file_shaped_json": {"file": "ordinary text, not a media reference"},
            }
        ),
    }


@node(
    "viewer_value_description",
    params={"prompt": str, "label": param(str, default="Local variation")},
    outputs={"text": "text/plain"},
    version=1,
)
def describe(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(f"{ctx.params['label']}\n{ctx.params['prompt']}\n")}
