"""Prepare and verify the real provider-free workflow-notes fixture.

    python/.venv/bin/python tools/check_notes.py --output .fx/notes-demo

The output must be new or empty. Source files, saved runs and disclosed/withheld
exports are retained for browser acceptance and manual inspection. No live mode,
provider keys, downloads or generated artwork are involved.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def prepare(output: Path, command: Path) -> dict:
    output = output.resolve()
    if output.exists() and any(output.iterdir()):
        raise ValueError("Choose a new or empty output directory")
    output.mkdir(parents=True, exist_ok=True)
    project = output / "project"
    shutil.copytree(
        ROOT / "fixtures/viewer",
        project,
        ignore=shutil.ignore_patterns("__pycache__", "*.pyc", ".env*", ".fx", "runs"),
    )
    environment = dict(os.environ)
    for name in (
        "GG_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
    ):
        environment.pop(name, None)
    environment.update(
        GRIDA_FX_NETWORK="off", GRIDA_FX_DISABLE_DOTENV="1", GRIDA_FX_PYTHON=sys.executable
    )

    def invoke(*arguments: str) -> str:
        result = subprocess.run(
            [str(command), *arguments],
            cwd=project,
            env=environment,
            capture_output=True,
            text=True,
            timeout=90,
        )
        if result.returncode:
            raise ValueError(result.stderr or result.stdout)
        return result.stdout

    run = output / "runs/notes"
    invoke("run", "viewer-notes", "--run", str(run), "--no-view", "--max-usd", "0")
    original_bytes = (run / "plan.json").read_bytes()
    original = json.loads(original_bytes)
    notes = original.get("notes", [])
    assert len(notes) >= 7, "root, repeated group and imported notes are recorded"
    assert any(n["format"] == "text" and "${{ inputs.prompt }}" in n["text"] for n in notes)
    assert sum(n["text"] is None for n in notes) == 1, "one missing-file note is unavailable"
    assert any(n.get("diagnostic") and n["text"] for n in notes), "bad association retains text"
    assert not any(str(project) in json.dumps(n) for n in notes), "portable note records"
    note_source = project / "notes/overview.md"
    note_source.write_text(
        "# Updated documentation\nThis is a newer source file.\n", encoding="utf-8"
    )
    invoke("run", "viewer-notes", "--run", str(run), "--no-view", "--max-usd", "0")
    assert (run / "plan.json").read_bytes() == original_bytes, "resume preserves the exact snapshot"
    later = output / "runs/fresh"
    invoke("run", "viewer-notes", "--run", str(later), "--no-view", "--max-usd", "0")
    refreshed = json.loads((later / "plan.json").read_text())
    assert original["plan"] == refreshed["plan"], "note-only file edit preserves plan identity"
    assert any(n["text"] and "Updated documentation" in n["text"] for n in refreshed["notes"])
    events = [json.loads(line) for line in (run / "events.jsonl").read_text().splitlines()]
    assert all(e.get("charged_usd", 0) == 0 for e in events)
    assert not any(e["event"] == "call" for e in events)
    exports = {}
    (output / "exports").mkdir()
    for name, flags in (("disclosed", ["--include-values"]), ("withheld", [])):
        destination = output / "exports" / name
        invoke("export", str(run), "--out", str(destination), *flags, "--json")
        manifest = json.loads((destination / "manifest.json").read_text())
        exported = manifest["view"].get("notes", [])
        assert len(exported) == len(notes)
        assert not any("source" in n for n in exported), "export strips local provenance"
        if flags:
            assert any(n["text"] and "Reference to variations" in n["text"] for n in exported)
        else:
            assert all(n.get("withheld") is True and n["text"] is None for n in exported)
        exports[name] = str(destination)
    report = {
        "project": str(project),
        "run": str(run),
        "fresh_run": str(later),
        "exports": exports,
        "notes": len(notes),
        "charged_usd": 0,
        "provider_calls": 0,
    }
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--command", type=Path, default=ROOT / "target/debug/grida-fx")
    args = parser.parse_args()
    output = args.output or Path(tempfile.mkdtemp(prefix="fx-notes-"))
    print(json.dumps(prepare(output, args.command.resolve()), indent=2))


if __name__ == "__main__":
    main()
