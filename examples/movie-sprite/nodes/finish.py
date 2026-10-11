"""Finish a take: key it to transparency, hold the face, close the loop, verify every frame.

The finishing is the movie sprite component's; this node hands it the clip and the
author's finishing settings and returns what it made. Nothing here is paid.
"""

from __future__ import annotations

from typing import Any

from pydantic import ValidationError

from grida.fx import Ctx, node

from nodes._movie_sprite import finish_video, validate_finish_config


@node(
    "finish",
    inputs={"video": "video", "settings": "json?"},
    outputs={
        "loop": "video/x-matroska",
        "canonical": "image/png",
        "preview": "video/mp4",
        "contact_sheet": "image/png",
        "manifest": "json",
        "report": "json",
        "frames": "file/zip?",
    },
    tools=["ffmpeg", "ffprobe"],
    version=1,
)
def finish(ctx: Ctx) -> dict[str, Any]:
    """Make the transparent loop, its exact first frame, a preview and their records."""

    settings = ctx.read.json("settings") if "settings" in ctx.inputs else {}
    try:
        validate_finish_config(settings)
        made = finish_video(ctx.inputs["video"].read_bytes(), settings)
    except (ValidationError, ValueError) as error:
        raise ctx.fail(f"the take cannot be finished: {error}") from error
    outputs = {
        "loop": ctx.out.bytes(made["video"], "video/x-matroska"),
        "canonical": ctx.out.bytes(made["canonical"], "image/png"),
        "preview": ctx.out.bytes(made["preview"], "video/mp4"),
        "contact_sheet": ctx.out.bytes(made["contact_sheet"], "image/png"),
        "manifest": ctx.out.bytes(made["manifest"], "json"),
        "report": ctx.out.bytes(made["report"], "json"),
    }
    if "frames_zip" in made:
        outputs["frames"] = ctx.out.bytes(made["frames_zip"], "file/zip")
    return outputs
