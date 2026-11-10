"""Keep the master portrait and feature masks in one native coordinate system."""

from __future__ import annotations

import hashlib
import io
from typing import Any

from grida.fx import Ctx, node
from PIL import Image, ImageDraw


def validate_geometry(portrait: bytes, geometry: dict[str, Any]) -> Image.Image:
    with Image.open(io.BytesIO(portrait)) as opened:
        image = opened.convert("RGBA")
    if geometry.get("schema_version") != 1 or geometry.get("source_size") != list(image.size):
        raise ValueError("geometry must describe the original portrait's native size")
    expected = geometry.get("source_sha256")
    if expected and expected != hashlib.sha256(portrait).hexdigest():
        raise ValueError("geometry belongs to a different portrait; author masks for this source")
    box = geometry.get("working_crop_xyxy", [])
    if len(box) != 4 or any(type(v) is not int for v in box):
        raise ValueError("working_crop_xyxy must contain four native integer coordinates")
    x0, y0, x1, y1 = box
    if not (0 <= x0 < x1 <= image.width and 0 <= y0 < y1 <= image.height):
        raise ValueError("working crop must be inside the original portrait")
    if min(x1 - x0, y1 - y0) < 128:
        raise ValueError("working crop must be at least 128 pixels per side")
    for feature in ("canvas_left_eye", "canvas_right_eye", "mouth"):
        points = geometry.get("features", {}).get(feature, {}).get("polygon_xy", [])
        if len(points) < 3 or any(
            len(p) != 2
            or any(type(v) is not int for v in p)
            or not (x0 <= p[0] < x1 and y0 <= p[1] < y1)
            for p in points
        ):
            raise ValueError(f"{feature}: author a polygon inside the working crop")
    return image


@node(
    "prepare",
    inputs={"portrait": "image", "geometry": "json"},
    outputs={"face": "image/png", "mask_overlay": "image/png"},
    version=1,
)
def prepare(ctx: Ctx) -> dict[str, Any]:
    geometry = ctx.read.json("geometry")
    try:
        source = validate_geometry(ctx.read.bytes("portrait"), geometry)
    except ValueError as error:
        raise ctx.fail(str(error)) from error
    crop = source.crop(tuple(geometry["working_crop_xyxy"]))
    matte = Image.new("RGBA", crop.size, (224, 224, 224, 255))
    matte.alpha_composite(crop)
    overlay = matte.copy()
    pen = ImageDraw.Draw(overlay)
    x0, y0, _, _ = geometry["working_crop_xyxy"]
    for feature, color in zip(
        ("canvas_left_eye", "canvas_right_eye", "mouth"),
        ((255, 80, 80), (80, 160, 255), (80, 220, 120)),
        strict=True,
    ):
        points = [(x - x0, y - y0) for x, y in geometry["features"][feature]["polygon_xy"]]
        pen.line([*points, points[0]], fill=color, width=2)
    return {"face": ctx.out.png(matte.convert("RGB")), "mask_overlay": ctx.out.png(overlay)}
