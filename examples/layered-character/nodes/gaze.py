"""Extend the native repaint package with real, clipped eye layers.

No feature is fitted to an atlas. The source supplies visible irises and original
lashes; registered contextual donors supply eye backing and hidden iris surfaces.
Authored apertures and native travel bounds remain the authority for movement.
"""

from __future__ import annotations

import hashlib
import io
import json
import math
import zipfile
from collections.abc import Mapping
from typing import Any

import numpy as np
from grida.fx import Ctx, node
from PIL import Image, ImageChops, ImageDraw, ImageFilter
from scipy.ndimage import distance_transform_edt

from nodes.character import VISEMES, _geometry, _open, _zip, build_package, png_bytes
from nodes.registered_patches import apply_patch, extract_patch, polygon_mask, register_donor

GAZE_DONORS = ("eye_skin", "eye_whites", "eye_complete")
Box = tuple[int, int, int, int]


def _pair(value: Any, name: str, *, positive: bool = False) -> tuple[float, float]:
    if (
        not isinstance(value, (list, tuple))
        or len(value) != 2
        or any(
            isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v)
            for v in value
        )
        or positive
        and any(v <= 0 for v in value)
    ):
        raise ValueError(f"{name} must contain two finite {'positive ' if positive else ''}numbers")
    return float(value[0]), float(value[1])


def _polygon(value: Any, crop: Box, name: str, *, smooth: bool = False) -> Image.Image:
    if not isinstance(value, list) or len(value) < 3:
        raise ValueError(f"{name} must be an authored polygon")
    x0, y0, x1, y1 = crop
    local = []
    for point in value:
        x, y = _pair(point, name)
        if not (x0 <= x < x1 and y0 <= y < y1):
            raise ValueError(f"{name} leaves the working crop")
        local.append([x - x0, y - y0])
    size = (x1 - x0, y1 - y0)
    if not smooth:
        return polygon_mask(size, local)
    scale = 4
    large = polygon_mask(
        (size[0] * scale, size[1] * scale),
        [[x * scale, y * scale] for x, y in local],
    )
    return large.resize(size, Image.Resampling.LANCZOS)


def _ellipse(
    size: tuple[int, int], center: tuple[float, float], radii: tuple[float, float]
) -> Image.Image:
    scale = 4
    large = Image.new("L", (size[0] * scale, size[1] * scale))
    x, y = center
    rx, ry = radii
    ImageDraw.Draw(large).ellipse(
        ((x - rx) * scale, (y - ry) * scale, (x + rx) * scale, (y + ry) * scale),
        fill=255,
    )
    return large.resize(size, Image.Resampling.LANCZOS)


def _contained(mask: Image.Image, outer: Image.Image, name: str) -> None:
    outside = outer.point(lambda v: 0 if v else 255)
    if ImageChops.multiply(mask, outside).getbbox() is not None:
        raise ValueError(f"{name} leaves its authored eye replacement region")


def _foreground(
    rgb: np.ndarray, backing: np.ndarray, contour: Image.Image, edge_px: float
) -> tuple[np.ndarray, np.ndarray, dict[str, int]]:
    """Unmix a true cutout edge instead of carrying white pixels with the iris.

    Interior pixels, including original specular highlights, remain opaque. Only
    the authored contour's narrow boundary band uses contrast against its known
    whites backing to estimate foreground coverage.
    """
    core = np.asarray(contour) >= 128
    distance = distance_transform_edt(core)
    interior = core & (distance > edge_px)
    if not interior.any():
        raise ValueError("iris contour is too small for its authored matte edge")
    _, nearest = distance_transform_edt(~interior, return_indices=True)
    foreground_hint = rgb[nearest[0], nearest[1]]
    direction = foreground_hint - backing
    denominator = np.sum(direction * direction, axis=2)
    projection = np.divide(
        np.sum((rgb - backing) * direction, axis=2),
        denominator,
        out=np.zeros(core.shape, dtype=np.float64),
        where=denominator > 16,
    )
    edge = core & ~interior
    alpha = core.astype(np.float64)
    alpha[edge] = np.clip(projection[edge], 0, 1)
    contrast = np.max(np.abs(rgb - backing), axis=2)
    alpha[edge & (contrast < 12)] = 0
    alpha[alpha < 0.035] = 0
    unmix = np.divide(
        rgb - backing * (1 - alpha[..., None]),
        alpha[..., None],
        out=rgb.copy(),
        where=alpha[..., None] > 0,
    )
    return (
        np.clip(unmix, 0, 255),
        alpha,
        {
            "authored_contour_pixels": int(core.sum()),
            "opaque_interior_pixels": int(interior.sum()),
            "edge_matte_pixels": int(edge.sum()),
            "rejected_white_edge_pixels": int((edge & (alpha == 0)).sum()),
            "unmix_clamped_components": int(((unmix < 0) | (unmix > 255))[alpha > 0].sum()),
        },
    )


def _paste_layer(canvas: Image.Image, image: Image.Image, layer: Mapping[str, Any]) -> Image.Image:
    placed = Image.new("RGBA", canvas.size)
    placed.alpha_composite(image, (layer["x"], layer["y"]))
    return Image.alpha_composite(canvas, placed)


def render_gaze_state(
    model: Mapping[str, Any],
    textures: Mapping[str, Image.Image],
    *,
    gaze_xy: tuple[float, float] = (0, 0),
    eye_open: float = 1,
    mouth_kind: str = "closed",
) -> Image.Image:
    """Native raster reference for fully open/closed eyes, without GPU sampling.

    Intermediate eyelid motion belongs to the actual player and is not simulated
    by this still-state helper. Mouth frames here are un-eased donor stills.
    """
    if eye_open not in (0, 1):
        raise ValueError("raster reference supports only fully open or fully closed eyes")
    if mouth_kind not in VISEMES:
        raise ValueError("unknown mouth frame")
    canvas = Image.new("RGBA", tuple(model["canvas"]))
    by_id = {layer["id"]: layer for layer in model["layers"]}
    for layer in model["layers"]:
        role = layer.get("role", "")
        if layer["id"] == "portrait":
            # Copy the original hidden RGB too. Compositing a transparent base
            # onto an empty canvas would discard those source bytes.
            canvas = textures[layer["texture"]].copy()
            continue
        if role.startswith("lid_") and eye_open:
            continue
        if (role.startswith("eye_") or role.startswith("iris_")) and not eye_open:
            continue
        path = layer["texture"]
        if role == "mouth":
            path = layer["frames"][VISEMES.index(mouth_kind)]
        image = textures[path]
        if role.startswith("iris_"):
            parent = by_id[layer["clip"]]
            moved = Image.new("RGBA", canvas.size)
            dx = round(gaze_xy[0] * layer["range"][0])
            dy = round(-gaze_xy[1] * layer["range"][1])
            moved.alpha_composite(image, (layer["x"] + dx, layer["y"] + dy))
            clip = Image.new("L", canvas.size)
            # The player uses a 0.5 alpha stencil in the neutral pose and soft
            # mask coverage during a gaze; keep that distinction in still QA.
            aperture = textures[parent["mask"]].getchannel("A")
            if gaze_xy == (0, 0):
                aperture = aperture.point(lambda a: 255 if a >= 128 else 0)
            clip.paste(aperture, (parent["x"], parent["y"]))
            moved.putalpha(ImageChops.multiply(moved.getchannel("A"), clip))
            canvas = Image.alpha_composite(canvas, moved)
        else:
            canvas = _paste_layer(canvas, image, layer)
    return canvas


def _difference(source: Image.Image, image: Image.Image, admitted: Image.Image) -> dict[str, Any]:
    a, b = np.asarray(source), np.asarray(image)
    selected = np.asarray(admitted) > 0
    changed = np.any(a != b, axis=2)
    if np.any(changed & ~selected):
        raise ValueError("gaze construction changed original pixels outside authored support")
    rgb = np.abs(a[..., :3].astype(np.int16) - b[..., :3].astype(np.int16))
    alpha = np.abs(a[..., 3].astype(np.int16) - b[..., 3].astype(np.int16))
    return {
        "outside_authored_support_rgba_exact": True,
        "changed_rgba_pixels": int(changed.sum()),
        "changed_rgb_pixels": int(np.any(rgb != 0, axis=2).sum()),
        "changed_alpha_pixels": int((alpha != 0).sum()),
        "maximum_rgb_error": int(rgb.max()),
        "maximum_alpha_error": int(alpha.max()),
        "mean_rgb_error_inside_support": float(rgb[selected].mean()) if selected.any() else 0,
    }


def build_gaze_package(
    portrait_bytes: bytes,
    donors: Mapping[str, bytes],
    geometry: Mapping[str, Any],
    gaze_donors: Mapping[str, bytes],
    gaze_geometry: Mapping[str, Any],
) -> tuple[dict[str, Any], bytes, bytes, dict[str, Any], dict[str, bytes], dict[str, bytes]]:
    """Add true clipped iris movement to the source-preserving mouth/lid package."""
    if set(gaze_donors) != set(GAZE_DONORS):
        raise ValueError(f"gaze_donors must contain exactly {', '.join(GAZE_DONORS)}")
    source = _open(portrait_bytes)
    if (
        gaze_geometry.get("schema_version") != 1
        or gaze_geometry.get("source_size") != list(source.size)
        or gaze_geometry.get("source_sha256") != hashlib.sha256(portrait_bytes).hexdigest()
    ):
        raise ValueError("gaze geometry must bind to this original portrait and its native size")
    eye_specs = gaze_geometry.get("eyes")
    if not isinstance(eye_specs, Mapping) or set(eye_specs) != {"L", "R"}:
        raise ValueError("gaze geometry must describe image-left L and image-right R eyes")
    model, native_archive, _, native_report, patches, _ = build_package(
        portrait_bytes, donors, geometry
    )
    with zipfile.ZipFile(io.BytesIO(native_archive)) as archive:
        files = {name: archive.read(name) for name in archive.namelist() if name != "model.json"}
    textures = {path: _open(data) for path, data in files.items()}
    crop, broad_masks, _ = _geometry(source, geometry)
    source_crop = source.crop(crop)
    source_rgb = np.asarray(source_crop)[..., :3].astype(np.float64)
    mutable = ImageChops.lighter(broad_masks["canvas_left_eye"], broad_masks["canvas_right_eye"])
    aligned = {}
    registration = {}
    for name in GAZE_DONORS:
        aligned[name], registration[name] = register_donor(
            source_crop, _open(gaze_donors[name]), mutable
        )
    canonical = source.copy()
    canonical_alpha = canonical.getchannel("A")
    base = source.copy()
    eye_layers = []
    eye_records = {}
    full_admission = Image.new("L", source.size)
    full_admission.paste(mutable, crop[:2])
    support_union = Image.new("L", source_crop.size)

    def add_texture(name: str, image: Image.Image) -> str:
        path = f"textures/{name}.png"
        textures[path] = image
        files[path] = png_bytes(image)
        return path

    def layer(name: str, image: Image.Image, local_box: Box, **fields: Any) -> dict[str, Any]:
        x0, y0, x1, y1 = local_box
        if image.size != (x1 - x0, y1 - y0):
            raise ValueError(f"{name} image does not match its native layer bounds")
        return {
            "id": name,
            "texture": add_texture(name, image),
            "x": crop[0] + x0,
            "y": crop[1] + y0,
            "w": image.width,
            "h": image.height,
            "group": "head",
            "depth": 0,
            **fields,
        }

    for side, feature in (("L", "canvas_left_eye"), ("R", "canvas_right_eye")):
        spec = eye_specs[side]
        if not isinstance(spec, Mapping):
            raise ValueError(f"eye {side} must be an authoring object")
        aperture = _polygon(spec.get("aperture_polygon_xy"), crop, f"{side}.aperture", smooth=True)
        support = _polygon(
            spec.get("eye_support_polygon_xy", spec.get("aperture_polygon_xy")),
            crop,
            f"{side}.eye_support",
        )
        broad = broad_masks[feature]
        _contained(support, broad, f"{side}.eye_support")
        # Antialiasing of the clip matte is bounded by the eye's foreground support.
        aperture = ImageChops.multiply(aperture, support)
        if aperture.getbbox() is None:
            raise ValueError(f"eye {side} has no visible aperture")
        center = _pair(spec.get("iris_center_xy"), f"{side}.iris_center_xy")
        radii = _pair(spec.get("iris_radii_xy"), f"{side}.iris_radii_xy", positive=True)
        travel = _pair(spec.get("range_xy"), f"{side}.range_xy")
        if min(travel) < 0:
            raise ValueError("iris travel must be nonnegative")
        pivot = _pair(spec.get("pivot_xy"), f"{side}.pivot_xy")
        cx, cy = center[0] - crop[0], center[1] - crop[1]
        if not (
            radii[0] <= cx < source_crop.width - radii[0]
            and radii[1] <= cy < source_crop.height - radii[1]
        ):
            raise ValueError(f"eye {side}'s complete iris leaves the working crop")
        iris_alpha = _ellipse(source_crop.size, (cx, cy), radii)
        source_visible = (
            _polygon(spec["source_visible_polygon_xy"], crop, f"{side}.source_visible")
            if "source_visible_polygon_xy" in spec
            else iris_alpha.point(lambda a: 255 if a >= 128 else 0)
        )
        visible = ImageChops.multiply(iris_alpha, source_visible)
        hole = (
            _polygon(spec["iris_hole_polygon_xy"], crop, f"{side}.iris_hole")
            if "iris_hole_polygon_xy" in spec
            else visible.point(lambda a: 255 if a else 0)
        )
        dilate = gaze_geometry.get("iris_hole_dilate_px", 0)
        if isinstance(dilate, bool) or not isinstance(dilate, int) or not 0 <= dilate <= 8:
            raise ValueError("iris_hole_dilate_px must be an integer from 0 to 8")
        if dilate:
            hole = hole.filter(ImageFilter.MaxFilter(2 * dilate + 1))
        hole = ImageChops.multiply(hole, aperture.point(lambda a: 255 if a >= 128 else 0))
        _contained(hole, support, f"{side}.iris_hole")

        skin_patch, skin_metrics = extract_patch(
            source_crop, aligned["eye_skin"], broad, feather_px=3
        )
        base = apply_patch(base, skin_patch, crop[:2])
        support_union = ImageChops.lighter(support_union, support)
        local_global = Image.new("L", source.size)
        local_global.paste(support, crop[:2])
        full_admission = ImageChops.lighter(full_admission, local_global)
        canonical_alpha.paste(255, (0, 0, source.width, source.height), local_global)

        white_patch, _ = extract_patch(source_crop, aligned["eye_whites"], hole, feather_px=0)
        white_picture = apply_patch(source_crop, white_patch, (0, 0))
        eye_rgb = np.asarray(white_picture)[..., :3].astype(np.float64)
        eye_rgba = np.dstack((eye_rgb.astype(np.uint8), np.asarray(support)))
        eye_image = Image.fromarray(eye_rgba)
        support_box = support.getbbox()
        if support_box is None:
            raise ValueError(f"eye {side} has no layer support")

        completion_center = _pair(
            spec.get("completion_center_xy", center), f"{side}.completion_center"
        )
        completion_radii = _pair(
            spec.get("completion_radii_xy", radii), f"{side}.completion_radii", positive=True
        )
        clip_all = np.asarray(aperture) >= 128
        edge_px = max(2.0, float(gaze_geometry.get("matte_feather_px", 1)) * 3)
        foreground_rgb, foreground_alpha, matte_metrics = _foreground(
            source_rgb, eye_rgb, source_visible, edge_px
        )
        foreground_alpha *= clip_all
        complete_rgb = np.asarray(aligned["eye_complete"]).astype(np.float64)
        white_all_patch, _ = extract_patch(source_crop, aligned["eye_whites"], broad, feather_px=0)
        white_all = np.asarray(apply_patch(source_crop, white_all_patch, (0, 0)))[..., :3]
        white_samples = white_all[clip_all]
        white_samples = white_samples[np.min(white_samples, axis=1) > 160]
        if not len(white_samples):
            raise ValueError(f"eye {side} has no usable whites background")
        completion_backing = np.broadcast_to(np.median(white_samples, axis=0), complete_rgb.shape)
        completion_contour = _ellipse(
            source_crop.size,
            (completion_center[0] - crop[0], completion_center[1] - crop[1]),
            completion_radii,
        )
        completed_rgb, completed_alpha, completion_metrics = _foreground(
            complete_rgb, completion_backing, completion_contour, edge_px
        )
        completed_rgba = Image.fromarray(
            np.dstack(
                (
                    np.rint(completed_rgb).astype(np.uint8),
                    np.rint(completed_alpha * 255).astype(np.uint8),
                )
            )
        )
        coverage = np.maximum(
            np.asarray(iris_alpha), np.rint(foreground_alpha * 255).astype(np.uint8)
        )
        iris_box = Image.fromarray(coverage).getbbox()
        if iris_box is None:
            raise ValueError(f"eye {side} has no iris coverage")
        ix0, iy0, ix1, iy1 = iris_box
        target_w, target_h = ix1 - ix0, iy1 - iy0
        # Map named ellipse coordinates, not a padded alpha bounding box.
        scale_x = completion_radii[0] / radii[0]
        scale_y = completion_radii[1] / radii[1]
        complete = (
            completed_rgba.convert("RGBa")
            .transform(
                (target_w, target_h),
                Image.Transform.AFFINE,
                (
                    scale_x,
                    0,
                    completion_center[0] - crop[0] + scale_x * (ix0 - cx),
                    0,
                    scale_y,
                    completion_center[1] - crop[1] + scale_y * (iy0 - cy),
                ),
                Image.Resampling.BICUBIC,
            )
            .convert("RGBA")
        )
        mapped = np.asarray(complete)
        iris_rgb = mapped[..., :3].astype(np.float64)
        clip_at_rest = np.asarray(aperture.crop(iris_box)) >= 128
        visible_alpha = foreground_alpha[iy0:iy1, ix0:ix1]
        original_visible = (visible_alpha > 0) & clip_at_rest
        hidden_alpha = mapped[..., 3].astype(np.float64) / 255
        hidden_alpha *= np.asarray(iris_alpha.crop(iris_box)).astype(np.float64) / 255
        alpha = np.where(clip_at_rest, visible_alpha, hidden_alpha)
        iris_rgb[original_visible] = foreground_rgb[iy0:iy1, ix0:ix1][original_visible]
        iris_image = Image.fromarray(
            np.dstack((np.rint(iris_rgb).astype(np.uint8), np.rint(alpha * 255).astype(np.uint8)))
        )

        # Reuse registered blink appearance, but discard broad skin/hair outside
        # an explicit lid support. This keeps automatic lash-curve fitting local.
        native_lid = next(entry for entry in model["layers"] if entry["id"] == f"lid_{side}")
        lid_full = Image.new("RGBA", source_crop.size)
        lid_full.alpha_composite(
            textures[native_lid["texture"]],
            (native_lid["x"] - crop[0], native_lid["y"] - crop[1]),
        )
        lid_support = _polygon(
            spec.get(
                "lid_support_polygon_xy",
                spec.get("eye_support_polygon_xy", spec.get("aperture_polygon_xy")),
            ),
            crop,
            f"{side}.lid_support",
        )
        _contained(lid_support, broad, f"{side}.lid_support")
        lid_full.putalpha(ImageChops.multiply(lid_full.getchannel("A"), lid_support))
        lid_box = lid_support.getbbox()
        if lid_box is None:
            raise ValueError(f"eye {side} has no closed lid support")
        lid_entry = layer(f"lid_{side}", lid_full.crop(lid_box), lid_box, role=f"lid_{side}")
        eye_entry = layer(
            f"eye_{side}",
            eye_image.crop(support_box),
            support_box,
            role=f"eye_{side}",
            pivot=list(pivot),
        )
        mask = Image.new("RGBA", aperture.size, (255, 255, 255, 0))
        mask.putalpha(aperture)
        eye_entry["mask"] = add_texture(f"eye_{side}_mask", mask.crop(support_box))
        iris_entry = layer(
            f"iris_{side}",
            iris_image,
            iris_box,
            role=f"iris_{side}",
            clip=f"eye_{side}",
            range=list(travel),
            pivot=list(pivot),
        )
        eye_layers.extend((lid_entry, eye_entry, iris_entry))
        hidden_visible = (alpha > 0) & clip_at_rest & ~original_visible
        eye_records[side] = {
            "eye_support_bbox_xyxy": [
                crop[0] + support_box[0],
                crop[1] + support_box[1],
                crop[0] + support_box[2],
                crop[1] + support_box[3],
            ],
            "iris_center_xy": list(center),
            "iris_radii_xy": list(radii),
            "range_xy": list(travel),
            "pivot_xy": list(pivot),
            "source_visible_iris_pixels": int(original_visible.sum()),
            "completed_hidden_iris_pixels": int(((alpha > 0) & ~original_visible).sum()),
            "completed_pixels_visible_at_rest": int(hidden_visible.sum()),
            "original_iris_matte": matte_metrics,
            "completion_iris_matte": completion_metrics,
            "skin_extraction": skin_metrics,
        }

    canonical.putalpha(canonical_alpha)
    base.putalpha(canonical_alpha)
    model["layers"] = [model["layers"][0], *eye_layers, model["layers"][-1]]
    files["textures/portrait.png"] = png_bytes(base)
    textures["textures/portrait.png"] = base
    model["profile"]["glasses"] = False
    canonical_neutral = render_gaze_state(model, textures)
    exactness = _difference(source, canonical_neutral, full_admission)
    allowed_all = full_admission.copy()
    mouth_full = Image.new("L", source.size)
    mouth_full.paste(broad_masks["mouth"], crop[:2])
    allowed_all = ImageChops.lighter(allowed_all, mouth_full)
    states = {"neutral": png_bytes(canonical_neutral)}
    state_checks = {}
    for name, gaze in (
        ("gaze_left", (-1, 0)),
        ("gaze_right", (1, 0)),
        ("gaze_up", (0, 1)),
        ("gaze_down", (0, -1)),
    ):
        picture = render_gaze_state(model, textures, gaze_xy=gaze)
        state_checks[name] = _difference(source, picture, full_admission)
        states[name] = png_bytes(picture)
    blink = render_gaze_state(model, textures, eye_open=0)
    state_checks["blink"] = _difference(source, blink, full_admission)
    states["blink"] = png_bytes(blink)
    for kind in VISEMES[1:]:
        picture = render_gaze_state(model, textures, mouth_kind=kind)
        state_checks[f"mouth_{kind}"] = _difference(source, picture, allowed_all)
        states[f"mouth_{kind}"] = png_bytes(picture)
    for path, image in textures.items():
        if path != "textures/portrait.png":
            patches[path.removeprefix("textures/").removesuffix(".png")] = png_bytes(image)
    preview = states["neutral"]
    patch_records = {}
    for entry in model["layers"]:
        paths = [entry["texture"], *entry.get("frames", [])]
        paths += [entry["mask"]] if "mask" in entry else []
        for path in paths:
            if path == "textures/portrait.png":
                continue
            name = path.removeprefix("textures/").removesuffix(".png")
            patch_records[name] = {
                "offset_xy": [entry["x"], entry["y"]],
                "size": list(textures[path].size),
                "support_bbox_xyxy": [
                    entry["x"],
                    entry["y"],
                    entry["x"] + entry["w"],
                    entry["y"] + entry["h"],
                ],
                "visible_alpha_pixels": sum(textures[path].getchannel("A").histogram()[1:]),
            }
    files["model.json"] = json.dumps(
        model, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    report = {
        **native_report,
        "kind": "native_portrait_gaze_assembly_v1",
        "construction": "native_eye_backing_whites_clipped_source_iris_with_hidden_completion",
        "neutral_rgba_exact": exactness["changed_rgba_pixels"] == 0,
        "neutral_preview_is_original_png_bytes": False,
        "neutral_reference": "explicit_canonical_eye_stack",
        "source_sha256": hashlib.sha256(portrait_bytes).hexdigest(),
        "canonical_neutral_sha256": hashlib.sha256(preview).hexdigest(),
        "canonical_neutral_difference": exactness,
        "eye_registration": registration,
        "eyes": eye_records,
        "patches": patch_records,
        "states": state_checks,
        "layer_count": len(model["layers"]),
        "texture_count": len(files) - 1,
        "supported_controls": [
            "left_eye_blink",
            "right_eye_blink",
            "gaze",
            "mouth_loudness",
            "round_wide_vowel",
        ],
        "unsupported_controls": [
            name for name in native_report["unsupported_controls"] if name != "gaze"
        ],
        "warnings": [
            "Eye-support interiors form an explicit opaque canonical master; "
            "original source is preserved outside authored eye masks.",
            "Visible original iris pixels are unmixed against the admitted whites; "
            "generated iris pixels complete hidden surfaces only.",
            "Raster snapshots do not reproduce the player's half-blink curve, "
            "inward gaze narrowing or GPU texture sampling.",
            "Inspect gaze extremes, half blinks and foreground hair/lash occlusion "
            "in the actual destination player.",
            native_report["warnings"][2],
            "Pose, breathing, hair, brows and expressions remain unsupported; gaze is implemented.",
            "The mouth still uses source-over playback over partially transparent source pixels; "
            "canonical eye interiors are opaque.",
        ],
    }
    return model, _zip(files), preview, report, patches, states


@node(
    "assemble_gaze",
    inputs={
        "portrait": "image",
        "donors": "image{}",
        "geometry": "json",
        "gaze_donors": "image{}",
        "gaze_geometry": "json",
    },
    outputs={
        "model": "json",
        "archive": "file",
        "preview": "image/png",
        "report": "json",
        "patches": "image/png{}",
        "states": "image/png{}",
    },
    version=1,
)
def assemble_gaze(ctx: Ctx) -> dict[str, Any]:
    try:
        model, archive, preview, report, patches, states = build_gaze_package(
            ctx.read.bytes("portrait"),
            {name: file.read_bytes() for name, file in ctx.inputs["donors"].items()},
            ctx.read.json("geometry"),
            {name: file.read_bytes() for name, file in ctx.inputs["gaze_donors"].items()},
            ctx.read.json("gaze_geometry"),
        )
    except ValueError as error:
        raise ctx.fail(str(error)) from error
    path = ctx.out.path("gaze-character.livliv")
    path.write_bytes(archive)
    ctx.fact("layer_count", report["layer_count"])
    ctx.fact("visual_review", report["visual_review"])
    return {
        "model": ctx.out.json(model),
        "archive": ctx.out.file(path, "file"),
        "preview": ctx.out.bytes(preview, "image/png"),
        "report": ctx.out.json(report),
        "patches": {name: ctx.out.bytes(data, "image/png") for name, data in patches.items()},
        "states": {name: ctx.out.bytes(data, "image/png") for name, data in states.items()},
    }
