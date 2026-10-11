"""Embedded GLB access, transforms and append-only rig data; no study imports."""

from __future__ import annotations

import copy
import hashlib
import json
import struct
from pathlib import Path
from typing import Any

import numpy as np
from numpy.typing import ArrayLike, NDArray

DTYPES = {5120: "i1", 5121: "u1", 5122: "<i2", 5123: "<u2", 5125: "<u4", 5126: "<f4"}
WIDTHS = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}


def sha(path: str | Path) -> str:
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def read_glb(path: str | Path) -> tuple[dict[str, Any], bytearray]:
    raw = Path(path).read_bytes()
    if len(raw) < 28 or struct.unpack_from("<4sII", raw) != (b"glTF", 2, len(raw)):
        raise ValueError("Invalid GLB header")
    offset, chunks = (12, [])
    while offset < len(raw):
        if offset + 8 > len(raw):
            raise ValueError("Truncated GLB chunk")
        size, kind = struct.unpack_from("<II", raw, offset)
        if size % 4 or offset + 8 + size > len(raw):
            raise ValueError("Invalid GLB chunk size")
        chunks.append((kind, raw[offset + 8 : offset + 8 + size]))
        offset += size + 8
    if [kind for kind, _ in chunks] != [1313821514, 5130562]:
        raise ValueError("Only JSON plus one embedded BIN chunk is supported")
    doc = json.loads(chunks[0][1])
    binary = bytearray(chunks[1][1])
    if len(doc.get("buffers", [])) != 1 or "uri" in doc["buffers"][0]:
        raise ValueError("Expected one embedded buffer")
    declared = doc["buffers"][0].get("byteLength")
    if type(declared) is not int or not 0 <= len(binary) - declared <= 3:
        raise ValueError("Buffer length does not match BIN chunk")
    if any("uri" in image or "bufferView" not in image for image in doc.get("images", [])):
        raise ValueError("Only bufferView-embedded images are supported")
    for view in doc.get("bufferViews", []):
        if (
            view.get("buffer", 0) != 0
            or view.get("byteOffset", 0) < 0
            or view.get("byteLength", -1) < 0
            or (view.get("byteOffset", 0) + view["byteLength"] > declared)
        ):
            raise ValueError("Buffer view lies outside embedded buffer")
    for index in range(len(doc.get("accessors", []))):
        accessor(doc, binary, index)
    worlds(doc)
    return (doc, binary)


def accessor(doc: dict[str, Any], binary: bytes | bytearray, index: int) -> NDArray[Any]:
    item = doc["accessors"][index]
    if (
        "sparse" in item
        or "bufferView" not in item
        or item.get("type") not in WIDTHS
        or (item.get("componentType") not in DTYPES)
    ):
        raise ValueError("Accessor format requires an explicit supported decoder")
    view = doc["bufferViews"][item["bufferView"]]
    dtype = np.dtype(DTYPES[item["componentType"]])
    width = WIDTHS[item["type"]]
    offset, count = (item.get("byteOffset", 0), item["count"])
    stride = view.get("byteStride", dtype.itemsize * width)
    if (
        type(count) is not int
        or count < 0
        or offset < 0
        or (stride < dtype.itemsize * width)
        or stride % dtype.itemsize
    ):
        raise ValueError("Invalid accessor count, offset or stride")
    end = offset + (count - 1) * stride + dtype.itemsize * width if count else offset
    if end > view["byteLength"]:
        raise ValueError("Accessor escapes its buffer view")
    result = np.ndarray(
        (count, width),
        dtype=dtype,
        buffer=binary,
        offset=view.get("byteOffset", 0) + offset,
        strides=(stride, dtype.itemsize),
    ).copy()
    if item.get("normalized"):
        if dtype.kind not in "iu":
            raise ValueError("Only integer accessor normalization is supported")
        result = result.astype(float) / np.iinfo(dtype).max
        if dtype.kind == "i":
            result = np.maximum(result, -1)
    if not np.isfinite(result).all():
        raise ValueError("Accessor contains nonfinite values")
    return result


def local(node: dict[str, Any]) -> NDArray[Any]:
    if "matrix" in node:
        if any(key in node for key in ("translation", "rotation", "scale")):
            raise ValueError("Node cannot have both matrix and TRS")
        result = np.array(node["matrix"], dtype=float).reshape(4, 4).T
    else:
        x, y, z, w = np.asarray(node.get("rotation", [0, 0, 0, 1]), dtype=float)
        length = x * x + y * y + z * z + w * w
        if abs(length - 1) > 0.0001:
            raise ValueError("Node rotation must be a unit quaternion")
        result = np.eye(4)
        result[:3, :3] = [
            [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
            [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
            [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
        ]
        result[:3, :3] *= np.asarray(node.get("scale", [1, 1, 1]))[None, :]
        result[:3, 3] = node.get("translation", [0, 0, 0])
    if not np.isfinite(result).all() or not np.allclose(result[3], [0, 0, 0, 1]):
        raise ValueError("Invalid node affine transform")
    return result


def worlds(doc: dict[str, Any]) -> list[NDArray[Any]]:
    nodes = doc.get("nodes", [])
    parents = {}
    for index, node in enumerate(nodes):
        for child in node.get("children", []):
            if type(child) is not int or not 0 <= child < len(nodes) or child in parents:
                raise ValueError("Invalid child or multiple node parents")
            parents[child] = index
    cache: dict[int, NDArray[Any]] = {}
    visiting: set[int] = set()

    def visit(index: int) -> NDArray[Any]:
        if index in visiting:
            raise ValueError("Node graph contains a cycle")
        if index not in cache:
            visiting.add(index)
            cache[index] = (visit(parents[index]) if index in parents else np.eye(4)) @ local(
                nodes[index]
            )
            visiting.remove(index)
        return cache[index]

    return [visit(index) for index in range(len(nodes))]


def points(values: NDArray[Any], matrix: NDArray[Any]) -> NDArray[Any]:
    result: NDArray[Any] = values @ matrix[:3, :3].T + matrix[:3, 3]
    return result


def append_accessor(
    doc: dict[str, Any],
    binary: bytearray,
    values: ArrayLike,
    component_type: int,
    kind: str,
    template: dict[str, Any] | None = None,
) -> int:
    values = np.asarray(values)
    if values.ndim != 2 or values.shape[1] != WIDTHS[kind] or (not np.isfinite(values).all()):
        raise ValueError("Invalid appended accessor shape or values")
    values = values.astype(DTYPES[component_type])
    binary.extend(b"\x00" * (-len(binary) % 4))
    offset = len(binary)
    raw = values.tobytes()
    binary.extend(raw)
    view = len(doc.setdefault("bufferViews", []))
    doc["bufferViews"].append({"buffer": 0, "byteOffset": offset, "byteLength": len(raw)})
    item = copy.deepcopy(template) if template else {}
    item.update(
        bufferView=view, byteOffset=0, componentType=component_type, count=len(values), type=kind
    )
    index = len(doc.setdefault("accessors", []))
    doc["accessors"].append(item)
    return index


def save_glb(path: str | Path, doc: dict[str, Any], binary: bytes | bytearray) -> None:
    path = Path(path)
    if path.exists() or path.is_symlink():
        raise ValueError("Refusing to overwrite an artifact")
    binary = bytearray(binary)
    binary.extend(b"\x00" * (-len(binary) % 4))
    doc["buffers"] = [{"byteLength": len(binary)}]
    encoded = json.dumps(doc, separators=(",", ":"), allow_nan=False).encode()
    encoded += b" " * (-len(encoded) % 4)
    payload = (
        struct.pack("<4sII", b"glTF", 2, 28 + len(encoded) + len(binary))
        + struct.pack("<II", len(encoded), 1313821514)
        + encoded
        + struct.pack("<II", len(binary), 5130562)
        + binary
    )
    with path.open("xb") as stream:
        stream.write(payload)


def distribution(values: ArrayLike) -> dict[str, float]:
    values = np.asarray(values)
    return {
        "min": float(values.min()),
        "median": float(np.median(values)),
        "p95": float(np.percentile(values, 95)),
        "max": float(values.max()),
    }
