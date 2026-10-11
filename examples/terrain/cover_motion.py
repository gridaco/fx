"""Render a real, evolving 47-mask terrain grid without a browser or provider."""

from __future__ import annotations

import contextlib
import os
import signal
import subprocess
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, replace
from pathlib import Path

from cover import BACKGROUND, CANVAS, CELL_PX, DIGIT_ROWS, ORIGIN, ROWS, render_cover_layers
from PIL import Image
from terrain_example.atlas import (
    Coordinate,
    Occupancy,
    TerrainAtlasLookup,
    cells_from_canonical_atlas,
    compose_terrain,
    load_terrain_atlas_lookup,
    parse_binary_rows,
)

from grida.fx import Ctx, node

WIDTH = 960
HEIGHT = 600
FPS = 20
FRAME_COUNT = 160
REMOVE_START = FPS
REMOVE_END = 3 * FPS
BUILD_START = 68
BUILD_END = 108
ENCODE_TIMEOUT_SECONDS = 180.0
STDERR_LIMIT = 8192


@dataclass(frozen=True)
class Motion:
    background: Image.Image
    base: Image.Image
    assembled: Image.Image
    cells: dict[Coordinate, Image.Image]
    lookup: TerrainAtlasLookup
    origin: tuple[int, int]
    map_size: tuple[int, int]
    baseline_y: int


class TerrainCoverState:
    """One persistent binary grid; every edit changes its current neighborhood topology."""

    def __init__(self) -> None:
        self._rows = [list(row) for row in parse_binary_rows(ROWS)]
        self._remove = tuple(
            (x, y)
            for y, row in enumerate(self._rows[:DIGIT_ROWS])
            for x, occupied in enumerate(row)
            if occupied
        )
        self._build = tuple(sorted(self._remove, key=lambda cell: (-cell[1], cell[0])))
        self._removed = 0
        self._built = 0
        self.frame_index = 0

    @property
    def rows(self) -> Occupancy:
        """A snapshot prevents callers from changing the authoritative mutable grid."""
        return tuple(tuple(row) for row in self._rows)

    def advance_to(self, frame_index: int) -> bool:
        """Replay pending cell edits in order, retaining all other cells unchanged."""
        if type(frame_index) is not int or not self.frame_index <= frame_index <= FRAME_COUNT:
            raise ValueError("terrain simulation frames must advance from zero through 160")
        count = len(self._remove)
        removed = min(
            count, max(0, (frame_index - REMOVE_START) * count // (REMOVE_END - REMOVE_START))
        )
        built = min(count, max(0, (frame_index - BUILD_START) * count // (BUILD_END - BUILD_START)))
        changed = removed != self._removed or built != self._built
        for x, y in self._remove[self._removed : removed]:
            self._rows[y][x] = False
        for x, y in self._build[self._built : built]:
            self._rows[y][x] = True
        self._removed, self._built = removed, built
        self.frame_index = frame_index
        return changed


def render_state(motion: Motion, occupied: Occupancy) -> Image.Image:
    """Select all tiles from this frame's actual occupancy, including ground neighbors."""
    terrain, _ = compose_terrain(occupied, motion.cells, motion.lookup)
    terrain = terrain.convert("RGBa").resize(motion.map_size, Image.Resampling.LANCZOS)
    frame = motion.background.convert("RGBA")
    frame.alpha_composite(terrain.convert("RGBA"), motion.origin)
    return frame.convert("RGB")


def prepare_motion(atlas: bytes, *, width: int = WIDTH) -> Motion:
    """Prepare the atlas and fixed background; no finished-image reveal masks are used."""
    if type(width) is not int or not 160 <= width <= WIDTH or width % 16:
        raise ValueError("motion width must be a multiple of sixteen from 160 to 960")
    _, _, layout = render_cover_layers(atlas)  # Validate the same source-owned cover geometry.
    scale = width / CANVAS[0]
    size = (width, round(CANVAS[1] * scale))
    cells, lookup = cells_from_canonical_atlas(atlas), load_terrain_atlas_lookup()
    ground_rows = parse_binary_rows(
        tuple("0" * len(ROWS[0]) for _ in range(DIGIT_ROWS)) + ROWS[DIGIT_ROWS:]
    )
    background = Image.new("RGB", size, BACKGROUND)
    motion = Motion(
        background=background,
        base=background,
        assembled=background,
        cells=cells,
        lookup=lookup,
        origin=(round(ORIGIN[0] * scale), round(ORIGIN[1] * scale)),
        map_size=(round(len(ROWS[0]) * CELL_PX * scale), round(len(ROWS) * CELL_PX * scale)),
        baseline_y=round(layout["digits"]["baseline_y"] * scale),
    )
    return replace(
        motion,
        base=render_state(motion, ground_rows),
        assembled=render_state(motion, parse_binary_rows(ROWS)),
    )


def frame_at(
    motion: Motion, frame_index: int, *, state: TerrainCoverState | None = None
) -> Image.Image:
    """Headless callers can seek independently or retain one simulation across every frame."""
    if type(frame_index) is not int or not 0 <= frame_index <= FRAME_COUNT:
        raise ValueError("frame index must be between zero and 160")
    state = state if state is not None else TerrainCoverState()
    state.advance_to(frame_index)
    return render_state(motion, state.rows)


def encode_motion(
    motion: Motion,
    executable: Path,
    destination: Path,
    *,
    cancelled: Callable[[], bool] = lambda: False,
    progress: Callable[[float], None] = lambda fraction: None,
    timeout_seconds: float = ENCODE_TIMEOUT_SECONDS,
) -> None:
    """Stream one RGB frame at a time, draining bounded diagnostics and cleaning failures."""
    process = subprocess.Popen(
        [
            str(executable),
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgb24",
            "-video_size",
            f"{motion.base.width}x{motion.base.height}",
            "-framerate",
            str(FPS),
            "-i",
            "pipe:0",
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            "20",
            "-threads",
            "2",
            "-pix_fmt",
            "yuv420p",
            "-frames:v",
            str(FRAME_COUNT),
            "-movflags",
            "+faststart",
            str(destination),
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    assert process.stdin is not None and process.stderr is not None
    diagnostics = bytearray()
    stopped = threading.Event()
    interrupted: list[str] = []
    deadline = time.monotonic() + timeout_seconds

    def kill() -> None:
        with contextlib.suppress(ProcessLookupError):
            if hasattr(os, "killpg"):
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()

    def drain() -> None:
        while chunk := process.stderr.read(4096):
            diagnostics.extend(chunk)
            del diagnostics[:-STDERR_LIMIT]

    def watch() -> None:
        # A watchdog also releases a blocked pipe write if encoding stalls or is cancelled.
        while not stopped.wait(0.1):
            reason = "terrain cover rendering was cancelled" if cancelled() else None
            if time.monotonic() >= deadline:
                reason = "terrain cover encoding exceeded its time limit"
            if reason:
                interrupted.append(reason)
                kill()
                return

    reader = threading.Thread(target=drain, name="terrain-cover-stderr", daemon=True)
    watcher = threading.Thread(target=watch, name="terrain-cover-watchdog", daemon=True)
    reader.start()
    watcher.start()
    complete = False
    try:
        try:
            state = TerrainCoverState()
            for index in range(FRAME_COUNT):
                if cancelled():
                    raise RuntimeError("terrain cover rendering was cancelled")
                process.stdin.write(frame_at(motion, index, state=state).tobytes())
                if index % FPS == 0:
                    progress(index / FRAME_COUNT)
            process.stdin.close()
            process.wait(timeout=max(1.0, deadline - time.monotonic()))
        except (BrokenPipeError, subprocess.TimeoutExpired):
            kill()
            process.wait()
        reader.join(timeout=5)
        if interrupted:
            raise RuntimeError(interrupted[0])
        if process.returncode != 0:
            detail = diagnostics.decode("utf-8", errors="replace").strip()[-2000:]
            detail = detail.replace(str(destination), destination.name).replace(
                str(executable), "ffmpeg"
            )
            raise RuntimeError(f"FFmpeg failed to encode the terrain cover: {detail}")
        if not destination.is_file() or destination.stat().st_size == 0:
            raise RuntimeError("FFmpeg produced no terrain cover video")
        complete = True
        progress(1.0)
    finally:
        stopped.set()
        if process.poll() is None:
            kill()
        with contextlib.suppress(BrokenPipeError):
            process.stdin.close()
        process.wait()
        reader.join(timeout=5)
        watcher.join(timeout=5)
        process.stderr.close()
        if not complete:
            destination.unlink(missing_ok=True)


@node(
    "terrain_cover_motion",
    inputs={"atlas": "image/png"},
    outputs={"video": "video/mp4"},
    tools=["ffmpeg"],
    resources=["terrain_example/resources/lookup.json"],
)
def terrain_cover_motion(ctx: Ctx) -> dict:
    """Render an eight-second loop locally; the atlas generation remains untouched."""
    executable = ctx.tool("ffmpeg").executable
    if executable is None:
        raise ctx.fail("FFmpeg is required for the optional animated terrain cover")
    motion = prepare_motion(ctx.read.bytes("atlas"))
    destination = ctx.out.path("cover.mp4")
    encode_motion(
        motion,
        executable,
        destination,
        cancelled=lambda: ctx.cancelled,
        progress=lambda fraction: ctx.progress("Rendering the terrain cover loop", fraction),
    )
    return {"video": ctx.out.file(destination, "video/mp4")}
