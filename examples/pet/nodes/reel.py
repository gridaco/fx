"""A finished pet acting out its states as a short looping video, rendered from its sheet and
sprite.json with Pillow and FFmpeg: no drawing, nothing to pay."""

from __future__ import annotations

import io
import math
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageDraw

from grida.fx import Ctx, node

SIZE = (960, 600)  # the video, 8:5
FPS = 30
SCALE = 2.0  # one sheet cell, enlarged
GROUND = 524  # where the bottom of a cell stands
BACKDROP = (244, 241, 234)  # the previews' backdrop
SHADOW = (60, 52, 44, 30)
SPEED = 240  # pixels a second while running
DEFAULT_MS = 140  # a frame's time when the layout gives none
ENCODE_SECONDS = 120


@dataclass
class Part:
    state: str
    start: float  # where it stands or starts running, as a share of the width
    end: float
    ms: int


def _durations(row: dict) -> list[int]:
    durations = row.get("durations_ms") or []
    return durations if len(durations) == row["frames"] else [DEFAULT_MS] * row["frames"]


def _cycle(row: dict) -> int:
    return sum(_durations(row))


def choreography(rows: dict[str, dict]) -> list[Part]:
    """With the nine states of the pet sheet, the pet idles, waves, runs right, looks things over,
    runs left, waits and runs back to where it began, so the video loops. A jump is left out: its
    row is drawn smaller, to fit the jump in a cell. Other states each play in place, about a
    second apiece."""
    if {"idle", "waving", "running-right", "running-left", "review", "waiting"} <= rows.keys():
        centre, right, left = 0.5, 0.74, 0.26

        def run(state: str, start: float, end: float) -> Part:
            return Part(state, start, end, round(abs(end - start) * SIZE[0] / SPEED * 1000))

        def stay(state: str, at: float, cycles: int) -> Part:
            return Part(state, at, at, cycles * _cycle(rows[state]))

        return [
            stay("idle", centre, 1),
            stay("waving", centre, 2),
            run("running-right", centre, right),
            stay("review", right, 1),
            run("running-left", right, left),
            stay("waiting", left, 1),
            run("running-right", left, centre),
        ]
    return [
        Part(state, 0.5, 0.5, max(1, math.ceil(900 / _cycle(row))) * _cycle(row))
        for state, row in rows.items()
    ]


def _frame(row: dict, ms: int) -> int:
    elapsed = ms % _cycle(row)
    for index, duration in enumerate(_durations(row)):
        if elapsed < duration:
            return index
        elapsed -= duration
    return 0


class Reel:
    def __init__(self, sheet: Image.Image, layout: dict):
        self.cell = (layout["cell"]["width"], layout["cell"]["height"])
        self.rows = {row["state"]: row for row in layout["rows"]}
        self.parts = choreography(self.rows)
        self.ms = sum(part.ms for part in self.parts)
        self.frames = round(self.ms * FPS / 1000)
        width, height = self.cell
        size = (round(width * SCALE), round(height * SCALE))
        self.cells: dict[tuple[str, int], tuple[Image.Image, int]] = {}
        for state, row in self.rows.items():
            for frame in range(row["frames"]):
                box = (
                    frame * width,
                    row["row"] * height,
                    (frame + 1) * width,
                    (row["row"] + 1) * height,
                )
                cell = sheet.crop(box).resize(size, Image.Resampling.LANCZOS)
                bottom = (cell.getchannel("A").getbbox() or (0, 0, 0, size[1]))[3]
                self.cells[state, frame] = (cell, size[1] - bottom)

    def picture(self, index: int) -> Image.Image:
        ms = index * 1000 / FPS
        for part in self.parts:
            if ms < part.ms:
                break
            ms -= part.ms
        cell, gap = self.cells[part.state, _frame(self.rows[part.state], int(ms))]
        x = (part.start + (part.end - part.start) * min(ms / part.ms, 1)) * SIZE[0]
        canvas = Image.new("RGBA", SIZE, BACKDROP + (255,))
        # A soft shadow on the ground, smaller while the pet is in the air.
        lift = max(0, gap - round(8 * SCALE))
        spread = max(0.45, 1 - lift / (cell.height * 0.5)) * cell.width * 0.32
        shadow = Image.new("RGBA", SIZE, (0, 0, 0, 0))
        ImageDraw.Draw(shadow).ellipse(
            (x - spread, GROUND - 14 - spread * 0.12, x + spread, GROUND - 14 + spread * 0.12),
            SHADOW,
        )
        canvas.alpha_composite(shadow)
        canvas.alpha_composite(cell, (round(x - cell.width / 2), GROUND - cell.height))
        return canvas.convert("RGB")


def encode(reel: Reel, ffmpeg: Path, destination: Path, ctx: Ctx) -> None:
    with tempfile.TemporaryDirectory(prefix="pet-reel-") as folder:
        for index in range(reel.frames):
            if ctx.cancelled:
                raise ctx.fail("the reel was cancelled")
            reel.picture(index).save(Path(folder) / f"{index:05d}.png", compress_level=1)
            ctx.progress("Drawing the reel", (index + 1) / reel.frames)
        command = [
            str(ffmpeg), "-hide_banner", "-loglevel", "error", "-nostdin", "-y",
            "-framerate", str(FPS), "-i", str(Path(folder) / "%05d.png"),
            "-an", "-c:v", "libx264", "-preset", "medium", "-crf", "20", "-threads", "2",
            "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(destination),
        ]  # fmt: skip
        try:
            done = subprocess.run(command, capture_output=True, timeout=ENCODE_SECONDS, check=False)
        except subprocess.TimeoutExpired:
            raise ctx.fail(f"FFmpeg took longer than {ENCODE_SECONDS} seconds") from None
        if done.returncode or not destination.is_file():
            detail = done.stderr.decode("utf-8", errors="replace").strip()[-800:]
            raise ctx.fail(
                f"FFmpeg could not encode the reel: {detail.replace(folder, '<frames>')}"
            )


@node(
    "reel",
    inputs={"sheet": "image", "layout": "json"},
    outputs={"video": "video/mp4", "poster": "image/webp"},
    tools=["ffmpeg"],
)
def reel(ctx: Ctx) -> dict:
    """The pet acting out its states as an MP4 that loops, and its first frame as a poster."""
    ffmpeg = ctx.tool("ffmpeg").executable
    if ffmpeg is None:
        raise ctx.fail("the reel needs FFmpeg on PATH")
    layout = ctx.read.json("layout")
    reel = Reel(ctx.read.image("sheet").convert("RGBA"), layout)
    ctx.fact("seconds", round(reel.ms / 1000, 2))
    ctx.fact("states", [part.state for part in reel.parts])
    destination = ctx.out.path("reel.mp4")
    encode(reel, Path(ffmpeg), destination, ctx)
    poster = io.BytesIO()
    reel.picture(0).save(poster, "WEBP", quality=90, method=6)
    return {
        "video": ctx.out.file(destination, "video/mp4"),
        "poster": ctx.out.bytes(poster.getvalue(), "image/webp"),
    }
