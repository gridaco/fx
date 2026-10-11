"""Fetch pinned public inputs for an FX showcase example; never calls a provider."""

from __future__ import annotations

import argparse
import hashlib
import ipaddress
import json
import os
import re
import secrets
import stat
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

MAX_BYTES = 64 * 1024 * 1024
CHUNK_BYTES = 64 * 1024
TIMEOUT_SECONDS = 20
TOTAL_SECONDS = 120


def checked_url(value: str) -> str:
    """Only public HTTPS URLs without credentials, queries or fragments are allowed."""
    if not isinstance(value, str) or any(character.isspace() for character in value):
        raise ValueError("The CDN URL must be an HTTPS URL without whitespace.")
    parsed = urlsplit(value)
    if (
        parsed.scheme != "https"
        or not parsed.hostname
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
        or "\\" in value
    ):
        raise ValueError("The CDN URL must be HTTPS without credentials, query or fragment.")
    try:
        port = parsed.port
    except ValueError as error:
        raise ValueError("The CDN URL has an invalid port.") from error
    if port not in (None, 443):
        raise ValueError("The CDN URL must use the standard HTTPS port.")
    hostname = parsed.hostname.lower()
    if hostname == "localhost" or hostname.endswith(".localhost"):
        raise ValueError("The CDN URL must identify a public host.")
    try:
        address = ipaddress.ip_address(hostname)
    except ValueError:
        address = None
    if address is not None and not address.is_global:
        raise ValueError("The CDN URL must identify a public host.")
    decoded_path = unquote(parsed.path)
    if "\\" in decoded_path or any(part in (".", "..") for part in decoded_path.split("/")):
        raise ValueError("The CDN URL must not contain path traversal.")
    return value


class SafeRedirects(HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        checked_url(new_url)
        return super().redirect_request(request, fp, code, message, headers, new_url)


def open_public(request: Request, *, timeout: int):
    return build_opener(SafeRedirects()).open(request, timeout=timeout)


def load_manifest(path: Path) -> dict[str, Any]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(document, dict) or document.get("schema") != "fx_showcase_inputs_v1":
        raise ValueError("Unsupported showcase inputs manifest.")
    checked_url(document.get("delivery_base"))
    artifacts = document.get("inputs")
    if not isinstance(artifacts, list) or not 1 <= len(artifacts) <= 16:
        raise ValueError("The manifest must list one to sixteen inputs.")
    names = set()
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            raise ValueError("Each input must be an object.")
        filename, digest = artifact.get("filename"), artifact.get("sha256")
        size, relative_path = artifact.get("bytes"), artifact.get("path")
        if (
            not isinstance(filename, str)
            or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", filename) is None
            or filename in names
        ):
            raise ValueError("Input filenames must be unique confined basenames.")
        names.add(filename)
        if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise ValueError("Each input requires a SHA-256 digest.")
        if type(size) is not int or not 0 < size <= MAX_BYTES:
            raise ValueError("The input size is outside the supported bounds.")
        if (
            not isinstance(relative_path, str)
            or not relative_path
            or relative_path.startswith("/")
            or any(part in ("", ".", "..") for part in relative_path.split("/"))
            or re.fullmatch(r"[A-Za-z0-9._/-]+", relative_path) is None
        ):
            raise ValueError("The input requires a confined relative CDN path.")
    if sum(artifact["bytes"] for artifact in artifacts) > 4 * MAX_BYTES:
        raise ValueError("The input set exceeds the download size limit.")
    return document


def _existing_matches(directory: int, filename: str, digest: str, size: int) -> bool:
    try:
        descriptor = os.open(
            filename, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory
        )
    except FileNotFoundError:
        return False
    except OSError as error:
        raise ValueError(
            "The existing showcase input must be a regular file, not a symlink."
        ) from error
    with os.fdopen(descriptor, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size != size:
            raise ValueError("An existing showcase input conflicts with the pinned artifact.")
        actual = hashlib.sha256()
        received = 0
        while chunk := source.read(min(CHUNK_BYTES, size - received + 1)):
            received += len(chunk)
            if received > size:
                raise ValueError("An existing showcase input changed during verification.")
            actual.update(chunk)
        if received != size or actual.hexdigest() != digest:
            raise ValueError("An existing showcase input conflicts with the pinned artifact.")
    return True


def _fetch_one(project: Path, artifact: dict, base: str, opener: Callable) -> Path:
    url = checked_url(f"{base.rstrip('/')}/{artifact['path']}")
    digest, size = artifact["sha256"], artifact["bytes"]
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    root = os.open(project, flags)
    try:
        try:
            os.mkdir(".showcase-inputs", mode=0o755, dir_fd=root)
        except FileExistsError:
            pass
        try:
            directory = os.open(".showcase-inputs", flags, dir_fd=root)
        except OSError as error:
            raise ValueError("The showcase input directory must not be a symlink.") from error
        try:
            filename = artifact["filename"]
            if _existing_matches(directory, filename, digest, size):
                return project / ".showcase-inputs" / filename
            temporary = f".download-{secrets.token_hex(12)}"
            descriptor = os.open(
                temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644, dir_fd=directory
            )
            try:
                actual = hashlib.sha256()
                received = 0
                started = time.monotonic()
                with os.fdopen(descriptor, "wb") as output:
                    request = Request(url, headers={"User-Agent": "FX-showcase-input/1"})
                    with opener(request, timeout=TIMEOUT_SECONDS) as response:
                        checked_url(response.geturl())
                        length = response.headers.get("Content-Length")
                        if length is not None and length != str(size):
                            raise ValueError(
                                "The CDN content length differs from the pinned input."
                            )
                        while True:
                            if time.monotonic() - started > TOTAL_SECONDS:
                                raise ValueError("The CDN input exceeded the download time limit.")
                            # HTTPResponse.read(n) can wait for n bytes across many socket
                            # reads. read1 makes at most one, so a slow trickle cannot hide
                            # the elapsed deadline; the socket timeout bounds each wait.
                            chunk = response.read1(min(CHUNK_BYTES, size - received + 1))
                            if time.monotonic() - started > TOTAL_SECONDS:
                                raise ValueError("The CDN input exceeded the download time limit.")
                            if not chunk:
                                break
                            received += len(chunk)
                            if received > size:
                                raise ValueError("The CDN input exceeds its pinned size.")
                            actual.update(chunk)
                            output.write(chunk)
                    if received != size or actual.hexdigest() != digest:
                        raise ValueError("The CDN input differs from its pinned size or SHA-256.")
                    output.flush()
                    os.fsync(output.fileno())
                try:
                    # Publish a verified sibling atomically without replacing existing art.
                    os.link(
                        temporary,
                        filename,
                        src_dir_fd=directory,
                        dst_dir_fd=directory,
                        follow_symlinks=False,
                    )
                except FileExistsError:
                    if not _existing_matches(directory, filename, digest, size):
                        raise ValueError(
                            "The destination changed while publishing the input."
                        ) from None
                os.fsync(directory)
            finally:
                os.unlink(temporary, dir_fd=directory)
            return project / ".showcase-inputs" / filename
        finally:
            os.close(directory)
    finally:
        os.close(root)


def fetch_showcase_inputs(
    project: Path, *, base_url: str | None = None, opener: Callable = open_public
) -> list[Path]:
    """Verify each input and atomically place it; existing conflicting files are refused."""
    document = load_manifest(project / "showcase-inputs.json")
    base = checked_url(base_url if base_url is not None else document["delivery_base"])
    return [_fetch_one(project, artifact, base, opener) for artifact in document["inputs"]]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("project", type=Path, help="Example folder with showcase-inputs.json")
    parser.add_argument("--base-url", help="Public HTTPS CDN base; paths and digests stay pinned.")
    args = parser.parse_args()
    try:
        paths = fetch_showcase_inputs(args.project.resolve(), base_url=args.base_url)
    except ValueError as error:
        parser.exit(1, f"Input fetch refused: {error}\n")
    except Exception:
        parser.exit(1, "Input fetch failed; no existing input was replaced.\n")
    for path in paths:
        print(f"Verified .showcase-inputs/{path.name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
