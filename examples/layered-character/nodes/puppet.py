"""Give the native gaze package an attached head volume and independent torso.

One authored ownership polygon separates existing source pixels. A registered
full-context donor completes only the neck hidden behind the head. The destination
player owns deformation; this node does not render or synthesize head poses.
"""

from __future__ import annotations

import hashlib
import io
import json
import math
import zipfile
from collections.abc import Mapping
from pathlib import PurePosixPath
from typing import Any

import numpy as np
from grida.fx import Ctx, node
from PIL import Image, ImageChops
from scipy.ndimage import distance_transform_edt

from nodes.character import _open, _zip, png_bytes
from nodes.gaze import render_gaze_state
from nodes.registered_patches import apply_patch, extract_patch, polygon_mask, register_donor

RIG = {
    "yaw": 0.35,
    "pitch": 0.25,
    "roll": 0.6,
    "bulge": 0.65,
    "drift": [0.04, 0.02],
    "pitch_ease": 0.25,
    "wind": 0,
    "breath": 1,
    "sway": 0,
    "brow": 0,
    "jaw": 0,
    "body": {"yaw": 0.12, "roll": 0.1, "breath": 0.35, "bust": 0},
    "neck": {"twist": 0.45, "reach": 0.4},
}
POSE_LIMITS = {
    "angle_x": (-30, 30),
    "angle_y": (-30, 30),
    "angle_z": (-30, 30),
    "body_x": (-10, 10),
    "body_z": (-10, 10),
    "breath": (0, 1),
}
DEFAULT_BOUNDS = {
    "angle_x": [-14, 14],
    "angle_y": [-10, 10],
    "angle_z": [-6, 6],
    "body_x": [-2, 2],
    "body_z": [-1, 1],
    "breath": [0, 1],
}


def _mask(size: tuple[int, int], value: Any, name: str) -> Image.Image:
    if not isinstance(value, list):
        raise ValueError(f"{name} must be a polygon in original source coordinates")
    try:
        return polygon_mask(size, value)
    except ValueError as error:
        raise ValueError(f"{name}: {error}") from error


def _anchors(value: Any, original: Mapping[str, Any], size: tuple[int, int]) -> dict[str, Any]:
    if not isinstance(value, Mapping):
        raise ValueError("puppet anchors must be an authoring object")
    result = {**original, **value}
    head = result.get("head")
    if (
        not isinstance(head, Mapping)
        or any(
            isinstance(head.get(key), bool)
            or not isinstance(head.get(key), (int, float))
            or not math.isfinite(head[key])
            for key in ("cx", "cy", "rx", "ry")
        )
        or head["rx"] <= 0
        or head["ry"] <= 0
    ):
        raise ValueError("anchors.head must be a finite ellipse with positive radii")
    face = result.get("face")
    if (
        not isinstance(face, (list, tuple))
        or len(face) != 4
        or any(
            isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v)
            for v in face
        )
        or not (0 <= face[0] < face[2] <= size[0] and 0 <= face[1] < face[3] <= size[1])
    ):
        raise ValueError("anchors.face must be a box inside the native canvas")
    for name in ("neck_y", "top_y", "bottom_y", "hair_root_y"):
        v = result.get(name)
        if isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v):
            raise ValueError(f"anchors.{name} must be finite")
    for name in ("jaw", "chest"):
        item = result.get(name)
        if item is not None and (
            not isinstance(item, Mapping)
            or any(
                isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v)
                for v in item.values()
            )
        ):
            raise ValueError(f"anchors.{name} must contain finite numeric fields")
    # The supplied jaw locates the chin and its cast shadow. No procedural mouth
    # drop is enabled; the existing source-native speech frames remain unchanged.
    if "jaw" in result:
        result["jaw"] = {**result["jaw"], **dict.fromkeys(("half", "open", "round", "wide"), 0)}
    return result


def _bounds(value: Any) -> dict[str, list[float]]:
    if value is None:
        value = DEFAULT_BOUNDS
    if not isinstance(value, Mapping) or set(value) != set(POSE_LIMITS):
        raise ValueError("pose_bounds must name angle_x/y/z, body_x/z and breath")
    result = {}
    for name, limit in POSE_LIMITS.items():
        supplied = value[name]
        if (
            not isinstance(supplied, (list, tuple))
            or len(supplied) != 2
            or any(
                isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v)
                for v in supplied
            )
            or not limit[0] <= supplied[0] <= supplied[1] <= limit[1]
            or not supplied[0] <= 0 <= supplied[1]
        ):
            raise ValueError(f"pose_bounds.{name} leaves the destination parameter range")
        result[name] = list(supplied)
    return result


def _package(data: bytes) -> tuple[dict[str, Any], dict[str, bytes], dict[str, Image.Image]]:
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            entries = archive.infolist()
            if len(entries) > 2048 or sum(entry.file_size for entry in entries) > 256 * 2**20:
                raise ValueError("gaze package exceeds portable package limits")
            names = [entry.filename for entry in entries]
            if len(names) != len(set(names)) or any(
                PurePosixPath(name).is_absolute() or ".." in PurePosixPath(name).parts
                for name in names
            ):
                raise ValueError("gaze package paths must be unique and confined")
            files = {entry.filename: archive.read(entry) for entry in entries}
    except (zipfile.BadZipFile, KeyError) as error:
        raise ValueError("package must be a readable LivLiv archive") from error
    try:
        model = json.loads(files.pop("model.json"))
    except (KeyError, json.JSONDecodeError, UnicodeDecodeError) as error:
        raise ValueError("package needs a valid model.json") from error
    if model.get("format") != "livliv-model" or model.get("version") != 4:
        raise ValueError("package must use the LivLiv v4 format")
    textures = {
        name: _open(content) for name, content in files.items() if name.startswith("textures/")
    }
    for entry in model.get("layers", []):
        paths = [entry.get("texture"), *entry.get("frames", [])]
        paths += [entry["mask"]] if "mask" in entry else []
        for path in paths:
            if path not in textures or textures[path].size != (entry["w"], entry["h"]):
                raise ValueError("all gaze textures must match their native layer boxes")
    return model, files, textures


def _shadow(
    reference: Image.Image,
    backing: Image.Image,
    mask: Image.Image,
    ownership: Image.Image,
) -> tuple[Image.Image, dict[str, Any]]:
    original = np.asarray(reference)[..., :3].astype(np.float64)
    clean = np.asarray(backing)[..., :3].astype(np.float64)
    selected = np.asarray(mask) > 0
    visible = selected & (np.asarray(ownership) == 0)
    if not visible.any():
        raise ValueError("neck shadow needs visible source pixels below the head")
    color = np.percentile(original[visible], 10, axis=0)
    direction = clean - color
    projection = np.divide(
        np.sum((clean - original) * direction, axis=2),
        np.sum(direction * direction, axis=2),
        out=np.zeros(selected.shape),
        where=np.sum(direction * direction, axis=2) > 16,
    )
    darker = original.mean(axis=2) < clean.mean(axis=2) - 2
    alpha = np.clip(projection, 0, 1) * selected * darker
    # Keep chromatic edge correction within the representable straight-alpha
    # foreground, rather than clipping the recovered color to a dark rectangle.
    lower = np.maximum(
        np.divide(clean - original, clean, out=np.zeros_like(clean), where=clean > 0),
        np.divide(original - clean, 255 - clean, out=np.zeros_like(clean), where=clean < 255),
    ).max(axis=2)
    alpha = np.where(alpha > 0, np.maximum(alpha, lower), 0)
    alpha *= np.minimum(1, distance_transform_edt(selected) / 2)
    rgb = np.divide(
        original - clean * (1 - alpha[..., None]),
        alpha[..., None],
        out=original.copy(),
        where=alpha[..., None] > 0,
    )
    rgba = np.dstack(
        (np.rint(np.clip(rgb, 0, 255)).astype(np.uint8), np.rint(alpha * 255).astype(np.uint8))
    )
    return Image.fromarray(rgba), {
        "method": "source_cast_shadow_unmixed_against_completed_neck",
        "support_pixels": int((alpha > 0).sum()),
        "source_visible_support_pixels": int((visible & (alpha > 0)).sum()),
        "reference_shadow_color": color.tolist(),
    }


def build_puppet_package(
    portrait_bytes: bytes,
    gaze_archive: bytes,
    donors: Mapping[str, bytes],
    geometry: Mapping[str, Any],
) -> tuple[dict[str, Any], bytes, bytes, dict[str, Any], dict[str, bytes], dict[str, bytes]]:
    """Split head and body without rescaling the source, then declare a native rig."""
    if set(donors) != {"body_neck"}:
        raise ValueError("puppet donors must contain exactly body_neck")
    source = _open(portrait_bytes)
    if (
        geometry.get("schema_version") != 1
        or geometry.get("source_size") != list(source.size)
        or geometry.get("source_sha256") != hashlib.sha256(portrait_bytes).hexdigest()
    ):
        raise ValueError("puppet geometry must bind to this exact source portrait")
    model, files, textures = _package(gaze_archive)
    if model.get("canvas") != list(source.size):
        raise ValueError("gaze package canvas differs from the original portrait")
    ids = {entry["id"] for entry in model["layers"]}
    if not {"portrait", "lid_L", "lid_R", "eye_L", "eye_R", "iris_L", "iris_R", "mouth"} <= ids:
        raise ValueError("puppet requires the source-native two-eye gaze package")
    base_entry = next(entry for entry in model["layers"] if entry["id"] == "portrait")
    if (base_entry["x"], base_entry["y"], base_entry["w"], base_entry["h"]) != (0, 0, *source.size):
        raise ValueError("gaze portrait backing must retain the full native canvas")
    reference = render_gaze_state(model, textures)
    backing = textures[base_entry["texture"]]
    ownership = _mask(source.size, geometry.get("head_polygon_xy"), "head_polygon_xy")
    neck_fill = _mask(source.size, geometry.get("neck_fill_polygon_xy"), "neck_fill_polygon_xy")
    ownership_array = np.asarray(ownership) > 0
    if not np.any(ownership_array & (np.asarray(backing)[..., 3] > 0)):
        raise ValueError("head ownership contains no visible original head")
    if not np.any(~ownership_array & (np.asarray(backing)[..., 3] > 0)):
        raise ValueError("head ownership must leave a visible independent torso")
    for entry in model["layers"]:
        if entry["id"] == "portrait":
            continue
        path = entry["texture"]
        a = np.asarray(textures[path])[..., 3] > 0
        owned = ownership_array[
            entry["y"] : entry["y"] + entry["h"], entry["x"] : entry["x"] + entry["w"]
        ]
        if owned.shape != a.shape or np.any(a & ~owned):
            raise ValueError(f"head ownership does not contain the {entry['id']} source layer")
    exclusion = ImageChops.lighter(ownership, neck_fill)
    aligned, registration = register_donor(
        source, _open(donors["body_neck"]), exclusion, required_support=neck_fill
    )
    neck_texture_report: dict[str, Any] = {
        "method": "registered_donor_rgb",
        "extrapolated_pixels": 0,
    }
    if geometry.get("neck_texture_polygon_xy") is not None:
        texture = _mask(source.size, geometry["neck_texture_polygon_xy"], "neck_texture_polygon_xy")
        valid = np.asarray(texture) > 0
        fill = np.asarray(neck_fill) > 0
        if not valid.any() or np.any(valid & ~fill):
            raise ValueError("neck_texture_polygon_xy must be nonempty and inside neck_fill")
        nearest = distance_transform_edt(~valid, return_distances=False, return_indices=True)
        pixels = np.asarray(aligned).copy()
        copied = fill & ~valid
        pixels[copied] = pixels[nearest[0][copied], nearest[1][copied]]
        aligned = Image.fromarray(pixels)
        neck_texture_report = {
            "method": "nearest_registered_rgb_from_authored_skin_interior",
            "authored_texture_pixels": int(valid.sum()),
            "extrapolated_pixels": int(copied.sum()),
            "resized_or_repositioned": False,
        }
    neck_patch, extraction = extract_patch(source, aligned, neck_fill, feather_px=3)
    completed = apply_patch(backing, neck_patch, (0, 0))
    body = np.asarray(completed).copy()
    body[..., 3][ownership_array] = 0
    body[..., 3][np.asarray(neck_fill) > 0] = 255
    body_image = Image.fromarray(body)

    head = np.asarray(backing).copy()
    original_head_alpha = head[..., 3].copy()
    canonical_head_alpha = np.where(original_head_alpha >= 250, 255, original_head_alpha)
    head[..., 3] = np.where(ownership_array, canonical_head_alpha, 0)
    head_image = Image.fromarray(head)
    head_box = head_image.getchannel("A").getbbox()
    if head_box is None:
        raise ValueError("head has no native alpha coverage")
    hx0, hy0, hx1, hy1 = head_box
    head_image = head_image.crop(head_box)
    files["textures/portrait.png"] = png_bytes(body_image)
    textures["textures/portrait.png"] = body_image
    files["textures/head.png"] = png_bytes(head_image)
    textures["textures/head.png"] = head_image
    body_entry = {**base_entry, "texture": "textures/portrait.png", "group": "body", "depth": 0}
    head_entry = {
        "id": "face",
        "texture": "textures/head.png",
        "x": hx0,
        "y": hy0,
        "w": hx1 - hx0,
        "h": hy1 - hy0,
        "group": "head",
        "depth": 0,
    }
    anchors = _anchors(geometry.get("anchors"), model["anchors"], source.size)
    shadow_layers = []
    shadow_report: dict[str, Any] = {"status": "omitted"}
    if geometry.get("neck_shadow_polygon_xy") is not None:
        supplied_shadow = _mask(
            source.size, geometry["neck_shadow_polygon_xy"], "neck_shadow_polygon_xy"
        )
        shadow_mask = ImageChops.multiply(supplied_shadow, neck_fill)
        shadow_image, shadow_report = _shadow(reference, body_image, shadow_mask, ownership)
        shadow_box = shadow_image.getchannel("A").getbbox()
        if shadow_box is not None:
            if "jaw" not in anchors:
                raise ValueError("a neck_shadow layer needs authored jaw top and chin anchors")
            x0, y0, x1, y1 = shadow_box
            shadow_image = shadow_image.crop(shadow_box)
            files["textures/neck_shadow.png"] = png_bytes(shadow_image)
            textures["textures/neck_shadow.png"] = shadow_image
            shadow_layers.append(
                {
                    "id": "neck_shadow",
                    "texture": "textures/neck_shadow.png",
                    "x": x0,
                    "y": y0,
                    "w": x1 - x0,
                    "h": y1 - y0,
                    "group": "head",
                    "depth": 0,
                    "role": "neck_shadow",
                }
            )
            shadow_report["status"] = "source_derived_layer"
    head_layers = [
        {**entry, "group": "head", "depth": 0}
        for entry in model["layers"]
        if entry["id"] != "portrait"
    ]
    model["layers"] = [body_entry, *shadow_layers, head_entry, *head_layers]
    model["anchors"] = anchors
    model["rig"] = json.loads(json.dumps(RIG))
    model["tuning"] = {
        "turn": 1,
        "nod": 1,
        "hair": 0,
        "blink": 1,
        "expr": 0,
        "mouth": 1,
        "breath": 1,
    }
    bounds = _bounds(geometry.get("pose_bounds"))
    neutral = render_gaze_state(model, textures)
    admitted = ImageChops.lighter(ownership, neck_fill)
    changed = np.any(np.asarray(neutral) != np.asarray(reference), axis=2)
    if np.any(changed & (np.asarray(admitted) == 0)):
        raise ValueError("neutral puppet changed the gaze master outside head and neck ownership")
    difference = np.abs(
        np.asarray(neutral).astype(np.int16) - np.asarray(reference).astype(np.int16)
    )
    patches = {}
    records = {}
    for entry in model["layers"]:
        paths = [entry["texture"], *entry.get("frames", [])]
        paths += [entry["mask"]] if "mask" in entry else []
        for path in paths:
            if path == "textures/portrait.png":
                continue
            name = path.removeprefix("textures/").removesuffix(".png")
            patches[name] = files[path]
            records[name] = {
                "offset_xy": [entry["x"], entry["y"]],
                "size": list(textures[path].size),
                "group": entry["group"],
                "role": entry.get("role"),
            }
    preview = png_bytes(neutral)
    files["model.json"] = json.dumps(
        model, sort_keys=True, ensure_ascii=False, separators=(",", ":")
    ).encode("utf-8")
    report = {
        "kind": "native_head_body_puppet_assembly_v1",
        "construction": "source_owned_head_volume_over_completed_neck_and_independent_body",
        "source_sha256": hashlib.sha256(portrait_bytes).hexdigest(),
        "gaze_package_sha256": hashlib.sha256(gaze_archive).hexdigest(),
        "neutral_reference_sha256": hashlib.sha256(png_bytes(reference)).hexdigest(),
        "canonical_neutral_sha256": hashlib.sha256(preview).hexdigest(),
        "canvas": list(source.size),
        "body_registration": registration,
        "neck_extraction": extraction,
        "neck_texture": neck_texture_report,
        "head_bbox_xyxy": list(head_box),
        "head_interior_alpha_canonicalized_pixels": int(
            (ownership_array & (original_head_alpha >= 250) & (original_head_alpha != 255)).sum()
        ),
        "neutral_difference": {
            "outside_head_and_neck_rgba_exact": True,
            "changed_rgba_pixels": int(changed.sum()),
            "changed_rgb_pixels": int(np.any(difference[..., :3] != 0, axis=2).sum()),
            "changed_alpha_pixels": int((difference[..., 3] != 0).sum()),
            "maximum_rgb_error": int(difference[..., :3].max()),
            "maximum_alpha_error": int(difference[..., 3].max()),
        },
        "neck_shadow": shadow_report,
        "rig": RIG,
        "pose_bounds": bounds,
        "pose_bound_status": "candidate_ranges_require_destination_renderer_visual_review",
        "keyforms": "omitted; no measured pose correspondence or authored keyform claim",
        "head_layers_share": {"group": "head", "depth": 0, "native_global_lattice": True},
        "patches": records,
        "layer_count": len(model["layers"]),
        "texture_count": len(files) - 1,
        "supported_controls": [
            "gaze",
            "left_eye_blink",
            "right_eye_blink",
            "mouth_loudness",
            "round_wide_vowel",
            "head_turn",
            "head_nod",
            "head_tilt",
            "body_lean",
            "breathing",
        ],
        "unsupported_controls": [
            "independent_hair_physics",
            "brow_movement",
            "expressions",
            "profile_views",
        ],
        "visual_review": "not_performed",
        "warnings": [
            "The head is an original source cutout on the player's procedural rounded volume, "
            "not a reconstructed 3D head.",
            "Only the authored completed-neck region imports generated RGB; "
            "source body and clothing elsewhere remain native.",
            "Opaque head interior pixels form a named canonical master; "
            "original soft outer silhouette is preserved.",
            "Pose bounds are candidate limits until actual sweeps pass visual review; "
            "the package cannot enforce caller parameters.",
            "Pose frames must be rendered by the destination player; "
            "this node only verifies native neutral composition.",
        ],
    }
    return model, _zip(files), preview, report, patches, {"neutral": preview}


@node(
    "assemble_puppet",
    inputs={"portrait": "image", "package": "file", "donors": "image{}", "geometry": "json"},
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
def assemble_puppet(ctx: Ctx) -> dict[str, Any]:
    try:
        model, archive, preview, report, patches, states = build_puppet_package(
            ctx.read.bytes("portrait"),
            ctx.read.bytes("package"),
            {name: file.read_bytes() for name, file in ctx.inputs["donors"].items()},
            ctx.read.json("geometry"),
        )
    except ValueError as error:
        raise ctx.fail(str(error)) from error
    path = ctx.out.path("puppet-character.livliv")
    path.write_bytes(archive)
    ctx.fact("layer_count", report["layer_count"])
    ctx.fact("pose_bound_status", report["pose_bound_status"])
    return {
        "model": ctx.out.json(model),
        "archive": ctx.out.file(path, "file"),
        "preview": ctx.out.bytes(preview, "image/png"),
        "report": ctx.out.json(report),
        "patches": {name: ctx.out.bytes(data, "image/png") for name, data in patches.items()},
        "states": {name: ctx.out.bytes(data, "image/png") for name, data in states.items()},
    }
