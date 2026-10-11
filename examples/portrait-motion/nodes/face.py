"""The face steps: where the face is, the square crop the motion works on, and the way back.

The locator answers only where the face is, never whether it can move. The crop is padded
on every side and resized once; its transform travels with it, so the accepted drawings go
back onto the original at its full size, at one integer offset, its transparency intact.
"""

from __future__ import annotations

import io
from typing import Any

import numpy as np
from PIL import Image

from grida.fx import Ctx, node

from nodes._portrait_motion.face_crop import create_working_crop
from nodes._portrait_motion.face_location import validate_location
from nodes._portrait_motion.face_playback import (
    build_face_combinations,
    encode_face_preview,
)
from nodes._portrait_motion.processing import png_bytes
from nodes.states import combination_key, sha256, spec_of

#: Context around the located face, as a share of its longest side, on every side.
PADDING_FRACTION = 0.35


def _original(data: bytes) -> Image.Image:
    with Image.open(io.BytesIO(data)) as opened:
        opened.load()
        return opened.copy()


@node("located", inputs={"location": "json"}, judge=True, version=1)
def located(ctx: Ctx) -> dict[str, Any]:
    """A box inside the picture, left above right and top above bottom, or a reasoned
    ``not_locatable``; nothing about whether the face can move."""

    try:
        location = validate_location(ctx.read.json("location"))
    except ValueError as error:
        ctx.fact("errors", [str(error)])
        ctx.fact("verdict", "reject")
        return {}
    ctx.fact("status", location["status"])
    ctx.fact("reason", location["reason"])
    ctx.fact("verdict", "accept")
    return {}


@node(
    "crop",
    inputs={"portrait": "image", "location": "json", "spec": "json"},
    outputs={"image": "image/png", "transform": "json"},
    version=1,
)
def crop(ctx: Ctx) -> dict[str, Any]:
    """Cut a square workspace around the face, padded, flattened on neutral grey, and
    resized once to the spec's canvas; keep the transform that maps it back."""

    location = validate_location(ctx.read.json("location"))
    if location["status"] != "located":
        raise ctx.fail("the face was not located")
    spec = spec_of(ctx.read.json("spec"))
    source = _original(ctx.read.bytes("portrait"))
    box = [
        value * source.size[index % 2] / 1000 for index, value in enumerate(location["bbox_xyxy"])
    ]
    work, transform = create_working_crop(source, box, (spec.width, spec.height), PADDING_FRACTION)
    return {
        "image": ctx.out.bytes(png_bytes(work), "image/png"),
        "transform": ctx.out.json(transform),
    }


@node(
    "render",
    inputs={
        "portrait": "image",
        "transform": "json",
        "donors": "image{}",
        "masks": "image{}",
        "spec": "json",
    },
    params={"features": list},
    outputs={
        "patches": "image/png{}",
        "states": "image/png{}",
        "animation": "image/webp",
        "exactness": "json",
        "manifest": "json",
    },
    version=1,
)
def render(ctx: Ctx) -> dict[str, Any]:
    """Each accepted feature's drawing as its own native patch, every combination at the
    original's full size, and the timeline as a lossless animation. Outside the patches every
    pixel, and the original's alpha everywhere, is checked unchanged."""

    spec = spec_of(ctx.read.json("spec"))
    transform = ctx.read.json("transform")
    features = list(ctx.params["features"])
    donor_files, mask_files = ctx.inputs["donors"], ctx.inputs["masks"]
    donors = {
        state.state_id: _original(donor_files[state.state_id].read_bytes()) for state in spec.states
    }
    masks = {}
    for feature in features:
        with Image.open(io.BytesIO(mask_files[feature].read_bytes())) as mask:
            masks[feature] = np.asarray(mask, dtype=np.float64) / 255
    source = _original(ctx.read.bytes("portrait"))
    frames = build_face_combinations(source, donors, masks, spec, transform)
    animation, playback = encode_face_preview(frames, spec)
    patches: dict[str, Any] = {}
    patch_records = []
    for (state, feature), image in frames.patches.items():
        data = png_bytes(image)
        key = f"{state}-{feature}"
        patches[key] = ctx.out.bytes(data, "image/png")
        patch_records.append(
            {"state_id": state, "feature_id": feature, "key": key, "sha256": sha256(data)}
        )
    states: dict[str, Any] = {}
    combination_records = []
    for (eyes, mouth), image in frames.combinations.items():
        data = png_bytes(image)
        key = combination_key(eyes, mouth)
        states[key] = ctx.out.bytes(data, "image/png")
        combination_records.append(
            {"eyes": eyes, "mouth": mouth, "key": key, "sha256": sha256(data)}
        )
    manifest = {
        "schema_version": 2,
        "kind": "portrait-face-motion-v2",
        "source_size": list(source.size),
        "offset_xy": list(frames.offset_xy),
        "patch_size": list(frames.crop_size),
        "features": features,
        "patches": patch_records,
        "combinations": combination_records,
        "animation_sha256": sha256(animation),
        "playback": playback,
        "timeline": [segment.model_dump(mode="json") for segment in spec.playback],
        "patch_application": "replace_selected_rgb_preserve_original_alpha",
        "feather_already_baked": True,
        "semantic_acceptance": "inherited_from_face_still_review",
        "temporal_review": "not_performed",
        "publication_authorized": False,
    }
    return {
        "patches": patches,
        "states": states,
        "animation": ctx.out.bytes(animation, "image/webp"),
        "exactness": ctx.out.json({**frames.exactness, **playback}),
        "manifest": ctx.out.json(manifest),
    }
