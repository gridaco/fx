"""The stand-in of the stand-in-errors case: what it does with an image.generate call depends
on the prompt, one way for each step of the workflow:

- `a good picture`: an opaque PNG of the size asked for;
- `refuse this`: raises CallRefused, a `capability_refused` error;
- `fail this`: raises CallFailed, a `call_failed` error;
- `decline this`: returns DECLINE;
- `too small`: a 32x32 PNG, which the image check refuses for a 64x64 request;
- `text instead`: text/plain bytes as `image`, which the answer's shape refuses;
- `an extra file`: a good `image` and a `thumbnail` the capability does not return;
- `no file at all`: an answer with no file;
- `data as well`: a good `image`, with data where image.generate returns none.

The body step's call is refused before the stand-in is asked, so it never gets here. It counts
its calls in `count.txt` and keeps what it was asked in `asked.json`, in its working directory,
which is the engine's: the project the case runs in. `asked.json` holds no path, only whether
each file the engine handed over can be read where its ref says.
"""

import json
import struct
import zlib
from pathlib import Path
from typing import Any

from grida.fx import DECLINE, Answer, CallFailed, CallRefused, InputFile, Output, StandInCall

COUNT = Path("count.txt")
ASKED = Path("asked.json")
BLUE = (40, 90, 160)


def answer(call: StandInCall) -> Any:
    note(call)
    prompt = call.request["prompt"]
    width, height = (int(side) for side in call.request["size"].split("x"))
    good = png(width, height, BLUE)
    if prompt == "a good picture":
        return Answer(files={"image": good})
    if prompt == "refuse this":
        raise CallRefused("the stand-in refuses this prompt")
    if prompt == "fail this":
        raise CallFailed("the stand-in failed this prompt")
    if prompt == "too small":
        return Answer(files={"image": png(width // 2, height // 2, BLUE)})
    if prompt == "text instead":
        return Answer(files={"image": Output(kind="text/plain", data=b"not a picture\n")})
    if prompt == "an extra file":
        return Answer(files={"image": good, "thumbnail": png(8, 8, BLUE)})
    if prompt == "no file at all":
        return Answer()
    if prompt == "data as well":
        return Answer(files={"image": good}, data={"seed": 7})
    return DECLINE


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
