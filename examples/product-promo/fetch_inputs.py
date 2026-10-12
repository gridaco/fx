"""Acquire checksum-pinned FX-owned original recording; no provider calls."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
import urllib.request
from pathlib import Path
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parent
MAX_BYTES = 64 * 1024 * 1024


def main() -> None:
    manifest = json.loads((ROOT / "showcase-inputs.json").read_text())
    if manifest.get("schema") != "fx_showcase_inputs_v1":
        raise ValueError("unsupported recording input manifest")
    base = manifest["delivery_base"].rstrip("/")
    url = urlsplit(base)
    if (
        url.scheme != "https"
        or url.hostname != "fx-cdn.grida.co"
        or url.username
        or url.password
        or url.query
        or url.fragment
    ):
        raise ValueError("recording inputs must use the declared FX HTTPS delivery host")
    directory = ROOT / ".showcase-inputs"
    if directory.is_symlink():
        raise ValueError("input directory must not be a symlink")
    directory.mkdir(exist_ok=True)
    for item in manifest["inputs"]:
        filename, remote = item["filename"], item["path"]
        if Path(filename).name != filename or filename in (".", "..") or not filename:
            raise ValueError("input filename must be a basename")
        if (
            remote.startswith("/")
            or "\\" in remote
            or any(part in ("", ".", "..") for part in remote.split("/"))
        ):
            raise ValueError("remote input path is not confined")
        destination = directory / filename
        if destination.is_symlink():
            raise ValueError("input destination must not be a symlink")
        if not 0 < item["bytes"] <= MAX_BYTES:
            raise ValueError("input exceeds the size limit")
        if destination.exists():
            data = destination.read_bytes()
        else:
            request = urllib.request.Request(
                base + "/" + remote, headers={"User-Agent": "FX canonical promo example/1.0"}
            )
            with urllib.request.urlopen(request, timeout=60) as response:
                if urlsplit(response.url).hostname != url.hostname:
                    raise ValueError("unexpected cross-host delivery redirect")
                data = response.read(item["bytes"] + 1)
        if len(data) != item["bytes"] or hashlib.sha256(data).hexdigest() != item["sha256"]:
            raise ValueError("input checksum mismatch; no existing input was replaced")
        if not destination.exists():
            handle, temporary = tempfile.mkstemp(prefix=".promo-", dir=directory)
            try:
                with os.fdopen(handle, "wb") as file:
                    file.write(data)
                os.replace(temporary, destination)
            finally:
                Path(temporary).unlink(missing_ok=True)
        print(f"Verified {filename}")


if __name__ == "__main__":
    main()
