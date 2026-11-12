"""Exercise genuine iris translation over clean eye plates, without providers or generated art."""

from __future__ import annotations

import copy
import hashlib
import io
import json
import math
import os
import re
import shutil
import sys
import tempfile
import zipfile
from pathlib import Path

import numpy as np
from grida.fx import Answer, plan, run
from PIL import Image, ImageChops, ImageDraw
from stand_in import CROP, FEATURES, INK, SIZE, SKIN, fixture_donors, geometry, png, source

from nodes.gaze import build_gaze_package
from nodes.registered_patches import polygon_mask


def ellipse_points(center, radii, count=48):
    return [
        [
            round(center[0] + radii[0] * math.cos(i * math.tau / count), 4),
            round(center[1] + radii[1] * math.sin(i * math.tau / count), 4),
        ]
        for i in range(count)
    ]


def pixels(data: bytes) -> np.ndarray:
    with Image.open(io.BytesIO(data)) as image:
        return np.asarray(image.convert("RGBA")).copy()


def _donor(image: Image.Image) -> bytes:
    crop = image.crop(CROP)
    matte = Image.new("RGBA", crop.size, (224, 224, 224, 255))
    matte.alpha_composite(crop)
    return png(matte.convert("RGB"))


def fixture():
    """Make one original geometric character with explicitly separate synthetic eye surfaces."""
    with Image.open(io.BytesIO(source())) as original:
        skin = original.convert("RGBA")
    pen = ImageDraw.Draw(skin)
    for feature in ("canvas_left_eye", "canvas_right_eye"):
        pen.polygon(FEATURES[feature], fill=SKIN)
    whites = skin.copy()
    pen = ImageDraw.Draw(whites)
    eyes = {}
    full_irises = []
    for side, left, cx in (("L", 145, 176), ("R", 304, 335)):
        pen.ellipse((left, 238, left + 62, 269), fill=(251, 249, 240, 255), outline=INK, width=3)
        aperture = ellipse_points((cx, 253.5), (27, 12.5))
        iris_shape = ellipse_points((cx, 253), (10, 14))
        iris = Image.new("RGBA", SIZE)
        painter = ImageDraw.Draw(iris)
        painter.polygon(iris_shape, fill=(40, 159, 110, 255))
        painter.ellipse((cx - 3, 249, cx + 3, 257), fill=(10, 25, 240, 255))
        painter.ellipse((cx - 5, 244, cx - 2, 247), fill=(255, 255, 255, 255))
        full_irises.append(iris)
        eyes[side] = {
            "aperture_polygon_xy": aperture,
            "eye_support_polygon_xy": [
                [left - 4, 234],
                [left + 66, 234],
                [left + 66, 274],
                [left - 4, 274],
            ],
            "iris_center_xy": [cx, 253],
            "iris_radii_xy": [10, 14],
            "source_visible_polygon_xy": iris_shape,
            "completion_center_xy": [cx, 253],
            "completion_radii_xy": [10, 14],
            "range_xy": [8, 5],
            "pivot_xy": [cx, 253],
        }
    master = whites.copy()
    complete = whites.copy()
    for iris, eye in zip(full_irises, eyes.values(), strict=True):
        complete.alpha_composite(iris)
        visible = iris.copy()
        aperture = (
            polygon_mask(
                (SIZE[0] * 4, SIZE[1] * 4),
                [[x * 4, y * 4] for x, y in eye["aperture_polygon_xy"]],
            )
            .resize(SIZE, Image.Resampling.LANCZOS)
            .point(lambda a: 255 if a >= 128 else 0)
        )
        visible.putalpha(ImageChops.multiply(visible.getchannel("A"), aperture))
        master.alpha_composite(visible)
    # Model the real source's almost-opaque interior without changing its RGB.
    for image in (master, skin, whites, complete):
        alpha = image.getchannel("A").point(lambda value: 253 if value else 0)
        image.putalpha(alpha)
    portrait = png(master)
    base_geometry = geometry(portrait)
    gaze_geometry = {
        "schema_version": 1,
        "source_size": list(SIZE),
        "source_sha256": hashlib.sha256(portrait).hexdigest(),
        "eyes": eyes,
    }
    donors = {
        "eye_skin": _donor(skin),
        "eye_whites": _donor(whites),
        "eye_complete": _donor(complete),
    }
    return portrait, fixture_donors(portrait), base_geometry, donors, gaze_geometry


def pupil_points(image: np.ndarray, eye: dict) -> tuple[np.ndarray, np.ndarray]:
    """Use the fixture's unique blue pupil color as independent pixel-level motion evidence."""
    xs = [point[0] for point in eye["eye_support_polygon_xy"]]
    ys = [point[1] for point in eye["eye_support_polygon_xy"]]
    x0, y0, x1, y1 = map(int, (min(xs), min(ys), max(xs) + 1, max(ys) + 1))
    region = image[y0:y1, x0:x1]
    blue = (region[..., 0] < 40) & (region[..., 1] < 90) & (region[..., 2] > 210)
    yy, xx = np.nonzero(blue)
    assert len(xx) > 10, "a pupil disappeared or no longer belongs to the original fixture"
    return xx + x0, yy + y0


def check_result(args, result):
    portrait, _mouth, base_geometry, _donors, gaze_geometry = args
    model, archive, preview, report, _patches, states = result
    original = pixels(portrait)
    expected = original.copy()
    allowed = np.zeros(original.shape[:2], dtype=bool)
    for feature in ("canvas_left_eye", "canvas_right_eye"):
        allowed |= (
            np.asarray(polygon_mask(SIZE, base_geometry["features"][feature]["polygon_xy"])) > 0
        )
    opaque_support = np.zeros(original.shape[:2], dtype=bool)
    for eye in gaze_geometry["eyes"].values():
        opaque_support |= np.asarray(polygon_mask(SIZE, eye["eye_support_polygon_xy"])) > 0
    expected[opaque_support, 3] = 255
    neutral = pixels(preview)
    assert np.array_equal(neutral, expected), (
        "neutral must reproduce the explicitly canonical master"
    )
    assert np.array_equal(pixels(states["neutral"]), expected)
    assert np.array_equal(neutral[~allowed], original[~allowed])
    assert model["format"] == "livliv-model" and model["version"] == 4
    assert model["canvas"] == list(SIZE)
    directions = {
        "gaze_left": (-1, 0),
        "gaze_right": (1, 0),
        "gaze_up": (0, -1),
        "gaze_down": (0, 1),
    }
    with zipfile.ZipFile(io.BytesIO(archive)) as package:
        assert json.loads(package.read("model.json")) == model
        layer_by_id = {layer["id"]: layer for layer in model["layers"]}
        for side in ("L", "R"):
            iris = layer_by_id[f"iris_{side}"]
            assert iris["clip"] == f"eye_{side}"
            assert f"eye_{side}" in layer_by_id
            eye_pixels = pixels(package.read(layer_by_id[f"eye_{side}"]["texture"]))
            old_iris = (
                (eye_pixels[..., 0] < 80)
                & (eye_pixels[..., 1] > 130)
                & (eye_pixels[..., 2] > 70)
                & (eye_pixels[..., 2] < 150)
                & (eye_pixels[..., 3] > 0)
            )
            assert not old_iris.any(), "the whites still contain original green iris pixels"
        for layer in model["layers"]:
            for path in [
                layer["texture"],
                *layer.get("frames", []),
                *([layer["mask"]] if layer.get("mask") else []),
            ]:
                assert path in package.namelist()
                with Image.open(io.BytesIO(package.read(path))) as texture:
                    assert texture.size == (layer["w"], layer["h"]), path
        portrait_layer = layer_by_id["portrait"]
        cleaned = pixels(package.read(portrait_layer["texture"]))
        for eye in gaze_geometry["eyes"].values():
            xs = [p[0] for p in eye["eye_support_polygon_xy"]]
            ys = [p[1] for p in eye["eye_support_polygon_xy"]]
            region = cleaned[int(min(ys)) : int(max(ys)) + 1, int(min(xs)) : int(max(xs)) + 1]
            blue = (region[..., 0] < 40) & (region[..., 1] < 90) & (region[..., 2] > 210)
            assert not blue.any(), "the original pupil is still baked into the base layer"
    for name, (dx, dy) in directions.items():
        frame = pixels(states[name])
        assert np.array_equal(frame[~allowed], neutral[~allowed]), name
        for eye in gaze_geometry["eyes"].values():
            start_x, start_y = pupil_points(neutral, eye)
            moved_x, moved_y = pupil_points(frame, eye)
            shift = (float(moved_x.mean() - start_x.mean()), float(moved_y.mean() - start_y.mean()))
            assert abs(len(moved_x) - len(start_x)) <= 3, (
                "a second baked pupil or clipped pupil remains"
            )
            if dx:
                assert shift[0] * dx > 3 and abs(shift[1]) < 1.5, (name, shift)
            if dy:
                assert shift[1] * dy > 2 and abs(shift[0]) < 1.5, (name, shift)
            aperture = np.asarray(polygon_mask(SIZE, eye["aperture_polygon_xy"])) > 0
            assert aperture[moved_y, moved_x].all(), "the iris escaped its eye aperture"
    closed = pixels(states["blink"])
    assert not ((closed[..., 0] < 40) & (closed[..., 1] < 90) & (closed[..., 2] > 210)).any(), (
        "a closed eye still contains pupil pixels"
    )
    return report


def check_rejections(args):
    portrait, mouth, base_geometry, donors, gaze_geometry = args
    wrong_hash = copy.deepcopy(gaze_geometry)
    wrong_hash["source_sha256"] = "0" * 64
    outside = copy.deepcopy(gaze_geometry)
    outside["eyes"]["L"]["aperture_polygon_xy"][0] = [-1, 253]
    invalid_iris = copy.deepcopy(gaze_geometry)
    invalid_iris["eyes"]["R"]["iris_radii_xy"] = [0, 14]
    for label, candidate in (
        ("source hash mismatch", wrong_hash),
        ("outside aperture", outside),
        ("zero iris radius", invalid_iris),
    ):
        try:
            build_gaze_package(portrait, mouth, base_geometry, donors, candidate)
        except ValueError:
            continue
        raise AssertionError(f"gaze export accepted {label}")


def check_workflow(args):
    """Run the public workflow with all eight full-context calls answered locally."""
    portrait, mouth, base_geometry, donors, gaze_geometry = args
    home = Path(__file__).resolve().parent
    os.environ["GRIDA_FX_DISABLE_DOTENV"] = "1"
    os.environ["GRIDA_FX_NETWORK"] = "off"
    os.environ["GRIDA_FX_PYTHON"] = sys.executable
    with Image.open(io.BytesIO(portrait)) as image:
        expected_crop = pixels(_donor(image.convert("RGBA")))
    with tempfile.TemporaryDirectory(prefix="fx-gaze-") as temporary:
        project = Path(temporary) / "project"
        shutil.copytree(
            home,
            project,
            ignore=shutil.ignore_patterns(
                ".fx", "runs", "out", ".fixtures", ".venv", ".env", ".env.*", "__pycache__"
            ),
        )
        inputs = project / "fixture.json"
        inputs.write_text(
            json.dumps(
                {
                    "portrait": "portrait.png",
                    "geometry": "geometry.json",
                    "gaze_geometry": "gaze.json",
                }
            )
        )
        (project / "portrait.png").write_bytes(portrait)
        (project / "geometry.json").write_text(json.dumps(base_geometry))
        (project / "gaze.json").write_text(json.dumps(gaze_geometry))
        planned = plan("workflows/gaze.yaml", cwd=project, input_files=[inputs])
        assert planned.ok, planned.problems
        assert planned.estimate() == (0, 0)
        refused = run("workflows/gaze.yaml", cwd=project, input_files=[inputs])
        assert not refused.ok and refused.cost == 0
        assert any("is a paid call; run with --live" in json.dumps(e) for e in refused.events)
        calls = []

        def answer(call):
            assert call.capability == "image.edit"
            assert np.array_equal(pixels(call.request["image"].read_bytes()), expected_crop)
            state = re.search(r"State id:\s*(\w+)", call.request["prompt"])
            assert state is not None, "every full-face request names its intended state"
            calls.append(state.group(1))
            return Answer(files={"image": {**mouth, **donors}[state.group(1)]})

        generated = run("workflows/gaze.yaml", cwd=project, input_files=[inputs], stand_in=answer)
        assert generated.ok and generated.cost == 0, generated.failed
        assert len(calls) == 8 and set(calls) == set(mouth) | set(donors), calls
        output = generated.outputs
        check_result(
            args,
            (
                json.loads(output["model"].read_bytes()),
                output["archive"].read_bytes(),
                output["preview"].read_bytes(),
                json.loads(output["report"].read_bytes()),
                {key: file.read_bytes() for key, file in output["patches"].items()},
                {key: file.read_bytes() for key, file in output["states"].items()},
            ),
        )
        cached = run("workflows/gaze.yaml", cwd=project, input_files=[inputs], stand_in=answer)
        assert cached.ok and cached.cost == 0
        assert len(calls) == 8, "an unchanged gaze run should reuse all eight full-face donors"
        assert cached.outputs["archive"].read_bytes() == output["archive"].read_bytes()


def main():
    args = fixture()
    result = build_gaze_package(*args)
    check_result(args, result)
    check_rejections(args)
    assert build_gaze_package(*args)[1] == result[1], "gaze packaging should be deterministic"
    check_workflow(args)
    print(
        "gaze: true left/right/up/down pupils, clipping, clean base, "
        "canonical neutral and refusals passed"
    )


if __name__ == "__main__":
    main()
