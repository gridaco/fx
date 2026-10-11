"""Draw a synthetic reference for offline planning; never replace the showcase input."""

from pathlib import Path

from PIL import Image, ImageDraw
from terrain_example.atlas import CANONICAL_CELL_PX, GRID_COLUMNS, GRID_ROWS, PLACEHOLDER_CELL


def make_fixture() -> Path:
    home = Path(__file__).resolve().parent
    target = home / ".fixture" / "meadow.png"
    target.parent.mkdir(exist_ok=True)
    image = Image.new("RGB", (480, 320), "#789bbb")
    draw = ImageDraw.Draw(image)
    draw.rectangle((0, 210, 479, 319), fill="#d4aa55")
    draw.ellipse((315, 90, 435, 230), fill="#315b4a")
    draw.rectangle((368, 180, 382, 260), fill="#675440")
    image.save(target, format="PNG")
    make_cover_fixture(target.parent)
    return target


def make_cover_fixture(destination: Path) -> Path:
    """A synthetic 47-cell atlas for the local cover path; no downloaded art is used."""
    destination.mkdir(parents=True, exist_ok=True)
    atlas = Image.new("RGBA", (GRID_COLUMNS * CANONICAL_CELL_PX, GRID_ROWS * CANONICAL_CELL_PX))
    draw = ImageDraw.Draw(atlas)
    for row in range(GRID_ROWS):
        for column in range(GRID_COLUMNS):
            if (column, row) == PLACEHOLDER_CELL:
                continue
            x, y = column * CANONICAL_CELL_PX, row * CANONICAL_CELL_PX
            color = (60 + column * 12, 90 + row * 30, 130 + (row + column) * 6, 255)
            draw.rectangle((x, y, x + 119, y + 119), fill=color)
            draw.rectangle((x + 12, y + 12, x + 107, y + 28), fill="#e9deaa")
    atlas.save(destination / "atlas.png", format="PNG")
    inputs = destination / "cover-inputs.yaml"
    inputs.write_text("atlas: atlas.png\n", encoding="utf-8")
    return inputs


if __name__ == "__main__":
    print(make_fixture().relative_to(Path(__file__).resolve().parent))
