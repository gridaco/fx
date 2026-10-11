"""Exercise the local cover workflow with synthesized input, without keys or network."""

from __future__ import annotations

import json
import os
import sys
from hashlib import sha256
from pathlib import Path

from cover import render_cover_layers
from make_fixture import make_cover_fixture
from PIL import Image
from terrain_example.atlas import load_terrain_atlas_lookup, peering_mask

from grida.fx import plan, run


def main() -> None:
    home = Path(__file__).resolve().parent
    os.environ.update(
        GRIDA_FX_DISABLE_DOTENV="1", GRIDA_FX_NETWORK="off", GRIDA_FX_PYTHON=sys.executable
    )
    for name in (
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
    ):
        os.environ.pop(name, None)
    inputs = make_cover_fixture(home / ".fixture")
    planned = plan("render-cover.yaml", cwd=home, input_files=[inputs])
    if not planned.ok:
        raise RuntimeError(planned.problems)
    result = run("render-cover.yaml", cwd=home, input_files=[inputs])
    if not result.ok or result.cost != 0:
        raise RuntimeError(f"cover fixture failed: {result.failures}; cost={result.cost}")
    with Image.open(result.outputs["cover"].path) as cover:
        assert cover.format == "WEBP" and cover.size == (1920, 1200)
    layout = json.loads(result.outputs["cover_manifest"].read_bytes())
    assert layout["kind"] == "terrain-type-cover-v1" and layout["text"] == "47"
    rows = layout["layout"]["rows"]
    coordinates = layout["layout"]["coordinates"]
    assert len(rows) == 17 and all(len(row) == 31 for row in rows)
    assert len(coordinates) == len(rows)
    for occupied, coordinate_row in zip(rows, coordinates, strict=True):
        assert len(coordinate_row) == len(occupied)
        for bit, coordinate in zip(occupied, coordinate_row, strict=True):
            assert (bit == "1") == (coordinate is not None)
            if coordinate is not None:
                assert 0 <= coordinate[0] < 12 and 0 <= coordinate[1] < 4
                assert coordinate != [10, 1]
    cells = layout["digits"]["cells"]
    assert len(cells) == 123
    assert len({tuple(cell["map_coordinate"]) for cell in cells}) == len(cells)
    for cell in cells:
        x, y = cell["origin"]
        width, height = cell["size"]
        assert 0 <= x < x + width <= 1920 and 0 <= y < y + height <= 1200
        mx, my = cell["map_coordinate"]
        assert cell["atlas_coordinate"] == coordinates[my][mx]
        assert y + height <= layout["digits"]["baseline_y"]
    ground = layout["ground"]
    assert ground["row_start"] == 12 and ground["row_count"] == 5
    assert all(set(row) == {"1"} for row in rows[ground["row_start"] :])
    gx, gy = ground["origin"]
    gw, gh = ground["size"]
    assert gx < 0 < 1920 < gx + gw and 0 < gy < 1200 < gy + gh
    cell_px = layout["layout"]["cell_px"]
    left = (0 - gx) // cell_px
    right = (1919 - gx) // cell_px
    bottom = (1199 - layout["layout"]["origin"][1]) // cell_px
    assert 0 < left < right < len(rows[0]) - 1
    assert ground["row_start"] < bottom < len(rows) - 1
    occupied = tuple(tuple(value == "1" for value in row) for row in rows)
    lookup = load_terrain_atlas_lookup()
    for x in (left, right):
        assert peering_mask(occupied, x, bottom) == (1,) * 9
        assert tuple(coordinates[bottom][x]) == lookup.by_mask[(1,) * 9]
    source = (home / ".fixture" / "atlas.png").read_bytes()
    assert result.outputs["atlas"].read_bytes() == source
    assert layout["source_atlas"] == {"sha256": sha256(source).hexdigest(), "bytes": len(source)}
    base, digits, _ = render_cover_layers(source)
    assert base.size == digits.size == (1920, 1200)
    assert base.crop((0, 0, 1920, gy - 128)).getextrema() == (
        (238, 238),
        (234, 234),
        (226, 226),
        (255, 255),
    )
    assert digits.crop((0, gy, 1920, 1200)).getbbox() is None
    # Opaque synthesized ground must reach the visible edges; physical map edges are culled.
    background = (238, 234, 226, 255)
    for box in (
        (0, 1199, 1920, 1200),
        (0, gy + cell_px, 1, 1200),
        (1919, gy + cell_px, 1920, 1200),
    ):
        strip = base.crop(box)
        assert all(color != background for _, color in strip.getcolors(strip.width * strip.height))
    print("cover: tile-built 47, frame-filling culled ground, 1920x1200 WebP, exact atlas; $0")


if __name__ == "__main__":
    main()
