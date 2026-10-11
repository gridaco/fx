"""Labeled review atlas: pose rows by view columns, cut at native pixels.

Cells are cropped from the worker's renders without resampling, so a cell at the
bar height shows exactly the pixels a gameplay camera would. Every cell carries
its own label and the manifest binds each cell to its source image hash.
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from PIL import Image, ImageChops, ImageDraw, ImageFont

JsonObject = dict[str, Any]
LABEL_HEIGHT = 16
CELL_PADDING = 8
GUTTER = 2
BACKGROUND_TOLERANCE = 12


@dataclass(frozen=True)
class AtlasCell:
    row_label: str
    column_label: str
    path: Path


def _content_box(image: Image.Image) -> tuple[int, int, int, int]:
    """Bounding box of pixels that differ from the corner background color."""
    rgb = image.convert("RGB")
    background = Image.new("RGB", rgb.size, rgb.getpixel((0, 0)))
    difference = ImageChops.difference(rgb, background).convert("L")
    mask = difference.point(lambda value: 255 if value > BACKGROUND_TOLERANCE else 0)
    box = mask.getbbox()
    return box if box is not None else (0, 0, image.width, image.height)


def build_atlas(cells: list[AtlasCell]) -> tuple[Image.Image, JsonObject]:
    if not cells:
        raise ValueError("An atlas needs at least one cell")
    rows = list(dict.fromkeys(cell.row_label for cell in cells))
    columns = list(dict.fromkeys(cell.column_label for cell in cells))
    if len(cells) != len(rows) * len(columns):
        raise ValueError("Atlas cells must form a complete grid")
    sources = []
    side = 0
    for cell in cells:
        image = Image.open(cell.path).convert("RGB")
        box = _content_box(image)
        side = max(side, box[2] - box[0], box[3] - box[1])
        sources.append((cell, image, box))
    side = side + 2 * CELL_PADDING
    side = min(
        side,
        max((image.width for _, image, _ in sources)),
        max((image.height for _, image, _ in sources)),
    )
    font = ImageFont.load_default(size=11)
    slot = side + GUTTER
    left_band = 8 + max(int(font.getlength(label)) for label in rows)
    width = left_band + len(columns) * slot
    height = LABEL_HEIGHT + len(rows) * (side + LABEL_HEIGHT)
    atlas = Image.new("RGB", (width, height), (236, 236, 236))
    draw = ImageDraw.Draw(atlas)
    for index, label in enumerate(columns):
        draw.text((left_band + index * slot + 4, 2), label, fill=(20, 20, 20), font=font)
    manifest_cells = []
    for cell, image, box in sources:
        row, column = (rows.index(cell.row_label), columns.index(cell.column_label))
        center_x, center_y = ((box[0] + box[2]) // 2, (box[1] + box[3]) // 2)
        x0 = min(max(center_x - side // 2, 0), image.width - side)
        y0 = min(max(center_y - side // 2, 0), image.height - side)
        crop = image.crop((x0, y0, x0 + side, y0 + side))
        left = left_band + column * slot
        top = LABEL_HEIGHT + row * (side + LABEL_HEIGHT)
        label = f"{cell.row_label} / {cell.column_label}"
        if font.getlength(label) > side - 8:
            label = cell.column_label
        draw.text((left + 4, top + 2), label, fill=(20, 20, 20), font=font)
        atlas.paste(crop, (left, top + LABEL_HEIGHT))
        manifest_cells.append(
            {
                "row": cell.row_label,
                "column": cell.column_label,
                "source_sha256": hashlib.sha256(cell.path.read_bytes()).hexdigest(),
                "source_crop": [x0, y0, x0 + side, y0 + side],
                "atlas_box": [left, top + LABEL_HEIGHT, left + side, top + LABEL_HEIGHT + side],
            }
        )
    for index, label in enumerate(rows):
        draw.text(
            (4, LABEL_HEIGHT + index * (side + LABEL_HEIGHT) + LABEL_HEIGHT + 2),
            label,
            fill=(20, 20, 20),
            font=font,
        )
    manifest = {
        "schema_version": 1,
        "rows": rows,
        "columns": columns,
        "cell_side_pixels": side,
        "width": width,
        "height": height,
        "resampled": False,
        "cells": manifest_cells,
    }
    return (atlas, manifest)


def build_face_strip(
    cells: list[AtlasCell], *, head_fraction: float = 0.42
) -> tuple[Image.Image, JsonObject]:
    """One row of head crops cut from inspection-height renders, native pixels.

    The head is taken as the top part of each render's content box; SD heads are
    roughly the top two fifths of the figure. Cells stay square and are labeled.
    """
    if not cells:
        raise ValueError("A face strip needs at least one cell")
    sources = []
    side = 0
    for cell in cells:
        image = Image.open(cell.path).convert("RGB")
        box = _content_box(image)
        head = int((box[3] - box[1]) * head_fraction)
        side = max(side, head, box[2] - box[0])
        sources.append((cell, image, box, head))
    side = min(side + 2 * CELL_PADDING, max((i.width for _, i, _, _ in sources)))
    font = ImageFont.load_default(size=11)
    slot = side + GUTTER
    strip = Image.new("RGB", (len(sources) * slot, LABEL_HEIGHT + side), (236, 236, 236))
    draw = ImageDraw.Draw(strip)
    manifest_cells = []
    for index, (cell, image, box, head) in enumerate(sources):
        center_x = (box[0] + box[2]) // 2
        x0 = min(max(center_x - side // 2, 0), max(image.width - side, 0))
        y0 = min(max(box[1] + head // 2 - side // 2, 0), max(image.height - side, 0))
        crop = image.crop((x0, y0, x0 + side, y0 + side))
        left = index * slot
        draw.text(
            (left + 4, 2), f"{cell.row_label} / {cell.column_label}", fill=(20, 20, 20), font=font
        )
        strip.paste(crop, (left, LABEL_HEIGHT))
        manifest_cells.append(
            {
                "row": cell.row_label,
                "column": cell.column_label,
                "source_sha256": hashlib.sha256(cell.path.read_bytes()).hexdigest(),
                "source_crop": [x0, y0, x0 + side, y0 + side],
                "atlas_box": [left, LABEL_HEIGHT, left + side, LABEL_HEIGHT + side],
            }
        )
    manifest = {
        "schema_version": 1,
        "kind": "face_strip",
        "columns": [cell.column_label for cell in cells],
        "cell_side_pixels": side,
        "width": strip.width,
        "height": strip.height,
        "resampled": False,
        "cells": manifest_cells,
    }
    return (strip, manifest)


def build_torso_strip(
    cells: list[AtlasCell],
    *,
    band: tuple[float, float] = (0.38, 0.80),
    width_fraction: float = 0.6,
) -> tuple[Image.Image, JsonObject]:
    """Chest to mid-thigh of each pose and view, cut at native pixels, every cell labeled.

    The band is a fraction of each render's figure height from the top of its content
    box (an SD figure's chest to mid-thigh); its width is a fraction of that height,
    centred on the figure, so raised arms do not widen the crop. Rows are poses,
    columns are views, as in the atlas; a reviewer gets one row per image, so the
    image stays small enough to reach the model at its native pixels.
    """
    if not cells:
        raise ValueError("A torso strip needs at least one cell")
    rows = list(dict.fromkeys(cell.row_label for cell in cells))
    columns = list(dict.fromkeys(cell.column_label for cell in cells))
    if len(cells) != len(rows) * len(columns):
        raise ValueError("Torso strip cells must form a complete grid")
    sources = []
    cell_width = cell_height = 0
    for cell in cells:
        image = Image.open(cell.path).convert("RGB")
        box = _content_box(image)
        figure = box[3] - box[1]
        cell_width = max(cell_width, int(figure * width_fraction))
        cell_height = max(cell_height, int(figure * (band[1] - band[0])))
        sources.append((cell, image, box, figure))
    cell_width = min(cell_width + 2 * CELL_PADDING, max(i.width for _, i, _, _ in sources))
    cell_height = min(cell_height, max(i.height for _, i, _, _ in sources))
    font = ImageFont.load_default(size=11)
    slot = cell_width + GUTTER
    width = len(columns) * slot
    height = len(rows) * (cell_height + LABEL_HEIGHT)
    strip = Image.new("RGB", (width, height), (236, 236, 236))
    draw = ImageDraw.Draw(strip)
    manifest_cells = []
    for cell, image, box, figure in sources:
        row, column = (rows.index(cell.row_label), columns.index(cell.column_label))
        center_x = (box[0] + box[2]) // 2
        x0 = min(max(center_x - cell_width // 2, 0), max(image.width - cell_width, 0))
        y0 = min(max(box[1] + int(figure * band[0]), 0), max(image.height - cell_height, 0))
        crop = image.crop((x0, y0, x0 + cell_width, y0 + cell_height))
        left = column * slot
        top = row * (cell_height + LABEL_HEIGHT)
        draw.text(
            (left + 4, top + 2),
            f"{cell.row_label} / {cell.column_label}",
            fill=(20, 20, 20),
            font=font,
        )
        strip.paste(crop, (left, top + LABEL_HEIGHT))
        manifest_cells.append(
            {
                "row": cell.row_label,
                "column": cell.column_label,
                "source_sha256": hashlib.sha256(cell.path.read_bytes()).hexdigest(),
                "source_crop": [x0, y0, x0 + cell_width, y0 + cell_height],
                "atlas_box": [
                    left,
                    top + LABEL_HEIGHT,
                    left + cell_width,
                    top + LABEL_HEIGHT + cell_height,
                ],
            }
        )
    manifest = {
        "schema_version": 1,
        "kind": "torso_strip",
        "rows": rows,
        "columns": columns,
        "band": list(band),
        "cell_pixels": [cell_width, cell_height],
        "width": width,
        "height": height,
        "resampled": False,
        "cells": manifest_cells,
    }
    return (strip, manifest)
