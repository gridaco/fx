"""Register full-face repaints and extract native, source-preserving replacement patches.

Adapted from this repository's portrait-motion example: processing.py's bounded
similarity registration and face_patches.py's already-blended RGB/binary-alpha
replacement contract. Kept here so this example works when copied on its own.
No helper locates features, changes their relative scale, or grants artistic approval.
"""

from __future__ import annotations

import math
from collections.abc import Sequence

import numpy as np
from PIL import Image, ImageDraw
from scipy.ndimage import (
    binary_dilation,
    distance_transform_edt,
    gaussian_filter,
    map_coordinates,
    sobel,
)
from scipy.optimize import least_squares

NEUTRAL = (224, 224, 224)


def _source(image: Image.Image) -> np.ndarray:
    if image.mode != "RGBA":
        raise ValueError("the source must use native RGBA pixels")
    return np.asarray(image, dtype=np.uint8)


def _mask(image: Image.Image, size: tuple[int, int]) -> np.ndarray:
    if image.mode != "L" or image.size != size:
        raise ValueError("an authored mask must be L mode at the source crop size")
    return np.asarray(image, dtype=np.uint8)


def _flatten(image: Image.Image) -> Image.Image:
    rgba = image.convert("RGBA")
    return Image.alpha_composite(Image.new("RGBA", image.size, (*NEUTRAL, 255)), rgba).convert(
        "RGB"
    )


def polygon_mask(size: tuple[int, int], polygons: Sequence) -> Image.Image:
    """Rasterize caller-authored source-local polygons without inferring anatomy.

    Accept either one list of [x,y] points or a list of such polygons. Coordinates
    are native pixel indices; every point must lie inside the supplied canvas.
    """
    if len(size) != 2 or any(isinstance(v, bool) or not isinstance(v, int) or v < 1 for v in size):
        raise ValueError("mask size must contain two positive integers")
    if not isinstance(polygons, (list, tuple)) or not polygons:
        raise ValueError("at least one authored polygon is required")
    if not isinstance(polygons[0], (list, tuple)) or not polygons[0]:
        raise ValueError("an authored polygon must contain coordinate points")
    if isinstance(polygons[0][0], (int, float)):
        polygons = [polygons]
    image = Image.new("L", size)
    draw = ImageDraw.Draw(image)
    for polygon in polygons:
        if not 3 <= len(polygon) <= 128:
            raise ValueError("an authored polygon needs 3 to 128 points")
        points = []
        for point in polygon:
            if len(point) != 2 or any(
                isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v)
                for v in point
            ):
                raise ValueError("polygon points must contain finite x and y numbers")
            x, y = point
            if not (0 <= x < size[0] and 0 <= y < size[1]):
                raise ValueError("an authored polygon leaves the source crop")
            points.append((float(x), float(y)))
        if len(set(points)) != len(points):
            raise ValueError("an authored polygon has duplicate points")
        edges = list(zip(points, points[1:] + points[:1], strict=True))
        if abs(sum(a[0] * b[1] - b[0] * a[1] for a, b in edges)) < 2:
            raise ValueError("an authored polygon has no usable area")
        draw.polygon(points, fill=255)
    return image


def _texture(rgb: np.ndarray) -> np.ndarray:
    gray = gaussian_filter(rgb.mean(axis=2), 0.8)
    return np.hypot(sobel(gray, axis=0), sobel(gray, axis=1)) / 8


def _coordinates(shape, parameters, ys, xs):
    height, width = shape
    cx, cy = (width - 1) / 2, (height - 1) / 2
    tx, ty, degrees, scale = parameters
    cosine, sine = np.cos(np.deg2rad(degrees)), np.sin(np.deg2rad(degrees))
    dx, dy = xs - cx - tx, ys - cy - ty
    return np.array(
        [(-sine * dx + cosine * dy) / scale + cy, (cosine * dx + sine * dy) / scale + cx]
    )


def _warp(rgb: np.ndarray, parameters: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    height, width = rgb.shape[:2]
    ys, xs = np.indices((height, width), dtype=np.float64)
    yy, xx = _coordinates((height, width), parameters, ys, xs)
    valid = (yy >= 0) & (xx >= 0) & (yy <= height - 1) & (xx <= width - 1)
    aligned = np.stack(
        [map_coordinates(rgb[..., c], [yy, xx], order=1, mode="nearest") for c in range(3)],
        axis=2,
    )
    return aligned, valid


def register_donor(
    source_crop: Image.Image,
    donor: Image.Image,
    mutable_mask: Image.Image,
    *,
    required_support: Image.Image | None = None,
) -> tuple[Image.Image, dict]:
    """Normalize the complete donor once, then register using immutable source texture.

    Similarity fitting is bounded to 2% translation (at most 24 native pixels),
    2 degrees and 3% scale. A failed/ambiguous fit raises ValueError. The donor is
    never cut into independently fitted eyes or mouths. Pixel errors outside the
    authored mutable region diagnose drift, and never authorize new patch support.
    A separate required support may restrict coverage checks to pixels actually
    imported, while larger changed regions stay excluded from registration.
    """
    _source(source_crop)
    mutable = _mask(mutable_mask, source_crop.size) > 0
    if not mutable.any():
        raise ValueError("registration requires an authored mutable region")
    required = (
        mutable if required_support is None else _mask(required_support, source_crop.size) > 0
    )
    if not required.any() or np.any(required & ~mutable):
        raise ValueError("required support must be nonempty and inside the mutable region")
    if donor.mode not in ("RGB", "RGBA"):
        raise ValueError("the complete donor must use RGB or RGBA pixels")
    original_size = donor.size
    normalized = donor
    if donor.size != source_crop.size:
        normalized = donor.resize(source_crop.size, Image.Resampling.LANCZOS)
    reference = np.asarray(_flatten(source_crop), dtype=np.float64)
    current = np.asarray(_flatten(normalized), dtype=np.float64)
    height, width = reference.shape[:2]
    thresholds = {
        "translation_limit_px": min(24.0, min(width, height) * 0.02),
        "rotation_limit_degrees": 2.0,
        "scale_limits": [0.97, 1.03],
        "maximum_trimmed_texture_mae": 14.0,
        "maximum_texture_p90": 48.0,
        "maximum_transform_disagreement_px": 1.5,
    }
    base_report = {
        "method": "bounded_similarity_fit_on_immutable_source_texture",
        "source_crop_size": list(source_crop.size),
        "donor_size": list(original_size),
        "whole_canvas_normalized": original_size != source_crop.size,
        "thresholds": thresholds,
        "semantic_acceptance": "not_granted",
        "coverage_scope": "mutable_region" if required_support is None else "required_support",
    }
    if np.array_equal(reference, current):
        return Image.fromarray(current.astype(np.uint8)), {
            **base_report,
            "status": "passed_technical_gate",
            "translation_xy_px": [0.0, 0.0],
            "rotation_degrees": 0.0,
            "scale": 1.0,
            "trimmed_texture_mae": 0.0,
            "texture_p90": 0.0,
            "identity_exact": True,
        }
    # Keep changed facial contours and their boundary gradients out of the fit.
    immutable = ~binary_dilation(mutable, iterations=3)
    texture = _texture(reference)
    points = immutable & (texture > 3)
    margin = min(20, max(2, min(width, height) // 32))
    points[:margin] = points[-margin:] = False
    points[:, :margin] = points[:, -margin:] = False
    yy, xx = np.indices((height, width))
    stride = 3 if min(width, height) >= 200 else 2
    points &= (yy % stride == 0) & (xx % stride == 0)
    ys, xs = [part.astype(np.float64) for part in np.nonzero(points)]
    if len(xs) < 40:
        raise ValueError("registration refused: insufficient immutable source texture")
    source_samples = gaussian_filter(reference.mean(axis=2), 0.6)[ys.astype(int), xs.astype(int)]
    current_gray = gaussian_filter(current.mean(axis=2), 0.6)
    limit, angle = thresholds["translation_limit_px"], thresholds["rotation_limit_degrees"]
    bounds = ([-limit, -limit, -angle, 0.97], [limit, limit, angle, 1.03])

    def residual(parameters):
        coordinates = _coordinates((height, width), parameters, ys, xs)
        return map_coordinates(current_gray, coordinates, order=1, mode="nearest") - source_samples

    # The phase seed only proposes a shift; the immutable samples decide the fit.
    window = np.hanning(height)[:, None] * np.hanning(width)[None, :]
    cross = np.fft.fft2(texture * immutable * window) * np.conj(
        np.fft.fft2(_texture(current) * immutable * window)
    )
    correlation = np.fft.ifft2(cross / np.maximum(np.abs(cross), 1e-8)).real
    sy, sx = np.unravel_index(np.argmax(correlation), correlation.shape)
    shift = [sx if sx <= width // 2 else sx - width, sy if sy <= height // 2 else sy - height]
    seed = np.array([*np.clip(shift, -limit + 0.01, limit - 0.01), 0.0, 1.0])
    fits = []
    for initial in (seed, np.array([0.0, 0.0, 0.0, 1.0])):
        fit = least_squares(
            residual,
            initial,
            bounds=bounds,
            loss="soft_l1",
            f_scale=8,
            x_scale=[5, 5, 1, 0.02],
            max_nfev=60,
        )
        absolute = np.abs(residual(fit.x))
        score = float(np.sort(absolute)[: max(1, int(len(absolute) * 0.8))].mean())
        fits.append((score, fit, absolute))
    fits.sort(key=lambda row: row[0])
    score, fit, absolute = fits[0]
    parameters = fit.x
    corners_y = np.array([0, 0, height - 1, height - 1], dtype=float)
    corners_x = np.array([0, width - 1, 0, width - 1], dtype=float)
    best_corners = _coordinates((height, width), parameters, corners_y, corners_x)
    near = [row for row in fits if row[0] <= score * 1.05 + 0.01]
    disagreement = max(
        float(
            np.linalg.norm(
                _coordinates((height, width), row[1].x, corners_y, corners_x) - best_corners,
                axis=0,
            ).max()
        )
        for row in near
    )
    p90 = float(np.percentile(absolute, 90))
    near_bound = any(
        abs(v - edge) < 0.0001
        for v, low, high in zip(parameters, *bounds, strict=True)
        for edge in (low, high)
    )
    failures = []
    if not fit.success:
        failures.append("optimizer_failed")
    if score > thresholds["maximum_trimmed_texture_mae"]:
        failures.append("texture_mae_exceeded")
    if p90 > thresholds["maximum_texture_p90"]:
        failures.append("texture_p90_exceeded")
    if disagreement > thresholds["maximum_transform_disagreement_px"]:
        failures.append("ambiguous_transform")
    if near_bound:
        failures.append("transform_bound_reached")
    aligned, valid = _warp(current, parameters)
    if np.any(required & ~valid):
        failures.append(
            "mutable_region_outside_registered_donor"
            if required_support is None
            else "required_support_outside_registered_donor"
        )
    if failures:
        raise ValueError("registration refused: " + ", ".join(failures))
    delta = np.abs(aligned - reference).max(axis=2)
    outside = ~mutable & valid
    report = {
        **base_report,
        "status": "passed_technical_gate",
        "translation_xy_px": parameters[:2].tolist(),
        "rotation_degrees": float(parameters[2]),
        "scale": float(parameters[3]),
        "trimmed_texture_mae": score,
        "texture_p90": p90,
        "immutable_texture_samples": len(xs),
        "near_best_corner_disagreement_px": disagreement,
        "outside_mask_rgb_mae": float(np.abs(aligned - reference)[outside].mean()),
        "outside_mask_changed_pixels_over_8": int((outside & (delta > 8)).sum()),
        "inside_mask_changed_pixels_over_8": int((mutable & (delta > 8)).sum()),
        "valid_fraction": float(valid.mean()),
        "identity_exact": False,
    }
    return Image.fromarray(np.clip(np.rint(aligned), 0, 255).astype(np.uint8)), report


def extract_patch(
    source_crop: Image.Image,
    aligned: Image.Image,
    authored_mask: Image.Image,
    feather_px: float = 3,
) -> tuple[Image.Image, dict]:
    """Blend donor RGB once, feathering inward, and emit native binary support alpha.

    The polygon is the only authority for replacement. Pixel differences are
    diagnostics, never an automatic mask expansion. The patch stays at the full
    native crop size; its caller supplies the original integer crop offset.
    """
    original = _source(source_crop)
    mask = _mask(authored_mask, source_crop.size)
    if aligned.mode != "RGB" or aligned.size != source_crop.size:
        raise ValueError("the registered full-face donor must be RGB at native crop size")
    if isinstance(feather_px, bool) or not isinstance(feather_px, (int, float)):
        raise ValueError("feather_px must be a finite nonnegative number")
    if not math.isfinite(feather_px) or feather_px < 0:
        raise ValueError("feather_px must be a finite nonnegative number")
    core = mask > 0
    weights = mask.astype(np.float64) / 255
    if feather_px:
        weights *= np.clip(distance_transform_edt(core) / feather_px, 0, 1)
    weights[original[..., 3] == 0] = 0
    support = weights > 0
    donor = np.asarray(aligned, dtype=np.float64)
    matte_source = np.asarray(_flatten(source_crop), dtype=np.uint8)
    # Registration uses a neutral matte. Undo it only where source alpha is
    # partial; exact unchanged matte pixels retain their original RGB verbatim.
    alpha = original[..., 3:4].astype(np.float64) / 255
    straight = np.clip(
        np.divide(
            donor - np.asarray(NEUTRAL) * (1 - alpha),
            alpha,
            out=np.zeros_like(donor),
            where=alpha > 0,
        ),
        0,
        255,
    )
    unchanged = np.all(donor == matte_source, axis=2)
    straight[unchanged] = original[..., :3][unchanged]
    pixels = np.rint(original[..., :3] * (1 - weights[..., None]) + straight * weights[..., None])
    patch = np.zeros_like(original)
    patch[support, :3] = np.clip(pixels[support], 0, 255).astype(np.uint8)
    patch[support, 3] = 255
    delta = np.abs(donor - matte_source).max(axis=2)
    return Image.fromarray(patch), {
        "patch_size": list(source_crop.size),
        "authored_mask_pixels": int(core.sum()),
        "support_pixels": int(support.sum()),
        "inside_mask_changed_pixels_over_8": int((core & (delta > 8)).sum()),
        "outside_mask_changed_pixels_over_8": int((~core & (delta > 8)).sum()),
        "feather_px": feather_px,
        "feather_direction": "inside_authored_support",
        "patch_alpha": "binary_replacement_support",
        "feather_already_baked": True,
        "mask_expanded_from_difference": False,
    }


def apply_patch(source: Image.Image, patch: Image.Image, offset: tuple[int, int]) -> Image.Image:
    """Replace selected RGB at an integer offset, preserving source alpha everywhere."""
    original = _source(source)
    supplied = _source(patch)
    if len(offset) != 2 or any(isinstance(v, bool) or not isinstance(v, int) for v in offset):
        raise ValueError("the native crop offset must contain two integers")
    if not np.isin(supplied[..., 3], [0, 255]).all():
        raise ValueError("patch alpha must be binary; feathering is already baked into RGB")
    left, top = offset
    x0, y0 = max(0, left), max(0, top)
    x1, y1 = min(source.width, left + patch.width), min(source.height, top + patch.height)
    result = original.copy()
    if x0 < x1 and y0 < y1:
        incoming = supplied[y0 - top : y1 - top, x0 - left : x1 - left]
        selected = incoming[..., 3] == 255
        region = result[y0:y1, x0:x1]
        if np.any(selected & (region[..., 3] == 0)):
            raise ValueError("a patch cannot paint fully transparent original pixels")
        region[selected, :3] = incoming[selected, :3]
    return Image.fromarray(result)


def self_check() -> None:
    """Exercise exact rest, source confinement, known shift, and failed-fit refusal."""
    random = np.random.default_rng(6)
    rgb = np.clip(gaussian_filter(random.uniform(30, 225, (160, 160, 3)), (0.7, 0.7, 0)), 0, 255)
    image = Image.fromarray(np.dstack((rgb.astype(np.uint8), np.full((160, 160), 255, np.uint8))))
    mask = polygon_mask(image.size, [[60, 65], [100, 65], [100, 95], [60, 95]])
    aligned, _ = register_donor(image, image, mask)
    patch, _ = extract_patch(image, aligned, mask)
    assert apply_patch(image, patch, (0, 0)).tobytes() == image.tobytes()
    partial = np.asarray(image).copy()
    partial[:8, :8, 3] = 0
    partial[72:80, 72:80, 3] = 128
    partial_source = Image.fromarray(partial)
    aligned, _ = register_donor(partial_source, partial_source, mask)
    patch, _ = extract_patch(partial_source, aligned, mask)
    assert apply_patch(partial_source, patch, (0, 0)).tobytes() == partial_source.tobytes()
    donor = np.asarray(image.convert("RGB")).copy()
    donor[70:90, 65:95] = (130, 45, 65)
    # A provider also changes an unrelated corner. It must never enter a patch.
    donor[5:15, 5:15] = (255, 0, 0)
    patch, metrics = extract_patch(image, Image.fromarray(donor), mask)
    composed = np.asarray(apply_patch(image, patch, (0, 0)))
    support = np.asarray(patch)[..., 3] > 0
    assert np.array_equal(composed[~support], np.asarray(image)[~support])
    assert np.array_equal(composed[..., 3], np.asarray(image)[..., 3])
    assert metrics["outside_mask_changed_pixels_over_8"] > 0
    canvas = Image.new("RGBA", (192, 192), (14, 23, 32, 73))
    canvas.paste(image, (17, 21))
    placed = np.asarray(apply_patch(canvas, patch, (17, 21)))
    placed_support = np.zeros((192, 192), dtype=bool)
    placed_support[21:181, 17:177] = support
    assert np.array_equal(placed[~placed_support], np.asarray(canvas)[~placed_support])
    assert np.array_equal(placed[..., 3], np.asarray(canvas)[..., 3])
    shifted = np.full_like(donor, 240)
    shifted[1:, 2:] = donor[:-1, :-2]
    _, report = register_donor(image, Image.fromarray(shifted), mask)
    assert abs(report["translation_xy_px"][0] + 2) < 0.3
    assert abs(report["translation_xy_px"][1] + 1) < 0.3
    try:
        register_donor(image, Image.new("RGB", image.size, "black"), mask)
    except ValueError:
        pass
    else:
        raise AssertionError("an unrelated donor passed registration")


if __name__ == "__main__":
    self_check()
    print("registered patches: exact rest, native replacement, registration and refusal passed")
