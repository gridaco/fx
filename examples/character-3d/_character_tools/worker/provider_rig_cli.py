"""Hash-bound, confined JSON boundary for existing-provider rig processing."""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import tempfile
from pathlib import Path
from typing import Any

if __package__ in {None, ""}:
    from _character_tools.worker import contract, provider_rig
else:
    from _character_tools.worker import contract, provider_rig


def validate_request(request: dict[str, Any]) -> dict[str, Any]:
    provider_rig._fields(
        request, {"schema_version", "operation", "source", "output_dir", "options"}
    )
    if (
        type(request["schema_version"]) is not int
        or request["schema_version"] != 1
        or request["operation"] != "provider_rig"
    ):
        raise ValueError("Unsupported provider-rig worker request")
    provider_rig._reference(request["source"])
    contract.relative(request["output_dir"])
    provider_rig.validate_options(request["options"])
    return request


def run(request: dict[str, Any], input_root: Path, output_root: Path) -> dict[str, Any]:
    validate_request(request)
    references = {"source": request["source"]}
    for key in ("preservation", "motion"):
        if key in request["options"]:
            references[key] = request["options"][key]["source"]
    inputs = {}
    for key, reference in references.items():
        path = contract.confined(input_root, reference["path"], exists=True)
        if contract.digest(path) != reference["sha256"]:
            raise ValueError("Declared input hash differs from the frozen request")
        inputs[key] = path
    output = contract.confined(output_root, request["output_dir"])
    if output.exists():
        raise ValueError("Operation output directory already exists")
    output.parent.mkdir(parents=True, exist_ok=True)
    output = contract.confined(output_root, request["output_dir"])
    staging = Path(tempfile.mkdtemp(prefix=".provider-rig-staging-", dir=output.parent))
    try:
        report = provider_rig.build_provider_rig(
            inputs["source"],
            request["options"],
            staging,
            preservation_source=inputs.get("preservation"),
            motion_source=inputs.get("motion"),
        )
        contract.write_json(staging / "request.json", request)
        if any(path.is_symlink() for path in staging.rglob("*")):
            raise ValueError("Unexpected output symlink")
        artifacts = [
            {
                "path": path.relative_to(staging).as_posix(),
                "sha256": contract.digest(path),
                "size_bytes": path.stat().st_size,
            }
            for path in sorted(staging.rglob("*"))
            if path.is_file()
        ]
        contract.write_json(
            staging / "manifest.json",
            {
                "schema_version": 1,
                "operation": "provider_rig",
                "inputs": references,
                "artifacts": artifacts,
                "implementation": [
                    {"path": path.name, "sha256": contract.digest(path)}
                    for path in sorted(Path(__file__).resolve().parent.glob("*.py"))
                ],
            },
        )
        if (
            any(
                (
                    contract.digest(inputs[key]) != reference["sha256"]
                    for key, reference in references.items()
                )
            )
            or output.exists()
        ):
            raise ValueError("Provider-rig source or destination changed during processing")
        staging.rename(output)
    except BaseException:
        shutil.rmtree(staging)
        raise
    return {
        "status": report["status"],
        "operation": "provider_rig",
        "source_unchanged": True,
        "output_transform": report.get("output_transform"),
        "report": (Path(request["output_dir"]) / "report.json").as_posix(),
        "manifest": (Path(request["output_dir"]) / "manifest.json").as_posix(),
        "output": {
            "path": (Path(request["output_dir"]) / report["output"]["path"]).as_posix(),
            "sha256": report["output"]["sha256"],
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--input-root", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    args = parser.parse_args(sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else None)
    if args.request.is_symlink() or not args.request.is_file():
        raise ValueError("Request must be a regular local JSON file")
    result = run(
        json.loads(args.request.read_text()),
        args.input_root.resolve(strict=True),
        args.output_root.resolve(strict=True),
    )
    print("WORKER_RESULT " + json.dumps(result), flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(
            "WORKER_ERROR "
            + json.dumps(
                {
                    "error_type": type(error).__name__,
                    "message": str(error)
                    if isinstance(error, ValueError)
                    else "Provider-rig processing failed; inspect the local worker log",
                }
            ),
            flush=True,
        )
        raise SystemExit(1) from None
