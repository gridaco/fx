"""Check actual head/torso ownership and guarded body completion without providers."""

from __future__ import annotations

import copy
import hashlib
import io
import json
import os
import re
import shutil
import sys
import tempfile
import zipfile
from pathlib import Path

import numpy as np
from check_gaze import check_result as check_gaze_result
from check_gaze import fixture as gaze_fixture
from check_gaze import pixels
from grida.fx import Answer, plan, run
from PIL import Image, ImageDraw
from scipy.ndimage import gaussian_filter
from stand_in import CROP, FEATURES, INK, SIZE, SKIN, fixture_donors, geometry, png

from nodes.gaze import build_gaze_package
from nodes.registered_patches import polygon_mask, register_donor


def fixture(*, include_gaze_inputs=False):
    """An original opaque synthetic portrait whose visible shoulders do not belong to its head."""
    old, _mouth, _base_geometry, old_donors, gaze_geometry = gaze_fixture()
    original = Image.open(io.BytesIO(old)).convert("RGBA")
    old_pixels = np.asarray(original).copy()
    body = Image.new("RGBA", SIZE)
    pen = ImageDraw.Draw(body)
    pen.polygon(
        ((185, 486), (88, 525), (43, 639), (469, 639), (424, 525), (327, 486)),
        fill=(55, 122, 129, 255),
        outline=INK,
        width=5,
    )
    pen.rectangle((218, 320, 294, 515), fill=SKIN)
    master = body.copy()
    hair = Image.new("RGBA", SIZE)
    pen = ImageDraw.Draw(hair)
    pen.ellipse((66, 34, 446, 480), fill=(65, 85, 110, 255), outline=INK, width=5)
    for index in range(60):
        x = 91 + (index * 37) % 330
        y = 56 + (index * 53) % 360
        pen.arc((x - 12, y - 19, x + 12, y + 19), 160, 340, fill=(108, 132, 158, 255), width=2)
    master.alpha_composite(hair)
    ImageDraw.Draw(master).rectangle((218, 420, 294, 515), fill=SKIN)
    # Keep the exact known face and eyes, and expose the torso's short neck below the jaw.
    face = Image.new("L", SIZE)
    pen = ImageDraw.Draw(face)
    pen.ellipse((109, 108, 403, 456), fill=255)
    pen.ellipse((93, 229, 147, 314), fill=255)
    pen.ellipse((365, 229, 419, 314), fill=255)
    array = np.asarray(master).copy()
    selected = np.asarray(face) > 0
    array[selected] = old_pixels[selected]
    array[array[..., 3] > 0, 3] = 255
    master = Image.fromarray(array)
    portrait = png(master)
    base_geometry = geometry(portrait)
    gaze_geometry = copy.deepcopy(gaze_geometry)
    gaze_geometry["source_sha256"] = hashlib.sha256(portrait).hexdigest()
    crop = master.crop(CROP)
    matte = Image.new("RGBA", crop.size, (224, 224, 224, 255))
    matte.alpha_composite(crop)
    new_crop = np.asarray(matte.convert("RGB")).copy()
    old_alpha = old_pixels[CROP[1] : CROP[3], CROP[0] : CROP[2], 3:4].astype(float) / 255
    neutral = np.array((224, 224, 224))
    mutable = Image.new("L", SIZE)
    draw = ImageDraw.Draw(mutable)
    for name in ("canvas_left_eye", "canvas_right_eye"):
        draw.polygon(FEATURES[name], fill=255)
    eye_selected = np.asarray(mutable.crop(CROP)) > 0
    eye_donors = {}
    for name, donor in old_donors.items():
        raw = pixels(donor)[..., :3].astype(float)
        straight = np.clip(
            np.rint(
                np.divide(
                    raw - neutral * (1 - old_alpha),
                    old_alpha,
                    out=np.zeros_like(raw),
                    where=old_alpha > 0,
                )
            ),
            0,
            255,
        ).astype(np.uint8)
        current = new_crop.copy()
        current[eye_selected] = straight[eye_selected]
        eye_donors[name] = png(Image.fromarray(current))
    gaze_args = (portrait, fixture_donors(portrait), base_geometry, eye_donors, gaze_geometry)
    gaze = build_gaze_package(*gaze_args)
    check_gaze_result(gaze_args, gaze)
    head = [
        [0, 0],
        [511, 0],
        [511, 482],
        [294, 482],
        [294, 452],
        [274, 457],
        [256, 459],
        [238, 457],
        [218, 452],
        [218, 482],
        [0, 482],
    ]
    neck = [[218, 320], [294, 320], [294, 515], [218, 515]]
    puppet_geometry = {
        "schema_version": 1,
        "source_size": list(SIZE),
        "source_sha256": hashlib.sha256(portrait).hexdigest(),
        "head_polygon_xy": head,
        "neck_fill_polygon_xy": neck,
        "anchors": base_geometry["anchors"],
        "pose_bounds": {
            "angle_x": [-12, 12],
            "angle_y": [-8, 8],
            "angle_z": [-6, 6],
            "body_x": [-2, 2],
            "body_z": [-1, 1],
            "breath": [0, 1],
        },
    }
    args = (portrait, gaze[1], {"body_neck": png(body)}, puppet_geometry)
    return (args, gaze, gaze_args) if include_gaze_inputs else (args, gaze)


def check_scoped_registration():
    """A changed head can touch image edges even though only the neck is imported."""
    random = np.random.default_rng(12)
    rgb = gaussian_filter(random.uniform(25, 230, (160, 160, 3)), (0.7, 0.7, 0))
    image = Image.fromarray(
        np.dstack((np.rint(rgb).astype(np.uint8), np.full((160, 160), 255, np.uint8)))
    )
    shifted = np.full((160, 160, 3), 224, np.uint8)
    shifted[1:, 1:] = np.asarray(image)[:-1, :-1, :3]
    donor = Image.fromarray(shifted)
    changed = polygon_mask(image.size, [[0, 0], [159, 0], [159, 45], [0, 45]])
    neck = polygon_mask(image.size, [[50, 15], [110, 15], [110, 40], [50, 40]])
    _, report = register_donor(image, donor, changed, required_support=neck)
    assert report["coverage_scope"] == "required_support"
    assert abs(report["translation_xy_px"][0] + 1) < 0.3
    assert abs(report["translation_xy_px"][1] + 1) < 0.3
    try:
        register_donor(image, donor, changed)
    except ValueError:
        pass
    else:
        raise AssertionError(
            "whole changed-region coverage should reject this border-crossing warp"
        )
    outside = polygon_mask(image.size, [[50, 70], [110, 70], [110, 100], [50, 100]])
    for bad in (Image.new("L", image.size), outside):
        try:
            register_donor(image, donor, changed, required_support=bad)
        except ValueError:
            continue
        raise AssertionError("required support must be nonempty and inside the excluded region")


def check_package(args, gaze, result):
    portrait, _archive, _donors, geometry_doc = args
    model, archive, preview, _report, _patches, states = result
    reference = pixels(gaze[2])
    assert np.array_equal(pixels(preview), reference), (
        "opaque synthetic ownership must reconstruct the independent gaze master exactly"
    )
    assert np.array_equal(pixels(states["neutral"]), reference)
    assert model["format"] == "livliv-model" and model["version"] == 4
    assert model["canvas"] == list(SIZE)
    body_layers = [layer for layer in model["layers"] if layer["group"] == "body"]
    assert len(body_layers) == 1, "the torso must have its own independent body layer"
    head_layers = [layer for layer in model["layers"] if layer["group"] == "head"]
    by_id = {layer["id"]: layer for layer in model["layers"]}
    for name in ("eye_L", "eye_R", "iris_L", "iris_R", "lid_L", "lid_R", "mouth"):
        assert by_id[name]["group"] == "head", f"{name} must follow the head, not the torso"
    primary_head = [layer for layer in head_layers if not layer.get("role")]
    assert len(primary_head) == 1
    assert model["rig"]["yaw"] > 0 and model["rig"]["pitch"] > 0 and model["rig"]["roll"] > 0
    assert model["tuning"]["turn"] > 0 and model["tuning"]["nod"] > 0
    assert model["rig"]["body"]["roll"] > 0, "body control must not be disabled"
    with zipfile.ZipFile(io.BytesIO(archive)) as package:
        assert json.loads(package.read("model.json")) == model

        def native(layer):
            image = Image.new("RGBA", SIZE)
            with Image.open(io.BytesIO(package.read(layer["texture"]))) as texture:
                assert texture.size == (layer["w"], layer["h"])
                image.alpha_composite(texture.convert("RGBA"), (layer["x"], layer["y"]))
            return np.asarray(image)

        body = native(body_layers[0])
        head = native(primary_head[0])
        pupils = (body[..., 0] < 40) & (body[..., 1] < 90) & (body[..., 2] > 210)
        assert not (pupils & (body[..., 3] > 0)).any(), "the old face is still baked in the torso"
        assert head[600, 256, 3] == 0, "shirt pixels incorrectly belong to the movable head"
        assert body[600, 256, 3] == 255
        assert np.array_equal(body[600, 256], pixels(portrait)[600, 256])
        assert body[400, 256, 3] > 0, "the neck behind the original head was not completed"
        assert np.array_equal(body[400, 256, :3], np.asarray(SKIN[:3]))
        assert head[400, 256, 3] > 0
        assert body[253, 176, 3] == 0, "body completion contains the original upper face"
        for layer in model["layers"]:
            assert 0 <= layer["x"] < layer["x"] + layer["w"] <= SIZE[0]
            assert 0 <= layer["y"] < layer["y"] + layer["h"] <= SIZE[1]
            for path in [layer["texture"], *layer.get("frames", [])]:
                assert path in package.namelist() and ".." not in path.split("/")
    assert geometry_doc["source_sha256"] == hashlib.sha256(portrait).hexdigest()


def check_rejections(args):
    from nodes.puppet import build_puppet_package

    portrait, archive, donors, geometry_doc = args
    wrong_hash = copy.deepcopy(geometry_doc)
    wrong_hash["source_sha256"] = "0" * 64
    outside = copy.deepcopy(geometry_doc)
    outside["head_polygon_xy"][0] = [-1, 0]
    empty_neck = copy.deepcopy(geometry_doc)
    empty_neck["neck_fill_polygon_xy"] = []
    invalid_pose = copy.deepcopy(geometry_doc)
    invalid_pose["pose_bounds"]["angle_x"] = [float("nan"), 12]
    for label, candidate in (
        ("source hash mismatch", wrong_hash),
        ("outside head ownership", outside),
        ("empty neck import", empty_neck),
        ("nonfinite pose bounds", invalid_pose),
    ):
        try:
            build_puppet_package(portrait, archive, donors, candidate)
        except ValueError:
            continue
        raise AssertionError(f"puppet export accepted {label}")
    broken = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(archive)) as original, zipfile.ZipFile(broken, "w") as target:
        for name in original.namelist():
            data = original.read(name)
            if name == "model.json":
                model = json.loads(data)
                model["canvas"] = [513, 640]
                data = json.dumps(model).encode()
            target.writestr(name, data)
    try:
        build_puppet_package(portrait, broken.getvalue(), donors, geometry_doc)
    except ValueError:
        pass
    else:
        raise AssertionError("a gaze package for a different canvas was accepted")


def check_safe_neck_texture(args, gaze, baseline):
    """An explicitly trusted skin interior must exclude a donor's artificial outline."""
    from nodes.puppet import build_puppet_package

    portrait, archive, donors, geometry_doc = args
    with Image.open(io.BytesIO(donors["body_neck"])) as image:
        striped = image.convert("RGBA")
    ImageDraw.Draw(striped).rectangle((218, 320, 227, 515), fill=(255, 0, 255, 255))
    corrupt = {"body_neck": png(striped)}
    unsafe = build_puppet_package(portrait, archive, corrupt, geometry_doc)
    safe_geometry = copy.deepcopy(geometry_doc)
    safe_geometry["neck_texture_polygon_xy"] = [[232, 334], [280, 334], [280, 501], [232, 501]]
    safe = build_puppet_package(portrait, archive, corrupt, safe_geometry)

    def body_pixels(result):
        model, package_bytes = result[:2]
        entry = next(layer for layer in model["layers"] if layer["group"] == "body")
        with zipfile.ZipFile(io.BytesIO(package_bytes)) as package:
            with Image.open(io.BytesIO(package.read(entry["texture"]))) as image:
                canvas = Image.new("RGBA", SIZE)
                canvas.alpha_composite(image.convert("RGBA"), (entry["x"], entry["y"]))
                return np.asarray(canvas).copy()

    uncorrected, corrected = body_pixels(unsafe), body_pixels(safe)
    assert np.array_equal(uncorrected[500, 224, :3], (255, 0, 255)), (
        "the fixture must expose the donor stripe before the trusted texture mask is used"
    )
    assert np.array_equal(corrected[500, 224, :3], SKIN[:3]), (
        "only the authored clean skin interior should supply the neck's extrapolated RGB"
    )
    fill = np.asarray(polygon_mask(SIZE, geometry_doc["neck_fill_polygon_xy"])) > 0
    assert np.array_equal(corrected[~fill], uncorrected[~fill]), (
        "safe texture selection changed pixels outside the original neck fill"
    )
    check_package((portrait, archive, corrupt, safe_geometry), gaze, safe)
    texture_report = safe[3]["neck_texture"]
    assert texture_report["extrapolated_pixels"] > 0
    assert texture_report["resized_or_repositioned"] is False
    assert build_puppet_package(*args)[1] == baseline[1], (
        "an optional texture selection must not mutate the default build"
    )
    for polygon in ([], [[100, 100], [120, 100], [120, 120], [100, 120]]):
        invalid = copy.deepcopy(geometry_doc)
        invalid["neck_texture_polygon_xy"] = polygon
        try:
            build_puppet_package(portrait, archive, donors, invalid)
        except ValueError:
            continue
        raise AssertionError("neck texture selection must be nonempty and inside the fill")


def check_workflow(args, gaze, gaze_args):
    portrait, _archive, body_donors, puppet_geometry = args
    _portrait, mouth_donors, base_geometry, eye_donors, gaze_geometry = gaze_args
    home = Path(__file__).resolve().parent
    os.environ["GRIDA_FX_DISABLE_DOTENV"] = "1"
    os.environ["GRIDA_FX_NETWORK"] = "off"
    os.environ["GRIDA_FX_PYTHON"] = sys.executable

    def opaque(data, box=None):
        with Image.open(io.BytesIO(data)) as image:
            rgba = image.convert("RGBA")
        if box:
            rgba = rgba.crop(box)
        matte = Image.new("RGBA", rgba.size, (224, 224, 224, 255))
        matte.alpha_composite(rgba)
        return png(matte.convert("RGB"))

    expected_full = pixels(opaque(portrait))
    expected_face = pixels(opaque(portrait, CROP))
    body_answer = opaque(body_donors["body_neck"])
    with tempfile.TemporaryDirectory(prefix="fx-puppet-") as temporary:
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
                    "puppet_geometry": "puppet.json",
                }
            )
        )
        (project / "portrait.png").write_bytes(portrait)
        for name, data in (
            ("geometry.json", base_geometry),
            ("gaze.json", gaze_geometry),
            ("puppet.json", puppet_geometry),
        ):
            (project / name).write_text(json.dumps(data))
        planned = plan("workflows/puppet.yaml", cwd=project, input_files=[inputs])
        assert planned.ok, planned.problems
        assert planned.estimate() == (0, 0)
        refused = run("workflows/puppet.yaml", cwd=project, input_files=[inputs])
        assert not refused.ok and refused.cost == 0
        assert any("is a paid call; run with --live" in json.dumps(e) for e in refused.events)
        calls = []

        def answer(call):
            assert call.capability == "image.edit"
            prompt = call.request["prompt"]
            state = re.search(r"State id:\s*(\w+)", prompt)
            image = pixels(call.request["image"].read_bytes())
            if state:
                assert np.array_equal(image, expected_face)
                calls.append(state.group(1))
                data = {**mouth_donors, **eye_donors}[state.group(1)]
            else:
                assert "hidden-surface body/neck backing plate" in prompt
                assert np.array_equal(image, expected_full)
                calls.append("body_neck")
                data = body_answer
            return Answer(files={"image": data})

        result = run("workflows/puppet.yaml", cwd=project, input_files=[inputs], stand_in=answer)
        assert result.ok and result.cost == 0, result.failed
        assert len(calls) == 9 and set(calls) == {*mouth_donors, *eye_donors, "body_neck"}
        output = result.outputs
        check_package(
            args,
            gaze,
            (
                json.loads(output["model"].read_bytes()),
                output["archive"].read_bytes(),
                output["preview"].read_bytes(),
                json.loads(output["report"].read_bytes()),
                {key: file.read_bytes() for key, file in output["patches"].items()},
                {key: file.read_bytes() for key, file in output["states"].items()},
            ),
        )
        cached = run("workflows/puppet.yaml", cwd=project, input_files=[inputs], stand_in=answer)
        assert cached.ok and cached.cost == 0
        assert len(calls) == 9, "unchanged full-puppet runs should reuse all nine drawing calls"
        assert cached.outputs["archive"].read_bytes() == output["archive"].read_bytes()


def main():
    from nodes.puppet import build_puppet_package

    check_scoped_registration()
    args, gaze, gaze_args = fixture(include_gaze_inputs=True)
    result = build_puppet_package(*args)
    check_package(args, gaze, result)
    check_rejections(args)
    check_safe_neck_texture(args, gaze, result)
    check_workflow(args, gaze, gaze_args)
    print(
        "puppet: separate head/torso, completed neck, neutral truth and registration guards passed"
    )


if __name__ == "__main__":
    main()
