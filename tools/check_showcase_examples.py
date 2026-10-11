"""Check the website's source examples on fresh copies, without CDN or provider access.

Install examples/requirements-ci.txt into the checking interpreter first.
Uses the current checkout's embedded engine and public Python SDK. Blender is
optional for this gate: the complete 3D graph always plans, and its source-artwork
workflow always exercises paid-call refusal. Actual 3D work requires Blender.
"""

from __future__ import annotations

import ast
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import grida.fx as fx

ROOT = Path(__file__).resolve().parents[1]
EXAMPLES = ROOT / "examples"
KEYS = ("OPENAI_API_KEY", "OPENROUTER_API_KEY", "FAL_KEY", "TRIPO_API_KEY", "ELEVENLABS_API_KEY")


def run_script(home: Path, script: str, *args: str) -> None:
    result = subprocess.run(
        [sys.executable, script, *args], cwd=home, text=True, capture_output=True
    )
    if result.returncode:
        raise RuntimeError(f"{script}: {result.stderr[-4000:]}")


def plan_and_refuse(home: Path, target: str, inputs: str | None = None) -> None:
    options = {"input_files": [inputs]} if inputs is not None else {}
    plan = fx.plan(target, cwd=home, **options)
    assert plan.ok, f"{target}: {plan.problems}"
    result = fx.run(target, cwd=home, **options)
    assert not result.ok and result.cost == 0, f"{target}: expected an offline refusal"
    assert any("is a paid call; run with --live" in json.dumps(event) for event in result.events), (
        f"{target}: no explicit paid-call refusal"
    )


def audit_source(home: Path) -> None:
    """Example bodies must not import the application checkout or private FX internals."""
    for path in home.rglob("*.py"):
        for item in ast.walk(ast.parse(path.read_text(encoding="utf-8"))):
            names = []
            if isinstance(item, ast.Import):
                names = [alias.name for alias in item.names]
            elif isinstance(item, ast.ImportFrom) and item.module is not None:
                names = [item.module]
            for name in names:
                assert not name.startswith(
                    ("stage_gen", "demo_game_tools", "demo_game_collection")
                ), f"{path.relative_to(home)} imports another application: {name}"
                assert not name.startswith("grida.fx."), (
                    f"{path.relative_to(home)} imports a private FX module: {name}"
                )


def check(home: Path, name: str) -> str:
    audit_source(home)
    if name in ("world", "terrain", "ui-kit"):
        run_script(home, "make_fixture.py")
        if name == "world":
            plan_and_refuse(home, "workflow.yaml", ".fixture/inputs.json")
        else:
            plan_and_refuse(home, "workflow.py:build_fixture")
        if name == "terrain":
            run_script(home, "check_cover.py")
            run_script(home, "check_cover_motion.py")
            return "graph planned; paid call refused; still and animated covers rendered at $0"
        return "current graph planned; paid call refused at $0"
    if name == "movie-sprite":
        run_script(home, "fixtures/make_inputs.py", ".fixtures")
        plan_and_refuse(home, "workflow.yaml", ".fixtures/take.yaml")
        result = fx.run("workflow.yaml", cwd=home, input_files=[".fixtures/clip.yaml"])
        assert result.ok and result.cost == 0, f"free finishing failed: {result.failed}"
        assert {"canonical", "loop", "preview"} <= set(result.outputs), result.outputs.keys()
        return "paid call refused; supplied synthetic footage finished at $0"
    if name == "portrait-motion":
        run_script(home, "inputs/make_inputs.py", ".fixtures")
        (home / ".fixtures/source.yaml").write_text("reference: sprite.png\n")
        plan_and_refuse(home, "source-workflow.yaml", ".fixtures/source.yaml")
        plan_and_refuse(home, "workflow.yaml", ".fixtures/face.yaml")
        run_script(home, "check.py")
        return "source and face calls refused; face and whole-portrait stand-ins finished at $0"
    if name == "character-3d":
        run_script(home, "make_fixture.py")
        plan = fx.plan("workflow.yaml", cwd=home, input_files=["inputs/fixture/inputs.yaml"])
        assert plan.ok, plan.problems
        plan_and_refuse(home / "source-artwork", "workflow.yaml")
        blender = os.environ.get("GRIDA_FX_TOOL_BLENDER") or shutil.which("blender")
        if blender:
            plan_and_refuse(home, "workflow.yaml", "inputs/fixture/inputs.yaml")
            return "full graph planned; 3D and source artwork paid calls refused at $0"
        return "full graph planned; source artwork refused at $0 (Blender execution not checked)"
    raise ValueError(f"Unknown example: {name}")


def main() -> int:
    for key in KEYS:
        os.environ.pop(key, None)
    os.environ.update(
        GRIDA_FX_NETWORK="off", GRIDA_FX_DISABLE_DOTENV="1", GRIDA_FX_PYTHON=sys.executable
    )
    names = sys.argv[1:] or [
        "world",
        "terrain",
        "ui-kit",
        "movie-sprite",
        "portrait-motion",
        "character-3d",
    ]
    failures = []
    for name in names:
        if name not in (
            "world",
            "terrain",
            "ui-kit",
            "movie-sprite",
            "portrait-motion",
            "character-3d",
        ):
            raise SystemExit(f"Unknown showcase example: {name}")
        with tempfile.TemporaryDirectory(prefix="fx-showcase-example-") as temporary:
            home = Path(temporary) / name
            shutil.copytree(
                EXAMPLES / name,
                home,
                ignore=shutil.ignore_patterns(
                    ".venv",
                    ".fx",
                    "runs",
                    "out",
                    ".showcase-inputs",
                    ".fixture",
                    ".fixtures",
                    "fixture",
                    "__pycache__",
                    "uv.lock",
                ),
            )
            try:
                print(f"ok   {name}: {check(home, name)}", flush=True)
            except Exception as error:
                failures.append(name)
                print(f"FAIL {name}: {error}", file=sys.stderr, flush=True)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
