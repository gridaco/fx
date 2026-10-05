"""Pictures for bodies: Pillow behind ``ctx.read.image`` and ``ctx.out.png``.

- :func:`read`: ``Image.open(path)``, ``load()``, and a copy (the first frame; no EXIF
  rotation).
- :func:`png`: ``picture.save(BytesIO, format="PNG", optimize=False)``; anything that is not a
  PIL image raises ``TypeError("ctx.out.png takes a PIL image, not <type>")``.

Pillow is imported when first needed, so a body that never touches a picture runs without it.
"""

from __future__ import annotations

import io
from pathlib import Path
from typing import Any


def read(path: str | Path) -> Any:
    """The picture at ``path`` (module docstring)."""
    from PIL import Image

    with Image.open(path) as picture:
        picture.load()
        return picture.copy()


def png(picture: Any) -> bytes:
    """``picture`` encoded as PNG (module docstring)."""
    from PIL import Image

    if not isinstance(picture, Image.Image):
        raise TypeError(f"ctx.out.png takes a PIL image, not {type(picture).__name__}")
    buffer = io.BytesIO()
    picture.save(buffer, format="PNG", optimize=False)
    return buffer.getvalue()
