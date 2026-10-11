"""The one place bytes become pixels and pixels become bytes.

There were five PNG encoders and six decoders in the tree, written where they
were first needed. Two of the encoders compressed at level nine and three at
PIL's default, so a pixel-identical image hashed differently depending on
which path produced it - and those hashes are cache lineage. The decoders
disagreed about errors: one refused animated input and raised ``ValueError``
for everything, the others raised whatever PIL raised, so a caller written to
the ``ValueError`` contract caught nothing on the other paths.

The level is a parameter here rather than a constant, on purpose. Every call
site keeps the level it had, because changing one changes the bytes that a
downstream cache key binds; the encoder test pins both levels so that a move
is a decision with a diff and never a surprise on a cold run.
"""

from __future__ import annotations

from io import BytesIO

from PIL import Image, UnidentifiedImageError

#: PIL's own default; what most of the tree encoded at before it was written down.
DEFAULT_COMPRESS_LEVEL = 6


def decode_image(data: bytes, *, label: str = "image", require_png: bool = False) -> Image.Image:
    """Decode one still image, refusing empty, animated, and undecodable input.

    Every failure is a ``ValueError`` naming ``label``, so a caller has one
    exception to catch whichever path the bytes came down.
    """

    if not data:
        raise ValueError(f"{label} data must be non-empty")
    try:
        with Image.open(BytesIO(data)) as opened:
            if require_png and opened.format != "PNG":
                raise ValueError(f"{label} must be PNG")
            if getattr(opened, "n_frames", 1) != 1:
                raise ValueError("animated images are not supported")
            opened.load()
            return opened.copy()
    except (UnidentifiedImageError, OSError, SyntaxError) as error:
        raise ValueError(f"{label} is not a decodable image") from error


def decode_rgba(data: bytes, *, label: str = "image", require_png: bool = False) -> Image.Image:
    return decode_image(data, label=label, require_png=require_png).convert("RGBA")


def encode_png(image: Image.Image, *, compress_level: int = DEFAULT_COMPRESS_LEVEL) -> bytes:
    if not 0 <= compress_level <= 9:
        raise ValueError("PNG compress_level must be between 0 and 9")
    stream = BytesIO()
    image.save(stream, format="PNG", compress_level=compress_level, optimize=False)
    return stream.getvalue()


__all__ = ["DEFAULT_COMPRESS_LEVEL", "decode_image", "decode_rgba", "encode_png"]
