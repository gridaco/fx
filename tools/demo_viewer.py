"""Run the delayed viewer demo with a fresh project and cache every time.

    uv run --project python python tools/demo_viewer.py --open

Three local image transforms are separated by bounded waits. The public run CLI
owns the viewer and execution; this helper only prepares an isolated fixture copy.
Generated files remain in temporary storage for inspection. No provider is enabled.
"""

from __future__ import annotations

import argparse
import os
import shlex
import shutil
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "fixtures" / "viewer"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--wait-seconds",
        type=int,
        choices=range(31),
        default=5,
        metavar="0-30",
        help="seconds per wait (three waits; default: 5)",
    )
    parser.add_argument(
        "--open", action="store_true", help="open the viewer in the default browser"
    )
    args = parser.parse_args()

    project = Path(tempfile.mkdtemp(prefix="fx-viewer-demo-"))
    (project / "nodes").mkdir()
    shutil.copy2(SOURCE / "fx.yaml", project / "fx.yaml")
    shutil.copy2(SOURCE / "workflows/viewer-observation.yaml", project / "workflow.yaml")
    for name in ("media.py", "observation.py"):
        shutil.copy2(SOURCE / "nodes" / name, project / "nodes" / name)
    run = project / "run"
    print(f"Demo run: {run}")
    print(
        "Inspect afterward: "
        + shlex.join(
            [sys.executable, "-m", "grida.fx", "inspect", str(run), "--standalone", "--open"]
        ),
        flush=True,
    )

    environment = os.environ.copy()
    for name in (
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
        "OPENAI_BASE_URL",
        "OPENROUTER_BASE_URL",
        "FAL_BASE_URL",
        "ELEVENLABS_BASE_URL",
    ):
        environment.pop(name, None)
    environment["GRIDA_FX_DISABLE_DOTENV"] = "1"
    environment["GRIDA_FX_NETWORK"] = "off"
    environment["GRIDA_FX_PYTHON"] = sys.executable
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    command = [
        sys.executable,
        "-m",
        "grida.fx",
        "run",
        str(project / "workflow.yaml"),
        "--standalone",
        "--run",
        str(run),
        "--wait-seconds",
        str(args.wait_seconds),
        "--max-usd",
        "0",
    ]
    if args.open:
        command.append("--open")
    os.chdir(project)
    os.execvpe(sys.executable, command, environment)


if __name__ == "__main__":
    main()
