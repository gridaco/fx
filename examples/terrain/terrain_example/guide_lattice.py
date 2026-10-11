"""Recipe-neutral colored-guide detection, extraction, chroma keying, and seam repair."""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass
from typing import Final, Protocol, cast

from PIL import Image

from terrain_example.codec import encode_png


@dataclass(frozen=True, slots=True)
class GuideColorContract:
    red_max: int
    green_min: int
    blue_min: int
    vertical_coverage_min: float
    horizontal_coverage_min: float


@dataclass(frozen=True, slots=True)
class GuideLattice:
    x_lines: tuple[tuple[int, int], ...]
    y_lines: tuple[tuple[int, int], ...]
    x_spacing_px: float
    y_spacing_px: float
    x_maximum_residual_px: float
    y_maximum_residual_px: float


CYAN_GUIDES: Final = GuideColorContract(
    red_max=80,
    green_min=170,
    blue_min=170,
    vertical_coverage_min=0.10,
    horizontal_coverage_min=0.12,
)


class _PixelAccess(Protocol):
    def __getitem__(self, xy: tuple[int, int]) -> tuple[int, ...]: ...

    def __setitem__(self, xy: tuple[int, int], color: tuple[int, ...]) -> None: ...


def _clusters(values: list[int]) -> tuple[tuple[int, int], ...]:
    if not values:
        return ()
    result: list[tuple[int, int]] = []
    start = previous = values[0]
    for value in values[1:]:
        if value > previous + 1:
            result.append((start, previous))
            start = value
        previous = value
    result.append((start, previous))
    return tuple(result)


def _fit_lattice(lines: tuple[tuple[int, int], ...]) -> tuple[float, float]:
    centers = [(start + end) / 2.0 for start, end in lines]
    count = len(centers)
    if count < 2:
        raise ValueError("a guide lattice requires at least two lines")
    mean_index = (count - 1) / 2.0
    mean_center = sum(centers) / count
    denominator = sum((index - mean_index) ** 2 for index in range(count))
    spacing = (
        sum((index - mean_index) * (center - mean_center) for index, center in enumerate(centers))
        / denominator
    )
    origin = mean_center - spacing * mean_index
    maximum_residual = max(
        abs(center - (origin + spacing * index)) for index, center in enumerate(centers)
    )
    return spacing, maximum_residual


def detect_guide_lattice(
    image: Image.Image,
    *,
    expected_columns: int,
    expected_rows: int,
    color: GuideColorContract = CYAN_GUIDES,
) -> GuideLattice:
    """Detect a regular colored lattice without relying on absolute canvas coordinates."""

    rgb = image.convert("RGB")
    pixels = cast(_PixelAccess, rgb.load())
    x_hits: list[int] = []
    for x in range(rgb.width):
        count = 0
        for y in range(rgb.height):
            red, green, blue = pixels[x, y]
            if red < color.red_max and green > color.green_min and blue > color.blue_min:
                count += 1
        if count > rgb.height * color.vertical_coverage_min:
            x_hits.append(x)
    y_hits: list[int] = []
    for y in range(rgb.height):
        count = 0
        for x in range(rgb.width):
            red, green, blue = pixels[x, y]
            if red < color.red_max and green > color.green_min and blue > color.blue_min:
                count += 1
        if count > rgb.width * color.horizontal_coverage_min:
            y_hits.append(y)
    x_lines = _clusters(x_hits)
    y_lines = _clusters(y_hits)
    if len(x_lines) != expected_columns + 1 or len(y_lines) != expected_rows + 1:
        raise ValueError(
            "guide lattice count mismatch: expected "
            f"{expected_columns + 1} vertical and {expected_rows + 1} horizontal, "
            f"got {len(x_lines)} and {len(y_lines)}"
        )
    x_spacing, x_residual = _fit_lattice(x_lines)
    y_spacing, y_residual = _fit_lattice(y_lines)
    return GuideLattice(
        x_lines=x_lines,
        y_lines=y_lines,
        x_spacing_px=x_spacing,
        y_spacing_px=y_spacing,
        x_maximum_residual_px=x_residual,
        y_maximum_residual_px=y_residual,
    )


def magenta_chroma_alpha(image: Image.Image) -> Image.Image:
    """Return the locked soft alpha estimate for a magenta-backed image."""

    rgb = image.convert("RGB")
    result = Image.new("L", rgb.size)
    output: list[int] = []
    values = cast(Iterable[tuple[int, int, int]], rgb.get_flattened_data())
    for red, green, blue in values:
        magenta_strength = min(red, blue) - green
        keyed = (
            red > 105
            and blue > 105
            and green < 175
            and magenta_strength > 35
            and abs(red - blue) < 170
        )
        hard_alpha = 0 if keyed else 255
        soft_alpha = round(max(0.0, min(1.0, (60.0 - magenta_strength) / 28.0)) * 255)
        output.append(max(hard_alpha, soft_alpha))
    result.putdata(output)
    return result


def extract_guided_cells(
    image: Image.Image,
    *,
    columns: int,
    rows: int,
    canonical_cell_px: int,
    color: GuideColorContract = CYAN_GUIDES,
) -> tuple[dict[tuple[int, int], Image.Image], GuideLattice]:
    """Slice cells from detected guides and normalize each crop to square RGBA."""

    lattice = detect_guide_lattice(
        image,
        expected_columns=columns,
        expected_rows=rows,
        color=color,
    )
    cells: dict[tuple[int, int], Image.Image] = {}
    for row in range(rows):
        for column in range(columns):
            left = lattice.x_lines[column][1] + 3
            right = lattice.x_lines[column + 1][0] - 2
            top = lattice.y_lines[row][1] + 3
            bottom = lattice.y_lines[row + 1][0] - 2
            if right <= left or bottom <= top:
                raise ValueError(f"collapsed guided crop at cell {(column, row)}")
            crop = image.crop((left, top, right, bottom)).resize(
                (canonical_cell_px, canonical_cell_px),
                Image.Resampling.LANCZOS,
            )
            rgba = crop.convert("RGBA")
            rgba.putalpha(magenta_chroma_alpha(crop))
            cells[(column, row)] = rgba
    return cells, lattice


def repair_internal_cell_seams(
    image: Image.Image,
    occupied: tuple[tuple[bool, ...], ...],
    *,
    cell_px: int,
    half_width: int = 2,
) -> Image.Image:
    """Blend only shared occupied-cell seams, leaving exposed boundaries byte-identical."""

    if half_width < 1:
        raise ValueError("seam half-width must be positive")
    if not occupied or not occupied[0] or any(len(row) != len(occupied[0]) for row in occupied):
        raise ValueError("occupancy must be a nonempty rectangular matrix")
    rgba = image.convert("RGBA")
    height, width = len(occupied), len(occupied[0])
    if rgba.size != (width * cell_px, height * cell_px):
        raise ValueError("seam-repair image dimensions do not match occupancy")
    pixels = cast(_PixelAccess, rgba.load())
    source = cast(_PixelAccess, rgba.copy().load())
    protection_width = half_width * 2

    def protected(px: int, py: int, x: int, y: int) -> bool:
        x0, x1 = x * cell_px, (x + 1) * cell_px
        y0, y1 = y * cell_px, (y + 1) * cell_px
        return (
            ((x == 0 or not occupied[y][x - 1]) and px < x0 + protection_width)
            or ((x + 1 == width or not occupied[y][x + 1]) and px >= x1 - protection_width)
            or ((y == 0 or not occupied[y - 1][x]) and py < y0 + protection_width)
            or ((y + 1 == height or not occupied[y + 1][x]) and py >= y1 - protection_width)
        )

    for y in range(height):
        for x in range(width - 1):
            if not (occupied[y][x] and occupied[y][x + 1]):
                continue
            boundary = (x + 1) * cell_px
            for local_y in range(cell_px):
                py = y * cell_px + local_y
                left = source[boundary - half_width - 1, py]
                right = source[boundary + half_width, py]
                if left[3] <= 128 or right[3] <= 128:
                    continue
                for offset, px in enumerate(range(boundary - half_width, boundary + half_width)):
                    owner_x = x if px < boundary else x + 1
                    if protected(px, py, owner_x, y):
                        continue
                    mix = (offset + 1) / (half_width * 2 + 1)
                    pixels[px, py] = tuple(
                        round(left[channel] * (1.0 - mix) + right[channel] * mix)
                        for channel in range(4)
                    )
    source = cast(_PixelAccess, rgba.copy().load())
    for y in range(height - 1):
        for x in range(width):
            if not (occupied[y][x] and occupied[y + 1][x]):
                continue
            boundary = (y + 1) * cell_px
            for local_x in range(cell_px):
                px = x * cell_px + local_x
                top = source[px, boundary - half_width - 1]
                bottom = source[px, boundary + half_width]
                if top[3] <= 128 or bottom[3] <= 128:
                    continue
                for offset, py in enumerate(range(boundary - half_width, boundary + half_width)):
                    owner_y = y if py < boundary else y + 1
                    if protected(px, py, x, owner_y):
                        continue
                    mix = (offset + 1) / (half_width * 2 + 1)
                    pixels[px, py] = tuple(
                        round(top[channel] * (1.0 - mix) + bottom[channel] * mix)
                        for channel in range(4)
                    )
    return rgba


def png_bytes(image: Image.Image) -> bytes:
    return encode_png(image)


__all__ = [
    "CYAN_GUIDES",
    "GuideColorContract",
    "GuideLattice",
    "detect_guide_lattice",
    "extract_guided_cells",
    "magenta_chroma_alpha",
    "png_bytes",
    "repair_internal_cell_seams",
]
