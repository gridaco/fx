# Example: a game that builds its art with FX

**The ask:** "My game, *Kitewharf*, keeps its levels in its own TOML files. My game code reads
them, and I won't change that format. For every level I want a ground plate, an icon for every
pickup and a looping sky, generated, cached, and copied into the game's engine project. CI must
check that everything still plans, without spending."

```
kitewharf/                              # the game's repo
  levels/docks.toml                     # the game's own level format
  art/far_cliffs.png, art/near_masts.png   # the sky's layer pictures (../draw_placeholders.py)
  assets/                               # the FX project
    fx.yaml
    level_art.py                        # a Python builder: the game's TOML → an FX workflow
    kitewharf_levels.py                 # stand-in for the game's own level reader
    workflows/icon.yaml                 # a plain workflow file, used as a step
    workflows/looping-parallax.yaml     # copied from the looping-parallax example
    nodes/plate.py                      # the game's own node type
    nodes/seams.py, nodes/compose.py    # copied with looping-parallax
    prompts/plate.md, prompts/seam.md
```

Using it adds files that this example does not ship:
- `kitewharf/game/`, the game's engine project, where `--deliver` copies the art;
- `assets/kitewharf-level-art.takes.yaml`, the takes chosen for level builds. A takes file is
  named `<workflow id>.takes.yaml` and sits next to the module that constructs the `Workflow`,
  here `level_art.py`. Commit it.
- `assets/workflows/icon.takes.yaml`, the takes chosen in standalone icon runs.

## Why this one uses a Python builder

The game's level format belongs to the game: its readers live in the game's code. A workflow
file's `inputs:` would mean converting it. A builder instead lets the game's own reader produce the
steps:

```python
# assets/level_art.py
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
```

The builder takes every workflow-file field, with the same names (`judges=`, `on_reject=`,
`regenerate=`, `takes=`, `budget=`, `assert_=` ...), so nothing is YAML-only.

The builder runs only while planning. The values it puts in `with_` are what FX keys on, so
the TOML file is never a hidden input: change a pickup's look and only that icon re-runs.

A file path in `with_` follows the rule of a workflow file: relative to the workflow's home (the
folder with `fx.yaml`, here `assets/`), starting with `./` or `../`. The level file names its sky
layers relative to itself, in `levels/`, so `project_path` rebases each one onto `assets/`. (The
two folders are siblings, so `../art/far_cliffs.png` reads the same from both.) An absolute path
would be refused, and would tie the plan to one machine.

## Building

From `kitewharf/assets/` (its `fx.yaml` lists the guide's [`routes.yaml`](../routes.yaml) under
`route_tables`):

```bash
grida-fx plan level_art.py:build --arg level=../levels/docks.toml
grida-fx run  level_art.py:build --arg level=../levels/docks.toml --live --max-usd 5 \
  --deliver plate=../game/art/docks/ground.png \
  --deliver icons=../game/art/docks/icons/{key}.png \
  --deliver sky=../game/art/docks/sky/{key}.png
```

```
kitewharf-level-art  ·  1 phase
phase 1   20 steps   3–7 provider calls   $0.12 – $2.10
cached    0 of 11 known steps
estimate  $0.12 – $2.10   ceiling $5.00
```

With the guide's prices:
- **`plate`:** one image, $0.04 – $0.30.
- **`icon`:** two pickups, each a drawing and its check, regenerated up to three takes: 2 – 6
  images, $0.08 – $1.80.
- **`sky`:** both of the docks' layers mirror by default, so the sky is free: seven local steps.

**Delivery:**
- **`--deliver`** copies declared outputs out of the run after it succeeds, and only then. Steps
  themselves never write outside FX.
- **Delivery is idempotent:** unchanged files are not rewritten, so the game engine doesn't re-import them.

Or from the game's own build script:

```python
from pathlib import Path

from grida.fx import run
from level_art import build

for level in Path("../levels").glob("*.toml"):
    result = run(build(str(level)), live=True, max_usd=5)
    result.deliver(
        {
            "plate": f"../game/art/{level.stem}/ground.png",
            "icons": f"../game/art/{level.stem}/icons/{{key}}.png",
        }
    )
```

## CI, with no spend

```bash
grida-fx plan level_art.py:build --arg level=../levels/docks.toml --check --expect-cached
```

- **`--check`** fails on any error: a missing tool, an unknown node type, a failed assertion, a
  missing take.
- **`--expect-cached`** fails if anything would be generated. That catches "someone changed a
  level but didn't build its art". It needs the team cache (`cache:` in `fx.yaml` pointing at
  shared storage), or it is skipped.

## What this example tests

- **A foreign authored format** through a Python builder, with no TOML rewrite. The builder emits
  the same graph a workflow file would.
- **Workflows used as steps,** shared with their standalone runs (the same cache entries).
- **Takes committed with the game,** so every teammate gets the same chosen pictures.
- **Delivery** across the consumer boundary.
- **Offline CI.**
