"""Keep an original portrait intact and export registered native face patches.

The full-face repaints are donors, not replacement portraits. Authored polygons
admit only the two eyelids and the mouth. Registration and local feathering happen
in the source crop; packaging never fits, mirrors or relocates a facial feature.
"""

from __future__ import annotations

import hashlib
import io
import json
import math
import zipfile
from collections.abc import Mapping
from typing import Any

from grida.fx import Ctx, node
from PIL import Image, ImageChops

from nodes.registered_patches import (
    apply_patch,
    extract_patch,
    polygon_mask,
    register_donor,
)

DONORS = ("blink", "mouth_half", "mouth_open", "mouth_round", "mouth_wide")
FEATURES = ("canvas_left_eye", "canvas_right_eye", "mouth")
VISEMES = ("closed", "half", "open", "round", "wide")
UNSUPPORTED = (
    "gaze",
    "head_turn",
    "head_nod",
    "head_tilt",
    "body_pose",
    "breathing",
    "hair_physics",
    "brow_movement",
    "expressions",
)
Box = tuple[int, int, int, int]


def png_bytes(image: Image.Image) -> bytes:
    out = io.BytesIO()
    image.save(out, format="PNG", optimize=False, compress_level=9)
    return out.getvalue()


def _open(data: bytes) -> Image.Image:
    with Image.open(io.BytesIO(data)) as image:
        image.load()
        return image.convert("RGBA")


def _source_png(data: bytes, source: Image.Image) -> bytes:
    with Image.open(io.BytesIO(data)) as original:
        if original.format == "PNG" and original.mode == "RGBA":
            return data
    return png_bytes(source)


def _zip(files: dict[str, bytes]) -> bytes:
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_STORED) as archive:
        names = ["model.json", *sorted(name for name in files if name != "model.json")]
        for name in names:
            entry = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            entry.create_system = 3
            entry.external_attr = 0o100644 << 16
            archive.writestr(entry, files[name], compress_type=zipfile.ZIP_STORED)
    return out.getvalue()


def _geometry(
    source: Image.Image, geometry: Mapping[str, Any]
) -> tuple[Box, dict[str, Image.Image], dict[str, Any]]:
    if geometry.get("schema_version") != 1:
        raise ValueError("geometry.schema_version must be 1")
    if geometry.get("source_size") != list(source.size):
        raise ValueError("geometry.source_size does not match the original portrait")
    raw_crop = geometry.get("working_crop_xyxy")
    if (
        not isinstance(raw_crop, list)
        or len(raw_crop) != 4
        or any(type(value) is not int for value in raw_crop)
    ):
        raise ValueError("geometry.working_crop_xyxy is four integer source coordinates")
    x0, y0, x1, y1 = raw_crop
    if not (0 <= x0 < x1 <= source.width and 0 <= y0 < y1 <= source.height):
        raise ValueError("the working crop must be inside the original portrait")
    crop: Box = (x0, y0, x1, y1)
    features = geometry.get("features")
    if not isinstance(features, Mapping):
        raise ValueError("geometry.features must name the two eyes and mouth")
    masks = {}
    for name in FEATURES:
        feature = features.get(name)
        polygon = feature.get("polygon_xy") if isinstance(feature, Mapping) else None
        if not isinstance(polygon, list) or len(polygon) < 3:
            raise ValueError(f"{name}.polygon_xy needs at least three source points")
        points = []
        for point in polygon:
            if (
                not isinstance(point, (list, tuple))
                or len(point) != 2
                or any(
                    isinstance(value, bool)
                    or not isinstance(value, (int, float))
                    or not math.isfinite(value)
                    for value in point
                )
            ):
                raise ValueError(f"{name}.polygon_xy contains an invalid point")
            x, y = point
            if not (x0 <= x < x1 and y0 <= y < y1):
                raise ValueError(f"{name}.polygon_xy leaves the working crop")
            points.append([x - x0, y - y0])
        mask = polygon_mask((x1 - x0, y1 - y0), points)
        if mask.getbbox() is None:
            raise ValueError(f"{name} has empty pixel support")
        masks[name] = mask
    for index, left in enumerate(FEATURES):
        for right in FEATURES[index + 1 :]:
            if ImageChops.multiply(masks[left], masks[right]).getbbox() is not None:
                raise ValueError(f"{left} and {right} must not overlap")
    anchors = geometry.get("anchors", {})
    if not isinstance(anchors, Mapping):
        raise ValueError("geometry.anchors must be an object when supplied")
    head = anchors.get(
        "head", {"cx": (x0 + x1) / 2, "cy": (y0 + y1) / 2, "rx": (x1 - x0) / 2, "ry": (y1 - y0) / 2}
    )
    face = anchors.get("face", list(crop))
    if (
        not isinstance(head, Mapping)
        or not all(
            isinstance(head.get(key), (int, float))
            and not isinstance(head[key], bool)
            and math.isfinite(head[key])
            for key in ("cx", "cy", "rx", "ry")
        )
        or head["rx"] <= 0
        or head["ry"] <= 0
    ):
        raise ValueError("anchors.head is a finite ellipse with positive radii")
    if (
        not isinstance(face, (list, tuple))
        or len(face) != 4
        or not all(
            isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)
            for value in face
        )
        or not (0 <= face[0] < face[2] <= source.width and 0 <= face[1] < face[3] <= source.height)
    ):
        raise ValueError("anchors.face is a source box inside the portrait")
    scalars = {"neck_y": y1, "top_y": 0, "bottom_y": source.height, "hair_root_y": y0}
    for name, default in scalars.items():
        value = anchors.get(name, default)
        if (
            isinstance(value, bool)
            or not isinstance(value, (int, float))
            or not math.isfinite(value)
        ):
            raise ValueError(f"anchors.{name} must be a finite number")
        scalars[name] = value
    return crop, masks, {"head": dict(head), "face": list(face), **scalars}


def _state_check(source: Image.Image, state: Image.Image, mask: Image.Image) -> dict[str, Any]:
    if source.size != state.size:
        raise ValueError("a recomposed state changed the original canvas")
    difference = ImageChops.difference(source.convert("RGB"), state.convert("RGB"))
    outside = mask.point(lambda value: 0 if value else 255).convert("RGB")
    outside_changed = ImageChops.multiply(difference, outside).getbbox() is not None
    alpha_exact = source.getchannel("A").tobytes() == state.getchannel("A").tobytes()
    changed = difference.getbbox()
    if outside_changed or not alpha_exact:
        raise ValueError("a recomposed state changed pixels outside its authored feature support")
    return {
        "source_alpha_exact": alpha_exact,
        "outside_authored_support_exact": not outside_changed,
        "changed_rgb_bbox_xyxy": list(changed) if changed else None,
        "has_visible_change": changed is not None,
    }


def build_package(
    portrait_bytes: bytes,
    donors: Mapping[str, bytes],
    geometry: Mapping[str, Any],
) -> tuple[dict[str, Any], bytes, bytes, dict[str, Any], dict[str, bytes], dict[str, bytes]]:
    """Return manifest, package, exact neutral PNG, report, patches and full states."""
    if set(donors) != set(DONORS):
        raise ValueError(f"donors must contain exactly {', '.join(DONORS)}")
    expected_source = geometry.get("source_sha256")
    if expected_source and expected_source != hashlib.sha256(portrait_bytes).hexdigest():
        raise ValueError("geometry belongs to a different original portrait")
    feather = geometry.get("feather_px", 3)
    if (
        isinstance(feather, bool)
        or not isinstance(feather, (int, float))
        or not math.isfinite(feather)
        or not 0 <= feather <= 32
    ):
        raise ValueError("geometry.feather_px must be a finite value from 0 to 32")
    source = _open(portrait_bytes)
    if min(source.size) < 16 or max(source.size) > 8192:
        raise ValueError("the original portrait canvas must be 16 to 8192 pixels per side")
    crop_box, masks, anchors = _geometry(source, geometry)
    source_crop = source.crop(crop_box)
    crop_offset = crop_box[:2]
    blink_mask = ImageChops.lighter(masks["canvas_left_eye"], masks["canvas_right_eye"])
    registration = {}
    extraction = {}
    full_patches = {}
    aligned_blink, registration["blink"] = register_donor(
        source_crop, _open(donors["blink"]), blink_mask
    )
    for name, feature in (("lid_L", "canvas_left_eye"), ("lid_R", "canvas_right_eye")):
        full_patches[name], extraction[name] = extract_patch(
            source_crop, aligned_blink, masks[feature], feather_px=feather
        )
    mouth_mask = masks["mouth"]
    for name in DONORS[1:]:
        aligned, registration[name] = register_donor(source_crop, _open(donors[name]), mouth_mask)
        full_patches[name], extraction[name] = extract_patch(
            source_crop, aligned, mouth_mask, feather_px=feather
        )
    closed = source_crop.copy()
    closed.putalpha(mouth_mask.point(lambda value: 255 if value else 0))
    full_patches["mouth_closed"] = closed

    supports = {"lid_L": masks["canvas_left_eye"], "lid_R": masks["canvas_right_eye"]}
    supports.update({f"mouth_{kind}": mouth_mask for kind in VISEMES})
    patch_images = {}
    offsets = {}
    support_records = {}
    for name, full_patch in full_patches.items():
        box = supports[name].getbbox()
        if box is None:
            raise ValueError(f"{name} has no authored support")
        patch_images[name] = full_patch.crop(box)
        offsets[name] = (crop_offset[0] + box[0], crop_offset[1] + box[1])
        support_records[name] = {
            "offset_xy": list(offsets[name]),
            "size": list(patch_images[name].size),
            "support_bbox_xyxy": [
                crop_offset[0] + box[0],
                crop_offset[1] + box[1],
                crop_offset[0] + box[2],
                crop_offset[1] + box[3],
            ],
            "visible_alpha_pixels": sum(patch_images[name].getchannel("A").histogram()[1:]),
        }

    neutral = apply_patch(source, patch_images["mouth_closed"], offsets["mouth_closed"])
    neutral_exact = neutral.tobytes() == source.tobytes()
    if not neutral_exact:
        raise ValueError("the neutral native composition differs from the original portrait")
    source_png = _source_png(portrait_bytes, source)
    states = {"neutral": source_png}
    state_checks = {}

    def state(name: str, patch_names: tuple[str, ...], local_mask: Image.Image) -> None:
        picture = source.copy()
        for patch_name in patch_names:
            picture = apply_patch(picture, patch_images[patch_name], offsets[patch_name])
        allowed = Image.new("L", source.size)
        allowed.paste(local_mask, crop_offset)
        state_checks[name] = _state_check(source, picture, allowed)
        states[name] = png_bytes(picture)

    state("blink", ("lid_L", "lid_R"), blink_mask)
    for name in DONORS[1:]:
        state(name, (name,), mouth_mask)

    patches = {name: png_bytes(image) for name, image in patch_images.items()}
    # The original mouth remains in the untouched portrait at rest. The main
    # texture supplies native mesh coverage; a transparent frame avoids drawing
    # a second copy over the source's partially transparent pixels.
    rest = Image.new("RGBA", patch_images["mouth_closed"].size)
    rest_png = png_bytes(rest)
    patches["mouth_rest"] = rest_png
    support_records["mouth_rest"] = {**support_records["mouth_closed"], "visible_alpha_pixels": 0}
    files = {"textures/portrait.png": source_png}
    files.update({f"textures/{name}.png": data for name, data in patches.items()})
    layers = [
        {
            "id": "portrait",
            "texture": "textures/portrait.png",
            "x": 0,
            "y": 0,
            "w": source.width,
            "h": source.height,
            "group": "head",
            "depth": 0,
        }
    ]
    for name in ("lid_L", "lid_R", "mouth_closed"):
        image, offset = patch_images[name], offsets[name]
        entry = {
            "id": "mouth" if name == "mouth_closed" else name,
            "texture": f"textures/{name}.png",
            "x": offset[0],
            "y": offset[1],
            "w": image.width,
            "h": image.height,
            "group": "head",
            "depth": 0,
            "role": "mouth" if name == "mouth_closed" else name,
        }
        if name == "mouth_closed":
            box = mouth_mask.getbbox()
            if box is None:
                raise ValueError("mouth has empty support")
            row_widths = [
                sum(value > 0 for value in mouth_mask.crop((box[0], y, box[2], y + 1)).getdata())
                for y in range(box[1], box[3])
            ]
            widest = max(row_widths)
            center_row = min(
                (index for index, count in enumerate(row_widths) if count == widest),
                key=lambda index: abs(index - len(row_widths) / 2),
            )
            entry.update(
                frames=[
                    "textures/mouth_rest.png",
                    *[f"textures/mouth_{kind}.png" for kind in VISEMES[1:]],
                ],
                visemes=list(VISEMES),
                pivot=[offset[0] + image.width // 2, crop_offset[1] + box[1] + center_row],
            )
        layers.append(entry)
    eye_boxes = [support_records[name]["support_bbox_xyxy"] for name in ("lid_L", "lid_R")]
    model: dict[str, Any] = {
        "format": "livliv-model",
        "version": 4,
        "name": "Fern",
        "meta": {"generator": {"name": "Grida FX"}},
        "requires": [],
        "canvas": list(source.size),
        "profile": {"id": "anime_half", "facing": "front"},
        "tuning": {"turn": 0, "nod": 0, "hair": 0, "blink": 1, "expr": 0, "mouth": 1, "breath": 0},
        "rig": {
            "yaw": 0,
            "pitch": 0,
            "roll": 0,
            "bulge": 0,
            "drift": [0, 0],
            "wind": 0,
            "breath": 0,
            "sway": 0,
            "brow": 0,
            "jaw": 0,
            "body": {"yaw": 0, "roll": 0, "breath": 0, "bust": 0},
        },
        "anchors": {**anchors, "eyes": eye_boxes},
        "expressions": [],
        "layers": layers,
    }
    files["model.json"] = json.dumps(
        model, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    warnings = [
        "Native raster composition is exact at rest; browser scaling and texture sampling "
        "can resample pixels.",
        "Blinking crossfades closed-eye replacement patches near closure; "
        "there is no reconstructed iris or gaze.",
        "The destination player slightly eases nonclosed mouth frames vertically during playback; "
        "inspect patch edges.",
        "Pose, breathing, hair, gaze, brows and expressions are unsupported "
        "until genuine native parts exist.",
        "Registered still states preserve original alpha exactly. The player alpha-overlays "
        "changed patches, so alpha inside those patches may increase; "
        "neutral uses no mouth overlay.",
    ]
    for name, check in state_checks.items():
        if not check["has_visible_change"]:
            warnings.append(
                f"{name}: no visible RGB change was extracted; inspect the donor and masks"
            )
    report = {
        "kind": "native_portrait_character_assembly_v1",
        "construction": "original_portrait_with_registered_local_repaint_patches",
        "canvas": list(source.size),
        "working_crop_xyxy": list(crop_box),
        "geometry_schema_version": 1,
        "neutral_rgba_exact": neutral_exact,
        "neutral_preview_is_original_png_bytes": source_png == portrait_bytes,
        "closed_mouth_rendering": "transparent_rest_frame_over_original_portrait_mouth",
        "mouth_mesh_texture": "original_native_mouth_crop",
        "exactness_scope": "native_raster_composition_before_player_resampling_or_animation",
        "registration": registration,
        "extraction": extraction,
        "patches": support_records,
        "states": state_checks,
        "layer_count": len(layers),
        "texture_count": len(files) - 1,
        "mouth_frames": list(VISEMES),
        "supported_controls": [
            "left_eye_blink",
            "right_eye_blink",
            "mouth_loudness",
            "round_wide_vowel",
        ],
        "unsupported_controls": list(UNSUPPORTED),
        "visual_review": "not_performed",
        "warnings": warnings,
    }
    return model, _zip(files), source_png, report, patches, states


@node(
    "assemble",
    inputs={"portrait": "image", "donors": "image{}", "geometry": "json"},
    outputs={
        "model": "json",
        "archive": "file",
        "preview": "image/png",
        "report": "json",
        "patches": "image/png{}",
        "states": "image/png{}",
    },
    version=2,
)
def assemble(ctx: Ctx) -> dict[str, Any]:
    try:
        model, archive, preview, report, patches, states = build_package(
            ctx.read.bytes("portrait"),
            {name: file.read_bytes() for name, file in ctx.inputs["donors"].items()},
            ctx.read.json("geometry"),
        )
    except ValueError as error:
        raise ctx.fail(str(error)) from error
    archive_path = ctx.out.path("character.livliv")
    archive_path.write_bytes(archive)
    ctx.fact("layer_count", report["layer_count"])
    ctx.fact("neutral_rgba_exact", report["neutral_rgba_exact"])
    ctx.fact("visual_review", report["visual_review"])
    return {
        "model": ctx.out.json(model),
        "archive": ctx.out.file(archive_path, "file"),
        "preview": ctx.out.bytes(preview, "image/png"),
        "report": ctx.out.json(report),
        "patches": {name: ctx.out.bytes(data, "image/png") for name, data in patches.items()},
        "states": {name: ctx.out.bytes(data, "image/png") for name, data in states.items()},
    }
