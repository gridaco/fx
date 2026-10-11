"""Deterministic portrait atlas registration and source-preserving composition.

These functions accept pixels and explicit policy only. They do not identify facial
features, author coordinates, read run files, or grant semantic acceptance. Geometry
must come from the caller's admitted feature declarations.
"""

from __future__ import annotations

import io
import math
from dataclasses import dataclass

import numpy as np
from numpy.typing import NDArray
from PIL import Image, ImageDraw
from scipy.ndimage import distance_transform_edt, gaussian_filter, map_coordinates, sobel
from scipy.optimize import least_squares

FloatArray = NDArray[np.float64]
BoolArray = NDArray[np.bool_]
Point = tuple[float, float]


@dataclass(frozen=True)
class _RegistrationPolicy:
    translation_limit_panel_px: float = 24.0
    rotation_limit_degrees: float = 3.0
    scale_limits: tuple[float, float] = (0.94, 1.06)
    source_texture_gradient_min: float = 3.0
    source_sample_stride: int = 3
    canvas_margin_panel_px: int = 20
    maximum_evaluations_per_seed: int = 50
    soft_l1_scale: float = 8.0
    trimmed_fraction: float = 0.8
    maximum_trimmed_texture_mae: float = 14.0
    maximum_texture_p90: float = 48.0
    minimum_source_texture_samples: int = 500
    ambiguous_near_best_fraction: float = 0.05
    ambiguous_transform_disagreement_panel_px: float = 1.5


def _number(value: object, label: str, *, minimum: float = 0.0) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{label} must be a finite number")
    result = float(value)
    if not math.isfinite(result) or result < minimum:
        raise ValueError(f"{label} must be finite and at least {minimum}")
    return result


def _integer(value: object, label: str, *, minimum: int = 1) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise ValueError(f"{label} must be an integer at least {minimum}")
    return value


def _registration_policy(value: dict[str, object] | None) -> _RegistrationPolicy:
    defaults = _RegistrationPolicy()
    fields = defaults.__dataclass_fields__
    supplied = value or {}
    if set(supplied) - set(fields):
        raise ValueError("Unknown registration policy fields")
    values = {name: supplied.get(name, getattr(defaults, name)) for name in fields}
    scales = values["scale_limits"]
    if not isinstance(scales, (list, tuple)) or len(scales) != 2:
        raise ValueError("scale_limits must contain lower and upper bounds")
    low, high = (_number(scales[0], "scale lower"), _number(scales[1], "scale upper"))
    if not 0 < low < 1 < high:
        raise ValueError("scale_limits must strictly contain identity scale 1")
    positive = (
        "translation_limit_panel_px",
        "rotation_limit_degrees",
        "soft_l1_scale",
        "trimmed_fraction",
        "maximum_trimmed_texture_mae",
        "maximum_texture_p90",
        "ambiguous_transform_disagreement_panel_px",
    )
    for name in positive:
        if _number(values[name], name) <= 0:
            raise ValueError(f"{name} must be positive")
    if _number(values["trimmed_fraction"], "trimmed_fraction") > 1:
        raise ValueError("trimmed_fraction must not exceed 1")
    return _RegistrationPolicy(
        translation_limit_panel_px=_number(values["translation_limit_panel_px"], "translation"),
        rotation_limit_degrees=_number(values["rotation_limit_degrees"], "rotation"),
        scale_limits=(low, high),
        source_texture_gradient_min=_number(values["source_texture_gradient_min"], "gradient"),
        source_sample_stride=_integer(values["source_sample_stride"], "stride"),
        canvas_margin_panel_px=_integer(values["canvas_margin_panel_px"], "margin", minimum=0),
        maximum_evaluations_per_seed=_integer(
            values["maximum_evaluations_per_seed"], "evaluations"
        ),
        soft_l1_scale=_number(values["soft_l1_scale"], "soft_l1_scale"),
        trimmed_fraction=_number(values["trimmed_fraction"], "trimmed_fraction"),
        maximum_trimmed_texture_mae=_number(values["maximum_trimmed_texture_mae"], "texture MAE"),
        maximum_texture_p90=_number(values["maximum_texture_p90"], "texture p90"),
        minimum_source_texture_samples=_integer(
            values["minimum_source_texture_samples"], "samples"
        ),
        ambiguous_near_best_fraction=_number(values["ambiguous_near_best_fraction"], "near best"),
        ambiguous_transform_disagreement_panel_px=_number(
            values["ambiguous_transform_disagreement_panel_px"], "transform disagreement"
        ),
    )


def _size(size: tuple[int, int], label: str) -> tuple[int, int]:
    if len(size) != 2:
        raise ValueError(f"{label} must contain width and height")
    return _integer(size[0], f"{label} width"), _integer(size[1], f"{label} height")


def _opaque_rgb(image: Image.Image) -> Image.Image:
    _size(image.size, "image")
    if image.convert("RGBA").getchannel("A").getextrema() != (255, 255):
        raise ValueError("Portrait processing requires opaque source and donor images")
    return image.convert("RGB")


def _panel_size(size: tuple[int, int], columns: int, rows: int) -> tuple[int, int]:
    width, height = _size(size, "canvas")
    _integer(columns, "columns")
    _integer(rows, "rows")
    if width % columns or height % rows:
        raise ValueError("Canvas dimensions must be divisible by the declared grid")
    return _size((width // columns, height // rows), "panel")


def png_bytes(image: Image.Image) -> bytes:
    """Encode the supplied image losslessly without changing its pixel mode."""
    stream = io.BytesIO()
    image.save(stream, format="PNG")
    return stream.getvalue()


def make_guide(source: Image.Image, columns: int, rows: int) -> Image.Image:
    """Fill a source-sized grid with identical reduced copies in every cell."""
    original = _opaque_rgb(source)
    width, height = _panel_size(original.size, columns, rows)
    panel = original.resize((width, height), Image.Resampling.LANCZOS)
    guide = Image.new("RGB", original.size)
    for row in range(rows):
        for column in range(columns):
            guide.paste(panel, (column * width, row * height))
    return guide


def _gray(array: FloatArray) -> FloatArray:
    return np.asarray(array.mean(axis=2), dtype=np.float64)


def _texture(array: FloatArray) -> FloatArray:
    smooth = gaussian_filter(_gray(array), 0.8)
    return np.asarray(np.hypot(sobel(smooth, axis=0), sobel(smooth, axis=1)) / 8.0)


def _coordinates(
    shape: tuple[int, int], parameters: FloatArray, ys: FloatArray, xs: FloatArray
) -> FloatArray:
    height, width = shape
    center_x, center_y = (width - 1) / 2, (height - 1) / 2
    tx, ty, degrees, scale = parameters
    cosine, sine = np.cos(np.deg2rad(degrees)), np.sin(np.deg2rad(degrees))
    dx, dy = xs - center_x - tx, ys - center_y - ty
    return np.array(
        [
            (-sine * dx + cosine * dy) / scale + center_y,
            (cosine * dx + sine * dy) / scale + center_x,
        ],
        dtype=np.float64,
    )


def _warp(array: FloatArray, parameters: FloatArray) -> tuple[FloatArray, BoolArray]:
    shape = (array.shape[0], array.shape[1])
    ys, xs = np.indices(shape, dtype=np.float64)
    yy, xx = _coordinates(shape, parameters, ys, xs)
    result = np.stack(
        [
            map_coordinates(array[:, :, channel], [yy, xx], order=1, mode="constant", cval=0)
            for channel in range(3)
        ],
        axis=2,
    )
    valid = (yy >= 0) & (xx >= 0) & (yy <= shape[0] - 1) & (xx <= shape[1] - 1)
    return np.asarray(result, dtype=np.float64), valid


def _phase_seed(
    reference: FloatArray, current: FloatArray, limit: float
) -> tuple[FloatArray, float]:
    ref, cur = _texture(reference), _texture(current)
    window = np.hanning(ref.shape[0])[:, None] * np.hanning(ref.shape[1])[None, :]
    cross = np.fft.fft2(ref * window) * np.conj(np.fft.fft2(cur * window))
    correlation = np.fft.ifft2(cross / np.maximum(np.abs(cross), 1e-8)).real
    iy, ix = np.unravel_index(int(np.argmax(correlation)), correlation.shape)
    shift_y = iy if iy <= ref.shape[0] // 2 else iy - ref.shape[0]
    shift_x = ix if ix <= ref.shape[1] // 2 else ix - ref.shape[1]
    peak = float(correlation[iy, ix])
    remaining = correlation.copy()
    for dy in range(-4, 5):
        for dx in range(-4, 5):
            remaining[(iy + dy) % ref.shape[0], (ix + dx) % ref.shape[1]] = 0
    ratio = peak / max(float(remaining.max()), 1e-8)
    interior = limit - min(0.01, limit / 2)
    return np.array(
        [
            float(np.clip(shift_x, -interior, interior)),
            float(np.clip(shift_y, -interior, interior)),
            0.0,
            1.0,
        ]
    ), ratio


def _valid_rows(valid: BoolArray) -> list[list[int]]:
    """Serialize the exact valid raster footprint of a convex similarity warp."""
    result = []
    for y, row in enumerate(valid):
        positions = np.flatnonzero(row)
        if positions.size:
            result.append([y, int(positions[0]), int(positions[-1]) + 1])
    return result


@dataclass(frozen=True)
class _Fit:
    score: float
    parameters: FloatArray
    absolute: FloatArray
    success: bool
    evaluations: int


def _register(
    reference: FloatArray, current: FloatArray, cfg: _RegistrationPolicy
) -> tuple[FloatArray, dict[str, object]]:
    shape = (reference.shape[0], reference.shape[1])
    points = _texture(reference) > cfg.source_texture_gradient_min
    margin = cfg.canvas_margin_panel_px
    if margin:
        points[:margin] = points[-margin:] = False
        points[:, :margin] = points[:, -margin:] = False
    yy, xx = np.indices(shape)
    points &= (yy % cfg.source_sample_stride == 0) & (xx % cfg.source_sample_stride == 0)
    ys, xs = (np.asarray(part, dtype=np.float64) for part in np.nonzero(points))
    if len(xs) < cfg.minimum_source_texture_samples:
        valid = np.ones(shape, dtype=np.bool_)
        return current.copy(), {
            "status": "failed_technical_gate",
            "failed_checks": ["insufficient_source_texture"],
            "source_texture_samples": len(xs),
            "current_to_reference": {
                "translation_xy_panel_px": [0.0, 0.0],
                "rotation_degrees": 0.0,
                "scale": 1.0,
            },
            "optimizer_success": False,
            "optimizer_evaluations": 0,
            "valid_fraction": 1.0,
            "valid_panel_rows": _valid_rows(valid),
        }
    source_samples = gaussian_filter(_gray(reference), 0.6)[ys.astype(int), xs.astype(int)]
    current_gray = gaussian_filter(_gray(current), 0.6)
    seed, peak_ratio = _phase_seed(reference, current, cfg.translation_limit_panel_px)
    limit, angle = cfg.translation_limit_panel_px, cfg.rotation_limit_degrees
    low_scale, high_scale = cfg.scale_limits
    bounds = ([-limit, -limit, -angle, low_scale], [limit, limit, angle, high_scale])

    def residual(parameters: FloatArray) -> FloatArray:
        coordinates = _coordinates(shape, parameters, ys, xs)
        return np.asarray(
            map_coordinates(current_gray, coordinates, order=1, mode="nearest") - source_samples,
            dtype=np.float64,
        )

    fits = []
    for initial in (seed, np.array([0.0, 0.0, 0.0, 1.0])):
        fit = least_squares(
            residual,
            initial,
            bounds=bounds,
            loss="soft_l1",
            f_scale=cfg.soft_l1_scale,
            x_scale=[5, 5, 1, 0.02],
            max_nfev=cfg.maximum_evaluations_per_seed,
        )
        parameters = np.asarray(fit.x, dtype=np.float64)
        absolute = np.abs(residual(parameters))
        keep = max(1, int(len(absolute) * cfg.trimmed_fraction))
        fits.append(
            _Fit(
                float(np.sort(absolute)[:keep].mean()),
                parameters,
                absolute,
                bool(fit.success),
                int(fit.nfev),
            )
        )
    fits.sort(key=lambda item: item.score)
    best = fits[0]
    parameters = best.parameters
    near_best = [
        row
        for row in fits
        if row.score <= best.score * (1 + cfg.ambiguous_near_best_fraction) + 0.01
    ]
    corner_y = np.array([0, 0, shape[0] - 1, shape[0] - 1], dtype=np.float64)
    corner_x = np.array([0, shape[1] - 1, 0, shape[1] - 1], dtype=np.float64)
    best_mapping = _coordinates(shape, parameters, corner_y, corner_x)
    disagreement = max(
        float(
            np.linalg.norm(
                _coordinates(shape, row.parameters, corner_y, corner_x) - best_mapping, axis=0
            ).max()
        )
        for row in near_best
    )
    near_bound = any(
        abs(value - edge) < 0.002
        for value, low, high in zip(parameters, *bounds, strict=True)
        for edge in (low, high)
    )
    p90 = float(np.percentile(best.absolute, 90))
    checks = {
        "optimizer_failed": not best.success,
        "texture_mae_exceeded": best.score > cfg.maximum_trimmed_texture_mae,
        "texture_p90_exceeded": p90 > cfg.maximum_texture_p90,
        "ambiguous_transform": disagreement > cfg.ambiguous_transform_disagreement_panel_px,
        "parameter_bound_reached": near_bound,
    }
    failures = [name for name, failed in checks.items() if failed]
    aligned, valid = _warp(current, parameters)
    return aligned, {
        "status": "failed_technical_gate" if failures else "passed_technical_gate",
        "failed_checks": failures,
        "current_to_reference": {
            "translation_xy_panel_px": parameters[:2].tolist(),
            "rotation_degrees": float(parameters[2]),
            "scale": float(parameters[3]),
        },
        "phase_translation_seed": seed[:2].tolist(),
        "phase_peak_ratio": peak_ratio,
        "source_texture_samples": len(xs),
        "trimmed_texture_mae": best.score,
        "texture_p90": p90,
        "near_best_corner_disagreement_panel_px": disagreement,
        "at_parameter_bound": near_bound,
        "optimizer_success": best.success,
        "optimizer_evaluations": best.evaluations,
        "valid_fraction": float(valid.mean()),
        "valid_panel_rows": _valid_rows(valid),
    }


def split_register(
    source: Image.Image,
    atlas: Image.Image,
    state_ids: list[str],
    columns: int,
    rows: int,
    policy: dict[str, object] | None = None,
) -> tuple[dict[str, Image.Image], dict[str, dict[str, object]]]:
    """Slice in declared row-major order and fit each donor to the reduced source.

    Donors remain at native panel resolution. Failed fits are returned for inspection
    with failed status, never silently accepted. ``valid_panel_rows`` contains exact
    half-open x intervals per y; consumers must keep patch support within that footprint.
    """
    original, sheet = _opaque_rgb(source), _opaque_rgb(atlas)
    width, height = _panel_size(sheet.size, columns, rows)
    if len(state_ids) != columns * rows or any(not state for state in state_ids):
        raise ValueError("One nonempty state ID is required for every grid cell")
    if len(set(state_ids)) != len(state_ids):
        raise ValueError("State IDs must be unique")
    cfg = _registration_policy(policy)
    reference = np.asarray(original.resize((width, height), Image.Resampling.LANCZOS), np.float64)
    donors: dict[str, Image.Image] = {}
    reports: dict[str, dict[str, object]] = {}
    for index, state in enumerate(state_ids):
        x, y = index % columns * width, index // columns * height
        box = (x, y, x + width, y + height)
        aligned, report = _register(reference, np.asarray(sheet.crop(box), np.float64), cfg)
        donors[state] = Image.fromarray(np.clip(np.rint(aligned), 0, 255).astype(np.uint8))
        reports[state] = {
            **report,
            "panel_size": [width, height],
            "raw_panel_box_xyxy": list(box),
            "method": "robust_source_texture_similarity_fit",
            "semantic_acceptance": "not_granted",
        }
    return donors, reports


def _orientation(a: Point, b: Point, c: Point) -> float:
    return (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])


def _on_segment(a: Point, b: Point, point: Point) -> bool:
    return (
        abs(_orientation(a, b, point)) <= 1e-9
        and min(a[0], b[0]) <= point[0] <= max(a[0], b[0])
        and min(a[1], b[1]) <= point[1] <= max(a[1], b[1])
    )


def _intersects(a: Point, b: Point, c: Point, d: Point) -> bool:
    if _orientation(a, b, c) * _orientation(a, b, d) < 0 and (
        _orientation(c, d, a) * _orientation(c, d, b) < 0
    ):
        return True
    return any(
        (_on_segment(a, b, c), _on_segment(a, b, d), _on_segment(c, d, a), _on_segment(c, d, b))
    )


def _polygon(value: object, panel_size: tuple[int, int]) -> list[Point]:
    if not isinstance(value, list) or not 3 <= len(value) <= 64:
        raise ValueError("Polygon must contain 3 to 64 vertices")
    points = []
    for point in value:
        if not isinstance(point, list) or len(point) != 2:
            raise ValueError("Each polygon point must contain x and y")
        x, y = _number(point[0], "polygon x"), _number(point[1], "polygon y")
        if x >= panel_size[0] or y >= panel_size[1]:
            raise ValueError("Polygon lies outside registered panel coordinates")
        points.append((x, y))
    if len(set(points)) != len(points):
        raise ValueError("Polygon contains duplicate vertices")
    edges = list(zip(points, points[1:] + points[:1], strict=True))
    for i, (a, b) in enumerate(edges):
        for j, (c, d) in enumerate(edges):
            if j <= i or j == i + 1 or (i == 0 and j == len(edges) - 1):
                continue
            if _intersects(a, b, c, d):
                raise ValueError("Polygon self-intersects")
    if abs(sum(a[0] * b[1] - b[0] * a[1] for a, b in edges)) / 2 < 1:
        raise ValueError("Polygon area is degenerate")
    return points


def geometry_masks(
    features: list[dict[str, object]],
    source_size: tuple[int, int],
    panel_size: tuple[int, int],
    feather_panel_px: float,
) -> tuple[dict[str, FloatArray], dict[str, dict[str, object]]]:
    """Rasterize only declared features, retaining fully opaque polygon cores.

    Float alpha is quantized to 255 levels as in the retained baseline. Feather
    distance is measured in panel coordinates, including for unequal x/y scales.
    No feature count is inferred: an empty list or one admitted eye is valid.
    Distinct features may not overlap even at nonzero feather support.
    """
    width, height = _size(source_size, "source")
    panel_width, panel_height = _size(panel_size, "panel")
    feather = _number(feather_panel_px, "feather_panel_px")
    masks: dict[str, FloatArray] = {}
    metrics: dict[str, dict[str, object]] = {}
    occupied = np.zeros((height, width), dtype=np.bool_)
    eye_core = np.zeros_like(occupied)
    for feature in features:
        feature_id = feature.get("feature_id")
        if feature_id not in ("canvas_left_eye", "canvas_right_eye", "mouth"):
            raise ValueError("Unsupported feature ID")
        name = feature_id
        if name in masks:
            raise ValueError("Feature IDs must be unique")
        points = _polygon(feature.get("points"), panel_size)
        image = Image.new("L", source_size, 0)
        ImageDraw.Draw(image).polygon(
            [(x * width / panel_width, y * height / panel_height) for x, y in points],
            fill=255,
        )
        core = np.asarray(image) == 255
        fraction = float(core.mean())
        if not 0.0001 <= fraction <= 0.08:
            raise ValueError(f"Feature area exceeds fixed bounds: {name}")
        if feather:
            distance = np.asarray(
                distance_transform_edt(
                    ~core, sampling=(panel_height / height, panel_width / width)
                ),
                dtype=np.float64,
            )
            alpha = np.clip(1 - distance / feather, 0, 1)
            alpha[core] = 1.0
            alpha = np.rint(alpha * 255) / 255
        else:
            alpha = core.astype(np.float64)
        support = alpha > 0
        if (occupied & support).any():
            raise ValueError("Feature masks overlap, including their feather")
        occupied |= support
        if name != "mouth":
            eye_core |= core
        masks[name] = alpha
        metrics[name] = {
            "core_source_pixels": int(core.sum()),
            "core_source_fraction": fraction,
            "support_source_pixels": int(support.sum()),
            "core_alpha_min": 1.0,
            "outward_feather_panel_px": feather,
            "alpha_quantization_levels": 255,
            "coordinate_space": "registered_panel_pixels",
        }
    if float(eye_core.mean()) > 0.08:
        raise ValueError("Combined eye area exceeds fixed bounds")
    return masks, metrics


def composite(source: Image.Image, donor: Image.Image, alpha: FloatArray) -> Image.Image:
    """Replace opaque cores and preserve source pixels outside actual alpha exactly."""
    original, replacement = _opaque_rgb(source), _opaque_rgb(donor)
    weights = np.asarray(alpha, dtype=np.float64)
    if weights.shape != (original.height, original.width):
        raise ValueError("Alpha dimensions must match the source")
    if not np.isfinite(weights).all() or (weights < 0).any() or (weights > 1).any():
        raise ValueError("Alpha must contain finite values in [0, 1]")
    base = np.asarray(original, dtype=np.uint8)
    layer = np.asarray(replacement.resize(original.size, Image.Resampling.LANCZOS), dtype=np.uint8)
    amount = weights[..., None]
    pixels = np.clip(np.rint(base * (1 - amount) + layer * amount), 0, 255).astype(np.uint8)
    pixels[weights == 0] = base[weights == 0]
    pixels[weights == 1] = layer[weights == 1]
    return Image.fromarray(pixels)


def heatmap(source: Image.Image, donor: Image.Image, saturation: int = 64) -> Image.Image:
    """Fixed-scale maximum RGB difference using the retained VLM evidence palette.

    The source is reduced to the donor panel size. This is evidence, never a feature
    mask. Equal pixels are black; the configured saturation does not normalize per image.
    """
    _integer(saturation, "saturation")
    original, current = _opaque_rgb(source), _opaque_rgb(donor)
    reference = original.resize(current.size, Image.Resampling.LANCZOS)
    delta = np.abs(np.asarray(reference, np.float64) - np.asarray(current, np.float64)).max(axis=2)
    colors = np.array(
        [
            (0, 0, 0),
            (30, 12, 75),
            (89, 24, 110),
            (182, 45, 79),
            (247, 121, 36),
            (255, 249, 173),
        ],
        dtype=np.float64,
    )
    levels = np.array([0, 4, 8, 16, 32, 64], dtype=np.float64) * (saturation / 64)
    channels = [np.interp(delta, levels, colors[:, index]) for index in range(3)]
    return Image.fromarray(np.rint(np.stack(channels, axis=-1)).astype(np.uint8))
