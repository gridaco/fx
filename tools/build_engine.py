"""Builds the engine from this checkout and puts it where the Python SDK finds it.

    python3 tools/build_engine.py [--debug]

It builds the viewer with Bun, runs ``cargo build --locked --release -p grida-fx``
(``--debug``: the dev profile) and copies
the binary to ``python/src/grida/fx/_bin/grida-fx``, the place a ``grida`` wheel carries it
(gitignored here). A ``grida`` installed from this checkout then finds the engine with no
setting: ``uv sync --project python`` here, or a path dependency on ``<checkout>/python`` in
another project (installed editable, so it reads this folder). ``python -m grida.fx`` is the
``grida-fx`` command.

Run it again after every pull or checkout: cargo rebuilds only what changed, and the copy is
replaced only when it differs, so the engine always matches the SDK beside it. ``CARGO_TARGET_DIR``
is honoured. Source builds need cargo (https://rustup.rs), Bun (https://bun.sh) and Python 3.9
or later. Install the root workspace dependencies once with
``bun --no-env-file install --frozen-lockfile``. Installed npm packages and Python wheels carry
the viewer inside the engine and need no Bun or frontend build. Any failure exits 1 with
``build_engine: <reason>``.
"""

from __future__ import annotations

import argparse
import filecmp
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PLACE = ROOT / "python" / "src" / "grida" / "fx" / "_bin" / "grida-fx"


class Refused(Exception):
    pass


def target_directory() -> Path:
    """Cargo's target folder for this workspace, wherever ``CARGO_TARGET_DIR`` or config puts it."""
    done = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if done.returncode != 0:
        raise Refused(f"cargo metadata failed: {done.stderr.strip()}")
    return Path(json.loads(done.stdout)["target_directory"])


def build(debug: bool) -> Path:
    if shutil.which("cargo") is None:
        raise Refused("cargo is not on PATH (install Rust from https://rustup.rs)")
    if shutil.which("bun") is None:
        raise Refused("bun is not on PATH (source builds need Bun from https://bun.sh)")
    done = subprocess.run(["bun", "--no-env-file", "run", "build:viewer"], cwd=ROOT)
    if done.returncode != 0:
        raise Refused(
            "viewer build failed; install its dependencies with "
            "`bun --no-env-file install --frozen-lockfile` from the FX checkout"
        )
    profile = [] if debug else ["--release"]
    done = subprocess.run(["cargo", "build", "--locked", *profile, "-p", "grida-fx"], cwd=ROOT)
    if done.returncode != 0:
        raise Refused(f"cargo build failed with status {done.returncode}")
    built = target_directory() / ("debug" if debug else "release") / "grida-fx"
    if not built.is_file():
        raise Refused(f"cargo built no {built}")
    return built


def place(built: Path) -> bool:
    """Copies ``built`` to :data:`PLACE` when it differs; returns whether it did."""
    if PLACE.is_file() and filecmp.cmp(built, PLACE, shallow=False):
        return False
    PLACE.parent.mkdir(parents=True, exist_ok=True)
    handle, temporary = tempfile.mkstemp(prefix=".grida-fx.", dir=PLACE.parent)
    try:
        with os.fdopen(handle, "wb") as copy, built.open("rb") as source:
            shutil.copyfileobj(source, copy)
        os.chmod(temporary, 0o755)
        os.replace(temporary, PLACE)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--debug", action="store_true", help="build the dev profile")
    arguments = parser.parse_args()
    try:
        replaced = place(build(arguments.debug))
    except (Refused, OSError) as error:
        print(f"build_engine: {error}", file=sys.stderr)
        return 1
    state = "updated" if replaced else "unchanged"
    print(f"build_engine: {PLACE.relative_to(ROOT)} ({state})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
