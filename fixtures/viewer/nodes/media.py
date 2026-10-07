"""Original, deterministic media for the viewer harness; all pictures and audio start here.

Explicit fixture-private versions distinguish the types exported from this shared module.
There are no routes, provider calls, external input files or media-generation dependencies.
"""

import io
import struct
import time
import wave

from PIL import Image, ImageDraw, ImageStat

from grida.fx import Ctx, node


@node("viewer_seed", outputs={"image": "image/png"}, version=1)
def seed(ctx: Ctx) -> dict:
    picture = Image.new("RGBA", (160, 120), (0, 0, 0, 0))
    pen = ImageDraw.Draw(picture)
    pen.rounded_rectangle((12, 12, 147, 107), radius=18, fill="#354b70")
    pen.ellipse((28, 24, 83, 79), fill="#f2a65a")
    pen.polygon([(91, 34), (134, 84), (70, 93)], fill="#74c4b4")
    pen.rectangle((34, 88, 59, 98), fill="#f4e9cd")
    return {"image": ctx.out.png(picture)}


@node(
    "viewer_tint",
    inputs={"image": "image"},
    params={"tone": str},
    outputs={"image": "image/png"},
    version=1,
)
def tint(ctx: Ctx) -> dict:
    picture = ctx.read.image("image").convert("RGBA")
    red, green, blue, alpha = picture.split()
    tone = ctx.params["tone"]
    if tone == "warm":
        channels = (red.point(lambda value: min(255, value + 45)), green, blue)
    elif tone == "cool":
        channels = (blue, green, red)
    elif tone == "green":
        channels = (blue, red, green)
    elif tone == "violet":
        channels = (green, blue, red)
    else:
        raise ctx.fail(f"unknown fixture tone: {tone}")
    return {"image": ctx.out.png(Image.merge("RGBA", (*channels, alpha)))}


@node(
    "viewer_metrics",
    inputs={"image": "image"},
    params={"enabled": bool},
    outputs={"report": "json"},
    version=1,
)
def metrics(ctx: Ctx) -> dict:
    picture = ctx.read.image("image").convert("RGBA")
    report = {
        "width": picture.width,
        "height": picture.height,
        "mean_red": round(ImageStat.Stat(picture).mean[0], 3),
        "enabled": ctx.params["enabled"],
        "disabled": False,
        "zero": 0,
        "empty_list": [],
        "label": "synthetic seed",
        "optional": None,
    }
    ctx.fact("pixel_count", picture.width * picture.height)
    ctx.fact("enabled", ctx.params["enabled"])
    return {"report": ctx.out.json(report)}


@node("viewer_caption", inputs={"image": "image"}, outputs={"text": "text"}, version=1)
def caption(ctx: Ctx) -> dict:
    picture = ctx.read.image("image")
    return {
        "text": ctx.out.text(
            f"Original geometric fixture, {picture.width} by {picture.height} pixels.\n"
        )
    }


@node("viewer_tone", inputs={"image": "image"}, outputs={"audio": "audio/wav"}, version=1)
def tone(ctx: Ctx) -> dict:
    # A 150 ms, 400 Hz square wave, computed with integer arithmetic and a quiet amplitude.
    # Reading the image makes this an ordinary data edge from the common seed.
    picture = ctx.read.image("image")
    amplitude = picture.width * 10
    samples = [amplitude if (index // 10) % 2 else -amplitude for index in range(1200)]
    data = io.BytesIO()
    with wave.open(data, "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(8000)
        audio.writeframes(struct.pack(f"<{len(samples)}h", *samples))
    return {"audio": ctx.out.bytes(data.getvalue(), "audio/wav")}


@node(
    "viewer_sheet",
    inputs={"images": "image[]"},
    outputs={"image": "image/png"},
    version=1,
)
def sheet(ctx: Ctx) -> dict:
    pictures = []
    for file in ctx.inputs["images"]:
        with Image.open(file.path) as opened:
            pictures.append(opened.convert("RGBA"))
    gap = 8
    width = sum(picture.width for picture in pictures) + gap * (len(pictures) + 1)
    height = max(picture.height for picture in pictures) + gap * 2
    canvas = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    left = gap
    for picture in pictures:
        canvas.paste(picture, (left, gap))
        left += picture.width + gap
    ctx.fact("count", len(pictures))
    return {"image": ctx.out.png(canvas)}


@node(
    "viewer_draw",
    params={"shape": str, "color": str},
    outputs={"image": "image/png"},
    version=1,
)
def draw(ctx: Ctx) -> dict:
    picture = Image.new("RGBA", (96, 96), (0, 0, 0, 0))
    pen = ImageDraw.Draw(picture)
    shape, color = ctx.params["shape"], ctx.params["color"]
    if shape == "circle":
        pen.ellipse((12, 12, 83, 83), fill=color)
    elif shape == "square":
        pen.rounded_rectangle((12, 12, 83, 83), radius=8, fill=color)
    elif shape == "diamond":
        pen.polygon([(48, 10), (86, 48), (48, 86), (10, 48)], fill=color)
    else:
        raise ctx.fail(f"unknown fixture shape: {shape}")
    return {"image": ctx.out.png(picture)}


@node("viewer_propose", outputs={"items": "json"}, version=1)
def propose(ctx: Ctx) -> dict:
    return {
        "items": ctx.out.json(
            {
                "entries": [
                    {"id": "amber", "shape": "circle", "color": "#dc983c"},
                    {"id": "fern", "shape": "diamond", "color": "#679b68"},
                    {"id": "slate", "shape": "square", "color": "#7a8eaa"},
                ]
            }
        )
    }


@node(
    "viewer_bundle",
    inputs={"seed": "image", "warm": "image", "cool": "image"},
    outputs={"images": "image[]", "collection": "image{}"},
    version=1,
)
def bundle(ctx: Ctx) -> dict:
    names = ("seed", "warm", "cool")
    return {
        "images": [ctx.inputs[name] for name in names],
        "collection": {name: ctx.inputs[name] for name in names},
    }


@node("viewer_note", params={"message": str}, outputs={"text": "text"}, version=1)
def note(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.params["message"] + "\n")}


@node(
    "viewer_fail",
    inputs={"image": "image"},
    outputs={"image": "image/png"},
    version=1,
)
def fail(ctx: Ctx) -> dict:
    raise ctx.fail("Intentional viewer fixture failure")


@node(
    "viewer_delay",
    inputs={"image": "image"},
    params={"seconds": int},
    outputs={"image": "image/png"},
    version=1,
)
def delay(ctx: Ctx) -> dict:
    seconds = ctx.params["seconds"]
    if not 0 <= seconds <= 5:
        raise ctx.fail("fixture delay must be between zero and five seconds")
    ctx.progress("Waiting in the viewer fixture", 0)
    time.sleep(seconds)
    ctx.fact("wait_seconds", seconds)
    return {"image": ctx.inputs["image"]}
