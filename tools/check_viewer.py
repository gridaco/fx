"""Check that an installed command serves its embedded viewer outside the checkout.

    python3 tools/check_viewer.py --command '/path/to/grida-fx'
    python3 tools/check_viewer.py --command '/path/to/python -m grida.fx'

Uses temporary synthetic plan/run records and loopback HTTP, with providers disabled.
Every built frontend file must be served byte-for-byte in both modes. No browser or
frontend runtime is needed.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path
from urllib.parse import quote

ROOT = Path(__file__).resolve().parent.parent
PROVIDER_KEYS = (
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "FAL_KEY",
    "TRIPO_API_KEY",
    "ELEVENLABS_API_KEY",
)


def check(command: list[str], assets: Path) -> int:
    if not command or not (assets / "index.html").is_file():
        raise ValueError("a command and a built web/viewer/dist/index.html are required")
    files = sorted(path for path in assets.rglob("*") if path.is_file())
    if not any(path.suffix == ".js" for path in files) or not any(
        path.suffix == ".css" for path in files
    ):
        raise ValueError("the viewer bundle must include JavaScript and CSS")
    environment = dict(os.environ)
    for key in PROVIDER_KEYS:
        environment.pop(key, None)
    environment.pop("GRIDA_FX_BIN", None)
    environment["GRIDA_FX_DISABLE_DOTENV"] = "1"
    environment["GRIDA_FX_NETWORK"] = "off"
    with tempfile.TemporaryDirectory(prefix="fx-viewer-check-") as folder:
        temporary = Path(folder)
        run = temporary / "run"
        run.mkdir()
        graph = {
            "kind": "fx-graph-v1",
            "plan": "0" * 64,
            "workflow": {"id": "preview", "title": "Preview"},
            "instances": [],
            "pending": [],
            "estimate": {"low_usd": 0, "high_usd": 0, "ceiling_usd": None},
        }
        saved_plan = run / "plan.json"
        saved_plan.write_text(json.dumps(graph), encoding="utf-8")
        for source, target in [("--run", run), ("--plan", saved_plan)]:
            log = temporary / "viewer.log"
            with log.open("w", encoding="utf-8") as output:
                process = subprocess.Popen(
                    [*command, "view", source, str(target), "--port", "0", "--no-open"],
                    cwd=temporary,
                    env=environment,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                )
            try:
                deadline = time.monotonic() + 15
                while True:
                    logged = log.read_text(encoding="utf-8")
                    address = re.search(r"^http://127\.0\.0\.1:\d+/\s*$", logged, re.MULTILINE)
                    if address:
                        base = address.group().strip()
                        break
                    if process.poll() is not None:
                        raise ValueError(
                            f"{source}: viewer exited before listening: {logged.strip()}"
                        )
                    if time.monotonic() >= deadline:
                        raise ValueError(
                            f"{source}: viewer did not report a loopback URL: {logged.strip()}"
                        )
                    time.sleep(0.05)
                client = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                with client.open(base + "api/view", timeout=5) as response:
                    document = json.load(response)
                if source == "--plan":
                    if document != graph:
                        raise ValueError("installed viewer changed the saved plan")
                elif document.get("kind") != "fx-viewer-run-v1":
                    raise ValueError("installed viewer returned no run projection")
                # The layout report: automatic cells for every view, never cached.
                with client.open(base + "api/layout", timeout=5) as response:
                    layout = json.load(response)
                    if (
                        response.headers.get("Cache-Control") != "no-store"
                        or response.headers.get("ETag") is not None
                    ):
                        raise ValueError(f"{source}: layout report may be cached")
                if layout.get("kind") != "fx-layout-report-v1" or not isinstance(
                    layout.get("cells"), dict
                ):
                    raise ValueError(f"{source}: installed viewer returned no layout report")
                if (layout.get("cursor") is None) != (source == "--plan"):
                    raise ValueError(f"{source}: only a run's layout report has a cursor")
                for path in files:
                    relative = path.relative_to(assets).as_posix()
                    url = base if relative == "index.html" else base + quote(relative)
                    with client.open(url, timeout=5) as response:
                        if response.status != 200 or response.read() != path.read_bytes():
                            raise ValueError(
                                f"{source}: embedded viewer differs from its build: {relative}"
                            )
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        return len(files)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--command", required=True, help="installed command, with absolute paths")
    parser.add_argument("--assets", type=Path, default=ROOT / "web" / "viewer" / "dist")
    args = parser.parse_args()
    try:
        count = check(shlex.split(args.command), args.assets.resolve())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"check_viewer: {error}", file=sys.stderr)
        return 1
    print(f"check_viewer: {count} embedded files match in plan/run modes outside the checkout")
    return 0


if __name__ == "__main__":
    sys.exit(main())
