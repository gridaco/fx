"""Run one bounded local mesh operation inside Blender, writing a fresh report."""

from __future__ import annotations

import argparse
import json
import sys
from importlib import import_module
from pathlib import Path
from typing import TYPE_CHECKING, Any

import numpy as np

if TYPE_CHECKING:
    from _character_tools.worker import (
        blender_io,
        contract,
        geometry,
        rendering,
    )
else:
    from _character_tools.worker import blender_io as blender_io
    from _character_tools.worker import contract as contract
    from _character_tools.worker import geometry as geometry
    from _character_tools.worker import rendering as rendering
bpy = import_module("bpy")


def inspect(
    objects: list[Any], arrays: dict[str, dict[str, Any]], options: dict[str, Any], output: Path
) -> dict[str, Any]:
    report = blender_io.inventory(objects, arrays)
    full_components, slice_results = ({}, [])
    for index, entry in enumerate(report["meshes"]):
        name = entry["name"]
        data = arrays[name]
        points, triangles = (data["positions"], data["triangles"])
        tolerance = options.get(
            "weld_tolerance", max(float(np.linalg.norm(np.ptp(points, axis=0))) * 1e-06, 1e-08)
        )
        welded, membership = geometry.topology(points, triangles, tolerance)
        full_components[name] = welded
        entry["indexed_topology"] = geometry.indexed_topology(points, triangles)
        entry["analysis_weld_topology"] = {
            **welded,
            "components": welded["components"][: options.get("component_limit", 20)],
        }
        areas = (
            np.linalg.norm(
                np.cross(
                    points[triangles[:, 1]] - points[triangles[:, 0]],
                    points[triangles[:, 2]] - points[triangles[:, 0]],
                ),
                axis=1,
            )
            / 2
        )
        entry["surface_area"] = float(areas.sum())
        entry["degenerate_triangles"] = int((areas <= 1e-16).sum())
        if options.get("save_geometry", True):
            filename = f"mesh-{index:03d}.npz"
            np.savez_compressed(
                output / filename,
                positions=points,
                triangles=triangles,
                triangle_polygon_indices=data["triangle_polygon_indices"],
                triangle_material_indices=data["triangle_material_indices"],
                triangle_loops=data["triangle_loops"],
                component_membership=membership,
                **{f"uv_layer_{i}": values for i, values in enumerate(data["uv_layers"].values())},
            )
            entry["geometry_archive"] = filename
    for query in options.get("slice_planes", []):
        names = [query["mesh"]] if "mesh" in query else list(arrays)
        if set(names) - arrays.keys():
            raise ValueError("Slice query names an unavailable mesh")
        results = {
            name: geometry.slice_mesh(
                arrays[name]["positions"],
                arrays[name]["triangles"],
                query["point"],
                query["normal"],
            )
            for name in names
        }
        filename = "slice-" + query["name"] + ".json"
        contract.write_json(output / filename, results)
        slice_results.append(
            {
                "name": query["name"],
                "artifact": filename,
                "segment_count": sum(result["segment_count"] for result in results.values()),
            }
        )
    contract.write_json(output / "components.json", full_components)
    report.update(
        components_artifact="components.json",
        slices=slice_results,
        nearest_points=blender_io.nearest_queries(arrays, options.get("nearest_points", [])),
        topology_limitations=(
            "Connectivity and proximity clusters are measured evidence, not"
            " anatomical segmentation or deformation readiness. Proximity w"
            "elding is transitive and analysis-only; source geometry is unc"
            "hanged."
        ),
    )
    return report


def run(request: dict[str, Any], input_root: Path, output_root: Path) -> dict[str, Any]:
    contract.validate_request(request)
    source = contract.confined(input_root, request["source"]["path"], exists=True)
    if contract.digest(source) != request["source"]["sha256"]:
        raise ValueError("Source hash differs from the frozen request")
    output = contract.confined(output_root, request["output_dir"])
    if output.exists():
        raise ValueError("Operation output directory exists; choose a fresh revision")
    output.mkdir(parents=True, exist_ok=False)
    options = request["options"]
    pose = options.get("pose")
    objects, raw = blender_io.import_source(source, pose["fps"] if pose else 30)
    if request["operation"] == "normalize":
        result = blender_io.normalize(objects, options, output)
        pose_report = None
    else:
        pose_report = blender_io.select_pose(objects, pose)
        arrays = blender_io.scene_arrays(objects)
        result = (
            inspect(objects, arrays, options, output)
            if request["operation"] == "inspect"
            else rendering.render(objects, arrays, options, output)
        )
    if contract.digest(source) != request["source"]["sha256"]:
        raise ValueError("Source changed during operation")
    report = {
        "schema_version": 1,
        "operation": request["operation"],
        "status": "completed_unreviewed",
        "request": request,
        "coordinate_system": contract.COORDINATES,
        "blender_version": bpy.app.version_string,
        "raw_glb_inventory": raw,
        "source_unchanged": True,
        "pose": pose_report,
        "result": result,
        "implementation": [
            {"path": path.name, "sha256": contract.digest(path)}
            for path in sorted(Path(__file__).resolve().parent.glob("*.py"))
        ],
    }
    contract.write_json(output / "report.json", report)
    artifacts = []
    for path in sorted(output.rglob("*")):
        if path.is_symlink():
            raise ValueError("Unexpected output symlink")
        if path.is_file():
            artifacts.append(
                {
                    "path": path.relative_to(output).as_posix(),
                    "sha256": contract.digest(path),
                    "size_bytes": path.stat().st_size,
                }
            )
    contract.write_json(
        output / "manifest.json",
        {"schema_version": 1, "source": request["source"], "artifacts": artifacts},
    )
    return {
        "status": report["status"],
        "operation": request["operation"],
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
    if args.request.is_symlink():
        raise ValueError("Request must be a regular local file")
    request = json.loads(args.request.read_text())
    result = run(
        request, args.input_root.resolve(strict=True), args.output_root.resolve(strict=True)
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
                    else "operation failed; inspect local diagnostic log",
                }
            ),
            flush=True,
        )
        raise SystemExit(1) from None
