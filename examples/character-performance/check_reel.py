"""Fresh-copy, provider-free cast reel checks with synthesized portrait test signals."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw

from grida.fx import Answer, FxError, StandInCall, plan, run

from nodes.reel import _crop, _poster_frame, _region, _switch_frame

REGIONS = {"first": [0.2, 0.125, 0.6, 0.75], "second": [0.1, 0.2, 0.8, 0.6]}


def command(args: list[str]) -> None:
    subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-y", *args], check=True)


def main() -> None:
    source = Path(__file__).resolve().parent
    with tempfile.TemporaryDirectory(prefix="fx-cast-reel-check-") as temporary:
        home = Path(temporary) / "example"
        shutil.copytree(
            source,
            home,
            ignore=shutil.ignore_patterns(
                ".fx",
                "runs",
                ".fixtures",
                ".delivery",
                ".showcase-inputs",
                ".venv",
                "__pycache__",
                "dist",
                ".env",
            ),
        )
        os.environ.update(
            GRIDA_FX_DISABLE_DOTENV="1", GRIDA_FX_NETWORK="off", GRIDA_FX_PYTHON=sys.executable
        )
        for key in (
            "GG_API_KEY",
            "OPENAI_API_KEY",
            "OPENROUTER_API_KEY",
            "FAL_KEY",
            "TRIPO_API_KEY",
            "ELEVENLABS_API_KEY",
        ):
            os.environ.pop(key, None)
        fixtures = home / ".fixtures"
        fixtures.mkdir(exist_ok=True)
        for key, size, seconds, color in (
            ("first", "360x640", 3, "00ff00"),
            ("second", "420x600", 5, "ff00ff"),
        ):
            path = fixtures / f"{key}.mp4"
            width, height = (int(side) for side in size.split("x"))
            x, y, w, h = _crop(REGIONS[key], width, height)
            inner_color = "00ffff" if key == "first" else "ffff00"
            command(
                [
                    "-f",
                    "lavfi",
                    "-i",
                    f"testsrc2=size={size}:rate=30:duration={seconds}",
                    "-vf",
                    f"drawbox=x=0:y=0:w=iw:h=ih:color=0x{color}:t=6,"
                    f"drawbox=x={x}:y={y}:w={w}:h={h}:color=0x{inner_color}:t=6,"
                    "drawbox=x=iw/3:y=ih/3:w=iw/3:h=ih/3:color=black:t=fill,"
                    "drawbox=x=iw/3:y=ih/3:w=iw/3:h=ih/3:color=white:t=fill:enable='gte(t,2)'",
                    "-an",
                    "-c:v",
                    "libx264",
                    "-threads",
                    "2",
                    "-pix_fmt",
                    "yuv420p",
                    "-movflags",
                    "+faststart",
                    str(path),
                ]
            )
            (fixtures / f"{key}-report.json").write_text(
                json.dumps(
                    {
                        "kind": "character_performance_finish_v1",
                        "review_status": "sampled_review_pass",
                        "video_sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                        "full_decode": "pass",
                        "audio": "removed",
                        "review_sha256": "1" * 64,
                        "motion_sha256": "2" * 64,
                        "review_scope": "synthetic structural fixture, no artistic approval",
                        "framing_scan": {
                            "status": "clear",
                            "frames_scanned": seconds * 30,
                            "scope": "Synthetic admission marker, no studio or character verdict.",
                        },
                    }
                )
            )
        inputs = {
            "first": "first.mp4",
            "first_report": "first-report.json",
            "first_name": "First fixture",
            "second": "second.mp4",
            "second_report": "second-report.json",
            "second_name": "Second fixture",
        }
        (fixtures / "reel.json").write_text(json.dumps(inputs))
        options = {"cwd": home, "input_files": [".fixtures/reel.json"]}
        planned = plan("reel.yaml", **options)
        assert planned.ok, planned.problems
        result = run("reel.yaml", **options, name="synthetic-reel")
        assert result.ok and result.cost == 0, result.failures
        report = json.loads(result.outputs["report"].read_bytes())
        assert report["finished"] == {
            "width": 1080,
            "height": 1920,
            "fps": 30.0,
            "frames": 240,
            "duration_seconds": 8.0,
        }
        assert report["full_decode"] == "pass" and report["audio"] == "silent"
        assert report["review_status"] == "requires_separate_content_review"
        assert report["video_sha256"] == result.outputs["video"].digest
        for index, item in enumerate(report["segments"]):
            name = ("first", "second")[index]
            assert (
                item["video_sha256"]
                == hashlib.sha256((fixtures / f"{name}.mp4").read_bytes()).hexdigest()
            )
            assert (
                item["report_sha256"]
                == hashlib.sha256((fixtures / f"{name}-report.json").read_bytes()).hexdigest()
            )
            width, height = item["fit_size"]
            assert item["source_region_xywh"] == [0, 0, 1, 1]
            assert item["source_crop_xywh"] == [
                0,
                0,
                item["source"]["width"],
                item["source"]["height"],
            ]
            assert item["complete_source_frame"]
            assert width <= 936 and height <= 1400
            assert abs(width / height - item["source"]["width"] / item["source"]["height"]) < 0.002
            frame = fixtures / f"{name}-reel-frame.png"
            command(
                [
                    "-xerror",
                    "-err_detect",
                    "explode",
                    "-ss",
                    str(1 + 3 * index),
                    "-i",
                    str(result.outputs["video"].path),
                    "-frames:v",
                    "1",
                    str(frame),
                ]
            )
            x, y = (1080 - width) // 2, 230 + (1400 - height) // 2
            # All four marked source corners survive; filling/cropping would lose them.
            with Image.open(frame) as image:
                for point in (
                    (x + 4, y + 4),
                    (x + width - 5, y + 4),
                    (x + 4, y + height - 5),
                    (x + width - 5, y + height - 5),
                ):
                    r, g, b = image.getpixel(point)
                    assert (
                        (g > 180 and r < 70 and b < 70)
                        if index == 0
                        else (r > 180 and b > 180 and g < 70)
                    ), (index, point, (r, g, b))
        with Image.open(result.outputs["poster"].path) as poster:
            assert poster.size == (1080, 1920) and poster.format == "WEBP"
        assert report["poster"]["frame_index"] == 30
        assert report["poster"]["at_seconds"] == 1
        assert report["poster"]["sha256"] == result.outputs["poster"].digest
        # Selecting the second performer changes the poster, never the clip pixels.
        (fixtures / "second-poster.json").write_text(json.dumps({**inputs, "poster_at": 4.5}))
        selected = run("reel.yaml", cwd=home, input_files=[".fixtures/second-poster.json"])
        assert selected.ok and selected.cost == 0, selected.failures
        selected_report = json.loads(selected.outputs["report"].read_bytes())
        assert selected.outputs["video"].digest == result.outputs["video"].digest
        assert selected.outputs["poster"].digest != result.outputs["poster"].digest
        assert selected_report["poster"]["frame_index"] == 135
        assert selected_report["poster"]["sha256"] == selected.outputs["poster"].digest
        width, height = selected_report["segments"][1]["fit_size"]
        with Image.open(selected.outputs["poster"].path) as poster:
            r, g, b = poster.getpixel(((1080 - width) // 2 + 4, 230 + (1400 - height) // 2 + 4))
            assert r > 180 and b > 180 and g < 70, "poster must show the second clip"
        assert _poster_frame(4.519, 240) == 135
        for value in (-1, 8, 9, True, float("nan"), float("inf")):
            try:
                _poster_frame(value, 240)
            except ValueError:
                pass
            else:
                raise AssertionError("invalid poster timestamp accepted")
        (fixtures / "invalid-poster.json").write_text(json.dumps({**inputs, "poster_at": 8}))
        refused = run("reel.yaml", cwd=home, input_files=[".fixtures/invalid-poster.json"])
        assert not refused.ok and refused.cost == 0, "end-of-reel poster timestamp accepted"
        # A shared timeline switches identity without restarting the second performance.
        command(
            [
                "-i",
                str(fixtures / "second.mp4"),
                "-t",
                "3",
                "-an",
                "-c:v",
                "libx264",
                "-threads",
                "2",
                str(fixtures / "second-short.mp4"),
            ]
        )
        short_report = json.loads((fixtures / "second-report.json").read_text())
        short_report["video_sha256"] = hashlib.sha256(
            (fixtures / "second-short.mp4").read_bytes()
        ).hexdigest()
        short_report["framing_scan"]["frames_scanned"] = 90
        (fixtures / "second-short-report.json").write_text(json.dumps(short_report))
        shared_inputs = {
            **inputs,
            "second": "second-short.mp4",
            "second_report": "second-short-report.json",
            "mode": "shared_timeline",
            "switch_at": 1.5,
            "poster_at": 2.5,
        }
        (fixtures / "shared.json").write_text(json.dumps(shared_inputs))
        shared = run("reel.yaml", cwd=home, input_files=[".fixtures/shared.json"])
        assert shared.ok and shared.cost == 0, shared.failures
        shared_report = json.loads(shared.outputs["report"].read_bytes())
        assert shared_report["finished"]["frames"] == 90
        assert shared_report["finished"]["duration_seconds"] == 3
        assert shared_report["shared_timeline"]["switch_frame"] == 45
        assert shared_report["shared_timeline"]["motion_sha256"] == "2" * 64
        assert shared_report["segments"][1]["source_interval_on_output_grid"] == {
            "start_frame": 45,
            "end_frame_exclusive": 90,
            "start_seconds": 1.5,
            "end_seconds": 3,
        }
        width, height = shared_report["segments"][1]["fit_size"]
        with Image.open(shared.outputs["poster"].path) as poster:
            x, y = (1080 - width) // 2, 230 + (1400 - height) // 2
            assert min(poster.getpixel((x + width // 2, y + height // 2))) > 220, (
                "second clip must retain its late white timing marker, not restart at black"
            )
        for label, values in (
            ("unequal", {**inputs, "mode": "shared_timeline"}),
            ("switch", {**shared_inputs, "switch_at": 2.9}),
        ):
            (fixtures / f"bad-{label}.json").write_text(json.dumps(values))
            refused = run("reel.yaml", cwd=home, input_files=[f".fixtures/bad-{label}.json"])
            assert not refused.ok and refused.cost == 0, label
        short_report["motion_sha256"] = "3" * 64
        (fixtures / "different-motion-report.json").write_text(json.dumps(short_report))
        (fixtures / "different-motion.json").write_text(
            json.dumps({**shared_inputs, "second_report": "different-motion-report.json"})
        )
        refused = run("reel.yaml", cwd=home, input_files=[".fixtures/different-motion.json"])
        assert not refused.ok and refused.cost == 0, "unrelated driving timelines accepted"
        assert _switch_frame(1.519, 90) == 45
        for value in (0, 0.49, 2.51, True, float("nan"), float("inf")):
            try:
                _switch_frame(value, 90)
            except ValueError:
                pass
            else:
                raise AssertionError("invalid switch timestamp accepted")
        replay = run("reel.yaml", **options)
        assert replay.ok and replay.cost == 0
        assert replay.outputs["video"].digest == result.outputs["video"].digest
        # Explicit regions preserve every marked crop corner while excluding outer borders.
        (fixtures / "cropped.json").write_text(
            json.dumps(
                {**inputs, "first_region": REGIONS["first"], "second_region": REGIONS["second"]}
            )
        )
        cropped = run("reel.yaml", cwd=home, input_files=[".fixtures/cropped.json"])
        assert cropped.ok and cropped.cost == 0, cropped.failures
        cropped_report = json.loads(cropped.outputs["report"].read_bytes())
        assert not cropped_report["automatic_subject_detection"]
        assert cropped_report["full_decode"] == "pass"
        for index, item in enumerate(cropped_report["segments"]):
            key = ("first", "second")[index]
            assert item["source_region_xywh"] == REGIONS[key]
            crop = _crop(REGIONS[key], item["source"]["width"], item["source"]["height"])
            assert item["source_crop_xywh"] == crop
            assert not item["complete_source_frame"]
            width, height = item["fit_size"]
            assert abs(width / height - crop[2] / crop[3]) < 0.002
            frame = fixtures / f"{key}-crop-frame.png"
            command(
                [
                    "-ss",
                    str(1 + 3 * index),
                    "-i",
                    str(cropped.outputs["video"].path),
                    "-frames:v",
                    "1",
                    str(frame),
                ]
            )
            x, y = (1080 - width) // 2, 230 + (1400 - height) // 2
            with Image.open(frame) as image:
                for point in (
                    (x + 4, y + 4),
                    (x + width - 5, y + 4),
                    (x + 4, y + height - 5),
                    (x + width - 5, y + height - 5),
                ):
                    r, g, b = image.getpixel(point)
                    assert (
                        (g > 180 and b > 180 and r < 70)
                        if index == 0
                        else (r > 180 and g > 180 and b < 70)
                    ), (index, point, (r, g, b))
        # Invalid regions cannot become paid work or successful output.
        for region in (
            [0, 0, 0, 1],
            [0.8, 0, 0.4, 1],
            [-0.1, 0, 1, 1],
            [0, 0, 1],
            [True, 0, 0.5, 1],
        ):
            (fixtures / "invalid-region.json").write_text(
                json.dumps({**inputs, "first_region": region})
            )
            try:
                invalid = run("reel.yaml", cwd=home, input_files=[".fixtures/invalid-region.json"])
            except FxError as error:
                assert "inputs.first_region" in str(error), str(error)
            else:
                assert not invalid.ok and invalid.cost == 0, region
        for region in ([0, 0, float("nan"), 1], [0, float("inf"), 1, 1]):
            try:
                _region(region)
            except ValueError:
                pass
            else:
                raise AssertionError("non-finite region accepted")
        assert _crop([0.101, 0.201, 0.501, 0.601], 100, 100) == [10, 20, 51, 61]
        for status, digest in (
            ("sampled_review_fail", inputs["first"]),
            ("sampled_review_pass", "wrong-digest"),
        ):
            bad = json.loads((fixtures / "first-report.json").read_text())
            bad["review_status"] = status
            if digest == "wrong-digest":
                bad["video_sha256"] = "0" * 64
            (fixtures / "bad-report.json").write_text(json.dumps(bad))
            (fixtures / "bad.json").write_text(
                json.dumps({**inputs, "first_report": "bad-report.json"})
            )
            refused = run("reel.yaml", cwd=home, input_files=[".fixtures/bad.json"])
            assert not refused.ok and refused.cost == 0, (
                "unreviewed or mismatched source must be refused"
            )
        for scan in (
            None,
            {"status": "edge_contact", "frames_scanned": 90},
            {"status": "clear", "frames_scanned": 1},
        ):
            bad = json.loads((fixtures / "first-report.json").read_text())
            bad["framing_scan"] = scan
            (fixtures / "bad-framing-report.json").write_text(json.dumps(bad))
            (fixtures / "bad-framing.json").write_text(
                json.dumps({**inputs, "first_report": "bad-framing-report.json"})
            )
            refused = run("reel.yaml", cwd=home, input_files=[".fixtures/bad-framing.json"])
            assert not refused.ok and refused.cost == 0, "missing, failed or partial scan accepted"
        # The composed graph must expand the two actual child workflows independently.
        for index in range(2):
            plate = Image.new("RGB", (720, 1280), "#f4eee6")
            draw = ImageDraw.Draw(plate)
            draw.rectangle((280, 320, 440, 1000), fill=("#6060aa", "#aa6060")[index])
            plate.save(fixtures / f"plate-{index}.png")
        command(
            [
                "-stream_loop",
                "-1",
                "-i",
                str(fixtures / "first.mp4"),
                "-t",
                "4",
                "-an",
                "-vf",
                "fps=24",
                "-c:v",
                "libx264",
                "-threads",
                "2",
                str(fixtures / "motion.mp4"),
            ]
        )
        # Paid-answer stand-ins have a moving inset subject and uniform studio margins.
        command(
            [
                "-f",
                "lavfi",
                "-i",
                "color=c=0xf4eee6:s=360x640:r=24:d=4",
                "-f",
                "lavfi",
                "-i",
                "color=c=0x6060aa:s=120x240:r=24:d=4",
                "-filter_complex",
                "[0:v][1:v]overlay=x='120+24*sin(2*PI*t)':y='200+12*cos(2*PI*t)':shortest=1",
                "-an",
                "-c:v",
                "libx264",
                "-threads",
                "2",
                "-pix_fmt",
                "yuv420p",
                str(fixtures / "studio-take.mp4"),
            ]
        )
        (fixtures / "cast.json").write_text(
            json.dumps(
                {
                    "first_character": "plate-0.png",
                    "second_character": "plate-1.png",
                    "motion": "motion.mp4",
                    "fps": 24,
                    "first_region": [0.1, 0.1, 0.8, 0.8],
                    "second_region": [0, 0, 1, 1],
                }
            )
        )
        cast = plan("cast.yaml", cwd=home, input_files=[".fixtures/cast.json"])
        assert cast.ok, cast.problems
        refused = run("cast.yaml", cwd=home, input_files=[".fixtures/cast.json"])
        assert not refused.ok and refused.cost == 0, (
            "the composed paid children must refuse offline"
        )
        calls = []

        def answer(call: StandInCall) -> Answer:
            calls.append(call.capability)
            if call.capability == "video.motion_transfer":
                return Answer(files={"video": (fixtures / "studio-take.mp4").read_bytes()})
            assert call.capability == "structured.generate"
            return Answer.json(
                {
                    "status": "pass",
                    "identity": "pass",
                    "motion_correspondence": "pass",
                    "anatomy_and_framing": "pass",
                    "style": "pass",
                    "observations": [],
                    "limitations": ["Synthetic structural fixture, no artistic acceptance."],
                    "review_scope": "sampled_frames_and_pose_correspondence",
                }
            )

        composed = run(
            "cast.yaml",
            cwd=home,
            input_files=[".fixtures/cast.json"],
            stand_in=answer,
            name="synthetic-cast",
        )
        assert composed.ok and composed.cost == 0, composed.failures
        assert calls.count("video.motion_transfer") == calls.count("structured.generate") == 2
        composed_report = json.loads(composed.outputs["report"].read_bytes())
        assert composed_report["finished"]["duration_seconds"] == 8
        assert composed_report["finished"]["frames"] == 240
        assert composed_report["segments"][0]["source_region_xywh"] == [0.1, 0.1, 0.8, 0.8]
        assert composed_report["segments"][1]["source_region_xywh"] == [0, 0, 1, 1]
        for key in ("first_report", "second_report"):
            child_report = json.loads(composed.outputs[key].read_bytes())
            assert child_report["review_status"] == "sampled_review_pass"
            assert child_report["framing_scan"]["status"] == "clear"
            assert child_report["framing_scan"]["frames_scanned"] == 96
        continued = run(
            "cast.yaml",
            cwd=home,
            input_files=[".fixtures/cast.json"],
            inputs={"mode": "shared_timeline", "switch_at": 2, "poster_at": 3.2},
            stand_in=answer,
        )
        assert continued.ok and continued.cost == 0, continued.failures
        continued_report = json.loads(continued.outputs["report"].read_bytes())
        assert continued_report["composition_mode"] == "shared_timeline"
        assert continued_report["finished"]["frames"] == 120
        assert continued_report["shared_timeline"]["switch_frame"] == 60
        assert continued_report["poster"]["frame_index"] == 96
        assert calls.count("video.motion_transfer") == calls.count("structured.generate") == 2
        print(
            "cast reel: $0 render, strict decode, 1080x1920/30fps, complete source corners; "
            "caller-region corners and invalid bounds; lineage/review refusal, exact cache replay; "
            "explicit poster frame and shared-timeline cut/phase checks; "
            "composed cast refuses paid calls "
            "and completes with four provider stand-ins at $0"
        )


if __name__ == "__main__":
    main()
