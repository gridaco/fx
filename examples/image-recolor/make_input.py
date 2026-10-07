"""Draw the example's original geometric fixture with deterministic Pillow operations.

No external media or generated art is used. The generated PNG is ignored by Git.
"""

from pathlib import Path

from PIL import Image, ImageDraw


def draw_source(path: Path) -> None:
    picture = Image.new("RGBA", (480, 320), (244, 239, 224, 255))
    pen = ImageDraw.Draw(picture)
    pen.rounded_rectangle((24, 24, 456, 296), radius=24, fill=(33, 61, 92, 255))
    pen.ellipse((66, 62, 242, 238), fill=(238, 112, 64, 255))
    pen.polygon([(320, 54), (422, 228), (218, 228)], fill=(69, 186, 159, 255))
    pen.rounded_rectangle((118, 246, 362, 266), radius=10, fill=(239, 198, 73, 255))
    path.parent.mkdir(parents=True, exist_ok=True)
    picture.save(path, format="PNG")


if __name__ == "__main__":
    draw_source(Path(__file__).resolve().parent / "inputs" / "source.png")
