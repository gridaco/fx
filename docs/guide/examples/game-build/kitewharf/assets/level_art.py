"""The game's level TOML into an FX workflow: the game's reader, unchanged, makes the steps."""

import os
from pathlib import Path

from kitewharf_levels import read_level  # the game's reader (installed with the game)

from grida.fx import Workflow

HOME = Path(__file__).resolve().parent  # the folder with fx.yaml: where ./ and ../ paths start


def project_path(file: Path) -> str:
    """A file as a workflow names it: relative to HOME, starting with ./ or ../, never absolute."""
    relative = Path(os.path.relpath(file.resolve(), HOME)).as_posix()
    return relative if relative.startswith("../") else f"./{relative}"


def build(level: str) -> Workflow:
    spec = read_level(Path(level))
    wf = Workflow("kitewharf-level-art", title=f"Level art: {spec.name}")

    plate = wf.step(
        "plate",
        uses="./nodes/plate.py#ground_plate",
        with_={
            "material": spec.ground["material"],
            "span_m": spec.ground["span_m"],
            "light": spec.light,
        },
    )
    icons = wf.step(
        "icon",
        for_each=[{"id": p["id"], "name": p["look"]} for p in spec.pickups],
        key="${{ item.id }}",
        uses="./workflows/icon.yaml",
        with_={"name": "${{ item.name }}"},
    )
    layers = [{**layer, "file": project_path(layer["file"])} for layer in spec.sky["layers"]]
    sky = wf.step(
        "sky",
        uses="./workflows/looping-parallax.yaml",
        with_={"canvas": spec.sky["canvas"], "layers": layers},
    )
    wf.outputs(plate=plate.outputs.image, icons=icons.all.outputs.icon, sky=sky.outputs.layers)
    return wf
