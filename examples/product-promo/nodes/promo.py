"""Inspection and rendering nodes for a bounded recording-to-promo workflow."""

from __future__ import annotations

import hashlib
import io
import subprocess
from fractions import Fraction
from pathlib import Path

from PIL import Image, ImageDraw

from grida.fx import Ctx, node

from nodes.composition import (
    FPS,
    SIZE,
    Composition,
    map_source_hint,
    probe,
    source_timeline,
    validate_brief,
    validate_edit,
)


def digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as source:
        while block := source.read(1024 * 1024):
            result.update(block)
    return result.hexdigest()


def sample_times(duration: float, count: int = 12, frame_rate: str = "30/1") -> list[float]:
    try:
        rate = Fraction(frame_rate)
    except (ValueError, ZeroDivisionError):
        raise ValueError("source frame rate must be a positive rational number") from None
    if rate <= 0:
        raise ValueError("source frame rate must be a positive rational number")
    tail = max(0.08, float(1 / rate) + 0.001)
    return [index * max(0, duration - tail) / (count - 1) for index in range(count)]


def sample_frames(
    ffmpeg: Path, source: Path, duration: float, count: int = 12, frame_rate: str = "30/1"
) -> list[Image.Image]:
    """Timestamped samples, including the final visible moment; no claims about audio."""
    frames = []
    for at in sample_times(duration, count, frame_rate):
        try:
            result = subprocess.run(
                [
                    str(ffmpeg),
                    "-v",
                    "error",
                    "-ss",
                    str(at),
                    "-i",
                    str(source),
                    "-frames:v",
                    "1",
                    "-f",
                    "image2pipe",
                    "-vcodec",
                    "png",
                    "-",
                ],
                capture_output=True,
                timeout=30,
                check=True,
            )
            with Image.open(io.BytesIO(result.stdout)) as image:
                frames.append(image.convert("RGB"))
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError):
            raise ValueError("FFmpeg could not sample the supplied video") from None
    return frames


def sheet(frames: list[Image.Image], seconds: float, frame_rate: str = "30/1") -> Image.Image:
    canvas = Image.new("RGB", (1600, 780), "#f3f4ee")
    draw = ImageDraw.Draw(canvas)
    for index, image in enumerate(frames):
        thumb = image.copy()
        thumb.thumbnail((390, 220), Image.Resampling.LANCZOS)
        x, y = (index % 4) * 400 + 5, (index // 4) * 260 + 5
        canvas.paste(thumb, (x + (390 - thumb.width) // 2, y))
        draw.text(
            (x + 8, y + 226),
            f"{index + 1:02d}  {sample_times(seconds, len(frames), frame_rate)[index]:.2f}s",
            fill="#14201b",
        )
    return canvas


@node(
    "inspect",
    inputs={"footage": "video", "brief": "json"},
    outputs={"source": "json", "storyboard": "image/png"},
    tools=["ffmpeg", "ffprobe"],
    resources=["nodes/composition.py"],
    version=2,
)
def inspect(ctx: Ctx) -> dict:
    brief = validate_brief(ctx.read.json("brief"))
    source = ctx.inputs["footage"].path
    metadata = probe(ctx.tool("ffprobe").executable, source)
    try:
        subprocess.run(
            [
                str(ctx.tool("ffmpeg").executable),
                "-v",
                "error",
                "-nostdin",
                "-xerror",
                "-err_detect",
                "explode",
                "-i",
                str(source),
                "-f",
                "null",
                "-",
            ],
            capture_output=True,
            timeout=120,
            check=True,
        )
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError):
        raise ctx.fail("FFmpeg refused the complete source recording decode") from None
    if any(hint["source_second"] > metadata["seconds"] for hint in brief.get("focus_hints", [])):
        raise ctx.fail("a focus hint falls after the source recording ends")
    metadata["sha256"] = digest(source)
    metadata["sampling"] = "12 equally spaced chronological frames; audio is not sampled"
    metadata["output"] = {"width": SIZE[0], "height": SIZE[1], "fps": FPS, "audio": False}
    storyboard = sheet(
        sample_frames(
            ctx.tool("ffmpeg").executable,
            source,
            metadata["seconds"],
            frame_rate=metadata["frame_rate"],
        ),
        metadata["seconds"],
        metadata["frame_rate"],
    )
    ctx.fact("source_seconds", metadata["seconds"])
    ctx.fact("source_pixels_preserved", True)
    return {"source": ctx.out.json(metadata), "storyboard": ctx.out.png(storyboard)}


@node(
    "render",
    inputs={"footage": "video", "brief": "json", "edit": "json", "font": "file"},
    outputs={
        "video": "video/mp4",
        "poster": "image/webp",
        "report": "json",
        "edit": "json",
        "storyboard": "image/png",
    },
    tools=["ffmpeg", "ffprobe"],
    resources=["nodes/composition.py"],
    version=4,
)
def render(ctx: Ctx) -> dict:
    brief = validate_brief(ctx.read.json("brief"))
    edit = validate_edit(ctx.read.json("edit"))
    composer = Composition(brief, edit, ctx.inputs["font"].path)
    ffmpeg, ffprobe = ctx.tool("ffmpeg").executable, ctx.tool("ffprobe").executable
    source = ctx.inputs["footage"].path
    metadata = probe(ffprobe, source)
    body_frames = round(edit["footage_seconds"] * FPS)
    timeline = source_timeline(edit, metadata["seconds"])
    hint_mapping = []
    if timeline is not None:
        hint_mapping = [
            {
                "source_second": hint["source_second"],
                "body_second": map_source_hint(timeline, hint["source_second"]),
                "video_second": edit["opening_seconds"]
                + map_source_hint(timeline, hint["source_second"]),
            }
            for hint in brief.get("focus_hints", [])
        ]
    source_width = min(metadata["width"], 1920)
    source_height = round(metadata["height"] * source_width / metadata["width"])
    if timeline is None:
        # Preserve the legacy command and its exact pixels when no intervals are supplied.
        filtering = [
            "-vf",
            f"setpts={edit['footage_seconds'] / metadata['seconds']}*PTS,"
            f"fps={FPS},scale={source_width}:{source_height}",
        ]
    else:
        graph = (
            f"[0:v]split={len(timeline)}"
            + "".join(f"[raw{index}]" for index in range(len(timeline)))
            + ";"
        )
        for index, segment in enumerate(timeline):
            graph += (
                f"[raw{index}]trim=start={segment['source_start']}:end={segment['source_end']},"
                f"setpts=(PTS-STARTPTS)/{segment['playback_rate']},fps={FPS},"
                f"tpad=stop_mode=clone:stop_duration={segment['duration_seconds']},"
                f"trim=end_frame={segment['frames']},"
                f"setpts=PTS-STARTPTS[cut{index}];"
            )
        graph += "".join(f"[cut{index}]" for index in range(len(timeline))) + (
            f"concat=n={len(timeline)}:v=1:a=0,scale={source_width}:{source_height}[out]"
        )
        filtering = ["-filter_complex", graph, "-map", "[out]"]
    decode = subprocess.Popen(
        [
            str(ffmpeg),
            "-v",
            "error",
            "-nostdin",
            "-xerror",
            "-err_detect",
            "explode",
            "-i",
            str(source),
            "-an",
            *filtering,
            "-frames:v",
            str(body_frames),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    destination = ctx.out.path("promo.mp4")
    encode = subprocess.Popen(
        [
            str(ffmpeg),
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            f"{SIZE[0]}x{SIZE[1]}",
            "-r",
            str(FPS),
            "-i",
            "-",
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-crf",
            "18",
            "-threads",
            "2",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            str(destination),
        ],
        stdin=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    last_frame = None
    opening_frames = round(edit["opening_seconds"] * FPS)
    body_end = opening_frames + body_frames
    poster = None
    selected = []
    sample_indices = {round(index * (composer.frames - 1) / 11) for index in range(12)}
    try:
        for index in range(composer.frames):
            if ctx.cancelled:
                raise ctx.fail("promo rendering cancelled")
            if opening_frames <= index < body_end:
                data = decode.stdout.read(source_width * source_height * 3)
                if data:
                    if len(data) != source_width * source_height * 3:
                        raise ValueError("the recording decoder returned a partial frame")
                    last_frame = Image.frombytes("RGB", (source_width, source_height), data)
                elif last_frame is None:
                    raise ValueError("the recording decoder produced no frame")
            frame = composer.picture(index, last_frame)
            encode.stdin.write(frame.tobytes())
            if index in sample_indices:
                selected.append(frame.copy())
            if index == round((edit["opening_seconds"] + edit["footage_seconds"] - 0.12) * FPS):
                poster = composer.body(last_frame, edit["footage_seconds"])
            if index % FPS == 0:
                ctx.progress("Rendering exact recording pixels", index / composer.frames)
        encode.stdin.close()
        encode.wait(timeout=120)
        decode.stdout.close()
        decode.wait(timeout=30)
        if encode.returncode or decode.returncode:
            raise ValueError("FFmpeg did not complete the recording composition")
    finally:
        for process in (decode, encode):
            if process.poll() is None:
                process.kill()
                process.wait()
    result = probe(ffprobe, destination)
    # Full-file decode catches corruption beyond the sampled poster frames.
    try:
        subprocess.run(
            [
                str(ffmpeg),
                "-v",
                "error",
                "-nostdin",
                "-xerror",
                "-err_detect",
                "explode",
                "-i",
                str(destination),
                "-f",
                "null",
                "-",
            ],
            capture_output=True,
            timeout=120,
            check=True,
        )
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError):
        raise ctx.fail("FFmpeg refused the full rendered video decode") from None
    if (result["width"], result["height"]) != SIZE or abs(
        result["seconds"] - composer.frames / FPS
    ) > 1 / FPS + 0.001:
        raise ValueError("the rendered dimensions or duration differ from the edit")
    if result["has_audio"]:
        raise ValueError("this silent composition unexpectedly contains audio")
    poster_buffer = io.BytesIO()
    (poster or selected[6]).save(poster_buffer, "WEBP", quality=92, method=6)
    report = {
        "contract": "product-promo/v1",
        "video": result,
        "source_sha256": digest(source),
        "brief_sha256": digest(ctx.inputs["brief"].path),
        "font_sha256": digest(ctx.inputs["font"].path),
        "edit": edit,
        "source_playback_rate": round(metadata["seconds"] / edit["footage_seconds"], 6),
        "frames": composer.frames,
        "full_decode": "pass",
        "audio": "silent",
        "ui_treatment": (
            "supplied recording pixels, uniformly scaled and framed; no generative redraw"
        ),
        "semantic_review": "required separately; structural validation is not artistic acceptance",
    }
    if timeline is not None:
        report.pop("source_playback_rate")
        report["source_timeline"] = timeline
        report["focus_hint_mapping"] = hint_mapping
        report["source_selection"] = (
            "explicit chronological intervals; omitted ranges are editorial cuts"
        )
    ctx.fact("seconds", result["seconds"])
    ctx.fact("frames", composer.frames)
    ctx.fact("full_decode", "pass")
    return {
        "video": ctx.out.file(destination, "video/mp4"),
        "poster": ctx.out.bytes(poster_buffer.getvalue(), "image/webp"),
        "storyboard": ctx.out.png(sheet(selected, composer.frames / FPS)),
        "report": ctx.out.json(report),
        "edit": ctx.out.json(edit),
    }
