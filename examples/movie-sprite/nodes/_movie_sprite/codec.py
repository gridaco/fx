"""PNG encoding shared by the local finishing helpers."""

from io import BytesIO

from PIL import Image


def encode_png(image: Image.Image, *, compress_level: int = 6) -> bytes:
    if not 0 <= compress_level <= 9:
        raise ValueError("PNG compress_level must be between 0 and 9")
    stream = BytesIO()
    image.save(stream, format="PNG", compress_level=compress_level, optimize=False)
    return stream.getvalue()
