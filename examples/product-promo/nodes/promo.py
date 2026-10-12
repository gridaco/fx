"""Inspection and rendering nodes for a bounded recording-to-promo workflow."""

from __future__ import annotations

import hashlib
import io
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw

from grida.fx import Ctx, node

from nodes.composition import FPS, SIZE, Composition, probe, validate_brief, validate_edit


def digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as source:
        while block := source.read(1024 * 1024):
            result.update(block)
    return result.hexdigest()


def sample_frames(
    ffmpeg: Path, source: Path, duration: float, count: int = 12
) -> list[Image.Image]:
    """Timestamped samples, including the final visible moment; no claims about audio."""
    frames = []
    for index in range(count):
        at = index * max(0, duration - 0.08) / (count - 1)
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


def sheet(frames: list[Image.Image], seconds: float) -> Image.Image:
    canvas = Image.new("RGB", (1600, 780), "#f3f4ee")
    draw = ImageDraw.Draw(canvas)
    for index, image in enumerate(frames):
        thumb = image.copy()
        thumb.thumbnail((390, 220), Image.Resampling.LANCZOS)
        x, y = (index % 4) * 400 + 5, (index // 4) * 260 + 5
        canvas.paste(thumb, (x + (390 - thumb.width) // 2, y))
        draw.text(
            (x + 8, y + 226),
            f"{index + 1:02d}  {index * max(0, seconds - 0.08) / 11:.2f}s",
            fill="#14201b",
        )
    return canvas


@node(
    "inspect",
    inputs={"footage": "video", "brief": "json"},
    outputs={"source": "json", "storyboard": "image/png"},
    tools=["ffmpeg", "ffprobe"],
    resources=["nodes/composition.py"],
)
def inspect(ctx: Ctx) -> dict:
    brief = validate_brief(ctx.read.json("brief"))
    source = ctx.inputs["footage"].path
    metadata = probe(ctx.tool("ffprobe").executable, source)
    if any(hint["source_second"] > metadata["seconds"] for hint in brief.get("focus_hints", [])):
        raise ctx.fail("a focus hint falls after the source recording ends")
    metadata["sha256"] = digest(source)
    metadata["sampling"] = "12 equally spaced chronological frames; audio is not sampled"
    metadata["output"] = {"width": SIZE[0], "height": SIZE[1], "fps": FPS, "audio": False}
    storyboard = sheet(
        sample_frames(ctx.tool("ffmpeg").executable, source, metadata["seconds"]),
        metadata["seconds"],
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
)
def render(ctx: Ctx) -> dict:
    brief = validate_brief(ctx.read.json("brief"))
    edit = validate_edit(ctx.read.json("edit"))
    composer = Composition(brief, edit, ctx.inputs["font"].path)
    ffmpeg, ffprobe = ctx.tool("ffmpeg").executable, ctx.tool("ffprobe").executable
    source = ctx.inputs["footage"].path
    metadata = probe(ffprobe, source)
    body_frames = round(edit["footage_seconds"] * FPS)
    # Even retiming is explicit; this utility does not guess where to remove pauses.
    source_width = min(metadata["width"], 1920)
    source_height = round(metadata["height"] * source_width / metadata["width"])
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
            "-vf",
            f"setpts={edit['footage_seconds'] / metadata['seconds']}*PTS,"
            f"fps={FPS},scale={source_width}:{source_height}",
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
