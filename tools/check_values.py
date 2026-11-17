"""Verify recorded values through the real provider-free notes-and-values fixture.

    python/.venv/bin/python tools/check_values.py --output .fx/values-demo

Retains a source project, plan, runs and both export policies for browser/manual review.
No provider calls, downloads or live mode. Input artwork is synthesized by committed code.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from check_notes import ROOT
from check_notes import prepare as prepare_notes


def prepare(output: Path, command: Path) -> dict:
    report = prepare_notes(output, command)
    project = Path(report["project"])
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
    command = command.resolve()
    run = output.resolve() / "runs/values"
    input_flags = ("--", "--reference", "inputs/reference.png")

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

    invoke("run", "viewer-values", "--run", str(run), "--no-view", "--max-usd", "0", *input_flags)
    report["run"] = str(run)
    for name, flags in (("disclosed", ["--include-values"]), ("withheld", [])):
        destination = output.resolve() / "exports" / ("values-" + name)
        invoke("export", str(run), "--out", str(destination), *flags)
        report["exports"][name] = str(destination)
    result = subprocess.run(
        [
            str(command),
            "plan",
            "viewer-values",
            "--check",
            "--json",
            "--",
            "--reference",
            "inputs/reference.png",
        ],
        cwd=project,
        env=environment,
        capture_output=True,
        text=True,
        timeout=90,
    )
    if result.returncode:
        raise ValueError(result.stderr or result.stdout)
    planned = json.loads(result.stdout)
    plan_path = output.resolve() / "plan.json"
    plan_path.write_text(result.stdout, encoding="utf-8")
    values = planned["value_views"]
    prompt = next(v for v in values if v["kind"] == "input" and v["name"] == "prompt")
    assert prompt["state"] == "known" and "geometric lantern" in prompt["data"]["value"]
    reference = next(v for v in values if v["kind"] == "input" and v["name"] == "reference")
    assert reference["data"]["file"]["kind"] == "image/png"
    assert reference["data"]["file"]["name"] == "reference.png"
    assert any(v["kind"] == "table" and v["name"] == "style" for v in values)
    variants = [v for v in values if v["kind"] == "let" and v["name"] == "variation_tone"]
    assert {v["data"]["value"] for v in variants} == {
        "warm",
        "cool",
    }, "one lazy declaration retains distinct contextual values"
    seed = next(instance for instance in planned["instances"] if instance["path"] == "seed")
    assert seed["with"]["size"] == 256 and seed["with"]["background"] == "warm paper"
    assert any(v["kind"] == "output" and v["state"] == "pending" for v in values)
    assert planned["value_links"], "engine records shared-source contributions"
    assert str(project) not in json.dumps(values), "value evidence has no private source paths"

    disclosed = json.loads((Path(report["exports"]["disclosed"]) / "manifest.json").read_text())[
        "view"
    ]
    withheld = json.loads((Path(report["exports"]["withheld"]) / "manifest.json").read_text())[
        "view"
    ]
    assert any(v["kind"] == "output" and v["state"] == "known" for v in disclosed["value_views"])
    assert any(v["state"] == "withheld" and "data" not in v for v in withheld["value_views"])
    assert not any(v.get("data", {}).get("value") for v in withheld["value_views"]), (
        "scalar contents obey export disclosure"
    )
    reference_digest = reference["data"]["file"]["digest"]
    assert any(
        artifact["digest"] == reference_digest and artifact["available"]
        for artifact in disclosed["artifacts"]
    ), "a transformed reference input still has its own recorded preview bytes"
    report.update(
        plan=str(plan_path),
        notes=len(planned.get("notes", [])),
        values=len(values),
        value_links=len(planned["value_links"]),
    )
    (output.resolve() / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--command", type=Path, default=ROOT / "target/debug/grida-fx")
    args = parser.parse_args()
    output = args.output or Path(tempfile.mkdtemp(prefix="fx-values-"))
    print(json.dumps(prepare(output, args.command.resolve()), indent=2))


if __name__ == "__main__":
    main()
