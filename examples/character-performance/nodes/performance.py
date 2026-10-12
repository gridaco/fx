"""Local preparation, complete decoding and sampled evidence for motion transfer."""

from __future__ import annotations

import hashlib
import io
import json
import math
import re
import statistics
import tempfile
from fractions import Fraction
from pathlib import Path
from typing import Any

from grida.fx import Ctx, node
from PIL import Image, ImageChops, ImageDraw, ImageOps, ImageStat


def _png(image: Image.Image) -> bytes:
    out = io.BytesIO()
    image.save(out, "PNG")
    return out.getvalue()


def _run(ctx: Ctx, tool: str, args: list[str], timeout: float = 120) -> str:
    result = ctx.tool(tool).run(args, timeout_s=timeout, check=False)
    if result.returncode:
        # Tool diagnostics contain temporary input locations; portable records do not.
        raise ctx.fail(f"{tool} could not process the supplied media")
    return result.stdout


def _facts(ctx: Ctx, path: Path, *, count: bool = False) -> dict[str, Any]:
    args = ["-v", "error", "-select_streams", "v"]
    if count:
        args.append("-count_frames")
    args += [
        "-show_entries",
        "stream=width,height,avg_frame_rate,nb_read_frames,duration:format=duration",
        "-of",
        "json",
        str(path),
    ]
    data = json.loads(_run(ctx, "ffprobe", args))
    streams = data.get("streams", [])
    if len(streams) != 1:
        raise ctx.fail("one readable video stream is required")
    stream = streams[0]
    fps = float(Fraction(stream["avg_frame_rate"]))
    duration = float(stream.get("duration", data["format"]["duration"]))
    width, height = int(stream["width"]), int(stream["height"])
    if not (0 < fps <= 60 and 0 < duration <= 180 and 0 < width <= 4096 and 0 < height <= 4096):
        raise ctx.fail("media exceeds the example's 60 fps, 180 second or 4096 pixel bounds")
    facts = {"width": width, "height": height, "fps": fps, "duration_seconds": duration}
    if count:
        facts["frames"] = int(stream["nb_read_frames"])
    return facts


def character_plate(
    source: bytes, background: str, body_fill: float = 0.6
) -> tuple[Image.Image, dict[str, Any]]:
    """Fit existing pixels into a 720 x 1280 plate; this is no model redraw."""
    if re.fullmatch(r"[a-fA-F0-9]{6}", background) is None:
        raise ValueError("background is a six-digit RGB hex value")
    with Image.open(io.BytesIO(source)) as opened:
        if opened.width * opened.height > 32_000_000:
            raise ValueError("character picture exceeds 32 million pixels")
        image = opened.convert("RGBA")
    source_size = list(image.size)
    alpha = image.getchannel("A")
    _, alpha_max = alpha.getextrema()
    if not alpha_max:
        raise ValueError("character picture is empty")
    alpha = alpha.point(lambda value: 0 if value <= 24 else round(value * 255 / alpha_max))
    image.putalpha(alpha)
    box = alpha.getbbox()
    if box is None:
        raise ValueError("character picture has no visible subject")
    image = image.crop(box)
    if not 0.4 <= body_fill <= 0.8:
        raise ValueError("body_fill must be between 0.4 and 0.8")
    scale = min(720 * body_fill / image.width, 1280 * body_fill / image.height)
    size = (max(1, round(image.width * scale)), max(1, round(image.height * scale)))
    image = image.resize(size, Image.Resampling.LANCZOS)
    offset = ((720 - size[0]) // 2, 1280 - 120 - size[1])
    plate = Image.new("RGBA", (720, 1280), f"#{background}")
    plate.alpha_composite(image, offset)
    return plate.convert("RGB"), {
        "source_size": source_size,
        "source_crop_xyxy": list(box),
        "alpha_floor": 24,
        "source_alpha_max": alpha_max,
        "alpha_max_normalized_to": 255,
        "resized_size": list(size),
        "offset_xy": list(offset),
        "plate_size": [720, 1280],
        "background_rgb_hex": background.lower(),
        "body_fill": body_fill,
        "repainted": False,
    }


@node(
    "prepare",
    inputs={"character": "image", "motion": "video"},
    params={
        "seconds": {"type": "number", "minimum": 3, "maximum": 10},
        "start": {"type": "number", "minimum": 0, "maximum": 120},
        "background": str,
        "body_fill": {"type": "number", "minimum": 0.4, "maximum": 0.8, "default": 0.6},
        "fps": {"type": "integer", "enum": [24, 30], "default": 24},
    },
    outputs={"character": "image/png", "motion": "video/mp4", "report": "json"},
    tools=["ffmpeg", "ffprobe"],
    version=1,
)
def prepare(ctx: Ctx) -> dict[str, Any]:
    """Validate, explicitly trim and silence the driving reference before spending."""
    motion = ctx.inputs["motion"]
    source = _facts(ctx, motion.path)
    if not 0.45 <= source["width"] / source["height"] <= 0.8:
        raise ctx.fail("this example takes a portrait driving reference (width/height 0.45 to 0.8)")
    seconds, start = float(ctx.params["seconds"]), float(ctx.params["start"])
    if start + seconds > source["duration_seconds"] + 1 / source["fps"]:
        raise ctx.fail("selected interval exceeds the driving reference")
    try:
        plate, report = character_plate(
            ctx.inputs["character"].read_bytes(),
            ctx.params["background"],
            float(ctx.params["body_fill"]),
        )
    except ValueError as error:
        raise ctx.fail(str(error)) from error
    destination = ctx.out.path("motion.mp4")
    _run(
        ctx,
        "ffmpeg",
        [
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-xerror",
            "-err_detect",
            "explode",
            "-ss",
            str(start),
            "-i",
            str(motion.path),
            "-t",
            str(seconds),
            "-map",
            "0:v:0",
            "-an",
            "-vf",
            f"scale=-2:1280:flags=lanczos,fps={ctx.params['fps']},setsar=1",
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            "18",
            "-threads",
            "2",
            "-pix_fmt",
            "yuv420p",
            "-map_metadata",
            "-1",
            "-movflags",
            "+faststart",
            str(destination),
        ],
    )
    prepared = _facts(ctx, destination, count=True)
    if abs(prepared["duration_seconds"] - seconds) > 1 / prepared["fps"] + 0.02:
        raise ctx.fail("prepared reference duration does not match the selected interval")
    ctx.fact("seconds", prepared["duration_seconds"])
    report.update(
        {
            "kind": "character_performance_inputs_v1",
            "character_sha256": ctx.inputs["character"].digest,
            "motion_source_sha256": motion.digest,
            "source_motion": source,
            "selected_start_seconds": start,
            "selected_duration_seconds": seconds,
            "prepared_motion": prepared,
            "reference_audio_removed": True,
            "frame_rate_conversion": "frame duplication or removal; no interpolation",
        }
    )
    return {
        "character": ctx.out.bytes(_png(plate), "image/png"),
        "motion": ctx.out.file(destination, "video/mp4"),
        "report": ctx.out.json(report),
    }


@node(
    "inspect_prepared",
    inputs={"character": "image", "motion": "video"},
    params={
        "seconds": {"type": "number", "minimum": 3, "maximum": 10},
        "fps": {"type": "integer", "enum": [24, 30], "default": 24},
    },
    outputs={"report": "json"},
    tools=["ffmpeg", "ffprobe"],
    version=1,
)
def inspect_prepared(ctx: Ctx) -> dict[str, Any]:
    """Validate externally prepared files before a paid call; expose no source copy."""
    source = _facts(ctx, ctx.inputs["motion"].path, count=True)
    if abs(source["fps"] - int(ctx.params["fps"])) > 0.01:
        raise ctx.fail("prepared motion frame rate must match the selected fps")
    if not 0.45 <= source["width"] / source["height"] <= 0.8:
        raise ctx.fail("prepared motion must be portrait (width/height 0.45 to 0.8)")
    if abs(source["duration_seconds"] - float(ctx.params["seconds"])) > max(
        0.15, 1 / source["fps"]
    ):
        raise ctx.fail("prepared motion duration differs from seconds")
    audio = json.loads(
        _run(
            ctx,
            "ffprobe",
            [
                "-v",
                "error",
                "-select_streams",
                "a",
                "-show_entries",
                "stream=index",
                "-of",
                "json",
                str(ctx.inputs["motion"].path),
            ],
        )
    )
    if audio.get("streams"):
        raise ctx.fail("prepared motion must be silent; run prepare.yaml first")
    with Image.open(ctx.inputs["character"].path) as image:
        if image.size != (720, 1280):
            raise ctx.fail("prepared character plate must be 720 x 1280; run prepare.yaml first")
        image.load()
        if image.convert("RGBA").getchannel("A").getextrema() != (255, 255):
            raise ctx.fail("prepared character plate must be fully opaque; run prepare.yaml first")
    _run(
        ctx,
        "ffmpeg",
        [
            "-v",
            "error",
            "-xerror",
            "-err_detect",
            "explode",
            "-nostdin",
            "-i",
            str(ctx.inputs["motion"].path),
            "-f",
            "null",
            "-",
        ],
    )
    return {
        "report": ctx.out.json(
            {
                "kind": "character_performance_prepared_inspection_v1",
                "character_sha256": ctx.inputs["character"].digest,
                "motion_sha256": ctx.inputs["motion"].digest,
                "motion": source,
                "full_decode": "pass",
                "audio": "absent",
            }
        )
    }


def _frame(ctx: Ctx, path: Path, time: float, name: str) -> Image.Image:
    destination = ctx.out.path(name)
    _run(
        ctx,
        "ffmpeg",
        [
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-ss",
            f"{time:.6f}",
            "-i",
            str(path),
            "-frames:v",
            "1",
            "-update",
            "1",
            str(destination),
        ],
    )
    with Image.open(destination) as opened:
        return opened.convert("RGB")


def studio_border_frame(image: Image.Image) -> dict[str, Any]:
    """Conservative edge-contact proxy, only for a uniform studio background."""
    image = image.convert("RGB")
    width, height = image.size
    if min(width, height) < 16:
        raise ValueError("studio border inspection requires at least 16 pixels per side")
    patch_size = 8
    corners = [
        image.crop(box)
        for box in (
            (0, 0, patch_size, patch_size),
            (width - patch_size, 0, width, patch_size),
            (0, height - patch_size, patch_size, height),
            (width - patch_size, height - patch_size, width, height),
        )
    ]
    medians = [ImageStat.Stat(corner).median for corner in corners]
    background = tuple(int(statistics.median(values)) for values in zip(*medians, strict=True))
    corner_delta = max(
        maximum
        for corner in corners
        for _, maximum in ImageStat.Stat(
            ImageChops.difference(corner, Image.new("RGB", corner.size, background))
        ).extrema
    )
    if corner_delta > 8:
        return {
            "status": "unknown",
            "background_rgb": list(background),
            "corner_max_channel_delta": corner_delta,
            "sides": [],
        }
    borders = {
        "left": (0, 0, 1, height),
        "right": (width - 1, 0, width, height),
        "top": (0, 0, width, 1),
        "bottom": (0, height - 1, width, height),
    }
    sides = []
    for side, box in borders.items():
        border = image.crop(box)
        delta = ImageStat.Stat(
            ImageChops.difference(border, Image.new("RGB", border.size, background))
        )
        if max(maximum for _, maximum in delta.extrema) > 20:
            sides.append(side)
    return {
        "status": "edge_contact" if sides else "clear",
        "background_rgb": list(background),
        "corner_max_channel_delta": corner_delta,
        "sides": sides,
    }


def _studio_border_scan(ctx: Ctx, path: Path, expected_frames: int) -> dict[str, Any]:
    """Decode every native frame, record exact timing, and remove only owned PNGs."""
    timestamps = json.loads(
        _run(
            ctx,
            "ffprobe",
            [
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_frames",
                "-show_entries",
                "frame=best_effort_timestamp_time",
                "-of",
                "json",
                str(path),
            ],
        )
    ).get("frames", [])
    try:
        times = [float(frame["best_effort_timestamp_time"]) for frame in timestamps]
    except (KeyError, TypeError, ValueError) as error:
        raise ctx.fail("native frame timestamps are unreadable") from error
    if len(times) != expected_frames or not all(math.isfinite(time) for time in times):
        raise ctx.fail("native frame timestamps do not match the decoded frame count")
    try:
        directory = Path(tempfile.mkdtemp(prefix="studio-border-", dir=ctx.work_path(".")))
    except OSError as error:
        raise ctx.fail("studio border scan could not create its temporary frames") from error
    contacts, unknown = [], []
    try:
        _run(
            ctx,
            "ffmpeg",
            [
                "-v",
                "error",
                "-nostdin",
                "-xerror",
                "-err_detect",
                "explode",
                "-i",
                str(path),
                "-map",
                "0:v:0",
                "-an",
                "-fps_mode",
                "passthrough",
                "-threads",
                "2",
                str(directory / "%06d.png"),
            ],
        )
        frames = sorted(directory.glob("*.png"))
        if len(frames) != expected_frames:
            raise ctx.fail("studio border scan did not decode every native frame")
        for index, (frame, time) in enumerate(zip(frames, times, strict=True)):
            with Image.open(frame) as image:
                inspected = studio_border_frame(image)
            item = {"frame_index": index, "time_seconds": time}
            if inspected["status"] == "unknown":
                unknown.append(
                    {**item, "corner_max_channel_delta": inspected["corner_max_channel_delta"]}
                )
            elif inspected["status"] == "edge_contact":
                contacts.append({**item, "sides": inspected["sides"]})
    except (OSError, ValueError) as error:
        raise ctx.fail("studio border scan could not inspect the decoded native frames") from error
    finally:
        try:
            # This fresh private directory is owned only by this one decode operation.
            for frame in directory.glob("[0-9][0-9][0-9][0-9][0-9][0-9].png"):
                frame.unlink()
            directory.rmdir()
        except OSError as error:
            raise ctx.fail("studio border scan could not remove its temporary frames") from error
    return {
        "kind": "studio_border_scan_v1",
        "status": "unknown" if unknown else "edge_contact" if contacts else "clear",
        "scope": (
            "all native frames; conservative plain-studio pixel proxy, not semantic certification"
        ),
        "frames_scanned": expected_frames,
        "frame_index_base": 0,
        "timing": "native best_effort_timestamp_time",
        "corner_patch_pixels": 8,
        "corner_uniformity_max_channel_threshold": 8,
        "foreground_edge_max_channel_threshold": 20,
        "edge_contacts": contacts,
        "unknown_frames": unknown,
        "limitation": (
            "Uniform corners and clear borders cannot establish anatomy "
            "or every-transition quality."
        ),
    }


@node(
    "finish",
    inputs={"video": "video", "motion": "video", "character": "image"},
    params={"seconds": {"type": "number", "minimum": 3, "maximum": 10}, "name": str},
    outputs={
        "video": "video/mp4",
        "poster": "image/webp",
        "evidence": "image/png",
        "identity": "image/png",
        "report": "json",
    },
    tools=["ffmpeg", "ffprobe"],
    version=2,
)
def finish(ctx: Ctx) -> dict[str, Any]:
    """Keep native video bytes where possible, strip audio, then decode all frames."""
    source = ctx.inputs["video"]
    seconds = float(ctx.params["seconds"])
    native = _facts(ctx, source.path, count=True)
    if abs(native["duration_seconds"] - seconds) > max(0.15, 2 / native["fps"]):
        raise ctx.fail("generated duration does not follow the prepared reference")
    destination = ctx.out.path("performance.mp4")
    _run(
        ctx,
        "ffmpeg",
        [
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-i",
            str(source.path),
            "-map",
            "0:v:0",
            "-an",
            "-c:v",
            "copy",
            "-map_metadata",
            "-1",
            "-movflags",
            "+faststart",
            str(destination),
        ],
    )
    finished = _facts(ctx, destination, count=True)
    framing_scan = _studio_border_scan(ctx, destination, finished["frames"])
    motion = _facts(ctx, ctx.inputs["motion"].path)
    duration = min(finished["duration_seconds"], motion["duration_seconds"])
    times = [round(index * (duration - max(0.1, 1 / finished["fps"])) / 7, 6) for index in range(8)]
    samples = [_frame(ctx, destination, t, f"result-{i}.png") for i, t in enumerate(times)]
    drivers = [
        _frame(ctx, ctx.inputs["motion"].path, t, f"motion-{i}.png") for i, t in enumerate(times)
    ]
    poster_buffer = io.BytesIO()
    samples[0].save(poster_buffer, "WEBP", quality=92, method=6)
    identity = Image.new("RGB", (1024, 808), "#f6f5f3")
    with Image.open(ctx.inputs["character"].path) as opened:
        identity.paste(ImageOps.contain(opened.convert("RGB"), (512, 768)), (0, 40))
    identity.paste(ImageOps.contain(samples[0], (512, 768)), (512, 40))
    draw = ImageDraw.Draw(identity)
    draw.text((16, 12), "AUTHORITATIVE CHARACTER", fill="#101010")
    draw.text((528, 12), "GENERATED OPENING FRAME", fill="#101010")
    # Every column is a synchronized pair: driving performance above generated result.
    cell_w, cell_h, header = 270, 480, 32
    evidence = Image.new("RGB", (cell_w * 4, (cell_h * 2 + header) * 2), "#f6f5f3")
    draw = ImageDraw.Draw(evidence)
    for index, (driver, result, time) in enumerate(zip(drivers, samples, times, strict=True)):
        x, y = (index % 4) * cell_w, (index // 4) * (cell_h * 2 + header)
        draw.text((x + 8, y + 10), f"{time:.2f}s: MOTION / RESULT", fill="#101010")
        for row, image in enumerate((driver, result)):
            fitted = ImageOps.contain(image, (cell_w, cell_h))
            evidence.paste(
                fitted,
                (
                    x + (cell_w - fitted.width) // 2,
                    y + header + row * cell_h + (cell_h - fitted.height) // 2,
                ),
            )
    # A rough change statistic detects trivial copies; it is not an artistic verdict.
    a = samples[0].resize((64, 64)).convert("RGB")
    differences = []
    for sample in samples[1:]:
        b = sample.resize((64, 64)).convert("RGB")
        differences.append(
            sum(
                abs(x - y)
                for p, q in zip(a.getdata(), b.getdata(), strict=True)
                for x, y in zip(p, q, strict=True)
            )
            / (64 * 64 * 3)
        )
    if max(differences) < 0.5:
        raise ctx.fail("generated take is effectively static in the sampled frames")
    report = {
        "kind": "character_performance_finish_v1",
        "character": ctx.params["name"],
        "character_plate_sha256": ctx.inputs["character"].digest,
        "motion_sha256": ctx.inputs["motion"].digest,
        "take_sha256": source.digest,
        "video_sha256": hashlib.sha256(destination.read_bytes()).hexdigest(),
        "native": native,
        "finished": finished,
        "full_decode": "pass",
        "framing_scan": framing_scan,
        "audio": "removed",
        "sample_times_seconds": times,
        "sample_mean_rgb_change": differences,
        "review_status": "pending",
        "sampling_limit": "Eight frame pairs cannot certify every transition.",
        "temporal_interpolation": False,
        "spatial_resizing_of_output": False,
    }
    return {
        "video": ctx.out.file(destination, "video/mp4"),
        "poster": ctx.out.bytes(poster_buffer.getvalue(), "image/webp"),
        "evidence": ctx.out.bytes(_png(evidence), "image/png"),
        "identity": ctx.out.bytes(_png(identity), "image/png"),
        "report": ctx.out.json(report),
    }


@node(
    "admit",
    inputs={"review": "json", "report": "json"},
    outputs={"report": "json"},
    judge=True,
    version=2,
)
def admit(ctx: Ctx) -> dict[str, Any]:
    """A failed review stays a failed take; no hidden semantic regeneration."""
    review, report = ctx.read.json("review"), ctx.read.json("report")
    keys = ("status", "identity", "motion_correspondence", "anatomy_and_framing", "style")
    sampled_pass = all(review.get(key) == "pass" for key in keys)
    framing = report.get("framing_scan", {})
    framing_pass = framing.get("status") == "clear" and framing.get("frames_scanned") == report.get(
        "finished", {}
    ).get("frames")
    accepted = sampled_pass and framing_pass
    report["sampled_review_status"] = "pass" if sampled_pass else "fail"
    report["review_status"] = (
        "framing_guard_fail"
        if not framing_pass
        else "sampled_review_pass"
        if sampled_pass
        else "sampled_review_fail"
    )
    report["rejection_reasons"] = ([] if sampled_pass else ["sampled_review_failed"]) + (
        [] if framing_pass else ["studio_border_not_clear_or_incomplete"]
    )
    report["review_sha256"] = ctx.inputs["review"].digest
    report["review_scope"] = review.get("review_scope")
    report["publication_status"] = "requires_separate_content_review"
    ctx.fact("verdict", "accept" if accepted else "reject")
    return {"report": ctx.out.json(report)}
