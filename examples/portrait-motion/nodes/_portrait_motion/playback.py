"""Lossless diagnostic playback of already composited portrait state drawings.

The caller supplies state meaning and timing. This module selects existing pixels;
it does not add drawings, apply alpha a second time, or synchronize speech audio.
"""

from __future__ import annotations

import hashlib
import io
from itertools import product

import numpy as np
from numpy.typing import NDArray
from PIL import Image

from .models import FeatureGroup, PlaybackSegment, PortraitMotionSpec
from .processing import FloatArray, composite

PixelArray = NDArray[np.uint8]
BoolArray = NDArray[np.bool_]


def _rgb(image: Image.Image, size: tuple[int, int]) -> Image.Image:
    if image.size != size:
        raise ValueError("Playback state frames must match the source dimensions")
    if image.convert("RGBA").getchannel("A").getextrema() != (255, 255):
        raise ValueError("Playback requires opaque source and state frames")
    return image.convert("RGB")


def _supports(
    masks: dict[str, FloatArray], size: tuple[int, int]
) -> tuple[dict[FeatureGroup, BoolArray], dict[FeatureGroup, list[str]]]:
    shape = (size[1], size[0])
    occupied = np.zeros(shape, dtype=np.bool_)
    supports: dict[FeatureGroup, BoolArray] = {
        "eyes": np.zeros_like(occupied),
        "mouth": np.zeros_like(occupied),
    }
    features: dict[FeatureGroup, list[str]] = {"eyes": [], "mouth": []}
    for feature, alpha in masks.items():
        if feature not in ("canvas_left_eye", "canvas_right_eye", "mouth"):
            raise ValueError("Playback received an unsupported feature ID")
        weights = np.asarray(alpha, dtype=np.float64)
        if weights.shape != shape or not np.isfinite(weights).all():
            raise ValueError("Playback mask shape or finite values are invalid")
        if (weights < 0).any() or (weights > 1).any():
            raise ValueError("Playback alpha must be in [0, 1]")
        support = weights > 0
        if not support.any():
            raise ValueError("An admitted playback feature must have nonempty support")
        if (support & occupied).any():
            raise ValueError("Playback feature supports overlap")
        occupied |= support
        group: FeatureGroup = "mouth" if feature == "mouth" else "eyes"
        supports[group] |= support
        features[group].append(feature)
    return supports, features


def _timeline(
    segments: list[PlaybackSegment], state_groups: dict[str, FeatureGroup]
) -> list[dict[str, object]]:
    if not 1 <= len(segments) <= 128:
        raise ValueError("Playback requires one to 128 explicit segments")
    if any((segments[index].eyes, segments[index].mouth) != ("rest", "rest") for index in (0, -1)):
        raise ValueError("Playback must begin and end at the unchanged source")
    timeline: list[dict[str, object]] = []
    elapsed = 0
    for index, segment in enumerate(segments):
        for group, state in (("eyes", segment.eyes), ("mouth", segment.mouth)):
            if state != "rest" and state_groups.get(state) != group:
                raise ValueError("Playback references an undeclared or wrong-group state")
        if not 20 <= segment.duration_ms <= 10000:
            raise ValueError("Playback segment duration must be 20 to 10000 milliseconds")
        timeline.append(
            {
                "segment_index": index,
                "start_ms": elapsed,
                "duration_ms": segment.duration_ms,
                "eyes": segment.eyes,
                "mouth": segment.mouth,
            }
        )
        elapsed += segment.duration_ms
    if elapsed > 60000:
        raise ValueError("Playback duration exceeds sixty seconds")
    return timeline


def encode_playback(
    source: Image.Image,
    frames: dict[str, Image.Image],
    state_groups: dict[str, FeatureGroup],
    masks: dict[str, FloatArray],
    segments: list[PlaybackSegment],
    *,
    size: tuple[int, int] | None = None,
) -> tuple[bytes, dict[str, object]]:
    """Select preblended group frames and verify a looping animated WebP.

    Each frame must already equal the source outside its declared group support.
    A partial feature set remains partial: absent feature groups select the source
    and are recorded as inactive. A timeline with no visible motion is refused,
    rather than returning a static WebP as an animation.
    """
    original = _rgb(source, source.size)
    if set(frames) != set(state_groups) or "rest" in frames:
        raise ValueError("State frames and group declarations must agree and reserve rest")
    if any(group not in ("eyes", "mouth") for group in state_groups.values()):
        raise ValueError("Playback state group must be eyes or mouth")
    timeline = _timeline(segments, state_groups)
    supports, active_features = _supports(masks, original.size)
    source_pixels = np.asarray(original, dtype=np.uint8)
    saved: dict[str, PixelArray] = {}
    for state, frame in frames.items():
        pixels = np.asarray(_rgb(frame, original.size), dtype=np.uint8)
        if not np.array_equal(
            pixels[~supports[state_groups[state]]], source_pixels[~supports[state_groups[state]]]
        ):
            raise ValueError(f"State frame changed source pixels outside its group: {state}")
        saved[state] = pixels
    selections: dict[tuple[str, str], Image.Image] = {}
    for segment in segments:
        key = (segment.eyes, segment.mouth)
        if key not in selections:
            selected = source_pixels.copy()
            choices: tuple[tuple[FeatureGroup, str], ...] = (
                ("eyes", segment.eyes),
                ("mouth", segment.mouth),
            )
            for group, state in choices:
                if state != "rest":
                    support = supports[group]
                    selected[support] = saved[state][support]
            selections[key] = Image.fromarray(selected)
    data, report = _encode_selected(original, selections, segments, timeline, size)
    return data, {
        **report,
        "active_features": active_features,
        "inactive_groups": [group for group, names in active_features.items() if not names],
        "source_exact_outside_active_support_at_native_size": True,
    }


def _encode_selected(
    original: Image.Image,
    selections: dict[tuple[str, str], Image.Image],
    segments: list[PlaybackSegment],
    timeline: list[dict[str, object]],
    size: tuple[int, int] | None,
) -> tuple[bytes, dict[str, object]]:
    if size is not None and (
        len(size) != 2
        or any(
            isinstance(value, bool) or not isinstance(value, int) or value <= 0 for value in size
        )
    ):
        raise ValueError("Playback output size must contain positive integer dimensions")
    output_size = size or original.size
    cache: dict[tuple[str, str], tuple[Image.Image, str]] = {}
    encoded: list[Image.Image] = []
    durations: list[int] = []
    encoded_records: list[dict[str, object]] = []
    previous_digest: str | None = None
    for index, segment in enumerate(segments):
        key = (segment.eyes, segment.mouth)
        if key not in cache:
            image = _rgb(selections[key], original.size)
            if output_size != original.size:
                image = image.resize(output_size, Image.Resampling.LANCZOS)
            cache[key] = image, hashlib.sha256(image.tobytes()).hexdigest()
        image, pixel_digest = cache[key]
        if previous_digest == pixel_digest:
            durations[-1] += segment.duration_ms
            encoded_records[-1]["segment_end_exclusive"] = index + 1
            encoded_records[-1]["duration_ms"] = durations[-1]
        else:
            encoded.append(image)
            durations.append(segment.duration_ms)
            encoded_records.append(
                {
                    "encoded_index": len(encoded) - 1,
                    "segment_start": index,
                    "segment_end_exclusive": index + 1,
                    "duration_ms": segment.duration_ms,
                    "rgb_sha256": pixel_digest,
                }
            )
        previous_digest = pixel_digest
    if len(encoded) < 2:
        raise ValueError("Playback contains no visible motion; refusing a static animation claim")
    expected_rest = original.resize(output_size, Image.Resampling.LANCZOS)
    if encoded[0].tobytes() != expected_rest.tobytes() or (
        encoded[-1].tobytes() != expected_rest.tobytes()
    ):
        raise ValueError("Playback opening and closing pixels must equal the source")
    stream = io.BytesIO()
    encoded[0].save(
        stream,
        format="WEBP",
        save_all=True,
        append_images=encoded[1:],
        duration=durations,
        loop=0,
        lossless=True,
        quality=100,
        exact=True,
        method=6,
    )
    data = stream.getvalue()
    with Image.open(io.BytesIO(data)) as decoded:
        if decoded.format != "WEBP" or not getattr(decoded, "is_animated", False):
            raise ValueError("Playback output is not an animated WebP")
        if getattr(decoded, "n_frames", 0) != len(encoded) or decoded.info.get("loop") != 0:
            raise ValueError("Lossless playback frame count or loop flag changed")
        for index, expected in enumerate(encoded):
            decoded.seek(index)
            decoded.load()
            if decoded.info.get("duration") != durations[index]:
                raise ValueError("Lossless playback duration changed")
            if decoded.convert("RGB").tobytes() != expected.tobytes():
                raise ValueError("Lossless playback pixels changed")
    return data, {
        "schema_version": 1,
        "size": list(output_size),
        "source_size": list(original.size),
        "loop": 0,
        "total_duration_ms": sum(durations),
        "logical_segment_count": len(segments),
        "encoded_frame_count": len(encoded),
        "segments": timeline,
        "encoded_segments": encoded_records,
        "opening_and_closing_equal_source_at_output_size": True,
        "lossless_decoded_pixels_exact": True,
        "decoded_durations_exact": True,
        "feather_applied_again": False,
        "motion": "discrete_drawing_substitution",
        "audio_synchronized": False,
        "semantic_acceptance": "not_granted",
    }


def build_combinations(
    source: Image.Image,
    donors: dict[str, Image.Image],
    masks: dict[str, FloatArray],
    spec: PortraitMotionSpec,
) -> tuple[dict[tuple[str, str], Image.Image], dict[str, object]]:
    """Compose every declared rest/eye/mouth combination with one blend per group."""
    original = _rgb(source, (spec.width, spec.height))
    if set(donors) != {state.state_id for state in spec.states}:
        raise ValueError("Donors must match the complete declared state set")
    if set(masks) - set(spec.requested_features):
        raise ValueError("Masks include features outside the requested set")
    supports, active_features = _supports(masks, original.size)
    base = np.asarray(original, dtype=np.uint8)
    alpha_by_group: dict[FeatureGroup, FloatArray] = {
        "eyes": np.zeros(base.shape[:2], dtype=np.float64),
        "mouth": np.zeros(base.shape[:2], dtype=np.float64),
    }
    for feature, alpha in masks.items():
        alpha_by_group["mouth" if feature == "mouth" else "eyes"] += alpha
    frames: dict[str, PixelArray] = {}
    group_states: dict[FeatureGroup, list[str]] = {"eyes": ["rest"], "mouth": ["rest"]}
    for state in spec.states:
        donor = donors[state.state_id]
        if donor.size != spec.panel_size:
            raise ValueError("Registered donors must remain at the declared panel size")
        alpha = alpha_by_group[state.feature_group]
        frame = np.asarray(composite(original, donor, alpha), dtype=np.uint8)
        resized_donor = np.asarray(
            donor.convert("RGB").resize(original.size, Image.Resampling.LANCZOS), dtype=np.uint8
        )
        if not np.array_equal(frame[alpha == 0], base[alpha == 0]):
            raise ValueError("Composition changed source pixels outside actual group alpha")
        if not np.array_equal(frame[alpha == 1], resized_donor[alpha == 1]):
            raise ValueError("Composition failed to fully replace an opaque core")
        frames[state.state_id] = frame
        group_states[state.feature_group].append(state.state_id)
    combinations: dict[tuple[str, str], Image.Image] = {}
    records: list[dict[str, object]] = []
    for eyes, mouth in product(group_states["eyes"], group_states["mouth"]):
        forward, reverse = base.copy(), base.copy()
        active = np.zeros(base.shape[:2], dtype=np.bool_)
        choices: tuple[tuple[FeatureGroup, str], ...] = (("eyes", eyes), ("mouth", mouth))
        for group, state_id in choices:
            if state_id != "rest":
                support = supports[group]
                forward[support] = frames[state_id][support]
                active |= support
        for group, state_id in reversed(choices):
            if state_id != "rest":
                support = supports[group]
                reverse[support] = frames[state_id][support]
        if not np.array_equal(forward, reverse):
            raise ValueError("Independent feature composition order changed pixels")
        if not np.array_equal(forward[~active], base[~active]):
            raise ValueError("Combination changed protected source pixels")
        combinations[(eyes, mouth)] = Image.fromarray(forward)
        records.append(
            {
                "eyes": eyes,
                "mouth": mouth,
                "active_support_pixels": int(active.sum()),
                "changed_source_pixels": int(np.any(forward != base, axis=2).sum()),
            }
        )
    return combinations, {
        "native_source_outside_active_alpha_exact": True,
        "native_donor_opaque_cores_exact": True,
        "composition_order_pixel_identical": True,
        "feature_masks_disjoint": True,
        "active_features": active_features,
        "inactive_groups": [group for group, names in active_features.items() if not names],
        "combinations": records,
        "semantic_acceptance": "not_granted",
    }


def encode_preview(
    combinations: dict[tuple[str, str], Image.Image], spec: PortraitMotionSpec
) -> tuple[bytes, dict[str, object]]:
    """Encode the declared timeline at panel size, verifying pixels and duration."""
    group_states: dict[FeatureGroup, list[str]] = {"eyes": ["rest"], "mouth": ["rest"]}
    state_groups: dict[str, FeatureGroup] = {}
    for state in spec.states:
        group_states[state.feature_group].append(state.state_id)
        state_groups[state.state_id] = state.feature_group
    if set(combinations) != set(product(group_states["eyes"], group_states["mouth"])):
        raise ValueError("Preview needs the complete declared Cartesian state combinations")
    source = _rgb(combinations[("rest", "rest")], (spec.width, spec.height))
    for image in combinations.values():
        _rgb(image, source.size)
    timeline = _timeline(spec.playback, state_groups)
    return _encode_selected(source, combinations, spec.playback, timeline, spec.panel_size)
