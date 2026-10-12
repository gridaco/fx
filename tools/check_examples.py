"""Plans and runs the examples offline through the Python SDK: what CI checks on a fresh clone
(examples/README.md, "From a clone").

    uv run --project python python tools/check_examples.py [<check> ...]

Each check works on a copy of its example in a temporary folder, so the checkout stays clean and
every run starts with an empty cache. The engine is the one ``tools/build_engine.py`` put beside
the SDK (or ``GRIDA_FX_BIN``); node bodies run in this interpreter (``GRIDA_FX_PYTHON``, unless
set). Nothing runs live: the provider keys are removed from the environment, the network is off,
and a paid call is refused at $0 with ``<capability> on <route> is a paid call; run with
--live``. A check named ``<example>:refused`` runs a workflow that has paid steps, to hold it to
that refusal; the others run only free steps.

Prints one line per check and exits 1 when any fails. Names given run only those checks.
"""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import traceback
from collections.abc import Callable
from pathlib import Path

from PIL import Image, ImageOps

import grida.fx as fx
from grida.fx import RunResult

ROOT = Path(__file__).resolve().parent.parent
EXAMPLES = ROOT / "examples"

#: Every provider key FX reads (spec/providers.md §3), kept out of the runs.
KEYS = (
    "GG_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "FAL_KEY",
    "TRIPO_API_KEY",
    "ELEVENLABS_API_KEY",
)
REFUSED = "is a paid call; run with --live"


class Failed(Exception):
    pass


def expect(condition: object, message: str) -> None:
    if not condition:
        raise Failed(message)


def planned(target: str, home: Path, **options: object) -> fx.Plan:
    plan = fx.plan(target, cwd=home, **options)
    expect(plan.ok, f"the plan of {target} has problems: {plan.problems}")
    return plan


def finished(result: RunResult, what: str) -> RunResult:
    expect(result.ok, f"{what} did not finish ok (failed: {result.failed})")
    expect(result.cost == 0, f"{what} was charged ${result.cost}")
    return result


def refused(result: RunResult, what: str) -> RunResult:
    """An offline run of a workflow with paid steps: $0, not ok, and the refusal says why."""
    expect(not result.ok, f"{what} finished ok offline, though it has paid steps")
    expect(result.cost == 0, f"{what} was charged ${result.cost}")
    said = [event for event in result.events if REFUSED in json.dumps(event)]
    expect(said, f"no event of {what} says a paid call needs --live")
    return result


def edges_meet(path: Path) -> bool:
    """Whether a picture's right edge column equals its left one: it repeats without a seam."""
    with Image.open(path) as opened:
        picture = opened.convert("RGBA")
    width, height = picture.size
    left = picture.crop((0, 0, 1, height)).tobytes()
    return left == picture.crop((width - 1, 0, width, height)).tobytes()


# ------------------------------------------------------------------------------------------------
# The checks, by example


def hello(home: Path) -> str:
    inputs = ["inputs/badges.yaml"]
    plan = planned("hello", home, input_files=inputs)
    expect(plan.estimate() == (0.0, 0.0), f"hello is estimated at {plan.estimate()}")
    first = finished(fx.run("hello", input_files=inputs, cwd=home), "the first run")
    misses = [path for path, step in first.steps.items() if step.cache != "miss"]
    expect(not misses, f"the first run took these from a cache it cannot have: {misses}")
    expect(set(first.outputs["badges"]) == {"sun", "leaf", "stone"}, "the badges are not keyed")
    with Image.open(first.outputs["sheet"].path) as sheet:
        expect(sheet.size == (3 * 64 + 4 * 8, 64 + 2 * 8), f"the sheet is {sheet.size}")
    second = finished(fx.run("hello", input_files=inputs, cwd=home), "the second run")
    ran = [path for path, step in second.steps.items() if step.cache != "hit"]
    expect(not ran, f"the second run ran these again instead of reading the cache: {ran}")
    return f"{len(first.steps)} steps ran at $0, then all {len(second.steps)} from the cache"


def image_recolor(home: Path) -> str:
    subprocess.run([sys.executable, "make_input.py"], cwd=home, check=True)
    inputs = ["inputs/example.yaml"]
    plan = planned("image-recolor", home, input_files=inputs)
    expect(plan.estimate() == (0.0, 0.0), "image-recolor has a nonzero estimate")
    first = finished(fx.run("image-recolor", input_files=inputs, cwd=home), "the first recolor")
    expect(set(first.steps) == {"input", "recolor", "delay"}, "the three image steps did not run")
    expect(all(step.cache == "miss" for step in first.steps.values()), "the first run was cached")
    with Image.open(first.outputs["before"].path) as before:
        red, green, blue, alpha = before.convert("RGBA").split()
        expected = Image.merge("RGBA", (blue, red, green, alpha)).tobytes()
    with Image.open(first.outputs["after"].path) as after:
        expect(after.size == (480, 320), f"the output dimensions changed: {after.size}")
        expect(after.convert("RGBA").tobytes() == expected, "the channel rotation is incorrect")
    expect(first.outputs["before"].digest != first.outputs["after"].digest, "no colors changed")
    delay = next(
        event
        for event in first.events
        if event.get("event") == "node_finished" and event.get("path") == "delay"
    )
    expect(delay["duration_ms"] >= 4900, "the uncached delay did not wait five seconds")
    expect(first.steps["delay"].facts["wait_seconds"] == 5, "the delay lost its parameter")
    second = finished(fx.run("image-recolor", input_files=inputs, cwd=home), "the cached recolor")
    expect(all(step.cache == "hit" for step in second.steps.values()), "a step missed the cache")
    expect(
        second.outputs["after"].digest == first.outputs["after"].digest, "cache changed the image"
    )
    return "three code-only steps at $0, exact recolor and five-second delay; then all cached"


def looping_parallax(home: Path) -> str:
    planned("looping-parallax", home, input_files=["inputs/harbor.yaml"])
    run = fx.run("looping-parallax", input_files=["inputs/mirror-only.yaml"], cwd=home)
    finished(run, "the mirror-only run")
    layers = run.outputs["layers"]
    expect(set(layers) == {"far_cliffs", "near_masts"}, f"layers: {sorted(layers)}")
    seams = [key for key, file in layers.items() if not edges_meet(file.path)]
    expect(not seams, f"these layers do not repeat: {seams}")
    with Image.open(run.outputs["preview"].path) as preview:
        expect(preview.size == (640, 360), f"the preview is {preview.size}")
    manifest = json.loads(Path(run.outputs["manifest"].path).read_text(encoding="utf-8"))
    order = [layer["id"] for layer in manifest["layers"]]
    expect(order == ["far_cliffs", "near_masts"], f"the manifest lists {order}")
    return "planned with a repaint; mirrored both layers at $0"


def looping_parallax_refused(home: Path) -> str:
    run = fx.run("looping-parallax", input_files=["inputs/harbor.yaml"], cwd=home)
    refused(run, "the harbor run")
    expect("layer['far_cliffs'].chosen" in run.steps, "far_cliffs, which mirrors, did not finish")
    return "far_cliffs mirrored; the repaint of near_masts is refused at $0"


def meadow_parallax(home: Path) -> str:
    options = {"input_files": ["inputs/meadow-fixture.yaml"]}
    plan = planned("workflows/meadow.yaml", home, inputs={"render_preview": True}, **options)
    low, high = plan.estimate()
    expect(0 < low <= high, f"meadow-parallax is estimated at {(low, high)}")
    paths = {instance["path"] for instance in plan.document.get("instances", [])}
    expect(
        {"far", "middle", "foreground", "render_preview"} <= paths,
        "the meadow plan is missing extraction or its optional preview renderer",
    )
    without_preview = planned("workflows/meadow.yaml", home, **options)
    expect(
        all(
            instance["path"] != "render_preview"
            for instance in without_preview.document["instances"]
        ),
        "disabling the meadow preview still plans its renderer",
    )
    return f"concept extraction and optional local preview planned at ${low:.2f} - ${high:.2f}"


def meadow_parallax_refused(home: Path) -> str:
    run = fx.run("workflows/meadow.yaml", input_files=["inputs/meadow-fixture.yaml"], cwd=home)
    refused(run, "the meadow run")
    expect(
        not any(event.get("event") == "budget_reserved" for event in run.events),
        "the offline meadow run reserved a provider attempt",
    )
    return "concept-to-layer generation is refused offline at $0"


def concept_gallery(home: Path) -> str:
    plan = planned("concept-gallery", home, input_files=["inputs/tidebell.yaml"])
    low, high = plan.estimate()
    expect(0 < low <= high, f"concept-gallery is estimated at {(low, high)}")
    return f"planned at ${low:.2f} – ${high:.2f}"


def concept_gallery_refused(home: Path) -> str:
    run = fx.run("concept-gallery", input_files=["inputs/tidebell.yaml"], cwd=home)
    refused(run, "the gallery run")
    expect("poster_small" in run.steps, "the free resize of the poster did not run")
    return "the poster was resized; the first paid call is refused at $0"


GAME = "kitewharf/assets"  # inside the game-build example
BUILDER = {"target": "level_art.py:build", "arguments": {"level": "../levels/docks.toml"}}


def game_build(home: Path) -> str:
    plan = planned(BUILDER["target"], home / GAME, arguments=BUILDER["arguments"])
    low, high = plan.estimate()
    return f"the builder planned at ${low:.2f} – ${high:.2f}"


def game_build_refused(home: Path) -> str:
    run = fx.run(BUILDER["target"], arguments=BUILDER["arguments"], cwd=home / GAME)
    refused(run, "the level art run")
    mirrored = [path for path in run.steps if path.endswith(".mirror")]
    expect(len(mirrored) == 2, f"the sky mirrored {mirrored}")
    return "the sky mirrored; the plate and the icons are refused at $0"


PET = ["inputs/pebble.yaml"]
PET_ROWS = [
    ("idle", 6),
    ("running-right", 8),
    ("running-left", 8),
    ("waving", 4),
    ("jumping", 5),
    ("failed", 8),
    ("waiting", 6),
    ("running", 6),
    ("review", 6),
]


def pet(home: Path) -> str:
    plan = planned("pet", home, input_files=PET)
    expect(plan.estimate() == (0.0, 0.0), f"pet, all on Codex, is estimated at {plan.estimate()}")
    loaded = importlib.util.spec_from_file_location("pet_stand_in", home / "stand_in.py")
    expect(loaded is not None and loaded.loader is not None, "the pet's stand-in does not load")
    stand_in = importlib.util.module_from_spec(loaded)
    loaded.loader.exec_module(stand_in)
    first = finished(
        fx.run("pet", input_files=PET, cwd=home, stand_in=stand_in.answer), "the stand-in run"
    )
    width, height = 192, 208
    with Image.open(first.outputs["spritesheet"].path) as opened:
        sheet = opened.convert("RGBA")
    expect(sheet.size == (8 * width, 9 * height), f"the sheet is {sheet.size}")

    def cell(row: int, column: int) -> Image.Image:
        return sheet.crop((column * width, row * height, (column + 1) * width, (row + 1) * height))

    for row, (state, frames) in enumerate(PET_ROWS):
        for column in range(8):
            empty = cell(row, column).getchannel("A").getbbox() is None
            expect(empty == (column >= frames), f"{state} cell {column}: empty is {empty}")
    flipped = [ImageOps.mirror(cell(1, column)).tobytes() for column in range(8)]
    left = [cell(2, column).tobytes() for column in range(8)]
    expect(flipped == left, "running-left is not running-right flipped frame by frame")
    manifest = json.loads(Path(first.outputs["pet"].path).read_text(encoding="utf-8"))
    expect(
        manifest
        == {
            "id": "pebble",
            "displayName": "Pebble",
            "description": "A small river stone that keeps you company while you work.",
            "spritesheetPath": "spritesheet.webp",
        },
        f"pet.json is {manifest}",
    )
    previews = first.outputs["previews"]
    expect(list(previews) == [state for state, _ in PET_ROWS], f"previews: {list(previews)}")
    with Image.open(previews["waving"].path) as gif:
        expect(gif.n_frames == 4, f"the waving preview has {gif.n_frames} frames")
    second = finished(
        fx.run("pet", input_files=PET, cwd=home, stand_in=stand_in.answer), "the second run"
    )
    # `base` is fx/select@1, which the engine runs every time: it keeps no result to read.
    ran = [p for p, step in second.steps.items() if step.cache == "miss" and p != "base"]
    expect(not ran, f"the second run ran these again instead of reading the cache: {ran}")
    # From a picture: the base is an edit of it, and every strip is drawn again from that base.
    # The picture is synthesized here.
    reference = home / "reference.png"
    Image.new("RGBA", (320, 320), (200, 120, 60, 255)).save(reference)
    third = finished(
        fx.run(
            "pet",
            input_files=PET,
            inputs={"reference": str(reference)},
            cwd=home,
            stand_in=stand_in.answer,
        ),
        "the run from a picture",
    )
    finished_steps = set(third.steps)
    expect("sketch" not in finished_steps, "the base was sketched although a picture was given")
    expect("redraw" in finished_steps, "the base was not redrawn from the picture")
    drawn = [path for path, step in third.steps.items() if path.endswith(".draw")]
    expect(
        drawn and all(third.steps[path].cache == "miss" for path in drawn),
        "the strips drawn from another base picture came from the cache",
    )
    return (
        "planned at $0 on Codex; a stand-in drew a base and 8 strips from it into a 9-state "
        "Codex pet, then cached; from a picture, the base is an edit and the strips redrawn"
    )


def pet_refused(home: Path) -> str:
    refused(fx.run("pet", input_files=PET, cwd=home), "the pet run")
    return "every strip is a Codex call, refused offline at $0"


def rigged_character(home: Path) -> str:
    plan = planned("rigged-character", home, input_files=["inputs/wren.yaml"])
    low, high = plan.estimate()
    return f"planned at ${low:.2f} – ${high:.2f} (plan-only: no review adapter, no Blender here)"


CHECKS: dict[str, tuple[str, Callable[[Path], str]]] = {
    "hello": ("hello", hello),
    "image-recolor": ("image-recolor", image_recolor),
    "looping-parallax": ("looping-parallax", looping_parallax),
    "looping-parallax:refused": ("looping-parallax", looping_parallax_refused),
    "looping-parallax:meadow": ("looping-parallax", meadow_parallax),
    "looping-parallax:meadow-refused": ("looping-parallax", meadow_parallax_refused),
    "concept-gallery": ("concept-gallery", concept_gallery),
    "concept-gallery:refused": ("concept-gallery", concept_gallery_refused),
    "game-build": ("game-build", game_build),
    "game-build:refused": ("game-build", game_build_refused),
    "pet": ("pet", pet),
    "pet:refused": ("pet", pet_refused),
    "rigged-character": ("rigged-character", rigged_character),
}


def offline() -> None:
    for key in KEYS:
        os.environ.pop(key, None)
    os.environ["GRIDA_FX_NETWORK"] = "off"
    os.environ["GRIDA_FX_DISABLE_DOTENV"] = "1"
    os.environ.setdefault("GRIDA_FX_PYTHON", sys.executable)


def main(names: list[str]) -> int:
    unknown = sorted(set(names) - set(CHECKS))
    if unknown:
        print(f"check_examples: no check {', '.join(unknown)}; the checks: {', '.join(CHECKS)}")
        return 2
    offline()
    failures = 0
    for name, (example, check) in CHECKS.items():
        if names and name not in names:
            continue
        with tempfile.TemporaryDirectory(prefix="fx-example-") as scratch:
            home = Path(scratch) / example
            shutil.copytree(
                EXAMPLES / example,
                home,
                ignore=shutil.ignore_patterns(".fx", "runs", ".venv", ".showcase-inputs"),
            )
            try:
                print(f"ok   {name}: {check(home)}")
            except Failed as failure:
                failures += 1
                print(f"FAIL {name}: {failure}")
            except Exception:  # an SDK or engine error is a failed check, not a crash
                failures += 1
                print(f"FAIL {name}:\n{traceback.format_exc()}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
