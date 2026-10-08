"""Prepare a fresh provider-free cancellation project and print commands to try it.

Does not start a service, open a browser or execute a workflow. The copied project
is retained so the user can run, cancel and resume it without cache surprises.
"""

from __future__ import annotations

import shlex
import shutil
import sys
import tempfile
from pathlib import Path

SOURCE = Path(__file__).resolve().parents[1] / "fixtures/control"


def main() -> None:
    project = Path(tempfile.mkdtemp(prefix="fx-control-demo-"))
    for name in ("fx.yaml", "workflow.yaml", "nodes.py", ".gitignore"):
        shutil.copy2(SOURCE / name, project / name)
    prefix = shlex.join([sys.executable, "-m", "grida.fx"])
    print(f"Project: {project}\n")
    print(f"cd {shlex.quote(str(project))}")
    print(f"{prefix} start --background --port 8790 --open")
    print(f"{prefix} run workflow.yaml --name test --max-usd 0 --open")
    print("\nWhile hold is running, from another terminal in this same folder:")
    print(f"{prefix} inspect control-harness/test --control --json")
    print(f"{prefix} cancel control-harness/test --wait --timeout 1s --json")
    print(f"{prefix} cancel control-harness/test --wait --json")
    print("\nThe first wait times out; cancellation remains effective. Then resume:")
    print("touch .control-release")
    print(f"{prefix} run workflow.yaml --resume test --max-usd 0 --open")
    print(f"{prefix} stop")
    print("\nNo providers or charges. The viewer stays available until stop.")
    print("Choose another exact port if 8790 is occupied. Run this script again for fresh data.")


if __name__ == "__main__":
    main()
