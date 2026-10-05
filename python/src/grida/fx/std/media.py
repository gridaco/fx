"""The nine standard bodies, ported byte for byte from FX's predecessor (``grida.fx.std``).

Each takes a :class:`~grida.fx.Ctx` and returns its outputs. Every picture output is
``_png(picture)``: ``picture.save(BytesIO(), format="PNG")`` with nothing else, so Pillow's own
encoder settles every byte; the RGBA conversion and rounding are Pillow's and Python's
(``round`` ties to even). Messages are the predecessor's, verbatim.

- ``mirror_repeat``: ``axis`` containing ``x`` doubles the width with a left-right mirror, then
  ``y`` doubles the height with a top-bottom flip; past 16384 px: ``mirroring <n> px on <axis>
  exceeds 16384 px``.
- ``check_alpha`` (judge): facts ``alpha_min``, ``alpha_max``, ``verdict``; ``transparent`` wants
  a fully transparent pixel, anything else every pixel opaque.
- ``check_size`` (judge): facts ``width``, ``height``, ``verdict``.
- ``resize``: ``longest_side`` (scale both sides, at least 1 px each), else ``width`` and
  ``height``, else ``resize takes longest_side, or width and height``; Lanczos.
- ``crop``: ``box`` ``[x0, y0, x1, y1]`` in fractions, else the first box mark of ``region``,
  grown by ``padding`` × its size on each side, clamped; fact ``box_px``.
- ``pad``: centred on a transparent ``width`` × ``height`` canvas (floor offsets).
- ``json_merge``: a shallow merge of JSON objects in order.
- ``files_copy``: the same bytes and kind.
- ``package``: lays out ``files`` (``{key}`` destinations take a keyed collection) and writes a
  manifest ``{"files": [sorted paths], **manifest}``.

How values arrive (``spec/protocol.md`` sections 5.3 and 8): inputs are
:class:`~grida.fx.InputFile` objects (a list of them for ``documents``); params are JSON values,
with a text or JSON file given to a param arriving as its content and every other file as an
:class:`~grida.fx.InputFile` at its place. Numbers arrive as JSON numbers, so a whole number is a
Python ``int`` however the workflow wrote it. Pillow is imported when a picture is first needed.
"""

from __future__ import annotations

import io
import json
import re
from collections.abc import Mapping
from pathlib import Path
from typing import Any

# The module, not the class: ``grida.fx._ctx`` may import ``grida.fx.std`` itself (for
# pictures), and the class is only needed once a body runs.
from grida.fx import _ctx
from grida.fx._errors import NodeFailure

#: The widest or tallest picture ``mirror_repeat`` makes.
MAX_EDGE_PX = 16_384


def _picture(ctx: Any, name: str) -> Any:
    from PIL import Image

    picture = ctx.read.image(name)
    if not isinstance(picture, Image.Image):
        raise NodeFailure(f"{name} did not read as a picture")
    return picture if picture.mode == "RGBA" else picture.convert("RGBA")


def _png(picture: Any) -> bytes:
    buffer = io.BytesIO()
    picture.save(buffer, format="PNG")
    return buffer.getvalue()


def mirror_repeat(ctx: Any) -> dict[str, Any]:
    """Reflect a picture onto itself along x, y or both, so its edges repeat exactly."""
    from PIL import Image, ImageOps

    picture = _picture(ctx, "image")
    axis = ctx.params["axis"]
    if "x" in axis:
        if picture.width * 2 > MAX_EDGE_PX:
            raise NodeFailure(f"mirroring {picture.width} px on x exceeds {MAX_EDGE_PX} px")
        wide = Image.new("RGBA", (picture.width * 2, picture.height))
        wide.paste(picture, (0, 0))
        wide.paste(ImageOps.mirror(picture), (picture.width, 0))
        picture = wide
    if "y" in axis:
        if picture.height * 2 > MAX_EDGE_PX:
            raise NodeFailure(f"mirroring {picture.height} px on y exceeds {MAX_EDGE_PX} px")
        tall = Image.new("RGBA", (picture.width, picture.height * 2))
        tall.paste(picture, (0, 0))
        tall.paste(ImageOps.flip(picture), (0, picture.height))
        picture = tall
    return {"image": ctx.out.bytes(_png(picture), "image/png")}


def check_alpha(ctx: Any) -> dict[str, Any]:
    """Judge whether a picture is transparent where it should be, or fully opaque."""

    alpha = _picture(ctx, "image").getchannel("A")
    low, high = alpha.getextrema()
    transparent = low == 0
    ctx.fact("alpha_min", low)
    ctx.fact("alpha_max", high)
    wanted = ctx.params["expect"]
    accepted = transparent if wanted == "transparent" else low == 255
    ctx.fact("verdict", "accept" if accepted else "reject")
    return {}


def check_size(ctx: Any) -> dict[str, Any]:
    """Judge a picture's size against the width and height asked for."""

    picture = _picture(ctx, "image")
    ctx.fact("width", picture.width)
    ctx.fact("height", picture.height)
    width, height = ctx.params.get("width"), ctx.params.get("height")
    accepted = (width is None or picture.width == width) and (
        height is None or picture.height == height
    )
    ctx.fact("verdict", "accept" if accepted else "reject")
    return {}


def resize(ctx: Any) -> dict[str, Any]:
    """Scale a picture to fit: by its longest side, or to a width and height."""
    from PIL import Image

    picture = _picture(ctx, "image")
    longest = ctx.params.get("longest_side")
    width, height = ctx.params.get("width"), ctx.params.get("height")
    if longest is not None:
        scale = longest / max(picture.size)
        size = (max(1, round(picture.width * scale)), max(1, round(picture.height * scale)))
    elif width is not None and height is not None:
        size = (width, height)
    else:
        raise NodeFailure("resize takes longest_side, or width and height")
    resized = picture.resize(size, Image.Resampling.LANCZOS)
    return {"image": ctx.out.bytes(_png(resized), "image/png")}


def crop(ctx: Any) -> dict[str, Any]:
    """Cut a box out of a picture: ``box`` as [x0, y0, x1, y1] from 0 to 1, padded."""

    picture = _picture(ctx, "image")
    box = ctx.params.get("box")
    if box is None and "region" in ctx.inputs and ctx.inputs["region"] is not None:
        marks = ctx.read.annotations("region").get("annotations", [])
        boxes = [mark["box"] for mark in marks if mark.get("shape") == "box"]
        if not boxes:
            raise NodeFailure("the region has no box to crop to")
        box = boxes[0]
    if box is None or len(box) != 4:
        raise NodeFailure("crop takes box: [x0, y0, x1, y1], or a region with a box mark")
    pad = float(ctx.params.get("padding", 0.0))
    x0, y0, x1, y1 = (float(value) for value in box)
    dx, dy = (x1 - x0) * pad, (y1 - y0) * pad
    left = max(0, round((x0 - dx) * picture.width))
    top = max(0, round((y0 - dy) * picture.height))
    right = min(picture.width, round((x1 + dx) * picture.width))
    bottom = min(picture.height, round((y1 + dy) * picture.height))
    if right <= left or bottom <= top:
        raise NodeFailure(f"the box {box} is empty on a {picture.width}x{picture.height} picture")
    cut = picture.crop((left, top, right, bottom))
    ctx.fact("box_px", [left, top, right, bottom])
    return {"image": ctx.out.bytes(_png(cut), "image/png")}


def pad(ctx: Any) -> dict[str, Any]:
    """Centre a picture on a transparent canvas of the asked size."""
    from PIL import Image

    picture = _picture(ctx, "image")
    width, height = ctx.params["width"], ctx.params["height"]
    if picture.width > width or picture.height > height:
        raise NodeFailure(
            f"a {picture.width}x{picture.height} picture does not fit {width}x{height}"
        )
    canvas = Image.new("RGBA", (width, height))
    canvas.paste(picture, ((width - picture.width) // 2, (height - picture.height) // 2))
    return {"image": ctx.out.bytes(_png(canvas), "image/png")}


def json_merge(ctx: Any) -> dict[str, Any]:
    """Merge JSON objects in order; a later key wins."""

    merged: dict[str, Any] = {}
    for document in ctx.inputs["documents"]:
        value = json.loads(document.read_bytes())
        if not isinstance(value, dict):
            raise NodeFailure("json.merge merges objects")
        merged.update(value)
    return {"json": ctx.out.json(merged)}


def files_copy(ctx: Any) -> dict[str, Any]:
    """Pass one file through unchanged: a stable name for something other steps made."""

    file = ctx.inputs["file"]
    return {"file": ctx.out.bytes(file.read_bytes(), file.kind)}


#: A destination inside the package: relative, portable, no traversal; ``{key}`` once at most.
_DESTINATION = re.compile(r"^(?!/)[A-Za-z0-9_.{}-]+(?:/[A-Za-z0-9_.{}-]+)*$")
_TEXT_KINDS = {".md": "text/markdown", ".txt": "text/plain"}


def _stored_bytes(file: _ctx.InputFile) -> bytes | None:
    """A file's bytes, or ``None`` when the engine handed it without a place to read them (a
    file ref always has a ``path``, ``spec/protocol.md`` section 3.1; this keeps the
    predecessor's sentence for one that does not)."""

    try:
        path = file.path
    except (AttributeError, KeyError, TypeError):
        return None
    if path is None:
        return None
    return file.read_bytes()


def _laid_out(ctx: Any, value: Any, destination: str) -> Any:
    """One value as a file of the package: a file as it is, text as text, data as JSON."""

    if isinstance(value, _ctx.InputFile):
        data = _stored_bytes(value)
        if data is None:
            raise NodeFailure(f"{destination}: {value.name} has no bytes")
        return ctx.out.bytes(data, value.kind)
    if isinstance(value, str):
        return ctx.out.text(value, _TEXT_KINDS.get(Path(destination).suffix, "text/plain"))
    return ctx.out.json(value)


def _manifest_value(value: Any) -> Any:
    if isinstance(value, _ctx.InputFile):
        return {"digest": value.digest, "kind": value.kind, "name": value.name}
    if isinstance(value, Mapping):
        return {str(key): _manifest_value(item) for key, item in value.items()}
    if isinstance(value, list | tuple):
        return [_manifest_value(item) for item in value]
    return value


def package(ctx: Any) -> dict[str, Any]:
    """Lay results out as a folder by destination path, beside a manifest written as JSON.

    A destination with ``{key}`` takes a keyed collection, one file per key; any other takes
    one value. A missing value (a step that did not run) leaves its file out.
    """

    files: dict[str, Any] = {}
    for destination, value in ctx.params["files"].items():
        if not _DESTINATION.match(destination) or ".." in destination.split("/"):
            raise NodeFailure(f"{destination!r} is not a relative path inside the package")
        if "{key}" not in destination:
            if value is not None:
                files[destination] = _laid_out(ctx, value, destination)
            continue
        if not isinstance(value, Mapping):
            raise NodeFailure(f"{destination} names {{key}}, so it takes a keyed collection")
        for key, item in value.items():
            if item is not None:
                path = destination.replace("{key}", str(key))
                files[path] = _laid_out(ctx, item, path)
    manifest = {"files": sorted(files), **_manifest_value(ctx.params["manifest"])}
    return {"files": files, "manifest": ctx.out.json(manifest)}
