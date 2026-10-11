"""Locked 47-mask terrain-atlas recipe and deterministic compositor.

The sheet is requested at an exact canvas and sliced on fixed boundaries. GPT Image 2.5
honours an exact ``size`` up to 3840 px and 3:1, and a 12-by-4 atlas is exactly 3:1, so a
2880-by-960 request comes back at 2880 by 960 and every cell edge is known before the
draw. That retires the cyan guide lattice, the fitted-residual admission and the
asymmetric crop inset, none of which were ever about the art.

It also retires magenta. The keep-out convention was designed before native alpha existed
and it had been quietly destroying the sheet: the locked template paints its rock
highlights in a pale pink that satisfies the chroma key, so every published atlas carried
holes where the key had eaten solid ground -- 31,701 transparent pixels, 4.68 per cent of
the forty-seven tiles, up to 19.2 per cent of one of them. The per-cell alpha that the
old admission compared against was that damage, not a silhouette: it is anti-correlated
with exposure, open along tops that are *covered* and closed along tops that are exposed,
and it collapses to six distinct shapes across forty-seven masks. A 3x3-minimal terrain
tile fills its cell. There is nothing to key, so the sheet is drawn and published opaque.
"""

from __future__ import annotations

import json
import math
import statistics
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from functools import lru_cache
from hashlib import sha256
from io import BytesIO
from typing import Final, cast

from PIL import Image, ImageChops, ImageDraw

from terrain_example.guide_lattice import detect_guide_lattice, png_bytes
from terrain_example.resources import terrain_atlas_lookup_path, terrain_atlas_template_path

GRID_COLUMNS: Final = 12
GRID_ROWS: Final = 4
CANONICAL_CELL_PX: Final = 120
PLACEHOLDER_CELL: Final = (10, 1)
MASK_ORDER: Final = ("nw", "n", "ne", "w", "center", "e", "sw", "s", "se")
TOPOLOGY_ID: Final = "terrain-atlas-3x3-minimal-v1"
MATERIAL_SOURCE_CONTRACT_ID: Final = "terrain-atlas-paintover-source-v9"
MATERIAL_ASSEMBLER_ID: Final = "terrain-atlas-paintover-canonicalization-v10"
PAINT_TARGET_ID: Final = "terrain-atlas-paint-target-v3"
#: A hairline fence at every cell boundary, in a colour that cannot be terrain. Not for
#: registration - the exact canvas settles that - but to tell the brush where a tile ends.
#: Removing it was the largest non-model change between the atlas that worked and the one
#: that did not: without it the model paints the sheet as one canvas, and a cell whose side
#: should terminate never gets a face at all. Measured on the sheet it replaces, colour
#: right across a boundary differed by 2.78 where half a cell apart differed by 8.35, and
#: cells whose bottom is exposed sat 2.13 from the cell below - their undersides were the
#: neighbour's material, not an underside. Three pixels, not the sixteen a grey channel was
#: tried at: a hairline in an impossible colour reads as a line to keep, a wide neutral
#: channel reads as a gap between objects and the model frames every tile.
GUIDE_RGB: Final = (0, 255, 255)
GUIDE_WIDTH_PX: Final = 3
#: Cut inside the fence rather than through it. The line sits at a known coordinate, so
#: this is arithmetic, not detection. Twelve, not the three the fence is drawn at: the
#: model returns the line thickened to roughly eight pixels with a soft skirt, and a
#: four-pixel cut published 11,047 fence-coloured pixels into the tiles. Measured on the
#: returned sheet, fence colour falls away by twelve.
GUIDE_INSET_PX: Final = 12
#: Anything of the fence that survives the cut is replaced from its neighbours rather than
#: published. Eight-neighbour, so one pass covers one pixel in any direction.
_DEFRINGE_PASSES: Final = 6
_DEFRINGE_OFFSETS: Final = ((1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1), (1, -1), (-1, 1))
#: The provider canvas. Twice the publication pitch, so every cell is supersampled once
#: on the way down, and exactly 3:1 -- the widest ratio the OpenAI image route accepts.
SOURCE_CELL_PX: Final = 240
PAINT_CANVAS_WIDTH: Final = GRID_COLUMNS * SOURCE_CELL_PX
PAINT_CANVAS_HEIGHT: Final = GRID_ROWS * SOURCE_CELL_PX
PAINT_CANVAS_SIZE: Final = f"{PAINT_CANVAS_WIDTH}x{PAINT_CANVAS_HEIGHT}"
MINIMUM_PAINTED_MATERIAL_STANDARD_DEVIATION: Final = 2.0
#: How deep either side of a join the tone comparison reads.
JOIN_TONE_STRIP_PX: Final = 12
#: The scale a repeated feature becomes countable at. The tile is read in blocks this size,
#: so grain survives as material and a pebble survives as an object.
QUIET_BLOCK_PX: Final = 10
#: How far a block must sit from the tile's own median colour to count as an object rather
#: than as material. Mean channel distance, so a grey boulder on brown earth counts: a
#: luminance-only reading missed exactly that, because the two are thirteen units apart in
#: brightness and a whole hillside came back chained with boulders under a gate that passed.
_OBJECT_BLOCK_DISTANCE: Final = 12.0
#: Recorded, never refused. Three attempts to make this a gate produced two regressions
#: and no working threshold. At 0.10 it refused the published atlas whose material reads
#: best (0.194) and the contract came back asking for gravel: largest feature 20 px of a
#: 120 px tile against 52 in the atlas it replaced. At 0.45 it fought the fabric the prompt
#: now asks for and exhausted a node's whole retry budget on a material that draws big
#: slabs. And the ordering never worked: the atlas a reviewer liked scores 0.157 on its
#: largest region, a hillside visibly chained with repeated boulders scores 0.179, and a
#: sheet that reads well scores 0.345. Whether a repeat is legible is an aesthetic
#: judgement - a course of slabs reads as a wall, one boulder reads as a copy - and no
#: statistic tried here separates them. The number stays in the record because it is worth
#: comparing across runs; the judgement belongs to the semantic review.
#: The worst mean-channel tone step allowed across any join the validation maps can make.
#: Calibrated on published atlases rather than on a new draw: the two whose material a
#: semantic reviewer accepted measure 26.3, the one the reviewer complained about measures
#: 84.6, and a repeat draw that came back visibly patchy measures 92.8.
MAXIMUM_JOIN_TONE_STEP: Final = 65.0

Mask = tuple[int, int, int, int, int, int, int, int, int]
Coordinate = tuple[int, int]
Occupancy = tuple[tuple[bool, ...], ...]


@dataclass(frozen=True, slots=True)
class TerrainAtlasLookup:
    by_mask: Mapping[Mask, Coordinate]
    placeholder_cell: Coordinate


_VALIDATION_MAPS: Final[dict[str, tuple[str, ...]]] = {
    "solid_ground": (
        "1111111111",
        "1111111111",
        "1111111111",
        "1111111111",
    ),
    "one_cell_floating": (
        "0000000000",
        "0111111110",
        "0000000000",
    ),
    "steps": (
        "0000000111",
        "0000011111",
        "0001111111",
        "0111111111",
    ),
    "concavity_and_hole": (
        "0011111100",
        "0110000110",
        "1110110111",
        "1111111111",
    ),
}


def load_terrain_atlas_lookup(data: bytes | None = None) -> TerrainAtlasLookup:
    """Load the authoritative lookup and reject incomplete or ambiguous variants."""

    payload = json.loads(
        (terrain_atlas_lookup_path().read_bytes() if data is None else data).decode("utf-8")
    )
    if payload.get("kind") != "terrain-atlas-3x3-minimal-lookup-v1":
        raise ValueError("terrain lookup identity is invalid")
    if tuple(payload.get("mask_order", ())) != MASK_ORDER:
        raise ValueError("terrain lookup mask order is invalid")
    if payload.get("terrain_mask_count") != 47:
        raise ValueError("terrain lookup must declare exactly 47 masks")
    if tuple(payload.get("placeholder_cell", ())) != PLACEHOLDER_CELL:
        raise ValueError("terrain lookup placeholder cell is invalid")
    raw_lookup = payload.get("lookup")
    if not isinstance(raw_lookup, dict):
        raise ValueError("terrain lookup entries are missing")
    lookup: dict[Mask, Coordinate] = {}
    for raw_mask, raw_coordinate in raw_lookup.items():
        if not isinstance(raw_mask, str) or len(raw_mask) != 9 or set(raw_mask) - {"0", "1"}:
            raise ValueError("terrain lookup contains an invalid mask")
        if not isinstance(raw_coordinate, list) or len(raw_coordinate) != 2:
            raise ValueError("terrain lookup contains an invalid coordinate")
        mask = cast(Mask, tuple(int(bit) for bit in raw_mask))
        coordinate = cast(Coordinate, tuple(int(value) for value in raw_coordinate))
        if mask in lookup:
            raise ValueError("terrain lookup contains duplicate masks")
        if coordinate == PLACEHOLDER_CELL or not (
            0 <= coordinate[0] < GRID_COLUMNS and 0 <= coordinate[1] < GRID_ROWS
        ):
            raise ValueError("terrain lookup contains a reserved or out-of-range coordinate")
        nw, n, ne, w, center, e, sw, s, se = mask
        if center != 1 or (
            (nw and not (n and w))
            or (ne and not (n and e))
            or (sw and not (s and w))
            or (se and not (s and e))
        ):
            raise ValueError("terrain lookup contains an invalid 3x3-minimal mask")
        lookup[mask] = coordinate
    if len(lookup) != 47 or len(set(lookup.values())) != 47:
        raise ValueError("terrain lookup masks and coordinates must both be unique and complete")
    expected = set(_reachable_masks())
    if set(lookup) != expected:
        missing = len(expected - set(lookup))
        extra = len(set(lookup) - expected)
        raise ValueError(f"terrain lookup reachability mismatch: {missing} missing, {extra} extra")
    return TerrainAtlasLookup(by_mask=lookup, placeholder_cell=PLACEHOLDER_CELL)


def _reachable_masks() -> tuple[Mask, ...]:
    masks: set[Mask] = set()
    for cardinal in range(16):
        n = (cardinal >> 0) & 1
        e = (cardinal >> 1) & 1
        s = (cardinal >> 2) & 1
        w = (cardinal >> 3) & 1
        possible = (
            ("nw", n and w),
            ("ne", n and e),
            ("sw", s and w),
            ("se", s and e),
        )
        enabled = [entry for entry in possible if entry[1]]
        for diagonal_bits in range(1 << len(enabled)):
            diagonals = {name: 0 for name, _ in possible}
            for index, (name, _) in enumerate(enabled):
                diagonals[name] = (diagonal_bits >> index) & 1
            masks.add(
                (
                    diagonals["nw"],
                    n,
                    diagonals["ne"],
                    w,
                    1,
                    e,
                    diagonals["sw"],
                    s,
                    diagonals["se"],
                )
            )
    return tuple(sorted(masks))


@lru_cache(maxsize=4)
def _paint_target(template_bytes: bytes) -> bytes:
    with Image.open(BytesIO(template_bytes)) as opened:
        template = opened.convert("RGB")
    lattice = detect_guide_lattice(template, expected_columns=GRID_COLUMNS, expected_rows=GRID_ROWS)
    packed = Image.new("RGB", (GRID_COLUMNS * CANONICAL_CELL_PX, GRID_ROWS * CANONICAL_CELL_PX))
    # The reserved cell is packed as ordinary buried ground, not as the template's checker.
    # A grey checker is the universal picture of transparency, and the model reads it that
    # way: handed one, it painted that cell in a different material from every other cell on
    # the sheet. Nothing needs it - the coordinate is reserved, the lookup never selects it,
    # and the compositor clears it - so it never reaches the provider.
    filler = load_terrain_atlas_lookup().by_mask[(1,) * 9]
    for row in range(GRID_ROWS):
        for column in range(GRID_COLUMNS):
            source_column, source_row = (
                filler if (column, row) == PLACEHOLDER_CELL else (column, row)
            )
            crop = template.crop(
                (
                    lattice.x_lines[source_column][1] + 3,
                    lattice.y_lines[source_row][1] + 3,
                    lattice.x_lines[source_column + 1][0] - 2,
                    lattice.y_lines[source_row + 1][0] - 2,
                )
            ).resize((CANONICAL_CELL_PX, CANONICAL_CELL_PX), Image.Resampling.LANCZOS)
            packed.paste(crop, (column * CANONICAL_CELL_PX, row * CANONICAL_CELL_PX))
    # Nearest on the way up, not Lanczos: the source is pixel art, and a crisp doubling
    # reads to the model as one drawing where a blurred one reads as a photograph of one.
    sheet = packed.resize((PAINT_CANVAS_WIDTH, PAINT_CANVAS_HEIGHT), Image.Resampling.NEAREST)
    draw = ImageDraw.Draw(sheet)
    half = GUIDE_WIDTH_PX // 2
    for column in range(GRID_COLUMNS + 1):
        x = min(max(column * SOURCE_CELL_PX, half), PAINT_CANVAS_WIDTH - half - 1)
        draw.line([(x, 0), (x, PAINT_CANVAS_HEIGHT - 1)], fill=GUIDE_RGB, width=GUIDE_WIDTH_PX)
    for row in range(GRID_ROWS + 1):
        y = min(max(row * SOURCE_CELL_PX, half), PAINT_CANVAS_HEIGHT - half - 1)
        draw.line([(0, y), (PAINT_CANVAS_WIDTH - 1, y)], fill=GUIDE_RGB, width=GUIDE_WIDTH_PX)
    return png_bytes(sheet)


def terrain_atlas_paint_target(template: bytes | None = None) -> bytes:
    """The locked template's 48 cells, packed edge to edge at the provider canvas.

    Derived rather than committed, so the packed sheet cannot drift from the template it
    comes from and the Godot documentation lineage stays attached to one file.

    Four rounds of a locally drawn block guide -- flat cap and rim bands standing for
    which of a cell's faces meet air -- were measured against this, and every one of them
    invented a literalism from the legend: a rim darker than the fill came back painted as
    a shadow gap between blocks, a corner mark as a stone cube sitting in the cell, a rim
    lighter than the fill as a cream frame drawn around every tile. The template needs no
    legend. It is already the answer -- all forty-seven finishes, corner turns included --
    drawn in the wrong style, and restyling a correct picture is the thing an image model
    is reliably good at.
    """

    return _paint_target(
        terrain_atlas_template_path().read_bytes() if template is None else template
    )


def terrain_atlas_generation_prompt(material_direction: str) -> str:
    """Bind biome direction to a strict model-painted 47-mask atlas contract.

    Structure comes from the paint target and nothing else; every word here is spent on
    the art. Two paid rounds established the split. Asked to hold the topology in words
    instead - once as a table of all forty-eight cells, once as sixteen prose runs over an
    exposure-ordered sheet - GPT Image 2.5 followed the wording closely and drew the
    wording: grooves where the prompt said "grid", marks where it said "notched", and, on
    the prose run, sky-blue showing through wherever it read "exposed underside", because
    a sheet described in words composes as a picture. Nine and four of the fifty-two faces
    that must be finished came back unfinished, against one for the same material drawn
    over the target. The same precision spent on the painting instead took every one of
    those fifty-two faces (0 unfinished), the worst join tone step from 22.5 to 19.4 and
    the mean from 5.9 to 4.9.
    """

    material = " ".join(material_direction.split())
    if not material:
        raise ValueError("terrain material direction must not be empty")
    return (
        "Use case: stylized-concept\n"
        "Asset type: production 2D side-view terrain tile atlas\n\n"
        "Repaint reference image 1 completely. It is a working terrain tile sheet: 12 "
        "columns by 4 rows of equal square tiles filling the canvas edge to edge, already "
        "correct in every structural respect and wrong only in its art. Every remaining "
        "image is an appearance reference: take its rendering quality, palette, material "
        "language, world scale and lighting restraint, never its scene composition. Create "
        f"original, brand-neutral terrain with this authored direction: {material}\n\n"
        "WHAT THIS SHEET IS\n"
        "Forty-eight cut-out tiles of one single ground material, not one picture and not a "
        "landscape. There is no skyline and no ground level on this sheet. Each tile is "
        "lifted out on its own and butted against any other tile, so two tiles that were "
        "never neighbours here will be neighbours in the game.\n\n"
        "WHAT TO KEEP FROM REFERENCE IMAGE 1\n"
        "The thin cyan lines are the fence between one tile and the next. Keep every one of "
        "them exactly where it is, straight, unbroken and the same width. Paint only inside "
        "the cells they enclose. Never paint over a line, never let anything cross one, and "
        "never let a tile's material run through into the tile beyond.\n"
        "And the structure, exactly: which of each tile's four sides is a finished face, "
        "which is a cut through solid ground running to the fence, and which corners the "
        "finish turns. Keep the 12 by 4 grid exactly where it is and do not move, rescale, "
        "rotate or crop it. Take nothing else from it - not its palette, its pixel-art "
        "finish, its dithering, nor the places it puts tufts, sprigs, highlights and "
        "speckles.\n\n"
        "WHAT TO PAINT\n"
        "Every cell is filled with terrain to all four of its edges. There is no sky, no "
        "background, no empty space and no transparency anywhere on the canvas.\n"
        "A finished top is the walk surface the player stands on. A finished side or bottom "
        "is where the mass ends in mid-air: paint a terminating edge with its own bevel and "
        "the shadow it casts. Light comes from the upper left, everywhere on the sheet, with "
        "no exception.\n"
        "A side that is not finished is a cut through solid ground. The material runs off "
        "that edge mid-stride, stopping at the fence. Nothing happens there: no surface, no "
        "cap, no growth, no bevel, no rim, no outline and no change of tone. Keep the "
        "material calm and even along every one of those edges, at the same brightness in "
        "every tile, so any two of them can meet and no one can see where.\n"
        "The cyan fence is the only thing that marks a tile boundary. Do not add one of your "
        "own anywhere: no groove, no seam, no frame, no moulding, no aligned row of stones "
        "and no tick at a corner.\n"
        "Every cell shows the same material at the same scale, under the same light, at the "
        "same overall brightness. Cells differ only in which of their sides are finished. "
        "Never give a cell its own substance, palette, value or composition.\n"
        "One cell is about 1.2 metres of ground, and it is built from a regular fabric: "
        "slabs, courses, cobbles, strata, whatever the direction calls for. Work at a size "
        "you could stand on - the biggest slab or stone about half a cell across. Gravel and "
        "scattered pebbles read as ground seen from far away and leave the body flat and "
        "empty. Nothing spans a cell boundary.\n"
        "Cells with no finished face are buried ground, laid side by side and stacked to "
        "fill whole hillsides, so the same square appears many times on one screen. Give "
        "them the same fabric as everywhere else, worked evenly across the whole square: a "
        "repeated course of slabs reads as a wall, which is right, while a repeated single "
        "flower or a lone bright pebble reads as a copy. Save the growth and the litter for "
        "the finished tops, sparingly.\n\n"
        "FINISH\n"
        "Polished hand-painted 2D game art with purposeful edge bevels, restrained local "
        "variation and broad quiet areas. Avoid flat texture stamping, mirrored repetition, "
        "repeated boulder rows and pixel art. No characters, buildings, scenery, text, "
        "labels, UI, logos, signatures or watermarks."
    )


def terrain_atlas_cells(painted_source: bytes) -> dict[Coordinate, Image.Image]:
    """Slice the provider canvas on fixed boundaries into opaque publication cells."""

    with Image.open(BytesIO(painted_source)) as opened:
        source = opened.convert("RGB")
    if source.size != (PAINT_CANVAS_WIDTH, PAINT_CANVAS_HEIGHT):
        raise ValueError(
            "terrain atlas source must be exactly "
            f"{PAINT_CANVAS_WIDTH}x{PAINT_CANVAS_HEIGHT}, got {source.width}x{source.height}"
        )
    cells: dict[Coordinate, Image.Image] = {}
    for row in range(GRID_ROWS):
        for column in range(GRID_COLUMNS):
            if (column, row) == PLACEHOLDER_CELL:
                cells[(column, row)] = Image.new(
                    "RGBA", (CANONICAL_CELL_PX, CANONICAL_CELL_PX), (0, 0, 0, 0)
                )
                continue
            cell = (
                source.crop(
                    (
                        column * SOURCE_CELL_PX + GUIDE_INSET_PX,
                        row * SOURCE_CELL_PX + GUIDE_INSET_PX,
                        (column + 1) * SOURCE_CELL_PX - GUIDE_INSET_PX,
                        (row + 1) * SOURCE_CELL_PX - GUIDE_INSET_PX,
                    )
                )
                .resize((CANONICAL_CELL_PX, CANONICAL_CELL_PX), Image.Resampling.LANCZOS)
                .convert("RGBA")
            )
            cells[(column, row)], _cleaned = _defringe(cell)
    return cells


#: How far in from a cell edge the fence can still have tinted the paint. Measured on the
#: published sheets: every cyan-cast pixel sat within seven.
_FENCE_MARGIN_PX: Final = 9


def _is_fence(pixel: tuple[int, int, int], *, near_edge: bool) -> bool:
    """Fence colour, read strictly in a cell's middle and by cast near its edge.

    The strict reading alone published 1,611 tinted pixels into one atlas. Saturated fence
    over cream terrain lands around (180, 215, 210) - a pale teal, nothing like the line it
    came from, and a red channel far above any threshold that would catch the line itself.
    Near an edge the test is therefore the cast rather than the colour: green and blue both
    well above red is cyan, which this palette's foliage (green over red, blue below) and
    its stone, soil and brass never are. Away from an edge the strict reading stands, so a
    turquoise the direction actually asked for survives in the middle of a tile.
    """

    red, green, blue = pixel[0], pixel[1], pixel[2]
    if near_edge:
        return green - red > 25 and blue - red > 25
    return red < 90 and green > 150 and blue > 150


def _defringe(cell: Image.Image) -> tuple[Image.Image, int]:
    """Replace any surviving fence colour from the material around it."""

    rgba = cell.convert("RGBA")
    edge = CANONICAL_CELL_PX - 1
    mask = [
        0
        if _is_fence(
            pixel[:3],
            near_edge=min(
                index % CANONICAL_CELL_PX,
                edge - index % CANONICAL_CELL_PX,
                index // CANONICAL_CELL_PX,
                edge - index // CANONICAL_CELL_PX,
            )
            < _FENCE_MARGIN_PX,
        )
        else 255
        for index, pixel in enumerate(
            cast(Iterable[tuple[int, int, int, int]], rgba.get_flattened_data())
        )
    ]
    cleaned = sum(1 for value in mask if value == 0)
    if not cleaned:
        return rgba, 0
    alpha = Image.new("L", rgba.size)
    alpha.putdata(mask)
    holed = rgba.copy()
    holed.putalpha(alpha)
    for _ in range(_DEFRINGE_PASSES):
        extrema = cast(tuple[int, int], holed.getchannel("A").getextrema())
        if extrema[0] > 0:
            break
        for offset in _DEFRINGE_OFFSETS:
            holed = Image.alpha_composite(ImageChops.offset(holed, *offset), holed)
    filled = holed.convert("RGB").convert("RGBA")
    filled.putalpha(rgba.getchannel("A"))
    return filled, cleaned


def _painted_standard_deviation(cells: Mapping[Coordinate, Image.Image]) -> float:
    """Mean per-channel spread over every published pixel: does this sheet carry paint."""

    totals = [0.0, 0.0, 0.0]
    squared = [0.0, 0.0, 0.0]
    samples = 0
    for coordinate, cell in cells.items():
        if coordinate == PLACEHOLDER_CELL:
            continue
        for pixel in cast(Iterable[tuple[int, int, int]], cell.convert("RGB").get_flattened_data()):
            for channel, value in enumerate(pixel):
                totals[channel] += value
                squared[channel] += value * value
            samples += 1
    if samples == 0:
        return 0.0
    deviations = [
        math.sqrt(max(0.0, squared[channel] / samples - (totals[channel] / samples) ** 2))
        for channel in range(3)
    ]
    return sum(deviations) / len(deviations)


def _object_share(cell: Image.Image) -> float:
    """The share of a tile that reads as a distinct object rather than as its material."""

    edge = CANONICAL_CELL_PX // QUIET_BLOCK_PX
    blocks = [
        tuple(block)
        for block in cast(
            Iterable[tuple[int, int, int]],
            cell.convert("RGB").resize((edge, edge), Image.Resampling.BOX).get_flattened_data(),
        )
    ]
    median = [statistics.median(block[channel] for block in blocks) for channel in range(3)]
    distant = sum(
        1
        for block in blocks
        if sum(abs(block[channel] - median[channel]) for channel in range(3)) / 3
        > _OBJECT_BLOCK_DISTANCE
    )
    return distant / len(blocks)


def _buried_quiet_facts(
    cells: Mapping[Coordinate, Image.Image],
    lookup: TerrainAtlasLookup,
) -> tuple[float, float]:
    """The hillside tile's object share, and the mean over the whole buried family."""

    by_coordinate = {coordinate: mask for mask, coordinate in lookup.by_mask.items()}
    buried = [
        _object_share(cell)
        for coordinate, cell in cells.items()
        if coordinate != PLACEHOLDER_CELL
        and all(by_coordinate[coordinate][bit] for bit in (1, 3, 5, 7))
    ]
    hillside = _object_share(cells[lookup.by_mask[(1,) * 9]])
    return hillside, sum(buried) / len(buried)


def _strip_mean(cell: Image.Image, side: str) -> tuple[float, float, float]:
    boxes = {
        "left": (0, 0, JOIN_TONE_STRIP_PX, CANONICAL_CELL_PX),
        "right": (CANONICAL_CELL_PX - JOIN_TONE_STRIP_PX, 0, CANONICAL_CELL_PX, CANONICAL_CELL_PX),
        "top": (0, 0, CANONICAL_CELL_PX, JOIN_TONE_STRIP_PX),
        "bottom": (0, CANONICAL_CELL_PX - JOIN_TONE_STRIP_PX, CANONICAL_CELL_PX, CANONICAL_CELL_PX),
    }
    pixels = list(
        cast(
            Iterable[tuple[int, int, int]],
            cell.convert("RGB").crop(boxes[side]).get_flattened_data(),
        )
    )
    return cast(
        tuple[float, float, float],
        tuple(sum(pixel[channel] for pixel in pixels) / len(pixels) for channel in range(3)),
    )


def _join_tone_metrics(
    cells: Mapping[Coordinate, Image.Image],
    lookup: TerrainAtlasLookup,
) -> tuple[float, float]:
    """The worst and mean tone step over every join the validation maps can make.

    Deliberately not the per-pixel connector comparison. On hand-painted material that one
    is dominated by texture -- a pale flagstone meeting a dark mortar line reads as a large
    error while the two tiles are in fact the same material at the same value -- and it is
    computed after the connector harmoniser has overwritten the very pixels it samples, so
    it reads zero on every draw including the patchy ones. What a player sees is whole-cell
    tone drift, so the strip either side of a join is averaged before it is compared.
    """

    steps: list[float] = []
    for rows in _VALIDATION_MAPS.values():
        occupied = parse_binary_rows(rows)
        height, width = len(occupied), len(occupied[0])
        for y in range(height):
            for x in range(width):
                if not occupied[y][x]:
                    continue
                here = lookup.by_mask[peering_mask(occupied, x, y)]
                for neighbour, near, far in (
                    ((x + 1, y), "right", "left"),
                    ((x, y + 1), "bottom", "top"),
                ):
                    nx, ny = neighbour
                    if not (nx < width and ny < height and occupied[ny][nx]):
                        continue
                    other = lookup.by_mask[peering_mask(occupied, nx, ny)]
                    if PLACEHOLDER_CELL in (here, other):
                        continue
                    first = _strip_mean(cells[here], near)
                    second = _strip_mean(cells[other], far)
                    steps.append(sum(abs(a - b) for a, b in zip(first, second, strict=True)) / 3)
    if not steps:
        return 0.0, 0.0
    return max(steps), sum(steps) / len(steps)


def require_terrain_atlas_source(
    raw: bytes,
    *,
    template: bytes | None = None,
) -> dict[str, object]:
    """Reject a model draw that cannot be safely sliced and published."""

    target = terrain_atlas_paint_target(template)
    cells = terrain_atlas_cells(raw)
    lookup = load_terrain_atlas_lookup()
    material_standard_deviation = _painted_standard_deviation(cells)
    worst_join, mean_join = _join_tone_metrics(cells, lookup)
    hillside_share, buried_share = _buried_quiet_facts(cells, lookup)
    if material_standard_deviation < MINIMUM_PAINTED_MATERIAL_STANDARD_DEVIATION:
        raise ValueError("terrain atlas source lacks usable painted material variation")
    if worst_join > MAXIMUM_JOIN_TONE_STEP:
        raise ValueError(
            "terrain atlas source tiles do not share one tone: worst join step "
            f"{worst_join:.1f} exceeds {MAXIMUM_JOIN_TONE_STEP}"
        )
    return {
        "schema_version": 1,
        "kind": "terrain-atlas-paintover-source-validation-v1",
        "contract": MATERIAL_SOURCE_CONTRACT_ID,
        "source": {
            "sha256": sha256(raw).hexdigest(),
            "width": PAINT_CANVAS_WIDTH,
            "height": PAINT_CANVAS_HEIGHT,
            "mode": "RGB",
        },
        "paint_target": {
            "id": PAINT_TARGET_ID,
            "sha256": sha256(target).hexdigest(),
            "cell_px": SOURCE_CELL_PX,
        },
        "registration": "fixed-pitch-exact-canvas-v1",
        "worst_join_tone_step": round(worst_join, 4),
        "mean_join_tone_step": round(mean_join, 4),
        "hillside_tile_object_share": round(hillside_share, 4),
        "buried_tile_object_share_mean": round(buried_share, 4),
        "painted_material_mean_standard_deviation": round(material_standard_deviation, 6),
        "thresholds": {
            "paint_canvas": PAINT_CANVAS_SIZE,
            "maximum_join_tone_step": MAXIMUM_JOIN_TONE_STEP,
            "minimum_painted_material_standard_deviation": (
                MINIMUM_PAINTED_MATERIAL_STANDARD_DEVIATION
            ),
        },
    }


def parse_binary_rows(rows: Sequence[str]) -> Occupancy:
    if not rows or not rows[0] or any(len(row) != len(rows[0]) for row in rows):
        raise ValueError("binary terrain rows must be a nonempty rectangle")
    if any(set(row) - {"0", "1"} for row in rows):
        raise ValueError("binary terrain rows may contain only zero and one")
    return tuple(tuple(value == "1" for value in row) for row in rows)


def peering_mask(occupied: Occupancy, x: int, y: int) -> Mask:
    height, width = len(occupied), len(occupied[0])

    def at(px: int, py: int) -> int:
        return int(0 <= px < width and 0 <= py < height and occupied[py][px])

    n, e, s, w = at(x, y - 1), at(x + 1, y), at(x, y + 1), at(x - 1, y)
    return (
        n and w and at(x - 1, y - 1),
        n,
        n and e and at(x + 1, y - 1),
        w,
        1,
        e,
        s and w and at(x - 1, y + 1),
        s,
        s and e and at(x + 1, y + 1),
    )


def compose_terrain(
    occupied: Occupancy,
    cells: Mapping[Coordinate, Image.Image],
    lookup: TerrainAtlasLookup,
) -> tuple[Image.Image, tuple[tuple[Coordinate | None, ...], ...]]:
    height, width = len(occupied), len(occupied[0])
    image = Image.new("RGBA", (width * CANONICAL_CELL_PX, height * CANONICAL_CELL_PX))
    coordinates: list[tuple[Coordinate | None, ...]] = []
    for y, row in enumerate(occupied):
        output_row: list[Coordinate | None] = []
        for x, solid in enumerate(row):
            if not solid:
                output_row.append(None)
                continue
            mask = peering_mask(occupied, x, y)
            coordinate = lookup.by_mask.get(mask)
            if coordinate is None:
                mask_text = "".join(map(str, mask))
                raise ValueError(f"terrain lookup has no coordinate for mask {mask_text}")
            cell = cells.get(coordinate)
            if cell is None:
                raise ValueError(f"terrain atlas is missing cell {coordinate}")
            image.alpha_composite(cell, (x * CANONICAL_CELL_PX, y * CANONICAL_CELL_PX))
            output_row.append(coordinate)
        coordinates.append(tuple(output_row))
    return image, tuple(coordinates)


def _connector_metrics(image: Image.Image, occupied: Occupancy) -> dict[str, float | int]:
    rgba = image.convert("RGBA")
    start = round(CANONICAL_CELL_PX * 0.40)
    end = round(CANONICAL_CELL_PX * 0.60)
    alpha_mismatches = alpha_samples = shared_edges = rgb_total = rgb_samples = 0
    height, width = len(occupied), len(occupied[0])
    for y in range(height):
        for x in range(width):
            if not occupied[y][x]:
                continue
            pairs: list[tuple[tuple[int, int], tuple[int, int]]] = []
            if x + 1 < width and occupied[y][x + 1]:
                boundary = (x + 1) * CANONICAL_CELL_PX
                pairs.extend(
                    (
                        (boundary - 1, y * CANONICAL_CELL_PX + offset),
                        (boundary, y * CANONICAL_CELL_PX + offset),
                    )
                    for offset in range(start, end)
                )
                shared_edges += 1
            if y + 1 < height and occupied[y + 1][x]:
                boundary = (y + 1) * CANONICAL_CELL_PX
                pairs.extend(
                    (
                        (x * CANONICAL_CELL_PX + offset, boundary - 1),
                        (x * CANONICAL_CELL_PX + offset, boundary),
                    )
                    for offset in range(start, end)
                )
                shared_edges += 1
            for first_at, second_at in pairs:
                first = cast(tuple[int, int, int, int], rgba.getpixel(first_at))
                second = cast(tuple[int, int, int, int], rgba.getpixel(second_at))
                first_solid, second_solid = first[3] > 128, second[3] > 128
                alpha_mismatches += int(first_solid != second_solid)
                alpha_samples += 1
                if first_solid and second_solid:
                    rgb_total += sum(abs(first[index] - second[index]) for index in range(3))
                    rgb_samples += 3
    return {
        "shared_edges": shared_edges,
        "connector_band_fraction": 0.20,
        "connector_alpha_mismatch_fraction": alpha_mismatches / max(1, alpha_samples),
        "connector_mean_absolute_rgb_error": rgb_total / max(1, rgb_samples),
    }


def assemble_terrain_atlas(
    painted_source: bytes,
    *,
    template: bytes | None = None,
    lookup_data: bytes | None = None,
) -> tuple[bytes, dict[str, object]]:
    """Canonicalize a model-painted atlas: fixed-pitch slice, harmonized connectors."""

    template_bytes = terrain_atlas_template_path().read_bytes() if template is None else template
    lookup_bytes = terrain_atlas_lookup_path().read_bytes() if lookup_data is None else lookup_data
    source_validation = require_terrain_atlas_source(painted_source, template=template_bytes)
    lookup = load_terrain_atlas_lookup(lookup_bytes)
    # No connector harmonisation. The three-pixel median-profile blend that used to run
    # here was written for a chroma-keyed repaint of one template, where every cell shared
    # a colour and forcing the outermost pixels to a common profile was invisible. On a
    # hand-painted sheet the common profile is a colour no cell actually has, so it stamped
    # a pale lattice down every join - plainly visible in a composed map, and absent from
    # the same map composed straight from the slice. It also wrote the exact pixels the
    # direct-connector check then sampled, so that check read zero on every draw including
    # the patchy ones. Both are gone; the join-tone gate measures unrepaired cells.
    cells = terrain_atlas_cells(painted_source)

    connector_rgb_max = 0.0
    map_reports: dict[str, object] = {}
    for name, rows in _VALIDATION_MAPS.items():
        occupied = parse_binary_rows(rows)
        direct, coordinates = compose_terrain(occupied, cells, lookup)
        metrics = _connector_metrics(direct, occupied)
        connector_rgb_max = max(
            connector_rgb_max,
            cast(float, metrics["connector_mean_absolute_rgb_error"]),
        )
        map_reports[name] = {
            "rows": list(rows),
            "coordinates": [
                [list(coordinate) if coordinate is not None else None for coordinate in row]
                for row in coordinates
            ],
            "direct": metrics,
        }

    atlas = Image.new(
        "RGBA",
        (GRID_COLUMNS * CANONICAL_CELL_PX, GRID_ROWS * CANONICAL_CELL_PX),
        (0, 0, 0, 0),
    )
    for row in range(GRID_ROWS):
        for column in range(GRID_COLUMNS):
            coordinate = (column, row)
            if coordinate == PLACEHOLDER_CELL:
                continue
            atlas.alpha_composite(
                cells[coordinate],
                (column * CANONICAL_CELL_PX, row * CANONICAL_CELL_PX),
            )
    canonical = png_bytes(atlas)
    published_holes = sum(
        1
        for coordinate, cell in cells.items()
        if coordinate != PLACEHOLDER_CELL
        for value in cast(Iterable[int], cell.getchannel("A").get_flattened_data())
        if value <= 128
    )
    direct_pass = published_holes == 0
    report: dict[str, object] = {
        "schema_version": 1,
        "kind": "terrain-atlas-paintover-canonicalization-validation-v1",
        "topology": TOPOLOGY_ID,
        "canonicalizer": MATERIAL_ASSEMBLER_ID,
        "material_source_contract": MATERIAL_SOURCE_CONTRACT_ID,
        "classification": "direct_pass" if direct_pass else "reject",
        "dynamic_tilemap_compatible": direct_pass,
        "source": cast(dict[str, object], source_validation["source"]),
        "source_validation": source_validation,
        "template_sha256": sha256(template_bytes).hexdigest(),
        "paint_target_sha256": sha256(terrain_atlas_paint_target(template_bytes)).hexdigest(),
        "lookup_sha256": sha256(lookup_bytes).hexdigest(),
        "lookup_masks": len(lookup.by_mask),
        "canonical": {
            "sha256": sha256(canonical).hexdigest(),
            "width": atlas.width,
            "height": atlas.height,
            "cell_px": CANONICAL_CELL_PX,
            "placeholder_cell": list(PLACEHOLDER_CELL),
            "placeholder_transparent_in_canonical": (
                cells[PLACEHOLDER_CELL].getchannel("A").getextrema() == (0, 0)
            ),
            "published_transparent_pixels": published_holes,
        },
        "construction": {
            "appearance_owner": "image-model-cell-paintover",
            "topology_owner": "locked-packaged-template-comparison-and-lookup",
            "registration": "fixed-pitch-exact-canvas-v1",
            "connector_harmonization": "none",
        },
        "worst_join_tone_step": source_validation["worst_join_tone_step"],
        "mean_join_tone_step": source_validation["mean_join_tone_step"],
        "hillside_tile_object_share": source_validation["hillside_tile_object_share"],
        "connector_rgb_mean": connector_rgb_max,
        "thresholds": {
            "paint_canvas": PAINT_CANVAS_SIZE,
            "maximum_join_tone_step": MAXIMUM_JOIN_TONE_STEP,
        },
        "maps": map_reports,
        "smooth_slopes_supported": False,
    }
    if not direct_pass:
        raise ValueError("deterministic terrain paintover canonicalization failed connector checks")
    return canonical, report


def cells_from_canonical_atlas(data: bytes) -> dict[Coordinate, Image.Image]:
    with Image.open(BytesIO(data)) as opened:
        atlas = opened.convert("RGBA")
    expected = (GRID_COLUMNS * CANONICAL_CELL_PX, GRID_ROWS * CANONICAL_CELL_PX)
    if atlas.size != expected:
        raise ValueError(f"canonical terrain atlas must be {expected[0]}x{expected[1]}")
    return {
        (column, row): atlas.crop(
            (
                column * CANONICAL_CELL_PX,
                row * CANONICAL_CELL_PX,
                (column + 1) * CANONICAL_CELL_PX,
                (row + 1) * CANONICAL_CELL_PX,
            )
        )
        for row in range(GRID_ROWS)
        for column in range(GRID_COLUMNS)
    }


def compose_canonical_terrain(
    atlas: bytes,
    rows: Sequence[str],
) -> tuple[bytes, dict[str, object]]:
    """Compose a deterministic binary map from a direct-pass canonical atlas."""

    occupied = parse_binary_rows(rows)
    image, coordinates = compose_terrain(
        occupied,
        cells_from_canonical_atlas(atlas),
        load_terrain_atlas_lookup(),
    )
    return png_bytes(image), {
        "topology": TOPOLOGY_ID,
        "processing": "direct",
        "rows": list(rows),
        "coordinates": [
            [list(coordinate) if coordinate is not None else None for coordinate in row]
            for row in coordinates
        ],
        "connector_metrics": _connector_metrics(image, occupied),
    }


__all__ = [
    "CANONICAL_CELL_PX",
    "GRID_COLUMNS",
    "GUIDE_INSET_PX",
    "GUIDE_RGB",
    "GUIDE_WIDTH_PX",
    "GRID_ROWS",
    "MATERIAL_ASSEMBLER_ID",
    "MATERIAL_SOURCE_CONTRACT_ID",
    "MAXIMUM_JOIN_TONE_STEP",
    "PAINT_CANVAS_HEIGHT",
    "PAINT_CANVAS_SIZE",
    "PAINT_CANVAS_WIDTH",
    "PAINT_TARGET_ID",
    "PLACEHOLDER_CELL",
    "SOURCE_CELL_PX",
    "TOPOLOGY_ID",
    "TerrainAtlasLookup",
    "assemble_terrain_atlas",
    "cells_from_canonical_atlas",
    "compose_canonical_terrain",
    "compose_terrain",
    "load_terrain_atlas_lookup",
    "parse_binary_rows",
    "peering_mask",
    "require_terrain_atlas_source",
    "terrain_atlas_cells",
    "terrain_atlas_generation_prompt",
    "terrain_atlas_paint_target",
]
