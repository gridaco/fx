"""Read-only numerical diagnostics of hash-bound, explicitly planned GLB rigs.

This checks the exported animation and linear skinning, never intended plan motion
as a substitute for the output. Numerical findings do not accept anatomy or art.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import re
import shutil
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path
from typing import Any

import numpy as np
from numpy.typing import ArrayLike, NDArray

if __package__ in {None, ""}:
    from _character_tools.worker import contract, rig_contract
    from _character_tools.worker import rig_glb as glb
else:
    from _character_tools.worker import contract, rig_contract
    from _character_tools.worker import rig_glb as glb
CODE_VERSION = "rig_metrics_v2_joint_identity_and_hierarchy"
SAMPLE_FRACTIONS = (0.25, 0.5, 0.75)
WEIGHT_EPSILON = 1e-08
WEIGHT_SUM_TOLERANCE = 1e-05


def plan_digest(plan: dict[str, Any]) -> str:
    return hashlib.sha256(
        json.dumps(plan, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
    ).hexdigest()


def distribution(values: ArrayLike) -> dict[str, Any]:
    values = np.asarray(values, dtype=float)
    if not len(values):
        return {"count": 0, "min": None, "median": None, "p95": None, "p99": None, "max": None}
    if not np.isfinite(values).all():
        raise ValueError("Cannot summarize nonfinite measurements")
    return {
        "count": len(values),
        "min": float(values.min()),
        "median": float(np.median(values)),
        "p95": float(np.percentile(values, 95)),
        "p99": float(np.percentile(values, 99)),
        "max": float(values.max()),
    }


def _integer(value: object, count: int, label: str) -> int:
    if type(value) is not int or not 0 <= value < count:
        raise ValueError(f"Invalid {label} index")
    return value


def slerp(left: NDArray[Any], right: NDArray[Any], fraction: float) -> NDArray[Any]:
    """glTF LINEAR quaternion interpolation, including hemisphere selection."""
    left, right = (np.asarray(left, dtype=float), np.asarray(right, dtype=float))
    lengths = (np.linalg.norm(left), np.linalg.norm(right))
    if any(abs(length - 1) > 0.0001 for length in lengths):
        raise ValueError("Animation quaternion is not unit length")
    left, right = (left / lengths[0], right / lengths[1])
    dot = float(left @ right)
    if dot < 0:
        right, dot = (-right, -dot)
    if dot > 0.9995:
        value = (1 - fraction) * left + fraction * right
        return value / np.linalg.norm(value)
    theta = np.arccos(np.clip(dot, -1, 1))
    result: NDArray[Any] = (
        np.sin((1 - fraction) * theta) * left + np.sin(fraction * theta) * right
    ) / np.sin(theta)
    return result


def decode_animation(
    doc: dict[str, Any], binary: bytes | bytearray, animation: dict[str, Any]
) -> list[dict[str, Any]]:
    """Validate supported channels once; refuse unimplemented interpolation."""
    tracks, seen = ([], set())
    for channel in animation.get("channels", []):
        target = channel.get("target", {})
        node = _integer(target.get("node"), len(doc.get("nodes", [])), "animation node")
        path = target.get("path")
        if path not in {"translation", "rotation", "scale"}:
            raise ValueError(
                "Animation path needs a supported TRS evaluator; morph animation is not evaluated"
            )
        if "matrix" in doc["nodes"][node]:
            raise ValueError("Animated node has a matrix instead of TRS")
        if (node, path) in seen:
            raise ValueError("Animation has duplicate channels for the same node and path")
        seen.add((node, path))
        sampler = animation.get("samplers", [])[
            _integer(
                channel.get("sampler"), len(animation.get("samplers", [])), "animation sampler"
            )
        ]
        mode = sampler.get("interpolation", "LINEAR")
        if mode not in {"LINEAR", "STEP"}:
            raise ValueError("CUBICSPLINE or unknown interpolation is not evaluated")
        times = glb.accessor(doc, binary, sampler["input"])
        values = glb.accessor(doc, binary, sampler["output"])
        if (
            times.shape[1] != 1
            or not len(times)
            or np.any(np.diff(times[:, 0]) <= 0)
            or (times[0, 0] < 0)
            or (len(values) != len(times))
            or (values.shape[1] != (4 if path == "rotation" else 3))
        ):
            raise ValueError("Invalid animation time or output shape")
        if path == "rotation" and np.any(np.abs(np.linalg.norm(values, axis=1) - 1) > 0.0001):
            raise ValueError("Animation contains nonunit quaternions")
        tracks.append(
            {"node": node, "path": path, "mode": mode, "times": times[:, 0], "values": values}
        )
    if not tracks:
        raise ValueError("Animation has no supported channels")
    return tracks


def sample_worlds(
    doc: dict[str, Any],
    tracks: Sequence[dict[str, Any]],
    time_seconds: float,
    reset_rotation: int | None = None,
) -> list[NDArray[Any]]:
    """Sample actual exported TRS; optional counterfactual resets one control."""
    posed = {"nodes": copy.deepcopy(doc["nodes"])}
    for track in tracks:
        times, values = (track["times"], track["values"])
        right = int(np.searchsorted(times, time_seconds, side="right"))
        left = max(0, min(right - 1, len(times) - 1))
        right = min(right, len(times) - 1)
        fraction = (
            float(np.clip((time_seconds - times[left]) / (times[right] - times[left]), 0, 1))
            if right != left
            else 0
        )
        if track["mode"] == "STEP" or right == left:
            value = values[left]
        elif track["path"] == "rotation":
            value = slerp(values[left], values[right], fraction)
        else:
            value = values[left] * (1 - fraction) + values[right] * fraction
        posed["nodes"][track["node"]][track["path"]] = value.tolist()
    if reset_rotation is not None:
        posed["nodes"][reset_rotation]["rotation"] = doc["nodes"][reset_rotation].get(
            "rotation", [0, 0, 0, 1]
        )
    return glb.worlds(posed)


def decode_surface(
    doc: dict[str, Any], binary: bytes | bytearray, node_index: int, primitive_index: int
) -> dict[str, Any]:
    node = doc["nodes"][node_index]
    primitive = doc["meshes"][node["mesh"]]["primitives"][primitive_index]
    if (
        primitive.get("mode", 4) != 4
        or primitive.get("targets")
        or doc["meshes"][node["mesh"]].get("weights")
    ):
        raise ValueError("Only triangle primitives without morph targets are evaluated")
    if primitive.get("extensions"):
        raise ValueError("Primitive extensions need an explicit supported decoder")
    attributes = primitive["attributes"]
    positions = glb.accessor(doc, binary, attributes["POSITION"]).astype(float)
    if positions.shape[1] != 3 or not len(positions):
        raise ValueError("Mesh needs nonempty VEC3 positions")
    indices = (
        glb.accessor(doc, binary, primitive["indices"])
        if "indices" in primitive
        else np.arange(len(positions)).reshape(-1, 1)
    )
    if indices.shape[1] != 1 or indices.dtype.kind not in "iu" or len(indices) % 3:
        raise ValueError("Triangle indices must be integer scalars in triples")
    triangles = indices.reshape(-1, 3).astype(np.int64)
    if triangles.size and (triangles.min() < 0 or triangles.max() >= len(positions)):
        raise ValueError("Triangle index lies outside POSITION accessor")
    result: dict[str, Any] = {
        "node_index": node_index,
        "node_name": node.get("name", ""),
        "primitive_index": primitive_index,
        "positions": positions,
        "triangles": triangles,
        "skin": None,
        "edges": np.unique(
            np.sort(triangles[:, [[0, 1], [1, 2], [2, 0]]].reshape(-1, 2), axis=1), axis=0
        ),
    }
    if "skin" not in node:
        return result
    skin = doc.get("skins", [])[_integer(node["skin"], len(doc.get("skins", [])), "skin")]
    joint_nodes = np.asarray(
        [_integer(j, len(doc["nodes"]), "skin joint") for j in skin["joints"]], dtype=int
    )
    if not len(joint_nodes) or len(np.unique(joint_nodes)) != len(joint_nodes):
        raise ValueError("Skin joints are empty or duplicated")
    inverse = (
        glb.accessor(doc, binary, skin["inverseBindMatrices"]).reshape(-1, 4, 4).transpose(0, 2, 1)
        if "inverseBindMatrices" in skin
        else np.repeat(np.eye(4)[None], len(joint_nodes), axis=0)
    )
    if inverse.shape != (len(joint_nodes), 4, 4) or not np.allclose(inverse[:, 3], [0, 0, 0, 1]):
        raise ValueError("Skin inverse bind matrices have invalid shape or affine rows")
    joint_keys = {key for key in attributes if key.startswith("JOINTS_")}
    weight_keys = {key for key in attributes if key.startswith("WEIGHTS_")}
    if not joint_keys or {key.replace("JOINTS_", "WEIGHTS_") for key in joint_keys} != weight_keys:
        raise ValueError("Joint and weight accessor sets are missing or unmatched")
    if any(not re.fullmatch("JOINTS_[0-9]+", key) for key in joint_keys):
        raise ValueError("Invalid joint attribute set name")
    sets = sorted(int(key.split("_")[1]) for key in joint_keys)
    if sets != list(range(len(sets))):
        raise ValueError("Joint attribute sets must start at zero and be contiguous")
    joint_values, weight_values = ([], [])
    for index in sets:
        joints = glb.accessor(doc, binary, attributes[f"JOINTS_{index}"])
        weights = glb.accessor(doc, binary, attributes[f"WEIGHTS_{index}"]).astype(float)
        if (
            joints.shape != (len(positions), 4)
            or weights.shape != joints.shape
            or joints.dtype.kind not in "iu"
        ):
            raise ValueError("Skin attributes need matching integer joints and VEC4 weights")
        if np.any(joints < 0) or np.any(joints >= len(joint_nodes)):
            raise ValueError("Skin joint index is out of range, including unused influence slots")
        joint_values.append(joints.astype(int))
        weight_values.append(weights)
    joints, weights = (np.hstack(joint_values), np.hstack(weight_values))
    result.update(skin={"joints": joint_nodes, "inverse": inverse}, joints=joints, weights=weights)
    return result


def surface_points(surface: dict[str, Any], world: Sequence[NDArray[Any]]) -> NDArray[Any]:
    if surface["skin"] is None:
        result = glb.points(surface["positions"], world[surface["node_index"]])
    else:
        skin = surface["skin"]
        matrices = np.asarray([world[index] for index in skin["joints"]]) @ skin["inverse"]
        homogeneous = np.column_stack((surface["positions"], np.ones(len(surface["positions"]))))
        result = np.zeros((len(homogeneous), 3))
        for slot in range(surface["joints"].shape[1]):
            posed = np.einsum("vij,vj->vi", matrices[surface["joints"][:, slot]], homogeneous)
            result += posed[:, :3] * surface["weights"][:, slot, None]
    if not np.isfinite(result).all():
        raise ValueError("Evaluated surface contains nonfinite positions")
    return result


def triangle_areas(points: NDArray[Any], triangles: NDArray[Any]) -> NDArray[Any]:
    return (
        np.linalg.norm(
            np.cross(
                points[triangles[:, 1]] - points[triangles[:, 0]],
                points[triangles[:, 2]] - points[triangles[:, 0]],
            ),
            axis=1,
        )
        * 0.5
    )


def edge_measurements(
    rest: NDArray[Any], posed: NDArray[Any], edges: NDArray[Any], epsilon: float
) -> dict[str, Any]:
    """Denominators belong to the exact same index pair in the rest surface."""
    before = np.linalg.norm(rest[edges[:, 1]] - rest[edges[:, 0]], axis=1)
    after = np.linalg.norm(posed[edges[:, 1]] - posed[edges[:, 0]], axis=1)
    keep = before > epsilon
    ratios = after[keep] / before[keep]
    kept_edges = edges[keep]
    highest = np.argsort(ratios)[-5:][::-1]
    return {
        "same_edge_length_ratio": distribution(ratios),
        "excluded_zero_or_short_rest_edges": int((~keep).sum()),
        "ratio_above": {str(limit): int((ratios > limit).sum()) for limit in (1.5, 2, 5, 10)},
        "ratio_below_0_5": int((ratios < 0.5).sum()),
        "largest_ratios": [
            {
                "vertex_indices": kept_edges[i].tolist(),
                "ratio": float(ratios[i]),
                "rest_length": float(before[keep][i]),
                "posed_length": float(after[keep][i]),
            }
            for i in highest
        ],
    }


def _curl_probe(
    doc: dict[str, Any],
    surfaces: list[dict[str, Any]],
    rest_world: list[NDArray[Any]],
    rest_surfaces: list[NDArray[Any]],
    posed_world: list[NDArray[Any]],
    tracks: list[dict[str, Any]],
    clip: dict[str, Any],
    track: dict[str, Any],
    time: float,
    names: dict[str, int],
    scale: float,
) -> dict[str, Any]:
    bone_name = track["bone"]
    bone = clip["_bones"][bone_name]
    index = names[bone_name]
    reference_name = bone["parent"]
    reference_index = names[reference_name] if reference_name is not None else None
    counterfactual = sample_worlds(doc, tracks, time, reset_rotation=index)
    posed_parent = posed_world[reference_index] if reference_index is not None else np.eye(4)
    rest_parent = rest_world[reference_index] if reference_index is not None else np.eye(4)
    palm_local = np.linalg.inv(rest_parent)[:3, :3] @ np.asarray(
        bone["palm_direction"], dtype=float
    )
    palm_local /= np.linalg.norm(palm_local)
    to_local = np.linalg.inv(posed_parent)
    tip = np.asarray([bone["tail"]], dtype=float)
    rest_bone_inverse = np.linalg.inv(rest_world[index])
    posed_tip = glb.points(tip, posed_world[index] @ rest_bone_inverse)
    control_tip = glb.points(tip, counterfactual[index] @ rest_bone_inverse)
    tip_delta = glb.points(posed_tip, to_local)[0] - glb.points(control_tip, to_local)[0]
    candidates = []
    for surface_index, surface in enumerate(surfaces):
        if surface["skin"] is None:
            continue
        nodes = surface["skin"]["joints"][surface["joints"]]
        influence = np.sum(surface["weights"] * (nodes == index), axis=1)
        for vertex_index in np.flatnonzero(influence > 0.05):
            candidates.append(
                (
                    float(np.linalg.norm(rest_surfaces[surface_index][vertex_index] - tip[0])),
                    surface_index,
                    int(vertex_index),
                )
            )
    selected = sorted(candidates)[:8]
    deltas, records = ([], [])
    for surface_index in sorted({item[1] for item in selected}):
        surface = surfaces[surface_index]
        actual = surface_points(surface, posed_world)
        control = surface_points(surface, counterfactual)
        for distance, sid, vertex in selected:
            if sid == surface_index:
                delta = to_local[:3, :3] @ (actual[vertex] - control[vertex])
                deltas.append(delta)
                records.append(
                    {
                        "node_index": surface["node_index"],
                        "primitive_index": surface["primitive_index"],
                        "vertex_index": vertex,
                        "rest_distance_to_declared_tip": distance,
                        "palmward_displacement": float(delta @ palm_local),
                    }
                )
    angle = float(
        np.interp(time, np.asarray(track["keyframes"])[:, 0], np.asarray(track["keyframes"])[:, 1])
    )
    signed = float(np.mean(deltas, axis=0) @ palm_local) if deltas else None
    mixed = sum(item["bone"] == bone_name for item in clip["tracks"]) != 1
    status = (
        "not_a_positive_curl_sample"
        if angle <= 0
        else "unavailable_no_weighted_tip_samples"
        if signed is None
        else "mixed_tracks_not_isolated"
        if mixed
        else "toward_declared_palm"
        if signed > scale * 1e-07
        else "opposite_declared_palm"
        if signed < -scale * 1e-07
        else "no_measurable_palmward_motion"
    )
    return {
        "bone": bone_name,
        "time_seconds": time,
        "planned_curl_degrees_at_sample": angle,
        "reference_node": reference_name,
        "reference_definition": "Immediate declared bone parent; no anatomical wrist is inferred.",
        "comparison": (
            "Same exported pose with only this control rotation reset to it"
            "s bind rotation; measured in the same posed-parent frame."
        ),
        "palm_direction_in_rest_parent_frame": palm_local.tolist(),
        "declared_bone_tip_palmward_displacement": float(tip_delta @ palm_local),
        "weighted_tip_mean_palmward_displacement": signed,
        "weighted_tip_records": records,
        "status": status,
        "assumptions": (
            "Plan palm direction and tail landmark are anatomically unverif"
            "ied. Nearest eight records with control weight above 0.05 may "
            "include UV duplicates. Mixed tracks on one control cannot be i"
            "solated by resetting its full rotation. A visual review must v"
            "erify anatomy, bend direction and surface shape."
        ),
    }


def measure(model_path: Path, plan: dict[str, Any], expected_sha256: str) -> dict[str, Any]:
    """Return a numerical report; admission failures raise before artifact creation."""
    if not isinstance(expected_sha256, str) or not re.fullmatch("[a-f0-9]{64}", expected_sha256):
        raise ValueError("Expected lowercase rigged GLB SHA-256")
    if glb.sha(model_path) != expected_sha256:
        raise ValueError("Rigged GLB hash differs from the request")
    expected_plan_sha = plan_digest(plan)
    plan = rig_contract.validate_plan(plan)
    report: dict[str, Any] = {
        "schema_version": 1,
        "code_version": CODE_VERSION,
        "coordinate_system": "gltf_y_up_z_front",
        "rigged_sha256": expected_sha256,
        "plan_sha256": expected_plan_sha,
        "source_sha256": plan["source_sha256"],
        "sample_fractions": list(SAMPLE_FRACTIONS),
        "implementation": [
            {"path": name, "sha256": glb.sha(Path(__file__).parent / name)}
            for name in ("rig_metrics.py", "rig_glb.py", "rig_contract.py", "contract.py")
        ],
        "blocking_findings": [],
        "advisories": [],
        "meshes": [],
        "clips": [],
        "visual_review": "required_separately",
        "limitations": [
            (
                "This is a finite-sample linear-skinning diagnostic, not semant"
                "ic acceptance or continuous-time deformation certification."
            ),
            (
                "The plan supplies the required anatomy; an omitted anatomical "
                "requirement cannot be discovered from joint names alone."
            ),
            (
                "Indices and weighted counts refer to primitive POSITION record"
                "s, including duplicated UV/material seam records and mesh inst"
                "ances."
            ),
            (
                "Stretch ratios compare identical rest and posed index pairs. R"
                "atio bins are descriptive and are not universal acceptable-def"
                "ormation thresholds."
            ),
            (
                "Morphs, primitive extensions and CUBICSPLINE are refused rathe"
                "r than approximated. Nonfinite input rejected by the GLB decod"
                "er leaves inventory counts unavailable."
            ),
            (
                "Source lineage is checked against rig metadata; this operation"
                " does not independently compare original source buffers or mat"
                "erials."
            ),
            (
                "Rest-bind identity compares this GLB's skinned and unskinned n"
                "ode transforms. It does not prove intended anatomy, contact, s"
                "eam coverage or source correctness."
            ),
        ],
    }

    def block(code: str, location: str, message: str) -> None:
        report["blocking_findings"].append({"code": code, "location": location, "message": message})

    def finish() -> dict[str, Any]:
        if glb.sha(model_path) != expected_sha256:
            raise ValueError("Source changed during read-only metrics")
        report["source_unchanged"] = True
        report["status"] = (
            "blocked_numeric_findings"
            if report["blocking_findings"]
            else "numeric_checks_complete_visual_review_pending"
        )
        return report

    try:
        doc, binary = glb.read_glb(model_path)
    except (ValueError, KeyError, IndexError, TypeError, OverflowError) as error:
        block("invalid_or_unsupported_glb", "asset", str(error))
        report["nonfinite_values"] = {"status": "decoder_rejected_input_inventory_unavailable"}
        return finish()
    metadata = doc.get("asset", {}).get("extras", {}).get("character_rig", {})
    if (
        metadata.get("source_sha256") != plan["source_sha256"]
        or metadata.get("plan_sha256") != expected_plan_sha
        or metadata.get("schema_version") != 1
    ):
        raise ValueError("Rig metadata does not match the exact source-bound plan")
    report["lineage"] = "rig_metadata_matches_exact_plan_and_original_source_hash"
    report["nonfinite_values"] = {
        "status": "all_decoded_accessors_and_node_transforms_finite",
        "accessor_count": len(doc.get("accessors", [])),
    }
    rest_world = glb.worlds(doc)
    skin_nodes = {
        index
        for skin in doc.get("skins", [])
        for index in skin.get("joints", [])
        if type(index) is int
    }
    joint_names: dict[str, list[int]] = {}
    for index in sorted(skin_nodes):
        if 0 <= index < len(doc["nodes"]):
            name = doc["nodes"][index].get("name")
            if isinstance(name, str):
                joint_names.setdefault(name, []).append(index)
    names = {name: indices[0] for name, indices in joint_names.items() if len(indices) == 1}
    duplicate_required = sorted(
        name for name in plan["bones"] if len(joint_names.get(name, [])) > 1
    )
    if duplicate_required:
        block("ambiguous_joint_names", "joints", ", ".join(duplicate_required))
    all_node_names = {node["name"] for node in doc["nodes"] if isinstance(node.get("name"), str)}
    missing_nodes = sorted(set(plan["bones"]) - all_node_names)
    missing_skin_joints = sorted(
        name for name in plan["bones"] if name in all_node_names and name not in joint_names
    )
    report["joints"] = {
        "required_count": len(plan["bones"]),
        "skin_count": len(doc.get("skins", [])),
        "unique_skin_joint_nodes": len(skin_nodes),
        "missing_nodes": missing_nodes,
        "required_nodes_missing_from_all_skins": missing_skin_joints,
        "duplicate_required_names": duplicate_required,
        "resolved_required_joint_nodes": {
            name: names[name] for name in plan["bones"] if name in names
        },
        "identity_rule": (
            "Logical bone names resolve only against unique actual skin-joi"
            "nt node names; non-joint mesh or wrapper names cannot supply o"
            "r shadow a joint."
        ),
        "weighted_vertices": {
            name: {"above_epsilon": 0, "above_0_05": 0} for name in plan["bones"]
        },
        "weighted_vertices_by_skin_node": {
            str(index): {
                "node_name": doc["nodes"][index].get("name", ""),
                "above_epsilon": 0,
                "above_0_05": 0,
            }
            for index in sorted(skin_nodes)
            if 0 <= index < len(doc["nodes"])
        },
    }
    if missing_nodes or missing_skin_joints:
        block(
            "required_joint_missing",
            "joints",
            "Required nodes or skin joints are missing; see joints inventory",
        )
    parents = {
        child: index
        for index, node in enumerate(doc["nodes"])
        for child in node.get("children", [])
    }
    hierarchy: dict[str, Any] = {
        "comparison": (
            "Every non-root planned bone must directly parent to the resolv"
            "ed planned joint. A planned root may have non-joint wrappers, "
            "but no skin-joint ancestor."
        ),
        "mismatches": [],
        "root_non_joint_ancestors": {},
        "verified_bone_count": 0,
    }
    for name, bone in plan["bones"].items():
        if name not in names or (bone["parent"] is not None and bone["parent"] not in names):
            continue
        actual_parent = parents.get(names[name])
        mismatch = None
        if bone["parent"] is None:
            wrappers, ancestor = ([], actual_parent)
            while ancestor is not None and ancestor not in skin_nodes:
                wrappers.append(ancestor)
                ancestor = parents.get(ancestor)
            hierarchy["root_non_joint_ancestors"][name] = wrappers
            if ancestor is not None:
                mismatch = {"unexpected_joint_ancestor_index": ancestor}
        elif actual_parent != names[bone["parent"]]:
            mismatch = {"expected_parent_node_index": names[bone["parent"]]}
        if mismatch is not None:
            hierarchy["mismatches"].append(
                {
                    "bone": name,
                    "node_index": names[name],
                    "planned_parent": bone["parent"],
                    "actual_parent_node_index": actual_parent,
                    **mismatch,
                }
            )
        else:
            hierarchy["verified_bone_count"] += 1
    hierarchy["status"] = (
        "matches_plan"
        if hierarchy["verified_bone_count"] == len(plan["bones"])
        else "mismatch_or_unresolved_joints"
    )
    report["joints"]["hierarchy"] = hierarchy
    if hierarchy["mismatches"]:
        block(
            "exported_joint_hierarchy_mismatch",
            "joints",
            "Actual node parenting differs from the exact rig plan",
        )
    actual_mesh_names = {node.get("name") for node in doc["nodes"] if "mesh" in node}
    missing_meshes = sorted(set(plan["part_roles"]) - actual_mesh_names)
    if missing_meshes:
        block("required_mesh_missing", "meshes", ", ".join(missing_meshes))
    surfaces, rest_surfaces = ([], [])
    for node_index, node in enumerate(doc["nodes"]):
        if "mesh" not in node:
            continue
        try:
            mesh_index = _integer(node["mesh"], len(doc.get("meshes", [])), "mesh")
        except ValueError as error:
            block("invalid_mesh", f"node:{node_index}", str(error))
            continue
        for primitive_index, _ in enumerate(doc["meshes"][mesh_index].get("primitives", [])):
            location = f"node:{node_index}/primitive:{primitive_index}"
            try:
                surface = decode_surface(doc, binary, node_index, primitive_index)
                static_points = glb.points(surface["positions"], rest_world[node_index])
                if surface["skin"] is None and node.get("name") in plan["part_roles"]:
                    block("required_mesh_unskinned", location, "A planned bound mesh has no skin")
                if surface["skin"] is not None:
                    weights = surface["weights"]
                    bad = int(
                        np.count_nonzero(np.abs(weights.sum(axis=1) - 1) > WEIGHT_SUM_TOLERANCE)
                    )
                    negative = int(np.count_nonzero(weights < 0))
                    zero = int(np.count_nonzero(weights.sum(axis=1) <= WEIGHT_EPSILON))
                    surface["weight_report"] = {
                        "influence_sets": weights.shape[1] // 4,
                        "negative_influences": negative,
                        "zero_weight_vertices": zero,
                        "nonunit_weight_sum_vertices": bad,
                        "maximum_weight_sum_error": float(np.abs(weights.sum(axis=1) - 1).max()),
                    }
                    if bad or negative or zero:
                        block(
                            "invalid_skin_weights",
                            location,
                            "Negative, zero-total or nonunit weights; see mesh inventory",
                        )
                    for index in surface["skin"]["joints"]:
                        influence = np.sum(
                            weights * (surface["skin"]["joints"][surface["joints"]] == index),
                            axis=1,
                        )
                        total = report["joints"]["weighted_vertices_by_skin_node"][str(index)]
                        total["above_epsilon"] += int(np.count_nonzero(influence > WEIGHT_EPSILON))
                        total["above_0_05"] += int(np.count_nonzero(influence > 0.05))
                    for name in plan["bones"]:
                        if name not in names:
                            continue
                        influence = np.sum(
                            weights * (surface["skin"]["joints"][surface["joints"]] == names[name]),
                            axis=1,
                        )
                        surface.setdefault("weighted_vertices", {})[name] = int(
                            np.count_nonzero(influence > WEIGHT_EPSILON)
                        )
                        report["joints"]["weighted_vertices"][name]["above_epsilon"] += int(
                            np.count_nonzero(influence > WEIGHT_EPSILON)
                        )
                        report["joints"]["weighted_vertices"][name]["above_0_05"] += int(
                            np.count_nonzero(influence > 0.05)
                        )
                rest = surface_points(surface, rest_world)
                surface["bind_error"] = float(np.linalg.norm(rest - static_points, axis=1).max())
                surfaces.append(surface)
                rest_surfaces.append(rest)
            except (ValueError, KeyError, IndexError, TypeError, np.linalg.LinAlgError) as error:
                block("invalid_or_unsupported_surface", location, str(error))
    if not surfaces:
        block("no_evaluable_surfaces", "meshes", "No physical triangle surface could be evaluated")
        return finish()
    all_rest = np.vstack(rest_surfaces)
    scale = max(float(np.linalg.norm(np.ptp(all_rest, axis=0))), 1e-12)
    edge_epsilon = max(scale * 1e-10, 1e-12)
    area_epsilon = edge_epsilon**2
    report["tolerances"] = {
        "scene_diagonal": scale,
        "rest_edge_length_epsilon": edge_epsilon,
        "triangle_area_epsilon": area_epsilon,
        "weight_epsilon": WEIGHT_EPSILON,
        "weight_sum_tolerance": WEIGHT_SUM_TOLERANCE,
        "rest_bind_position_tolerance": max(scale * 1e-05, 1e-10),
    }
    for surface, rest in zip(surfaces, rest_surfaces, strict=True):
        area = triangle_areas(rest, surface["triangles"])
        surface["rest_areas"] = area
        degenerate = int(np.count_nonzero(area <= area_epsilon))
        if degenerate:
            report["advisories"].append(
                {
                    "code": "rest_degenerate_triangles",
                    "node_index": surface["node_index"],
                    "primitive_index": surface["primitive_index"],
                    "count": degenerate,
                }
            )
        if surface["bind_error"] > report["tolerances"]["rest_bind_position_tolerance"]:
            block(
                "rest_bind_identity_mismatch",
                f"node:{surface['node_index']}/primitive:{surface['primitive_index']}",
                "Skin at bind rest moves its original node-transformed surface",
            )
        report["meshes"].append(
            {
                "node_index": surface["node_index"],
                "node_name": surface["node_name"],
                "primitive_index": surface["primitive_index"],
                "position_records": len(rest),
                "triangles": len(surface["triangles"]),
                "unique_index_edges": len(surface["edges"]),
                "degenerate_rest_triangles": degenerate,
                "rest_bind_maximum_position_error": surface["bind_error"],
                "skin": surface.get("weight_report"),
                "weighted_vertices": surface.get("weighted_vertices", {}),
            }
        )
    report["joints"]["zero_weight_required_bones"] = [
        name
        for name, value in report["joints"]["weighted_vertices"].items()
        if value["above_epsilon"] == 0
    ]
    report["joints"]["zero_weight_interpretation"] = (
        "Reported for review; parent controls may intentionally influen"
        "ce geometry only through descendants. Zero direct weights alon"
        "e are not a missing-joint verdict."
    )
    animations = {}
    for animation in doc.get("animations", []):
        name = animation.get("name", "")
        if name in animations:
            block("duplicate_clip_name", "clips", name)
        animations[name] = animation
    report["extra_clips_not_sampled"] = sorted(
        set(animations) - {clip["name"] for clip in plan["clips"]}
    )
    for clip in plan["clips"]:
        name = clip["name"]
        entry = {
            "name": name,
            "duration_seconds": clip["duration_seconds"],
            "samples": [],
            "curl_probes": [],
        }
        report["clips"].append(entry)
        if name not in animations:
            block("required_clip_missing", name, "Planned animation is absent")
            continue
        try:
            tracks = decode_animation(doc, binary, animations[name])
            entry["channels"] = [
                {
                    "node_index": track["node"],
                    "node_name": doc["nodes"][track["node"]].get("name", ""),
                    "path": track["path"],
                    "interpolation": track["mode"],
                    "keyframes": len(track["times"]),
                    "time_range": [float(track["times"][0]), float(track["times"][-1])],
                }
                for track in tracks
            ]
            entry["channel_count"] = len(tracks)
            entry["channel_counts_by_path"] = {
                path: sum(track["path"] == path for track in tracks)
                for path in ("translation", "rotation", "scale")
            }
            expected = {
                (names[track["bone"]], "rotation")
                for track in clip["tracks"]
                if track["bone"] in names
            }
            if plan.get("grounding") and plan["grounding"]["root_bone"] in names:
                expected.add((names[plan["grounding"]["root_bone"]], "translation"))
            actual = {(track["node"], track["path"]) for track in tracks}
            entry["missing_channels"] = [
                {"node_name": doc["nodes"][node].get("name", ""), "path": path}
                for node, path in sorted(expected - actual)
            ]
            if entry["missing_channels"]:
                block("required_animation_channel_missing", name, "Planned channels are missing")
            for track in tracks:
                if (
                    track["times"][0] > 1e-05
                    or track["times"][-1] < clip["duration_seconds"] - 1e-05
                ):
                    block(
                        "animation_does_not_cover_plan_duration",
                        name,
                        "An exported channel does not span the full planned clip",
                    )
            for fraction in SAMPLE_FRACTIONS:
                time = clip["duration_seconds"] * fraction
                world = sample_worlds(doc, tracks, time)
                sample = {"fraction": fraction, "time_seconds": time, "surfaces": []}
                for surface, rest in zip(surfaces, rest_surfaces, strict=True):
                    posed = surface_points(surface, world)
                    areas = triangle_areas(posed, surface["triangles"])
                    newly_degenerate = int(
                        np.count_nonzero(
                            (areas <= area_epsilon) & (surface["rest_areas"] > area_epsilon)
                        )
                    )
                    metric = {
                        "node_index": surface["node_index"],
                        "primitive_index": surface["primitive_index"],
                        **edge_measurements(rest, posed, surface["edges"], edge_epsilon),
                        "degenerate_posed_triangles": int(np.count_nonzero(areas <= area_epsilon)),
                        "newly_degenerate_triangles": newly_degenerate,
                        "vertex_displacement": distribution(np.linalg.norm(posed - rest, axis=1)),
                        "bounds": {
                            "min": posed.min(axis=0).tolist(),
                            "max": posed.max(axis=0).tolist(),
                        },
                    }
                    if newly_degenerate:
                        report["advisories"].append(
                            {
                                "code": "newly_degenerate_posed_triangles",
                                "clip": name,
                                "time_seconds": time,
                                "node_index": surface["node_index"],
                                "primitive_index": surface["primitive_index"],
                                "count": newly_degenerate,
                            }
                        )
                    sample["surfaces"].append(metric)
                entry["samples"].append(sample)
            if hierarchy["status"] == "matches_plan":
                for track in clip["tracks"]:
                    if track.get("kind") != "curl":
                        continue
                    peak_time, peak_angle = max(track["keyframes"], key=lambda pair: pair[1])
                    probe_times = {
                        clip["duration_seconds"] * fraction for fraction in SAMPLE_FRACTIONS
                    }
                    if peak_angle > 0:
                        probe_times.add(float(peak_time))
                    for time in sorted(probe_times):
                        probe = _curl_probe(
                            doc,
                            surfaces,
                            rest_world,
                            rest_surfaces,
                            sample_worlds(doc, tracks, time),
                            tracks,
                            {**clip, "_bones": plan["bones"]},
                            track,
                            time,
                            names,
                            scale,
                        )
                        entry["curl_probes"].append(probe)
                        if probe["status"] in {
                            "opposite_declared_palm",
                            "no_measurable_palmward_motion",
                            "unavailable_no_weighted_tip_samples",
                        }:
                            block(
                                "curl_does_not_demonstrate_declared_palmward_motion",
                                f"{name}/{track['bone']}/{time}",
                                probe["status"] + "; anatomical labels still need visual review",
                            )
                        elif probe["status"] == "mixed_tracks_not_isolated":
                            report["advisories"].append(
                                {
                                    "code": "curl_probe_not_isolated",
                                    "clip": name,
                                    "bone": track["bone"],
                                    "time_seconds": time,
                                }
                            )
            else:
                entry["curl_probes_skipped_reason"] = (
                    "Required joint identity or exported hierarchy does not match t"
                    "he plan; declared-parent curl frames would not be reliable."
                )
        except (ValueError, KeyError, IndexError, TypeError, np.linalg.LinAlgError) as error:
            block("animation_evaluation_incomplete", name, str(error))
    return finish()


def run(request: dict[str, Any], input_root: Path, output_root: Path) -> dict[str, Any]:
    rig_contract.fields(request, {"schema_version", "operation", "source", "plan", "output_dir"})
    if (
        type(request["schema_version"]) is not int
        or request["schema_version"] != 1
        or request["operation"] != "rig_metrics"
    ):
        raise ValueError("Unsupported rig metrics request")
    rig_contract.fields(request["source"], {"path", "sha256"})
    source = contract.confined(input_root, request["source"]["path"], exists=True)
    output = contract.confined(output_root, request["output_dir"])
    if output.exists():
        raise ValueError("Operation output directory already exists")
    report = measure(source, request["plan"], request["source"]["sha256"])
    output.parent.mkdir(parents=True, exist_ok=True)
    output = contract.confined(output_root, request["output_dir"])
    staging = Path(tempfile.mkdtemp(prefix=".rig-metrics-staging-", dir=output.parent))
    try:
        contract.write_json(staging / "report.json", report)
        contract.write_json(staging / "request.json", request)
        contract.write_json(
            staging / "manifest.json",
            {
                "schema_version": 1,
                "operation": "rig_metrics",
                "source": request["source"],
                "implementation": report["implementation"],
                "artifacts": [
                    {"path": path.name, "sha256": glb.sha(path), "size_bytes": path.stat().st_size}
                    for path in sorted(staging.iterdir())
                ],
            },
        )
        if glb.sha(source) != request["source"]["sha256"] or output.exists():
            raise ValueError("Source or destination changed during metrics persistence")
        contract.confined(output_root, request["output_dir"])
        staging.rename(output)
    except BaseException:
        shutil.rmtree(staging)
        raise
    return {
        "status": report["status"],
        "operation": "rig_metrics",
        "report": (Path(request["output_dir"]) / "report.json").as_posix(),
        "manifest": (Path(request["output_dir"]) / "manifest.json").as_posix(),
        "blocking_findings": len(report["blocking_findings"]),
        "source_unchanged": True,
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
                    else "Rig metrics failed; inspect the local worker log",
                }
            ),
            flush=True,
        )
        raise SystemExit(1) from None
