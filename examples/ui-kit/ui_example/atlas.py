"""Nine-slice atlas roles: geometry, layout templates, the pixel gate, and the runtime draw.

The two roles here are the ``game-ui-v2`` executable slice of the atlas taxonomy in
the fixed geometry below: a ``panel_frame`` (one body)
and a ``button_rect`` state sheet (four bodies stacked in reading order). Everything
a consumer needs to draw them is *resolved* here and published in the manifest:
detected cell rectangles, per-side insets, content rects, and the band fill the
artwork was admitted under. No runtime rediscovers geometry from pixels.

The gate proves what the format promises. A nine-slice is its four corners plus five
repeatable regions; so each cell is rebuilt from those regions the way a runtime draws it
and diffed against the original (``stretch``), or its band ends are compared where a
repeat would meet (``tile``). A textured medium tiles and a flat medium stretches, which is
a fact about the art rather than a prompt failure, so admission records the first fill the
cell passes rather than rejecting textured art. State sheets additionally prove one
silhouette across states and that each state is visibly distinct from ``normal``.

Thresholds were set from the 2026-09-02 spike rounds (two mediums, two takes, sixteen
cells). They are part of the ``prepared-ui-atlas-v1`` contract identity: changing one is a
contract bump, not a tweak.
"""

from __future__ import annotations

import io
from collections.abc import Callable
from dataclasses import dataclass
from typing import Literal, cast

from PIL import Image, ImageChops, ImageDraw, ImageStat

ATLAS_SCALE_MODE = "nine_slice"
ATLAS_ALPHA_POLICY = "transparent_exterior_opaque_body_v1"
PANEL_FRAME_LAYOUT = "nine_slice_panel_1024_v1"
BUTTON_RECT_LAYOUT = "nine_slice_button_sheet_4x1024_v1"
ATLAS_CANVAS = 1024

BandFill = Literal["stretch", "tile"]
BAND_FILLS: tuple[BandFill, ...] = ("stretch", "tile")

TRANSPARENT_ADMISSION_MAX = 16
OPAQUE_ADMISSION_MIN = 250
#: Edge bands are painted texture, and a painterly medium leaves grain strokes a little short of
#: full opacity (measured 242 and 248 on carved wood, never near a hole). A band is refused only
#: when it drops below this floor; canonicalization then clamps admitted band pixels to 255.
BAND_OPAQUE_MIN = 224
MIN_TRANSPARENT_FRACTION = 0.10
BORDER_EDGE_PX = 8
STRIP_PX = 8
RECONSTRUCTION_MAE_MAX = 6.0
TILE_SEAM_EXCESS_MAX = 8.0
CONTENT_LUMA_STD_MAX = 12.0
CONTENT_CONTRAST_MIN = 4.5
STATE_IOU_MIN = 0.97
STATE_SIZE_DELTA_MAX_PX = 4
STATE_DISTINCT_MIN = 3.0
BAND_MEAN_TOL = 10.0
EXTENT_SMOOTH_PX = 8
EXTENT_SPREAD_FACTOR = 3.0
EXTENT_WIDEN_FRACTION = 1.0
MASK_THRESHOLD = 128
MIN_RUN_PX = 8
#: Corner ornament may curl past the band into the content rect. The safe rect is the largest
#: interior rectangle whose border rows and columns carry no run of ornament-coloured pixels,
#: measured against the centre's own colour and spread; it can shrink each side by at most this
#: fraction of the content rect, so a busy centre is reported as such rather than vanishing.
SAFE_RECT_TOL = 24.0
SAFE_RECT_SPREAD_FACTOR = 3.0
SAFE_RECT_MAX_SHRINK = 0.45

_MAGENTA = (255, 0, 255, 255)
_YELLOW = (255, 255, 0, 255)
_CYAN = (0, 255, 255, 255)


@dataclass(frozen=True)
class Rect:
    x: int
    y: int
    width: int
    height: int

    @property
    def box(self) -> tuple[int, int, int, int]:
        return (self.x, self.y, self.x + self.width, self.y + self.height)

    def as_dict(self) -> dict[str, int]:
        return {"x": self.x, "y": self.y, "width": self.width, "height": self.height}


@dataclass(frozen=True)
class Insets:
    """Per-side nine-slice insets. The template declares them uniform; the gate may widen a
    side when the drawn corner ornament runs past the guide."""

    left: int
    top: int
    right: int
    bottom: int

    @classmethod
    def uniform(cls, amount: int) -> Insets:
        return cls(amount, amount, amount, amount)

    def as_dict(self) -> dict[str, int]:
        return {"left": self.left, "top": self.top, "right": self.right, "bottom": self.bottom}

    def content(self, width: int, height: int) -> Rect:
        return Rect(
            self.left, self.top, width - self.left - self.right, height - self.top - self.bottom
        )

    def widest(self, other: Insets) -> Insets:
        return Insets(
            max(self.left, other.left),
            max(self.top, other.top),
            max(self.right, other.right),
            max(self.bottom, other.bottom),
        )


@dataclass(frozen=True)
class AtlasRole:
    """One role's declared geometry: what the template draws and what the gate measures."""

    role: str
    layout: str
    cells: tuple[Rect, ...]
    states: tuple[str, ...]
    insets: int
    canvas: tuple[int, int] = (ATLAS_CANVAS, ATLAS_CANVAS)
    #: Sheet pixels per screen pixel. A 1024 canvas is authored at twice the density a HUD
    #: draws at, so a consumer lays the slices out at ``draw_scale`` times its target size and
    #: scales the result down; corners land at half their sheet size and seams shrink with them.
    #: A projection hint for consumers, not geometry: it is published in the contract but kept
    #: out of the generation cache key, because changing it must not re-bill an image.
    draw_scale: int = 2

    @property
    def declared_insets(self) -> Insets:
        return Insets.uniform(self.insets)

    def geometry_record(self) -> dict[str, object]:
        """The declared geometry as a portable record; part of the generation cache key."""

        return {
            "role": self.role,
            "layout": self.layout,
            "scale_mode": ATLAS_SCALE_MODE,
            "canvas": {"width": self.canvas[0], "height": self.canvas[1]},
            "insets": self.declared_insets.as_dict(),
            "states": list(self.states),
            "cells": [
                {"state": state, **cell.as_dict()}
                for state, cell in zip(self.states, self.cells, strict=True)
            ],
        }


PANEL_FRAME = AtlasRole(
    role="panel_frame",
    layout=PANEL_FRAME_LAYOUT,
    cells=(Rect(160, 256, 704, 512),),
    states=("default",),
    insets=96,
)

_BUTTON_H = 128
_BUTTON_GAP = 48
_BUTTON_TOP = (ATLAS_CANVAS - (4 * _BUTTON_H + 3 * _BUTTON_GAP)) // 2
BUTTON_RECT = AtlasRole(
    role="button_rect",
    layout=BUTTON_RECT_LAYOUT,
    cells=tuple(
        Rect(192, _BUTTON_TOP + index * (_BUTTON_H + _BUTTON_GAP), 640, _BUTTON_H)
        for index in range(4)
    ),
    states=("normal", "hover", "pressed", "disabled"),
    insets=40,
)

ATLAS_ROLES: dict[str, AtlasRole] = {PANEL_FRAME.role: PANEL_FRAME, BUTTON_RECT.role: BUTTON_RECT}


def render_atlas_template(role: AtlasRole) -> bytes:
    """Transparent canvas, magenta bodies, yellow outer edges, cyan slice guides.

    The same visual language as the packaged inventory template: magenta is "opaque body
    here", yellow is "the silhouette edge", cyan is "geometry guide". Rendered from the
    geometry rather than shipped as a file so the two cannot drift.
    """

    image = Image.new("RGBA", role.canvas, (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    for cell in role.cells:
        draw.rectangle(cell.box, fill=_MAGENTA)
        draw.rectangle(
            (cell.x, cell.y, cell.x + cell.width - 1, cell.y + cell.height - 1),
            outline=_YELLOW,
            width=3,
        )
        inset = role.insets
        for x in (cell.x + inset, cell.x + cell.width - inset):
            draw.line((x, cell.y, x, cell.y + cell.height - 1), fill=_CYAN, width=2)
        for y in (cell.y + inset, cell.y + cell.height - inset):
            draw.line((cell.x, y, cell.x + cell.width - 1, y), fill=_CYAN, width=2)
    stream = io.BytesIO()
    image.save(stream, format="PNG", optimize=False)
    return stream.getvalue()


# --- measurement -------------------------------------------------------------------------


def _mae(a: Image.Image, b: Image.Image) -> float:
    stat = ImageStat.Stat(ImageChops.difference(a.convert("RGB"), b.convert("RGB")))
    if not stat.count or stat.count[0] == 0:
        return 0.0
    return float(sum(stat.mean) / len(stat.mean))


def _runs(profile: list[int], minimum: int) -> list[tuple[int, int]]:
    runs: list[tuple[int, int]] = []
    start: int | None = None
    for index, value in enumerate([*profile, 0]):
        if value and start is None:
            start = index
        elif not value and start is not None:
            if index - start >= minimum:
                runs.append((start, index))
            start = None
    return runs


def detect_cells(image: Image.Image) -> list[Rect]:
    """Opaque bodies in reading order, from alpha row runs then column runs.

    The model keeps a sheet's count and order but re-spaces its cells across the canvas, so
    bodies are detected rather than trusted from the template. A row-band projection is
    enough for these sheets; two bodies sharing a row band would merge and fail the count.
    """

    mask = image.getchannel("A").point(lambda v: 255 if v >= MASK_THRESHOLD else 0)
    w, h = mask.size
    rows = [1 if mask.crop((0, y, w, y + 1)).getbbox() else 0 for y in range(h)]
    cells: list[Rect] = []
    for top, bottom in _runs(rows, MIN_RUN_PX):
        band = mask.crop((0, top, w, bottom))
        cols = [1 if band.crop((x, 0, x + 1, band.height)).getbbox() else 0 for x in range(w)]
        for left, right in _runs(cols, MIN_RUN_PX):
            cells.append(Rect(left, top, right - left, bottom - top))
    return cells


def _tile(patch: Image.Image, width: int, height: int) -> Image.Image:
    out = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    for y in range(0, height, patch.height):
        for x in range(0, width, patch.width):
            out.paste(patch, (x, y))
    return out


def nine_slice_render(
    cell: Image.Image, insets: Insets, width: int, height: int, fill: BandFill = "stretch"
) -> Image.Image:
    """Draw ``cell`` at ``width`` by ``height`` the way a runtime nine-slice does.

    Corners are copied. With ``stretch`` each band is one ``STRIP_PX`` sample from the band's
    middle scaled to fill, so ornament inside a band shows up as error rather than being
    carried along. With ``tile`` the whole band repeats end to end; the error that mode can
    show is the seam where one repeat meets the next.
    """

    w, h = cell.size
    li, t, r, b = insets.left, insets.top, insets.right, insets.bottom
    out = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    inner_w, inner_h = width - li - r, height - t - b
    out.paste(cell.crop((0, 0, li, t)), (0, 0))
    out.paste(cell.crop((w - r, 0, w, t)), (width - r, 0))
    out.paste(cell.crop((0, h - b, li, h)), (0, height - b))
    out.paste(cell.crop((w - r, h - b, w, h)), (width - r, height - b))
    band: Callable[[Image.Image, int, int], Image.Image]
    if fill == "stretch":
        strip = STRIP_PX
        mid_x = (li + (w - r)) // 2 - strip // 2
        mid_y = (t + (h - b)) // 2 - strip // 2
        top, bottom = (
            cell.crop((mid_x, 0, mid_x + strip, t)),
            cell.crop((mid_x, h - b, mid_x + strip, h)),
        )
        left, right = (
            cell.crop((0, mid_y, li, mid_y + strip)),
            cell.crop((w - r, mid_y, w, mid_y + strip)),
        )
        centre = cell.crop((mid_x, mid_y, mid_x + strip, mid_y + strip))

        def stretch(patch: Image.Image, bw: int, bh: int) -> Image.Image:
            return patch.resize((bw, bh), Image.Resampling.BILINEAR)

        band = stretch

    else:
        top, bottom = cell.crop((li, 0, w - r, t)), cell.crop((li, h - b, w - r, h))
        left, right = cell.crop((0, t, li, h - b)), cell.crop((w - r, t, w, h - b))
        centre = cell.crop((li, t, w - r, h - b))
        band = _tile
    if inner_w > 0:
        out.paste(band(top, inner_w, t), (li, 0))
        out.paste(band(bottom, inner_w, b), (li, height - b))
    if inner_h > 0:
        out.paste(band(left, li, inner_h), (0, t))
        out.paste(band(right, r, inner_h), (width - r, t))
    if inner_w > 0 and inner_h > 0:
        out.paste(band(centre, inner_w, inner_h), (li, t))
    return out


def _band_strip_alpha_min(alpha: Image.Image, insets: Insets) -> int:
    """Lowest alpha across the four strips a stretch draws, inside the border line."""

    w, h = alpha.size
    li, t, r, b = insets.left, insets.top, insets.right, insets.bottom
    strip, edge = STRIP_PX, BORDER_EDGE_PX
    mid_x = (li + (w - r)) // 2 - strip // 2
    mid_y = (t + (h - b)) // 2 - strip // 2
    boxes = (
        (mid_x, edge, mid_x + strip, t),
        (mid_x, h - b, mid_x + strip, h - edge),
        (edge, mid_y, li, mid_y + strip),
        (w - r, mid_y, w - edge, mid_y + strip),
    )
    return min(cast(tuple[int, int], alpha.crop(box).getextrema())[0] for box in boxes)


_Profile = list[tuple[float, float, float]]


def _smooth(profile: _Profile, window: int) -> _Profile:
    half, n = window // 2, len(profile)
    out: _Profile = []
    for index in range(n):
        lo, hi = max(0, index - half), min(n, index + half + 1)
        out.append(
            cast(
                tuple[float, float, float],
                tuple(sum(v[c] for v in profile[lo:hi]) / (hi - lo) for c in range(3)),
            )
        )
    return out


def _flat_run_containing_middle(profile: _Profile) -> tuple[int, int, float]:
    """Longest run near the profile's middle-half mean, after smoothing.

    The tolerance widens to a multiple of the middle half's own spread so a band whose
    colour drifts gently along its length still reads as one run while a corner cap, which
    is a step, does not.
    """

    n = len(profile)
    if n == 0:
        return (0, 0, BAND_MEAN_TOL)
    smoothed = _smooth(profile, EXTENT_SMOOTH_PX)
    core = smoothed[n // 4 : max(n // 4 + 1, 3 * n // 4)]
    reference = tuple(sum(v[c] for v in core) / len(core) for c in range(3))
    spread = max(
        (sum((v[c] - reference[c]) ** 2 for v in core) / len(core)) ** 0.5 for c in range(3)
    )
    used = max(BAND_MEAN_TOL, EXTENT_SPREAD_FACTOR * spread)
    flat = [1 if max(abs(v[c] - reference[c]) for c in range(3)) <= used else 0 for v in smoothed]
    best = (0, 0)
    for start, end in _runs(flat, 1):
        if end - start > best[1] - best[0]:
            best = (start, end)
    return (best[0], best[1], round(used, 2))


def _column_means(region: Image.Image) -> _Profile:
    rgb = region.convert("RGB")
    return [
        cast(
            tuple[float, float, float],
            tuple(ImageStat.Stat(rgb.crop((x, 0, x + 1, rgb.height))).mean),
        )
        for x in range(rgb.width)
    ]


def _row_means(region: Image.Image) -> _Profile:
    rgb = region.convert("RGB")
    return [
        cast(
            tuple[float, float, float],
            tuple(ImageStat.Stat(rgb.crop((0, y, rgb.width, y + 1))).mean),
        )
        for y in range(rgb.height)
    ]


def band_extents(cell: Image.Image, insets: Insets) -> dict[str, dict[str, object]]:
    """Where each edge band's repeatable run starts and ends, in cell coordinates.

    A band's per-column (per-row) mean colour is flat along the repeatable run and deviates
    where the corner ornament spills past the guide. Grain averages out across the band's
    thickness, so this reads textured bands as well as flat ones.
    """

    w, h = cell.size
    li, t, r, b = insets.left, insets.top, insets.right, insets.bottom
    out: dict[str, dict[str, object]] = {}
    for name, box, along_x in (
        ("top", (li, 0, w - r, t), True),
        ("bottom", (li, h - b, w - r, h), True),
        ("left", (0, t, li, h - b), False),
        ("right", (w - r, t, w, h - b), False),
    ):
        region = cell.crop(box)
        profile = _column_means(region) if along_x else _row_means(region)
        start, end, used = _flat_run_containing_middle(profile)
        origin = box[0] if along_x else box[1]
        out[name] = {
            "start": origin + start,
            "end": origin + end,
            "declared": [box[0], box[2]] if along_x else [box[1], box[3]],
            "covers_declared": start == 0 and end == len(profile),
            "tolerance": used,
        }
    return out


def effective_insets(
    cell: Image.Image, insets: Insets
) -> tuple[Insets, dict[str, dict[str, object]]]:
    """Widen each inset to the far end of the corner ornament its bands reveal.

    Widening is capped at ``EXTENT_WIDEN_FRACTION`` of the declared inset: a corner cap drawn
    past the guide is absorbed, while a grain streak deep inside the band is left to the fill
    gate, which is where a repeat defect belongs.
    """

    w, h = cell.size
    extents = band_extents(cell, insets)

    def widen(declared: int, *candidates: int) -> int:
        cap = declared + int(declared * EXTENT_WIDEN_FRACTION)
        return min(cap, max(declared, *candidates))

    def at(name: str, key: str) -> int:
        return int(cast(int, extents[name][key]))

    return (
        Insets(
            widen(insets.left, at("top", "start"), at("bottom", "start")),
            widen(insets.top, at("left", "start"), at("right", "start")),
            widen(insets.right, w - at("top", "end"), w - at("bottom", "end")),
            widen(insets.bottom, h - at("left", "end"), h - at("right", "end")),
        ),
        extents,
    )


def reconstruction_error(cell: Image.Image, insets: Insets) -> dict[str, float]:
    """Per-band MAE between the cell and its own stretch reconstruction."""

    w, h = cell.size
    li, t, r, b = insets.left, insets.top, insets.right, insets.bottom
    rebuilt = nine_slice_render(cell, insets, w, h, "stretch")
    bands = {
        "top": (li, 0, w - r, t),
        "bottom": (li, h - b, w - r, h),
        "left": (0, t, li, h - b),
        "right": (w - r, t, w, h - b),
        "center": (li, t, w - r, h - b),
    }
    return {name: round(_mae(cell.crop(box), rebuilt.crop(box)), 3) for name, box in bands.items()}


def _seam_vs_floor(
    rgb: Image.Image, box: tuple[int, int, int, int], along_x: bool
) -> tuple[float, float]:
    band = rgb.crop(box)
    probe = STRIP_PX
    length = band.width if along_x else band.height

    def patch(offset: int) -> Image.Image:
        if along_x:
            return band.crop((offset, 0, offset + probe, band.height))
        return band.crop((0, offset, band.width, offset + probe))

    seam = _mae(patch(length - probe), patch(0))
    pairs = [_mae(patch(o), patch(o + probe)) for o in range(0, length - 2 * probe + 1, probe)]
    return seam, (sum(pairs) / len(pairs) if pairs else 0.0)


def tile_seam_excess(
    cell: Image.Image, insets: Insets
) -> tuple[dict[str, float], dict[str, dict[str, float]]]:
    """Per-band seam excess: how far the diff where a tiled band's end meets its start stands
    above the band's own neighbouring-patch floor. A grained band whose ends match as well as
    any two neighbouring patches scores near zero; only a visible step scores."""

    w, h = cell.size
    li, t, r, b = insets.left, insets.top, insets.right, insets.bottom
    rgb = cell.convert("RGB")
    bands = {
        "top": ((li, 0, w - r, t), True),
        "bottom": ((li, h - b, w - r, h), True),
        "left": ((0, t, li, h - b), False),
        "right": ((w - r, t, w, h - b), False),
        "center_x": ((li, t, w - r, h - b), True),
        "center_y": ((li, t, w - r, h - b), False),
    }
    excess: dict[str, float] = {}
    raw: dict[str, dict[str, float]] = {}
    for name, (box, along_x) in bands.items():
        seam, floor = _seam_vs_floor(rgb, box, along_x)
        excess[name] = round(max(0.0, seam - floor), 3)
        raw[name] = {"seam": round(seam, 3), "floor": round(floor, 3)}
    return excess, raw


def ornament_signal(cell: Image.Image, insets: Insets) -> dict[str, float]:
    """How different the corners and the top band are from the centre fill. Evidence only: a
    frameless slab reconstructs perfectly and scores near zero here."""

    w, h = cell.size
    li, t, r, b = insets.left, insets.top, insets.right, insets.bottom
    centre = cell.crop((li, t, w - r, h - b)).convert("RGB")
    mean = cast(tuple[int, int, int], tuple(int(v) for v in ImageStat.Stat(centre).mean))
    corners = [
        cell.crop(box)
        for box in ((0, 0, li, t), (w - r, 0, w, t), (0, h - b, li, h), (w - r, h - b, w, h))
    ]
    corner_vs_centre = sum(_mae(c, Image.new("RGB", c.size, mean)) for c in corners) / 4
    edge = cell.crop((li, 0, w - r, t)).convert("RGB")
    return {
        "corner_vs_centre_mae": round(corner_vs_centre, 3),
        "edge_vs_centre_mae": round(_mae(edge, Image.new("RGB", edge.size, mean)), 3),
    }


def _relative_luminance(rgb: tuple[float, float, float]) -> float:
    def channel(value: float) -> float:
        c = value / 255.0
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4

    r, g, b = (channel(v) for v in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def safe_rect(cell: Image.Image, content: Rect) -> Rect:
    """The largest ornament-free rectangle inside the content rect, in cell coordinates.

    The centre's mean colour and spread come from the middle half of the content rect, which the
    flatness gate already proves quiet. A row or column is unsafe while it carries a run of at
    least ``MIN_RUN_PX`` pixels farther than the tolerance from that mean; each side moves inward
    past its unsafe rows, capped at ``SAFE_RECT_MAX_SHRINK`` of the content size.
    """

    region = cell.crop(content.box).convert("RGB")
    w, h = region.size
    core = region.crop((w // 4, h // 4, w - w // 4, h - h // 4))
    mean = cast(tuple[float, float, float], tuple(ImageStat.Stat(core).mean))
    spread = max(ImageStat.Stat(core).stddev)
    tol = max(SAFE_RECT_TOL, SAFE_RECT_SPREAD_FACTOR * spread)
    raw = region.tobytes()

    def unsafe(coords: list[tuple[int, int]]) -> bool:
        run = 0
        for x, y in coords:
            offset = (y * w + x) * 3
            r, g, b = raw[offset], raw[offset + 1], raw[offset + 2]
            distance = (abs(r - mean[0]) + abs(g - mean[1]) + abs(b - mean[2])) / 3
            run = run + 1 if distance > tol else 0
            if run >= MIN_RUN_PX:
                return True
        return False

    max_dx, max_dy = int(w * SAFE_RECT_MAX_SHRINK), int(h * SAFE_RECT_MAX_SHRINK)

    def rows(left: int, right: int) -> tuple[int, int]:
        top = 0
        while top < max_dy and unsafe([(x, top) for x in range(left, w - right)]):
            top += 1
        bottom = 0
        while bottom < max_dy and unsafe([(x, h - 1 - bottom) for x in range(left, w - right)]):
            bottom += 1
        return top, bottom

    def columns(top: int, bottom: int) -> tuple[int, int]:
        left = 0
        while left < max_dx and unsafe([(left, y) for y in range(top, h - bottom)]):
            left += 1
        right = 0
        while right < max_dx and unsafe([(w - 1 - right, y) for y in range(top, h - bottom)]):
            right += 1
        return left, right

    # A corner curl can be excluded by giving up rows or by giving up columns; take whichever
    # keeps more of the interior.
    top_a, bottom_a = rows(0, 0)
    left_a, right_a = columns(top_a, bottom_a)
    left_b, right_b = columns(0, 0)
    top_b, bottom_b = rows(left_b, right_b)
    candidates = (
        Rect(content.x + left_a, content.y + top_a, w - left_a - right_a, h - top_a - bottom_a),
        Rect(content.x + left_b, content.y + top_b, w - left_b - right_b, h - top_b - bottom_b),
    )
    return max(candidates, key=lambda rect: rect.width * rect.height)


def content_stats(cell: Image.Image, content: Rect) -> dict[str, object]:
    """Flatness and best-case text contrast of the content rect."""

    region = cell.crop(content.box)
    mean = cast(tuple[float, float, float], tuple(ImageStat.Stat(region.convert("RGB")).mean))
    luma_std = float(ImageStat.Stat(region.convert("L")).stddev[0])
    lum = _relative_luminance(mean)
    against_white = (1.0 + 0.05) / (lum + 0.05)
    against_black = (lum + 0.05) / 0.05
    return {
        "mean_rgb": [round(v, 1) for v in mean],
        "luma_std": round(luma_std, 3),
        "contrast_vs_white": round(against_white, 3),
        "contrast_vs_black": round(against_black, 3),
        "best_text": "white" if against_white >= against_black else "black",
        "best_contrast": round(max(against_white, against_black), 3),
    }


def _mask(cell: Image.Image, size: tuple[int, int]) -> Image.Image:
    canvas = Image.new("L", size, 0)
    canvas.paste(cell.getchannel("A").point(lambda v: 255 if v >= MASK_THRESHOLD else 0), (0, 0))
    return canvas


def state_checks(
    cells: list[Image.Image], states: tuple[str, ...], insets: Insets
) -> dict[str, dict[str, float]]:
    """Silhouette identity and distinctness of every state against the first, top-left aligned
    so a state drawn a few pixels larger shows as an IoU drop rather than being hidden."""

    size = (max(c.width for c in cells), max(c.height for c in cells))
    base = cells[0]
    base_mask = _mask(base, size)
    results: dict[str, dict[str, float]] = {}
    for state, cell in zip(states[1:], cells[1:], strict=True):
        mask = _mask(cell, size)
        inter = sum(ImageChops.multiply(base_mask, mask).histogram()[1:])
        union = sum(ImageChops.lighter(base_mask, mask).histogram()[1:])
        common = insets.content(min(base.width, cell.width), min(base.height, cell.height))
        results[state] = {
            "silhouette_iou": round(inter / union, 4) if union else 0.0,
            "size_delta_px": max(abs(base.width - cell.width), abs(base.height - cell.height)),
            "distinct_from_normal_mae": round(
                _mae(base.crop(common.box), cell.crop(common.box)), 3
            ),
        }
    return results


# --- the gate ------------------------------------------------------------------------------


class AtlasAdmissionError(ValueError):
    """The image failed the deterministic gate; ``failures`` lists every reason."""

    def __init__(self, failures: list[str]) -> None:
        super().__init__("; ".join(failures))
        self.failures = failures


def validate_atlas_image(data: bytes, role: AtlasRole) -> dict[str, object]:
    """Gate one provider output for ``role`` and return the facts a manifest publishes.

    Raises :class:`AtlasAdmissionError` on any failure, so the generation service treats it
    as a retryable attempt, exactly like the inventory panel. On success the facts carry the
    detected cells, the sheet insets, the admitted ``band_fill``, and every number the gate
    used, so a cached artifact can be re-checked against the same contract.
    """

    with Image.open(io.BytesIO(data)) as opened:
        if "A" not in opened.getbands():
            raise AtlasAdmissionError([f"{role.role} output must carry an alpha channel"])
        image = opened.convert("RGBA")
    if image.size != role.canvas:
        raise AtlasAdmissionError(
            [f"{role.role} output must be exactly {role.canvas[0]}x{role.canvas[1]}"]
        )
    failures: list[str] = []
    alpha = image.getchannel("A")
    border = [
        *alpha.crop((0, 0, alpha.width, 1)).get_flattened_data(),
        *alpha.crop((0, alpha.height - 1, alpha.width, alpha.height)).get_flattened_data(),
        *alpha.crop((0, 0, 1, alpha.height)).get_flattened_data(),
        *alpha.crop((alpha.width - 1, 0, alpha.width, alpha.height)).get_flattened_data(),
    ]
    border_max = max(border)
    transparent_fraction = sum(alpha.histogram()[: TRANSPARENT_ADMISSION_MAX + 1]) / (
        alpha.width * alpha.height
    )
    if border_max > TRANSPARENT_ADMISSION_MAX:
        failures.append(f"canvas border alpha {border_max} > {TRANSPARENT_ADMISSION_MAX}")
    if transparent_fraction < MIN_TRANSPARENT_FRACTION:
        failures.append(
            f"transparent fraction {transparent_fraction:.3f} < {MIN_TRANSPARENT_FRACTION}"
        )

    detected = detect_cells(image)
    if len(detected) != len(role.cells):
        failures.append(f"detected {len(detected)} opaque bodies, declared {len(role.cells)}")
        raise AtlasAdmissionError(failures)

    declared = role.declared_insets
    registration: list[dict[str, object]] = []
    found_cells: list[tuple[str, Rect, Insets, dict[str, dict[str, object]]]] = []
    sheet_insets = declared
    for state, expected, found in zip(role.states, role.cells, detected, strict=True):
        registration.append(
            {
                "state": state,
                "declared": expected.as_dict(),
                "detected": found.as_dict(),
                "drift_px": {
                    "x": found.x - expected.x,
                    "y": found.y - expected.y,
                    "width": found.width - expected.width,
                    "height": found.height - expected.height,
                },
            }
        )
        if found.width <= 2 * role.insets + STRIP_PX or found.height <= 2 * role.insets + STRIP_PX:
            failures.append(
                f"{state}: body {found.width}x{found.height} too small for insets {role.insets}"
            )
            continue
        own, extents = effective_insets(image.crop(found.box), declared)
        sheet_insets = sheet_insets.widest(own)
        found_cells.append((state, found, own, extents))
    if failures:
        raise AtlasAdmissionError(failures)

    cells_out: list[dict[str, object]] = []
    cell_images: list[Image.Image] = []
    fills_per_cell: list[set[str]] = []
    for state, found, own, extents in found_cells:
        if (
            sheet_insets.left + sheet_insets.right + STRIP_PX >= found.width
            or sheet_insets.top + sheet_insets.bottom + STRIP_PX >= found.height
        ):
            failures.append(
                f"{state}: sheet insets {sheet_insets.as_dict()} leave no band in a "
                f"{found.width}x{found.height} body"
            )
            continue
        cell = image.crop(found.box)
        cell_images.append(cell)
        content_rect = sheet_insets.content(found.width, found.height)
        core_min = cast(tuple[int, int], alpha.crop(found.box).crop(content_rect.box).getextrema())[
            0
        ]
        if core_min < OPAQUE_ADMISSION_MIN:
            failures.append(f"{state}: content rect alpha {core_min} < {OPAQUE_ADMISSION_MIN}")
        band_min = _band_strip_alpha_min(alpha.crop(found.box), sheet_insets)
        if band_min < BAND_OPAQUE_MIN:
            failures.append(f"{state}: band strip alpha {band_min} < {BAND_OPAQUE_MIN}")
        recon = reconstruction_error(cell, sheet_insets)
        seam, seam_raw = tile_seam_excess(cell, sheet_insets)
        worst_recon = max(recon, key=lambda k: recon[k])
        worst_seam = max(seam, key=lambda k: seam[k])
        fills = [
            fill
            for fill, ok in (
                ("stretch", recon[worst_recon] <= RECONSTRUCTION_MAE_MAX),
                ("tile", seam[worst_seam] <= TILE_SEAM_EXCESS_MAX),
            )
            if ok
        ]
        fills_per_cell.append(set(fills))
        if not fills:
            failures.append(
                f"{state}: no band fill passes (stretch {worst_recon} {recon[worst_recon]} > "
                f"{RECONSTRUCTION_MAE_MAX}; tile {worst_seam} {seam[worst_seam]} > "
                f"{TILE_SEAM_EXCESS_MAX})"
            )
        content = content_stats(cell, content_rect)
        if cast(float, content["luma_std"]) > CONTENT_LUMA_STD_MAX:
            failures.append(
                f"{state}: content luma std {content['luma_std']} > {CONTENT_LUMA_STD_MAX}"
            )
        if cast(float, content["best_contrast"]) < CONTENT_CONTRAST_MIN:
            failures.append(
                f"{state}: content contrast {content['best_contrast']} < {CONTENT_CONTRAST_MIN}"
            )
        safe = safe_rect(cell, content_rect)
        cells_out.append(
            {
                "state": state,
                "cell": found.as_dict(),
                "content_rect": Rect(
                    found.x + content_rect.x,
                    found.y + content_rect.y,
                    content_rect.width,
                    content_rect.height,
                ).as_dict(),
                "safe_rect": Rect(
                    found.x + safe.x, found.y + safe.y, safe.width, safe.height
                ).as_dict(),
                "content_alpha_min": core_min,
                "band_strip_alpha_min": band_min,
                "own_insets": own.as_dict(),
                "band_extents": extents,
                "reconstruction_mae": recon,
                "tile_seam_excess": seam,
                "tile_seam_raw": seam_raw,
                "fills": fills,
                "ornament": ornament_signal(cell, sheet_insets),
                "content": content,
            }
        )

    band_fill: BandFill | None = None
    if fills_per_cell and len(fills_per_cell) == len(role.cells):
        common = set(BAND_FILLS).intersection(*fills_per_cell)
        band_fill = next((fill for fill in BAND_FILLS if fill in common), None)
        if band_fill is None and not any(
            f.split(":")[-1].strip().startswith("no band fill") for f in failures
        ):
            failures.append(
                "cells admit no common band fill: "
                + ", ".join(sorted(str(sorted(f)) for f in fills_per_cell))
            )

    states_out: dict[str, dict[str, float]] = {}
    if len(cell_images) == len(role.cells) > 1:
        states_out = state_checks(cell_images, role.states, sheet_insets)
        for state in role.states[1:]:
            entry = states_out[state]
            if entry["silhouette_iou"] < STATE_IOU_MIN:
                failures.append(
                    f"{state}: silhouette IoU {entry['silhouette_iou']} < {STATE_IOU_MIN}"
                )
            if entry["size_delta_px"] > STATE_SIZE_DELTA_MAX_PX:
                failures.append(
                    f"{state}: size delta {entry['size_delta_px']} px > {STATE_SIZE_DELTA_MAX_PX}"
                )
            if entry["distinct_from_normal_mae"] < STATE_DISTINCT_MIN:
                failures.append(
                    f"{state}: indistinct from normal (MAE {entry['distinct_from_normal_mae']})"
                )
    if failures or band_fill is None:
        raise AtlasAdmissionError(failures or ["no band fill admitted"])

    return {
        "role": role.role,
        "layout": role.layout,
        "scale_mode": ATLAS_SCALE_MODE,
        "alpha_policy": ATLAS_ALPHA_POLICY,
        "band_fill": band_fill,
        "canvas": {"width": image.width, "height": image.height},
        "insets": sheet_insets.as_dict(),
        "states": list(role.states),
        "registration": registration,
        "cells": cells_out,
        "state_checks": states_out,
        "alpha": {"border_max": border_max, "transparent_fraction": round(transparent_fraction, 6)},
        "thresholds": {
            "transparent_admission_max": TRANSPARENT_ADMISSION_MAX,
            "opaque_admission_min": OPAQUE_ADMISSION_MIN,
            "band_opaque_min": BAND_OPAQUE_MIN,
            "min_transparent_fraction": MIN_TRANSPARENT_FRACTION,
            "reconstruction_mae_max": RECONSTRUCTION_MAE_MAX,
            "tile_seam_excess_max": TILE_SEAM_EXCESS_MAX,
            "content_luma_std_max": CONTENT_LUMA_STD_MAX,
            "content_contrast_min": CONTENT_CONTRAST_MIN,
            "state_iou_min": STATE_IOU_MIN,
            "state_size_delta_max_px": STATE_SIZE_DELTA_MAX_PX,
            "state_distinct_min": STATE_DISTINCT_MIN,
            "band_mean_tol": BAND_MEAN_TOL,
            "extent_smooth_px": EXTENT_SMOOTH_PX,
            "extent_spread_factor": EXTENT_SPREAD_FACTOR,
            "extent_widen_fraction": EXTENT_WIDEN_FRACTION,
            "strip_px": STRIP_PX,
            "mask_threshold": MASK_THRESHOLD,
            "safe_rect_tol": SAFE_RECT_TOL,
            "safe_rect_spread_factor": SAFE_RECT_SPREAD_FACTOR,
            "safe_rect_max_shrink": SAFE_RECT_MAX_SHRINK,
        },
        "pixel_rewrite_performed": False,
    }


def canonicalize_atlas_image(data: bytes, role: AtlasRole) -> tuple[bytes, dict[str, object]]:
    """Normalize only the admitted alpha boundary, the way the inventory panel is normalized.

    Already-transparent exterior goes to alpha 0, each admitted content rect to alpha 255, and
    the admitted edge bands (between the corners, inside the border line) have their
    already-opaque pixels clamped to 255. Nothing infers a silhouette; corners and the outer
    edge keep whatever chamfer, cap, or antialiasing they were drawn with.
    """

    source_facts = validate_atlas_image(data, role)
    with Image.open(io.BytesIO(data)) as opened:
        image = opened.convert("RGBA")
    alpha = image.getchannel("A").point(
        lambda value: 0 if value <= TRANSPARENT_ADMISSION_MAX else value
    )
    insets = Insets(**cast(dict[str, int], source_facts["insets"]))
    for entry in cast(list[dict[str, object]], source_facts["cells"]):
        rect = cast(dict[str, int], entry["content_rect"])
        alpha.paste(255, Rect(rect["x"], rect["y"], rect["width"], rect["height"]).box)
        cell = cast(dict[str, int], entry["cell"])
        for band in _band_boxes(cell, insets):
            region = alpha.crop(band).point(
                lambda value: 255 if value >= BAND_OPAQUE_MIN else value
            )
            alpha.paste(region, band)
    image.putalpha(alpha)
    output = io.BytesIO()
    image.save(output, format="PNG", optimize=False)
    canonical_data = output.getvalue()
    canonical_facts = validate_atlas_image(canonical_data, role)
    return canonical_data, {
        "source": source_facts,
        "canonical": canonical_facts,
        "pixel_rewrite_performed": True,
        "pixel_rewrite": "alpha_boundary_normalization_v1",
    }


def _band_boxes(cell: dict[str, int], insets: Insets) -> tuple[tuple[int, int, int, int], ...]:
    """The four edge-band rectangles of a cell in sheet coordinates: between the corners, and
    inside the border line so the outer antialiased edge is never rewritten."""

    x, y, w, h = cell["x"], cell["y"], cell["width"], cell["height"]
    li, t, r, b = insets.left, insets.top, insets.right, insets.bottom
    edge = BORDER_EDGE_PX
    return (
        (x + li, y + edge, x + w - r, y + t),
        (x + li, y + h - b, x + w - r, y + h - edge),
        (x + edge, y + t, x + li, y + h - b),
        (x + w - r, y + t, x + w - edge, y + h - b),
    )


def atlas_evidence(data: bytes, facts: dict[str, object]) -> bytes:
    """Reviewer evidence: the raw sheet over a checkerboard, and each cell re-drawn through
    the admitted nine-slice at two other sizes, then reduced by the role's draw scale so the
    reviewer judges the density a consumer actually shows."""

    with Image.open(io.BytesIO(data)) as opened:
        sheet = opened.convert("RGBA")
    insets = Insets(**cast(dict[str, int], facts["insets"]))
    fill = cast(BandFill, facts["band_fill"])
    draw_scale = ATLAS_ROLES[str(facts["role"])].draw_scale
    cells = [
        cast(dict[str, int], entry["cell"])
        for entry in cast(list[dict[str, object]], facts["cells"])
    ]
    samples: list[Image.Image] = []
    for rect in cells:
        cell = sheet.crop(Rect(rect["x"], rect["y"], rect["width"], rect["height"]).box)
        wide = nine_slice_render(
            cell, insets, min(sheet.width, rect["width"] + 320), rect["height"], fill
        )
        tall = nine_slice_render(
            cell,
            insets,
            max(2 * (insets.left + insets.right) + 64, rect["width"] // 2),
            rect["height"] + 96,
            fill,
        )
        for sample in (wide, tall):
            samples.append(
                sample.resize(
                    (max(1, sample.width // draw_scale), max(1, sample.height // draw_scale)),
                    Image.Resampling.LANCZOS,
                )
            )
    gap = 24
    right_w = max(s.width for s in samples)
    right_h = sum(s.height + gap for s in samples)
    canvas = _checkerboard((sheet.width + gap + right_w, max(sheet.height, right_h)))
    canvas.alpha_composite(sheet, (0, 0))
    y = 0
    for sample in samples:
        canvas.alpha_composite(sample, (sheet.width + gap, y))
        y += sample.height + gap
    stream = io.BytesIO()
    canvas.convert("RGB").save(stream, format="PNG", optimize=False)
    return stream.getvalue()


def _checkerboard(size: tuple[int, int]) -> Image.Image:
    image = Image.new("RGBA", size, (220, 220, 220, 255))
    draw = ImageDraw.Draw(image)
    block = 20
    for y in range(0, size[1], block):
        for x in range(0, size[0], block):
            if (x // block + y // block) % 2:
                draw.rectangle((x, y, x + block - 1, y + block - 1), fill=(174, 174, 174, 255))
    return image


def atlas_role_contract(facts: dict[str, object]) -> dict[str, object]:
    """Project the resolved geometry the manifest binds beside the artifact."""

    return {
        "role": facts["role"],
        "layout": facts["layout"],
        "scale_mode": ATLAS_SCALE_MODE,
        "alpha_policy": ATLAS_ALPHA_POLICY,
        "band_fill": facts["band_fill"],
        "draw_scale": ATLAS_ROLES[str(facts["role"])].draw_scale,
        "canvas": facts["canvas"],
        "insets": facts["insets"],
        "cells": [
            {
                "state": entry["state"],
                "cell": entry["cell"],
                "content_rect": entry["content_rect"],
                "safe_rect": entry["safe_rect"],
            }
            for entry in cast(list[dict[str, object]], facts["cells"])
        ],
    }


__all__ = [
    "ATLAS_ALPHA_POLICY",
    "ATLAS_ROLES",
    "ATLAS_SCALE_MODE",
    "BUTTON_RECT",
    "BUTTON_RECT_LAYOUT",
    "PANEL_FRAME",
    "PANEL_FRAME_LAYOUT",
    "AtlasAdmissionError",
    "AtlasRole",
    "Insets",
    "Rect",
    "atlas_evidence",
    "atlas_role_contract",
    "canonicalize_atlas_image",
    "detect_cells",
    "nine_slice_render",
    "render_atlas_template",
    "safe_rect",
    "validate_atlas_image",
]
