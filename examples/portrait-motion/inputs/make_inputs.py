"""Draw an original geometric character, and answer its paid calls from its own colours.

``uv run python make_inputs.py out/portrait-motion-input`` writes there:

- ``sprite.png``, ``spec.json`` and ``face.yaml``: a full sprite on transparency, to plan
  the face path (``uv run python -m grida.fx plan portrait-motion --inputs <dir>/face.yaml``);
- ``portrait.png``, ``whole-spec.json`` and ``whole.yaml``: an opaque portrait at the
  four-card canvas, animated whole (``face_crop: false``).

``stand_in(call)`` answers every paid call of either path without a provider: the face box, the
eyes and the mouth are found by the exact colours they are drawn in, the sheet comes back as
it was sent, and every decision passes. Tests and the identity gate run the whole
chain with it; it proves the plumbing, never a picture.
"""

from __future__ import annotations

import argparse
import json
import shutil
from collections.abc import Mapping
from pathlib import Path
from typing import Any

import numpy as np
import yaml
from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
SKIN = (240, 201, 160)
IRIS = (30, 42, 90)
MOUTH = (176, 48, 58)
FEATURES = ("canvas_left_eye", "canvas_right_eye", "mouth")


def draw_head(draw: ImageDraw.ImageDraw, cx: int, cy: int, r: int) -> None:
    """A round head with a fringe, two open eyes and a resting mouth, centred on (cx, cy)."""

    draw.ellipse((cx - r, cy - r, cx + r, cy + r), fill=SKIN)
    draw.chord((cx - r - 8, cy - r - 30, cx + r + 8, cy + r // 3), 180, 360, fill=(92, 58, 40))
    for side in (-1, 1):
        ex, ey = cx + side * r * 36 // 100, cy + r // 10
        draw.ellipse((ex - r // 6, ey - r // 9, ex + r // 6, ey + r // 9), fill=(250, 250, 246))
        draw.ellipse((ex - r // 14, ey - r // 14, ex + r // 14, ey + r // 14), fill=IRIS)
        draw.ellipse((ex - r // 3, ey + r // 5, ex - r // 8, ey + r // 3), fill=(242, 170, 160))
    mx, my = cx, cy + r * 45 // 100
    draw.rounded_rectangle((mx - r // 5, my - r // 22, mx + r // 5, my + r // 22), 6, fill=MOUTH)


def sprite() -> Image.Image:
    """The full figure on transparency, its face in the upper third."""

    picture = Image.new("RGBA", (900, 1400))
    draw = ImageDraw.Draw(picture)
    draw.rounded_rectangle((250, 640, 650, 1300), radius=90, fill=(64, 120, 132))
    draw.rectangle((410, 600, 490, 680), fill=SKIN)
    draw_head(draw, 450, 400, 220)
    return picture


def portrait() -> Image.Image:
    """The same head, opaque, at the four-card canvas."""

    picture = Image.new("RGB", (1024, 1536), (214, 224, 232))
    draw = ImageDraw.Draw(picture)
    draw.rounded_rectangle((212, 1060, 812, 1700), radius=120, fill=(64, 120, 132))
    draw.rectangle((462, 960, 562, 1100), fill=SKIN)
    draw_head(draw, 512, 640, 360)
    return picture


def write_inputs(target: Path) -> Path:
    """The face path's sample: the sprite, its spec, and the inputs file that names them."""

    target.mkdir(parents=True, exist_ok=True)
    sprite().save(target / "sprite.png")
    shutil.copyfile(HERE / "face-four-card.json", target / "spec.json")
    face = target / "face.yaml"
    face.write_text(
        yaml.safe_dump({"portrait": "sprite.png", "spec": "spec.json", "face_crop": True}),
        encoding="utf-8",
    )
    return face


def write_whole(target: Path) -> Path:
    """The whole-portrait sample: an opaque portrait at the four-card canvas, no crop."""

    target.mkdir(parents=True, exist_ok=True)
    portrait().save(target / "portrait.png")
    shutil.copyfile(HERE / "four-card.json", target / "whole-spec.json")
    whole = target / "whole.yaml"
    whole.write_text(
        yaml.safe_dump({"portrait": "portrait.png", "spec": "whole-spec.json", "face_crop": False}),
        encoding="utf-8",
    )
    return whole


# ------------------------------------------------------------------------- stand-in answers


def _pixels(file: Any) -> np.ndarray:
    with Image.open(file.path) as opened:
        return np.asarray(opened.convert("RGBA"))


def _text(value: Any) -> str:
    """A request's text, sent as it was written or as the file that holds it."""

    return value if isinstance(value, str) else str(value.read_bytes().decode("utf-8"))


def _schema(value: Any) -> Mapping[str, Any]:
    """A request's schema, sent as the object or as the file that holds it."""

    if isinstance(value, Mapping):
        return value
    schema: Mapping[str, Any] = json.loads(value.read_bytes())
    return schema


def _matching(pixels: np.ndarray, colour: tuple[int, int, int]) -> np.ndarray:
    distance = np.abs(pixels[..., :3].astype(int) - np.array(colour)).sum(axis=2)
    matched: np.ndarray = (distance < 40) & (pixels[..., 3] > 0)
    return matched


def _box(mask: np.ndarray) -> tuple[int, int, int, int]:
    ys, xs = np.nonzero(mask)
    if not len(xs):
        raise ValueError("the drawn colour is not in the picture")
    return int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1


def _rectangle(
    box: tuple[int, int, int, int], margin: int, size: tuple[int, int]
) -> list[list[int]]:
    left, top, right, bottom = box
    width, height = size
    left, top = max(0, left - margin), max(0, top - margin)
    right, bottom = min(width - 1, right + margin), min(height - 1, bottom + margin)
    return [[left, top], [right, top], [right, bottom], [left, bottom]]


def locate(pixels: np.ndarray) -> dict[str, Any]:
    left, top, right, bottom = _box(_matching(pixels, SKIN))
    height, width = pixels.shape[:2]
    return {
        "status": "located",
        "bbox_xyxy": [
            left * 1000 // width,
            top * 1000 // height,
            -(-right * 1000 // width),
            -(-bottom * 1000 // height),
        ],
        "reason": "One round face, upper third.",
    }


def admission() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "subject_count": 1,
        "subject_unambiguous": True,
        "image_readable": True,
        "features": [
            {
                "feature_id": feature,
                "route": "direct",
                "source_state": "rest" if feature == "mouth" else "open",
                "reason": "clear_feature",
                "evidence": "Drawn plainly, nothing covers it.",
                "confidence": "high",
            }
            for feature in FEATURES
        ],
    }


def geometry(panel: np.ndarray) -> dict[str, Any]:
    """Each iris widened to its eye, and the mouth, as rectangles on the reduced copy."""

    height, width = panel.shape[:2]
    irises = _matching(panel, IRIS)
    middle = width // 2
    halves = {"canvas_left_eye": irises.copy(), "canvas_right_eye": irises.copy()}
    halves["canvas_left_eye"][:, middle:] = False
    halves["canvas_right_eye"][:, :middle] = False
    features = []
    for feature, mask in halves.items():
        left, top, right, bottom = _box(mask)
        reach = right - left
        features.append(
            {
                "feature_id": feature,
                "points": _rectangle(
                    (left - reach, top - reach // 2, right + reach, bottom + reach // 2),
                    2,
                    (width, height),
                ),
            }
        )
    mouth = _box(_matching(panel, MOUTH))
    features.append({"feature_id": "mouth", "points": _rectangle(mouth, 6, (width, height))})
    return {
        "schema_version": 1,
        "status": "pass",
        "coordinate_space": "registered_panel_pixels",
        "canvas_size": [width, height],
        "features": features,
        "reason": "Each feature outlined with a margin.",
    }


def quality(eligible: list[str]) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "status": "pass",
        "features": [
            {"feature_id": feature, "status": "pass", "reason": "Unchanged and readable."}
            for feature in eligible
        ],
        "reason": "Every combination holds.",
        "temporal_review": "not_performed",
    }


#: The answer each schema asks for, told by a property only that schema has (the location
#: schema has no title).
_SCHEMAS = (
    ("bbox_xyxy", "location"),
    ("subject_count", "admission"),
    ("coordinate_space", "geometry"),
    ("temporal_review", "quality"),
)


def schema_name(schema: Mapping[str, Any]) -> str:
    """Which of the workflow's four schemas a structured call answers to."""

    properties = schema.get("properties") or {}
    for marker, name in _SCHEMAS:
        if marker in properties:
            return name
    raise AssertionError(f"no stand-in answer for a schema with {sorted(properties)}")


def stand_in(call: Any) -> Any:
    """One paid call of the workflow, answered from the pictures it is shown: the stand-in for
    ``grida.fx.run(..., stand_in=stand_in)``."""

    from grida.fx import DECLINE, Answer

    request = call.request
    if call.capability == "structured.generate":
        schema = schema_name(_schema(request["schema"]))
        pictures = [_pixels(file) for file in request.get("context") or []]
        if schema == "location":
            return Answer.json(locate(pictures[0]))
        if schema == "admission":
            return Answer.json(admission())
        if schema == "geometry":
            return Answer.json(geometry(pictures[0]))
        prompt = _text(request["prompt"])
        return Answer.json(quality([f for f in FEATURES if f in prompt]))
    if call.capability == "image.edit":
        return Answer(files={"image": request["image"]})
    return DECLINE


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("target", type=Path)
    target = parser.parse_args().target
    write_inputs(target)
    write_whole(target)
    print(json.dumps({"face": str(target / "face.yaml"), "whole": str(target / "whole.yaml")}))


if __name__ == "__main__":
    main()
