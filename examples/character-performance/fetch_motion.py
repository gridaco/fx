"""Fetch one explicitly selected, pinned motion clip; never scrape a stock catalog."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import tempfile
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent
MAX_SOURCE_BYTES = 250_000_000


def confined_path(relative: Path, *, output: bool = False) -> Path:
    """Keep reference reads in the example and writes in its ignored input folder."""
    if relative.is_absolute() or not relative.parts or ".." in relative.parts:
        raise ValueError("reference and output paths must be relative and inside this example")
    if output and (
        len(relative.parts) < 2
        or relative.parts[0] != ".showcase-inputs"
        or relative.suffix.lower() != ".mp4"
    ):
        raise ValueError("output must name an MP4 inside .showcase-inputs")
    path = ROOT
    for part in relative.parts:
        path = path / part
        if path.is_symlink():
            raise ValueError("reference and output paths must not contain symlinks")
    return path


def acquire(
    *,
    reference: Path = Path("motion-reference.json"),
    output: Path = Path(".showcase-inputs/motion.mp4"),
    source: Path | None = None,
) -> None:
    reference_path = confined_path(reference)
    destination = confined_path(output, output=True)
    with reference_path.open("rb") as file:
        encoded = file.read(65_537)
    if len(encoded) > 65_536:
        raise ValueError("selected reference metadata is too large")
    spec = json.loads(encoded)
    expected = spec["bytes"]
    digest = spec["sha256"]
    if type(expected) is not int or not 0 < expected <= MAX_SOURCE_BYTES:
        raise ValueError("selected reference size must be a positive bounded integer")
    if (
        not isinstance(digest, str)
        or len(digest) != 64
        or any(character not in "0123456789abcdef" for character in digest)
    ):
        raise ValueError("selected reference must contain a lowercase SHA256 digest")
    url = urllib.parse.urlsplit(spec["download_url"])
    if url.scheme != "https" or not url.netloc or url.username or url.password:
        raise ValueError("selected reference download must use a public HTTPS URL")
    destination.parent.mkdir(parents=True, exist_ok=True)
    confined_path(output, output=True)
    if source:
        with source.open("rb") as file:
            data = file.read(expected + 1)
    elif destination.exists():
        with destination.open("rb") as file:
            data = file.read(expected + 1)
    else:
        request = urllib.request.Request(
            spec["download_url"], headers={"User-Agent": "FX motion example/1.0"}
        )
        with urllib.request.urlopen(request, timeout=60) as response:
            data = response.read(expected + 1)
    if len(data) != expected or hashlib.sha256(data).hexdigest() != digest:
        raise ValueError("motion bytes do not match the selected reference; no file was replaced")
    if destination.exists():
        with destination.open("rb") as file:
            if file.read(expected + 1) != data:
                raise ValueError("refusing to replace a different existing input")
    else:
        handle, temporary = tempfile.mkstemp(prefix=".motion-", dir=destination.parent)
        try:
            with os.fdopen(handle, "wb") as file:
                file.write(data)
            os.replace(temporary, destination)
        finally:
            Path(temporary).unlink(missing_ok=True)
    print("Verified the selected motion clip. See its reference metadata for source and terms.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source", type=Path, help="Use a manually acquired copy instead of downloading"
    )
    parser.add_argument(
        "--reference",
        type=Path,
        default=Path("motion-reference.json"),
        help="Selected reference JSON, relative to this example",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(".showcase-inputs/motion.mp4"),
        help="Local MP4 path inside this example's .showcase-inputs folder",
    )
    args = parser.parse_args()
    acquire(reference=args.reference, output=args.output, source=args.source)


if __name__ == "__main__":
    main()
