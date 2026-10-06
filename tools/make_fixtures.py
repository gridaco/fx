"""Synthesizes the media fixtures that pin spec/facts.md, and checks them.

Every fixture is made by this script from first principles, never generated art: WAV files with
Python's standard library (``wave`` and ``struct``), video with ffmpeg's ``testsrc`` and ``sine``
sources. The committed bytes are the fixtures: another ffmpeg, x264 or libvpx build writes other
bytes, so ``--check`` never regenerates anything. It re-reads the committed files with the readers
below, which implement spec/facts.md in Python with no binary (WAV through ``wave`` itself), and
compares what they read with each folder's ``expected.json``.

Generating (it needs ``ffmpeg`` and ``ffprobe`` on PATH) rewrites spec/vectors/facts/{wav,mp4,
matroska}/ and the two media files of conformance/facts-media/in/, and cross-checks every fixture
against what stage-gen's engine read from it (Python's ``wave`` for WAV, ``ffprobe`` for video;
spec/facts.md section 7). A fixture whose facts differ from that reading fails the run unless
DIFFERENCES lists it with the section of spec/facts.md that makes the difference deliberate.

Run from the repository root:

    uv run --project python python tools/make_fixtures.py            # generate
    uv run --project python python tools/make_fixtures.py --check    # re-read and compare

``expected.json`` maps each file name to its facts (``bytes``, ``kind``, then the members
spec/facts.md gives it, in that order), or to ``{"refused": "<message>"}`` for a file whose facts
are refused.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import shutil
import struct
import subprocess
import sys
import tempfile
import wave
import zlib
from collections.abc import Callable, Iterator
from fractions import Fraction
from math import floor, gcd
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parent.parent
FACTS = REPO / "spec" / "vectors" / "facts"
FOLDERS = ("wav", "mp4", "matroska")
EXPECTED = "expected.json"
# The conformance case facts-media plans with two of the fixtures, copied into its project.
CASE = REPO / "conformance" / "facts-media" / "in"
CASE_COPIES = {"voice.wav": "wav/pcm16_8k.wav", "clip.mp4": "mp4/h264_2997.mp4"}

# identity.md section 4, for the suffixes the fixtures use; any other suffix is kind `file`.
KINDS = {
    ".wav": "audio/wav",
    ".mp4": "video/mp4",
    ".webm": "video/webm",
    ".mkv": "video/x-matroska",
}


def kind_of(name: str) -> str:
    suffix = Path(name).suffix.lower()
    return KINDS.get(suffix, "file")


def round6(x: float) -> float:
    """Six decimal places, ties to even on the exact binary value (spec/facts.md section 1)."""

    return round(x, 6)


def plain_number(x: float | int) -> float | int:
    """A whole number as an int, so expected.json writes `24`, not `24.0` (identity.md 1)."""

    if isinstance(x, float) and x.is_integer() and abs(x) <= 2**53:
        return int(x)
    return x


class Refusal(Exception):
    """A file whose facts are refused; the message is the reason in parentheses."""


# ---------------------------------------------------------------------------------------------
# WAV (spec/facts.md section 3): the rule is what Python's `wave` reads, with every error
# (including those stage-gen's engine did not catch) giving no duration.


def wav_duration(data: bytes) -> float | None:
    try:
        with wave.open(io.BytesIO(data)) as clip:
            return round6(clip.getnframes() / clip.getframerate())
    except Exception:  # every failure means "no duration"
        return None


def wav_facts(data: bytes) -> dict[str, Any]:
    duration = wav_duration(data)
    return {} if duration is None else {"duration": duration}


# ---------------------------------------------------------------------------------------------
# MP4 (spec/facts.md section 4)

NO_ALPHA_ENTRIES = {
    b"avc1",
    b"avc2",
    b"avc3",
    b"avc4",
    b"hvc1",
    b"hev1",
    b"av01",
    b"vp08",
    b"vp09",
}
# esds objectTypeIndication of an `mp4v` entry: MPEG-4, MPEG-2 and MPEG-1 Visual, JPEG; and PNG.
NO_ALPHA_OBJECTS = {0x20, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x6A, 0x6C}
PNG_OBJECT = 0x6D


def _boxes(data: bytes, start: int, end: int, top: bool) -> Iterator[tuple[bytes, int, int]]:
    """(type, content start, end) of each box laid end to end in data[start:end]."""

    at = start
    while at < end:
        if end - at < 8:
            raise Refusal(f"a box header at byte {at} is cut short")
        size, kind = struct.unpack_from(">I4s", data, at)
        header = 8
        if size == 1:
            if end - at < 16:
                raise Refusal(f"a box header at byte {at} is cut short")
            size = struct.unpack_from(">Q", data, at + 8)[0]
            header = 16
        elif size == 0:
            size = end - at
        if size < header:
            raise Refusal(f"a box at byte {at} is smaller than its header")
        if size > end - at:
            where = "the file" if top else "its parent box"
            raise Refusal(f"a box at byte {at} runs past the end of {where}")
        yield kind, at + header, at + size
        at += size


def _first_children(data: bytes, start: int, end: int) -> dict[bytes, tuple[int, int]]:
    first: dict[bytes, tuple[int, int]] = {}
    for kind, s, e in _boxes(data, start, end, False):
        first.setdefault(kind, (s, e))
    return first


def _fields(data: bytes, box: tuple[int, int], offset: int, fmt: str, kind: str) -> tuple:
    s, e = box
    if offset + struct.calcsize(fmt) > e - s:
        raise Refusal(f"its {kind} box is too short")
    return struct.unpack_from(fmt, data, s + offset)


def _timescale(data: bytes, box: tuple[int, int], kind: str) -> int:
    """The timescale of an mvhd or mdhd box (content offset 20 in version 1, else 12)."""

    (version,) = _fields(data, box, 0, ">B", kind)
    return _fields(data, box, 20 if version == 1 else 12, ">I", kind)[0]


def _scale_and_duration(data: bytes, box: tuple[int, int], kind: str) -> tuple[int, int, int]:
    """(timescale, duration, the duration that means unknown) of an mdhd box."""

    (version,) = _fields(data, box, 0, ">B", kind)
    if version == 1:
        scale, duration = _fields(data, box, 20, ">IQ", kind)
        return scale, duration, 2**64 - 1
    scale, duration = _fields(data, box, 12, ">II", kind)
    return scale, duration, 2**32 - 1


class Rate:
    """The sample durations of a track (spec/facts.md 4.3): their sum, and the runs of those above
    0 that its frame rate is read from."""

    def __init__(self) -> None:
        self.total = 0
        self.runs: list[list[int]] = []

    def add(self, count: int, delta: int) -> None:
        self.total += count * delta
        if not count or not delta:
            return
        if self.runs and self.runs[-1][1] == delta:
            self.runs[-1][0] += count
        else:
            self.runs.append([count, delta])

    def fps(self, scale: int) -> float | None:
        samples = sum(count for count, _ in self.runs)
        if not scale or not samples:
            return None
        first = self.runs[0][1]
        if samples == 1:
            return ratio(scale, first)
        runs = [list(run) for run in self.runs]
        # The last duration says only when the track ends.
        runs[-1][0] -= 1
        if not runs[-1][0]:
            runs.pop()
        if samples - 1 >= 2:
            second = runs[0][1] if runs[0][0] > 1 else runs[1][1]
            if 2 * abs(first - second) > second:
                runs[0][0] -= 1
                if not runs[0][0]:
                    runs.pop(0)
        count = sum(c for c, _ in runs)
        if count >= 2**64:
            return None
        total = sum(c * d for c, d in runs)
        smallest = min(d for _, d in runs)
        divisor = 0
        for _, d in runs:
            divisor = gcd(divisor, d)
        if smallest == divisor:
            return ratio(scale, smallest)
        return rounded_rate(scale, count, total, divisor)


def ratio(p: int, q: int) -> float:
    """p/q with each converted to binary64 and divided, rounded to 6 places (spec/facts.md 1)."""

    return round6(float(p) / float(q))


def rounded_rate(scale: int, count: int, total: int, grid: int) -> float:
    """The rate of `count` durations summing to `total` that are a constant rate rounded to a grid
    of `grid` units (spec/facts.md 4.3): of the rates strictly between scale*count/(total+grid)
    and scale*count/(total-grid), the whole number nearest scale*count/total, else the multiple of
    1000/1001 nearest it, else the fraction with the smallest denominator."""

    units = scale * count
    low, high = total + grid, total - grid
    whole = _nearest_whole(units, total, low, high)
    if whole is not None:
        return ratio(whole, 1)
    k = _nearest_whole(units * 1001, total * 1000, low * 1000, high * 1000)
    if k is not None:
        return ratio(1000 * k, 1001)
    p, q = _simplest(Fraction(units, low), Fraction(units, high))
    return ratio(p, q)


def _nearest_whole(n: int, mid: int, low: int, high: int) -> int | None:
    """The whole number strictly between n/low and n/high nearest n/mid, the smaller of two
    equally near; None when there is none. n/mid lies between the bounds, so the nearest whole
    number inside is one of the two around it."""

    lo, hi, mean = Fraction(n, low), Fraction(n, high), Fraction(n, mid)
    inside = [k for k in (floor(mean), floor(mean) + 1) if lo < k < hi]
    return min(inside, key=lambda k: (abs(k - mean), k)) if inside else None


def _simplest(lo: Fraction, hi: Fraction) -> tuple[int, int]:
    """The fraction with the smallest denominator strictly between lo and hi (0 <= lo < hi),
    as (numerator, denominator): the smallest whole number above lo when it lies below hi, else
    the whole part k of lo followed by the simplest fraction between the reciprocals of what is
    left over."""

    whole = floor(lo) + 1
    if whole < hi:
        return whole, 1
    k = floor(lo)
    if lo == k:
        m = floor(1 / (hi - k)) + 1
        return k * m + 1, m
    p, q = _simplest(1 / (hi - k), 1 / (lo - k))
    return k * p + q, p


def png_has_alpha(frame: bytes) -> bool | None:
    """The image `has_alpha` of a PNG frame: a color type with alpha (4 or 6) or a tRNS chunk
    before the first IDAT. An ancillary chunk failing its CRC is ignored. None when the chunks up
    to the first IDAT do not read: a bad signature, no IHDR first, a chunk cut short, a critical
    chunk failing its CRC (as the engine's PNG decoder refuses them; it also checks IHDR's
    values, which this reader does not)."""

    if frame[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    at = 8
    color = None
    trns = False
    while at + 8 <= len(frame):
        length, kind = struct.unpack_from(">I4s", frame, at)
        if kind == b"IDAT":
            return None if color is None else color in (4, 6) or trns
        end = at + 12 + length
        if end > len(frame):
            return None
        body = frame[at + 8 : end - 4]
        critical = not kind[0] & 0x20
        if zlib.crc32(kind + body) != struct.unpack_from(">I", frame, end - 4)[0]:
            if critical:
                return None
            at = end
            continue
        if color is None and kind != b"IHDR":
            return None
        if kind == b"IHDR":
            if length != 13:
                return None
            color = body[9]
        elif kind == b"tRNS":
            trns = True
        at = end
    return None


def mp4_facts(data: bytes) -> dict[str, Any]:
    try:
        return _mp4(data)
    except Refusal as refusal:
        raise Refusal(f"not an MP4 file ({refusal})") from None


def _mp4(data: bytes) -> dict[str, Any]:
    moov = None
    moofs = []
    for kind, s, e in _boxes(data, 0, len(data), True):
        if kind == b"moov" and moov is None:
            moov = (s, e)
        elif kind == b"moof":
            moofs.append((s, e))
    if moov is None:
        raise Refusal("it has no moov box")
    mvhd = None
    mvex = None
    video = None
    for kind, s, e in _boxes(data, *moov, False):
        if kind == b"mvhd" and mvhd is None:
            mvhd = (s, e)
        elif kind == b"mvex" and mvex is None:
            mvex = (s, e)
        elif kind == b"trak" and video is None:
            video = _video_trak(data, s, e)
    if video is None:
        return {}
    children = video["children"]
    mdia = video["mdia"]
    if b"mdhd" not in mdia:
        raise Refusal("its video track has no mdhd box")
    scale, media_duration, unknown = _scale_and_duration(data, mdia[b"mdhd"], "mdhd")
    stbl: dict[bytes, tuple[int, int]] = {}
    if b"minf" in mdia:
        minf = _first_children(data, *mdia[b"minf"])
        if b"stbl" in minf:
            stbl = _first_children(data, *minf[b"stbl"])
    if b"stsd" not in stbl:
        raise Refusal("its video track has no stsd box")
    stsd = stbl[b"stsd"]
    (entries,) = _fields(data, stsd, 4, ">I", "stsd")
    entry = next(iter(_boxes(data, stsd[0] + 8, stsd[1], False)), None) if entries else None
    if entry is None:
        raise Refusal("its video track has no sample entry")
    fourcc, es, ee = entry
    if ee - es < 78:
        raise Refusal("its video sample entry is too short")
    width, height = struct.unpack_from(">HH", data, es + 24)

    has_alpha = None
    png = fourcc == b"png "
    if fourcc in NO_ALPHA_ENTRIES:
        has_alpha = False
    elif fourcc == b"mp4v":
        object_type = _object_type(data, es + 78, ee)
        if object_type in NO_ALPHA_OBJECTS:
            has_alpha = False
        png = object_type == PNG_OBJECT

    rate = Rate()
    if b"stts" in stbl:
        (count,) = _fields(data, stbl[b"stts"], 4, ">I", "stts")
        for i in range(count):
            rate.add(*_fields(data, stbl[b"stts"], 8 + 8 * i, ">II", "stts"))
    frames = 0
    first_size = None
    if b"stsz" in stbl:
        size, frames = _fields(data, stbl[b"stsz"], 4, ">II", "stsz")
        # The table's first entry is read only for a PNG track's first sample (4.4).
        if png and frames:
            first_size = size or _fields(data, stbl[b"stsz"], 12, ">I", "stsz")[0]
    elif b"stz2" in stbl:
        (frames,) = _fields(data, stbl[b"stz2"], 8, ">I", "stz2")

    track_id = None
    fragmented = False
    if b"tkhd" in children:
        (version,) = _fields(data, children[b"tkhd"], 0, ">B", "tkhd")
        offset = 20 if version == 1 else 12
        (track_id,) = _fields(data, children[b"tkhd"], offset, ">I", "tkhd")
    if track_id is not None:
        trex_duration = 0
        if mvex is not None:
            for kind, s, e in _boxes(data, *mvex, False):
                if kind == b"trex" and _fields(data, (s, e), 4, ">I", "trex")[0] == track_id:
                    # track_ID, default_sample_description_index, default_sample_duration
                    (trex_duration,) = _fields(data, (s, e), 12, ">I", "trex")
                    break
        for moof in moofs:
            for kind, s, e in _boxes(data, *moof, False):
                if kind == b"traf":
                    matched, count = _traf(data, s, e, track_id, trex_duration, rate)
                    fragmented = fragmented or matched
                    frames += count

    if not fragmented and 0 < media_duration < unknown:
        units = media_duration
    else:
        units = rate.total
    edits = _edits(data, children, scale, mvhd)
    if edits is not None:
        units = min(units, edits)

    facts: dict[str, Any] = {"width": width, "height": height}
    fps = rate.fps(scale)
    if fps is not None:
        facts["fps"] = fps
    if scale:
        facts["duration"] = round6(float(units) / scale)
    facts["frames"] = frames
    if has_alpha is not None:
        facts["has_alpha"] = has_alpha
    if first_size is not None:
        offset = None
        if b"stco" in stbl:
            (n,) = _fields(data, stbl[b"stco"], 4, ">I", "stco")
            if n:
                (offset,) = _fields(data, stbl[b"stco"], 8, ">I", "stco")
        elif b"co64" in stbl:
            (n,) = _fields(data, stbl[b"co64"], 4, ">I", "co64")
            if n:
                (offset,) = _fields(data, stbl[b"co64"], 8, ">Q", "co64")
        if offset is not None and offset + first_size <= len(data):
            alpha = png_has_alpha(data[offset : offset + first_size])
            if alpha is not None:
                facts["has_alpha"] = alpha
    return facts


def _object_type(data: bytes, start: int, end: int) -> int | None:
    """The objectTypeIndication in the first esds box among a sample entry's child boxes, or
    None when there is none or the boxes or descriptors do not parse."""

    try:
        esds = next(((s, e) for kind, s, e in _boxes(data, start, end, False) if kind == b"esds"))
    except (Refusal, StopIteration):
        return None
    body = data[esds[0] : esds[1]]

    def descriptor(at: int) -> tuple[int, int] | None:
        """(tag, content start) of the descriptor at `at`."""

        if at >= len(body):
            return None
        tag = body[at]
        at += 1
        for _ in range(4):
            if at >= len(body):
                return None
            at += 1
            if not body[at - 1] & 0x80:
                break
        return tag, at

    found = descriptor(4)
    if found is None or found[0] != 0x03:
        return None
    at = found[1] + 3
    if at > len(body):
        return None
    flags = body[at - 1]
    if flags & 0x80:
        at += 2
    if flags & 0x40:
        if at >= len(body):
            return None
        at += 1 + body[at]
    if flags & 0x20:
        at += 2
    found = descriptor(at)
    if found is None or found[0] != 0x04 or found[1] >= len(body):
        return None
    return body[found[1]]


def _video_trak(data: bytes, s: int, e: int) -> dict[str, Any] | None:
    children = _first_children(data, s, e)
    if b"mdia" not in children:
        return None
    mdia = _first_children(data, *children[b"mdia"])
    if b"hdlr" not in mdia:
        return None
    (handler,) = _fields(data, mdia[b"hdlr"], 8, ">4s", "hdlr")
    if handler != b"vide":
        return None
    return {"children": children, "mdia": mdia}


def _edits(
    data: bytes,
    children: dict[bytes, tuple[int, int]],
    scale: int,
    mvhd: tuple[int, int] | None,
) -> int | None:
    """The non-empty edits' durations rescaled to the media timescale, or None. mvhd's timescale
    is read only here, for a track with an edts; its duration never (4.3)."""

    if b"edts" not in children or mvhd is None:
        return None
    movie_scale = _timescale(data, mvhd, "mvhd")
    if not movie_scale:
        return None
    edts = _first_children(data, *children[b"edts"])
    if b"elst" not in edts:
        return None
    elst = edts[b"elst"]
    version, count = _fields(data, elst, 0, ">B3xI", "elst")
    total = 0
    found = False
    for i in range(count):
        if version == 1:
            duration, media_time = _fields(data, elst, 8 + 20 * i, ">Qq", "elst")
        else:
            duration, media_time = _fields(data, elst, 8 + 12 * i, ">Ii", "elst")
        if media_time != -1 and duration > 0:
            found = True
            total += (duration * scale + movie_scale // 2) // movie_scale
    return total if found else None


def _traf(
    data: bytes, s: int, e: int, track_id: int, trex_duration: int, rate: Rate
) -> tuple[bool, int]:
    """(whether the fragment belongs to the track, its frames)."""

    children = list(_boxes(data, s, e, False))
    tfhd = next(((cs, ce) for kind, cs, ce in children if kind == b"tfhd"), None)
    if tfhd is None:
        return False, 0
    flags, tid = _fields(data, tfhd, 0, ">II", "tfhd")
    flags &= 0xFFFFFF
    if tid != track_id:
        return False, 0
    default = trex_duration
    if flags & 0x08:
        offset = 8 + (8 if flags & 0x01 else 0) + (4 if flags & 0x02 else 0)
        (default,) = _fields(data, tfhd, offset, ">I", "tfhd")
    frames = 0
    for kind, cs, ce in children:
        if kind != b"trun":
            continue
        flags, count = _fields(data, (cs, ce), 0, ">II", "trun")
        flags &= 0xFFFFFF
        offset = 8 + (4 if flags & 0x001 else 0) + (4 if flags & 0x004 else 0)
        per_sample = 4 * bin(flags & 0xF00).count("1")
        if offset + count * per_sample > ce - cs:
            raise Refusal("its trun box is too short")
        frames += count
        if flags & 0x100:
            for i in range(count):
                (duration,) = _fields(data, (cs, ce), offset + i * per_sample, ">I", "trun")
                rate.add(1, duration)
        else:
            rate.add(count, default)
    return True, frames


# ---------------------------------------------------------------------------------------------
# Matroska and WebM (spec/facts.md section 5)

EBML = 0x1A45DFA3
SEGMENT = 0x18538067
INFO = 0x1549A966
TRACKS = 0x1654AE6B
CLUSTER = 0x1F43B675
TRACK_ENTRY = 0xAE
VIDEO = 0xE0
BLOCK_GROUP = 0xA0
BLOCK = 0xA1
SIMPLE_BLOCK = 0xA3
# The children of a Segment: an element of unknown size (a Cluster) ends where one of them starts.
LEVEL_ONE = {
    0x114D9B74,  # SeekHead
    INFO,
    TRACKS,
    CLUSTER,
    0x1C53BB6B,  # Cues
    0x1941A469,  # Attachments
    0x1043A770,  # Chapters
    0x1254C367,  # Tags
    EBML,
    SEGMENT,
}
NO_ALPHA_CODECS = {
    "V_VP8",
    "V_VP9",
    "V_AV1",
    "V_MPEG4/ISO/AVC",
    "V_MPEGH/ISO/HEVC",
    "V_MPEG4/ISO/SP",
    "V_MPEG4/ISO/ASP",
    "V_MPEG4/ISO/AP",
    "V_MPEG1",
    "V_MPEG2",
    "V_THEORA",
    "V_MJPEG",
}
PNG_FOURCCS = {b"MPNG", b"PNG1", b"png "}


class Short(Exception):
    """A variable-size integer that runs past the end of what holds it."""


class Invalid(Exception):
    """A variable-size integer whose first byte is 0."""


def _vint(data: bytes, at: int, end: int) -> tuple[int, int, bool]:
    """(value without its marker, length, all ones) of the variable-size integer at `at`."""

    if at >= end:
        raise Short
    first = data[at]
    length = 1
    while length <= 8 and not first & (0x80 >> (length - 1)):
        length += 1
    if length > 8:
        raise Invalid
    if at + length > end:
        raise Short
    value = first & (0xFF >> length)
    for byte in data[at + 1 : at + length]:
        value = value << 8 | byte
    return value, length, value == (1 << (7 * length)) - 1


def _element(data: bytes, at: int, end: int) -> tuple[int, int, int | None]:
    """(id with its marker, content start, size or None when unknown) of the element at `at`."""

    if at >= end:
        raise Refusal(f"an element header at byte {at} is cut short")
    first = data[at]
    length = 1
    while length <= 4 and not first & (0x80 >> (length - 1)):
        length += 1
    if length > 4:
        raise Refusal(f"an element ID at byte {at} is not valid")
    if at + length > end:
        raise Refusal(f"an element header at byte {at} is cut short")
    eid = int.from_bytes(data[at : at + length], "big")
    try:
        size, size_length, unknown = _vint(data, at + length, end)
    except Short:
        raise Refusal(f"an element header at byte {at} is cut short") from None
    except Invalid:
        raise Refusal(f"an element size at byte {at + length} is not valid") from None
    return eid, at + length + size_length, None if unknown else size


def _children(data: bytes, start: int, end: int) -> Iterator[tuple[int, int, int]]:
    """(id, content start, content end) of the elements of known size in data[start:end]."""

    at = start
    while at < end:
        eid, content, size = _element(data, at, end)
        if size is None:
            raise Refusal(f"an element at byte {at} has an unknown size")
        if size > end - content:
            raise Refusal(f"an element at byte {at} runs past the end of its parent")
        yield eid, content, content + size
        at = content + size


def _segment_children(data: bytes, start: int, end: int) -> Iterator[tuple[int, int, int]]:
    """Like _children, except that a Cluster of unknown size ends at the next level-1 element."""

    at = start
    while at < end:
        eid, content, size = _element(data, at, end)
        if size is None:
            if eid != CLUSTER:
                raise Refusal(f"an element at byte {at} has an unknown size")
            stop = content
            while stop < end:
                child, child_content, child_size = _element(data, stop, end)
                if child in LEVEL_ONE:
                    break
                if child_size is None:
                    raise Refusal(f"an element at byte {stop} has an unknown size")
                if child_size > end - child_content:
                    raise Refusal(f"an element at byte {stop} runs past the end of its parent")
                stop = child_content + child_size
            yield eid, content, stop
            at = stop
            continue
        if size > end - content:
            raise Refusal(f"an element at byte {at} runs past the end of its parent")
        yield eid, content, content + size
        at = content + size


def _uint(data: bytes, s: int, e: int) -> int:
    if e - s > 8:
        raise Refusal(f"an integer at byte {s} is longer than 8 bytes")
    return int.from_bytes(data[s:e], "big")


def _float(data: bytes, s: int, e: int) -> float:
    if e - s == 0:
        return 0.0
    if e - s == 4:
        return struct.unpack_from(">f", data, s)[0]
    if e - s == 8:
        return struct.unpack_from(">d", data, s)[0]
    raise Refusal(f"a float at byte {s} is {e - s} bytes long")


def _text(data: bytes, s: int, e: int) -> str:
    return data[s:e].rstrip(b"\0").decode("latin-1")


def matroska_facts(data: bytes) -> dict[str, Any]:
    try:
        return _matroska(data)
    except Refusal as refusal:
        raise Refusal(f"not a Matroska or WebM file ({refusal})") from None


def _matroska(data: bytes) -> dict[str, Any]:
    end = len(data)
    if data[:4] != EBML.to_bytes(4, "big"):
        raise Refusal("it does not start with an EBML header")
    eid, content, size = _element(data, 0, end)
    if size is None:
        raise Refusal("an element at byte 0 has an unknown size")
    if size > end - content:
        raise Refusal("an element at byte 0 runs past the end of the file")
    doctype = "matroska"
    for child, s, e in _children(data, content, content + size):
        if child == 0x4282:
            doctype = _text(data, s, e)
            break
    if doctype not in ("matroska", "webm"):
        raise Refusal("its DocType is not matroska or webm")
    at = content + size
    segment = None
    while at < end:
        eid, content, size = _element(data, at, end)
        if size is None and eid != SEGMENT:
            raise Refusal(f"an element at byte {at} has an unknown size")
        stop = end if size is None else content + size
        if stop > end:
            raise Refusal(f"an element at byte {at} runs past the end of the file")
        if eid == SEGMENT:
            segment = (content, stop)
            break
        at = stop
    if segment is None:
        raise Refusal("it has no Segment")

    scale = None
    duration = None
    track = None
    seen_info = seen_tracks = False
    for eid, s, e in _segment_children(data, *segment):
        if eid == INFO and not seen_info:
            seen_info = True
            for child, cs, ce in _children(data, s, e):
                if child == 0x2AD7B1 and scale is None:
                    scale = _uint(data, cs, ce)
                elif child == 0x4489 and duration is None:
                    duration = _float(data, cs, ce)
        elif eid == TRACKS and not seen_tracks:
            seen_tracks = True
            for child, cs, ce in _children(data, s, e):
                if child == TRACK_ENTRY:
                    entry = _track_entry(data, cs, ce)
                    if entry.get("type") == 1:
                        track = entry
                        break
    if track is None:
        return {}
    if "width" not in track or "height" not in track:
        raise Refusal("its video track has no pixel size")
    if max(track["width"], track["height"]) > 0xFFFFFFFF:
        raise Refusal("its video track's pixel size is too large")

    frames = 0
    first_frame = None
    number = track.get("number")
    for eid, s, e in _segment_children(data, *segment):
        if eid != CLUSTER:
            continue
        for child, cs, ce in _children(data, s, e):
            blocks = []
            if child == SIMPLE_BLOCK:
                blocks.append((cs, ce))
            elif child == BLOCK_GROUP:
                blocks.extend((bs, be) for b, bs, be in _children(data, cs, ce) if b == BLOCK)
            for bs, be in blocks:
                block_track, count, frame = _block(data, bs, be)
                if block_track == number:
                    frames += count
                    if first_frame is None:
                        first_frame = frame

    facts: dict[str, Any] = {"width": track["width"], "height": track["height"]}
    default = track.get("default_duration", 0)
    rate = nearest_fraction(Fraction(10**9, default), 1001) if default > 0 else Fraction(0)
    if rate > 0:
        facts["fps"] = round6(float(rate))
    if scale is None:
        scale = 1_000_000
    if (
        duration is not None
        and duration == duration
        and duration > 0
        and duration != float("inf")
        and scale > 0
    ):
        micros = duration * scale * 1000 / 1000000
        if micros != float("inf"):
            facts["duration"] = round6(int(micros) / 1e6)
    facts["frames"] = frames
    alpha = _codec_alpha(track, first_frame)
    if alpha is not None:
        facts["has_alpha"] = alpha
    return facts


TRACK_FIELDS = {0xD7: "number", 0x83: "type", 0x23E383: "default_duration"}
PIXEL_FIELDS = {0xB0: "width", 0xBA: "height"}


def _track_entry(data: bytes, s: int, e: int) -> dict[str, Any]:
    """What the facts need from a TrackEntry; the first of each element counts."""

    entry: dict[str, Any] = {}
    for child, cs, ce in _children(data, s, e):
        name = TRACK_FIELDS.get(child)
        if name is not None and name not in entry:
            entry[name] = _uint(data, cs, ce)
        elif child == 0x86 and "codec" not in entry:
            entry["codec"] = _text(data, cs, ce)
        elif child == 0x63A2 and "private" not in entry:
            entry["private"] = data[cs:ce]
        elif child == VIDEO and "video" not in entry:
            entry["video"] = True
            for item, vs, ve in _children(data, cs, ce):
                name = PIXEL_FIELDS.get(item)
                if name is not None and name not in entry:
                    entry[name] = _uint(data, vs, ve)
    return entry


def _block(data: bytes, s: int, e: int) -> tuple[int, int, bytes]:
    """(track number, frames, the first frame's bytes) of a SimpleBlock or Block."""

    try:
        return _laced(data, s, e)
    except Short:
        raise Refusal(f"a block at byte {s} is too short") from None
    except Invalid:
        raise Refusal(f"a block at byte {s} is not valid") from None


def _laced(data: bytes, s: int, e: int) -> tuple[int, int, bytes]:
    track, length, _ = _vint(data, s, e)
    at = s + length + 3
    if at > e:
        raise Short
    lacing = (data[at - 1] >> 1) & 3
    if lacing == 0:
        return track, 1, data[at:e]
    if at >= e:
        raise Short
    count = data[at] + 1
    at += 1
    if count == 1:
        return track, 1, data[at:e]
    if lacing == 2:  # fixed-size lacing
        return track, count, data[at : at + (e - at) // count]
    if lacing == 1:  # Xiph lacing: each size is a run of 255s and a byte below 255
        first = None
        for _ in range(count - 1):
            size = 0
            while True:
                if at >= e:
                    raise Short
                size += data[at]
                at += 1
                if data[at - 1] != 255:
                    break
            if first is None:
                first = size
    else:  # EBML lacing: the first size, then count - 2 signed differences
        first, length, _ = _vint(data, at, e)
        at += length
        for _ in range(count - 2):
            _, length, _ = _vint(data, at, e)
            at += length
    return track, count, data[at : min(e, at + first)]


def nearest_fraction(x: Fraction, limit: int) -> Fraction:
    """The fraction closest to x whose denominator is at most `limit`; of two equally close,
    the one with the smaller denominator (spec/facts.md 5.2)."""

    if x.denominator <= limit:
        return x
    p0, q0, p1, q1 = 0, 1, 1, 0
    n, d = x.numerator, x.denominator
    while True:
        a = n // d
        q2 = q0 + a * q1
        if q2 > limit:
            break
        p0, q0, p1, q1 = p1, q1, p0 + a * p1, q2
        n, d = d, n - a * d
    k = (limit - q0) // q1
    semi = Fraction(p0 + k * p1, q0 + k * q1)
    convergent = Fraction(p1, q1)
    a_err, b_err = abs(semi - x), abs(convergent - x)
    if a_err < b_err or (a_err == b_err and semi.denominator < convergent.denominator):
        return semi
    return convergent


def _codec_alpha(track: dict[str, Any], first_frame: bytes | None) -> bool | None:
    codec = track.get("codec", "")
    private = track.get("private", b"")
    if codec in NO_ALPHA_CODECS:
        return False
    if codec == "V_FFV1":
        return ffv1_alpha(private, first_frame)
    if codec == "V_MS/VFW/FOURCC" and len(private) >= 40:
        fourcc = private[16:20]
        if fourcc in PNG_FOURCCS:
            return None if first_frame is None else png_has_alpha(first_frame)
        if fourcc == b"FFV1":
            header = struct.unpack_from("<I", private, 0)[0]
            extra = private[header:] if 40 <= header <= len(private) else b""
            return ffv1_alpha(extra, first_frame)
    return None


# FFV1 (RFC 9043): the transparency flag of a configuration record (version 2 and later) or of a
# version 0 or 1 key frame's header, read with the range coder and its default state table.


def _rac_states() -> tuple[list[int], list[int]]:
    factor = int(0.05 * (1 << 32))
    max_p = 256 - 8
    one = 1 << 32
    one_state = [0] * 256
    zero_state = [0] * 256
    last_p8 = 0
    p = one // 2
    for _ in range(128):
        p8 = (256 * p + one // 2) >> 32
        if p8 <= last_p8:
            p8 = last_p8 + 1
        if last_p8 and last_p8 < 256 and p8 <= max_p:
            one_state[last_p8] = p8
        p += ((one - p) * factor + one // 2) >> 32
        last_p8 = p8
    for i in range(256 - max_p, max_p + 1):
        if one_state[i]:
            continue
        p = (i * one + 128) >> 8
        p += ((one - p) * factor + one // 2) >> 32
        p8 = (256 * p + one // 2) >> 32
        if p8 <= i:
            p8 = i + 1
        if p8 > max_p:
            p8 = max_p
        one_state[i] = p8
    for i in range(1, 255):
        zero_state[i] = 256 - one_state[256 - i]
    return one_state, zero_state


ONE_STATE, ZERO_STATE = _rac_states()


class RangeDecoder:
    def __init__(self, data: bytes) -> None:
        self.data = data
        self.end = len(data)
        self.range = 0xFF00
        self.low = (data[0] << 8 | data[1]) if len(data) >= 2 else 0
        self.at = 2
        self.overread = 0
        if len(data) < 2:
            self.overread = 2
        if self.low >= 0xFF00:
            self.low = 0xFF00
            self.end = self.at

    def bit(self, states: list[int], i: int) -> int:
        split = (self.range * states[i]) >> 8
        self.range -= split
        if self.low < self.range:
            states[i] = ZERO_STATE[states[i]]
            result = 0
        else:
            self.low -= self.range
            states[i] = ONE_STATE[states[i]]
            self.range = split
            result = 1
        if self.range < 0x100:
            self.range <<= 8
            self.low <<= 8
            if self.at < self.end:
                self.low += self.data[self.at]
                self.at += 1
            else:
                self.overread += 1
        return result

    def symbol(self, states: list[int], signed: bool = False) -> int | None:
        if self.bit(states, 0):
            return 0
        e = 0
        while self.bit(states, 1 + min(e, 9)):
            e += 1
            if e > 31:
                return None
        a = 1
        for i in range(e - 1, -1, -1):
            a = 2 * a + self.bit(states, 22 + min(i, 9))
        if signed and self.bit(states, 11 + min(e, 10)):
            return -a
        return a


def _ffv1_header(rc: RangeDecoder, states: list[int], version: int) -> bool | None:
    """Reads from the coder field after `version` up to the transparency flag."""

    if version > 2:
        rc.end = max(rc.at, rc.end - 4)
        if rc.symbol(states) is None:  # micro_version
            return None
    coder = rc.symbol(states)
    if coder is None:
        return None
    if coder == 2:
        for _ in range(255):
            if rc.symbol(states, signed=True) is None:
                return None
    if rc.symbol(states) is None:  # colorspace
        return None
    if version > 0 and rc.symbol(states) is None:  # bits per raw sample
        return None
    rc.bit(states, 0)  # chroma planes
    if rc.symbol(states) is None or rc.symbol(states) is None:  # chroma shifts
        return None
    transparency = rc.bit(states, 0)
    return None if rc.overread > 2 else bool(transparency)


def _crc32(data: bytes) -> int:
    """FFV1's CRC-32: polynomial 0x04C11DB7, most significant bit first, from 0, not inverted."""

    crc = 0
    for byte in data:
        crc ^= byte << 24
        for _ in range(8):
            crc = ((crc << 1) ^ 0x04C11DB7) if crc & 0x80000000 else crc << 1
            crc &= 0xFFFFFFFF
    return crc


def ffv1_alpha(record: bytes, first_frame: bytes | None) -> bool | None:
    if record:
        rc = RangeDecoder(record)
        states = [128] * 32
        version = rc.symbol(states)
        if version not in (2, 3) or (version == 3 and (len(record) < 4 or _crc32(record))):
            return None
        return _ffv1_header(rc, states, version)
    if not first_frame:
        return None
    rc = RangeDecoder(first_frame)
    if not rc.bit([128], 0):  # not a key frame
        return None
    states = [128] * 32
    version = rc.symbol(states)
    if version is None or version > 1:
        return None
    return _ffv1_header(rc, states, version)


# ---------------------------------------------------------------------------------------------
# The facts of a file


def facts_of(name: str, data: bytes) -> dict[str, Any]:
    kind = kind_of(name)
    facts: dict[str, Any] = {"bytes": len(data), "kind": kind}
    try:
        if kind == "audio/wav":
            facts.update(wav_facts(data))
        elif kind == "video/mp4":
            facts.update(mp4_facts(data))
        elif kind in ("video/webm", "video/x-matroska"):
            facts.update(matroska_facts(data))
    except Refusal as refusal:
        return {"refused": str(refusal)}
    return {key: plain_number(value) for key, value in facts.items()}


# ---------------------------------------------------------------------------------------------
# What stage-gen's engine read (for the cross-check while generating)

ALPHA_FORMATS = {"rgba", "bgra", "argb", "abgr", "yuva420p", "yuva422p", "yuva444p", "gbrap", "ya8"}


def predecessor_facts(name: str, path: Path) -> dict[str, Any]:
    """Facts as stage-gen's engine computed them, or {"crashed": ...} where it raised."""

    data = path.read_bytes()
    kind = kind_of(name)
    facts: dict[str, Any] = {"bytes": len(data), "kind": kind}
    if kind == "audio/wav":
        try:
            with wave.open(io.BytesIO(data)) as clip:
                facts["duration"] = round(clip.getnframes() / clip.getframerate(), 6)
        except wave.Error:
            pass
        except Exception as error:  # it did not catch these
            return {"crashed": type(error).__name__}
    elif kind.startswith("video"):
        facts.update(_ffprobe_facts(path))
    return {key: plain_number(value) for key, value in facts.items()}


def _ffprobe_facts(path: Path) -> dict[str, Any]:
    command = [
        "ffprobe",
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-count_packets",
        "-show_entries",
        "stream=width,height,r_frame_rate,duration,nb_read_packets,pix_fmt:format=duration",
        "-of",
        "json",
        str(path),
    ]
    try:
        result = subprocess.run(command, capture_output=True, timeout=120, check=True)
        probe = json.loads(result.stdout)
        stream = probe["streams"][0]
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, IndexError):
        return {}
    facts: dict[str, Any] = {}
    if isinstance(stream.get("width"), int) and isinstance(stream.get("height"), int):
        facts["width"], facts["height"] = stream["width"], stream["height"]
    try:
        rate = Fraction(str(stream["r_frame_rate"]))
        if rate > 0:
            facts["fps"] = round(float(rate), 6)
    except (KeyError, ValueError, ZeroDivisionError):
        pass
    duration = stream.get("duration") or probe.get("format", {}).get("duration")
    with contextlib.suppress(TypeError, ValueError):
        facts["duration"] = round(float(duration), 6)
    if str(stream.get("nb_read_packets", "")).isdigit():
        facts["frames"] = int(stream["nb_read_packets"])
    if isinstance(stream.get("pix_fmt"), str):
        facts["has_alpha"] = stream["pix_fmt"] in ALPHA_FORMATS
    return facts


# Fixtures whose facts differ from what stage-gen's engine read, each with the reason. Every
# entry must differ, and every other fixture must agree.
DIFFERENCES = {
    "wav/empty.wav": "it raised EOFError; FX gives no duration (facts.md 3)",
    "wav/rate_zero.wav": "it raised ZeroDivisionError; FX gives no duration (facts.md 3)",
    "wav/truncated_fmt.wav": "it raised EOFError; FX gives no duration (facts.md 3)",
    "wav/odd_chunk_unpadded.wav": "it raised RuntimeError; FX gives no duration (facts.md 3)",
    "mp4/truncated.mp4": "ffprobe failed and it gave no facts; FX refuses (facts.md 4.1)",
    "mp4/garbage.mp4": "ffprobe failed and it gave no facts; FX refuses (facts.md 4.1)",
    "matroska/truncated.mkv": "ffprobe read what was there; FX refuses (facts.md 5.1)",
    "matroska/garbage.webm": "ffprobe failed and it gave no facts; FX refuses (facts.md 5.1)",
    "matroska/vp9_5994.webm": "ffmpeg's 19001/317 (59.940063); FX's 60000/1001 (facts.md 5.2)",
    "matroska/png_rgba64.mkv": "rgba64be was not in its list of formats; FX reads the PNG (5.3)",
    "matroska/block_track_zero.mkv": "ffprobe skipped the broken block; FX refuses (5.2)",
    "mp4/mvhd_no_timescale.mp4": "ffprobe read past the mvhd; FX needs its timescale (4.1, 4.3)",
    "mp4/stsd_count_zero.mp4": "ffprobe found no codec and gave no facts; FX refuses (4.2)",
    "mp4/stsz_no_table.mp4": "ffprobe read one packet; FX reads only the sample count (4.3)",
}


# ---------------------------------------------------------------------------------------------
# WAV fixtures

PCM_GUID = bytes.fromhex("0100000000001000800000aa00389b71")
FLOAT_GUID = bytes.fromhex("0300000000001000800000aa00389b71")


def _pcm(rate: int, frames: int, channels: int = 1, width: int = 2) -> bytes:
    out = io.BytesIO()
    with wave.open(out, "wb") as clip:
        clip.setnchannels(channels)
        clip.setsampwidth(width)
        clip.setframerate(rate)
        clip.writeframes(b"\0" * (frames * channels * width))
    return out.getvalue()


def _chunk(cid: bytes, data: bytes, size: int | None = None, pad: bool = True) -> bytes:
    out = cid + struct.pack("<I", len(data) if size is None else size) + data
    return out + b"\0" if pad and len(data) % 2 else out


def _riff(*chunks: bytes, size: int | None = None, magic: bytes = b"RIFF") -> bytes:
    body = b"WAVE" + b"".join(chunks)
    return magic + struct.pack("<I", len(body) if size is None else size) + body


def _fmt(
    rate: int = 8000,
    channels: int = 1,
    bits: int = 16,
    tag: int = 1,
    subformat: bytes | None = None,
) -> bytes:
    width = (bits + 7) // 8
    data = struct.pack(
        "<HHIIHH", tag, channels, rate, rate * channels * width, channels * width, bits
    )
    if subformat is not None:
        data += struct.pack("<HHI", 22, bits, 3) + subformat
    return _chunk(b"fmt ", data)


def wav_fixtures() -> dict[str, bytes]:
    silence = b"\0" * 16
    return {
        "pcm16_8k.wav": _pcm(8000, 1000),
        "pcm16_44k.wav": _pcm(44100, 1234),
        "pcm16_48k_stereo.wav": _pcm(48000, 960, channels=2),
        "pcm8.wav": _pcm(4000, 2000, width=1),
        "pcm24.wav": _pcm(96000, 480, width=3),
        "one_frame.wav": _pcm(44100, 1),
        "three_hz.wav": _pcm(3, 1),
        "no_frames.wav": _pcm(22050, 0),
        "extensible_pcm.wav": _riff(
            _fmt(44100, channels=2, tag=0xFFFE, subformat=PCM_GUID),
            _chunk(b"data", b"\0" * (441 * 4)),
        ),
        "extensible_float.wav": _riff(
            _fmt(8000, bits=32, tag=0xFFFE, subformat=FLOAT_GUID),
            _chunk(b"data", silence),
        ),
        "float.wav": _riff(_fmt(8000, bits=32, tag=3), _chunk(b"data", silence)),
        "mp3_named.wav": b"ID3\x03\0\0\0\0\0\0" + b"\0" * 32,
        "empty.wav": b"",
        "rate_zero.wav": _riff(_fmt(0), _chunk(b"data", silence)),
        "truncated_fmt.wav": _riff(_chunk(b"fmt ", struct.pack("<HHI", 1, 1, 8000))),
        "declared_longer.wav": _riff(_fmt(48000, channels=2), _chunk(b"data", b"", size=96000 * 4)),
        "odd_chunk.wav": _riff(_chunk(b"LIST", b"abc"), _fmt(8000), _chunk(b"data", b"\0" * 800)),
        "odd_chunk_unpadded.wav": _riff(
            _chunk(b"LIST", b"abc", pad=False), _fmt(8000), _chunk(b"data", b"\0" * 800)
        ),
        "data_before_fmt.wav": _riff(_chunk(b"data", silence), _fmt(8000)),
        "rf64.wav": _riff(_fmt(8000), _chunk(b"data", silence), magic=b"RF64"),
        "zero_channels.wav": _riff(_fmt(8000, channels=0), _chunk(b"data", silence)),
        "partial_frame.wav": _riff(_fmt(8000), _chunk(b"data", b"\0" * 5)),
        "riff_size_cuts_data_header.wav": _riff(
            _fmt(8000), _chunk(b"data", silence), size=4 + 24 + 4
        ),
        "riff_size_unknown.wav": _riff(_fmt(8000), _chunk(b"data", b"\0" * 800), size=0xFFFFFFFF),
        "data_size_unknown.wav": _riff(_fmt(8000), _chunk(b"data", silence, size=0xFFFFFFFF)),
        "two_fmt_chunks.wav": _riff(_fmt(8000), _fmt(16000), _chunk(b"data", b"\0" * 800)),
    }


# ---------------------------------------------------------------------------------------------
# Video fixtures: (path, ffmpeg arguments before the output). "@<path>" names an earlier fixture.

TESTSRC = "testsrc=size=64x48:rate=24"
BITEXACT = ("-fflags", "+bitexact", "-flags:v", "+bitexact", "-flags:a", "+bitexact")
NO_METADATA = ("-map_metadata", "-1")
# x264 without its SEI message (the encoder's version and settings): smaller, and the same
# pictures.
X264 = ("-c:v", "libx264", "-pix_fmt", "yuv420p", "-bsf:v", "filter_units=remove_types=6")
AAC = ("-c:a", "aac", "-b:a", "16k")
SINE = ("-f", "lavfi", "-i", "sine=frequency=440:sample_rate=8000")


def lavfi(source: str) -> tuple[str, ...]:
    return ("-f", "lavfi", "-i", source)


VIDEO_FIXTURES: list[tuple[str, tuple[str, ...]]] = [
    ("mp4/h264_24fps.mp4", (*lavfi(TESTSRC), "-frames:v", "48", *X264)),
    (
        "mp4/h264_2997.mp4",
        (*lavfi("testsrc=size=64x48:rate=30000/1001"), "-frames:v", "30", *X264),
    ),
    (
        "mp4/h264_25fps_no_bframes.mp4",
        (*lavfi("testsrc=size=64x48:rate=25"), "-frames:v", "10", *X264, "-bf", "0"),
    ),
    ("mp4/h264_60fps.mp4", (*lavfi("testsrc=size=64x48:rate=60"), "-frames:v", "30", *X264)),
    ("mp4/h264_one_frame.mp4", (*lavfi(TESTSRC), "-frames:v", "1", *X264)),
    (
        "mp4/h264_timescale_600.mp4",
        (*lavfi(TESTSRC), "-frames:v", "24", *X264, "-video_track_timescale", "600"),
    ),
    ("mp4/h264_66x50.mp4", (*lavfi("testsrc=size=66x50:rate=24"), "-frames:v", "12", *X264)),
    (
        "mp4/h264_yuv444.mp4",
        (
            *lavfi(TESTSRC),
            "-frames:v",
            "24",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv444p",
            "-bsf:v",
            "filter_units=remove_types=6",
        ),
    ),
    (
        "mp4/audio_first.mp4",
        (*SINE, *lavfi(TESTSRC), "-map", "0:a", "-map", "1:v", "-t", "1", *X264, *AAC),
    ),
    ("mp4/audio_only.mp4", (*SINE, "-t", "1", *AAC)),
    ("mp4/faststart.mp4", (*lavfi(TESTSRC), "-frames:v", "24", *X264, "-movflags", "+faststart")),
    (
        "mp4/fragmented.mp4",
        (*lavfi(TESTSRC), "-frames:v", "24", *X264, "-movflags", "+frag_keyframe+empty_moov"),
    ),
    (
        "mp4/fragmented_two.mp4",
        (
            *lavfi(TESTSRC),
            "-frames:v",
            "24",
            *X264,
            "-g",
            "12",
            "-movflags",
            "+frag_keyframe+empty_moov+default_base_moof",
        ),
    ),
    (
        "mp4/vfr.mp4",
        (
            *lavfi(TESTSRC),
            "-frames:v",
            "24",
            "-vf",
            "setpts='if(lt(N,12),N/24/TB,(12/24+(N-12)/12)/TB)'",
            "-fps_mode",
            "vfr",
            *X264,
        ),
    ),
    ("mp4/edit_cut.mp4", ("-ss", "0.5", "-i", "@mp4/h264_24fps.mp4", "-c", "copy")),
    (
        "mp4/empty_edit.mp4",
        (*lavfi(TESTSRC), "-frames:v", "24", *X264, "-output_ts_offset", "0.5"),
    ),
    (
        "mp4/rotated.mp4",
        ("-display_rotation", "90", "-i", "@mp4/h264_24fps.mp4", "-c", "copy"),
    ),
    ("mp4/sar_2_1.mp4", (*lavfi(TESTSRC), "-frames:v", "24", "-vf", "setsar=2", *X264)),
    ("mp4/mpeg4_part2.mp4", (*lavfi(TESTSRC), "-frames:v", "24", "-c:v", "mpeg4")),
    (
        "mp4/fragmented_moov_samples.mp4",
        (*lavfi(TESTSRC), "-frames:v", "24", *X264, "-g", "12", "-movflags", "+frag_keyframe"),
    ),
    (
        "mp4/fragmented_zero_edit.mp4",
        (
            *lavfi(TESTSRC),
            "-frames:v",
            "24",
            *X264,
            "-g",
            "12",
            "-movflags",
            "+frag_keyframe+empty_moov+delay_moov",
        ),
    ),
    ("mp4/png_rgba.mp4", (*lavfi(TESTSRC), "-frames:v", "3", "-c:v", "png", "-pix_fmt", "rgba")),
    ("mp4/png_rgb.mp4", (*lavfi(TESTSRC), "-frames:v", "3", "-c:v", "png", "-pix_fmt", "rgb24")),
    ("mp4/quicktime.mov", (*lavfi(TESTSRC), "-frames:v", "24", *X264)),
    # Constant rates whose timestamps are rounded to the timescale (facts.md 4.3).
    (
        "mp4/h264_ts1000_24fps.mp4",
        (*lavfi(TESTSRC), "-frames:v", "48", *X264, "-video_track_timescale", "1000"),
    ),
    (
        "mp4/h264_ts1000_2997.mp4",
        (
            *lavfi("testsrc=size=64x48:rate=30000/1001"),
            "-frames:v",
            "120",
            *X264,
            "-video_track_timescale",
            "1000",
        ),
    ),
    (
        "mp4/h264_ts1000000_30fps.mp4",
        (
            *lavfi("testsrc=size=64x48:rate=30"),
            "-frames:v",
            "31",
            *X264,
            "-video_track_timescale",
            "1000000",
        ),
    ),
    (
        "mp4/h264_ts90000_5994.mp4",
        (
            *lavfi("testsrc=size=64x48:rate=60000/1001"),
            "-frames:v",
            "60",
            *X264,
            "-video_track_timescale",
            "90000",
        ),
    ),
    ("mp4/ismv.mp4", (*lavfi(TESTSRC), "-frames:v", "24", *X264, "-f", "ismv")),
    (
        "mp4/fragmented_av.mp4",
        (
            *SINE,
            *lavfi(TESTSRC),
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-frames:v",
            "24",
            "-t",
            "1",
            *X264,
            *AAC,
            "-g",
            "12",
            "-movflags",
            "+frag_keyframe+empty_moov",
        ),
    ),
    ("work/ms30.mkv", (*lavfi("testsrc=size=64x48:rate=30"), "-frames:v", "30", *X264)),
    ("mp4/remux_ms.mp4", ("-i", "@work/ms30.mkv", "-c", "copy")),
    ("mp4/mjpeg.mp4", (*lavfi(TESTSRC), "-frames:v", "3", "-c:v", "mjpeg")),
    ("matroska/h264.mkv", (*lavfi(TESTSRC), "-frames:v", "24", *X264)),
    (
        "matroska/vp9.webm",
        (*lavfi(TESTSRC), "-frames:v", "24", "-c:v", "libvpx-vp9", "-pix_fmt", "yuv420p"),
    ),
    (
        "matroska/vp9_5994.webm",
        (
            *lavfi("testsrc=size=64x48:rate=60000/1001"),
            "-frames:v",
            "6",
            "-c:v",
            "libvpx-vp9",
            "-pix_fmt",
            "yuv420p",
        ),
    ),
    (
        "matroska/vp9_alpha.webm",
        (*lavfi(TESTSRC), "-frames:v", "24", "-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p"),
    ),
    (
        "matroska/ffv1_yuva.mkv",
        (*lavfi(TESTSRC), "-frames:v", "6", "-c:v", "ffv1", "-pix_fmt", "yuva420p"),
    ),
    (
        "matroska/ffv1_yuv.mkv",
        (*lavfi(TESTSRC), "-frames:v", "6", "-c:v", "ffv1", "-pix_fmt", "yuv420p"),
    ),
    (
        "matroska/ffv1_level1_yuva.mkv",
        (*lavfi(TESTSRC), "-frames:v", "6", "-c:v", "ffv1", "-level", "1", "-pix_fmt", "yuva420p"),
    ),
    (
        "matroska/png_rgba.mkv",
        (*lavfi(TESTSRC), "-frames:v", "6", "-c:v", "png", "-pix_fmt", "rgba"),
    ),
    (
        "matroska/png_rgba64.mkv",
        (*lavfi(TESTSRC), "-frames:v", "3", "-c:v", "png", "-pix_fmt", "rgba64be"),
    ),
    (
        "matroska/png_rgb.mkv",
        (*lavfi(TESTSRC), "-frames:v", "6", "-c:v", "png", "-pix_fmt", "rgb24"),
    ),
    ("matroska/mjpeg.mkv", (*lavfi(TESTSRC), "-frames:v", "3", "-c:v", "mjpeg")),
]
# Paths under work/ are made only for later fixtures to read ("@work/…"), and never kept.
WORK = "work/"


def _box_at(data: bytes, path: tuple[bytes, ...]) -> tuple[int, int, int]:
    """(start, content start, end) of the first box of each type along `path`, from the top."""

    start, content, end = 0, 0, len(data)
    for kind in path:
        at, limit = content, end
        while True:
            size, found = struct.unpack_from(">I4s", data, at)
            header = 16 if size == 1 else 8
            if size == 1:
                size = struct.unpack_from(">Q", data, at + 8)[0]
            elif size == 0:
                size = limit - at
            if found == kind:
                start, content, end = at, at + header, at + size
                break
            at += size
    return start, content, end


def _cut_box(data: bytes, path: tuple[bytes, ...], keep: int) -> bytes:
    """`data` with the box at `path` cut to its first `keep` bytes of content and a `free` box
    after it in the bytes it gave up, so that no other size or offset changes."""

    start, content, end = _box_at(data, path)
    assert content - start == 8, "an 8-byte box header"
    lost = end - content - keep
    assert lost >= 8, "room for the free box"
    cut = struct.pack(">I", 8 + keep) + data[start + 4 : content] + data[content : content + keep]
    return data[:start] + cut + struct.pack(">I", lost) + b"free" + bytes(lost - 8) + data[end:]


def _patched(data: bytes, at: int, new: bytes) -> bytes:
    return data[:at] + new + data[at + len(new) :]


STBL = (b"moov", b"trak", b"mdia", b"minf", b"stbl")


def _matroska_element(data: bytes, path: tuple[int, ...]) -> tuple[int, int, int]:
    """(start, content start, end) of the first element of each ID along `path` below the
    Segment (a Cluster of unknown size is not followed)."""

    _, content, size = _element(data, 0, len(data))
    at = content + size
    while True:
        eid, content, size = _element(data, at, len(data))
        if eid == SEGMENT:
            break
        at = content + size
    end = len(data) if size is None else content + size
    start = at
    for want in path:
        at, limit = content, end
        while True:
            eid, child, size = _element(data, at, limit)
            if eid == want:
                start, content, end = at, child, child + size
                break
            at = child + size
    return start, content, end


def _video_codec_private(data: bytes) -> tuple[int, int, int]:
    """(start, content start, end) of the video track's CodecPrivate."""

    _, content, end = _matroska_element(data, (TRACKS,))
    for eid, cs, ce in _children(data, content, end):
        if eid == TRACK_ENTRY and _track_entry(data, cs, ce).get("type") == 1:
            at = cs
            while at < ce:
                child, child_content, size = _element(data, at, ce)
                if child == 0x63A2:
                    return at, child_content, child_content + size
                at = child_content + size
    raise ValueError("no CodecPrivate")


def _private_cut_to_20(data: bytes) -> bytes:
    """png_rgba.mkv with its 40-byte BITMAPINFOHEADER cut to its first 20 bytes and a Void
    element in the 20 bytes it gave up."""

    start, content, end = _video_codec_private(data)
    assert end - content == 40 and content - start == 3, "a 40-byte private with a 1-byte size"
    private = b"\x63\xa2\x94" + data[content : content + 20]
    void = b"\xec\x92" + bytes(18)
    return data[:start] + private + void + data[end:]


def _first_block_track_zero(data: bytes) -> bytes:
    """h264.mkv with the track number of its first SimpleBlock starting with a 0 byte."""

    _, content, _ = _matroska_element(data, (CLUSTER, SIMPLE_BLOCK))
    assert data[content] == 0x81, "a 1-byte track number"
    return _patched(data, content, b"\x00")


# Fixtures cut from, written over or written instead of an encoder's output.
DERIVED: dict[str, Callable[[dict[str, bytes]], bytes]] = {
    "mp4/truncated.mp4": lambda made: made["mp4/h264_24fps.mp4"][:2000],
    "mp4/garbage.mp4": lambda made: b"not a video",
    # A version 0 mvhd that ends after its timescale, or before it.
    "mp4/mvhd_short.mp4": lambda made: _cut_box(
        made["mp4/h264_25fps_no_bframes.mp4"], (b"moov", b"mvhd"), 16
    ),
    "mp4/mvhd_short_audio.mp4": lambda made: _cut_box(
        made["mp4/audio_only.mp4"], (b"moov", b"mvhd"), 16
    ),
    "mp4/mvhd_no_timescale.mp4": lambda made: _cut_box(
        made["mp4/h264_25fps_no_bframes.mp4"], (b"moov", b"mvhd"), 12
    ),
    # An H.264 track whose stsz has a sample size of 0 and its count but no table.
    "mp4/stsz_no_table.mp4": lambda made: _cut_box(
        made["mp4/h264_25fps_no_bframes.mp4"], (*STBL, b"stsz"), 12
    ),
    # An stsd whose entry count is 0, its sample entry still there.
    "mp4/stsd_count_zero.mp4": lambda made: _patched(
        made["mp4/h264_25fps_no_bframes.mp4"],
        _box_at(made["mp4/h264_25fps_no_bframes.mp4"], (*STBL, b"stsd"))[1] + 4,
        bytes(4),
    ),
    "matroska/truncated.mkv": lambda made: made["matroska/h264.mkv"][:1500],
    "matroska/garbage.webm": lambda made: b"not a video",
    "matroska/png_private20.mkv": lambda made: _private_cut_to_20(made["matroska/png_rgba.mkv"]),
    "matroska/block_track_zero.mkv": lambda made: _first_block_track_zero(
        made["matroska/h264.mkv"]
    ),
}


def make_video(work: Path) -> dict[str, bytes]:
    made: dict[str, bytes] = {}
    for path, args in VIDEO_FIXTURES:
        out = work / path.replace("/", "_")
        argv = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-y"]
        for arg in args:
            argv.append(str(work / arg[1:].replace("/", "_")) if arg.startswith("@") else arg)
        argv += [*BITEXACT, *NO_METADATA, str(out)]
        subprocess.run(argv, check=True)
        made[path] = out.read_bytes()
    for path, derive in DERIVED.items():
        made[path] = derive(made)
    return made


# ---------------------------------------------------------------------------------------------
# Generate and check


def write_folders(files: dict[str, bytes]) -> None:
    for folder in FOLDERS:
        target = FACTS / folder
        if target.exists():
            shutil.rmtree(target)
        target.mkdir(parents=True)
    expected: dict[str, dict[str, Any]] = {folder: {} for folder in FOLDERS}
    for path in sorted(files):
        folder, name = path.split("/")
        (FACTS / path).write_bytes(files[path])
        expected[folder][name] = facts_of(name, files[path])
    for folder, entries in expected.items():
        text = json.dumps(entries, indent=1, ensure_ascii=False) + "\n"
        (FACTS / folder / EXPECTED).write_text(text, encoding="utf-8")
    for name, source in CASE_COPIES.items():
        (CASE / name).write_bytes(files[source])


def cross_check(files: dict[str, bytes]) -> list[str]:
    problems = []
    for path in sorted(files):
        name = path.split("/")[1]
        ours = facts_of(name, files[path])
        theirs = predecessor_facts(name, FACTS / path)
        if path in DIFFERENCES:
            if ours == theirs:
                problems.append(f"{path}: listed in DIFFERENCES but reads the same: {ours}")
        elif ours != theirs:
            problems.append(f"{path}: FX {ours} but stage-gen's engine {theirs}")
    return problems


def check() -> list[str]:
    problems = []
    for folder in FOLDERS:
        expected_path = FACTS / folder / EXPECTED
        if not expected_path.is_file():
            problems.append(f"{folder}/{EXPECTED} is missing")
            continue
        expected = json.loads(expected_path.read_text(encoding="utf-8"))
        names = {p.name for p in (FACTS / folder).iterdir() if p.name != EXPECTED}
        for name in sorted(names - set(expected)):
            problems.append(f"{folder}/{name} is not in {EXPECTED}")
        for name in sorted(set(expected) - names):
            problems.append(f"{folder}/{name} is in {EXPECTED} but not on disk")
        for name in sorted(names & set(expected)):
            got = facts_of(name, (FACTS / folder / name).read_bytes())
            if got != expected[name] or list(got) != list(expected[name]):
                problems.append(f"{folder}/{name}: read {got}, expected {expected[name]}")
    for name, source in CASE_COPIES.items():
        copy = CASE / name
        if not copy.is_file() or copy.read_bytes() != (FACTS / source).read_bytes():
            problems.append(f"{copy.relative_to(REPO)} is not a copy of {source}")
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--check", action="store_true", help="re-read the committed fixtures and compare"
    )
    args = parser.parse_args(argv)
    if args.check:
        problems = check()
    else:
        for tool in ("ffmpeg", "ffprobe"):
            if shutil.which(tool) is None:
                print(f"make_fixtures: {tool} is not on PATH", file=sys.stderr)
                return 2
        files = {f"wav/{name}": data for name, data in wav_fixtures().items()}
        with tempfile.TemporaryDirectory(prefix="fx-fixtures-") as work:
            made = make_video(Path(work))
        files.update({path: data for path, data in made.items() if not path.startswith(WORK)})
        write_folders(files)
        problems = cross_check(files) + check()
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        return 1
    count = sum(1 for folder in FOLDERS for p in (FACTS / folder).iterdir() if p.name != EXPECTED)
    print(f"make_fixtures: {count} fixtures {'match' if args.check else 'written'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
