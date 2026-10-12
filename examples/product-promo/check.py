"""Provider-free contract, refusal, rendering and cache checks for product-promo."""

from __future__ import annotations

import copy
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch

from PIL import Image

from grida.fx import plan, run

from nodes.composition import camera, validate_brief, validate_edit
from nodes.promo import sample_frames


def refused(function, value, label):
    try:
        function(value)
    except ValueError:
        return
    raise AssertionError(f"accepted invalid {label}")


def main():
    source = Path(__file__).resolve().parent
    temporary = tempfile.TemporaryDirectory(prefix="fx-product-promo-check-")
    home = Path(temporary.name) / "project"
    shutil.copytree(
        source,
        home,
        ignore=shutil.ignore_patterns(
            ".fx",
            ".fixture",
            ".preview",
            ".showcase-inputs",
            ".venv",
            "__pycache__",
            "runs",
            "dist",
            ".env",
        ),
    )
    os.environ.update(
        GRIDA_FX_DISABLE_DOTENV="1", GRIDA_FX_NETWORK="off", GRIDA_FX_PYTHON=sys.executable
    )
    for name in (
        "GG_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
    ):
        os.environ.pop(name, None)
    # Failed tools must not serialize their command or executable/source locations.
    command = [str(home / "hidden-tool" / "ffmpeg"), str(home / "hidden-source.mp4")]
    for failure in (
        subprocess.CalledProcessError(1, command),
        subprocess.TimeoutExpired(command, 30),
        FileNotFoundError(2, "missing executable", command[0]),
    ):
        with patch("nodes.promo.subprocess.run", side_effect=failure):
            try:
                sample_frames(Path(command[0]), Path(command[1]), 12)
            except ValueError as error:
                assert str(error) == "FFmpeg could not sample the supplied video"
                assert error.__suppress_context__
            else:
                raise AssertionError("frame sampling must reject a failed tool")
    subprocess.run([sys.executable, "make_fixture.py"], cwd=home, check=True)
    planned = plan("workflow.yaml", cwd=home, input_files=[".fixture/garden.json"])
    assert planned.ok, planned.problems
    offline = run("workflow.yaml", cwd=home, input_files=[".fixture/garden.json"])
    assert not offline.ok and offline.cost == 0, "the proposal must refuse offline at $0"
    assert any("is a paid call; run with --live" in json.dumps(event) for event in offline.events)
    result = run("workflow.yaml", cwd=home, input_files=[".fixture/garden-replay.json"])
    assert result.ok and result.cost == 0, result.failed
    assert {"video", "poster", "report", "edit", "storyboard", "source"} <= set(result.outputs)
    report = json.loads(result.outputs["report"].read_bytes())
    assert report["full_decode"] == "pass" and report["audio"] == "silent"
    assert report["video"]["width"] == 1920 and report["video"]["height"] == 1080
    assert report["video"]["frame_rate"] == "30/1" and report["frames"] == 555
    assert math.isclose(report["video"]["seconds"], 18.5)
    with Image.open(result.outputs["poster"].path) as poster:
        assert poster.size == (1920, 1080) and poster.format == "WEBP"
    repeated = run("workflow.yaml", cwd=home, input_files=[".fixture/garden-replay.json"])
    assert repeated.ok and repeated.cost == 0, repeated.failed
    hits = [event for event in repeated.events if event.get("event") == "node_finished"]
    assert len(hits) == 2 and all(event.get("cache") == "hit" for event in hits), hits
    assert result.outputs["video"].digest == repeated.outputs["video"].digest
    assert result.outputs["poster"].digest == repeated.outputs["poster"].digest
    # A damaged recording is rejected for free instead of becoming a successful artifact.
    original = (home / ".fixture/garden.mp4").read_bytes()
    damaged = bytearray(original)
    at = original.index(b"mdat") + 32
    damaged[at : at + 4096] = b"\0" * 4096
    (home / ".fixture/damaged.mp4").write_bytes(damaged)
    (home / ".fixture/damaged.json").write_text(
        json.dumps(
            {
                "footage": "damaged.mp4",
                "brief": "../inputs/forma.json",
                "edit": "../inputs/forma-edit.json",
            }
        )
    )
    invalid = run("workflow.yaml", cwd=home, input_files=[".fixture/damaged.json"])
    assert not invalid.ok and invalid.cost == 0, "damaged footage must not produce an artifact"
    edit = json.loads((home / "inputs/forma-edit.json").read_text())
    for label, mutate in (
        ("ordering", lambda value: value["focus"].reverse()),
        ("zoom", lambda value: value["focus"][1].update(zoom=3)),
        ("non-finite focus", lambda value: value["focus"][1].update(x=float("nan"))),
        ("incomplete exit", lambda value: value["focus"][-1].update(zoom=1.8)),
        ("crop outside source", lambda value: value["focus"][1].update(crop=[-0.1, 0, 1, 1])),
        ("empty crop", lambda value: value["focus"][1].update(crop=[0, 0, 0, 1])),
    ):
        candidate = copy.deepcopy(edit)
        mutate(candidate)
        refused(validate_edit, candidate, label)
    brief = json.loads((home / "inputs/forma.json").read_text())
    brief["opening"] = ["Hidden\nline", "Shape it."]
    refused(validate_brief, brief, "multiline title")
    # Framing stays on a beat until the next move's final 0.75 seconds.
    assert camera(edit["focus"], 0.1) == (0.5, 0.5, 1.0, 0, 0, 1, 1)
    assert camera(edit["focus"], 2.5) == (0.16, 0.35, 1.0, 0.014, 0.154, 0.248, 0.664)
    print(
        "product-promo: paid refusal at $0; 1080p30 render; full decode; "
        "exact cache replay; damaged source and invalid edits rejected"
    )
    temporary.cleanup()


if __name__ == "__main__":
    main()
