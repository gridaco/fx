"""Keep a full portrait in its native framing for contextual hidden-surface edits."""

from grida.fx import Ctx, node
from PIL import Image


@node(
    "prepare_full",
    inputs={"portrait": "image"},
    outputs={"portrait": "image/png"},
    version=1,
)
def prepare_full(ctx: Ctx) -> dict:
    source = ctx.read.image("portrait").convert("RGBA")
    matte = Image.new("RGBA", source.size, (224, 224, 224, 255))
    matte.alpha_composite(source)
    return {"portrait": ctx.out.png(matte.convert("RGB"))}
