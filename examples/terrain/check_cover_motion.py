"""Check the complete free cover workflow and its bounded animation with synthetic tiles."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

from cover import DIGIT_ROWS, ROWS
from cover_motion import (
    FPS,
    FRAME_COUNT,
    TerrainCoverState,
    encode_motion,
    frame_at,
    prepare_motion,
)
from make_fixture import make_cover_fixture
from PIL import Image, ImageChops
from terrain_example.atlas import peering_mask

from grida.fx import plan, run


def check_state_and_frames(source: bytes) -> None:
    motion = prepare_motion(source, width=240)
    assembled = motion.assembled.tobytes()
    assert frame_at(motion, 0).tobytes() == assembled
    assert frame_at(motion, FRAME_COUNT - 1).tobytes() == assembled
    assert frame_at(motion, FRAME_COUNT).tobytes() == assembled
    assert frame_at(motion, 64).tobytes() == motion.base.tobytes()
    assert assembled != motion.base.tobytes()

    state = TerrainCoverState()
    identity = id(state)
    target = state.rows
    assert target == tuple(tuple(value == "1" for value in row) for row in ROWS)
    assert state.frame_index == 0
    assert state.advance_to(20) is False
    state.advance_to(40)
    removed = [
        (x, y)
        for y in range(DIGIT_ROWS)
        for x, occupied in enumerate(target[y])
        if occupied and not state.rows[y][x]
    ]
    order = [(x, y) for y in range(DIGIT_ROWS) for x, occupied in enumerate(target[y]) if occupied]
    assert removed == order[: len(removed)] and 0 < len(removed) < len(order)
    assert state.advance_to(60) is True
    assert not any(any(row) for row in state.rows[:DIGIT_ROWS])
    assert state.rows[DIGIT_ROWS:] == target[DIGIT_ROWS:]
    assert any(
        motion.lookup.by_mask[peering_mask(state.rows, x, DIGIT_ROWS)]
        != motion.lookup.by_mask[peering_mask(target, x, DIGIT_ROWS)]
        for x, occupied in enumerate(target[DIGIT_ROWS])
        if occupied
    )
    assert state.advance_to(68) is False
    state.advance_to(88)
    grown = [(x, y) for y in range(DIGIT_ROWS) for x, value in enumerate(state.rows[y]) if value]
    grow_order = sorted(order, key=lambda cell: (-cell[1], cell[0]))
    assert set(grown) == set(grow_order[: len(grown)]) and 0 < len(grown) < len(order)
    state.advance_to(108)
    assert state.rows == target
    assert state.advance_to(FRAME_COUNT) is False
    assert state.rows == target and id(state) == identity
    assert state.advance_to(FRAME_COUNT) is False
    for invalid in (-1, 0, FRAME_COUNT + 1, 1.5):
        try:
            state.advance_to(invalid)
        except ValueError:
            pass
        else:
            raise AssertionError("state must reject invalid or backwards frame indices")

    sequential = TerrainCoverState()
    for index in range(FRAME_COUNT):
        frame = frame_at(motion, index, state=sequential)
        assert sequential.frame_index == index
        assert sequential.rows[DIGIT_ROWS:] == target[DIGIT_ROWS:]
        assert all(value in (False, True) for row in sequential.rows for value in row)
        for y, row in enumerate(sequential.rows):
            for x, occupied in enumerate(row):
                if occupied:
                    coordinate = motion.lookup.by_mask[peering_mask(sequential.rows, x, y)]
                    assert coordinate in motion.cells
        if index in (0, 30, 40, 60, 64, 75, 88, 108, 159):
            assert frame.tobytes() == frame_at(motion, index).tobytes()
        # The synthesized opaque ground covers every visible side/bottom pixel.
        for box in (
            (0, frame.height - 1, frame.width, frame.height),
            (0, motion.baseline_y + 2, 1, frame.height),
            (frame.width - 1, motion.baseline_y + 2, frame.width, frame.height),
        ):
            strip = frame.crop(box)
            assert all(
                color != (238, 234, 226) for _, color in strip.getcolors(strip.width * strip.height)
            )
    assert sequential.rows == target
    assert ImageChops.difference(frame_at(motion, 40), motion.base).getbbox() is not None
    assert ImageChops.difference(frame_at(motion, 40), motion.assembled).getbbox() is not None
    ground_box = (0, motion.baseline_y, motion.base.width, motion.base.height)
    assert (
        ImageChops.difference(
            motion.base.crop(ground_box), motion.assembled.crop(ground_box)
        ).getbbox()
        is not None
    )

    # The synthetic cells have opaque, flat centers. A partially revealed final image
    # cannot satisfy these intermediate-state tile-color checks at normal cover size.
    normal = prepare_motion(source)
    assert normal.origin[0] < 0
    assert normal.origin[0] + normal.map_size[0] > normal.base.width
    assert normal.origin[1] + normal.map_size[1] > normal.base.height
    visible_left = int((0 - normal.origin[0]) * len(target[0]) / normal.map_size[0])
    visible_right = int(
        (normal.base.width - 1 - normal.origin[0]) * len(target[0]) / normal.map_size[0]
    )
    visible_bottom = int(
        (normal.base.height - 1 - normal.origin[1]) * len(target) / normal.map_size[1]
    )
    assert 0 < visible_left < visible_right < len(target[0]) - 1
    assert DIGIT_ROWS < visible_bottom < len(target) - 1
    assert peering_mask(target, visible_left, visible_bottom) == (1,) * 9
    assert peering_mask(target, visible_right, visible_bottom) == (1,) * 9
    visible_samples = culled_samples = 0
    for index in (40, 88):
        current = TerrainCoverState()
        frame = frame_at(normal, index, state=current)
        for y, row in enumerate(current.rows):
            for x, occupied in enumerate(row):
                if not occupied:
                    continue
                coordinate = normal.lookup.by_mask[peering_mask(current.rows, x, y)]
                tile = normal.cells[coordinate]
                expected = tile.getpixel((tile.width // 2, tile.height // 2))
                assert expected[3] == 255
                location = (
                    normal.origin[0] + int((x + 0.5) * normal.map_size[0] / len(row)),
                    normal.origin[1] + int((y + 0.5) * normal.map_size[1] / len(current.rows)),
                )
                if not (0 <= location[0] < frame.width and 0 <= location[1] < frame.height):
                    culled_samples += 1
                    continue
                assert frame.getpixel(location) == expected[:3]
                visible_samples += 1
    assert visible_samples > 0 and culled_samples > 0


def check_cancellation(source: bytes, ffmpeg: str, destination: Path) -> None:
    motion = prepare_motion(source, width=240)
    try:
        encode_motion(motion, Path(ffmpeg), destination, cancelled=lambda: True)
    except RuntimeError as error:
        assert "cancelled" in str(error)
    else:
        raise AssertionError("cancelled rendering must fail")
    assert not destination.exists()


def main() -> None:
    home = Path(__file__).resolve().parent
    os.environ.update(
        GRIDA_FX_DISABLE_DOTENV="1", GRIDA_FX_NETWORK="off", GRIDA_FX_PYTHON=sys.executable
    )
    for name in (
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
    ):
        os.environ.pop(name, None)
    ffmpeg, ffprobe = shutil.which("ffmpeg"), shutil.which("ffprobe")
    if not ffmpeg or not ffprobe:
        raise RuntimeError("this optional cover check requires FFmpeg and ffprobe on PATH")
    inputs = make_cover_fixture(home / ".fixture")
    source = (inputs.parent / "atlas.png").read_bytes()
    check_state_and_frames(source)
    check_cancellation(source, ffmpeg, inputs.parent / "cancelled-cover.mp4")

    planned = plan("render-cover-video.yaml", cwd=home, input_files=[inputs])
    if not planned.ok:
        raise RuntimeError(planned.problems)
    result = run("render-cover-video.yaml", cwd=home, input_files=[inputs])
    if not result.ok or result.cost != 0:
        raise RuntimeError(f"motion fixture failed: {result.failures}; cost={result.cost}")
    assert result.outputs["atlas"].read_bytes() == source
    with Image.open(result.outputs["cover"].path) as poster:
        assert poster.format == "WEBP" and poster.size == (1920, 1200)
    report = json.loads(
        subprocess.run(
            [
                ffprobe,
                "-v",
                "error",
                "-count_frames",
                "-show_streams",
                "-of",
                "json",
                str(result.outputs["cover_video"].path),
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        ).stdout
    )
    assert len(report["streams"]) == 1
    stream = report["streams"][0]
    assert stream["codec_type"] == "video" and stream["codec_name"] == "h264"
    assert stream["pix_fmt"] == "yuv420p"
    assert (stream["width"], stream["height"]) == (960, 600)
    assert stream["avg_frame_rate"] == f"{FPS}/1"
    assert int(stream["nb_read_frames"]) == FRAME_COUNT
    assert float(stream["duration"]) == FRAME_COUNT / FPS
    print(
        "terrain motion: exact assembled endpoints, fixed ground occupancy, "
        "current-state autotiling, frame-filling ground; "
        "complete 960x600 H.264 workflow at $0"
    )


if __name__ == "__main__":
    main()
