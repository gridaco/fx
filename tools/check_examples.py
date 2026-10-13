"""Plans and runs the examples offline through the Python SDK: what CI checks on a fresh clone
(examples/README.md, "From a clone").

    uv run --project python python tools/check_examples.py [<check> ...]

Each check works on a copy of its example in a temporary folder, so the checkout stays clean and
every run starts with an empty cache. The engine is the one ``tools/build_engine.py`` put beside
the SDK (or ``GRIDA_FX_BIN``); node bodies run in this interpreter (``GRIDA_FX_PYTHON``, unless
set). An example with a ``package.json`` is a TypeScript project: its copy gets the checkout's
built ``js/fx`` as ``node_modules/@grida/fx``, as ``npm install`` would put it, and is type checked
with ``js/fx``'s own ``tsc`` and ``@types/node`` before it runs; its bodies run on Node 24 or later
(``GRIDA_FX_NODE``, else ``node`` on ``PATH``).

Nothing runs live: the provider keys are removed from the environment, the network is off, and a
paid call is refused at $0 with ``<capability> on <route> is a paid call; run with --live``. A
check named ``<example>:refused`` runs a workflow that has paid steps, to hold it to that refusal;
the others run only free steps, or have the example's stand-in answer the paid ones.
``self-contained`` checks, without running anything, that no example links outside its own folder
except by absolute URL.

An example ``examples/templates.json`` offers as an ``init`` template is not copied: the engine
starts it with ``init --template``, from an archive shaped like the release's (one top-level
folder, the examples' tracked files as they are in the working tree) served on loopback, so each
check runs on exactly what a user gets. ``templates`` checks that every template has a check and
that no indexed folder is ``export-ignore``d out of the release archive.

Prints one line per check and exits 1 when any fails. Names given run only those checks.
"""

from __future__ import annotations

import gzip
import http.server
import importlib.util
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import traceback
from collections.abc import Callable
from pathlib import Path

from PIL import Image, ImageOps

import grida.fx as fx
from grida.fx import RunResult, _api

ROOT = Path(__file__).resolve().parent.parent
EXAMPLES = ROOT / "examples"
#: The checkout's JavaScript SDK: its ``dist`` is built by ``bun run build`` there.
JS_SDK = ROOT / "js" / "fx"

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


def hello_ts(home: Path) -> str:
    inputs = ["inputs/badges.yaml"]
    plan = planned("hello", home, input_files=inputs)
    expect(plan.estimate() == (0.0, 0.0), f"hello-ts is estimated at {plan.estimate()}")
    first = finished(fx.run("hello", input_files=inputs, cwd=home), "the first run")
    misses = [path for path, step in first.steps.items() if step.cache != "miss"]
    expect(not misses, f"the first run took these from a cache it cannot have: {misses}")
    expect(len(first.steps) == 10, f"the first run ran {len(first.steps)} steps, not 10")
    expect(list(first.outputs["badges"]) == ["sun", "leaf", "stone"], "the badges are not keyed")
    for key, file in first.outputs["badges"].items():
        with Image.open(file.path) as badge:
            expect(badge.size == (64, 64), f"badge {key} is {badge.size}")
            expect(badge.getchannel("A").getextrema()[0] == 0, f"badge {key} has no clear pixel")
    with Image.open(first.outputs["sheet"].path) as sheet:
        expect(sheet.size == (3 * 64 + 4 * 8, 64 + 2 * 8), f"the sheet is {sheet.size}")
    second = finished(fx.run("hello", input_files=inputs, cwd=home), "the second run")
    ran = [path for path, step in second.steps.items() if step.cache != "hit"]
    expect(not ran, f"the second run ran these again instead of reading the cache: {ran}")
    return f"{len(first.steps)} TypeScript steps ran at $0, then all from the cache"


ITEM_ICONS = "workflows/icons.ts:build"
ITEMS = ["copper_key", "storm_lantern", "moon_compass"]


def stand_in_run(home: Path, folder: str, *options: str) -> RunResult:
    """A run of item-icons answered by its TypeScript stand-in, through the engine's command line
    (the SDK's ``stand_in`` takes a Python function)."""
    command = [str(_api.binary()), "run", ITEM_ICONS, "--stand-in", "stand_in.ts#answer"]
    done = subprocess.run(
        [*command, "--run", folder, *options, "--no-view"],
        cwd=home,
        env=_api.engine_environment(),
        capture_output=True,
        text=True,
    )
    expect(done.returncode == 0, f"the stand-in run exited {done.returncode}: {done.stderr}")
    result = RunResult(home / folder, _api._read_events(home / folder))
    result._store = _api._project_store(home)
    return finished(result, f"the stand-in run {folder}")


def item_icons(home: Path) -> str:
    plan = planned(ITEM_ICONS, home)
    low, high = plan.estimate()
    expect(0 < low <= high, f"item-icons is estimated at {(low, high)}")
    paths = sorted(instance["path"] for instance in plan.document["instances"])
    expect(paths == sorted(f"icon['{item}']" for item in ITEMS), f"the plan has {paths}")
    shutil.rmtree(home / "public" / "icons", ignore_errors=True)
    first = stand_in_run(home, "runs/first", "--deliver", "icons=public/icons/{key}.png")
    expect(first.stand_in, "the run does not say it was answered by a stand-in")
    expect(list(first.outputs["icons"]) == ITEMS, "the icons are not keyed by the items")
    for item in ITEMS:
        with Image.open(home / "public" / "icons" / f"{item}.png") as icon:
            expect(icon.size == (1024, 1024), f"the icon of {item} is {icon.size}")
            expect(icon.getchannel("A").getextrema()[0] == 0, f"the icon of {item} is opaque")
    second = stand_in_run(home, "runs/second")
    ran = [path for path, step in second.steps.items() if step.cache != "hit"]
    expect(not ran, f"the second stand-in run asked again for {ran}")
    return f"planned at ${low:.2f} – ${high:.2f}; the stand-in drew and delivered 3 icons at $0"


def item_icons_refused(home: Path) -> str:
    refused(fx.run(ITEM_ICONS, cwd=home), "the icons run")
    return "each icon is refused at $0 without a stand-in"


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
    "hello-ts": ("hello-ts", hello_ts),
    "image-recolor": ("image-recolor", image_recolor),
    "looping-parallax": ("looping-parallax", looping_parallax),
    "looping-parallax:refused": ("looping-parallax", looping_parallax_refused),
    "looping-parallax:meadow": ("looping-parallax", meadow_parallax),
    "looping-parallax:meadow-refused": ("looping-parallax", meadow_parallax_refused),
    "item-icons": ("item-icons", item_icons),
    "item-icons:refused": ("item-icons", item_icons_refused),
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


def typescript(home: Path, check_types: bool) -> None:
    """Installs the checkout's ``@grida/fx`` into a TypeScript example's copy, as ``npm install``
    would (copied, not linked), with ``js/fx``'s ``typescript`` and ``@types/node``, then, with
    ``check_types``, checks the example's types with them."""
    expect((JS_SDK / "dist" / "host-entry.js").is_file(), "js/fx is not built: bun run build there")
    modules = home / "node_modules"
    (modules / "@grida" / "fx").mkdir(parents=True)
    shutil.copy2(JS_SDK / "package.json", modules / "@grida" / "fx" / "package.json")
    shutil.copytree(JS_SDK / "dist", modules / "@grida" / "fx" / "dist")
    (modules / "@types").mkdir()
    (modules / "@types" / "node").symlink_to(JS_SDK / "node_modules" / "@types" / "node")
    (modules / "typescript").symlink_to(JS_SDK / "node_modules" / "typescript")
    if not check_types:
        return
    node = os.environ.get("GRIDA_FX_NODE", "node")
    checked = subprocess.run(
        [node, str(modules / "typescript" / "bin" / "tsc"), "-p", str(home)],
        capture_output=True,
        text=True,
    )
    expect(checked.returncode == 0, f"tsc found errors:\n{checked.stdout}{checked.stderr}")


#: A markdown link's target, as written: ``[text](target)``.
LINK = re.compile(r"\]\(([^)\s]+)\)")


def self_contained() -> list[str]:
    """Every markdown link in an example that leaves its folder, as ``file: target``.

    An example is copied out on its own (a template, a user's project), so a link to anything
    outside it must be an absolute URL; links within it stay relative (AGENTS.md, "Examples run").
    """
    found = []
    for example in sorted(path for path in EXAMPLES.iterdir() if path.is_dir()):
        for page in sorted(example.rglob("*.md")):
            if any(part in {".venv", "runs", ".fx", "node_modules"} for part in page.parts):
                continue
            for target in LINK.findall(page.read_text(encoding="utf-8")):
                if re.match(r"^([a-z][a-z0-9+.-]*:|#)", target):
                    continue
                resolved = (page.parent / target.split("#")[0]).resolve()
                if not resolved.is_relative_to(example.resolve()):
                    found.append(f"{page.relative_to(ROOT)}: {target}")
    return found


def templates() -> dict[str, str]:
    """The templates ``init`` offers, by example folder name: ``{folder: template name}``."""
    index = json.loads((EXAMPLES / "templates.json").read_text(encoding="utf-8"))
    return {Path(entry["path"]).name: entry["name"] for entry in index["templates"]}


def template_problems(offered: dict[str, str]) -> list[str]:
    problems = []
    checked = {example for example, _ in CHECKS.values()}
    for folder, name in offered.items():
        if folder not in checked:
            problems.append(f"template {name} ({folder}) has no check here")
        attribute = subprocess.run(
            ["git", "check-attr", "export-ignore", "--", f"examples/{folder}/fx.yaml"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        if not attribute.rstrip().endswith("unspecified"):
            problems.append(f"examples/{folder} is left out of the release archive: {attribute}")
    return problems


def release_archive(folders: list[str]) -> bytes:
    """A ``.tar.gz`` shaped like the tag's source archive: a pax global header, one top-level
    folder, and the tracked files of ``folders`` as they are in the working tree."""
    listed = subprocess.run(
        ["git", "ls-files", "-z", "--", *(f"examples/{folder}" for folder in folders)],
        cwd=ROOT,
        capture_output=True,
        check=True,
    ).stdout
    buffer = io.BytesIO()
    with tarfile.open(
        fileobj=buffer,
        mode="w",
        format=tarfile.PAX_FORMAT,
        pax_headers={"comment": "0" * 40},
    ) as archive:
        for name in sorted(filter(None, listed.decode().split("\0"))):
            path = ROOT / name
            if path.is_file():
                archive.add(path, arcname=f"fx-check/{name}", recursive=False)
    return gzip.compress(buffer.getvalue())


def serve(archive: bytes) -> tuple[str, http.server.ThreadingHTTPServer]:
    """Serves ``archive`` on loopback, as codeload serves a tag's archive."""

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802 (the handler's interface)
            self.send_response(200)
            self.send_header("content-type", "application/x-gzip")
            self.send_header("content-length", str(len(archive)))
            self.end_headers()
            self.wfile.write(archive)

        def log_message(self, *args: object) -> None:
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{server.server_address[1]}/gridaco/fx/tar.gz/vcheck", server


def started(name: str, home: Path, source: str) -> None:
    """Starts ``home`` from template ``name`` with the engine's own ``init`` (network on, for the
    loopback source only)."""
    environment = _api.engine_environment()
    environment.pop("GRIDA_FX_NETWORK", None)
    command = [str(_api.binary()), "init", str(home), "--template", name]
    result = subprocess.run(
        [*command, "--template-source", source, "--json"],
        env=environment,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise Failed(f"init --template {name} failed: {result.stderr.strip()}")
    outcome = json.loads(result.stdout)
    expect(outcome["template"] == {"name": name, "source": "local"}, f"init said {outcome}")
    for file in ("fx.yaml", "AGENTS.md", ".gitignore", ".agents/skills/grida-fx/SKILL.md"):
        expect((home / file).is_file(), f"init --template {name} wrote no {file}")


def main(names: list[str]) -> int:
    unknown = sorted(set(names) - set(CHECKS) - {"self-contained", "templates"})
    if unknown:
        print(f"check_examples: no check {', '.join(unknown)}; the checks: {', '.join(CHECKS)}")
        return 2
    offline()
    failures = 0
    offered = templates()
    if not names or "templates" in names:
        problems = template_problems(offered)
        if problems:
            failures += 1
            print("FAIL templates: " + "; ".join(problems))
        else:
            print(f"ok   templates: {', '.join(offered.values())} each have a check and ship")
    url, server = serve(release_archive(sorted(offered)))
    typed: set[str] = set()  # the TypeScript examples whose types are checked: once each
    if not names or "self-contained" in names:
        leaving = self_contained()
        if leaving:
            failures += 1
            print("FAIL self-contained: links that leave their example; make them absolute URLs:")
            for link in leaving:
                print(f"     {link}")
        else:
            print("ok   self-contained: every example links outside itself by absolute URL")
    for name, (example, check) in CHECKS.items():
        if names and name not in names:
            continue
        with tempfile.TemporaryDirectory(prefix="fx-example-") as scratch:
            home = Path(scratch) / example
            try:
                if example in offered:
                    started(offered[example], home, url)
                else:
                    shutil.copytree(
                        EXAMPLES / example,
                        home,
                        ignore=shutil.ignore_patterns(
                            ".fx", "runs", ".venv", ".showcase-inputs", "node_modules"
                        ),
                    )
                if (home / "package.json").is_file():
                    typescript(home, check_types=example not in typed)
                    typed.add(example)
                print(f"ok   {name}: {check(home)}")
            except Failed as failure:
                failures += 1
                print(f"FAIL {name}: {failure}")
            except Exception:  # an SDK or engine error is a failed check, not a crash
                failures += 1
                print(f"FAIL {name}:\n{traceback.format_exc()}")
    server.shutdown()
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
