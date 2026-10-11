"""The UI sheet roles: geometry, gates, review questions and the manifest projection.

Every 2D game draws panels, buttons and a few system icons, so this is the one piece of
generation that is genuinely the same work in every genre: the geometry template, the
pixel gate and the review question do not know which genre asked for them. Three sheet
families share one shape. A nine-slice role (``panel_frame``, ``button_rect``) is gated by
slice reconstruction and published with insets; the preview icon grid is gated by cell
registration and published with glyph bounds; the cursor set is the icon grid's gate with
one measured hotspot per glyph. Only each family's own template, gate, evidence and review
question differ, and they are looked up from the role. The build steps that draw them are
``ui_example.steps``. Nothing here reads a game, a genre, or a camera.
"""

from __future__ import annotations

import json
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Literal

from pydantic import Field

from ui_example.atlas import (
    ATLAS_ALPHA_POLICY,
    ATLAS_ROLES,
    BUTTON_RECT,
    PANEL_FRAME,
    AtlasRole,
    atlas_evidence,
    atlas_role_contract,
    canonicalize_atlas_image,
    render_atlas_template,
    validate_atlas_image,
)
from ui_example.contracts import PersistedContractModel
from ui_example.cursors import (
    CURSOR_ALPHA_POLICY,
    CURSOR_GLYPHS,
    CURSOR_ROLES,
    CURSOR_SET,
    CursorGridRole,
    canonicalize_cursor_sheet,
    cursor_evidence,
    cursor_role_contract,
    validate_cursor_sheet,
)
from ui_example.icons import (
    ICON_ALPHA_POLICY,
    ICON_ROLES,
    PREVIEW_ICON_GLYPHS,
    PREVIEW_ICONS,
    IconGridRole,
    canonicalize_icon_sheet,
    icon_evidence,
    icon_role_contract,
    render_icon_template,
    validate_icon_sheet,
)
from ui_example.input_contracts import SNAKE_ID_PATTERN
from ui_example.models import (
    AtlasRoleDirection,
    CursorSetDirection,
    IconSetDirection,
    UiArtwork,
)

#: The generate node's cache contract: what the model is asked to paint. Bumping it
#: re-bills every sheet, so measured facts that only the local gate reads live under
#: the validation version below instead.
UI_ATLAS_CONTRACT_VERSION = "prepared-ui-atlas-v1"
#: The validate node's own identity, so a richer record (a new measured fact) can re-run
#: the local gate over cached sheets without re-billing the image above.
UI_ATLAS_VALIDATION_VERSION = "prepared-ui-atlas-validation-v2"
UI_ATLAS_REVIEW_VERSION = "prepared-ui-atlas-review-v1"
UI_ATLAS_EVIDENCE_VERSION = "prepared-ui-atlas-evidence-v1"
#: The structured card the review node declares and the review request sends. One name,
#: so a reader of the plan and a reader of the provider call see the same contract.
UI_ATLAS_REVIEW_SCHEMA_NAME = "prepared_ui_atlas_review"

UI_ATLAS_RAW_KIND = "ui-atlas-raw-v1"
UI_ATLAS_IMAGE_KIND = "ui-atlas-v1"
UI_ATLAS_VALIDATION_KIND = "ui-atlas-validation-v1"
UI_ATLAS_EVIDENCE_KIND = "ui-atlas-evidence-v1"
UI_ATLAS_VERDICT_KIND = "review-verdict-v1"

#: Every type this module owns, for a recipe's own type census and registry checks.
#: Any sheet family's role: what the triplet is fanned out over.
UiSheetRole = AtlasRole | IconGridRole | CursorGridRole

#: Every role the triplet can generate, by name. A node's ``role`` param resolves here.
UI_SHEET_ROLES: dict[str, UiSheetRole] = {**ATLAS_ROLES, **ICON_ROLES, **CURSOR_ROLES}

#: The roles a `game-ui-v5` document requires, in the order a graph fans them out: the
#: two nine-slice roles promoted in `game-ui-v2` and the preview icon set.
DEFAULT_ATLAS_ROLES: tuple[UiSheetRole, ...] = (PANEL_FRAME, BUTTON_RECT, PREVIEW_ICONS)

#: The direction a document authors for a role, by family.
UiSheetDirection = AtlasRoleDirection | IconSetDirection | CursorSetDirection


def document_roles(ui: UiArtwork) -> tuple[UiSheetRole, ...]:
    """The sheet roles one document plans: the three every document requires, then the
    optional ones it declares, in fan-out order. A host fans the triplet out over this, so
    a declared role is never silently left undrawn and an undeclared one is never billed."""

    return tuple(
        role for role in (*DEFAULT_ATLAS_ROLES, CURSOR_SET) if getattr(ui, role.role) is not None
    )


# ------------------------------------------------------------------ prompt


_GEOMETRY_COMMON = (
    "Use the supplied layout template as the exact geometry authority. It is layout guidance, "
    "not the requested visual style: do not draw its magenta, yellow, or cyan. Magenta marks "
    "where the opaque body goes, the yellow outline is the body's outer edge, and the cyan lines "
    "divide each body into nine regions. Keep the canvas exterior transparent alpha 0 with no "
    "glow, drop shadow, colour wash, backdrop, or scenery; every body must be fully opaque. "
    "No text, numbers, labels, icons, items, characters, logo, signature, or watermark."
)

_NINE_SLICE_RULE = (
    "Nine-slice rule: the four corner regions may carry ornament, and every corner ornament must "
    "stay inside its own corner region without touching or crossing the cyan divider. The four "
    "edge regions must each be one straight, uniform band that repeats identically along its "
    "whole length, with no motif, emblem, medallion, knot, notch, or break anywhere in the band. "
    "The centre region must be one flat, even, unornamented surface that text can sit on, with no "
    "vignette, gradient, hotspot, texture cluster, or pattern. Nothing crosses from a corner into "
    "an edge band except the continuous border itself."
)

_STATE_LOOKS = {
    "hover": "Hover is slightly brighter with a gentle highlight.",
    "pressed": "Pressed is darker with an inset, pushed-in look.",
    "disabled": "Disabled is desaturated and dimmer.",
}


def atlas_content_task(role: AtlasRole, direction: str) -> str:
    """The content task for one atlas role, read off its geometry rather than its name."""

    width, height = role.canvas
    if len(role.cells) == 1:
        body = (
            "Create one nine-slice panel frame for the game's screen-fixed interface.\n"
            f"Authored direction: {direction}\n"
            f"One {width} by {height} canvas holding one rectangular panel body at the "
            "template's magenta rectangle. "
        )
    else:
        looks = " ".join(_STATE_LOOKS[state] for state in role.states if state in _STATE_LOOKS)
        body = (
            f"Create one {role.role.replace('_', ' ')} state sheet for the game's screen-fixed "
            "interface.\n"
            f"Authored direction: {direction}\n"
            f"One {width} by {height} canvas holding {len(role.cells)} identical rectangular "
            "bodies stacked top to bottom at the template's magenta rectangles, in this order: "
            f"{', '.join(role.states)}. All share exactly the same outer silhouette, size, "
            "corner shape, border and ornament; only lighting, value, and hue change between "
            f"states. {looks} Each body is itself a nine-slice. "
        )
    return f"{body}{_NINE_SLICE_RULE} {_GEOMETRY_COMMON}"


def atlas_review_prompt(role: AtlasRole, direction: str, band_fill: object) -> str:
    """What the judge is asked, given what the pixel gate has already proved."""

    bodies = len(role.cells)
    states_clause = (
        ""
        if bodies == 1
        else (
            f"that the {bodies} bodies read as the states {', '.join(role.states)} in "
            "that order from top to bottom, "
        )
    )
    return (
        f"Review the generated {role.role} nine-slice sheet against its authored "
        "direction. Image 1 shows the sheet over a checkerboard on the left and, on the "
        "right, every body re-drawn through the admitted nine-slice at a wider and a "
        "taller size, which is exactly what the game will show; remaining images are "
        "authored visual references. Deterministic pixel validation has already proved "
        f"a transparent exterior, {bodies} fully opaque "
        f"{'body' if bodies == 1 else 'bodies in order'}, repeatable edge bands under "
        f"{band_fill} fill, a flat readable centre, and one silhouette across states. "
        "Do not mistake the checkerboard for artwork. Judge style coherence with the "
        "references, that ornament lives in the corners while the edge bands stay "
        "plain, that the centre is a quiet surface text can sit on, "
        f"{states_clause}"
        "and the absence of text, pseudo-text, labels, icons, items, logos, or "
        f"scenery. Authored direction: {direction} Uncertainty must not be "
        "called accept."
    )


_ICON_GEOMETRY = (
    "Use the supplied layout template as the exact geometry authority. It is layout "
    "guidance, not the requested visual style: do not draw its cyan or yellow. Each cyan "
    "square is one cell of the grid; the yellow square inside it is the extent the glyph "
    "should fill. Draw one glyph centred in each cell and nothing at all between or around "
    "the cells: no cell backgrounds, plates, tiles, frames, badges, shadows, or glow behind "
    "or around the icons, because these are glyphs and not buttons. Keep the canvas exterior "
    "transparent alpha 0. Every glyph must have a fully opaque body with clean edges. No "
    "text, letters, numbers, labels, logo, signature, or watermark anywhere."
)


def icon_content_task(role: IconGridRole, direction: str) -> str:
    """The content task for the icon grid: the fixed vocabulary, then the authored style.

    The glyph list is stated symbol by symbol in reading order, because that list is the
    contract a consumer indexes into; the direction may colour it and nothing more.
    """

    described = dict(PREVIEW_ICON_GLYPHS)
    listing = ", ".join(
        f"{index + 1} {name.replace('_', ' ')} ({described[name]})"
        for index, name in enumerate(role.glyphs)
    )
    width, height = role.canvas
    return (
        f"Create one icon set sheet for the game's screen-fixed interface: {len(role.glyphs)} "
        f"icons in a {role.columns} by {role.rows} grid on one {width} by {height} canvas, "
        "in reading order left to right then top to bottom: "
        f"{listing}. Each icon is one bold, instantly readable symbol that stays legible at "
        "24 pixels: simple, centred in its cell, filling about seventy percent of the cell, "
        "with the same visual weight, stroke thickness, and size across the whole set.\n"
        f"Style direction: {direction}\n{_ICON_GEOMETRY}"
    )


def icon_review_prompt(role: IconGridRole, direction: str) -> str:
    """What the judge is asked about an icon sheet: identity, one set, and nothing else."""

    return (
        f"Review the generated {role.role} icon sheet against its style direction. Image 1 "
        "shows the sheet over a checkerboard on the left and, on the right, one row per cell "
        "in reading order: the glyph the cell was asked to hold, then that cell re-drawn at "
        "the two sizes a game shows it. The names on the right are annotation added by the "
        "validator for you, not part of the sheet. Remaining images are authored visual "
        "references. Deterministic pixel validation has already proved a transparent canvas, "
        f"exactly {len(role.glyphs)} glyphs registered to the grid with nothing drawn between "
        "the cells, and one coherent size across the set. Do not mistake the checkerboard for "
        "artwork. Judge: that every cell reads unmistakably as its named glyph, listing each "
        "mismatch as an issue in the form 'cell <n> <name>: <what it shows instead>'; that all "
        "glyphs share one style, stroke weight and visual density, as one set drawn by one "
        "hand; style coherence with the references and the direction; that no cell carries a "
        "background plate, tile, frame, badge, or shadow behind its glyph; and the absence of "
        f"text, letters, numbers, or labels on the sheet itself. Style direction: {direction} "
        "Uncertainty must not be called accept."
    )


def cursor_content_task(role: CursorGridRole, direction: str) -> str:
    """The content task for the cursor set: the fixed vocabulary with where each pointer's
    pointing part goes, then the authored style. The placement clauses are stated because
    the hotspot rule the gate applies assumes them."""

    described = {name: text for name, text, _ in CURSOR_GLYPHS}
    listing = ", ".join(
        f"{index + 1} {name.replace('_', ' ')} ({described[name]})"
        for index, name in enumerate(role.glyphs)
    )
    width, height = role.canvas
    return (
        f"Create one mouse cursor set sheet for the game's screen-fixed interface: "
        f"{len(role.glyphs)} cursors in a {role.columns} by {role.rows} grid on one {width} by "
        f"{height} canvas, in reading order left to right then top to bottom: {listing}. Each "
        "cursor is one bold, instantly readable pointer shape that stays legible at 32 pixels: "
        "simple, centred in its cell, filling about seventy percent of the cell, with the same "
        "visual weight, stroke thickness, and size across the whole set, and its pointing part "
        "placed exactly as described so the pointer's hotspot lands where a player expects.\n"
        f"Style direction: {direction}\n{_ICON_GEOMETRY}"
    )


def cursor_review_prompt(role: CursorGridRole, direction: str) -> str:
    """What the judge is asked about a cursor sheet: identity, the pointing part where the
    hotspot rule assumes it, one set, and nothing else."""

    return (
        f"Review the generated {role.role} mouse cursor sheet against its style direction. "
        "Image 1 shows the sheet over a checkerboard on the left and, on the right, one row "
        "per cell in reading order: the cursor the cell was asked to hold with its hotspot "
        "rule, then that cell re-drawn at the two sizes a desktop shows a pointer, with the "
        "hotspot the validator measured marked as a small red cross. The names and red marks "
        "on the right are annotation added by the validator for you, not part of the sheet. "
        "Remaining images are authored visual references. Deterministic pixel validation has "
        f"already proved a transparent canvas, exactly {len(role.glyphs)} glyphs registered to "
        "the grid with nothing drawn between the cells, and one coherent size across the set. "
        "Do not mistake the checkerboard for artwork. Judge: that every cell reads "
        "unmistakably as its named cursor, listing each mismatch as an issue in the form "
        "'cell <n> <name>: <what it shows instead>'; that the red mark sits where a player "
        "expects the pointer to be — on the arrow's tip, on the pointing finger's tip, at the "
        "centre of every symmetric shape — listing each miss the same way; that all cursors "
        "share one style, stroke weight and visual density, as one set drawn by one hand; "
        "style coherence with the references and the direction; that no cell carries a "
        "background plate, tile, frame, badge, or shadow behind its glyph; and the absence of "
        f"text, letters, numbers, or labels on the sheet itself. Style direction: {direction} "
        "Uncertainty must not be called accept."
    )


# ------------------------------------------------------------------ family


@dataclass(frozen=True)
class SheetFamily:
    """What differs between a nine-slice sheet, an icon grid and a cursor set, looked up
    from the role.

    Everything else about the triplet — node types, ids, ports, cache identity, provider
    request shape, manifest binding — is one code path.
    """

    direction_type: type[UiSheetDirection]
    template: Callable[[Any], bytes]
    validate: Callable[[bytes, Any], dict[str, object]]
    canonicalize: Callable[[bytes, Any], tuple[bytes, dict[str, object]]]
    evidence: Callable[[bytes, dict[str, object]], bytes]
    contract: Callable[[dict[str, object]], dict[str, object]]
    content_task: Callable[[Any, str], str]
    #: The review prompt from the role, the authored direction, and the validation record;
    #: at plan time the record is empty and the prompt names what it will be given.
    review_prompt: Callable[[Any, str, Mapping[str, object]], str]
    review_checks: tuple[str, ...]
    alpha_policy: str
    generate_description: str
    validate_description: str
    review_description: str
    canonical_prompt: str
    evidence_prompt: str


NINE_SLICE_FAMILY = SheetFamily(
    direction_type=AtlasRoleDirection,
    template=render_atlas_template,
    validate=validate_atlas_image,
    canonicalize=canonicalize_atlas_image,
    evidence=atlas_evidence,
    contract=atlas_role_contract,
    content_task=atlas_content_task,
    review_prompt=lambda role, direction, record: atlas_review_prompt(
        role, direction, record.get("band_fill", "the admitted")
    ),
    review_checks=(
        "style_coherence",
        "ornament_in_corners",
        "bands_plain",
        "centre_quiet",
        "state_order",
        "text_free",
    ),
    alpha_policy=ATLAS_ALPHA_POLICY,
    generate_description="generate the authored {role} nine-slice atlas",
    validate_description="detect bodies, admit a band fill, and normalize the alpha boundary",
    review_description="review {role} style, ornament placement, and state order",
    canonical_prompt=(
        "Normalize only the admitted alpha boundary: clear the already-transparent "
        "exterior and clamp every admitted content rect to alpha 255."
    ),
    evidence_prompt=(
        "Composite the atlas sheet over a checkerboard and re-draw every cell through "
        "the admitted nine-slice at a wider and a taller size for review evidence."
    ),
)

ICON_GRID_FAMILY = SheetFamily(
    direction_type=IconSetDirection,
    template=render_icon_template,
    validate=validate_icon_sheet,
    canonicalize=canonicalize_icon_sheet,
    evidence=icon_evidence,
    contract=icon_role_contract,
    content_task=icon_content_task,
    review_prompt=lambda role, direction, _record: icon_review_prompt(role, direction),
    review_checks=(
        "style_coherence",
        "glyph_identity",
        "one_set",
        "glyphs_only",
        "text_free",
    ),
    alpha_policy=ICON_ALPHA_POLICY,
    generate_description="generate the fixed {role} glyph grid in the authored style",
    validate_description="register a glyph to every cell and normalize the exterior alpha",
    review_description="review {role} glyph identity, set coherence, and style",
    canonical_prompt=(
        "Normalize only the admitted exterior: clear already-transparent pixels to alpha 0 "
        "and touch nothing inside a glyph."
    ),
    evidence_prompt=(
        "Composite the icon sheet over a checkerboard and draw every cell at two consumer "
        "sizes beside the glyph name it was asked to hold, for review evidence."
    ),
)


CURSOR_GRID_FAMILY = SheetFamily(
    direction_type=CursorSetDirection,
    template=render_icon_template,
    validate=validate_cursor_sheet,
    canonicalize=canonicalize_cursor_sheet,
    evidence=cursor_evidence,
    contract=cursor_role_contract,
    content_task=cursor_content_task,
    review_prompt=lambda role, direction, _record: cursor_review_prompt(role, direction),
    review_checks=(
        "style_coherence",
        "glyph_identity",
        "hotspot_placement",
        "one_set",
        "glyphs_only",
        "text_free",
    ),
    alpha_policy=CURSOR_ALPHA_POLICY,
    generate_description="generate the fixed {role} pointer grid in the authored style",
    validate_description=(
        "register a pointer to every cell, measure its hotspot, and normalize the exterior alpha"
    ),
    review_description=(
        "review {role} pointer identity, hotspot placement, set coherence, and style"
    ),
    canonical_prompt=(
        "Normalize only the admitted exterior: clear already-transparent pixels to alpha 0 "
        "and touch nothing inside a glyph."
    ),
    evidence_prompt=(
        "Composite the cursor sheet over a checkerboard and draw every cell at two pointer "
        "sizes beside the cursor name it was asked to hold, with the measured hotspot "
        "marked, for review evidence."
    ),
)


def sheet_family(role: UiSheetRole) -> SheetFamily:
    """The family a role belongs to; a role is exactly one of them by construction."""

    if isinstance(role, CursorGridRole):
        return CURSOR_GRID_FAMILY
    return ICON_GRID_FAMILY if isinstance(role, IconGridRole) else NINE_SLICE_FAMILY


def validate_ui_sheet(data: bytes, role: str) -> dict[str, object]:
    """Gate ``data`` as the named role, whichever family it belongs to.

    For a host that re-checks a cached sheet against the contract before reusing it.
    """

    sheet_role = UI_SHEET_ROLES[role]
    return sheet_family(sheet_role).validate(data, sheet_role)


# ------------------------------------------------------------------- graph


def ui_atlas_review_schema(
    checks: Sequence[str] = NINE_SLICE_FAMILY.review_checks,
) -> dict[str, object]:
    """The judge's answer shape: the questions the pixel gate cannot decide, per family."""

    check_properties = {key: {"type": "boolean"} for key in checks}
    return {
        "type": "object",
        "properties": {
            "verdict": {"type": "string", "enum": ["accept", "reject", "uncertain"]},
            "confidence": {"type": "number"},
            "checks": {"type": "object", "properties": check_properties},
            "issues": {"type": "array", "items": {"type": "string"}},
            "evidence": {"type": "string"},
        },
    }


def _parse_review(value: object) -> dict[str, object]:
    if not isinstance(value, dict) or value.get("verdict") not in {
        "accept",
        "reject",
        "uncertain",
    }:
        raise ValueError("UI atlas review has an invalid verdict")
    return value


# ---------------------------------------------------------------- manifest


class AtlasRect(PersistedContractModel):
    """One published rectangle, in sheet pixels."""

    x: int = Field(ge=0)
    y: int = Field(ge=0)
    width: int = Field(ge=1)
    height: int = Field(ge=1)


class AtlasCanvas(PersistedContractModel):
    width: int = Field(ge=1)
    height: int = Field(ge=1)


class AtlasInsets(PersistedContractModel):
    """The admitted corner widths, which is where a consumer cuts the nine slices."""

    left: int = Field(ge=1)
    top: int = Field(ge=1)
    right: int = Field(ge=1)
    bottom: int = Field(ge=1)


class AtlasCellLayout(PersistedContractModel):
    state: str = Field(pattern=SNAKE_ID_PATTERN, max_length=32)
    cell: AtlasRect
    #: The geometric interior: the cell minus the sheet insets.
    content_rect: AtlasRect
    #: The measured ornament-free interior, where text is safe. Inside ``content_rect``.
    safe_rect: AtlasRect


class AtlasRoleLayout(PersistedContractModel):
    """The resolved geometry one role publishes, as a typed contract.

    The same projection ``ui_atlas_manifest`` emits as a plain mapping, for consumers whose
    manifest is a validated document rather than a JSON blob. Detected, not declared: the
    model keeps a sheet's cell count and order but re-spaces its bodies, so these numbers
    come from the gate that measured the artwork.
    """

    role: str = Field(pattern=SNAKE_ID_PATTERN, max_length=64)
    layout: str = Field(pattern=SNAKE_ID_PATTERN, max_length=96)
    scale_mode: Literal["nine_slice"]
    alpha_policy: Literal["transparent_exterior_opaque_body_v1"]
    band_fill: Literal["stretch", "tile"]
    #: Sheet pixels per screen pixel: lay slices out at this multiple, then scale down.
    draw_scale: int = Field(ge=1)
    canvas: AtlasCanvas
    insets: AtlasInsets
    cells: list[AtlasCellLayout] = Field(min_length=1, max_length=16)


class IconCellLayout(PersistedContractModel):
    glyph: str = Field(pattern=SNAKE_ID_PATTERN, max_length=32)
    #: The published cell: what a consumer cuts as one frame and scales as the glyph's box.
    cell: AtlasRect
    #: The detected bounds of the drawn glyph, inside ``cell``.
    glyph_rect: AtlasRect


class IconSetLayout(PersistedContractModel):
    """The resolved geometry the icon grid publishes, as a typed contract.

    ``scale_mode`` is ``fixed``: a cell is drawn at one size, never sliced. ``cell_size`` is
    every cell's side, so a consumer sizes an icon by scaling its cell and keeps the set's
    own proportions between glyphs.
    """

    role: str = Field(pattern=SNAKE_ID_PATTERN, max_length=64)
    layout: str = Field(pattern=SNAKE_ID_PATTERN, max_length=96)
    scale_mode: Literal["fixed"]
    alpha_policy: Literal["transparent_exterior_opaque_glyph_v1"]
    draw_scale: int = Field(ge=1)
    canvas: AtlasCanvas
    cell_size: int = Field(ge=1)
    cells: list[IconCellLayout] = Field(min_length=1, max_length=64)


class AtlasHotspot(PersistedContractModel):
    """One pixel, in sheet pixels relative to its cell's origin."""

    x: int = Field(ge=0)
    y: int = Field(ge=0)


class CursorCellLayout(IconCellLayout):
    """An icon cell with the pointer's hotspot: the rule it was read by and the pixel, in
    sheet pixels relative to the cell's origin, that a consumer scales with the cell."""

    hotspot_rule: Literal["tip_top_left", "tip_top", "centre"]
    hotspot: AtlasHotspot


class CursorSetLayout(PersistedContractModel):
    """The resolved geometry the cursor set publishes: the icon grid's, cell by cell with a
    measured hotspot. A consumer cuts a cell, scales it to its pointer size, and scales the
    hotspot by the same factor."""

    role: str = Field(pattern=SNAKE_ID_PATTERN, max_length=64)
    layout: str = Field(pattern=SNAKE_ID_PATTERN, max_length=96)
    scale_mode: Literal["fixed"]
    alpha_policy: Literal["transparent_exterior_opaque_glyph_v1"]
    draw_scale: int = Field(ge=1)
    canvas: AtlasCanvas
    cell_size: int = Field(ge=1)
    cells: list[CursorCellLayout] = Field(min_length=1, max_length=64)


#: Any family's published block: the nine-slice and icon blocks are told apart by
#: ``scale_mode``, the cursor block by the hotspot on every cell.
UiSheetLayout = AtlasRoleLayout | IconSetLayout | CursorSetLayout


def ui_atlas_manifest(
    role: str,
    *,
    read_validation: Callable[[str], bytes],
    publish: Callable[[str], object],
    publish_provenance: Callable[[str], None],
) -> dict[str, object]:
    """One published ``ui.<role>`` block, identical in every consumer's manifest.

    The validate node is the only place the detected geometry exists, so this reads the
    resolved contract from its record rather than from the declared template.
    """

    validation_path = f"ui/{role}.validation.json"
    publish_provenance(validation_path)
    record = json.loads(read_validation(validation_path))
    if not isinstance(record, dict) or record.get("role") != role:
        raise ValueError(f"UI atlas {role} validation names a different role")
    try:
        contract = sheet_family(UI_SHEET_ROLES[role]).contract(record)
    except (KeyError, TypeError) as error:
        raise ValueError(f"UI atlas {role} validation lacks resolved geometry: {error}") from error
    return {**contract, "asset": publish(f"ui/{role}.png")}


def ui_atlas_manifest_block(
    *,
    read_validation: Callable[[str], bytes],
    publish: Callable[[str], object],
    publish_provenance: Callable[[str], None],
    roles: Sequence[UiSheetRole] = DEFAULT_ATLAS_ROLES,
) -> dict[str, object]:
    """Every generated role as one ``ui`` block."""

    return {
        role.role: ui_atlas_manifest(
            role.role,
            read_validation=read_validation,
            publish=publish,
            publish_provenance=publish_provenance,
        )
        for role in roles
    }


# ----------------------------------------------------------------- helpers


__all__ = [
    "CURSOR_GRID_FAMILY",
    "DEFAULT_ATLAS_ROLES",
    "ICON_GRID_FAMILY",
    "NINE_SLICE_FAMILY",
    "AtlasCanvas",
    "AtlasCellLayout",
    "AtlasHotspot",
    "AtlasInsets",
    "AtlasRect",
    "AtlasRoleLayout",
    "CursorCellLayout",
    "CursorSetLayout",
    "IconCellLayout",
    "IconSetLayout",
    "UI_SHEET_ROLES",
    "UI_ATLAS_CONTRACT_VERSION",
    "UI_ATLAS_EVIDENCE_KIND",
    "UI_ATLAS_EVIDENCE_VERSION",
    "UI_ATLAS_IMAGE_KIND",
    "UI_ATLAS_RAW_KIND",
    "UI_ATLAS_REVIEW_SCHEMA_NAME",
    "UI_ATLAS_REVIEW_VERSION",
    "UI_ATLAS_VALIDATION_KIND",
    "UI_ATLAS_VALIDATION_VERSION",
    "UI_ATLAS_VERDICT_KIND",
    "SheetFamily",
    "UiSheetDirection",
    "UiSheetLayout",
    "UiSheetRole",
    "atlas_content_task",
    "atlas_review_prompt",
    "cursor_content_task",
    "cursor_review_prompt",
    "document_roles",
    "icon_content_task",
    "icon_review_prompt",
    "sheet_family",
    "ui_atlas_manifest",
    "ui_atlas_manifest_block",
    "ui_atlas_review_schema",
    "validate_ui_sheet",
]
