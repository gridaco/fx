"""Native face patches, independent feature combinations, and lossless playback.

Reconstruction stays inside the native face crop. The outer sprite receives
already-blended RGB patches at an integer offset, preserving its original alpha.
These deterministic helpers do not run providers or grant semantic acceptance.
"""

from __future__ import annotations

import io
from collections.abc import Mapping
from dataclasses import dataclass
from itertools import product
from typing import Any

import numpy as np
from numpy.typing import NDArray
from PIL import Image

from .face_crop import FloatArray, Transform, restore_feature
from .face_patches import apply_offset_patch, isolate_patch, make_face_input
from .models import FeatureGroup, PortraitMotionSpec

PixelArray = NDArray[np.uint8]
BoolArray = NDArray[np.bool_]
StatePair = tuple[str, str]


@dataclass(frozen=True)
class FaceMotionFrames:
    """Native patches keyed by (state, feature), and complete eye/mouth states."""

    patches: dict[StatePair, Image.Image]
    combinations: dict[StatePair, Image.Image]
    masks: dict[str, FloatArray]
    offset_xy: tuple[int, int]
    crop_size: tuple[int, int]
    exactness: dict[str, Any]


def _groups(spec: PortraitMotionSpec) -> dict[FeatureGroup, list[str]]:
    groups: dict[FeatureGroup, list[str]] = {"eyes": ["rest"], "mouth": ["rest"]}
    for state in spec.states:
        groups[state.feature_group].append(state.state_id)
    return groups


def _supports(
    masks: Mapping[str, FloatArray], size: tuple[int, int]
) -> tuple[dict[FeatureGroup, BoolArray], dict[FeatureGroup, list[str]]]:
    occupied = np.zeros((size[1], size[0]), dtype=np.bool_)
    supports: dict[FeatureGroup, BoolArray] = {
        "eyes": occupied.copy(),
        "mouth": occupied.copy(),
    }
    features: dict[FeatureGroup, list[str]] = {"eyes": [], "mouth": []}
    for feature, mask in masks.items():
        if feature not in ("canvas_left_eye", "canvas_right_eye", "mouth"):
            raise ValueError("Face playback received an unsupported feature ID")
        alpha = np.asarray(mask, dtype=np.float64)
        if alpha.shape != occupied.shape or not np.isfinite(alpha).all():
            raise ValueError("Native feature mask shape or finite values are invalid")
        if (alpha < 0).any() or (alpha > 1).any():
            raise ValueError("Native feature mask alpha must be in [0, 1]")
        support = alpha > 0
        if not support.any():
            raise ValueError("An admitted native feature must have nonempty support")
        if (occupied & support).any():
            raise ValueError("Mapped feature supports overlap")
        occupied |= support
        group: FeatureGroup = "mouth" if feature == "mouth" else "eyes"
        supports[group] |= support
        features[group].append(feature)
    return supports, features


def build_face_combinations(
    source: Image.Image,
    donors: Mapping[str, Image.Image],
    masks: Mapping[str, FloatArray],
    spec: PortraitMotionSpec,
    transform: Transform,
) -> FaceMotionFrames:
    """Restore raw registered cells locally and place native patches without reblending.

    Masks describe accepted features in the working-image coordinate system.
    Donors stay at the declared atlas panel size. An absent feature remains at
    source rest even when the supplied timeline names a state for its group.
    Source dimensions are independent of the working image and atlas dimensions.
    """
    if set(donors) != {state.state_id for state in spec.states}:
        raise ValueError("Donors must match the complete declared state set")
    if set(masks) - set(spec.requested_features):
        raise ValueError("Masks include features outside the requested set")
    local_source, local_transform = make_face_input(source, transform)
    if local_transform["work_size"] != [spec.width, spec.height]:
        raise ValueError("Face transform work_size differs from the declared working canvas")
    left, top = transform["crop_box_xyxy"][:2]
    offset = (int(left), int(top))
    original = source.convert("RGBA")
    base = np.asarray(original, dtype=np.uint8)
    patches: dict[StatePair, Image.Image] = {}
    native_masks: dict[str, FloatArray] = {}
    mapped: dict[str, PixelArray] = {}
    for state in spec.states:
        donor = donors[state.state_id]
        if donor.mode != "RGB" or donor.size != spec.panel_size:
            raise ValueError("Registered RGB donors must remain at the declared panel size")
        current = original.copy()
        for feature, mask in masks.items():
            if ("mouth" if feature == "mouth" else "eyes") != state.feature_group:
                continue
            local_composed, local_mask = restore_feature(local_source, donor, mask, local_transform)
            patch = isolate_patch(local_composed, local_mask)
            current = apply_offset_patch(current, patch, offset)
            patches[(state.state_id, feature)] = patch
            native_mask = np.zeros(base.shape[:2], dtype=np.float64)
            x0, y0 = max(0, offset[0]), max(0, offset[1])
            x1 = min(source.width, offset[0] + patch.width)
            y1 = min(source.height, offset[1] + patch.height)
            native_mask[y0:y1, x0:x1] = local_mask[
                y0 - offset[1] : y1 - offset[1], x0 - offset[0] : x1 - offset[0]
            ]
            if feature in native_masks and not np.array_equal(native_masks[feature], native_mask):
                raise ValueError("Native feature support changed between donor states")
            native_masks[feature] = native_mask
        mapped[state.state_id] = np.asarray(current, dtype=np.uint8)

    supports, active_features = _supports(native_masks, source.size)
    groups = _groups(spec)
    combinations: dict[StatePair, Image.Image] = {}
    records: list[dict[str, Any]] = []
    for eyes, mouth in product(groups["eyes"], groups["mouth"]):
        pixels = base.copy()
        active = np.zeros(base.shape[:2], dtype=np.bool_)
        choices: tuple[tuple[FeatureGroup, str], ...] = (("eyes", eyes), ("mouth", mouth))
        for group, state_id in choices:
            if state_id != "rest":
                support = supports[group]
                pixels[support] = mapped[state_id][support]
                active |= support
        if not np.array_equal(pixels[~active], base[~active]):
            raise ValueError("Reconstruction changed source outside feature support")
        if not np.array_equal(pixels[..., 3], base[..., 3]):
            raise ValueError("Reconstruction changed original alpha")
        combinations[(eyes, mouth)] = Image.fromarray(pixels)
        records.append(
            {
                "eyes": eyes,
                "mouth": mouth,
                "changed_source_pixels": int(np.any(pixels != base, axis=2).sum()),
                "active_support_pixels": int(active.sum()),
            }
        )
    return FaceMotionFrames(
        patches=patches,
        combinations=combinations,
        masks=native_masks,
        offset_xy=offset,
        crop_size=local_source.size,
        exactness={
            "alpha_exact": True,
            "outside_support_rgba_exact": True,
            "rest_rgba_exact": combinations[("rest", "rest")].tobytes() == original.tobytes(),
            "feature_masks_disjoint": True,
            "feather_applied_again": False,
            "outer_placement": "native_patches_plus_offset",
            "patch_alpha": "binary_replacement_support",
            "active_features": active_features,
            "inactive_groups": [group for group, names in active_features.items() if not names],
            "combinations": records,
            "semantic_acceptance": "not_granted",
        },
    )


def encode_face_preview(
    frames: FaceMotionFrames, spec: PortraitMotionSpec
) -> tuple[bytes, dict[str, Any]]:
    """Encode the supplied timeline at original sprite resolution as lossless WebP.

    Every decoded visible RGBA pixel, alpha byte, and frame duration is checked.
    Fully transparent RGB may be normalized by WebP; native PNG combinations
    retain those bytes. Consecutive identical selections share one held frame.
    """
    groups = _groups(spec)
    if set(frames.combinations) != set(product(groups["eyes"], groups["mouth"])):
        raise ValueError("Preview needs the complete declared Cartesian state combinations")
    original = frames.combinations[("rest", "rest")].convert("RGBA")
    base = np.asarray(original, dtype=np.uint8)
    supports, active_features = _supports(frames.masks, original.size)
    selections: dict[StatePair, Image.Image] = {}
    for (eyes, mouth), image in frames.combinations.items():
        if image.mode not in ("RGB", "RGBA") or image.size != original.size:
            raise ValueError("Face playback states must share the native RGB/RGBA canvas")
        rgba = image.convert("RGBA")
        pixels = np.asarray(rgba, dtype=np.uint8)
        if not np.array_equal(pixels[..., 3], base[..., 3]):
            raise ValueError("Face playback state changed original alpha")
        active = np.zeros(base.shape[:2], dtype=np.bool_)
        if eyes != "rest":
            active |= supports["eyes"]
        if mouth != "rest":
            active |= supports["mouth"]
        if not np.array_equal(pixels[~active], base[~active]):
            raise ValueError("Face playback state changed pixels outside active feature support")
        selections[(eyes, mouth)] = rgba

    unique: list[Image.Image] = []
    held: list[int] = []
    encoded_segments: list[dict[str, int]] = []
    previous: bytes | None = None
    for index, segment in enumerate(spec.playback):
        image = selections[(segment.eyes, segment.mouth)]
        pixels_bytes = image.tobytes()
        if pixels_bytes == previous:
            held[-1] += segment.duration_ms
            encoded_segments[-1]["segment_end_exclusive"] = index + 1
            encoded_segments[-1]["duration_ms"] = held[-1]
        else:
            unique.append(image)
            held.append(segment.duration_ms)
            encoded_segments.append(
                {
                    "segment_start": index,
                    "segment_end_exclusive": index + 1,
                    "duration_ms": segment.duration_ms,
                }
            )
        previous = pixels_bytes
    if len(unique) < 2:
        raise ValueError("Timeline has no visible motion; refusing a static animation preview")
    stream = io.BytesIO()
    unique[0].save(
        stream,
        format="WEBP",
        save_all=True,
        append_images=unique[1:],
        duration=held,
        loop=0,
        lossless=True,
        quality=100,
        method=4,
        exact=True,
    )
    data = stream.getvalue()
    with Image.open(io.BytesIO(data)) as decoded:
        if getattr(decoded, "n_frames", 1) != len(unique) or decoded.info.get("loop") != 0:
            raise ValueError("WebP frame count or loop differs")
        for index, (expected, duration) in enumerate(zip(unique, held, strict=True)):
            decoded.seek(index)
            decoded.load()
            actual = np.asarray(decoded.convert("RGBA"), dtype=np.uint8)
            want = np.asarray(expected, dtype=np.uint8)
            if not np.array_equal(actual[..., 3], want[..., 3]):
                raise ValueError("WebP alpha changed")
            visible = want[..., 3] > 0
            if not np.array_equal(actual[visible], want[visible]):
                raise ValueError("Visible WebP pixels changed")
            if decoded.info.get("duration") != duration:
                raise ValueError("WebP timing changed")
    return data, {
        "frames": len(unique),
        "duration_ms": sum(held),
        "size": list(original.size),
        "logical_segment_count": len(spec.playback),
        "encoded_segments": encoded_segments,
        "decoded_visible_rgba_and_alpha_exact": True,
        "transparent_rgb_note": "PNG state files retain invisible RGB; WebP may normalize it.",
        "active_features": active_features,
        "inactive_groups": [group for group, names in active_features.items() if not names],
        "feather_applied_again": False,
        "audio_synchronized": False,
        "semantic_acceptance": "not_granted",
        "temporal_review": "not_performed",
    }
