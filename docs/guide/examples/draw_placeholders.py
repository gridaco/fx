"""Draw the example projects' placeholder pictures: flat shapes, drawn by code.

The guide's examples read three pictures: a poster for the concept gallery, and two background
layers for the looping parallax (the game-build example keeps its own copy of the layers). They
stand in for art a user would bring. Run from the repository root:

    cd python && uv run --with pillow python ../docs/guide/examples/draw_placeholders.py

The drawing is deterministic: no randomness, no anti-aliasing and no metadata, so every run draws
the same pixels.
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw

EXAMPLES = Path(__file__).resolve().parent

CLEAR = (0, 0, 0, 0)


def poster() -> Image.Image:
    """768x1024, opaque: a lighthouse tower with a lit lamp, standing in the sea at night."""
    image = Image.new("RGB", (768, 1024), (24, 40, 64))  # night sky
    pen = ImageDraw.Draw(image)
    pen.rectangle((0, 880, 767, 1023), fill=(32, 72, 96))  # the sea
    pen.rectangle((340, 300, 428, 879), fill=(220, 214, 196))  # the tower
    pen.polygon([(330, 300), (384, 220), (438, 300)], fill=(170, 60, 48))  # its roof
    pen.ellipse((360, 330, 408, 378), fill=(250, 226, 140))  # the lamp
    return image


def far_cliffs() -> Image.Image:
    """640x360 with a clear sky: one flat ridge of cliffs. Its two edges do not meet."""
    image = Image.new("RGBA", (640, 360), CLEAR)
    pen = ImageDraw.Draw(image)
    ridge = [(0, 260), (120, 170), (260, 230), (400, 150), (520, 210), (639, 180)]
    pen.polygon([*ridge, (639, 359), (0, 359)], fill=(92, 104, 128, 255))
    return image


def near_masts() -> Image.Image:
    """640x360 with a clear sky: four masts with sails. The last sail runs off the right edge."""
    image = Image.new("RGBA", (640, 360), CLEAR)
    pen = ImageDraw.Draw(image)
    for x in (60, 250, 430, 590):
        pen.polygon([(x + 6, 110), (x + 70, 250), (x + 6, 250)], fill=(220, 210, 190, 255))  # sail
        pen.rectangle((x, 90, x + 6, 330), fill=(48, 40, 36, 255))  # mast
    return image


TARGETS = {
    "concept-gallery/inputs/tidebell/poster.png": poster,
    "looping-parallax/art/far_cliffs.png": far_cliffs,
    "looping-parallax/art/near_masts.png": near_masts,
    "game-build/kitewharf/art/far_cliffs.png": far_cliffs,
    "game-build/kitewharf/art/near_masts.png": near_masts,
}


def main() -> None:
    for relative, draw in TARGETS.items():
        path = EXAMPLES / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        draw().save(path, format="PNG")
        print(f"wrote {relative}")


if __name__ == "__main__":
    main()
