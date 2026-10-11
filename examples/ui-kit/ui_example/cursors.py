"""The cursor set: a fixed pointer vocabulary a game may restyle, each glyph with a hotspot.

A mouse cursor is an icon with one more fact: the pixel that is the pointer. The set is
built like the preview icon grid, and for the same reason — an image model draws a named
pointer arrow, a pointing hand or an hourglass dependably and a bespoke pointer not — so
the glyphs, their order, the grid and the hotspot rule per glyph belong to the layout, and
the authored prompt is style direction alone.

The hotspot is measured, never declared by hand and never read off the template. Each
glyph names the rule the gate applies to the alpha it actually drew: a pointer arrow's
hotspot is its tip, the topmost-leftmost opaque pixel; a pointing hand's is the fingertip,
the middle of the topmost opaque row; every symmetric shape's is the centre of its drawn
bounds. The manifest publishes the hotspot beside the cell in sheet pixels relative to the
cell's origin, so a consumer scales the cell to its cursor size and scales the hotspot by
the same factor. Whether the arrow reads as an arrow, and points where its rule assumes, is
the review's question; the evidence marks every measured hotspot on the glyph for it.

The grid fills its canvas the way the icon grid does, because that is the condition under
which the model honours a template at all: the first cut, eight cells as a small island inside
a wide margin on a 3:2 canvas, was ignored on every draw — the model laid the pointers out on a
grid of its own across the whole canvas, drawn well and registered nowhere. Nine pointers on a
3x3 grid tile a 1024 square with the icon grid's own margins; the ninth is the text I-beam,
the one remaining pointer every desktop knows.
"""

from __future__ import annotations

import io
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Literal, cast

from PIL import Image, ImageDraw, ImageFont

from ui_example.atlas import MASK_THRESHOLD, Rect, _checkerboard
from ui_example.icons import (
    ICON_ALPHA_POLICY,
    ICON_SCALE_MODE,
    IconGridRole,
    _canonicalize_glyph_sheet,
    validate_icon_sheet,
)

CURSOR_SET_LAYOUT = "cursor_grid_3x3_1024_v1"
#: A cursor is a glyph: the same alpha promise as the preview icons.
CURSOR_ALPHA_POLICY = ICON_ALPHA_POLICY
CURSOR_CANVAS = (1024, 1024)

HotspotRule = Literal["tip_top_left", "tip_top", "centre"]
HOTSPOT_RULES: tuple[HotspotRule, ...] = ("tip_top_left", "tip_top", "centre")

#: The fixed vocabulary, in reading order: name, the description the prompt states, and the
#: rule the gate measures the hotspot by. The descriptions say where the pointing part goes,
#: because the rule assumes it.
CURSOR_GLYPHS: tuple[tuple[str, str, HotspotRule], ...] = (
    (
        "arrow",
        "a classic pointer arrow, its tip at the upper left pointing up and to the left",
        "tip_top_left",
    ),
    (
        "hand",
        "a pointing hand with the index finger straight up, the fingertip at the top",
        "tip_top",
    ),
    ("grab", "a closed grabbing hand, the fingers curled into the palm", "centre"),
    ("crosshair", "a crosshair, a thin ring with four short ticks and an open centre", "centre"),
    ("inspect", "a magnifying glass", "centre"),
    ("busy", "an hourglass", "centre"),
    ("forbidden", "a circle with one diagonal slash", "centre"),
    ("move", "four arrows pointing outward from one centre", "centre"),
    ("text", "a text I-beam, one thin vertical bar with short serifs at both ends", "centre"),
)


@dataclass(frozen=True)
class CursorGridRole(IconGridRole):
    """An icon grid whose every glyph carries a hotspot rule.

    Geometry, template and registration gate are the icon grid's; the rule per glyph is
    the one addition, and it is part of the geometry record because it is part of what the
    prompt asks the model to draw (the tip at the upper left, the finger straight up).
    """

    hotspots: tuple[HotspotRule, ...] = ()

    def __post_init__(self) -> None:
        super().__post_init__()
        if len(self.hotspots) != len(self.glyphs):
            raise ValueError("cursor grid needs one hotspot rule per glyph")
        if any(rule not in HOTSPOT_RULES for rule in self.hotspots):
            raise ValueError("cursor grid hotspot rules must be one of " + ", ".join(HOTSPOT_RULES))

    def geometry_record(self) -> dict[str, object]:
        return {**super().geometry_record(), "hotspots": list(self.hotspots)}


CURSOR_SET = CursorGridRole(
    role="cursor_set",
    layout=CURSOR_SET_LAYOUT,
    glyphs=tuple(name for name, _, _ in CURSOR_GLYPHS),
    columns=3,
    rows=3,
    cell=288,
    gutter=48,
    margin=32,
    slack=16,
    canvas=CURSOR_CANVAS,
    hotspots=tuple(rule for _, _, rule in CURSOR_GLYPHS),
)

CURSOR_ROLES: dict[str, CursorGridRole] = {CURSOR_SET.role: CURSOR_SET}


def measure_hotspot(mask: Image.Image, cell: Rect, glyph_rect: Rect, rule: HotspotRule) -> Rect:
    """The hotspot one rule names on the glyph as drawn, in sheet pixels relative to ``cell``.

    ``mask`` is the whole sheet's thresholded alpha; ``glyph_rect`` its detected bounds. The
    tip rules read the topmost opaque row: its leftmost pixel for a pointer arrow, the middle
    of its run for a finger straight up. The centre rule is the bounds' centre. Returned as a
    one-pixel ``Rect`` so the record carries it in the same shape as every other geometry.
    """

    if rule == "centre":
        x = glyph_rect.x + glyph_rect.width // 2
        y = glyph_rect.y + glyph_rect.height // 2
    else:
        row = mask.crop(
            (glyph_rect.x, glyph_rect.y, glyph_rect.x + glyph_rect.width, glyph_rect.y + 1)
        )
        values = cast(Sequence[int], row.get_flattened_data())
        opaque = [index for index, value in enumerate(values) if value >= MASK_THRESHOLD]
        y = glyph_rect.y
        if rule == "tip_top_left":
            x = glyph_rect.x + opaque[0]
        else:
            x = glyph_rect.x + (opaque[0] + opaque[-1]) // 2
    return Rect(x - cell.x, y - cell.y, 1, 1)


def validate_cursor_sheet(data: bytes, role: CursorGridRole) -> dict[str, object]:
    """The icon gate, then one measured hotspot per registered glyph.

    Nothing new is refused here: the hotspot is a reading of what the gate already admitted,
    and whether the reading is where a player expects the pointer to be is the review's
    question, asked with the hotspot marked on the evidence.
    """

    facts = validate_icon_sheet(data, role)
    with Image.open(io.BytesIO(data)) as opened:
        mask = (
            opened.convert("RGBA")
            .getchannel("A")
            .point(lambda value: 255 if value >= MASK_THRESHOLD else 0)
        )
    cells = cast(list[dict[str, object]], facts["cells"])
    for entry, rule in zip(cells, role.hotspots, strict=True):
        cell = Rect(**cast(dict[str, int], entry["cell"]))
        glyph_rect = Rect(**cast(dict[str, int], entry["glyph_rect"]))
        hotspot = measure_hotspot(mask, cell, glyph_rect, rule)
        entry["hotspot_rule"] = rule
        entry["hotspot"] = {"x": hotspot.x, "y": hotspot.y}
    return {**facts, "alpha_policy": CURSOR_ALPHA_POLICY, "hotspot_rules": list(role.hotspots)}


def canonicalize_cursor_sheet(data: bytes, role: CursorGridRole) -> tuple[bytes, dict[str, object]]:
    """The icon grid's exterior normalization, measured with the cursor gate."""

    return _canonicalize_glyph_sheet(data, role, validate_cursor_sheet)


#: The two on-screen sizes the evidence shows each cursor at: a comfortable pointer on a
#: dense display and the size a desktop draws by default.
CURSOR_EVIDENCE_SIZES = (64, 32)
_HOTSPOT_MARK = (255, 40, 40, 255)


def cursor_evidence(data: bytes, facts: dict[str, object]) -> bytes:
    """Reviewer evidence: the sheet over a checkerboard, then one row per cell in reading
    order — the name and hotspot rule the cell was asked for, and the cell at two cursor
    sizes with its measured hotspot marked in red — so the judge checks that the arrow's
    tip and the finger's tip are where the rule read them. The names and marks are
    annotation drawn here, never part of the sheet."""

    with Image.open(io.BytesIO(data)) as opened:
        sheet = opened.convert("RGBA")
    cells = cast(list[dict[str, object]], facts["cells"])
    font = ImageFont.load_default(size=18)
    gap, row_h, name_w = 24, 80, 260
    right_w = name_w + sum(size + gap for size in CURSOR_EVIDENCE_SIZES)
    canvas = _checkerboard((sheet.width + gap + right_w, max(sheet.height, row_h * len(cells))))
    canvas.alpha_composite(sheet, (0, 0))
    draw = ImageDraw.Draw(canvas)
    for index, entry in enumerate(cells):
        rect = cast(dict[str, int], entry["cell"])
        hotspot = cast(dict[str, int], entry["hotspot"])
        cell = sheet.crop(Rect(rect["x"], rect["y"], rect["width"], rect["height"]).box)
        y = index * row_h
        draw.text(
            (sheet.width + gap, y + row_h // 2),
            f"{index + 1}. {entry['glyph']} ({entry['hotspot_rule']})",
            fill=(20, 20, 20, 255),
            font=font,
            anchor="lm",
        )
        x = sheet.width + gap + name_w
        for size in CURSOR_EVIDENCE_SIZES:
            sample = cell.resize((size, size), Image.Resampling.LANCZOS)
            top = y + (row_h - size) // 2
            canvas.alpha_composite(sample, (x, top))
            factor = size / rect["width"]
            hx, hy = x + round(hotspot["x"] * factor), top + round(hotspot["y"] * factor)
            draw.line((hx - 6, hy, hx + 6, hy), fill=_HOTSPOT_MARK, width=1)
            draw.line((hx, hy - 6, hx, hy + 6), fill=_HOTSPOT_MARK, width=1)
            x += size + gap
    stream = io.BytesIO()
    canvas.convert("RGB").save(stream, format="PNG", optimize=False)
    return stream.getvalue()


def cursor_role_contract(facts: dict[str, object]) -> dict[str, object]:
    """Project the resolved geometry the manifest binds beside the artifact: the icon
    grid's cells, each with the hotspot the gate measured on the drawn glyph."""

    return {
        "role": facts["role"],
        "layout": facts["layout"],
        "scale_mode": ICON_SCALE_MODE,
        "alpha_policy": CURSOR_ALPHA_POLICY,
        "draw_scale": CURSOR_ROLES[str(facts["role"])].draw_scale,
        "canvas": facts["canvas"],
        "cell_size": facts["cell_size"],
        "cells": [
            {
                "glyph": entry["glyph"],
                "cell": entry["cell"],
                "glyph_rect": entry["glyph_rect"],
                "hotspot_rule": entry["hotspot_rule"],
                "hotspot": entry["hotspot"],
            }
            for entry in cast(list[dict[str, object]], facts["cells"])
        ],
    }


__all__ = [
    "CURSOR_ALPHA_POLICY",
    "CURSOR_CANVAS",
    "CURSOR_GLYPHS",
    "CURSOR_ROLES",
    "CURSOR_SET",
    "CURSOR_SET_LAYOUT",
    "HOTSPOT_RULES",
    "CursorGridRole",
    "HotspotRule",
    "canonicalize_cursor_sheet",
    "cursor_evidence",
    "cursor_role_contract",
    "measure_hotspot",
    "validate_cursor_sheet",
]
