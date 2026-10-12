"""Answers the pet's drawing calls offline, so the whole workflow runs for nothing and without
Codex: the base picture is one plain creature, and each strip a row of them drawn here with
Pillow, one per frame the prompt asks for, bobbing so the previews move.

    grida fx run pet --inputs inputs/pebble.yaml --stand-in stand_in.py#answer
"""

from __future__ import annotations

import io
import math
import re

from PIL import Image, ImageDraw

from grida.fx import Answer, StandInCall

WIDTH, HEIGHT = 1774, 887  # a wide strip, the shape Codex tends to draw a row of poses in
BODY, OUTLINE, EYE = (112, 138, 160, 255), (40, 44, 52, 255), (20, 20, 24, 255)
REDRAWN = (196, 128, 84, 255)  # the body of a base redrawn from a picture


def creature(
    pen: ImageDraw.ImageDraw, middle: float, bottom: float, radius: float, look: float, body=BODY
):
    top = bottom - 2 * radius
    pen.ellipse(
        (middle - radius, top, middle + radius, bottom), fill=body, outline=OUTLINE, width=6
    )
    for side in (-1, 1):
        x, y = middle + side * radius * 0.35 + look, top + radius * 0.8
        pen.ellipse((x - 10, y - 14, x + 10, y + 14), fill=EYE)


def png(picture: Image.Image) -> bytes:
    data = io.BytesIO()
    picture.save(data, "PNG")
    return data.getvalue()


def base(body: tuple[int, int, int, int]) -> bytes:
    picture = Image.new("RGBA", (1254, 1254), (0, 0, 0, 0))
    creature(ImageDraw.Draw(picture), 627, 1000, 300, 0, body)
    return png(picture)


def strip(frames: int) -> bytes:
    picture = Image.new("RGBA", (WIDTH, HEIGHT), (0, 0, 0, 0))
    pen = ImageDraw.Draw(picture)
    slot = WIDTH / frames
    radius = min(slot * 0.32, 150)
    for frame in range(frames):
        lift = 40 * math.sin(math.pi * frame / max(1, frames - 1))
        look = radius * 0.25 * math.cos(2 * math.pi * frame / frames)
        creature(pen, slot * (frame + 0.5), HEIGHT * 0.78 - lift, radius, look)
    return png(picture)


def answer(call: StandInCall) -> Answer:
    if call.capability not in ("image.generate", "image.edit"):
        raise AssertionError(f"the pet makes no {call.capability} calls")
    asked = re.search(r"(\d+) frames of one animation", call.request["prompt"])
    if asked is None:  # the base picture, from the look or from the reference
        body = REDRAWN if call.capability == "image.edit" else BODY
        return Answer(files={"image": base(body)})
    if len(call.request.get("references") or []) != 1:
        raise AssertionError("a strip is drawn from the base picture")
    return Answer(files={"image": strip(int(asked.group(1)))})
