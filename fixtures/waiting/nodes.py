"""Generate three distinct PNG swatches and select one locally on timeout."""

import struct
import zlib

from grida.fx import Ctx, node


def chunk(kind: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + kind
        + payload
        + struct.pack(">I", zlib.crc32(kind + payload))
    )


@node(
    "draw_swatches",
    outputs={"a": "image/png", "b": "image/png", "c": "image/png"},
    version=1,
)
def draw(ctx: Ctx) -> dict:
    outputs = {}
    for key, color in {
        "a": (194, 79, 75),
        "b": (62, 128, 112),
        "c": (80, 102, 169),
    }.items():
        header = struct.pack(">2I5B", 32, 32, 8, 2, 0, 0, 0)
        pixels = (b"\0" + bytes(color) * 32) * 32
        path = ctx.out.path(f"{key}.png")
        path.write_bytes(
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(pixels))
            + chunk(b"IEND", b"")
        )
        outputs[key] = ctx.out.file(path, "image/png")
    return outputs


@node(
    "judge_swatches",
    inputs={"a": "image", "b": "image", "c": "image"},
    outputs={"image": "image/png"},
    version=1,
)
def judge(ctx: Ctx) -> dict:
    return {"image": ctx.inputs["c"]}


@node(
    "build_manifest",
    inputs={"image": "image"},
    params={"revision": str},
    outputs={"record": "json"},
)
def manifest(ctx: Ctx) -> dict:
    return {
        "record": ctx.out.json(
            {"revision": ctx.params["revision"], "image_digest": ctx.inputs["image"].digest}
        )
    }
