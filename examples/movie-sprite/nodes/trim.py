"""Select a source-bound frame interval from opaque RGB/chroma footage."""

from __future__ import annotations

import hashlib
import json
import re
from fractions import Fraction
from typing import Any

from grida.fx import Ctx, node


@node(
    "trim",
    inputs={"video": "video", "settings": "json"},
    outputs={"video": "video/x-matroska", "report": "json"},
    tools=["ffmpeg", "ffprobe"],
    version=2,
)
def trim(ctx: Ctx) -> dict[str, Any]:
    """Keep opaque [start_frame, end_frame); no interpolation or provider call."""
    settings = ctx.read.json("settings")
    if not isinstance(settings, dict) or set(settings) != {
        "source_sha256",
        "start_frame",
        "end_frame",
    }:
        raise ctx.fail("trim settings require source_sha256, start_frame and end_frame")
    digest = settings["source_sha256"]
    if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
        raise ctx.fail("trim source_sha256 must be a SHA-256 digest")
    source = ctx.inputs["video"]
    if hashlib.sha256(source.read_bytes()).hexdigest() != digest:
        raise ctx.fail("trim settings do not match the supplied video")
    start, end = settings["start_frame"], settings["end_frame"]
    if type(start) is not int or type(end) is not int or not 0 <= start < end:
        raise ctx.fail("trim requires integer frames with 0 <= start_frame < end_frame")
    probe = json.loads(
        ctx.tool("ffprobe")
        .run(
            [
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-count_frames",
                "-show_entries",
                "stream=width,height,avg_frame_rate,nb_read_frames",
                "-of",
                "json",
                str(source.path),
            ],
            timeout_s=120,
        )
        .stdout
    )
    streams = probe.get("streams", [])
    if len(streams) != 1:
        raise ctx.fail("trim requires one decodable video stream")
    facts = streams[0]
    count = int(facts["nb_read_frames"])
    rate = Fraction(facts["avg_frame_rate"])
    if rate <= 0 or rate > 60 or end > count:
        raise ctx.fail("trim interval exceeds the video or its frame rate is unsupported")
    output = ctx.out.path("trimmed.mkv")
    ctx.tool("ffmpeg").run(
        [
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-i",
            str(source.path),
            "-map",
            "0:v:0",
            "-vf",
            f"trim=start_frame={start}:end_frame={end},setpts=PTS-STARTPTS",
            "-an",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-pix_fmt",
            "bgr0",
            "-map_metadata",
            "-1",
            "-fflags",
            "+bitexact",
            "-flags:v",
            "+bitexact",
            str(output),
        ],
        timeout_s=180,
    )
    report = {
        "kind": "movie_sprite_frame_interval_v1",
        "source_sha256": digest,
        "source_frames": count,
        "start_frame": start,
        "end_frame_exclusive": end,
        "retained_frames": end - start,
        "source_frame_rate": str(rate),
        "retained_seconds": float(Fraction(end - start, 1) / rate),
        "output_sha256": hashlib.sha256(output.read_bytes()).hexdigest(),
        "width": facts["width"],
        "height": facts["height"],
        "codec": "ffv1",
        "container_bitexact": True,
        "audio": False,
        "interpolation": False,
        "reason": "Explicit author-selected interval; semantic review remains separate.",
    }
    return {"video": ctx.out.file(output, "video/x-matroska"), "report": ctx.out.json(report)}
