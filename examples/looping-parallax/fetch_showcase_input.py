"""Fetch the public concept input, verifying its immutable bytes before placing it."""

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
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import unquote, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

DESTINATION = ".showcase-inputs/meadow-concept.png"
MAX_BYTES = 16 * 1024 * 1024
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
    if not isinstance(document, dict) or document.get("schema") != "fx_showcase_input_v1":
        raise ValueError("Unsupported showcase input manifest.")
    if document.get("destination") != DESTINATION:
        raise ValueError("The showcase destination must be the fixed project-relative input path.")
    digest = document.get("sha256")
    size = document.get("bytes")
    relative_path = document.get("path")
    if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
        raise ValueError("The showcase input requires a SHA-256 digest.")
    if isinstance(size, bool) or not isinstance(size, int) or not 0 < size <= MAX_BYTES:
        raise ValueError("The showcase input size is outside the supported bounds.")
    if (
        not isinstance(relative_path, str)
        or not relative_path
        or relative_path.startswith("/")
        or any(part in ("", ".", "..") for part in relative_path.split("/"))
        or re.fullmatch(r"[A-Za-z0-9._/-]+", relative_path) is None
        or PurePosixPath(relative_path).name != f"{digest}.png"
    ):
        raise ValueError("The showcase input requires a confined digest-named artifact path.")
    checked_url(document.get("delivery_base"))
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


def fetch_showcase_input(
    project: Path,
    *,
    base_url: str | None = None,
    opener: Callable = open_public,
) -> Path:
    """Return the fixed input path; never replace a user's existing file."""
    document = load_manifest(project / "showcase-input.json")
    base = checked_url(base_url if base_url is not None else document["delivery_base"])
    url = checked_url(f"{base.rstrip('/')}/{document['path']}")
    digest, size = document["sha256"], document["bytes"]
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
            filename = "meadow-concept.png"
            if _existing_matches(directory, filename, digest, size):
                return project / DESTINATION
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
                        while chunk := response.read(min(CHUNK_BYTES, size - received + 1)):
                            received += len(chunk)
                            if received > size:
                                raise ValueError("The CDN input exceeds its pinned size.")
                            if time.monotonic() - started > TOTAL_SECONDS:
                                raise ValueError("The CDN input exceeded the download time limit.")
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
            return project / DESTINATION
        finally:
            os.close(directory)
    finally:
        os.close(root)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--base-url", help="Public HTTPS CDN base; the artifact path/digest stay pinned."
    )
    args = parser.parse_args()
    try:
        fetch_showcase_input(Path(__file__).resolve().parent, base_url=args.base_url)
    except ValueError as error:
        parser.exit(1, f"Input fetch refused: {error}\n")
    except Exception:
        parser.exit(1, "Input fetch failed; no existing input was replaced.\n")
    print(f"Verified {DESTINATION}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
