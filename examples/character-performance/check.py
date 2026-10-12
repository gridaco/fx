"""Exercise the actual graph with synthetic media and stand-ins, without network or keys."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from grida.fx import Answer, StandInCall, plan, run
from PIL import Image, ImageDraw

from nodes.performance import studio_border_frame

ROOT = Path(__file__).resolve().parent
KEYS = (
    "GG_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "FAL_KEY",
    "TRIPO_API_KEY",
    "ELEVENLABS_API_KEY",
)


def check_studio_border() -> None:
    """Exercise the conservative pixel proxy without relying on a model verdict."""
    clear = Image.new("RGB", (96, 128), (240, 234, 226))
    ImageDraw.Draw(clear).rectangle((24, 30, 70, 100), fill="#403080")
    assert studio_border_frame(clear)["status"] == "clear"
    for side, rectangle in {
        "left": (0, 40, 30, 80),
        "right": (60, 40, 95, 80),
        "top": (35, 0, 60, 40),
        "bottom": (35, 90, 60, 127),
    }.items():
        touching = clear.copy()
        ImageDraw.Draw(touching).rectangle(rectangle, fill="#403080")
        result = studio_border_frame(touching)
        assert result["status"] == "edge_contact" and result["sides"] == [side], result
    unknown = clear.copy()
    ImageDraw.Draw(unknown).rectangle((0, 0, 7, 7), fill="#403080")
    assert studio_border_frame(unknown)["status"] == "unknown"
    threshold = clear.copy()
    threshold.putpixel((0, 64), (220, 234, 226))
    assert studio_border_frame(threshold)["status"] == "clear"
    threshold.putpixel((0, 64), (219, 234, 226))
    assert studio_border_frame(threshold)["status"] == "edge_contact"


def check(home: Path) -> None:
    check_studio_border()
    os.environ.update(
        GRIDA_FX_DISABLE_DOTENV="1", GRIDA_FX_NETWORK="off", GRIDA_FX_PYTHON=sys.executable
    )
    for key in KEYS:
        os.environ.pop(key, None)
    fixtures = home / ".fixtures"
    fixtures.mkdir(exist_ok=True)
    image = Image.new("RGBA", (240, 420))
    draw = ImageDraw.Draw(image)
    draw.ellipse((80, 20, 160, 100), fill="#ea99b0")
    draw.rounded_rectangle((70, 100, 170, 270), radius=20, fill="#4b639c")
    draw.line((90, 260, 70, 400), fill="#44444a", width=25)
    draw.line((150, 260, 170, 400), fill="#44444a", width=25)
    image.save(fixtures / "character.png")
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
            "testsrc2=size=256x448:rate=15:duration=4",
            "-c:v",
            "libx264",
            "-threads",
            "2",
            "-pix_fmt",
            "yuv420p",
            "-an",
            str(fixtures / "motion.mp4"),
        ],
        check=True,
    )
    subprocess.run(
        [
            "ffmpeg",
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-i",
            str(fixtures / "motion.mp4"),
            "-vf",
            "scale=128:224,hue=h=70,pad=256:448:(ow-iw)/2:(oh-ih)/2:color=0xf4eee6",
            "-an",
            "-c:v",
            "libx264",
            "-threads",
            "2",
            str(fixtures / "take.mp4"),
        ],
        check=True,
    )
    inputs = fixtures / "inputs.json"
    inputs.write_text(
        json.dumps({"character": "character.png", "motion": "motion.mp4", "name": "Fixture"})
    )
    options = {"cwd": home, "input_files": [inputs]}
    planned = plan("workflow.yaml", **options)
    assert planned.ok, planned.problems
    refused = run("workflow.yaml", **options)
    assert not refused.ok and refused.cost == 0
    assert any("is a paid call; run with --live" in json.dumps(event) for event in refused.events)
    calls = []
    verdict = {
        "status": "pass",
        "identity": "pass",
        "motion_correspondence": "pass",
        "anatomy_and_framing": "pass",
        "style": "pass",
        "observations": [],
        "limitations": ["Synthetic structural fixture, no semantic quality verdict."],
        "review_scope": "sampled_frames_and_pose_correspondence",
    }

    def answer(call: StandInCall) -> Answer:
        calls.append(call.capability)
        if call.capability == "video.motion_transfer":
            assert call.request["duration"] == 4
            assert call.request["keep_original_sound"] is False
            (fixtures / "motion-prepared.mp4").write_bytes(call.request["motion"].read_bytes())
            return Answer(files={"video": (fixtures / "take.mp4").read_bytes()})
        assert call.capability == "structured.generate"
        return Answer.json(verdict)

    result = run("workflow.yaml", **options, stand_in=answer)
    assert result.ok and result.cost == 0, result.failures
    assert calls == ["video.motion_transfer", "structured.generate"], calls
    report = json.loads(result.outputs["report"].read_bytes())
    assert report["full_decode"] == "pass" and report["finished"]["frames"] == 60
    assert report["review_status"] == "sampled_review_pass"
    assert report["sampled_review_status"] == "pass"
    assert report["framing_scan"]["status"] == "clear"
    assert report["framing_scan"]["frames_scanned"] == 60
    assert not report["framing_scan"]["edge_contacts"]
    assert not report["framing_scan"]["unknown_frames"]
    assert len(report["sample_times_seconds"]) == 8
    assert report["publication_status"] == "requires_separate_content_review"
    # A cache replay must ask no provider stand-in and must reproduce the exact finished bytes.
    calls.clear()
    replay = run("workflow.yaml", **options, stand_in=answer)
    assert replay.ok and replay.cost == 0 and not calls
    assert replay.outputs["video"].digest == result.outputs["video"].digest
    # Public runs consume externally prepared footage, so stock input bytes remain unplaced.
    prepared_inputs = fixtures / "prepared.json"
    prepared_inputs.write_text(
        json.dumps(
            {
                "character": str(result.outputs["character"].path),
                "motion": "motion-prepared.mp4",
                "prepared": True,
                "seconds": 4,
                "name": "Fixture",
            }
        )
    )
    prepared = run("workflow.yaml", cwd=home, input_files=[prepared_inputs], stand_in=answer)
    assert prepared.ok and prepared.cost == 0 and not calls, prepared.failures
    exported = fixtures / "export"
    subprocess.run(
        [
            sys.executable,
            "-m",
            "grida.fx",
            "export",
            str(prepared.run_dir),
            "--out",
            str(exported),
        ],
        check=True,
        capture_output=True,
    )
    manifest = json.loads((exported / "manifest.json").read_text())
    motion_digest = hashlib.sha256((fixtures / "motion-prepared.mp4").read_bytes()).hexdigest()
    motion_artifact = next(
        item for item in manifest["artifacts"] if item["digest"] == motion_digest
    )
    assert motion_artifact["path"] is None and motion_artifact["omitted_reason"] == "unplaced_input"
    # A semantic rejection is recorded and must not launch an implicit new creative take.
    review_prompt = home / "prompts/review.md"
    review_prompt.write_text(review_prompt.read_text() + "\nSynthetic refusal branch.\n")
    verdict.update(status="fail", anatomy_and_framing="fail")
    rejected = run("workflow.yaml", **options, stand_in=answer)
    assert calls == ["structured.generate"], calls
    assert rejected.cost == 0
    assert rejected.steps["admit"].facts["verdict"] == "reject"
    decision = json.loads(rejected.outputs["report"].read_bytes())
    assert decision["review_status"] == "sampled_review_fail"
    # A model's sampled pass cannot override an inter-sample border rejection.
    subprocess.run(
        [
            "ffmpeg",
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-i",
            str(fixtures / "take.mp4"),
            "-vf",
            "drawbox=x=iw-20:y=100:w=20:h=40:color=red:t=fill:enable='between(t,1.2,1.5)'",
            "-c:v",
            "libx264",
            "-threads",
            "2",
            "-an",
            str(fixtures / "edge-take.mp4"),
        ],
        check=True,
    )
    edge_inputs = fixtures / "edge.json"
    edge_inputs.write_text(
        json.dumps(
            {
                "character": "character.png",
                "motion": "motion.mp4",
                "name": "Fixture",
                "direction": "Synthetic inter-sample border rejection branch.",
            }
        )
    )
    verdict.update(status="pass", anatomy_and_framing="pass")
    calls.clear()

    def edge_answer(call: StandInCall) -> Answer:
        calls.append(call.capability)
        if call.capability == "video.motion_transfer":
            return Answer(files={"video": (fixtures / "edge-take.mp4").read_bytes()})
        assert call.capability == "structured.generate"
        return Answer.json(verdict)

    edge_result = run("workflow.yaml", cwd=home, input_files=[edge_inputs], stand_in=edge_answer)
    assert edge_result.ok and edge_result.cost == 0, edge_result.failures
    assert calls == ["video.motion_transfer", "structured.generate"], calls
    edge_report = json.loads(edge_result.outputs["report"].read_bytes())
    assert edge_report["sampled_review_status"] == "pass"
    assert edge_report["review_status"] == "framing_guard_fail"
    assert edge_result.steps["admit"].facts["verdict"] == "reject"
    contacts = edge_report["framing_scan"]["edge_contacts"]
    assert edge_report["framing_scan"]["status"] == "edge_contact" and contacts
    assert all(item["sides"] == ["right"] for item in contacts)
    assert all(1.19 <= item["time_seconds"] <= 1.51 for item in contacts), contacts
    assert not list(home.glob(".fx/**/studio-border-*/*.png")), "scan PNGs must be removed"
    # Input mistakes must be rejected during free preparation, before any call is admitted.
    invalid = fixtures / "too-long.json"
    invalid.write_text(
        json.dumps({"character": "character.png", "motion": "motion.mp4", "seconds": 5})
    )
    bad = plan("workflow.yaml", cwd=home, input_files=[invalid])
    assert not bad.ok, "an interval beyond the source should fail before spending"
    Image.new("RGBA", (720, 1280)).save(fixtures / "empty.png")
    empty = fixtures / "empty.json"
    empty.write_text(
        json.dumps(
            {
                "character": "empty.png",
                "motion": "motion-prepared.mp4",
                "seconds": 4,
                "prepared": True,
            }
        )
    )
    assert not plan("workflow.yaml", cwd=home, input_files=[empty]).ok
    subprocess.run(
        [
            "ffmpeg",
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-i",
            str(fixtures / "motion-prepared.mp4"),
            "-map",
            "0:v:0",
            "-map",
            "0:v:0",
            "-c",
            "copy",
            str(fixtures / "multi.mp4"),
        ],
        check=True,
    )
    multi = fixtures / "multi.json"
    multi.write_text(
        json.dumps(
            {
                "character": str(result.outputs["character"].path),
                "motion": "multi.mp4",
                "seconds": 4,
                "prepared": True,
            }
        )
    )
    assert not plan("workflow.yaml", cwd=home, input_files=[multi]).ok
    packets = json.loads(
        subprocess.check_output(
            [
                "ffprobe",
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_packets",
                "-show_entries",
                "packet=pos,size",
                "-of",
                "json",
                str(fixtures / "motion.mp4"),
            ]
        )
    )["packets"]
    damaged = bytearray((fixtures / "motion.mp4").read_bytes())
    packet = packets[2]
    position = int(packet["pos"])
    count = min(500, int(packet["size"]))
    damaged[position : position + count] = bytes([255]) * count
    (fixtures / "damaged.mp4").write_bytes(damaged)
    damaged_inputs = fixtures / "damaged.json"
    damaged_inputs.write_text(json.dumps({"character": "character.png", "motion": "damaged.mp4"}))
    assert not plan("workflow.yaml", cwd=home, input_files=[damaged_inputs]).ok
    # Authored versions invalidate node behavior; Python body edits alone do not.
    version_inputs = fixtures / "version.json"
    version_inputs.write_text(
        json.dumps(
            {
                "character": "character.png",
                "motion": "motion.mp4",
                "video": "take.mp4",
                "name": "Version fixture",
                "seconds": 4,
            }
        )
    )
    version_options = {"cwd": home, "input_files": [version_inputs]}
    baseline = run("finish.yaml", **version_options)
    assert baseline.ok and baseline.cost == 0, baseline.failures
    worker = home / "nodes/performance.py"
    original = worker.read_text()
    changed = original.replace(
        '"framing_scan": framing_scan,',
        '"framing_scan": {**framing_scan, "test_body_version_marker": True},',
    )
    assert changed != original
    try:
        worker.write_text(changed)
        shutil.rmtree(home / "nodes/__pycache__", ignore_errors=True)
        unchanged_version = run("finish.yaml", **version_options)
        assert unchanged_version.ok and unchanged_version.cost == 0
        unchanged_report = json.loads(unchanged_version.outputs["report"].read_bytes())
        assert "test_body_version_marker" not in unchanged_report["framing_scan"]
        assert unchanged_version.outputs["report"].digest == baseline.outputs["report"].digest
        changed = changed.replace("version=2,\n)\ndef finish(", "version=3,\n)\ndef finish(")
        assert "version=3,\n)\ndef finish(" in changed
        worker.write_text(changed)
        shutil.rmtree(home / "nodes/__pycache__", ignore_errors=True)
        new_version = run("finish.yaml", **version_options)
        assert new_version.ok and new_version.cost == 0, new_version.failures
        changed_report = json.loads(new_version.outputs["report"].read_bytes())
        assert changed_report["framing_scan"]["test_body_version_marker"] is True
        assert new_version.outputs["report"].digest != baseline.outputs["report"].digest
        assert new_version.outputs["video"].digest == baseline.outputs["video"].digest
    finally:
        worker.write_text(original)
        shutil.rmtree(home / "nodes/__pycache__", ignore_errors=True)
    print(
        "Motion graph checked at $0, including border rejection, explicit node versions, "
        "corruption refusal and stock export exclusion."
    )


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="fx-performance-check-") as temporary:
        home = Path(temporary) / "project"
        shutil.copytree(
            ROOT,
            home,
            ignore=shutil.ignore_patterns(
                ".fx",
                ".fixtures",
                ".showcase-inputs",
                ".venv",
                "__pycache__",
                "uv.lock",
                ".delivery",
                "runs",
                "dist",
                ".env",
            ),
        )
        check(home)


if __name__ == "__main__":
    main()
