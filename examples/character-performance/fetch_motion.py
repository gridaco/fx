"""Fetch one explicitly selected, pinned motion clip; never scrape a stock catalog."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source", type=Path, help="Use a manually acquired copy instead of downloading"
    )
    args = parser.parse_args()
    spec = json.loads((ROOT / "motion-reference.json").read_text())
    destination = ROOT / ".showcase-inputs" / "motion.mp4"
    if destination.parent.is_symlink():
        raise ValueError("the input directory must not be a symlink")
    destination.parent.mkdir(exist_ok=True)
    if destination.is_symlink():
        raise ValueError("the destination must not be a symlink")
    if args.source:
        data = args.source.read_bytes()
    elif destination.exists():
        data = destination.read_bytes()
    else:
        request = urllib.request.Request(
            spec["download_url"], headers={"User-Agent": "FX motion example/1.0"}
        )
        with urllib.request.urlopen(request, timeout=60) as response:
            data = response.read(spec["bytes"] + 1)
    if len(data) != spec["bytes"] or hashlib.sha256(data).hexdigest() != spec["sha256"]:
        raise ValueError("motion bytes do not match the selected reference; no file was replaced")
    if destination.exists():
        if destination.read_bytes() != data:
            raise ValueError("refusing to replace a different existing input")
    else:
        handle, temporary = tempfile.mkstemp(prefix=".motion-", dir=destination.parent)
        try:
            with os.fdopen(handle, "wb") as file:
                file.write(data)
            os.replace(temporary, destination)
        finally:
            Path(temporary).unlink(missing_ok=True)
    print("Verified the selected motion clip. See motion-reference.json for source and terms.")


if __name__ == "__main__":
    main()
