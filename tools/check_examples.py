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

import json
import os
import shutil
import subprocess
import sys
import tempfile
import traceback
from collections.abc import Callable
from pathlib import Path

from PIL import Image

import grida.fx as fx
from grida.fx import RunResult

ROOT = Path(__file__).resolve().parent.parent
EXAMPLES = ROOT / "examples"

#: Every provider key FX reads (spec/providers.md §3), kept out of the runs.
KEYS = (
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


def rigged_character(home: Path) -> str:
    plan = planned("rigged-character", home, input_files=["inputs/wren.yaml"])
    low, high = plan.estimate()
    return f"planned at ${low:.2f} – ${high:.2f} (plan-only: no review adapter, no Blender here)"


CHECKS: dict[str, tuple[str, Callable[[Path], str]]] = {
    "hello": ("hello", hello),
    "image-recolor": ("image-recolor", image_recolor),
    "looping-parallax": ("looping-parallax", looping_parallax),
    "looping-parallax:refused": ("looping-parallax", looping_parallax_refused),
    "concept-gallery": ("concept-gallery", concept_gallery),
    "concept-gallery:refused": ("concept-gallery", concept_gallery_refused),
    "game-build": ("game-build", game_build),
    "game-build:refused": ("game-build", game_build_refused),
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
                EXAMPLES / example, home, ignore=shutil.ignore_patterns(".fx", "runs", ".venv")
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
