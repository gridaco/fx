"""Check registered whole-face repaints, exact source preservation and offline execution."""

from __future__ import annotations

import copy
import io
import json
import os
import shutil
import sys
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

import numpy as np
from grida.fx import plan, run
from PIL import Image
from stand_in import (
    SIZE,
    STATES,
    answer,
    fixture_donors,
    geometry,
    png,
    source,
    write_inputs,
)

from nodes.character import build_package
from nodes.registered_patches import apply_patch, polygon_mask, self_check


def pixels(data: bytes) -> np.ndarray:
    with Image.open(io.BytesIO(data)) as image:
        return np.asarray(image.convert("RGBA")).copy()


def check_package(portrait: bytes, geometry_doc: dict, result: tuple) -> None:
    model, archive, preview, report, patches, states = result
    original = pixels(portrait)
    assert model["format"] == "livliv-model" and model["version"] == 4
    assert model["canvas"] == list(SIZE)
    assert preview == portrait, "an original RGBA PNG should be delivered byte for byte"
    assert np.array_equal(pixels(preview), original), "neutral preview must preserve native RGBA"
    assert np.array_equal(pixels(states["neutral"]), original)
    assert report["neutral_rgba_exact"] is True
    assert set(states) == {"neutral", *STATES}
    assert set(patches) == {
        "lid_L",
        "lid_R",
        "mouth_closed",
        "mouth_half",
        "mouth_open",
        "mouth_round",
        "mouth_wide",
        "mouth_rest",
    }
    with zipfile.ZipFile(io.BytesIO(archive)) as package:
        names = package.namelist()
        assert names[0] == "model.json" and len(names) == len(set(names))
        assert all(
            not PurePosixPath(name).is_absolute() and ".." not in PurePosixPath(name).parts
            for name in names
        )
        assert json.loads(package.read("model.json")) == model
        layers = model["layers"]
        assert {layer["id"] for layer in layers} == {"portrait", "lid_L", "lid_R", "mouth"}
        for layer in layers:
            assert 0 <= layer["x"] and 0 <= layer["y"]
            assert layer["x"] + layer["w"] <= SIZE[0]
            assert layer["y"] + layer["h"] <= SIZE[1]
            for path in [layer["texture"], *layer.get("frames", [])]:
                assert path in names and path.startswith("textures/") and path.endswith(".png")
                with Image.open(io.BytesIO(package.read(path))) as image:
                    assert image.format == "PNG" and image.mode == "RGBA"
                    assert image.size == (layer["w"], layer["h"])
                    if path == "textures/mouth_rest.png":
                        assert image.getchannel("A").getextrema() == (0, 0)
                    else:
                        assert image.getchannel("A").getextrema()[1] > 0
        mouth = next(layer for layer in layers if layer["id"] == "mouth")
        assert mouth["visemes"] == ["closed", "half", "open", "round", "wide"]
        assert len(mouth["frames"]) == 5
        assert mouth["frames"][0] == "textures/mouth_rest.png"
    for name in STATES:
        features = ("canvas_left_eye", "canvas_right_eye") if name == "blink" else ("mouth",)
        support = np.zeros((SIZE[1], SIZE[0]), dtype=bool)
        for feature in features:
            mask = polygon_mask(SIZE, geometry_doc["features"][feature]["polygon_xy"])
            support |= np.asarray(mask) > 0
        composed = pixels(states[name])
        assert np.array_equal(composed[~support], original[~support]), name
        assert np.array_equal(composed[..., 3], original[..., 3]), name
        assert np.any(composed[support] != original[support]), f"the fixture has no {name} motion"
    closed = Image.open(io.BytesIO(patches["mouth_closed"])).convert("RGBA")
    with Image.open(io.BytesIO(portrait)) as master:
        closed_rest = apply_patch(
            master.convert("RGBA"), closed, tuple(report["patches"]["mouth_closed"]["offset_xy"])
        )
    assert np.array_equal(np.asarray(closed_rest), original), "mouth frame zero must be exact rest"


def check_rejections(portrait: bytes, donors: dict, geometry_doc: dict) -> None:
    wrong_hash = copy.deepcopy(geometry_doc)
    wrong_hash["source_sha256"] = "0" * 64
    outside = copy.deepcopy(geometry_doc)
    outside["features"]["mouth"]["polygon_xy"][0] = [-1, 100]
    unrelated = dict(donors)
    stream = io.BytesIO()
    Image.new("RGB", (512, 512), "black").save(stream, "PNG")
    unrelated["mouth_open"] = stream.getvalue()
    for label, candidate_donors, candidate_geometry in (
        ("source hash mismatch", donors, wrong_hash),
        ("mask outside crop", donors, outside),
        ("unrelated full-face donor", unrelated, geometry_doc),
    ):
        try:
            build_package(portrait, candidate_donors, candidate_geometry)
        except ValueError:
            continue
        raise AssertionError(f"the exporter accepted {label}")


def main() -> None:
    home = Path(__file__).resolve().parent
    os.environ["GRIDA_FX_DISABLE_DOTENV"] = "1"
    os.environ["GRIDA_FX_NETWORK"] = "off"
    os.environ["GRIDA_FX_PYTHON"] = sys.executable
    for key in (
        "GG_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
    ):
        os.environ.pop(key, None)
    self_check()
    portrait = source()
    geometry_doc = geometry(portrait)
    donors = fixture_donors(portrait)
    assembled = build_package(portrait, donors, geometry_doc)
    check_package(portrait, geometry_doc, assembled)
    assert build_package(portrait, donors, geometry_doc)[1] == assembled[1]
    check_rejections(portrait, donors, geometry_doc)
    # Almost-opaque face pixels must retain their exact native alpha too.
    partial = pixels(portrait)
    partial[partial[..., 3] > 0, 3] = 253
    partial_portrait = png(Image.fromarray(partial))
    partial_geometry = geometry(partial_portrait)
    partial_result = build_package(
        partial_portrait, fixture_donors(partial_portrait), partial_geometry
    )
    check_package(partial_portrait, partial_geometry, partial_result)
    with tempfile.TemporaryDirectory(prefix="fx-registered-portrait-") as temporary:
        project = Path(temporary) / "project"
        shutil.copytree(
            home,
            project,
            ignore=shutil.ignore_patterns(
                ".fx", "runs", "out", ".fixtures", ".venv", ".env", ".env.*", "__pycache__"
            ),
        )
        inputs = write_inputs(project / ".fixtures")
        planned = plan("workflow.yaml", cwd=project, input_files=[inputs])
        assert planned.ok, planned.problems
        assert planned.estimate() == (0, 0), planned.estimate()
        refused = run("workflow.yaml", cwd=project, input_files=[inputs])
        assert not refused.ok and refused.cost == 0
        assert any(
            "is a paid call; run with --live" in json.dumps(event) for event in refused.events
        ), "ordinary execution must explicitly refuse the Codex calls"
        calls = []

        def count_answer(call):
            calls.append(call.key)
            return answer(call)

        generated = run("workflow.yaml", cwd=project, input_files=[inputs], stand_in=count_answer)
        assert generated.ok and generated.cost == 0, generated.failed
        assert len(calls) == 5, calls

        def read_result(result):
            outputs = result.outputs
            return (
                json.loads(outputs["model"].read_bytes()),
                outputs["archive"].read_bytes(),
                outputs["preview"].read_bytes(),
                json.loads(outputs["report"].read_bytes()),
                {key: file.read_bytes() for key, file in outputs["patches"].items()},
                {key: file.read_bytes() for key, file in outputs["states"].items()},
            )

        check_package(portrait, geometry_doc, read_result(generated))
        cached = run("workflow.yaml", cwd=project, input_files=[inputs], stand_in=count_answer)
        assert cached.ok and cached.cost == 0
        assert len(calls) == 5, "an unchanged rerun should reuse all five donor calls"
        assert cached.outputs["archive"].read_bytes() == generated.outputs["archive"].read_bytes()
    print(
        "layered-character: registered full-face repaints, exact native rest, confined patches, "
        "refusals and cache passed at $0"
    )


if __name__ == "__main__":
    main()
