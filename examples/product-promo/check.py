"""Provider-free contract, refusal, rendering and cache checks for product-promo."""

from __future__ import annotations

import copy
import io
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

from nodes.composition import (
    camera,
    map_source_hint,
    source_timeline,
    validate_brief,
    validate_edit,
)
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
    # Actual decoded output must omit the green interval and cut from red to blue
    # on the authored frame, independently of the timeline helper's arithmetic.
    subprocess.run(
        [
            "ffmpeg",
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=640x360:r=12:d=1",
            "-f",
            "lavfi",
            "-i",
            "color=lime:s=640x360:r=12:d=1",
            "-f",
            "lavfi",
            "-i",
            "color=blue:s=640x360:r=12:d=1",
            "-filter_complex",
            "[0:v][1:v][2:v]concat=n=3:v=1:a=0[out]",
            "-map",
            "[out]",
            "-an",
            "-c:v",
            "libx264",
            "-threads",
            "2",
            str(home / ".fixture/colors.mp4"),
        ],
        check=True,
    )
    segment_edit = {
        "opening_seconds": 2,
        "footage_seconds": 6,
        "closing_seconds": 2.5,
        "focus": [
            {"at": at, "x": 0.5, "y": 0.5, "zoom": 1, "crop": [0, 0, 1, 1]} for at in (0, 3, 6)
        ],
        "source_segments": [
            {"source_start": 0, "source_end": 0.9, "duration_seconds": 3},
            {"source_start": 2.1, "source_end": 2.9, "duration_seconds": 3},
        ],
        "editorial_note": "Synthetic red and blue intervals; the green wait is omitted.",
    }
    segment_brief = json.loads((home / "inputs/forma.json").read_text())
    segment_brief["focus_hints"] = [
        {"source_second": 0.3, "x": 0.5, "y": 0.5},
        {"source_second": 2.3, "x": 0.5, "y": 0.5},
    ]
    for name, value in (("segment-edit", segment_edit), ("segment-brief", segment_brief)):
        (home / f".fixture/{name}.json").write_text(json.dumps(value))
    segment_inputs = {
        "footage": "colors.mp4",
        "brief": "segment-brief.json",
        "edit": "segment-edit.json",
    }
    (home / ".fixture/segments.json").write_text(json.dumps(segment_inputs))
    segmented = run("workflow.yaml", cwd=home, input_files=[".fixture/segments.json"])
    assert segmented.ok and segmented.cost == 0, segmented.failed
    segment_report = json.loads(segmented.outputs["report"].read_bytes())
    assert segment_report["frames"] == 315 and segment_report["full_decode"] == "pass"
    assert [item["frames"] for item in segment_report["source_timeline"]] == [90, 90]
    assert math.isclose(segment_report["focus_hint_mapping"][0]["body_second"], 1)
    assert math.isclose(segment_report["focus_hint_mapping"][1]["body_second"], 3.75)
    for frame_index, channel in ((90, 0), (149, 0), (150, 2), (210, 2)):
        pixels = subprocess.run(
            [
                "ffmpeg",
                "-v",
                "error",
                "-i",
                str(segmented.outputs["video"].path),
                "-vf",
                f"select=eq(n\\,{frame_index})",
                "-frames:v",
                "1",
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "-",
            ],
            check=True,
            capture_output=True,
        ).stdout
        with Image.open(io.BytesIO(pixels)) as frame:
            color = frame.convert("RGB").getpixel((960, 540))
        assert color[channel] > 220 and color[1] < 30 and color[2 - channel] < 30, (
            frame_index,
            color,
        )
    timeline = source_timeline(segment_edit, 3)
    boundary_edit = copy.deepcopy(segment_edit)
    boundary_edit["source_segments"][0].update(source_start=0.2, source_end=0.3)
    assert source_timeline(boundary_edit, 3)[0]["frames"] == 90
    assert math.isclose(map_source_hint(timeline, 2.1), 3)
    for second in (0.9, 1.5, 2.9):
        refused(lambda value: map_source_hint(timeline, value), second, "omitted source hint")
    refused(lambda value: source_timeline(value, 2.5), segment_edit, "source-confined interval")
    assert source_timeline(json.loads((home / "inputs/forma-edit.json").read_text()), 12) is None
    # Slow a sub-frame-aligned, low-fps interval to the maximum body duration.
    # Its last sampled pixel must hold to the declared output length.
    slow_edit = copy.deepcopy(segment_edit)
    slow_edit.update(
        footage_seconds=16,
        source_segments=[{"source_start": 0.01, "source_end": 0.12, "duration_seconds": 16}],
        focus=[
            {"at": at, "x": 0.5, "y": 0.5, "zoom": 1, "crop": [0, 0, 1, 1]} for at in (0, 8, 16)
        ],
    )
    slow_brief = copy.deepcopy(segment_brief)
    slow_brief["focus_hints"] = [{"source_second": 0.1, "x": 0.5, "y": 0.5}]
    (home / ".fixture/slow-edit.json").write_text(json.dumps(slow_edit))
    (home / ".fixture/slow-brief.json").write_text(json.dumps(slow_brief))
    (home / ".fixture/slow.json").write_text(
        json.dumps({"footage": "colors.mp4", "brief": "slow-brief.json", "edit": "slow-edit.json"})
    )
    slowed = run("workflow.yaml", cwd=home, input_files=[".fixture/slow.json"])
    assert slowed.ok and slowed.cost == 0, slowed.failed
    slow_report = json.loads(slowed.outputs["report"].read_bytes())
    assert slow_report["frames"] == 615 and slow_report["full_decode"] == "pass"
    for label, mutate in (
        ("overlap", lambda value: value["source_segments"][1].update(source_start=0.5)),
        ("out of order", lambda value: value["source_segments"].reverse()),
        ("zero interval", lambda value: value["source_segments"][0].update(source_end=0)),
        (
            "nonfinite interval",
            lambda value: value["source_segments"][0].update(source_start=float("nan")),
        ),
        ("duration sum", lambda value: value["source_segments"][0].update(duration_seconds=2)),
        (
            "partial output frame",
            lambda value: value["source_segments"][0].update(duration_seconds=3.01),
        ),
        ("boolean interval", lambda value: value["source_segments"][0].update(source_start=True)),
    ):
        candidate = copy.deepcopy(segment_edit)
        mutate(candidate)
        refused(validate_edit, candidate, label)
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
        "; explicit interval output pixels/cut frame and source-hint mapping verified"
    )
    temporary.cleanup()


if __name__ == "__main__":
    main()
