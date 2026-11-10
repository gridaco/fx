"""Draw a geometric master and repaint its complete face in place, without a provider.

    python stand_in.py /tmp/registered-portrait-fixture

The synthesized shapes exercise native registration and pixel preservation, not art quality.
"""

from __future__ import annotations

import hashlib
import io
import json
import re
import sys
from pathlib import Path

from grida.fx import Answer, StandInCall
from PIL import Image, ImageDraw

SIZE = (512, 640)
CROP = (0, 64, 512, 576)
SKIN = (233, 190, 159, 255)
INK = (39, 37, 53, 255)
FEATURES = {
    "canvas_left_eye": [[134, 226], [219, 226], [219, 281], [134, 281]],
    "canvas_right_eye": [[293, 226], [378, 226], [378, 281], [293, 281]],
    "mouth": [[218, 335], [294, 335], [294, 390], [218, 390]],
}
STATES = ("blink", "mouth_half", "mouth_open", "mouth_round", "mouth_wide")


def png(image: Image.Image) -> bytes:
    stream = io.BytesIO()
    image.save(stream, format="PNG")
    return stream.getvalue()


def source() -> bytes:
    """An original geometric portrait with enough unchanged texture to register."""
    image = Image.new("RGBA", SIZE, (17, 29, 43, 0))
    pen = ImageDraw.Draw(image)
    pen.ellipse((66, 34, 446, 498), fill=(65, 85, 110, 255), outline=INK, width=5)
    for index in range(90):
        x = 80 + (index * 41) % 352
        y = 52 + (index * 67) % 410
        pen.arc(
            (x - 18, y - 24, x + 18, y + 24),
            160,
            340,
            fill=(94 + index % 40, 116 + index % 35, 142 + index % 25, 255),
            width=2,
        )
    pen.polygon(
        ((185, 428), (88, 475), (43, 639), (469, 639), (424, 475), (327, 428)),
        fill=(55, 122, 129, 255),
        outline=INK,
        width=5,
    )
    pen.rectangle((218, 411, 294, 485), fill=SKIN)
    pen.ellipse((93, 229, 147, 314), fill=SKIN, outline=INK, width=4)
    pen.ellipse((365, 229, 419, 314), fill=SKIN, outline=INK, width=4)
    pen.ellipse((109, 108, 403, 456), fill=SKIN, outline=INK, width=5)
    for left in (145, 304):
        pen.ellipse((left, 238, left + 62, 269), fill=(251, 249, 240, 255), outline=INK, width=3)
        pen.ellipse((left + 20, 239, left + 42, 268), fill=(128, 99, 47, 255), outline=INK, width=2)
        pen.ellipse((left + 28, 246, left + 35, 263), fill=INK)
        pen.line(((left - 2, 218), (left + 28, 212), (left + 65, 218)), fill=INK, width=3)
    pen.line(((255, 280), (248, 315), (259, 317)), fill=(174, 123, 110, 255), width=3)
    pen.line(((237, 355), (256, 360), (275, 355)), fill=INK, width=3)
    return png(image)


def geometry(portrait: bytes) -> dict:
    return {
        "schema_version": 1,
        "source_size": list(SIZE),
        "source_sha256": hashlib.sha256(portrait).hexdigest(),
        "working_crop_xyxy": list(CROP),
        "feather_px": 3,
        "features": {name: {"polygon_xy": points} for name, points in FEATURES.items()},
        "anchors": {
            "head": {"cx": 256, "cy": 270, "rx": 180, "ry": 230},
            "face": [109, 108, 403, 456],
            "neck_y": 438,
            "top_y": 34,
            "bottom_y": 640,
            "hair_root_y": 60,
        },
        "mask_authoring": "Synthesized geometric fixture; native source pixel polygons.",
    }


def repaint(crop: Image.Image, state: str) -> Image.Image:
    """Repaint the entire supplied crop, keeping all pixels outside one feature unchanged."""
    if crop.size != (512, 512) or state not in STATES:
        raise ValueError("this stand-in serves only its synthesized 512-square fixture")
    image = crop.convert("RGB").copy()
    pen = ImageDraw.Draw(image)
    if state == "blink":
        for feature, left in (("canvas_left_eye", 145), ("canvas_right_eye", 304)):
            points = [(x, y - CROP[1]) for x, y in FEATURES[feature]]
            pen.polygon(points, fill=SKIN[:3])
            pen.arc((left, 238 - CROP[1], left + 62, 264 - CROP[1]), 0, 180, fill=INK[:3], width=3)
    else:
        pen.polygon([(x, y - CROP[1]) for x, y in FEATURES["mouth"]], fill=SKIN[:3])
        width, height = {
            "mouth_half": (26, 10),
            "mouth_open": (27, 33),
            "mouth_round": (17, 26),
            "mouth_wide": (46, 15),
        }[state]
        cx, cy = 256, 358 - CROP[1]
        pen.ellipse(
            (cx - width // 2, cy - height // 2, cx + width // 2, cy + height // 2),
            fill=(112, 49, 63),
            outline=INK[:3],
            width=2,
        )
    return image


def fixture_donors(portrait: bytes) -> dict[str, bytes]:
    with Image.open(io.BytesIO(portrait)) as image:
        crop = image.convert("RGBA").crop(CROP)
    matte = Image.new("RGBA", crop.size, (224, 224, 224, 255))
    matte.alpha_composite(crop)
    return {state: png(repaint(matte, state)) for state in STATES}


def write_inputs(directory: Path) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    portrait = source()
    (directory / "portrait.png").write_bytes(portrait)
    (directory / "geometry.json").write_text(json.dumps(geometry(portrait)) + "\n")
    inputs = directory / "inputs.json"
    inputs.write_text(json.dumps({"portrait": "portrait.png", "geometry": "geometry.json"}) + "\n")
    return inputs


def answer(call: StandInCall) -> Answer:
    if call.capability != "image.edit":
        raise AssertionError("the registered portrait makes only full-face image.edit calls")
    asked = re.search(r"State id:\s*(\w+)", str(call.request.get("prompt", "")))
    if asked is None:
        raise AssertionError("the full-face repaint prompt must name its state")
    with Image.open(io.BytesIO(call.request["image"].read_bytes())) as crop:
        image = repaint(crop, asked.group(1))
    return Answer(files={"image": png(image)})


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: python stand_in.py OUTPUT_DIRECTORY")
    print(write_inputs(Path(sys.argv[1])).resolve())
