"""Capture an original scripted browser fixture, not a shipping product or a model output.

Optional tool: uv run --with playwright python make_dashboard.py --browser /path/to/chrome
The main workflow and its provider-free checks do not require a browser.
"""

from __future__ import annotations

import argparse
import io
import json
import subprocess
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent
SIZE = (1600, 1000)
FPS = 12
SECONDS = 44

HTML = (ROOT / "fixtures/dashboard.html").read_text()


def main() -> None:
    from playwright.sync_api import sync_playwright

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", nargs="?", type=Path, default=ROOT / ".fixture")
    parser.add_argument("--browser", type=Path)
    args = parser.parse_args()
    directory = args.directory.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    html = directory / "dashboard.html"
    html.write_text(HTML.replace("FONT_URL", (ROOT / "fonts/Manrope.ttf").as_uri()))
    destination = directory / "dashboard.mp4"
    encode = subprocess.Popen(
        [
            "ffmpeg",
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            f"{SIZE[0]}x{SIZE[1]}",
            "-r",
            str(FPS),
            "-i",
            "-",
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-crf",
            "17",
            "-threads",
            "2",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            str(destination),
        ],
        stdin=subprocess.PIPE,
    )
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(
                headless=True, **({"executable_path": str(args.browser)} if args.browser else {})
            )
            page = browser.new_page(
                viewport={"width": SIZE[0], "height": SIZE[1]}, device_scale_factor=1
            )
            page.goto(html.as_uri())
            page.evaluate("document.fonts.ready")
            for index in range(FPS * SECONDS):
                page.evaluate("time => window.renderAt(time)", index / FPS)
                with Image.open(io.BytesIO(page.screenshot())) as picture:
                    encode.stdin.write(picture.convert("RGB").tobytes())
                if index % (FPS * 5) == 0:
                    print(
                        f"Captured original browser fixture: {index / FPS:.0f}/{SECONDS}s",
                        flush=True,
                    )
            browser.close()
        encode.stdin.close()
        if encode.wait(timeout=120):
            raise RuntimeError("FFmpeg refused the browser fixture capture")
    finally:
        if encode.poll() is None:
            encode.kill()
            encode.wait()
    (directory / "dashboard.json").write_text(
        json.dumps(
            {
                "footage": "dashboard.mp4",
                "brief": "../inputs/dashboard.json",
            },
            indent=2,
        )
        + "\n"
    )
    (directory / "dashboard-source.json").write_text(
        json.dumps(
            {
                "kind": "original_scripted_browser_fixture_v1",
                "seconds": SECONDS,
                "fps": FPS,
                "size": list(SIZE),
                "network": False,
                "shipping_product_claim": False,
                "operation": (
                    "A supplied table becomes a dashboard, then chat focuses returning customers."
                ),
                "capture": (
                    "Original DOM and CSS rendered by Chromium at authored timeline positions."
                ),
            },
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
