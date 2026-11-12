"""Generate and serve the focused, entirely local media-preview fixture.

    uv run --project python python tools/demo_media_previews.py --open

The canonical fixture checker prepares and verifies a fresh saved run. The public
viewer then serves that recording without rerunning any nodes or using a provider.
Generated files stay outside the fixture source and remain available afterward.
"""

from __future__ import annotations

import argparse
import os
import shlex
import sys
import tempfile
from pathlib import Path

from check_viewer_fixtures import generate_media_previews, prepare_output


def port(value: str) -> int:
    number = int(value)
    if not 0 <= number <= 65535:
        raise argparse.ArgumentTypeError("port must be between 0 and 65535")
    return number


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output", type=Path, help="new or empty destination; default: fresh temporary storage"
    )
    parser.add_argument(
        "--port", type=port, default=0, help="loopback port; 0 selects an available port"
    )
    parser.add_argument(
        "--open", action="store_true", help="open the saved run in the default browser"
    )
    parser.add_argument(
        "--prepare-only",
        action="store_true",
        help="verify and retain the run without starting its viewer",
    )
    args = parser.parse_args()

    destination = args.output or Path(tempfile.mkdtemp(prefix="fx-media-previews-"))
    destination = prepare_output(destination)
    generate_media_previews(destination)
    run = destination / "runs" / "media-previews"
    command = [sys.executable, "-m", "grida.fx", "view", "--run", str(run)]
    print(f"\nVerified media preview run: {run}")
    print(
        "Image, video, unsupported video, custom posters, static 3D and animated 3D; "
        "$0, no providers."
    )
    print("Inspect afterward: " + shlex.join([*command, "--open"]), flush=True)
    if args.prepare_only:
        return
    command.extend(["--port", str(args.port)])
    if args.open:
        command.append("--open")
    os.execvpe(sys.executable, command, os.environ.copy())


if __name__ == "__main__":
    main()
