"""Deterministic face working crops and source-preserving feature restoration.

Coordinates describe pixel edges: a source pixel at index (x, y) has its center
at (x + 0.5, y + 0.5), and xyxy boxes exclude their right and bottom edges.
The caller locates the face. These helpers impose no style or quality judgment.

The generated donor must be the registered, uncomposited RGB cell. Passing an
already feathered composite would apply its boundary blending a second time.
Visible source pixels may receive donor color; original alpha remains intact.
The working donor is neutral-matted, so its RGB is unmatted with original alpha
before blending. Fully transparent pixels retain their original hidden RGB.
"""

from __future__ import annotations

import math
from collections.abc import Sequence
from typing import Any

import numpy as np
from numpy.typing import NDArray
from PIL import Image
from scipy.ndimage import map_coordinates

FloatArray = NDArray[np.float64]
Transform = dict[str, Any]
_VERSION = "face-crop-transform-v1"
_COORDINATES = "xyxy_pixel_edges_exclusive"
_NEUTRAL = (240, 240, 240)


def _number(value: object, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{label} must be a finite number")
    if not math.isfinite(value):
        raise ValueError(f"{label} must be a finite number")
    return float(value)


def _size(value: object, label: str, *, square: bool = False) -> tuple[int, int]:
    if not isinstance(value, (list, tuple)) or len(value) != 2:
        raise ValueError(f"{label} must contain width and height")
    if any(isinstance(part, bool) or not isinstance(part, int) or part < 1 for part in value):
        raise ValueError(f"{label} dimensions must be positive integers")
    width, height = value
    if square and width != height:
        raise ValueError(f"{label} must be square to retain uniform scale")
    return width, height


def _source(source: Image.Image) -> None:
    if source.mode not in {"RGB", "RGBA"}:
        raise ValueError("source must use RGB or RGBA pixels")
    _size(source.size, "source_size")


def _make_transform(
    source_size: tuple[int, int],
    bbox: Sequence[float],
    work_size: tuple[int, int],
    padding_fraction: float,
) -> Transform:
    width, height = _size(source_size, "source_size")
    work_width, work_height = _size(work_size, "work_size", square=True)
    if not isinstance(bbox, (list, tuple)) or len(bbox) != 4:
        raise ValueError("face bbox must contain four source-pixel xyxy coordinates")
    x0, y0, x1, y1 = (_number(part, "face bbox coordinate") for part in bbox)
    if not (0 <= x0 < x1 <= width and 0 <= y0 < y1 <= height):
        raise ValueError("face bbox must have positive area and lie within the source canvas")
    padding = _number(padding_fraction, "padding_fraction")
    if padding < 0:
        raise ValueError("padding_fraction must be nonnegative")
    span = max(x1 - x0, y1 - y0) * (1 + 2 * padding)
    if not math.isfinite(span):
        raise ValueError("padded crop size must be finite")
    side = math.ceil(span)
    left = math.floor((x0 + x1 - side) / 2)
    top = math.floor((y0 + y1 - side) / 2)
    # Rounding the center down must not clip a fractional face bound when the
    # requested context is zero. Expand the square by one pixel if needed.
    if left + side < x1 or top + side < y1:
        side += 1
    right, bottom = left + side, top + side
    return {
        "version": _VERSION,
        "coordinate_convention": _COORDINATES,
        "source_size": [width, height],
        "face_box_xyxy": [x0, y0, x1, y1],
        "crop_box_xyxy": [left, top, right, bottom],
        "work_size": [work_width, work_height],
        "source_to_work_scale": work_width / side,
        "padding_fraction": padding,
        "padding_ltrb": [
            max(0, -left),
            max(0, -top),
            max(0, right - width),
            max(0, bottom - height),
        ],
        "neutral_rgb": list(_NEUTRAL),
    }


def _validated_transform(source: Image.Image, transform: Transform) -> Transform:
    if not isinstance(transform, dict):
        raise ValueError("transform must be a mapping")
    required = {
        "version",
        "coordinate_convention",
        "source_size",
        "face_box_xyxy",
        "crop_box_xyxy",
        "work_size",
        "source_to_work_scale",
        "padding_fraction",
        "padding_ltrb",
        "neutral_rgb",
    }
    if set(transform) != required:
        raise ValueError("transform fields do not match the face-crop contract")
    if _size(transform["source_size"], "transform source_size") != source.size:
        raise ValueError("transform source_size does not match the original image")
    expected = _make_transform(
        source.size,
        transform["face_box_xyxy"],
        _size(transform["work_size"], "transform work_size", square=True),
        transform["padding_fraction"],
    )
    if transform != expected:
        raise ValueError("transform mapping is inconsistent with its source and face bbox")
    return expected


def create_working_crop(
    source: Image.Image,
    bbox: Sequence[float],
    work_size: tuple[int, int] = (1024, 1024),
    padding_fraction: float = 0.35,
) -> tuple[Image.Image, Transform]:
    """Flatten locally, pad around the face, and resize once without distortion.

    Padding adds 35 percent of the longest face dimension on each side by
    default. A square crop may extend beyond the source; those pixels use the
    same neutral background as source transparency. The source is never edited.
    The returned transform is portable JSON data and uses source-to-work scale.
    """
    _source(source)
    transform = _make_transform(source.size, bbox, work_size, padding_fraction)
    left, top, right, bottom = transform["crop_box_xyxy"]
    local_box = (max(0, left), max(0, top), min(source.width, right), min(source.height, bottom))
    local = source.crop(local_box).convert("RGBA")
    neutral = Image.new("RGBA", local.size, (*_NEUTRAL, 255))
    flattened = Image.alpha_composite(neutral, local).convert("RGB")
    square = Image.new("RGB", (right - left, bottom - top), _NEUTRAL)
    square.paste(flattened, (local_box[0] - left, local_box[1] - top))
    return square.resize(work_size, Image.Resampling.LANCZOS), transform


def restore_feature(
    source: Image.Image,
    donor: Image.Image,
    mask: FloatArray,
    transform: Transform,
) -> tuple[Image.Image, FloatArray]:
    """Inverse-map one raw registered donor and work-sized alpha exactly once.

    The donor may be at cell or working-image resolution; it represents the
    same complete square crop as the work-sized mask. Donor and mask are each
    sampled directly at original pixel centers with bilinear interpolation.
    There is no intermediate donor upscaling or whole-face replacement. An
    all-zero mask is an exact rest frame, including hidden transparent RGB.
    """
    _source(source)
    mapping = _validated_transform(source, transform)
    if donor.mode != "RGB":
        raise ValueError("donor must be an opaque RGB image")
    _size(donor.size, "donor_size", square=True)
    work_width, work_height = mapping["work_size"]
    try:
        supplied_mask = np.asarray(mask)
        if supplied_mask.dtype.kind not in "biuf":
            raise ValueError("mask must contain real numeric alpha")
        alpha = np.asarray(supplied_mask, dtype=np.float64)
    except (TypeError, ValueError) as error:
        raise ValueError("mask must contain finite numeric alpha in [0, 1]") from error
    if alpha.shape != (work_height, work_width):
        raise ValueError("mask must have the exact work_size shape")
    if not np.isfinite(alpha).all() or np.any(alpha < 0) or np.any(alpha > 1):
        raise ValueError("mask must contain finite numeric alpha in [0, 1]")
    source_mask = np.zeros((source.height, source.width), dtype=np.float64)
    if not np.any(alpha):
        return source.copy(), source_mask

    left, top, right, bottom = mapping["crop_box_xyxy"]
    side = right - left
    x0, y0 = max(0, left), max(0, top)
    x1, y1 = min(source.width, right), min(source.height, bottom)
    yy, xx = np.mgrid[y0:y1, x0:x1].astype(np.float64)

    def coordinates(size: tuple[int, int]) -> FloatArray:
        return np.asarray(
            [
                (yy + 0.5 - top) * size[1] / side - 0.5,
                (xx + 0.5 - left) * size[0] / side - 0.5,
            ]
        )

    sampled_mask = np.clip(
        map_coordinates(
            alpha, coordinates((work_width, work_height)), order=1, mode="nearest", prefilter=False
        ),
        0,
        1,
    )
    original = np.asarray(source.convert("RGBA"))
    sampled_mask[original[y0:y1, x0:x1, 3] == 0] = 0
    source_mask[y0:y1, x0:x1] = sampled_mask
    if not np.any(sampled_mask):
        return source.copy(), source_mask

    donor_array = np.asarray(donor, dtype=np.float64)
    donor_coordinates = coordinates(donor.size)
    sampled_donor = np.stack(
        [
            map_coordinates(
                donor_array[..., channel],
                donor_coordinates,
                order=1,
                mode="nearest",
                prefilter=False,
            )
            for channel in range(3)
        ],
        axis=2,
    )
    source_alpha = original[y0:y1, x0:x1, 3:4].astype(np.float64) / 255
    sampled_donor = np.clip(
        np.divide(
            sampled_donor - np.asarray(mapping["neutral_rgb"]) * (1 - source_alpha),
            source_alpha,
            out=np.zeros_like(sampled_donor),
            where=source_alpha > 0,
        ),
        0,
        255,
    )
    output = original.copy()
    alpha3 = sampled_mask[..., None]
    blended = np.rint(original[y0:y1, x0:x1, :3] * (1 - alpha3) + sampled_donor * alpha3).astype(
        np.uint8
    )
    changed = sampled_mask > 0
    output_region = output[y0:y1, x0:x1, :3]
    output_region[changed] = blended[changed]
    result = Image.fromarray(output)
    return (result if source.mode == "RGBA" else result.convert("RGB")), source_mask
