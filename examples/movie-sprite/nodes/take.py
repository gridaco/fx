"""What the take is drawn from: the character on a green plate, and the motion brief.

The plate is what the video model draws from at both ends of the take, so its pixels
are part of every paid request: the character's meaningful silhouette is cropped, scaled
uniformly into the frame with a margin, and composited onto flat green, never repainted.
The brief is the request's prompt: the standing idle template, then the author's general
direction, the motion they ask for and their constraints, in that order.
"""

from __future__ import annotations

import io
from typing import Any

import numpy as np
from PIL import Image

from grida.fx import Ctx, node

#: The frame each aspect ratio is drawn at, before the route scales it.
PLATE_SIZE = {"9:16": (720, 1280), "16:9": (1280, 720)}
#: Said when the author gives no direction at all.
QUIET_IDLE = (
    "A quiet listening idle with barely perceptible local breathing below the fixed "
    "shoulders and gentle settling of loose hair ends or clothing, where present. "
    "Keep the original pose and contact points."
)
_TEXTS = {"type": "array", "items": {"type": "string"}, "default": []}


def fit_on_plate(
    source: bytes, *, aspect_ratio: str, padding: int, alpha_floor: int
) -> tuple[bytes, dict[str, Any]]:
    """Fit the meaningful source alpha inside a green plate without repainting the art."""

    with Image.open(io.BytesIO(source)) as opened:
        if opened.width * opened.height > 32_000_000:
            raise ValueError("the character picture is larger than 32 million pixels")
        rgba = np.array(opened.convert("RGBA"))
    visible = rgba[:, :, 3] > alpha_floor
    if not visible.any():
        raise ValueError("the character picture has no visible subject")
    yy, xx = np.nonzero(visible)
    crop = (int(xx.min()), int(yy.min()), int(xx.max()) + 1, int(yy.max()) + 1)
    rgba[~visible] = 0
    subject = Image.fromarray(rgba).crop(crop)
    width, height = PLATE_SIZE[aspect_ratio]
    scale = min((width - 2 * padding) / subject.width, (height - 2 * padding) / subject.height)
    resized = subject.resize(
        (round(subject.width * scale), round(subject.height * scale)), Image.Resampling.LANCZOS
    )
    offset = ((width - resized.width) // 2, (height - resized.height) // 2)
    plate = Image.new("RGBA", (width, height), (0, 255, 0, 255))
    plate.alpha_composite(resized, offset)
    output = io.BytesIO()
    plate.convert("RGB").save(output, format="PNG")
    return output.getvalue(), {
        "source_size": [rgba.shape[1], rgba.shape[0]],
        "crop_xyxy": list(crop),
        "alpha_floor": alpha_floor,
        "resized_size": list(resized.size),
        "scale_xy": [resized.width / subject.width, resized.height / subject.height],
        "paste_xy": list(offset),
        "plate_size": [width, height],
        "background_rgb": [0, 255, 0],
    }


@node(
    "green_plate",
    inputs={"image": "image"},
    params={
        "aspect_ratio": ("9:16", "16:9"),
        "padding": {"type": "integer", "minimum": 1, "maximum": 100, "default": 32},
        "alpha_floor": {"type": "integer", "minimum": 0, "maximum": 64, "default": 24},
    },
    outputs={"image": "image/png", "report": "json"},
    version=1,
)
def green_plate(ctx: Ctx) -> dict[str, Any]:
    """Fit the character onto a flat green plate the size of the take."""

    try:
        plate, report = fit_on_plate(
            ctx.inputs["image"].read_bytes(),
            aspect_ratio=ctx.params["aspect_ratio"],
            padding=ctx.params["padding"],
            alpha_floor=ctx.params["alpha_floor"],
        )
    except ValueError as error:
        raise ctx.fail(str(error)) from error
    return {"image": ctx.out.bytes(plate, "image/png"), "report": ctx.out.json(report)}


def _section(title: str, lines: list[str]) -> str:
    return f"{title}:\n" + "\n".join(f"- {line}" for line in lines)


@node(
    "idle_brief",
    params={
        "seconds": int,
        "direction": {"type": "string", "default": ""},
        "requested_motion": _TEXTS,
        "constraints": _TEXTS,
    },
    outputs={"prompt": "text/plain"},
    resources=["prompts/idle.md"],
    version=1,
)
def idle_brief(ctx: Ctx) -> dict[str, Any]:
    """Write the take's prompt: the idle template, then the author's own sections."""

    requested = [line for line in ctx.params["requested_motion"] if line.strip()]
    constraints = [line for line in ctx.params["constraints"] if line.strip()]
    direction = ctx.params["direction"]
    if not direction.strip() and not requested and not constraints:
        direction = QUIET_IDLE
    # The template file ends with a newline; the prompt does not.
    sections = [ctx.prompt("prompts/idle.md", seconds=ctx.params["seconds"]).rstrip("\n")]
    if direction.strip():
        sections.append("General direction:\n" + direction)
    if requested:
        sections.append(_section("Requested motion", requested))
    if constraints:
        sections.append(_section("Constraints", constraints))
    return {"prompt": ctx.out.text("\n\n".join(sections))}
