"""Confined JSON request boundary for the local Blender rig worker."""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import tempfile
from pathlib import Path
from typing import Any

from _character_tools.worker import contract, rig_contract


def validate_request(request: dict[str, Any]) -> None:
    rig_contract.fields(request, {"schema_version", "operation", "source", "plan", "output_dir"})
    if (
        type(request["schema_version"]) is not int
        or request["schema_version"] != 1
        or request["operation"] != "rig"
    ):
        raise ValueError("Unsupported rig worker request")
    rig_contract.fields(request["source"], {"path", "sha256"})
    contract.relative(request["source"]["path"])
    contract.relative(request["output_dir"])
    rig_contract.validate_plan(request["plan"])
    if request["source"]["sha256"] != request["plan"]["source_sha256"]:
        raise ValueError("Source request and plan hashes disagree")


def run(request: dict[str, Any], input_root: Path, output_root: Path) -> dict[str, Any]:
    validate_request(request)
    source = contract.confined(input_root, request["source"]["path"], exists=True)
    if contract.digest(source) != request["source"]["sha256"]:
        raise ValueError("Source hash differs from the frozen request")
    output = contract.confined(output_root, request["output_dir"])
    if output.exists():
        raise ValueError("Operation output directory already exists")
    output.parent.mkdir(parents=True, exist_ok=True)
    output = contract.confined(output_root, request["output_dir"])
    staging = Path(tempfile.mkdtemp(prefix=".rig-staging-", dir=output.parent))
    try:
        from _character_tools.worker.rigging import build_rig

        report = build_rig(source, request["plan"], staging)
        contract.write_json(staging / "request.json", request)
        artifacts = [
            {
                "path": path.relative_to(staging).as_posix(),
                "sha256": contract.digest(path),
                "size_bytes": path.stat().st_size,
            }
            for path in sorted(staging.rglob("*"))
            if path.is_file()
        ]
        if any(path.is_symlink() for path in staging.rglob("*")):
            raise ValueError("Unexpected output symlink")
        contract.write_json(
            staging / "manifest.json",
            {
                "schema_version": 1,
                "operation": "rig",
                "source": request["source"],
                "artifacts": artifacts,
                "implementation": [
                    {"path": path.name, "sha256": contract.digest(path)}
                    for path in sorted(Path(__file__).resolve().parent.glob("*.py"))
                ],
            },
        )
        if contract.digest(source) != request["source"]["sha256"] or output.exists():
            raise ValueError("Source or destination changed during operation")
        staging.rename(output)
    except BaseException:
        shutil.rmtree(staging)
        raise
    return {
        "status": report["status"],
        "operation": "rig",
        "report": (Path(request["output_dir"]) / "report.json").as_posix(),
        "manifest": (Path(request["output_dir"]) / "manifest.json").as_posix(),
        "source_unchanged": True,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--input-root", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    args = parser.parse_args(sys.argv[sys.argv.index("--") + 1 :])
    if args.request.is_symlink() or not args.request.is_file():
        raise ValueError("Request must be a regular local JSON file")
    request = json.loads(args.request.read_text())
    result = run(
        request, args.input_root.resolve(strict=True), args.output_root.resolve(strict=True)
    )
    print("WORKER_RESULT " + json.dumps(result), flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        message = (
            str(error)
            if isinstance(error, ValueError)
            else "Rig operation failed; inspect the local worker log"
        )
        print(
            "WORKER_ERROR " + json.dumps({"error_type": type(error).__name__, "message": message}),
            flush=True,
        )
        raise SystemExit(1) from None
