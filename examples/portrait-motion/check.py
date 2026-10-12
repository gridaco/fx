"""Run both synthesized portrait paths with stand-ins; no provider credentials needed."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

from inputs.make_inputs import stand_in, write_inputs, write_whole
from PIL import Image

from grida.fx import plan, run


def main() -> None:
    home = Path(__file__).resolve().parent
    os.environ["GRIDA_FX_DISABLE_DOTENV"] = "1"
    os.environ["GRIDA_FX_NETWORK"] = "off"
    os.environ["GRIDA_FX_PYTHON"] = sys.executable
    for name in (
        "GG_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
    ):
        os.environ.pop(name, None)
    for make, expected_patches in ((write_inputs, True), (write_whole, False)):
        inputs = make(home / ".fixtures")
        planned = plan("workflow.yaml", cwd=home, input_files=[inputs])
        if not planned.ok:
            raise RuntimeError(planned.problems)
        result = run("workflow.yaml", cwd=home, input_files=[inputs], stand_in=stand_in)
        if not result.ok or result.cost != 0:
            raise RuntimeError(f"offline fixture failed: {result.failed}; cost={result.cost}")
        decision = json.loads(result.outputs["result"].read_bytes())
        if decision["status"] != "complete":
            raise RuntimeError(f"the fixture was refused: {decision}")
        if ("patches" in result.outputs) != expected_patches:
            raise RuntimeError("the face and whole-portrait branches disagreed")
        with Image.open(result.outputs["animation"].path) as image:
            if image.format != "WEBP" or image.n_frames < 2:
                raise RuntimeError("the fixture did not produce animated WebP")
        print(f"{inputs.name}: complete at $0; {len(result.outputs['states'])} combinations")


if __name__ == "__main__":
    main()
