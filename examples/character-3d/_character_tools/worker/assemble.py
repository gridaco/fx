"""Combine explicitly transformed static parts; no anatomy fitting heuristics."""

from __future__ import annotations

import argparse
import json
import sys
from importlib import import_module
from pathlib import Path
from typing import TYPE_CHECKING, Any

import numpy as np

if TYPE_CHECKING:
    from _character_tools.worker import blender_io, contract
else:
    from _character_tools.worker import blender_io as blender_io
    from _character_tools.worker import contract as contract
bpy = import_module("bpy")
Matrix = import_module("mathutils").Matrix


def assemble(request: dict[str, Any], input_root: Path, output_root: Path) -> dict[str, Any]:
    if not isinstance(request, dict) or set(request) != {
        "schema_version",
        "operation",
        "parts",
        "output_dir",
    }:
        raise ValueError("Assembly requires schema_version, operation, parts and output_dir")
    if (
        type(request["schema_version"]) is not int
        or request["schema_version"] != 1
        or request["operation"] != "assemble"
    ):
        raise ValueError("Unsupported assembly request")
    parts = request["parts"]
    if not isinstance(parts, list) or not 1 <= len(parts) <= 16:
        raise ValueError("Assembly accepts 1 through 16 parts")
    ids, validated = (set(), [])
    for part in parts:
        if not isinstance(part, dict) or set(part) != {
            "part_id",
            "role",
            "source",
            "transform",
            "material_mode",
        }:
            raise ValueError("Invalid assembly part fields")
        name = contract.identifier(part["part_id"])
        contract.identifier(part["role"])
        if name in ids:
            raise ValueError("Duplicate part identity")
        ids.add(name)
        source = part["source"]
        if not isinstance(source, dict) or set(source) != {"path", "sha256"}:
            raise ValueError("Each part requires a source path and SHA-256")
        path = contract.confined(input_root, source["path"], exists=True)
        if path.suffix.lower() != ".glb" or contract.digest(path) != source["sha256"]:
            raise ValueError("Assembly requires hash-verified embedded normalized GLB parts")
        matrix = part["transform"]
        if not isinstance(matrix, list) or len(matrix) != 4:
            raise ValueError("Transform must be a row-major 4x4 matrix")
        transform = np.array([contract.vector(row, 4) for row in matrix])
        linear = transform[:3, :3]
        scale2 = np.trace(linear.T @ linear) / 3
        if (
            not np.allclose(transform[3], [0, 0, 0, 1])
            or scale2 < 1e-12
            or scale2 > 1000000000000.0
            or (np.linalg.det(linear) <= 0)
            or (not np.allclose(linear.T @ linear, np.eye(3) * scale2, rtol=1e-06, atol=1e-10))
        ):
            raise ValueError(
                "Only proper rotation, translation and positive uniform scale are supported"
            )
        if part["material_mode"] != "preserve":
            raise ValueError(
                "Assembly preserves materials; use the separate surface stage to change them"
            )
        validated.append(
            (part, path, blender_io.TO_BLENDER @ Matrix(transform.tolist()) @ blender_io.TO_PUBLIC)
        )
    output = contract.confined(output_root, request["output_dir"])
    if output.exists():
        raise ValueError("Assembly output exists; choose a fresh revision")
    output.mkdir(parents=True)
    bpy.ops.wm.read_factory_settings(use_empty=True)
    assembled, records, role_map = ([], [], {})
    for part, path, transform in validated:
        objects, _ = blender_io.import_source(path, reset=False)
        blender_io.assert_static(objects)
        meshes = sorted((obj for obj in objects if obj.type == "MESH"), key=lambda obj: obj.name)
        world = {obj: obj.matrix_world.copy() for obj in meshes}
        names = []
        for index, obj in enumerate(meshes):
            obj.data = obj.data.copy()
            obj.data.transform(transform @ world[obj])
            obj.parent = None
            obj.matrix_world = Matrix.Identity(4)
            obj.name = f"{part['part_id']}__mesh_{index:02d}"
            obj.data.name = obj.name + "_geometry"
            obj["part_id"] = part["part_id"]
            obj["anatomy_role"] = part["role"]
            role_map[obj.name] = part["role"]
            names.append(obj.name)
        for obj in objects:
            if obj.type != "MESH":
                bpy.data.objects.remove(obj, do_unlink=True)
        bpy.context.view_layer.update()
        records.append(
            {
                **part,
                "mesh_names": names,
                "inventory": blender_io.inventory(meshes, blender_io.scene_arrays(meshes)),
            }
        )
        assembled.extend(meshes)
    exported = blender_io.normalize(assembled, {"require_textures": True}, output)
    for part, path, _ in validated:
        if contract.digest(path) != part["source"]["sha256"]:
            raise ValueError("Assembly source changed during operation")
    report = {
        "schema_version": 1,
        "operation": "assemble",
        "status": "candidate_unreviewed",
        "coordinates": contract.COORDINATES,
        "parts": records,
        "part_roles": role_map,
        "export": exported,
        "seams": (
            "Rigidly fitted parts remain separate surfaces; no seam welding or repair is implied."
        ),
        "artifacts": [
            {
                "path": name,
                "sha256": contract.digest(output / name),
                "bytes": (output / name).stat().st_size,
            }
            for name in ("model.glb", "native.blend")
        ],
    }
    contract.write_json(output / "report.json", report)
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--input-root", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    args = parser.parse_args(sys.argv[sys.argv.index("--") + 1 :])
    report = assemble(json.loads(args.request.read_text()), args.input_root, args.output_root)
    print(
        json.dumps(
            {
                "operation": report["operation"],
                "status": report["status"],
                "artifacts": report["artifacts"],
            }
        )
    )


if __name__ == "__main__":
    main()
