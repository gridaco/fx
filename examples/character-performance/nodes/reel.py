"""Free composition of reviewed performances with explicit caller-selected framing."""

from __future__ import annotations

import hashlib
import io
import json
import math
import re
from fractions import Fraction
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

from grida.fx import Ctx, node

SIZE = (1080, 1920)
FPS = 30
WINDOW = (72, 230, 936, 1400)
BACKGROUND = "#f4eee6"
INK = "#24232b"
FULL_REGION = [0, 0, 1, 1]
REGION = {
    "type": "array",
    "items": {"type": "number", "minimum": 0, "maximum": 1},
    "minItems": 4,
    "maxItems": 4,
    "default": FULL_REGION,
}


def _run(ctx: Ctx, tool: str, args: list[str], timeout: int = 120) -> str:
    result = ctx.tool(tool).run(args, timeout_s=timeout, check=False)
    if result.returncode:
        raise ctx.fail(f"{tool} refused the supplied reel media")
    return result.stdout


def _probe(ctx: Ctx, path: Path) -> dict:
    data = json.loads(
        _run(
            ctx,
            "ffprobe",
            [
                "-v",
                "error",
                "-count_frames",
                "-show_streams",
                "-show_format",
                "-of",
                "json",
                str(path),
            ],
        )
    )
    videos = [item for item in data.get("streams", []) if item.get("codec_type") == "video"]
    if len(videos) != 1 or any(item.get("codec_type") == "audio" for item in data["streams"]):
        raise ctx.fail("supply exactly one silent video stream per performance")
    stream = videos[0]
    facts = {
        "width": int(stream["width"]),
        "height": int(stream["height"]),
        "fps": float(Fraction(stream["avg_frame_rate"])),
        "duration_seconds": float(stream.get("duration", data["format"]["duration"])),
        "frames": int(stream["nb_read_frames"]),
    }
    if not all(math.isfinite(value) and value > 0 for value in facts.values()):
        raise ctx.fail("the supplied video has invalid dimensions, frames or timing")
    return facts


def _decode(ctx: Ctx, path: Path) -> None:
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
            "-f",
            "null",
            "-",
        ],
    )


def _name(value: str) -> str:
    if not isinstance(value, str) or not 1 <= len(value) <= 40 or any(ord(c) < 32 for c in value):
        raise ValueError("character labels need 1-40 printable characters")
    return value


def _region(value: list) -> list:
    if (
        not isinstance(value, list)
        or len(value) != 4
        or any(
            isinstance(number, bool)
            or not isinstance(number, (int, float))
            or not math.isfinite(number)
            for number in value
        )
    ):
        raise ValueError("a source region needs four finite numbers: x, y, width, height")
    x, y, width, height = value
    if not (0 <= x < 1 and 0 <= y < 1 and width > 0 and height > 0):
        raise ValueError("a source region must be nonempty and inside the source frame")
    if x + width > 1 or y + height > 1:
        raise ValueError("a source region must be nonempty and inside the source frame")
    return value


def _crop(region: list, width: int, height: int) -> list:
    """Round outwards so all requested source pixels survive the explicit crop."""
    x, y, w, h = region
    left, top = math.floor(x * width), math.floor(y * height)
    right, bottom = min(width, math.ceil((x + w) * width)), min(height, math.ceil((y + h) * height))
    if right - left < 2 or bottom - top < 2:
        raise ValueError("a source region must cover at least two pixels per side")
    return [left, top, right - left, bottom - top]


def _poster_frame(value: float, frames: int) -> int:
    if (
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not math.isfinite(value)
        or not 0 <= value < frames / FPS
    ):
        raise ValueError("poster_at must name a finite timestamp inside the finished reel")
    return min(math.floor(value * FPS), frames - 1)


def _switch_frame(value: float, frames: int) -> int:
    if (
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not math.isfinite(value)
        or not 0.5 <= value <= frames / FPS - 0.5
    ):
        raise ValueError("switch_at must leave at least half a second for each character")
    return math.floor(value * FPS)


def _font(path: Path, size: int, weight: int = 650) -> ImageFont.FreeTypeFont:
    font = ImageFont.truetype(str(path), size)
    if font.get_variation_axes():
        font.set_variation_by_axes([weight])
    return font


def _card(font: Path, name: str, index: int) -> Image.Image:
    image = Image.new("RGB", SIZE, BACKGROUND)
    draw = ImageDraw.Draw(image)
    draw.text((72, 64), "CAST / 01 + 02", font=_font(font, 25), fill=INK)
    draw.text((72, 110), "Meet the cast.", font=_font(font, 82, 700), fill=INK)
    size = 84
    while _font(font, size).getlength(name) > 936 and size > 24:
        size -= 2
    if _font(font, size).getlength(name) > 936:
        raise ValueError("character label does not fit the reel composition")
    draw.text((72, 1688), name, font=_font(font, size, 750), fill=INK)
    draw.text(
        (72, 1804),
        "Two perspectives. One shared stage.",
        font=_font(font, 25, 500),
        fill=INK,
    )
    draw.line((72, 1860, 1008, 1860), fill="#c7beb3", width=2)
    draw.line((72, 1860, 72 + 468 * index, 1860), fill="#6f7061", width=5)
    draw.text((72, 1880), f"0{index} / CHARACTER PERFORMANCE", font=_font(font, 20), fill=INK)
    return image


@node(
    "reel",
    inputs={
        "first": "video",
        "first_report": "json",
        "second": "video",
        "second_report": "json",
        "font": "file",
    },
    params={
        "first_name": str,
        "second_name": str,
        "first_region": REGION,
        "second_region": REGION,
        "poster_at": {"type": "number", "minimum": 0, "default": 1},
        "mode": {"type": "string", "enum": ["sequence", "shared_timeline"], "default": "sequence"},
        "switch_at": {"type": "number", "minimum": 0.5, "default": 2},
    },
    outputs={"video": "video/mp4", "poster": "image/webp", "report": "json"},
    tools=["ffmpeg", "ffprobe"],
    version=5,
)
def reel(ctx: Ctx) -> dict:
    """Review admission is inherited; composition still needs separate visual acceptance."""
    sources = []
    # Admit both exact lineages before probing, decoding, or encoding either segment.
    for key in ("first", "second"):
        report = ctx.read.json(f"{key}_report")
        if report.get("review_status") != "sampled_review_pass":
            raise ctx.fail("both source reports must have sampled_review_pass")
        if report.get("video_sha256") != ctx.inputs[key].digest:
            raise ctx.fail("a reviewed report does not match its supplied video digest")
        if report.get("full_decode") != "pass" or report.get("audio") not in ("removed", "silent"):
            raise ctx.fail("each source report must record full decoding and silent delivery")
        scan = report.get("framing_scan")
        if not isinstance(scan, dict) or scan.get("status") != "clear":
            raise ctx.fail("each source report must record a clear full-frame framing scan")
        if not isinstance(report.get("review_sha256"), str) or not re.fullmatch(
            r"[0-9a-f]{64}", report["review_sha256"]
        ):
            raise ctx.fail("each source report must identify its sampled review digest")
        sources.append(
            {
                "key": key,
                "name": _name(ctx.params[f"{key}_name"]),
                "region": _region(ctx.params[f"{key}_region"]),
                "review": report,
            }
        )
    font = ctx.inputs["font"].path
    for item in sources:
        path = ctx.inputs[item["key"]].path
        facts = _probe(ctx, path)
        if item["review"]["framing_scan"].get("frames_scanned") != facts["frames"]:
            raise ctx.fail("a source framing scan must cover every native video frame")
        if not (
            facts["width"] < facts["height"] <= 4096
            and facts["width"] <= 4096
            and 1 <= facts["fps"] <= 60
            and 3 <= facts["duration_seconds"] <= 10.1
        ):
            raise ctx.fail(
                "each source must be a portrait performance of 3-10 seconds at at most 60 fps"
            )
        _decode(ctx, path)
        item["source"] = facts
        item["frames"] = round(facts["duration_seconds"] * FPS)
        item["start_frame"] = 0
        item["end_frame"] = item["frames"]
    mode = ctx.params["mode"]
    if mode == "shared_timeline":
        motion = sources[0]["review"].get("motion_sha256")
        if (
            not isinstance(motion, str)
            or not re.fullmatch(r"[0-9a-f]{64}", motion)
            or sources[1]["review"].get("motion_sha256") != motion
        ):
            raise ctx.fail("shared_timeline requires two reports bound to the same driving video")
        if (
            sources[0]["frames"] != sources[1]["frames"]
            or abs(
                sources[0]["source"]["duration_seconds"] - sources[1]["source"]["duration_seconds"]
            )
            > 1 / FPS
        ):
            raise ctx.fail(
                "shared_timeline requires matching source durations on the output frame grid"
            )
        switch_frame = _switch_frame(ctx.params["switch_at"], sources[0]["frames"])
        sources[0]["end_frame"] = switch_frame
        sources[1]["start_frame"] = switch_frame
        for item in sources:
            item["frames"] = item["end_frame"] - item["start_frame"]
    frames = sum(item["frames"] for item in sources)
    poster_frame = _poster_frame(ctx.params["poster_at"], frames)
    segments = []
    for index, item in enumerate(sources, 1):
        if ctx.cancelled:
            raise ctx.fail("cast reel composition cancelled")
        key, path = item["key"], ctx.inputs[item["key"]].path
        facts, frame_count = item["source"], item["frames"]
        crop = _crop(item["region"], facts["width"], facts["height"])
        scale = min(WINDOW[2] / crop[2], WINDOW[3] / crop[3])
        fitted = [math.floor(side * scale / 2) * 2 for side in crop[2:]]
        if min(fitted) < 2:
            raise ctx.fail("a source region is too narrow to aspect-fit the reel window")
        cropping = (
            ""
            if item["region"] == FULL_REGION
            else (f"crop={crop[2]}:{crop[3]}:{crop[0]}:{crop[1]}:exact=1,")
        )
        selection = (
            f"trim=start_frame={item['start_frame']}:end_frame={item['end_frame']},setpts=PTS-STARTPTS,"
            if mode == "shared_timeline"
            else ""
        )
        card = ctx.work_path(f"reel/card-{index}.png")
        _card(font, item["name"], index).save(card, "PNG")
        segment = ctx.work_path(f"reel/segment-{index}.mp4")
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
                "-y",
                "-i",
                str(path),
                "-loop",
                "1",
                "-framerate",
                str(FPS),
                "-i",
                str(card),
                "-filter_complex_threads",
                "1",
                "-filter_complex",
                f"[0:v]{cropping}fps=30,{selection}scale={fitted[0]}:{fitted[1]},setsar=1[v];"
                "[1:v][v]overlay=x=(W-w)/2:y=230+(1400-h)/2:shortest=1[out]",
                "-map",
                "[out]",
                "-frames:v",
                str(frame_count),
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
                "-map_metadata",
                "-1",
                "-movflags",
                "+faststart",
                str(segment),
            ],
        )
        item.update(
            {
                "source": facts,
                "frames": frame_count,
                "duration_seconds": frame_count / FPS,
                "fit_size": fitted,
                "crop": crop,
                "video_sha256": ctx.inputs[key].digest,
                "report_sha256": ctx.inputs[f"{key}_report"].digest,
            }
        )
        segments.append(segment)
    destination = ctx.out.path("cast-reel.mp4")
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
            "-y",
            "-i",
            str(segments[0]),
            "-i",
            str(segments[1]),
            "-filter_complex_threads",
            "1",
            "-filter_complex",
            "[0:v][1:v]concat=n=2:v=1:a=0[out]",
            "-map",
            "[out]",
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
            "-map_metadata",
            "-1",
            "-movflags",
            "+faststart",
            str(destination),
        ],
    )
    _decode(ctx, destination)
    finished = _probe(ctx, destination)
    if (
        (finished["width"], finished["height"]) != SIZE
        or finished["fps"] != FPS
        or finished["frames"] != frames
    ):
        raise ctx.fail(
            "the finished reel differs from the required dimensions, frame rate or frame count"
        )
    if abs(finished["duration_seconds"] - frames / FPS) > 1 / FPS:
        raise ctx.fail("the finished reel duration differs from its segments")
    poster_path = ctx.work_path("reel/poster.png")
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
            "-y",
            "-i",
            str(destination),
            "-vf",
            f"select=eq(n\\,{poster_frame})",
            "-frames:v",
            "1",
            str(poster_path),
        ],
    )
    poster = io.BytesIO()
    with Image.open(poster_path) as image:
        image.save(poster, "WEBP", quality=92, method=6)
    report = {
        "kind": "character_cast_reel_v1",
        "finished": finished,
        "video_sha256": hashlib.sha256(destination.read_bytes()).hexdigest(),
        "font_sha256": ctx.inputs["font"].digest,
        "poster": {
            "requested_seconds": ctx.params["poster_at"],
            "frame_index": poster_frame,
            "at_seconds": poster_frame / FPS,
            "sha256": hashlib.sha256(poster.getvalue()).hexdigest(),
            "selection": "caller-selected timestamp, rounded down to the output frame grid",
        },
        "composition_mode": mode,
        "segments": [
            {
                "name": item["name"],
                "source": item["source"],
                "video_sha256": item["video_sha256"],
                "report_sha256": item["report_sha256"],
                "review_sha256": item["review"]["review_sha256"],
                "source_review_status": "sampled_review_pass",
                "source_framing_scan": {
                    "status": "clear",
                    "frames_scanned": item["source"]["frames"],
                },
                "frames": item["frames"],
                "duration_seconds": item["duration_seconds"],
                "source_interval_on_output_grid": {
                    "start_frame": item["start_frame"],
                    "end_frame_exclusive": item["end_frame"],
                    "start_seconds": item["start_frame"] / FPS,
                    "end_seconds": item["end_frame"] / FPS,
                },
                "fit_size": item["fit_size"],
                "source_region_xywh": item["region"],
                "source_crop_xywh": item["crop"],
                "complete_source_frame": item["crop"]
                == [0, 0, item["source"]["width"], item["source"]["height"]],
                "frame_rate_conversion": item["source"]["fps"] != FPS,
            }
            for item in sources
        ],
        "fit_window_xywh": list(WINDOW),
        "framing": (
            "complete source frame, aspect fit, no crop"
            if all(item["region"] == FULL_REGION for item in sources)
            else "caller-selected static source regions, rounded outwards and aspect fit"
        ),
        "spatial_resizing": "uniform aspect fit of each selected region, rounded to even pixels",
        "automatic_subject_detection": False,
        "region_selection": (
            "caller authored; complete-character visibility requires content review"
        ),
        "audio": "silent",
        "full_decode": "pass",
        "temporal_interpolation": False,
        "review_status": "requires_separate_content_review",
        "review_scope": (
            "Source sampled reviews are inherited as admission evidence; this local composition "
            "makes no artistic or continuous-anatomy claim."
        ),
    }
    if mode == "shared_timeline":
        report["shared_timeline"] = {
            "motion_sha256": motion,
            "requested_switch_seconds": ctx.params["switch_at"],
            "switch_frame": switch_frame,
            "switch_seconds": switch_frame / FPS,
            "scope": (
                "Editorial identity cut on a shared driving timeline; "
                "pose continuity requires content review."
            ),
        }
    ctx.fact("seconds", finished["duration_seconds"])
    ctx.fact("full_decode", "pass")
    return {
        "video": ctx.out.file(destination, "video/mp4"),
        "poster": ctx.out.bytes(poster.getvalue(), "image/webp"),
        "report": ctx.out.json(report),
    }
