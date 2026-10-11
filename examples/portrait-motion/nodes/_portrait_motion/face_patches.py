"""Keep repaint reconstruction local; cross the sprite boundary with offset only.

The contained face operation receives a native-resolution RGBA crop and returns
native-resolution patches whose edge blending is already baked into RGB. Their
binary alpha marks replacement support, so pasting onto the original must never
feather or resize again. Only the integer crop offset crosses that boundary.
"""

from __future__ import annotations

import math
from collections.abc import Sequence
from decimal import Decimal

import numpy as np
from PIL import Image

from .face_crop import FloatArray, Transform, _source, _validated_transform


def make_face_input(source: Image.Image, transform: Transform) -> tuple[Image.Image, Transform]:
    """Return the exact native crop and its equivalent face-local transform.

    Source pixels are copied unchanged, including hidden RGB and partial alpha.
    Parts outside the original canvas are transparent padding. The returned
    transform describes only this crop: its top-left coordinate is always zero.
    """
    _source(source)
    mapping = _validated_transform(source, transform)
    left, top, right, bottom = mapping["crop_box_xyxy"]
    width, height = right - left, bottom - top
    local = source.convert("RGBA").crop((left, top, right, bottom))
    x0, y0, x1, y1 = mapping["face_box_xyxy"]
    local_mapping = {
        **mapping,
        "source_size": [width, height],
        "face_box_xyxy": [x0 - left, y0 - top, x1 - left, y1 - top],
        "crop_box_xyxy": [0, 0, width, height],
        "padding_ltrb": [0, 0, 0, 0],
    }
    try:
        return local, _validated_transform(local, local_mapping)
    except ValueError as error:
        # Subtracting an integer offset in binary floating point can move a
        # mathematically integral center just below zero before floor(). Retry
        # the same decimal-coordinate subtraction without that cancellation.
        # Only bbox metadata is normalized, by at most four source-coordinate
        # ULPs. The native crop, offset, scale and reconstruction geometry stay
        # exactly as declared by the validated original mapping.
        shifted = [
            float(Decimal(str(value)) - Decimal(offset))
            for value, offset in zip((x0, y0, x1, y1), (left, top, left, top), strict=True)
        ]
        tolerance = 4 * math.ulp(max(1.0, width, height, abs(x0), abs(y0), abs(x1), abs(y1)))
        if any(
            abs(before - after) > tolerance
            for before, after in zip(local_mapping["face_box_xyxy"], shifted, strict=True)
        ):
            raise ValueError(
                "local face-coordinate normalization exceeds rounding tolerance"
            ) from error
        local_mapping["face_box_xyxy"] = shifted
        return local, _validated_transform(local, local_mapping)


def isolate_patch(local_composed: Image.Image, local_mask: FloatArray) -> Image.Image:
    """Isolate already-blended native pixels with binary replacement alpha."""
    _source(local_composed)
    try:
        supplied_mask = np.asarray(local_mask)
        if supplied_mask.dtype.kind not in "biuf":
            raise ValueError("mask must contain real numeric alpha")
        alpha = np.asarray(supplied_mask, dtype=np.float64)
    except (TypeError, ValueError) as error:
        raise ValueError("mask must contain finite numeric alpha in [0, 1]") from error
    if alpha.shape != (local_composed.height, local_composed.width):
        raise ValueError("mask must have the exact native crop shape")
    if not np.isfinite(alpha).all() or np.any(alpha < 0) or np.any(alpha > 1):
        raise ValueError("mask must contain finite numeric alpha in [0, 1]")
    pixels = np.asarray(local_composed.convert("RGBA"))
    support = alpha > 0
    if np.any(support & (pixels[..., 3] == 0)):
        raise ValueError("patch support must contain only visible original pixels")
    patch = np.zeros(pixels.shape, dtype=np.uint8)
    patch[support, :3] = pixels[support, :3]
    patch[support, 3] = 255
    return Image.fromarray(patch)


def apply_offset_patch(
    original: Image.Image, patch: Image.Image, offset: Sequence[int]
) -> Image.Image:
    """Replace supported original pixels at an integer offset, with no scaling.

    Clipping at all canvas edges is deterministic. Original alpha is immutable;
    a patch that attempts to paint fully transparent source pixels is refused.
    Empty or wholly clipped patches are exact rest frames.
    """
    _source(original)
    if patch.mode != "RGBA":
        raise ValueError("patch must use RGBA pixels with binary alpha")
    if not isinstance(offset, (list, tuple)) or len(offset) != 2:
        raise ValueError("offset must contain two integers")
    if any(isinstance(value, bool) or not isinstance(value, int) for value in offset):
        raise ValueError("offset must contain two integers")
    left, top = offset
    patch_pixels = np.asarray(patch)
    if not np.isin(patch_pixels[..., 3], [0, 255]).all():
        raise ValueError("patch must use binary alpha; feathering is already baked into RGB")
    x0, y0 = max(0, left), max(0, top)
    x1, y1 = min(original.width, left + patch.width), min(original.height, top + patch.height)
    if x0 >= x1 or y0 >= y1:
        return original.copy()
    clipped = patch_pixels[y0 - top : y1 - top, x0 - left : x1 - left]
    support = clipped[..., 3] == 255
    output = np.array(original.convert("RGBA"))
    region = output[y0:y1, x0:x1]
    if np.any(support & (region[..., 3] == 0)):
        raise ValueError("patch support must contain only visible original pixels")
    region[support, :3] = clipped[support, :3]
    result = Image.fromarray(output)
    return result if original.mode == "RGBA" else result.convert("RGB")
