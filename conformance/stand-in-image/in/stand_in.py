"""The stand-in of the stand-in-image case: it answers every image.generate call with an opaque
PNG of the size asked for, made here, and declines anything else.

It counts its calls in `count.txt` and keeps what it was asked in `asked.json`, both in its
working directory, which is the engine's: the project the case runs in. `asked.json` holds no
path, only whether each file the engine handed over can be read where its ref says.
"""

import json
import struct
import zlib
from pathlib import Path
from typing import Any

from grida.fx import DECLINE, Answer, InputFile, StandInCall

COUNT = Path("count.txt")
ASKED = Path("asked.json")


def answer(call: StandInCall) -> Any:
    note(call)
    if call.capability != "image.generate":
        return DECLINE
    width, height = (int(side) for side in call.request["size"].split("x"))
    return Answer(files={"image": png(width, height, (40, 90, 160))})


# ------------------------------------------------------------------------------ the notes


def note(call: StandInCall) -> None:
    """Counts the call in count.txt and adds what it was asked to asked.json, sorted, so the
    file does not depend on the order calls arrive in."""

    count = int(COUNT.read_text()) if COUNT.is_file() else 0
    COUNT.write_text(f"{count + 1}\n")
    asked = json.loads(ASKED.read_text()) if ASKED.is_file() else []
    asked.append(
        {
            "capability": call.capability,
            "route": {"id": call.route.id, "fingerprint": call.route.fingerprint},
            "request": plain(call.request),
            "take": list(call.takes),
            "key": call.key,
            "instance": {
                "id": call.instance.id,
                "path": call.instance.path,
                "step": call.instance.step,
            },
            "files": {
                digest: {
                    "name": file.name,
                    "kind": file.kind,
                    "size": file.size,
                    "facts": file.facts,
                    "path_is_file": file.path.is_file(),
                }
                for digest, file in sorted(call.files.items())
            },
        }
    )
    asked.sort(key=lambda entry: json.dumps(entry, sort_keys=True))
    ASKED.write_text(json.dumps(asked, indent=1, sort_keys=True) + "\n")


def plain(value: Any) -> Any:
    """A request as the wire carries it: each file as {"file": digest}."""

    if isinstance(value, InputFile):
        return {"file": value.digest}
    if isinstance(value, dict):
        return {key: plain(item) for key, item in value.items()}
    if isinstance(value, list):
        return [plain(item) for item in value]
    return value


# ------------------------------------------------------------------------------- pictures


def png(width: int, height: int, pixel: tuple[int, ...]) -> bytes:
    """A PNG of one colour: RGB for three channels (opaque, no alpha channel), RGBA for four.

    The image data is zlib with stored (uncompressed) deflate blocks, written here rather than
    by zlib.compress, so the bytes, and every digest made from them, are the same whatever
    zlib the Python that runs the stand-in was built with."""

    color_type = {3: 2, 4: 6}[len(pixel)]
    row = b"\x00" + bytes(pixel) * width
    header = struct.pack(">IIBBBBB", width, height, 8, color_type, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", stored_zlib(row * height))
        + chunk(b"IEND", b"")
    )


def chunk(tag: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))


def stored_zlib(data: bytes) -> bytes:
    """A zlib stream (RFC 1950) of stored deflate blocks (RFC 1951 section 3.2.4)."""

    blocks = [data[at : at + 0xFFFF] for at in range(0, len(data), 0xFFFF)] or [b""]
    out = bytearray(b"\x78\x01")
    for number, block in enumerate(blocks, 1):
        out.append(1 if number == len(blocks) else 0)
        out += struct.pack("<HH", len(block), len(block) ^ 0xFFFF) + block
    return bytes(out + struct.pack(">I", zlib.adler32(data)))
