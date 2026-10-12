"""The pet example's own nodes: find the poses in a drawn strip, cut them into equal cells, lay the
rows out as one sprite sheet, and play a row back as a preview (Pillow)."""

from __future__ import annotations

import io
import statistics
from dataclasses import dataclass

from PIL import Image, ImageOps

from grida.fx import Ctx, node

ALPHA = 24  # a pixel at least this opaque belongs to the pet
GAP = 6  # empty rows or columns that part two poses
SPECK = 0.04  # a piece with less than this share of the largest pose's pixels is a speck
PAD = 8  # the clear margin inside a cell
STANDING = 0.8  # the pet's usual height, as a share of the cell's inner height
BACKDROP = (244, 241, 234, 255)  # behind the previews, which are opaque GIFs

#: Codex's pet sheet, version 1: nine states in this order, eight columns of 192 x 208 cells.
CODEX_STATES = (
    ("idle", 6),
    ("running-right", 8),
    ("running-left", 8),
    ("waving", 4),
    ("jumping", 5),
    ("failed", 8),
    ("waiting", 6),
    ("running", 6),
    ("review", 6),
)
CODEX_CELL = (192, 208)


@dataclass
class Pose:
    box: tuple[int, int, int, int]  # left, top, right, bottom, in the strip
    band: tuple[int, int]  # the top and bottom of the row of poses it stands in
    pixels: int


def _mask(picture: Image.Image) -> Image.Image:
    return picture.getchannel("A").point(lambda a: 255 if a >= ALPHA else 0)


def _counts(mask: Image.Image) -> list[int]:
    """How many pet pixels each row of the mask has."""
    width, height = mask.size
    data = mask.tobytes()
    return [width - data[y * width : (y + 1) * width].count(0) for y in range(height)]


def _runs(counts: list[int]) -> list[list[int]]:
    """Spans [start, end) of nonzero counts; spans parted by fewer than GAP zeros are one."""
    runs: list[list[int]] = []
    for index, count in enumerate(counts):
        if not count:
            continue
        if runs and index - runs[-1][1] < GAP:
            runs[-1][1] = index + 1
        else:
            runs.append([index, index + 1])
    return runs


def _absorb(runs: list[list[int]], weights: list[int], share: float) -> list[list[int]]:
    """Joins each run lighter than ``share`` of the heaviest to its nearest neighbour."""
    runs, weights = [list(run) for run in runs], list(weights)
    while len(runs) > 1:
        light = min(range(len(runs)), key=lambda index: weights[index])
        if weights[light] >= share * max(weights):
            break
        left = runs[light][0] - runs[light - 1][1] if light > 0 else None
        right = runs[light + 1][0] - runs[light][1] if light + 1 < len(runs) else None
        into = light - 1 if right is None or (left is not None and left <= right) else light + 1
        low, high = sorted((light, into))
        runs[low] = [runs[low][0], runs[high][1]]
        weights[low] += weights[high]
        del runs[high], weights[high]
    return runs


def find_poses(strip: Image.Image) -> list[Pose]:
    """The poses in a strip, in reading order: rows of poses top to bottom, then left to right.

    A row of poses is a band of rows with pet pixels; a pose is a span of columns inside it.
    Specks (a stray pixel, a tear drawn apart from the face) join the nearest pose."""
    mask = _mask(strip.convert("RGBA"))
    width, _ = mask.size
    rows = _counts(mask)
    bands = _runs(rows)
    bands = _absorb(bands, [sum(rows[top:bottom]) for top, bottom in bands], 0.2)
    poses: list[Pose] = []
    for top, bottom in bands:
        band = mask.crop((0, top, width, bottom)).transpose(Image.Transpose.TRANSPOSE)
        columns = _counts(band)
        spans = _runs(columns)
        spans = _absorb(spans, [sum(columns[left:right]) for left, right in spans], SPECK)
        for left, right in spans:
            piece = mask.crop((left, top, right, bottom))
            x0, y0, x1, y1 = piece.getbbox() or (0, 0, right - left, bottom - top)
            box = (left + x0, top + y0, left + x1, top + y1)
            poses.append(Pose(box, (top, bottom), sum(columns[left:right])))
    return poses


def _anchor(mask: Image.Image) -> float:
    """Where the pet stands: the mean column of its lowest quarter's pixels."""
    width, height = mask.size
    feet = mask.crop((0, height - max(1, height // 4), width, height))
    columns = _counts(feet.transpose(Image.Transpose.TRANSPOSE))
    total = sum(columns)
    return sum(x * n for x, n in enumerate(columns)) / total if total else width / 2


def _clean(picture: Image.Image) -> Image.Image:
    """Clears the colour of fully transparent pixels, so they hold no hidden residue."""
    clear = picture.getchannel("A").point(lambda a: 255 if a == 0 else 0)
    picture.paste((0, 0, 0, 0), mask=clear)
    return picture


@node(
    "poses",
    inputs={"strip": "image"},
    params={"frames": int},
    outputs={"annotations": "annotations?"},
    judge=True,
)
def poses(ctx: Ctx) -> dict:
    """A drawn strip is accepted when it holds exactly ``frames`` poses, none cut by its edge."""
    strip = ctx.read.image("strip").convert("RGBA")
    width, height = strip.size
    found = find_poses(strip)
    clipped = [
        pose
        for pose in found
        if pose.box[0] == 0 or pose.box[1] == 0 or pose.box[2] == width or pose.box[3] == height
    ]
    for index, pose in enumerate(found, 1):
        x0, y0, x1, y1 = pose.box
        ctx.annotate(
            shape="box",
            box=[x0 / width, y0 / height, x1 / width, y1 / height],
            label=f"pose {index}" + (" (cut by the edge)" if pose in clipped else ""),
        )
    expected = ctx.params["frames"]
    ctx.fact("poses", len(found))
    ctx.fact("clipped", len(clipped))
    ctx.fact("verdict", "accept" if len(found) == expected and not clipped else "reject")
    return {}


@node(
    "row",
    inputs={"strip": "image"},
    params={"frames": int, "cell_width": int, "cell_height": int},
    outputs={"image": "image/png"},
)
def row(ctx: Ctx) -> dict:
    """The strip's poses cut into ``frames`` equal cells, side by side.

    Poses in one row of the strip keep their height above a shared ground line, so a jump stays a
    jump. One scale serves the whole row: the pet's usual height fills ``STANDING`` of the cell,
    unless the widest pose or the highest jump needs less. Each pose is centred on its feet."""
    strip = ctx.read.image("strip").convert("RGBA")
    frames = ctx.params["frames"]
    width, height = ctx.params["cell_width"], ctx.params["cell_height"]
    found = find_poses(strip)
    if len(found) != frames:
        raise ctx.fail(f"the strip holds {len(found)} poses, not {frames}")
    inner_width, inner_height = width - 2 * PAD, height - 2 * PAD
    lowest: dict[tuple[int, int], int] = {}
    highest: dict[tuple[int, int], int] = {}
    for pose in found:
        lowest[pose.band] = max(lowest.get(pose.band, 0), pose.box[3])
        highest[pose.band] = min(highest.get(pose.band, pose.box[1]), pose.box[1])
    usual = statistics.median(pose.box[3] - pose.box[1] for pose in found)
    reach = max(lowest[pose.band] - highest[pose.band] for pose in found)
    widest = max(pose.box[2] - pose.box[0] for pose in found)
    scale = min(STANDING * inner_height / usual, inner_height / reach, inner_width / widest)
    sheet = Image.new("RGBA", (frames * width, height), (0, 0, 0, 0))
    for index, pose in enumerate(found):
        x0, _, x1, _ = pose.box
        crop = strip.crop((x0, highest[pose.band], x1, lowest[pose.band]))
        size = (max(1, round(crop.width * scale)), max(1, round(crop.height * scale)))
        crop = crop.resize(size, Image.Resampling.LANCZOS)
        left = round(width / 2 - _anchor(_mask(crop)))
        left = min(max(left, PAD), width - PAD - crop.width)
        sheet.alpha_composite(crop, (index * width + left, height - PAD - crop.height))
    ctx.fact("scale", round(scale, 4))
    return {"image": ctx.out.png(_clean(sheet))}


def _codex(states: list[dict], cell: tuple[int, int]) -> bool:
    """Whether the sheet is laid out as Codex's pet sheet, version 1."""
    layout = tuple((state["id"], state["frames"]) for state in states)
    return layout == CODEX_STATES and cell == CODEX_CELL


@node(
    "sheet",
    inputs={"rows": "image{}"},
    params={
        "states": list,
        "cell_width": int,
        "cell_height": int,
        "id": str,
        "name": str,
        "description": str,
    },
    outputs={"spritesheet": "image/webp", "layout": "json", "pet": "json?"},
)
def sheet(ctx: Ctx) -> dict:
    """Every state's row, one under another, as a lossless WebP. A state that mirrors another is
    that state's row with each frame flipped in its own cell, so the frames keep their order."""
    states = ctx.params["states"]
    width, height = ctx.params["cell_width"], ctx.params["cell_height"]
    rows = ctx.inputs["rows"]
    columns = max(state["frames"] for state in states)
    canvas = Image.new("RGBA", (columns * width, len(states) * height), (0, 0, 0, 0))
    for index, state in enumerate(states):
        source = state.get("mirror_of") or state["id"]
        if source not in rows:
            raise ctx.fail(f"no row was drawn for {source}")
        with Image.open(rows[source].path) as opened:
            strip = opened.convert("RGBA")
        if strip.size != (state["frames"] * width, height):
            raise ctx.fail(f"the row of {source} is {strip.size[0]}x{strip.size[1]}")
        for frame in range(state["frames"]):
            cell = strip.crop((frame * width, 0, (frame + 1) * width, height))
            if state.get("mirror_of"):
                cell = ImageOps.mirror(cell)
            canvas.paste(cell, (frame * width, index * height))
    data = io.BytesIO()
    canvas.save(data, "WEBP", lossless=True, quality=100, method=6, exact=True)
    layout = {
        "cell": {"width": width, "height": height},
        "columns": columns,
        "rows": [
            {
                "state": state["id"],
                "row": index,
                "frames": state["frames"],
                "durations_ms": state.get("durations") or [],
            }
            for index, state in enumerate(states)
        ],
    }
    codex = _codex(states, (width, height))
    ctx.fact("codex_pet", codex)
    pet = {
        "id": ctx.params["id"],
        "displayName": ctx.params["name"],
        "description": ctx.params["description"],
        "spritesheetPath": "spritesheet.webp",
    }
    return {
        "spritesheet": ctx.out.bytes(data.getvalue(), "image/webp"),
        "layout": ctx.out.json(layout),
        "pet": ctx.out.json(pet) if codex else None,
    }


@node(
    "preview",
    inputs={"sheet": "image"},
    params={"states": list, "state": str, "cell_width": int, "cell_height": int},
    outputs={"image": "image/gif"},
)
def preview(ctx: Ctx) -> dict:
    """One state's row played back at its own timing, on a plain backdrop."""
    states = ctx.params["states"]
    width, height = ctx.params["cell_width"], ctx.params["cell_height"]
    index = next(i for i, state in enumerate(states) if state["id"] == ctx.params["state"])
    state = states[index]
    durations = state.get("durations") or [140] * state["frames"]
    if len(durations) != state["frames"]:
        raise ctx.fail(f"{state['id']} has {state['frames']} frames but {len(durations)} durations")
    with Image.open(ctx.inputs["sheet"].path) as opened:
        picture = opened.convert("RGBA")
    frames = []
    for frame in range(state["frames"]):
        box = (frame * width, index * height, (frame + 1) * width, (index + 1) * height)
        cell = picture.crop(box)
        backdrop = Image.new("RGBA", cell.size, BACKDROP)
        frames.append(Image.alpha_composite(backdrop, cell).convert("RGB"))
    data = io.BytesIO()
    frames[0].save(data, "GIF", save_all=True, append_images=frames[1:], duration=durations, loop=0)
    return {"image": ctx.out.bytes(data.getvalue(), "image/gif")}
