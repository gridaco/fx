"""Generate a one-pixel PNG directly from the PNG specification; no external artwork."""

import struct
import zlib

from grida.fx import Ctx, node


def chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


@node(
    "portable_draw",
    params={"message": str},
    outputs={"image": "image/png", "text": "text"},
    version=1,
)
def draw(ctx: Ctx) -> dict:
    header = struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
    data += chunk(b"IDAT", zlib.compress(b"\x00\x30\x60\x90")) + chunk(b"IEND", b"")
    return {"image": ctx.out.bytes(data, "image/png"), "text": ctx.out.text(ctx.params["message"])}
