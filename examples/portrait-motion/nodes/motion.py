"""The motion steps: what may change, one drawn sheet, its fit, every combination, the verdict.

Every paid step's question is built here or in a prompt file, and every answer is held by a
judge to the rules the component's validators state. A valid refusal (no feature can move,
a drawing does not line up, no safe outline, a failed still review) is a result: it ends the
chain and the result step says where and why. A malformed answer is drawn again.
"""

from __future__ import annotations

import io
from typing import Any

import numpy as np
from PIL import Image, ImageDraw
from pydantic import ValidationError

from grida.fx import Ctx, node

from nodes._portrait_motion import PortraitMotionSpec
from nodes._portrait_motion.models import PortraitMotionResult
from nodes._portrait_motion.playback import build_combinations, encode_preview
from nodes._portrait_motion.processing import (
    geometry_masks,
    heatmap,
    make_guide,
    png_bytes,
    split_register,
)
from nodes._portrait_motion.review import (
    atlas_prompt,
    geometry_prompt,
    quality_prompt,
    validate_admission,
    validate_geometry,
    validate_quality,
)
from nodes.states import (
    combination_key,
    picture,
    reviewed,
    sha256,
    spec_of,
)

#: The largest canvas a native WebP preview of the whole sprite can hold.
_WEBP_EDGE = 16_383


def _png(ctx: Ctx, image: Image.Image) -> Any:
    return ctx.out.bytes(png_bytes(image), "image/png")


def _rejected(ctx: Ctx, errors: list[str]) -> dict[str, Any]:
    ctx.fact("errors", errors[:20])
    ctx.fact("verdict", "reject")
    return {}


def _problems(error: Exception) -> list[str]:
    if isinstance(error, ValidationError):
        return [f"{e['msg']} at {e['loc']}" for e in error.errors()]
    return [str(error)]


@node(
    "check",
    inputs={"portrait": "image", "spec": "json"},
    params={"face_crop": bool},
    outputs={"spec": "json"},
    version=1,
)
def check(ctx: Ctx) -> dict[str, Any]:
    """Hold the inputs to what the motion can use, before anything is paid for."""

    try:
        spec = spec_of(ctx.read.json("spec"))
    except ValidationError as error:
        raise ctx.fail(f"the spec is not a portrait-motion spec: {error}") from error
    with Image.open(io.BytesIO(ctx.read.bytes("portrait"))) as opened:
        opened.load()
        if opened.format != "PNG" or opened.mode not in {"RGB", "RGBA"}:
            raise ctx.fail("the portrait is an RGB or RGBA PNG")
        size, opaque = opened.size, opened.convert("RGBA").getchannel("A").getextrema()
    if ctx.params["face_crop"]:
        if spec.width != spec.height or spec.columns != spec.rows:
            raise ctx.fail("a face crop is square: the spec's canvas and grid are square")
        if max(size) > _WEBP_EDGE:
            raise ctx.fail(f"the portrait is wider or taller than {_WEBP_EDGE} pixels")
    else:
        if size != (spec.width, spec.height):
            raise ctx.fail(f"the portrait is {size[0]}x{size[1]}, not the spec's canvas")
        if opaque != (255, 255):
            raise ctx.fail("without a face crop, the portrait is opaque")
    ctx.fact("size", f"{spec.width}x{spec.height}")
    ctx.fact("requested_features", list(spec.requested_features))
    return {"spec": ctx.out.json(spec.model_dump(mode="json"))}


@node(
    "admitted",
    inputs={"admission": "json", "spec": "json"},
    judge=True,
    version=1,
)
def admitted(ctx: Ctx) -> dict[str, Any]:
    """Each requested feature has exactly one route; the direct ones may move."""

    spec = spec_of(ctx.read.json("spec"))
    try:
        decision = validate_admission(ctx.read.json("admission"), list(spec.requested_features))
    except (ValidationError, ValueError) as error:
        return _rejected(ctx, _problems(error))
    ctx.fact("eligible_features", decision["eligible_features"])
    ctx.fact("routes", {item["feature_id"]: item["route"] for item in decision["features"]})
    ctx.fact("verdict", "accept")
    return {}


@node(
    "guide",
    inputs={"portrait": "image", "spec": "json"},
    outputs={"guide": "image/png", "panel": "image/png", "coordinates": "image/png"},
    version=1,
)
def guide(ctx: Ctx) -> dict[str, Any]:
    """The sheet the image model edits: the portrait reduced into every cell. Beside it, one
    reduced copy, and that copy with a labelled pixel grid for outlining."""

    spec = spec_of(ctx.read.json("spec"))
    source = picture(ctx.read.bytes("portrait"))
    panel = source.resize(spec.panel_size, Image.Resampling.LANCZOS)
    coordinates = panel.copy()
    draw = ImageDraw.Draw(coordinates)
    step = max(16, min(spec.panel_size) // 8)
    for x in range(0, spec.panel_size[0], step):
        draw.line((x, 0, x, spec.panel_size[1]), fill=(0, 220, 220), width=1)
        draw.text((x + 2, 2), str(x), fill="white", stroke_width=1, stroke_fill="black")
    for y in range(0, spec.panel_size[1], step):
        draw.line((0, y, spec.panel_size[0], y), fill=(0, 220, 220), width=1)
        draw.text((2, y + 2), str(y), fill="white", stroke_width=1, stroke_fill="black")
    return {
        "guide": _png(ctx, make_guide(source, spec.columns, spec.rows)),
        "panel": _png(ctx, panel),
        "coordinates": _png(ctx, coordinates),
    }


@node(
    "briefs",
    inputs={"spec": "json"},
    params={"eligible": list},
    outputs={"atlas": "text/plain", "geometry": "text/plain", "quality": "text/plain"},
    version=1,
)
def briefs(ctx: Ctx) -> dict[str, Any]:
    """The three questions that depend on what was admitted: the sheet to draw, the outlines
    to trace, and the still review with its pictures named in order."""

    spec = spec_of(ctx.read.json("spec"))
    eligible = list(ctx.params["eligible"])
    states = [state.model_dump() for state in spec.states]
    labels = "\n".join(
        f"Image {index + 2}: eyes={eyes}, mouth={mouth}"
        for index, (eyes, mouth) in enumerate(reviewed(spec))
    )
    return {
        "atlas": ctx.out.text(atlas_prompt(spec, eligible)),
        "geometry": ctx.out.text(geometry_prompt(states, eligible, spec.panel_size)),
        "quality": ctx.out.text(
            quality_prompt(states, eligible)
            + "\nReference order:\nImage 1: original source\n"
            + labels
        ),
    }


@node(
    "registration",
    inputs={
        "portrait": "image",
        "atlas": "image",
        "panel": "image",
        "coordinates": "image",
        "spec": "json",
    },
    outputs={
        "donors": "image/png{}",
        "heatmaps": "image/png{}",
        "fits": "json",
        "review": "image[]",
    },
    version=1,
)
def registration(ctx: Ctx) -> dict[str, Any]:
    """Cut the sheet into its cells and align each to the portrait; a cell that moved, turned
    or changed scale fails the technical gate. ``review`` is what the outliner is shown, in
    order: the reduced copy, its grid, then each state's drawing and its difference map."""

    spec = spec_of(ctx.read.json("spec"))
    source = picture(ctx.read.bytes("portrait"))
    donors, fits = split_register(
        source,
        picture(ctx.read.bytes("atlas")),
        [state.state_id for state in spec.states],
        spec.columns,
        spec.rows,
    )
    panel = source.resize(spec.panel_size, Image.Resampling.LANCZOS)
    donor_out = {state: _png(ctx, donor) for state, donor in donors.items()}
    heat_out = {state: _png(ctx, heatmap(panel, donor)) for state, donor in donors.items()}
    passed = all(fit["status"] == "passed_technical_gate" for fit in fits.values())
    ctx.fact("passed", passed)
    review = [ctx.inputs["panel"], ctx.inputs["coordinates"]]
    for state in spec.states:
        review += [donor_out[state.state_id], heat_out[state.state_id]]
    return {
        "donors": donor_out,
        "heatmaps": heat_out,
        "fits": ctx.out.json(fits),
        "review": review,
    }


@node(
    "geometry_ok",
    inputs={"geometry": "json", "spec": "json"},
    params={"eligible": list},
    judge=True,
    version=1,
)
def geometry_ok(ctx: Ctx) -> dict[str, Any]:
    """Exactly the admitted features, on the reduced copy's canvas, as polygons whose masks
    are disjoint and fit; or a reasoned ``cannot_segment``."""

    spec = spec_of(ctx.read.json("spec"))
    try:
        parsed = validate_geometry(ctx.read.json("geometry"), list(ctx.params["eligible"]))
        if parsed["canvas_size"] != list(spec.panel_size):
            raise ValueError("the outline canvas differs from the registered drawings")
        if parsed["status"] == "pass":
            geometry_masks(
                parsed["features"],
                (spec.width, spec.height),
                spec.panel_size,
                spec.feather_panel_px,
            )
    except (ValidationError, ValueError) as error:
        return _rejected(ctx, _problems(error))
    ctx.fact("status", parsed["status"])
    ctx.fact("reason", parsed["reason"])
    ctx.fact("verdict", "accept")
    return {}


@node(
    "composition",
    inputs={
        "portrait": "image",
        "donors": "image{}",
        "fits": "json",
        "geometry": "json",
        "spec": "json",
    },
    params={"eligible": list},
    outputs={
        "states": "image/png{}",
        "masks": "image/png{}",
        "preview": "image/webp",
        "exactness": "json",
        "manifest": "json",
        "review": "image[]",
    },
    version=1,
)
def composition(ctx: Ctx) -> dict[str, Any]:
    """Every eyes-and-mouth combination: each group's drawing blended through its outline
    over the untouched portrait, checked to change nothing outside it. ``review`` is what the
    still review is shown: the portrait, then every combination but the rest."""

    spec = spec_of(ctx.read.json("spec"))
    eligible = list(ctx.params["eligible"])
    geometry = validate_geometry(ctx.read.json("geometry"), eligible)
    masks, _ = geometry_masks(
        geometry["features"], (spec.width, spec.height), spec.panel_size, spec.feather_panel_px
    )
    fits = ctx.read.json("fits")
    donor_files = ctx.inputs["donors"]
    donors = {
        state.state_id: picture(donor_files[state.state_id].read_bytes()) for state in spec.states
    }
    for state in spec.states:
        valid = np.zeros((spec.panel_size[1], spec.panel_size[0]), dtype=np.uint8)
        for y, left, right in fits[state.state_id]["valid_panel_rows"]:
            valid[y, left:right] = 255
        native_valid = (
            np.asarray(
                Image.fromarray(valid).resize((spec.width, spec.height), Image.Resampling.NEAREST)
            )
            == 255
        )
        for feature, alpha in masks.items():
            group = "mouth" if feature == "mouth" else "eyes"
            if group == state.feature_group and np.any((alpha > 0) & ~native_valid):
                raise ctx.fail(f"the {feature} outline reaches past {state.state_id}'s drawing")
    source = picture(ctx.read.bytes("portrait"))
    combinations, facts = build_combinations(source, donors, masks, spec)
    animation, playback = encode_preview(combinations, spec)
    states: dict[str, Any] = {}
    records = []
    for (eyes, mouth), image in combinations.items():
        data = png_bytes(image)
        key = combination_key(eyes, mouth)
        states[key] = ctx.out.bytes(data, "image/png")
        records.append({"eyes": eyes, "mouth": mouth, "key": key, "sha256": sha256(data)})
    mask_out = {
        feature: _png(ctx, Image.fromarray(np.rint(mask * 255).astype(np.uint8)))
        for feature, mask in masks.items()
    }
    manifest = {
        "schema_version": 2,
        "combinations": records,
        "preview_sha256": sha256(animation),
        "features": eligible,
        "semantic_acceptance": "pending_quality",
        "temporal_review": "not_performed",
        "playback": playback,
    }
    return {
        "states": states,
        "masks": mask_out,
        "preview": ctx.out.bytes(animation, "image/webp"),
        "exactness": ctx.out.json({**facts, **playback}),
        "manifest": ctx.out.json(manifest),
        "review": [
            ctx.inputs["portrait"],
            *(states[combination_key(*pair)] for pair in reviewed(spec)),
        ],
    }


@node(
    "quality_ok",
    inputs={"quality": "json"},
    params={"eligible": list},
    judge=True,
    version=1,
)
def quality_ok(ctx: Ctx) -> dict[str, Any]:
    """One verdict per admitted feature and an overall one; a valid fail is a result."""

    try:
        parsed = validate_quality(ctx.read.json("quality"), list(ctx.params["eligible"]))
    except (ValidationError, ValueError) as error:
        return _rejected(ctx, _problems(error))
    ctx.fact("status", parsed["status"])
    ctx.fact("reason", parsed["reason"])
    ctx.fact("verdict", "accept")
    return {}


def _result(
    spec: PortraitMotionSpec,
    *,
    location: dict[str, Any] | None,
    admission: dict[str, Any] | None,
    fits: dict[str, Any] | None,
    geometry: dict[str, Any] | None,
    quality: dict[str, Any] | None,
) -> PortraitMotionResult:
    requested: list[str] = list(spec.requested_features)
    eligible: list[str] = []
    refusal: tuple[str, str] | None = None
    if location is not None and location["status"] != "located":
        refusal = ("face", "Face not locatable: " + location["reason"])
    elif admission is None:
        raise ValueError("the admission decision is missing")
    else:
        eligible = validate_admission(admission, requested)["eligible_features"]
        if not eligible:
            refusal = ("admission", "No requested feature is directly usable")
        elif fits is None:
            raise ValueError("the registration record is missing")
        elif any(fit["status"] != "passed_technical_gate" for fit in fits.values()):
            refusal = ("registration", "Registration quality refused")
        elif geometry is None:
            raise ValueError("the outline answer is missing")
        elif (outlined := validate_geometry(geometry, eligible))["status"] != "pass":
            refusal = ("geometry", outlined["reason"])
        elif quality is None:
            raise ValueError("the still review is missing")
        elif (verdict := validate_quality(quality, eligible))["status"] != "pass":
            refusal = ("quality", verdict["reason"])
    if refusal is not None:
        return PortraitMotionResult(
            status="refused",
            reason=refusal[1],
            refused_at=refusal[0],  # type: ignore[arg-type]
            admitted_features=eligible,  # type: ignore[arg-type]
            accepted_features=[],
        )
    return PortraitMotionResult(
        status="complete" if set(eligible) == set(requested) else "partial",
        reason="All admitted features passed still-image quality; temporal review remains separate",
        refused_at=None,
        admitted_features=eligible,  # type: ignore[arg-type]
        accepted_features=eligible,  # type: ignore[arg-type]
    )


@node(
    "result",
    inputs={
        "spec": "json",
        "location": "json?",
        "admission": "json?",
        "fits": "json?",
        "geometry": "json?",
        "quality": "json?",
    },
    outputs={"result": "json"},
    version=1,
)
def result(ctx: Ctx) -> dict[str, Any]:
    """What the run accepted, read from the decisions it kept: complete, partial (some
    features could not move), or refused at the first stage that refused, with its reason."""

    def optional(name: str) -> dict[str, Any] | None:
        return ctx.read.json(name) if name in ctx.inputs else None

    try:
        made = _result(
            spec_of(ctx.read.json("spec")),
            location=optional("location"),
            admission=optional("admission"),
            fits=optional("fits"),
            geometry=optional("geometry"),
            quality=optional("quality"),
        )
    except ValueError as error:
        raise ctx.fail(f"the run's decisions do not add up: {error}") from error
    ctx.fact("status", made.status)
    ctx.fact("accepted_features", list(made.accepted_features))
    return {"result": ctx.out.json(made.model_dump(mode="json"))}


@node(
    "keep",
    inputs={"states": "image{}", "animation": "image", "manifest": "json"},
    outputs={"states": "image/png{}", "animation": "image/webp", "manifest": "json"},
    version=1,
)
def keep(ctx: Ctx) -> dict[str, Any]:
    """Hand on the combinations the still review passed, as they are."""

    return {
        "states": dict(ctx.inputs["states"]),
        "animation": ctx.inputs["animation"],
        "manifest": ctx.inputs["manifest"],
    }
