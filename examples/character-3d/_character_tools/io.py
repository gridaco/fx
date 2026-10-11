"""Portable, confined run inputs and immutable records."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
from typing import Any


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def canonical_digest(value: object) -> str:
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
    ).hexdigest()


def confined(root: Path, ref: str, *, must_exist: bool = True) -> Path:
    if not isinstance(ref, str) or not ref or "\\" in ref or ("\x00" in ref):
        raise ValueError("Expected a portable relative path")
    parts = ref.split("/")
    if Path(ref).is_absolute() or any(part in {"", ".", ".."} for part in parts):
        raise ValueError("Absolute paths and traversal are refused")
    root = root.resolve(strict=True)
    target = root
    for part in parts:
        target /= part
        if target.is_symlink():
            raise ValueError("Symlinks are refused")
    if not target.resolve().is_relative_to(root):
        raise ValueError("Path escapes its declared root")
    if must_exist and (not target.is_file()):
        raise ValueError("Input is missing or is not a regular file")
    return target


def write_bytes(path: Path, payload: bytes) -> None:
    for parent in (path, *path.parents):
        if parent.is_symlink():
            raise ValueError("Artifact path contains a symlink")
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(payload)


def write_json(path: Path, value: object) -> None:
    write_bytes(path, json.dumps(value, indent=2, allow_nan=False).encode() + b"\n")


def read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise ValueError("Expected a JSON object")
    return value


def verified_input(root: Path, source: dict[str, Any]) -> Path:
    if set(source) != {"path", "sha256"}:
        raise ValueError("Source requires path and sha256")
    path = confined(root, source["path"])
    if digest(path) != source["sha256"]:
        raise ValueError("Source content does not match its declared hash")
    return path
