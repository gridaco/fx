"""Prepare a fresh code-only project and print the public project-service commands.

    uv run --offline --no-sync --project python python tools/demo_service.py

Does not start a service, open a browser or execute a workflow. The temporary
project is retained for the user; all images are drawn by ordinary local code.
"""

from __future__ import annotations

import os
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SOURCE = Path(__file__).resolve().parent.parent / "fixtures" / "viewer"


def main() -> None:
    project = Path(tempfile.mkdtemp(prefix="fx-service-demo-"))
    (project / "nodes").mkdir()
    (project / "workflows").mkdir()
    for name in ("media.py", "observation.py"):
        shutil.copy2(SOURCE / "nodes" / name, project / "nodes" / name)
    shutil.copy2(
        SOURCE / "workflows" / "viewer-observation.yaml",
        project / "workflows" / "viewer-observation.yaml",
    )
    command = [sys.executable, "-m", "grida.fx"]
    environment = os.environ.copy()
    environment["GRIDA_FX_DISABLE_DOTENV"] = "1"
    subprocess.run([*command, "init", str(project)], env=environment, check=True)
    prefix = shlex.join(command)
    print(f"\nProject: {project}\n")
    print(f"cd {shlex.quote(str(project))}")
    print(f"{prefix} start --background --open")
    print(f"{prefix} plan viewer-observation --open")
    print(f"{prefix} run viewer-observation --name baseline --wait-seconds 8 --max-usd 0 --open")
    print(f"{prefix} run viewer-observation --name variant --wait-seconds 9 --max-usd 0 --open")
    print(f"{prefix} run viewer-observation --resume baseline --wait-seconds 8 --max-usd 0")
    print(f"{prefix} inspect viewer-observation/baseline --verify --json")
    print(f"{prefix} inspect viewer-observation --open")
    print(f"{prefix} status --json")
    print(f"{prefix} stop")
    print("\nRuns stay inspectable after each command exits, until you stop FX.")
    print("The dashboard groups both runs and the plan under one workflow.")
    print("Names create once; --resume retains that record and requires the same inputs.")
    print("Omit the name for fresh history; identical inputs can reuse cache.")
    print("If port 8787 is occupied, select an exact port with start --port NUMBER.")


if __name__ == "__main__":
    main()
