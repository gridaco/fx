"""The inputs of ``test_std.py``, synthesized: small pictures and JSON documents.

Every input is made here, from a formula, so no media is committed. The golden outputs that
``test_std.py`` checks were recorded by FX's predecessor on inputs with exactly these bytes, made
with CPython 3.12 (its ``zlib`` module 1.2.12) and Pillow 12.3.0 (its bundled zlib-ng 1.3.1,
libjpeg-turbo and LittleCMS). :data:`INPUT_DIGESTS` holds those inputs' digests: another Pillow or
zlib build writes other bytes, and then the goldens cannot be compared (the tests skip, saying
which input differs).

``python tests/std_fixtures.py <folder>`` writes every input to ``<folder>``.
"""

from __future__ import annotations

import hashlib
import io
import struct
import sys
import zlib
from collections.abc import Callable
from pathlib import Path


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _png_file(picture, **options) -> bytes:
    buffer = io.BytesIO()
    picture.save(buffer, format="PNG", **options)
    return buffer.getvalue()


def _gradient(width: int, height: int):
    """RGBA with pixel (x, y) = (255x / (w-1), 255y / (h-1), xy mod 256, (7x + 3y) mod 256),
    the divisions rounded down: alpha is 0 at (0, 0)."""
    from PIL import Image

    picture = Image.new("RGBA", (width, height))
    for y in range(height):
        for x in range(width):
            picture.putpixel(
                (x, y),
                (
                    (x * 255) // max(1, width - 1),
                    (y * 255) // max(1, height - 1),
                    (x * y) % 256,
                    (x * 7 + y * 3) % 256,
                ),
            )
    return picture


def _srgb_profile() -> bytes:
    """LittleCMS's built-in sRGB profile, with its creation time fixed to 2026-10-05 18:32:18
    (header bytes 24 to 36: year, month, day, hour, minute, second), so the bytes do not depend
    on when they are made."""
    from PIL import ImageCms

    profile = bytearray(ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes())
    profile[24:36] = struct.pack(">6H", 2026, 10, 5, 18, 32, 18)
    return bytes(profile)


def rgba_37x23() -> bytes:
    """An RGBA gradient, 37 x 23."""
    return _png_file(_gradient(37, 23))


def rgba_icc() -> bytes:
    """The same pixels, with an sRGB ``iCCP`` chunk."""
    return _png_file(_gradient(37, 23), icc_profile=_srgb_profile())


def p_trns() -> bytes:
    """A 16 x 8 palette picture of three colours, index 0 transparent (and used)."""
    from PIL import Image

    picture = Image.new("P", (16, 8))
    picture.putpalette([0, 0, 0, 255, 0, 0, 0, 255, 0] + [0] * (253 * 3))
    for x in range(16):
        for y in range(8):
            picture.putpixel((x, y), (x + y) % 3)
    return _png_file(picture, transparency=0)


def rgb_40x30_jpg() -> bytes:
    """A flat (10, 200, 30) JPEG, 40 x 30, quality 90."""
    from PIL import Image

    buffer = io.BytesIO()
    Image.new("RGB", (40, 30), (10, 200, 30)).save(buffer, format="JPEG", quality=90)
    return buffer.getvalue()


def _chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def gray16() -> bytes:
    """A 4 x 4 16-bit greyscale PNG of grey 40000, written chunk by chunk (CPython's zlib)."""
    row = b"\0" + struct.pack(">H", 40000) * 4
    return (
        b"\x89PNG\r\n\x1a\n"
        + _chunk(b"IHDR", struct.pack(">IIBBBBB", 4, 4, 16, 0, 0, 0, 0))
        + _chunk(b"IDAT", zlib.compress(row * 4))
        + _chunk(b"IEND", b"")
    )


def _flat(width: int, height: int) -> bytes:
    from PIL import Image

    return _png_file(Image.new("RGBA", (width, height), (1, 2, 3, 4)))


def wide_8192() -> bytes:
    """8192 x 2 of (1, 2, 3, 4): mirrors on x to exactly 16384 px."""
    return _flat(8192, 2)


def wide_8193() -> bytes:
    """8193 x 2: one pixel too wide to mirror on x."""
    return _flat(8193, 2)


def tall_8193() -> bytes:
    """3 x 8193: one pixel too tall to mirror on y."""
    return _flat(3, 8193)


def not_an_image() -> bytes:
    return b"hello"


def a_json() -> bytes:
    """An object with a nested object, a non-ASCII string and numbers in exponent form."""
    return '{"b": 1, "a": {"x": 1.0, "y": [1, 2]}, "z": "é", "n": 1e16, "s": 1e-05}\n'.encode()


def b_json() -> bytes:
    return b'{"a": "replaced", "c": null, "d": true}\n'


def array_json() -> bytes:
    return b"[1, 2]\n"


def region_json() -> bytes:
    """Annotations: a note, a point, then two box marks (the first one is cropped to)."""
    return (
        b'{"kind": "gnode-annotations-v1", "annotations": [{"label": "note"}, '
        b'{"shape": "point", "at": [0.1, 0.1]}, '
        b'{"shape": "box", "box": [0.25, 0.25, 0.75, 0.75], "label": "face"}, '
        b'{"shape": "box", "box": [0, 0, 1, 1]}]}\n'
    )


def noshape_json() -> bytes:
    """Annotations with no box mark."""
    return b'{"annotations": [{"shape": "point", "at": [0.5, 0.5]}]}\n'


def notes_md() -> bytes:
    """Text with a CRLF line and an LF line."""
    return b"line one\r\nline two\n"


#: Each input by its file name.
INPUTS: dict[str, Callable[[], bytes]] = {
    "rgba_37x23.png": rgba_37x23,
    "rgba_icc.png": rgba_icc,
    "p_trns.png": p_trns,
    "rgb_40x30.jpg": rgb_40x30_jpg,
    "gray16.png": gray16,
    "wide_8192.png": wide_8192,
    "wide_8193.png": wide_8193,
    "tall_8193.png": tall_8193,
    "not_an_image.png": not_an_image,
    "a.json": a_json,
    "b.json": b_json,
    "array.json": array_json,
    "region.json": region_json,
    "noshape.json": noshape_json,
    "notes.md": notes_md,
}

#: The digests of the inputs the goldens were recorded on.
INPUT_DIGESTS: dict[str, str] = {
    "rgba_37x23.png": "26bd2d461ae12cbc705921d8a9901dc1660e24ea57acfec9547bb7281604f0a0",
    "rgba_icc.png": "d615b4bc601f72e5adaf71df5a2922136b8990aab2cbe8362259a014e72c01e1",
    "p_trns.png": "6455b4fec77d6bb30fd7284559ea057fc549ad2da7f4dc8d2afae265d80e9854",
    "rgb_40x30.jpg": "a9a1314ec4a2da52fc0db56764f322afec04ecb2523cac3e5435a17d60d0ec2e",
    "gray16.png": "4000e1bfa768c3acdc3889fca51b259165dc3912caae0358016b31aa97bee81c",
    "wide_8192.png": "adb08d3c8470275c4302dbf580a592fbc6c428990bf730558d1adadda5b34d1c",
    "wide_8193.png": "b6b7391e30954cddf9717b64b2f206cd219b37da235a67c21626a0cdd6e9ed9a",
    "tall_8193.png": "c711a2cc44a4dd2070a9a4a6e676978c8fc6f7ba7207b807849144a78492943a",
    "not_an_image.png": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    "a.json": "eb8eb453f3baf17a73e7fe52406bbda4645ad77a79951ff21830f27f91b6cc00",
    "b.json": "2d13dfb80f7b88014b7c3a90170a52a8bd9d8c60b526796f2f1476d1a73c7c22",
    "array.json": "1fb3ffcf3df89e31f56932d4a64a23eede8a2f6707898bf14d1fc42a78286868",
    "region.json": "95e323597f8c4e1554ed1ab72852178d934cfda2b0824d2efa8223d4ab823ae3",
    "noshape.json": "e66e840b36666d7ac53a8d473926e031593e425cc1dbd14c22c8aaa87b8d5690",
    "notes.md": "af28611c8dd7cdaa70b328947a47e7236543cff6aee512d92f80132b7f8db82f",
}


def write_all(folder: Path) -> dict[str, Path]:
    """Writes every input to ``folder``; returns each one's path by name."""
    folder.mkdir(parents=True, exist_ok=True)
    written = {}
    for name, make in INPUTS.items():
        path = folder / name
        path.write_bytes(make())
        written[name] = path
    return written


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: python tests/std_fixtures.py <folder>")
    for name, path in write_all(Path(sys.argv[1])).items():
        print(f"{sha256(path.read_bytes())}  {name}")
