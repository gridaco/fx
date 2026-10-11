"""The preview icon set: a fixed glyph grid that a game may restyle but not redefine.

An image model draws well-known symbols — play, pause, a gear, a heart — reliably when it
is asked for them by name on a plain grid, and draws bespoke symbols unreliably however
carefully they are described. So this role does not let a game describe its icons. The
glyph list, their order and the grid are the contract; the authored prompt may say only
how they should look. That is the highest-yield way to give every genre a first icon set,
and it is named ``preview`` because it is exactly that: the set will be rewritten, most
likely as several declared families, once the games being generated need more than this.

Geometry follows the nine-slice roles' discipline. The producer renders the grid template
from the declared record, the gate proves what the format promises from alpha alone —
one glyph registered to each cell, nothing drawn between cells, one coherent size across
the set — and the manifest publishes the detected glyph bounds beside the declared cell,
so a consumer scales a cell and never rediscovers geometry from pixels. Whether cell nine
actually reads as a magnifying glass is the review's question, not the gate's.
"""

from __future__ import annotations

import io
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any, cast

from PIL import Image, ImageChops, ImageDraw, ImageFont

from ui_example.atlas import (
    MASK_THRESHOLD,
    TRANSPARENT_ADMISSION_MAX,
    Rect,
    _checkerboard,
)

ICON_SCALE_MODE = "fixed"
ICON_ALPHA_POLICY = "transparent_exterior_opaque_glyph_v1"
PREVIEW_ICONS_LAYOUT = "icon_grid_4x4_1024_preview_v1"
ICON_CANVAS = 1024

#: Alpha at or below this is exterior; the same boundary the nine-slice roles admit.
ICON_TRANSPARENT_ADMISSION_MAX = TRANSPARENT_ADMISSION_MAX
#: A glyph sheet is mostly air; a canvas that is not was painted with plates or a backdrop.
ICON_MIN_TRANSPARENT_FRACTION = 0.5
#: Every glyph must have a fully opaque core; antialiased edges are expected around it.
ICON_GLYPH_ALPHA_MAX_MIN = 250
#: Opaque coverage of the guide cell: below is a speck or an empty cell, above is a plate.
ICON_GLYPH_COVER_MIN = 0.02
ICON_GLYPH_COVER_MAX = 0.85
#: The glyph's larger dimension as a fraction of the guide cell.
ICON_GLYPH_EXTENT_MIN = 0.30
ICON_GLYPH_EXTENT_MAX = 1.0
#: Largest over smallest glyph extent across the set: one coherent size, not one hero icon.
ICON_SET_EXTENT_RATIO_MAX = 2.5

_YELLOW = (255, 255, 0, 255)
_CYAN = (0, 255, 255, 255)

#: The fixed vocabulary, in reading order, each with the description the prompt states.
PREVIEW_ICON_GLYPHS: tuple[tuple[str, str], ...] = (
    ("play", "a right-pointing triangle"),
    ("pause", "two vertical bars"),
    ("close", "an X cross"),
    ("menu", "three stacked horizontal bars"),
    ("gear", "a settings cog"),
    ("home", "a house"),
    ("retry", "a circular arrow"),
    ("check", "a tick mark"),
    ("search", "a magnifying glass"),
    ("hand", "an open hand, palm forward"),
    ("heart", "a heart"),
    ("star", "a five-pointed star"),
    ("arrow_left", "a left-pointing arrow"),
    ("arrow_right", "a right-pointing arrow"),
    ("sound_on", "a speaker with sound waves"),
    ("sound_off", "a speaker with a cross beside it"),
)


@dataclass(frozen=True)
class IconGridRole:
    """One icon grid's declared geometry: the cells the template draws and the gate reads.

    ``cell`` is the guide square the template shows; the published cell is that square grown
    by ``slack`` on every side, because the model keeps the grid but drifts each glyph by
    some pixels, and a consumer's frame must hold the glyph the model actually drew. The
    gutters stay wider than twice the slack, so published cells never touch.
    """

    role: str
    layout: str
    glyphs: tuple[str, ...]
    columns: int
    rows: int
    cell: int
    gutter: int
    margin: int
    slack: int
    canvas: tuple[int, int] = (ICON_CANVAS, ICON_CANVAS)
    #: Sheet pixels per screen pixel, as for the nine-slice roles: a projection hint that
    #: names the density the set was drawn for and stays out of the generation cache key.
    draw_scale: int = 2

    def __post_init__(self) -> None:
        if len(self.glyphs) != self.columns * self.rows:
            raise ValueError("icon grid glyph count must equal its cell count")
        if len(set(self.glyphs)) != len(self.glyphs):
            raise ValueError("icon grid glyph names must be unique")
        if 2 * self.slack >= self.gutter or self.slack > self.margin:
            raise ValueError("icon grid slack must fit inside the gutter and the margin")
        span_x = 2 * self.margin + self.columns * self.cell + (self.columns - 1) * self.gutter
        span_y = 2 * self.margin + self.rows * self.cell + (self.rows - 1) * self.gutter
        if (span_x, span_y) != self.canvas:
            raise ValueError("icon grid geometry must tile its canvas exactly")

    @property
    def cell_size(self) -> int:
        """The published cell's side: what a consumer cuts as one frame."""

        return self.cell + 2 * self.slack

    @property
    def guide_cells(self) -> tuple[Rect, ...]:
        """The template's guide squares, in reading order."""

        pitch = self.cell + self.gutter
        return tuple(
            Rect(self.margin + column * pitch, self.margin + row * pitch, self.cell, self.cell)
            for row in range(self.rows)
            for column in range(self.columns)
        )

    @property
    def cells(self) -> tuple[Rect, ...]:
        """The published cells: each guide square grown by the registration slack."""

        return tuple(
            Rect(guide.x - self.slack, guide.y - self.slack, self.cell_size, self.cell_size)
            for guide in self.guide_cells
        )

    def geometry_record(self) -> dict[str, object]:
        """The declared geometry as a portable record; part of the generation cache key."""

        return {
            "role": self.role,
            "layout": self.layout,
            "scale_mode": ICON_SCALE_MODE,
            "canvas": {"width": self.canvas[0], "height": self.canvas[1]},
            "grid": {"columns": self.columns, "rows": self.rows},
            "cell": self.cell,
            "gutter": self.gutter,
            "margin": self.margin,
            "slack": self.slack,
            "glyphs": list(self.glyphs),
        }


PREVIEW_ICONS = IconGridRole(
    role="preview_icons",
    layout=PREVIEW_ICONS_LAYOUT,
    glyphs=tuple(name for name, _ in PREVIEW_ICON_GLYPHS),
    columns=4,
    rows=4,
    cell=200,
    gutter=48,
    margin=40,
    slack=16,
)

ICON_ROLES: dict[str, IconGridRole] = {PREVIEW_ICONS.role: PREVIEW_ICONS}

#: The fraction of the guide cell a glyph should fill; the template's yellow square.
ICON_GLYPH_TARGET_FRACTION = 0.7


def render_icon_template(role: IconGridRole) -> bytes:
    """Transparent canvas, one cyan guide square per cell, a yellow target square inside it.

    No magenta: for a nine-slice, magenta says "opaque body here", and an icon cell has no
    body — a model told the cell is a body paints a plate. The yellow square is the extent
    the glyph should fill, so sixteen glyphs come back at one size.
    """

    image = Image.new("RGBA", role.canvas, (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    for guide in role.guide_cells:
        draw.rectangle(
            (guide.x, guide.y, guide.x + guide.width - 1, guide.y + guide.height - 1),
            outline=_CYAN,
            width=3,
        )
        inset = round(guide.width * (1 - ICON_GLYPH_TARGET_FRACTION) / 2)
        draw.rectangle(
            (
                guide.x + inset,
                guide.y + inset,
                guide.x + guide.width - 1 - inset,
                guide.y + guide.height - 1 - inset,
            ),
            outline=_YELLOW,
            width=2,
        )
    stream = io.BytesIO()
    image.save(stream, format="PNG", optimize=False)
    return stream.getvalue()


class IconAdmissionError(ValueError):
    """The sheet failed the deterministic gate; ``failures`` lists every reason."""

    def __init__(self, failures: list[str]) -> None:
        super().__init__("; ".join(failures))
        self.failures = failures


def validate_icon_sheet(data: bytes, role: IconGridRole) -> dict[str, object]:
    """Gate one provider output for ``role`` and return the facts a manifest publishes.

    What alpha can prove: the canvas is mostly transparent and its border clear; nothing at
    all is drawn outside the published cells, which is what registers each glyph to its
    named cell; every cell holds one glyph with an opaque core that is neither a speck nor a
    plate; and the set shares one size. Whether a glyph is the symbol its cell names is the
    structured review's question.
    """

    with Image.open(io.BytesIO(data)) as opened:
        if "A" not in opened.getbands():
            raise IconAdmissionError([f"{role.role} output must carry an alpha channel"])
        image = opened.convert("RGBA")
    if image.size != role.canvas:
        raise IconAdmissionError(
            [f"{role.role} output must be exactly {role.canvas[0]}x{role.canvas[1]}"]
        )
    failures: list[str] = []
    alpha = image.getchannel("A")
    mask = alpha.point(lambda value: 255 if value >= MASK_THRESHOLD else 0)
    width, height = image.size
    border = [
        *alpha.crop((0, 0, width, 1)).get_flattened_data(),
        *alpha.crop((0, height - 1, width, height)).get_flattened_data(),
        *alpha.crop((0, 0, 1, height)).get_flattened_data(),
        *alpha.crop((width - 1, 0, width, height)).get_flattened_data(),
    ]
    border_max = max(border)
    transparent_fraction = sum(alpha.histogram()[: ICON_TRANSPARENT_ADMISSION_MAX + 1]) / (
        width * height
    )
    if border_max > ICON_TRANSPARENT_ADMISSION_MAX:
        failures.append(f"canvas border alpha {border_max} > {ICON_TRANSPARENT_ADMISSION_MAX}")
    if transparent_fraction < ICON_MIN_TRANSPARENT_FRACTION:
        failures.append(
            f"transparent fraction {transparent_fraction:.3f} < {ICON_MIN_TRANSPARENT_FRACTION}"
        )

    outside = Image.new("L", image.size, 255)
    for cell in role.cells:
        outside.paste(0, cell.box)
    outside_max = cast(tuple[int, int], ImageChops.multiply(alpha, outside).getextrema())[1]
    if outside_max > ICON_TRANSPARENT_ADMISSION_MAX:
        failures.append(
            f"alpha {outside_max} drawn between or around the cells "
            f"(> {ICON_TRANSPARENT_ADMISSION_MAX})"
        )

    cells_out: list[dict[str, object]] = []
    extents: list[float] = []
    for index, (glyph, guide, cell) in enumerate(
        zip(role.glyphs, role.guide_cells, role.cells, strict=True)
    ):
        label = f"cell {index + 1} ({glyph})"
        bbox = mask.crop(cell.box).getbbox()
        if bbox is None:
            failures.append(f"{label}: no glyph drawn")
            continue
        glyph_rect = Rect(cell.x + bbox[0], cell.y + bbox[1], bbox[2] - bbox[0], bbox[3] - bbox[1])
        alpha_max = cast(tuple[int, int], alpha.crop(cell.box).getextrema())[1]
        opaque = sum(mask.crop(cell.box).histogram()[MASK_THRESHOLD:])
        cover = opaque / (guide.width * guide.height)
        extent = max(glyph_rect.width, glyph_rect.height) / guide.width
        if alpha_max < ICON_GLYPH_ALPHA_MAX_MIN:
            failures.append(
                f"{label}: glyph alpha peaks at {alpha_max} < {ICON_GLYPH_ALPHA_MAX_MIN}"
            )
        if cover < ICON_GLYPH_COVER_MIN:
            failures.append(
                f"{label}: glyph covers {cover:.3f} of its cell < {ICON_GLYPH_COVER_MIN}"
            )
        if cover > ICON_GLYPH_COVER_MAX:
            failures.append(
                f"{label}: glyph covers {cover:.3f} of its cell > {ICON_GLYPH_COVER_MAX}, "
                "which is a plate rather than a glyph"
            )
        if extent < ICON_GLYPH_EXTENT_MIN:
            failures.append(f"{label}: glyph extent {extent:.3f} < {ICON_GLYPH_EXTENT_MIN}")
        if extent > ICON_GLYPH_EXTENT_MAX:
            failures.append(f"{label}: glyph extent {extent:.3f} > {ICON_GLYPH_EXTENT_MAX}")
        extents.append(extent)
        cells_out.append(
            {
                "glyph": glyph,
                "guide": guide.as_dict(),
                "cell": cell.as_dict(),
                "glyph_rect": glyph_rect.as_dict(),
                "alpha_max": alpha_max,
                "cover": round(cover, 4),
                "extent": round(extent, 4),
                "centre_offset": {
                    "x": (glyph_rect.x + glyph_rect.width / 2) - (guide.x + guide.width / 2),
                    "y": (glyph_rect.y + glyph_rect.height / 2) - (guide.y + guide.height / 2),
                },
            }
        )
    set_facts: dict[str, float] = {}
    if len(extents) == len(role.glyphs):
        ratio = max(extents) / min(extents) if min(extents) > 0 else float("inf")
        set_facts = {
            "extent_min": round(min(extents), 4),
            "extent_max": round(max(extents), 4),
            "extent_ratio": round(ratio, 4),
        }
        if ratio > ICON_SET_EXTENT_RATIO_MAX:
            failures.append(
                f"glyph sizes vary {ratio:.2f}x across the set > {ICON_SET_EXTENT_RATIO_MAX}"
            )
    if failures:
        raise IconAdmissionError(failures)

    return {
        "role": role.role,
        "layout": role.layout,
        "scale_mode": ICON_SCALE_MODE,
        "alpha_policy": ICON_ALPHA_POLICY,
        "canvas": {"width": width, "height": height},
        "cell_size": role.cell_size,
        "glyphs": list(role.glyphs),
        "cells": cells_out,
        "set": set_facts,
        "alpha": {
            "border_max": border_max,
            "transparent_fraction": round(transparent_fraction, 6),
            "outside_cells_max": outside_max,
        },
        "thresholds": {
            "transparent_admission_max": ICON_TRANSPARENT_ADMISSION_MAX,
            "min_transparent_fraction": ICON_MIN_TRANSPARENT_FRACTION,
            "glyph_alpha_max_min": ICON_GLYPH_ALPHA_MAX_MIN,
            "glyph_cover_min": ICON_GLYPH_COVER_MIN,
            "glyph_cover_max": ICON_GLYPH_COVER_MAX,
            "glyph_extent_min": ICON_GLYPH_EXTENT_MIN,
            "glyph_extent_max": ICON_GLYPH_EXTENT_MAX,
            "set_extent_ratio_max": ICON_SET_EXTENT_RATIO_MAX,
            "mask_threshold": MASK_THRESHOLD,
        },
        "pixel_rewrite_performed": False,
    }


def canonicalize_icon_sheet(data: bytes, role: IconGridRole) -> tuple[bytes, dict[str, object]]:
    """Normalize only the admitted exterior: already-transparent pixels go to alpha 0.

    Nothing inside a glyph is touched. An icon's edge is its antialiasing, and a clamp that
    hardened it would change the drawing; the gate already proved every glyph has a fully
    opaque core, which is all the alpha policy promises.
    """

    return _canonicalize_glyph_sheet(data, role, validate_icon_sheet)


def _canonicalize_glyph_sheet(
    data: bytes,
    role: IconGridRole,
    validate: Callable[[bytes, Any], dict[str, object]],
) -> tuple[bytes, dict[str, object]]:
    """The exterior normalization every glyph grid shares, measured by the given gate."""

    source_facts = validate(data, role)
    with Image.open(io.BytesIO(data)) as opened:
        image = opened.convert("RGBA")
    alpha = image.getchannel("A").point(
        lambda value: 0 if value <= ICON_TRANSPARENT_ADMISSION_MAX else value
    )
    image.putalpha(alpha)
    output = io.BytesIO()
    image.save(output, format="PNG", optimize=False)
    canonical_data = output.getvalue()
    canonical_facts = validate(canonical_data, role)
    return canonical_data, {
        "source": source_facts,
        "canonical": canonical_facts,
        "pixel_rewrite_performed": True,
        "pixel_rewrite": "alpha_exterior_normalization_v1",
    }


#: The two on-screen cell sizes the evidence shows each glyph at: a comfortable button icon
#: and the smallest a HUD is likely to draw.
ICON_EVIDENCE_SIZES = (48, 24)


def icon_evidence(data: bytes, facts: dict[str, object]) -> bytes:
    """Reviewer evidence: the sheet over a checkerboard, then one row per cell in reading
    order — the glyph name the cell was asked to hold and the cell drawn at two consumer
    sizes — so the judge checks identity cell by cell rather than by impression. The names
    are annotation, drawn by this function and never part of the sheet."""

    with Image.open(io.BytesIO(data)) as opened:
        sheet = opened.convert("RGBA")
    cells = cast(list[dict[str, object]], facts["cells"])
    font = ImageFont.load_default(size=18)
    gap, row_h, name_w = 24, 60, 150
    right_w = name_w + sum(size + gap for size in ICON_EVIDENCE_SIZES)
    canvas = _checkerboard((sheet.width + gap + right_w, max(sheet.height, row_h * len(cells))))
    canvas.alpha_composite(sheet, (0, 0))
    draw = ImageDraw.Draw(canvas)
    for index, entry in enumerate(cells):
        rect = cast(dict[str, int], entry["cell"])
        cell = sheet.crop(Rect(rect["x"], rect["y"], rect["width"], rect["height"]).box)
        y = index * row_h
        draw.text(
            (sheet.width + gap, y + row_h // 2),
            f"{index + 1}. {entry['glyph']}",
            fill=(20, 20, 20, 255),
            font=font,
            anchor="lm",
        )
        x = sheet.width + gap + name_w
        for size in ICON_EVIDENCE_SIZES:
            sample = cell.resize((size, size), Image.Resampling.LANCZOS)
            canvas.alpha_composite(sample, (x, y + (row_h - size) // 2))
            x += size + gap
    stream = io.BytesIO()
    canvas.convert("RGB").save(stream, format="PNG", optimize=False)
    return stream.getvalue()


def icon_role_contract(facts: dict[str, object]) -> dict[str, object]:
    """Project the resolved geometry the manifest binds beside the artifact."""

    return {
        "role": facts["role"],
        "layout": facts["layout"],
        "scale_mode": ICON_SCALE_MODE,
        "alpha_policy": ICON_ALPHA_POLICY,
        "draw_scale": ICON_ROLES[str(facts["role"])].draw_scale,
        "canvas": facts["canvas"],
        "cell_size": facts["cell_size"],
        "cells": [
            {"glyph": entry["glyph"], "cell": entry["cell"], "glyph_rect": entry["glyph_rect"]}
            for entry in cast(list[dict[str, object]], facts["cells"])
        ],
    }


__all__ = [
    "ICON_ALPHA_POLICY",
    "ICON_ROLES",
    "ICON_SCALE_MODE",
    "PREVIEW_ICONS",
    "PREVIEW_ICONS_LAYOUT",
    "PREVIEW_ICON_GLYPHS",
    "IconAdmissionError",
    "IconGridRole",
    "canonicalize_icon_sheet",
    "icon_evidence",
    "icon_role_contract",
    "render_icon_template",
    "validate_icon_sheet",
]
