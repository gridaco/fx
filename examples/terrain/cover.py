"""Tile-built typography, composed locally with the canonical terrain lookup."""

from __future__ import annotations

from hashlib import sha256
from io import BytesIO

from PIL import Image, UnidentifiedImageError
from terrain_example.atlas import (
    CANONICAL_CELL_PX,
    GRID_COLUMNS,
    GRID_ROWS,
    compose_canonical_terrain,
)
from terrain_example.resources import terrain_atlas_lookup_path

from grida.fx import Ctx, node

CANVAS = (1920, 1200)
BACKGROUND = "#eeeae2"
TEXT = "47"
CELL_PX = 68
HORIZONTAL_PADDING = 3
GROUND_ROWS = 5
ORIGIN = (110 - HORIZONTAL_PADDING * CELL_PX, 124)
DIGIT_ROWS = 12
DIGIT_SHAPE = (
    "0011100011100011111111100",
    "0011100011100011111111100",
    "0011100011100011111111100",
    "0011100011100000000011100",
    "0011100011100000000011100",
    "0011111111100000000111000",
    "0011111111100000001110000",
    "0011111111100000011100000",
    "0000000011100000111000000",
    "0000000011100001110000000",
    "0000000011100011100000000",
    "0000000011100011100000000",
)
# The map continues past the frame on three sides. Its physical outer edges
# must remain offscreen so the visible ground reads as terrain, not a platform.
ROWS = tuple("0" * HORIZONTAL_PADDING + row + "0" * HORIZONTAL_PADDING for row in DIGIT_SHAPE)
ROWS += ("1" * len(ROWS[0]),) * GROUND_ROWS


def render_cover_layers(atlas: bytes) -> tuple[Image.Image, Image.Image, dict]:
    """Ground and digit layers share one lookup-composed map, split at its baseline."""
    try:
        with Image.open(BytesIO(atlas)) as image:
            expected = (GRID_COLUMNS * CANONICAL_CELL_PX, GRID_ROWS * CANONICAL_CELL_PX)
            if image.format != "PNG" or image.size != expected or image.n_frames != 1:
                raise ValueError("cover needs one canonical 1440x480 PNG terrain atlas")
    except (UnidentifiedImageError, OSError, SyntaxError) as error:
        raise ValueError("cover needs a decodable PNG terrain atlas") from error

    lookup_data = terrain_atlas_lookup_path().read_bytes()
    terrain_bytes, terrain_manifest = compose_canonical_terrain(atlas, ROWS)
    with Image.open(BytesIO(terrain_bytes)) as image:
        terrain = image.convert("RGBA").resize(
            (len(ROWS[0]) * CELL_PX, len(ROWS) * CELL_PX),
            Image.Resampling.LANCZOS,
        )
    baseline = DIGIT_ROWS * CELL_PX
    ground = Image.new("RGBA", CANVAS)
    ground.alpha_composite(
        terrain.crop((0, baseline, terrain.width, terrain.height)),
        (ORIGIN[0], ORIGIN[1] + baseline),
    )
    digits = Image.new("RGBA", CANVAS)
    digits.alpha_composite(terrain.crop((0, 0, terrain.width, baseline)), ORIGIN)

    base = Image.new("RGBA", CANVAS, BACKGROUND)
    base.alpha_composite(ground)

    return (
        base,
        digits,
        {
            "kind": "terrain-type-cover-v1",
            "text": TEXT,
            "source_atlas": {"sha256": sha256(atlas).hexdigest(), "bytes": len(atlas)},
            "lookup_sha256": sha256(lookup_data).hexdigest(),
            "canvas": {"width": CANVAS[0], "height": CANVAS[1], "background": BACKGROUND},
            "layout": {
                "origin": list(ORIGIN),
                "cell_px": CELL_PX,
                "size": list(terrain.size),
                **terrain_manifest,
            },
            "digits": {
                "rows": list(ROWS[:DIGIT_ROWS]),
                "baseline_y": ORIGIN[1] + baseline,
                "cells": [
                    {
                        "map_coordinate": [x, y],
                        "atlas_coordinate": coordinate,
                        "origin": [ORIGIN[0] + x * CELL_PX, ORIGIN[1] + y * CELL_PX],
                        "size": [CELL_PX, CELL_PX],
                    }
                    for y, row in enumerate(terrain_manifest["coordinates"][:DIGIT_ROWS])
                    for x, coordinate in enumerate(row)
                    if coordinate is not None
                ],
            },
            "ground": {
                "row_start": DIGIT_ROWS,
                "row_count": len(ROWS) - DIGIT_ROWS,
                "origin": [ORIGIN[0], ORIGIN[1] + baseline],
                "size": [terrain.width, terrain.height - baseline],
            },
            "encoding": {"media_type": "image/webp", "quality": 92, "method": 6},
        },
    )


def render_cover(atlas: bytes) -> tuple[bytes, dict]:
    """Encode the complete still; the same source-owned layers can also be animated."""
    base, digits, layout = render_cover_layers(atlas)
    base.alpha_composite(digits)
    output = BytesIO()
    base.convert("RGB").save(output, format="WEBP", quality=92, method=6)
    return output.getvalue(), layout


@node(
    "terrain_cover",
    inputs={"atlas": "image/png"},
    outputs={"cover": "image/webp", "layout": "json", "atlas": "image/png"},
    resources=["terrain_example/resources/lookup.json"],
)
def terrain_cover(ctx: Ctx) -> dict:
    """A source-owned cover; the original atlas is also placed for portable inspection."""
    atlas = ctx.read.bytes("atlas")
    cover, layout = render_cover(atlas)
    return {
        "cover": ctx.out.bytes(cover, "image/webp"),
        "layout": ctx.out.json(layout),
        "atlas": ctx.out.bytes(atlas, "image/png"),
    }
