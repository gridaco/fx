"""Read-only recorded file discovery and private native path resolution.

The engine owns source binding, confinement and byte verification. These wrappers never
execute author code or derive a path from a portable source or an artifact URL.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from grida.fx._api import FxError, _call, _Exit, _failure, _no_running_loop

_DIGEST = re.compile(r"[a-f0-9]{64}")
_SYMBOL = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_ROLES = {"artifact", "workflow", "node", "source", "resource"}


@dataclass(frozen=True)
class FileDescriptor:
    """A portable recorded file target and its association with the inspected graph.

    ``target`` retains its wire shape and can be paired with a captured viewer subject.
    """

    target: dict[str, Any]
    role: str
    instance_id: str | None = None
    scope_id: str | None = None


@dataclass(frozen=True)
class FileResolution:
    """Private native lookup result. Never forward ``local_path`` to a viewer message.

    Artifact copies are verified; source files are current editable files and may have
    changed. A path is a point-in-time lookup, not a lease on the file.
    """

    target: dict[str, Any]
    local_path: Path | None
    source_match: str | None
    reason: str | None


class FileAccessError(FxError):
    """Structured engine refusal, including unknown future ``code`` values."""

    def __init__(self, document: dict[str, Any]) -> None:
        super().__init__(document["message"], 2)
        self.code: str = document["code"]
        self.document = document


def _identifier(value: Any) -> bool:
    return (
        isinstance(value, str)
        and bool(value)
        and not any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in value)
    )


def _source(value: Any) -> bool:
    return (
        _identifier(value)
        and not value.startswith("/")
        and "\\" not in value
        and ":" not in value
        and all(part not in ("", ".", "..") for part in value.split("/"))
    )


def _url(value: Any) -> bool:
    if value is None:
        return True
    if not _identifier(value) or any(char in value for char in "\\?#"):
        return False
    if value.startswith("/"):
        return not value.startswith("//")
    if not re.match(r"^https?://", value, re.IGNORECASE) or any(
        not 33 <= ord(char) <= 126 for char in value
    ):
        return False
    try:
        parsed = urlsplit(value)
        # Accessing port validates an explicit port rather than accepting an invalid authority.
        _ = parsed.port
        return (
            parsed.scheme in ("http", "https")
            and bool(parsed.hostname)
            and "@" not in parsed.netloc
        )
    except ValueError:
        return False


def _name(value: Any) -> bool:
    return (
        isinstance(value, str)
        and not any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in value)
        and not re.match(r"^[\\/]|^[A-Za-z]:|^file:", value, re.IGNORECASE)
    )


def _file_target(value: Any) -> bool:
    if not isinstance(value, dict):
        return False
    if value.get("kind") == "source":
        return (
            {"kind", "source"} <= value.keys() <= {"kind", "source", "symbol", "recorded_digest"}
            and _source(value["source"])
            and ("symbol" not in value or _symbol(value["symbol"]))
            and ("recorded_digest" not in value or _digest(value["recorded_digest"]))
        )
    artifact = value.get("artifact")
    return (
        value.get("kind") == "artifact"
        and value.keys() == {"kind", "artifact"}
        and isinstance(artifact, dict)
        and artifact.keys() == {"digest", "kind", "name", "size", "url"}
        and _digest(artifact.get("digest"))
        and _name(artifact["name"])
        and _identifier(artifact["kind"])
        and type(artifact.get("size")) is int
        and 0 <= artifact["size"] <= 9_007_199_254_740_991
        and "url" in artifact
        and _url(artifact["url"])
    )


def _symbol(value: Any) -> bool:
    return isinstance(value, str) and _SYMBOL.fullmatch(value) is not None


def _digest(value: Any) -> bool:
    return isinstance(value, str) and _DIGEST.fullmatch(value) is not None


def _path(value: str | os.PathLike[str], name: str) -> str:
    if not isinstance(value, str | os.PathLike):
        raise TypeError(f"{name} is a nonempty filesystem path")
    text = os.fspath(value)
    if not isinstance(text, str) or not text or "\0" in text:
        raise ValueError(f"{name} is nonempty and contains no NUL")
    return text


def _arguments(
    target: str | Path, cwd: str | Path | None, project: str | Path | None
) -> tuple[str, Path, list[str]]:
    selected = _path(target, "target")
    if selected.startswith("-"):
        raise ValueError("target cannot start with '-'")
    base = Path.cwd() if cwd is None else Path(_path(cwd, "cwd")).absolute()
    options = []
    if project is not None:
        bound = Path(_path(project, "project"))
        if not bound.is_absolute():
            bound = base / bound
        options.append(f"--project={bound.absolute()}")
    return selected, base, options


def _reject_constant(value: str) -> None:
    raise ValueError(f"non-JSON constant {value}")


def _document(done: _Exit, kind: str, verb: str) -> dict[str, Any]:
    try:
        value = json.loads(done.stdout, parse_constant=_reject_constant)
    except ValueError:
        raise FxError(f"grida-fx {verb} printed malformed JSON", done.status) from None
    if (
        done.status == 2
        and isinstance(value, dict)
        and value.get("kind") == "fx-file-error-v1"
        and _identifier(value.get("code"))
        and isinstance(value.get("message"), str)
    ):
        raise FileAccessError(value)
    if done.status != 0:
        raise _failure(verb, done)
    if not isinstance(value, dict) or value.get("kind") != kind:
        raise FxError(f"grida-fx {verb} printed an incompatible file document", done.status)
    return value


async def list_files_async(
    target: str | Path,
    *,
    project: str | Path | None = None,
    instance_id: str | None = None,
    cwd: str | Path | None = None,
) -> list[FileDescriptor]:
    """Enumerate recorded files in an explicit run folder or saved plan JSON file."""
    selected, base, options = _arguments(target, cwd, project)
    if instance_id is not None:
        if not _identifier(instance_id):
            raise ValueError("instance_id is a recorded instance identifier")
        options.append(f"--instance={instance_id}")
    done = await _call(["files", "list", selected, "--json", *options], base)
    document = _document(done, "fx-files-v1", "files list")
    rows = document.get("files")
    if not isinstance(rows, list):
        raise FxError("grida-fx files list printed malformed file descriptors", done.status)
    result = []
    for entry in rows:
        if (
            not isinstance(entry, dict)
            or not _file_target(entry.get("target"))
            or not isinstance(entry.get("role"), str)
            or entry["role"] not in _ROLES
            or ("instance_id" in entry and not _identifier(entry["instance_id"]))
            or ("scope_id" in entry and not _identifier(entry["scope_id"]))
        ):
            raise FxError("grida-fx files list printed malformed file descriptors", done.status)
        result.append(
            FileDescriptor(
                target=entry["target"],
                role=entry["role"],
                instance_id=entry.get("instance_id"),
                scope_id=entry.get("scope_id"),
            )
        )
    return result


def list_files(
    target: str | Path,
    *,
    project: str | Path | None = None,
    instance_id: str | None = None,
    cwd: str | Path | None = None,
) -> list[FileDescriptor]:
    """Sync counterpart of :func:`list_files_async`; no workflow code is loaded."""
    _no_running_loop("list_files")
    return asyncio.run(list_files_async(target, project=project, instance_id=instance_id, cwd=cwd))


async def resolve_file_async(
    target: str | Path,
    reference: Mapping[str, Any],
    *,
    project: str | Path | None = None,
    source_root: str | Path | None = None,
    cwd: str | Path | None = None,
) -> FileResolution:
    """Resolve a captured viewer reference using explicit local binding for source files."""
    selected, base, options = _arguments(target, cwd, project)
    if source_root is not None:
        root = Path(_path(source_root, "source_root"))
        if not root.is_absolute():
            root = base / root
        options.append(f"--source-root={root.absolute()}")
    if (
        not isinstance(reference, Mapping)
        or not isinstance(reference.get("subject"), dict)
        or not _file_target(reference.get("target"))
    ):
        raise TypeError("reference is a portable viewer file reference")
    encoded = json.dumps(
        dict(reference), ensure_ascii=False, allow_nan=False, separators=(",", ":")
    )
    done = await _call(
        ["files", "resolve", selected, "--reference", encoded, "--json", *options], base
    )
    value = _document(done, "fx-file-resolution-v1", "files resolve")
    local = value.get("local_path")
    if (
        not _file_target(value.get("target"))
        or "local_path" not in value
        or not (local is None or (_identifier(local) and Path(local).is_absolute()))
        or "source_match" not in value
        or value["source_match"] not in (None, "match", "changed", "unknown")
        or "reason" not in value
        or not (value["reason"] is None or isinstance(value["reason"], str))
    ):
        raise FxError("grida-fx files resolve printed a malformed resolution", done.status)
    return FileResolution(
        target=value["target"],
        local_path=None if local is None else Path(local),
        source_match=value["source_match"],
        reason=value["reason"],
    )


def resolve_file(
    target: str | Path,
    reference: Mapping[str, Any],
    *,
    project: str | Path | None = None,
    source_root: str | Path | None = None,
    cwd: str | Path | None = None,
) -> FileResolution:
    """Sync counterpart of :func:`resolve_file_async`; returned paths are private native data."""
    _no_running_loop("resolve_file")
    return asyncio.run(
        resolve_file_async(target, reference, project=project, source_root=source_root, cwd=cwd)
    )
