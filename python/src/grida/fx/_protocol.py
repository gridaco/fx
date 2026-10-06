"""The host side of the node protocol's transport (``spec/protocol.md`` section 1).

Framing is LSP's base protocol: ``Content-Length: <n>\\r\\n\\r\\n`` then n bytes of UTF-8 JSON.
Messages are JSON-RPC 2.0; every message must be I-JSON (no NaN or infinity: dump with
``allow_nan=False``; a received message holding one is answered ``-32700``).

The I-JSON domain is ``spec/identity.md`` section 1: finite numbers, integer literals that read
without rounding (the literal is the canonical form of the number it reads as), strings without
lone surrogates, and objects whose keys are strings, unique within the object. A message also
nests no deeper than the engine reads: no value inside more than :data:`MAX_DEPTH` lists and
objects, the message itself counted.
"""

from __future__ import annotations

import json
import re
from decimal import Decimal
from typing import Any, BinaryIO

PROTOCOL = "fx-node-protocol-v1"

#: Error codes the host sends (section 7).
PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
NODE_FAILURE = -32000
NODE_ERROR = -32001
CANCELLED = -32002
PROTOCOL_MISMATCH = -32003
LOAD_FAILED = -32004
BUILD_FAILED = -32005
INTERNAL = -32099
#: Every code of section 7. An error a body lets propagate keeps its code only when it is one.
ERROR_CODES = frozenset(
    {
        PARSE_ERROR,
        INVALID_REQUEST,
        METHOD_NOT_FOUND,
        INVALID_PARAMS,
        NODE_FAILURE,
        NODE_ERROR,
        CANCELLED,
        PROTOCOL_MISMATCH,
        LOAD_FAILED,
        BUILD_FAILED,
        *range(-32017, -32009),  # capability_undeclared .. job_unsettled
        *range(-32024, -32019),  # agent_unfinished .. unknown_file
        INTERNAL,
    }
)

#: The deepest a value may sit in a message: inside at most this many lists and objects, the
#: message's own object counted (the engine's reader refuses anything deeper).
MAX_DEPTH = 512

#: The longest header line read; a longer one breaks the transport.
_MAX_HEADER_LINE = 8192
_CONTENT_LENGTH = re.compile(rb"[ \t]*([0-9]{1,15})[ \t]*")
_SURROGATE = re.compile("[\ud800-\udfff]")
#: An integer literal longer than this cannot be canonical (at most 21 digits and a sign).
_LONGEST_INTEGER = 22


def read_message(stream: BinaryIO) -> bytes | None:
    """Reads one frame's body; ``None`` at a clean end of stream."""
    length: int | None = None
    first = True
    while True:
        line = stream.readline(_MAX_HEADER_LINE + 1)
        if not line:
            if first:
                return None
            raise ProtocolError("the stream ended inside a message header")
        first = False
        if len(line) > _MAX_HEADER_LINE:
            raise ProtocolError("a message header line is too long")
        if not line.endswith(b"\n"):
            raise ProtocolError("the stream ended inside a message header")
        line = line[:-1]
        if line.endswith(b"\r"):
            line = line[:-1]
        if not line:
            break
        name, colon, value = line.partition(b":")
        if not colon:
            raise ProtocolError(f"a message header is not a field: {line[:80]!r}")
        if name.strip().lower() != b"content-length":
            continue  # other header fields are ignored
        match = _CONTENT_LENGTH.fullmatch(value)
        if match is None:
            raise ProtocolError(f"Content-Length is not a byte count: {value[:80]!r}")
        found = int(match[1])
        if length is not None and found != length:
            raise ProtocolError("a message header gives two different lengths")
        length = found
    if length is None:
        raise ProtocolError("a message header has no Content-Length")
    body = bytearray()
    while len(body) < length:
        chunk = stream.read(length - len(body))
        if not chunk:
            raise ProtocolError(f"the stream ended after {len(body)} of a message's {length} bytes")
        body += chunk
    return bytes(body)


def write_message(stream: BinaryIO, message: dict[str, Any]) -> None:
    """Writes one message as a frame (compact JSON, ``ensure_ascii=False``) and flushes."""
    body = encode_message(message)
    stream.write(b"Content-Length: " + str(len(body)).encode("ascii") + b"\r\n\r\n" + body)
    stream.flush()


def encode_message(message: Any) -> bytes:
    """A message's frame body. ``ValueError`` when the message is not an I-JSON value."""
    check_value(message)
    text = json.dumps(message, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
    return text.encode("utf-8")


def parse_message(body: bytes) -> Any:
    """A frame body as a JSON value; :class:`ProtocolError` for anything outside I-JSON."""
    try:
        text = body.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ProtocolError(f"the message is not UTF-8: {error}") from None
    try:
        value = json.loads(
            text,
            parse_constant=_refuse_constant,
            parse_int=_read_integer,
            parse_float=_read_float,
            object_pairs_hook=_unique_object,
        )
    except ProtocolError:
        raise
    except (ValueError, RecursionError) as error:
        raise ProtocolError(f"the message is not JSON: {error}") from None
    try:
        _check_strings(value)
    except RecursionError:
        raise ProtocolError("the message nests too deeply") from None
    return value


def check_value(value: Any, where: str = "", depth: int = 0) -> None:
    """Raises ``ValueError`` unless ``value`` is an I-JSON value built from ``dict``, ``list``,
    ``tuple``, ``str``, ``int``, ``float``, ``bool`` and ``None`` that the engine can read where it
    goes: ``depth`` is how many lists and objects of its message hold it (0 for a whole message;
    1 for a request's params), and nothing in it may sit deeper than :data:`MAX_DEPTH`. A value
    that holds itself is refused. The message names where, as a JSON pointer below ``where``."""
    try:
        _check(value, where, depth, set())
    except RecursionError:
        raise ValueError(f"{_at(where)}the value nests too deeply") from None


def _check(value: Any, where: str, depth: int, holding: set[int]) -> None:
    """``holding``: the ids of the lists and objects ``value`` sits in."""
    if value is None or isinstance(value, bool):
        return
    if isinstance(value, str):
        if _SURROGATE.search(value):
            raise ValueError(f"{_at(where)}a string holds a lone surrogate")
        return
    if isinstance(value, int):
        if not _canonical_integer(value):
            raise ValueError(f"{_at(where)}the integer {_short(value)} cannot be read exactly")
        return
    if isinstance(value, float):
        if value != value or value in (float("inf"), float("-inf")):
            raise ValueError(f"{_at(where)}{value!r} is not a JSON number")
        return
    if not isinstance(value, dict | list | tuple):
        raise ValueError(f"{_at(where)}a {type(value).__name__} is not a JSON value")
    if id(value) in holding:
        raise ValueError(f"{_at(_short_pointer(where))}the value holds itself")
    if value and depth >= MAX_DEPTH:
        raise ValueError(
            f"{_at(_short_pointer(where))}the value nests deeper than the {MAX_DEPTH} levels "
            "the engine reads"
        )
    holding.add(id(value))
    if isinstance(value, dict):
        for key, item in value.items():
            if not isinstance(key, str):
                raise ValueError(f"{_at(where)}the key {key!r} is not a string")
            if _SURROGATE.search(key):
                raise ValueError(f"{_at(where)}a key holds a lone surrogate")
            _check(item, f"{where}/{_pointer(key)}", depth + 1, holding)
    else:
        for index, item in enumerate(value):
            _check(item, f"{where}/{index}", depth + 1, holding)
    holding.discard(id(value))


class ProtocolError(Exception):
    """A frame or message that breaks the transport."""


def _at(where: str) -> str:
    return f"at {where}: " if where else ""


def _short_pointer(where: str, keep: int = 4) -> str:
    """A JSON pointer cut to its first ``keep`` segments (``/value/0/0/0/…``)."""
    segments = where.split("/")[1:]
    if len(segments) <= keep:
        return where
    return "/" + "/".join(segments[:keep]) + "/…"


def _pointer(key: str) -> str:
    return key.replace("~", "~0").replace("/", "~1")


def _short(value: int) -> str:
    return str(value) if abs(value) < 10**40 else f"of {value.bit_length()} bits"


def _canonical_integer(value: int) -> bool:
    """Whether an integer's digits are the canonical form of the number they read as."""
    if abs(value) >= 10**21:
        return False
    if abs(value) <= 2**53:
        return True
    shortest = format(Decimal(repr(float(value))), "f")
    return shortest == str(value)


def _refuse_constant(name: str) -> Any:
    raise ProtocolError(f"the message holds {name}, which is not a JSON number")


def _read_integer(text: str) -> int:
    if len(text) > _LONGEST_INTEGER or not _canonical_integer(int(text)):
        raise ProtocolError(f"the integer {text[:40]} cannot be read without rounding")
    return int(text)


def _read_float(text: str) -> float:
    value = float(text)
    if value in (float("inf"), float("-inf")):
        raise ProtocolError(f"the number {text[:40]} is too large")
    return value


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ProtocolError(f"the message repeats the key {key!r} in one object")
        result[key] = value
    return result


def _check_strings(value: Any) -> None:
    if isinstance(value, str):
        if _SURROGATE.search(value):
            raise ProtocolError("the message holds a lone surrogate")
    elif isinstance(value, dict):
        for key, item in value.items():
            _check_strings(key)
            _check_strings(item)
    elif isinstance(value, list):
        for item in value:
            _check_strings(item)
