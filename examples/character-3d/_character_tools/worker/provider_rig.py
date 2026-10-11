"""Provider-rig preservation, semantic mapping, and bounded local motion adapters.

This worker never predicts joints or skin weights. GLB inputs remain byte-exact
controls; binary FBX is imported with embedded-only dependencies. Animation and
verified normal restoration append data to the actual provider rig. Blender is
loaded only for native format conversion or an explicitly declared FBX motion.
"""

from __future__ import annotations

import copy
import hashlib
import importlib
import itertools
import json
import math
import re
import shutil
import tempfile
from collections import Counter, defaultdict
from collections.abc import Iterable, Iterator, Sequence
from pathlib import Path
from typing import Any, Literal, cast

import numpy as np
from numpy.typing import ArrayLike, NDArray

from _character_tools.worker import contract, rig_metrics
from _character_tools.worker import rig_glb as glb

Record = dict[str, Any]
FloatArray = NDArray[np.floating[Any]]
IntArray = NDArray[np.integer[Any]]
Binary = bytes | bytearray
Worlds = Sequence[FloatArray]
CODE_VERSION = "provider_rig_v2_output_space"
MIXAMO = {
    "pelvis": "Hips",
    "spine_lower": "Spine",
    "spine_upper": "Spine1",
    "chest": "Spine2",
    "neck": "Neck",
    "head": "Head",
    **{
        f"{side}_{role}": prefix + name
        for side, prefix in (("left", "Left"), ("right", "Right"))
        for role, name in (
            ("shoulder", "Shoulder"),
            ("upper_arm", "Arm"),
            ("forearm", "ForeArm"),
            ("hand", "Hand"),
            ("upper_leg", "UpLeg"),
            ("lower_leg", "Leg"),
            ("foot", "Foot"),
            ("toes", "ToeBase"),
        )
    },
}
CHILD = {
    "pelvis": "spine_lower",
    "spine_lower": "spine_upper",
    "spine_upper": "chest",
    "chest": "neck",
    "neck": "head",
    **{
        f"{side}_{a}": f"{side}_{b}"
        for side in ("left", "right")
        for a, b in (
            ("shoulder", "upper_arm"),
            ("upper_arm", "forearm"),
            ("forearm", "hand"),
            ("upper_leg", "lower_leg"),
            ("lower_leg", "foot"),
        )
    },
}
PROTECTED = (
    "nodes",
    "skins",
    "meshes",
    "materials",
    "textures",
    "images",
    "samplers",
    "extensionsUsed",
    "extensionsRequired",
    "scenes",
    "scene",
)
TO_PUBLIC = np.asarray(contract.COORDINATES["blender_to_gltf"], dtype=float)


def _fields(value: object, required: Iterable[str], optional: Iterable[str] = ()) -> None:
    if (
        not isinstance(value, dict)
        or not set(required) <= value.keys()
        or value.keys() - set(required) - set(optional)
    ):
        raise ValueError("Unexpected or missing provider-rig fields")


def _reference(value: Record) -> None:
    _fields(value, {"path", "sha256"})
    contract.relative(value["path"])
    if not isinstance(value["sha256"], str) or not re.fullmatch("[a-f0-9]{64}", value["sha256"]):
        raise ValueError("Expected lowercase input SHA-256")


def _mapping(value: dict[str, str]) -> None:
    if not isinstance(value, dict) or not value or len(value) > 256:
        raise ValueError("Semantic mapping must contain 1 through 256 entries")
    for role, name in value.items():
        if not re.fullmatch("[a-z][a-z0-9_]{0,79}", role):
            raise ValueError("Semantic identifiers must use lower_snake_case")
        if not isinstance(name, str) or not name or len(name) > 256 or ("\x00" in name):
            raise ValueError("Joint names must be nonempty exact names")
    if len(set(value.values())) != len(value):
        raise ValueError("Multiple semantics cannot alias the same joint")


def validate_options(options: Record) -> Record:
    _fields(
        options,
        {"required_joints"},
        {
            "mapping_preset",
            "semantic_mapping",
            "diagnostics",
            "fps",
            "motion",
            "preservation",
            "part_roles",
            "material_policy",
            "target_height",
        },
    )
    if ("mapping_preset" in options) == ("semantic_mapping" in options):
        raise ValueError("Choose exactly one mapping preset or exact semantic mapping")
    if "mapping_preset" in options and options["mapping_preset"] != "mixamo_biped":
        raise ValueError("Unsupported explicit mapping preset")
    if "semantic_mapping" in options:
        _mapping(options["semantic_mapping"])
    required = options["required_joints"]
    if (
        not isinstance(required, list)
        or not required
        or len(required) > 256
        or (len(set(required)) != len(required))
        or any(
            not isinstance(x, str) or not re.fullmatch("[a-z][a-z0-9_]{0,79}", x) for x in required
        )
    ):
        raise ValueError("Required joints must be unique semantic identifiers")
    if type(options.get("diagnostics", True)) is not bool:
        raise ValueError("diagnostics must be boolean")
    if options.get("material_policy", "preserve") not in {"preserve", "matte"}:
        raise ValueError("Material policy must be preserve or matte")
    if "target_height" in options and contract.finite(options["target_height"]) <= 0:
        raise ValueError("Target height must be positive")
    if type(options.get("fps", 24)) is not int or not 1 <= options.get("fps", 24) <= 60:
        raise ValueError("Diagnostic FPS must be from 1 through 60")
    roles = options.get("part_roles", {})
    if not isinstance(roles, dict) or any(
        (
            not isinstance(k, str) or not isinstance(v, str) or (not k) or (not v)
            for k, v in roles.items()
        )
    ):
        raise ValueError("Part roles require exact mesh names and nonempty role names")
    if "motion" in options:
        motion = options["motion"]
        _fields(
            motion,
            {"source", "clip", "fps"},
            {"source_clip", "source_mapping", "source_mapping_preset"},
        )
        _reference(motion["source"])
        contract.identifier(motion["clip"])
        if type(motion["fps"]) is not int or not 1 <= motion["fps"] <= 60:
            raise ValueError("Motion FPS must be from 1 through 60")
        if ("source_mapping" in motion) == ("source_mapping_preset" in motion):
            raise ValueError("Motion requires an explicit source mapping or mapping preset")
        if "source_mapping" in motion:
            _mapping(motion["source_mapping"])
        if "source_mapping_preset" in motion and motion["source_mapping_preset"] != "mixamo_biped":
            raise ValueError("Unsupported source mapping preset")
        if "source_clip" in motion and (
            not isinstance(motion["source_clip"], str) or not motion["source_clip"]
        ):
            raise ValueError("Source clip must be an exact nonempty name")
    if "preservation" in options:
        policy = options["preservation"]
        _fields(policy, {"source", "mode"}, {"transform", "max_position_error", "uv_tolerance"})
        _reference(policy["source"])
        if policy["mode"] not in {"audit", "restore_normals"}:
            raise ValueError("Only surface audit and verified normal restoration are supported")
        if "transform" in policy:
            similarity(policy["transform"])
        for key, default, maximum in (
            ("max_position_error", 1e-05, 0.0015),
            ("uv_tolerance", 1e-06, 1e-05),
        ):
            if not 0 < contract.finite(policy.get(key, default)) <= maximum:
                raise ValueError("Preservation tolerance exceeds the bounded geometry or UV gate")
    return options


def detect_format(path: Path) -> Literal["glb", "fbx"]:
    with Path(path).open("rb") as stream:
        header = stream.read(24)
    if header.startswith(b"glTF"):
        return "glb"
    if header.startswith(b"Kaydara FBX Binary"):
        return "fbx"
    raise ValueError(
        "Provider asset must be embedded GLB or binary FBX; filename is not format evidence"
    )


def semantic_mapping(
    doc: Record,
    *,
    preset: str | None = None,
    mapping: dict[str, str] | None = None,
    required: Iterable[str] = (),
) -> dict[str, int]:
    """Resolve semantics only against actual skin joints, never similarly named meshes."""
    joint_nodes = sorted({index for skin in doc.get("skins", []) for index in skin["joints"]})
    if not joint_nodes:
        raise ValueError("Provider asset has no actual skin joints")
    names = defaultdict(list)
    for index in joint_nodes:
        if type(index) is not int or not 0 <= index < len(doc["nodes"]):
            raise ValueError("Invalid provider joint index")
        name = doc["nodes"][index].get("name", "")
        key = (
            re.sub("[^a-z0-9]", "", name.lower().split(":")[-1]).removeprefix("mixamorig")
            if preset
            else name
        )
        names[key].append(index)
    if preset is not None and preset != "mixamo_biped":
        raise ValueError("Unsupported explicit mapping preset")
    targets = MIXAMO if preset else mapping
    if not targets:
        raise ValueError("An explicit semantic mapping is required")
    resolved = {}
    for role, name in targets.items():
        candidates = names.get(name.lower() if preset else name, [])
        if len(candidates) > 1:
            raise ValueError(f"Ambiguous actual skin-joint name for {role}")
        if candidates:
            resolved[role] = candidates[0]
    missing = sorted(set(required) - resolved.keys())
    if missing:
        raise ValueError("Required provider joints are absent: " + ", ".join(missing))
    if len(set(resolved.values())) != len(resolved):
        raise ValueError("Semantic roles resolve to duplicate provider joints")
    return resolved


def surfaces(doc: Record, binary: Binary) -> list[Record]:
    records = []
    for node_index, node in enumerate(doc.get("nodes", [])):
        if "mesh" not in node:
            continue
        for primitive_index, _ in enumerate(doc["meshes"][node["mesh"]]["primitives"]):
            value = rig_metrics.decode_surface(doc, binary, node_index, primitive_index)
            if value["skin"] is not None and (
                np.any(value["weights"] < 0) or np.any(np.abs(value["weights"].sum(1) - 1) > 1e-05)
            ):
                raise ValueError("Provider skin weights are negative, zero or not normalized")
            records.append(value)
    if not records:
        raise ValueError("Provider asset has no supported mesh surfaces")
    return records


def inventory(
    doc: Record, binary: Binary, mapping: dict[str, int], part_roles: dict[str, str] | None = None
) -> Record:
    world = glb.worlds(doc)
    records = surfaces(doc, binary)
    parents = {
        child: index
        for index, node in enumerate(doc["nodes"])
        for child in node.get("children", [])
    }
    meshes, all_points = ([], [])
    totals: Counter[str] = Counter()
    weighted_nodes: set[int] = set()
    for record in records:
        points = rig_metrics.surface_points(record, world)
        referenced = np.unique(record["triangles"])
        all_points.append(points[referenced])
        name = doc["nodes"][record["node_index"]].get("name", "")
        usage = {}
        if record["skin"] is not None:
            weighted_nodes.update(
                int(node)
                for node in record["skin"]["joints"][record["joints"][record["weights"] > 1e-08]]
            )
            for role, node in mapping.items():
                indexes = np.flatnonzero(record["skin"]["joints"] == node)
                weights = (record["weights"] * np.isin(record["joints"], indexes)).sum(1)
                usage[role] = {
                    "vertices_above_1e_8": int((weights > 1e-08).sum()),
                    "mean_weight": float(weights.mean()),
                    "vertices_above_0_05": int((weights > 0.05).sum()),
                }
                totals[role] += int(usage[role]["vertices_above_1e_8"])
        meshes.append(
            {
                "name": name,
                "node_index": record["node_index"],
                "primitive_index": record["primitive_index"],
                "part_role": (part_roles or {}).get(name),
                "vertex_count": len(points),
                "triangle_count": len(record["triangles"]),
                "skinned": record["skin"] is not None,
                "bounds": _bounds(points[referenced]),
                "joint_usage": usage,
            }
        )
    joints = sorted({node for skin in doc.get("skins", []) for node in skin["joints"]})

    def descendants(node: int) -> set[int]:
        return {node} | set().union(
            *(descendants(child) for child in doc["nodes"][node].get("children", []))
        )

    controls = {role: sorted(descendants(node) & weighted_nodes) for role, node in mapping.items()}
    return {
        "joint_count": len(joints),
        "skin_count": len(doc.get("skins", [])),
        "semantic_mapping": {
            role: {
                "node_index": node,
                "name": doc["nodes"][node].get("name", ""),
                "head": world[node][:3, 3].tolist(),
                "world_matrix": world[node].tolist(),
                "parent_node_index": parents.get(node),
            }
            for role, node in mapping.items()
        },
        "joints": [
            {
                "node_index": node,
                "name": doc["nodes"][node].get("name", ""),
                "parent_node_index": parents.get(node),
                "world_matrix": world[node].tolist(),
            }
            for node in joints
        ],
        "zero_weight_semantics": sorted(role for role in mapping if totals[role] == 0),
        "unsupported_control_semantics": sorted(role for role in mapping if not controls[role]),
        "controlled_weighted_nodes": controls,
        "bounds": _bounds(np.vstack(all_points)),
        "meshes": meshes,
        "material_count": len(doc.get("materials", [])),
        "image_count": len(doc.get("images", [])),
        "anatomical_front": "measured_from_mapped_leg_axis_when_diagnostics_requested",
    }


def material_finish(doc: Record, policy: str) -> list[Record]:
    """Persist the declared global finish while retaining the raw provider control."""
    if policy == "preserve":
        return []
    changes = []
    for index, material in enumerate(doc.get("materials", [])):
        before = copy.deepcopy(material)
        pbr = material.setdefault("pbrMetallicRoughness", {})
        pbr.update(metallicFactor=0, roughnessFactor=1)
        pbr.pop("metallicRoughnessTexture", None)
        extensions = material.setdefault("extensions", {})
        for key in (
            "KHR_materials_unlit",
            "KHR_materials_clearcoat",
            "KHR_materials_sheen",
            "KHR_materials_specular",
        ):
            extensions.pop(key, None)
        extensions["KHR_materials_specular"] = {"specularFactor": 0.08}
        if "normalTexture" in material:
            material["normalTexture"]["scale"] = min(
                material["normalTexture"].get("scale", 1), 0.15
            )
        changes.append(
            {
                "material_index": index,
                "policy": policy,
                "before": before,
                "after": copy.deepcopy(material),
            }
        )
    if changes:
        doc["extensionsUsed"] = sorted(
            set(doc.get("extensionsUsed", [])) | {"KHR_materials_specular"}
        )
    return changes


def _bounds(points: FloatArray) -> Record:
    if not np.isfinite(points).all() or not len(points):
        raise ValueError("Cannot measure empty or nonfinite geometry")
    return {
        "min": points.min(0).tolist(),
        "max": points.max(0).tolist(),
        "dimensions": np.ptp(points, axis=0).tolist(),
    }


def rest_surface_bounds(doc: Record, binary: Binary) -> Record:
    """Measure referenced vertices with actual skin bind matrices in world space."""
    world = glb.worlds(doc)
    points = [
        rig_metrics.surface_points(surface, world)[np.unique(surface["triangles"])]
        for surface in surfaces(doc, binary)
    ]
    return _bounds(np.vstack(points))


def _output_postcondition(bounds: Record, target_height: float) -> None:
    height = float(bounds["dimensions"][1])
    ground = float(bounds["min"][1])
    if (
        not math.isfinite(height)
        or not math.isfinite(ground)
        or (not math.isclose(height, target_height, rel_tol=1e-06, abs_tol=0))
        or (abs(ground) > target_height * 1e-06)
    ):
        raise ValueError("Exported output height or ground differs from the requested space")


def normalize_output_space(doc: Record, binary: Binary, target_height: float) -> Record:
    """Parent the entire rig once; preserve bind matrices and all existing node locals."""
    target_height = contract.finite(target_height)
    if target_height <= 0:
        raise ValueError("Target height must be positive")
    before = rest_surface_bounds(doc, binary)
    height = float(before["dimensions"][1])
    if not math.isfinite(height) or height <= 0:
        raise ValueError("Cannot normalize a degenerate or nonfinite rest height")
    nodes = doc["nodes"]
    scenes = doc.get("scenes", [])
    if len(scenes) != 1 or type(doc.get("scene", 0)) is not int or doc.get("scene", 0) != 0:
        raise ValueError("Output normalization requires one unambiguous scene")
    roots = scenes[0].get("nodes", [])
    children = {child for node in nodes for child in node.get("children", [])}
    expected = {index for index in range(len(nodes)) if index not in children}
    if (
        not isinstance(roots, list)
        or not roots
        or any(type(index) is not int for index in roots)
        or (len(roots) != len(set(roots)))
        or (set(roots) != expected)
    ):
        raise ValueError("Scene roots must contain each mesh and skeleton hierarchy exactly once")
    scale = target_height / height
    ground = float(before["min"][1])
    translation = [0.0, -scale * ground, 0.0]
    if scale <= 0 or not math.isfinite(scale) or (not all(map(math.isfinite, translation))):
        raise ValueError("Output normalization requires a finite positive uniform transform")
    node_index = len(nodes)
    wrapper = {
        "name": f"provider_output_space_{node_index}",
        "children": list(roots),
        "scale": [scale, scale, scale],
        "translation": translation,
    }
    nodes.append(wrapper)
    scenes[0]["nodes"] = [node_index]
    after = rest_surface_bounds(doc, binary)
    _output_postcondition(after, target_height)
    return {
        "operation": "uniform_common_scene_parent",
        "target_height": target_height,
        "before_bounds": before,
        "after_bounds": after,
        "height_before": height,
        "height_after": after["dimensions"][1],
        "uniform_scale": scale,
        "ground_before": ground,
        "ground_after": after["min"][1],
        "translation": translation,
        "matrix": glb.local(wrapper).tolist(),
        "node_index": node_index,
        "original_scene_roots": list(roots),
        "existing_node_locals_exact": True,
        "inverse_bind_matrices_and_weights_exact": True,
        "local_animation_channels_exact": True,
        "postcondition_passed": True,
        "postcondition_relative_tolerance": 1e-06,
        "postcondition_basis": "Referenced world-space vertices evaluated with actual skin binds",
    }


def _unit(value: ArrayLike) -> FloatArray:
    value = np.asarray(value, dtype=float)
    length = np.linalg.norm(value)
    if not np.isfinite(value).all() or length < 1e-10:
        raise ValueError("Degenerate anatomical direction")
    return cast(FloatArray, value / length)


def _rotation(matrix: ArrayLike) -> FloatArray:
    values = np.asarray(matrix, dtype=float)[:3, :3]
    u, _, vt = np.linalg.svd(values)
    result = u @ vt
    if np.linalg.det(result) < 0:
        raise ValueError("Reflected joint transforms require an explicit motion adapter")
    return result


def _quaternion(matrix: ArrayLike) -> FloatArray:
    m = np.asarray(matrix, dtype=float)
    k = (
        np.array(
            [
                [
                    m[0, 0] - m[1, 1] - m[2, 2],
                    m[0, 1] + m[1, 0],
                    m[0, 2] + m[2, 0],
                    m[2, 1] - m[1, 2],
                ],
                [
                    m[0, 1] + m[1, 0],
                    m[1, 1] - m[0, 0] - m[2, 2],
                    m[1, 2] + m[2, 1],
                    m[0, 2] - m[2, 0],
                ],
                [
                    m[0, 2] + m[2, 0],
                    m[1, 2] + m[2, 1],
                    m[2, 2] - m[0, 0] - m[1, 1],
                    m[1, 0] - m[0, 1],
                ],
                [m[2, 1] - m[1, 2], m[0, 2] - m[2, 0], m[1, 0] - m[0, 1], np.trace(m)],
            ]
        )
        / 3
    )
    _, vectors = np.linalg.eigh(k)
    result = vectors[:, -1]
    return result if result[3] >= 0 else -result


def _axis_rotation(axis: ArrayLike, radians: float) -> FloatArray:
    x, y, z = _unit(axis)
    cross = np.array([[0, -z, y], [z, 0, -x], [-y, x, 0]])
    return cast(
        FloatArray,
        np.eye(3) + math.sin(radians) * cross + (1 - math.cos(radians)) * (cross @ cross),
    )


def body_frame(world: Worlds, mapping: dict[str, int]) -> FloatArray:
    if not {"left_upper_leg", "right_upper_leg"} <= mapping.keys():
        raise ValueError("Anatomical motion needs both upper-leg joints")
    up = np.array([0.0, 1.0, 0.0])
    across: FloatArray = (
        world[mapping["left_upper_leg"]][:3, 3] - world[mapping["right_upper_leg"]][:3, 3]
    )
    across = _unit(across - up * np.dot(across, up))
    return np.column_stack((across, up, _unit(np.cross(across, up))))


def _append_clip(
    doc: Record,
    binary: bytearray,
    name: str,
    times: FloatArray,
    rotations: dict[int, list[FloatArray]],
    translations: dict[int, list[FloatArray]] | None = None,
) -> Record:
    if name in {a.get("name") for a in doc.get("animations", [])}:
        raise ValueError("New animation name collides with an existing provider clip")
    animation: Record = {"name": name, "channels": [], "samplers": []}
    timeline = glb.append_accessor(
        doc,
        binary,
        np.asarray(times)[:, None],
        5126,
        "SCALAR",
        {"min": [float(times[0])], "max": [float(times[-1])]},
    )
    for path, tracks, width in (
        ("rotation", rotations, "VEC4"),
        ("translation", translations or {}, "VEC3"),
    ):
        for node, track_values in tracks.items():
            if "matrix" in doc["nodes"][node]:
                raise ValueError("Animating a matrix joint requires explicit TRS normalization")
            values = np.asarray(track_values, dtype=float)
            if len(values) != len(times):
                raise ValueError("Animation samples and times disagree")
            if path == "rotation":
                for i in range(1, len(values)):
                    if np.dot(values[i], values[i - 1]) < 0:
                        values[i] *= -1
            output = glb.append_accessor(doc, binary, values, 5126, width)
            animation["channels"].append(
                {"sampler": len(animation["samplers"]), "target": {"node": node, "path": path}}
            )
            animation["samplers"].append(
                {"input": timeline, "output": output, "interpolation": "LINEAR"}
            )
    if not animation["channels"]:
        raise ValueError("Cannot append an empty animation")
    doc.setdefault("animations", []).append(animation)
    return animation


def append_diagnostics(
    doc: Record, binary: bytearray, mapping: dict[str, int], fps: int = 24
) -> list[Record]:
    """Fixed-hand joint diagnostics; wrist bend is never represented as finger curl."""
    world = glb.worlds(doc)
    axes = body_frame(world, mapping)
    needed = {
        "head",
        "chest",
        *[
            f"{side}_{role}"
            for side in ("left", "right")
            for role in ("upper_arm", "forearm", "hand", "lower_leg")
        ],
    }
    if not needed <= mapping.keys():
        raise ValueError(
            "Diagnostics lack required mapped joints: " + ", ".join(sorted(needed - mapping.keys()))
        )
    specs: dict[str, list[tuple[str, FloatArray, float]]] = {
        "rest": [("head", axes[:, 0], 0.0)],
        "shoulder_raise": [],
        "elbow_bend": [],
        "knee_bend": [],
        "wrist_bend": [],
        "cheer": [],
    }
    for side, sign in (("left", 1), ("right", -1)):
        specs["shoulder_raise"].append((f"{side}_upper_arm", axes[:, 2], sign * 55))
        specs["elbow_bend"].append((f"{side}_forearm", axes[:, 0], -65))
        specs["knee_bend"].append((f"{side}_lower_leg", axes[:, 0], 55))
        specs["wrist_bend"].append((f"{side}_hand", axes[:, 0], -25))
        specs["cheer"] += [
            (f"{side}_upper_arm", axes[:, 2], sign * 65),
            (f"{side}_forearm", axes[:, 0], -25),
        ]
    specs["cheer"].append(("chest", axes[:, 2], 5))
    reports = []
    for name, spec in specs.items():
        duration = 4.0 if name == "cheer" else 2.0
        times = np.linspace(0, duration, round(duration * fps) + 1)
        amount = np.interp(times, [0, duration * 0.25, duration * 0.75, duration], [0, 1, 1, 0])
        rotations = {}
        for role, axis, degrees in spec:
            node = mapping[role]
            rest_local = _rotation(glb.local(doc["nodes"][node]))
            axis_local = _rotation(world[node]).T @ axis
            values = amount
            if name == "cheer" and role == "chest":
                values = np.interp(times, [0, 1, 2, 3, 4], [0, 1, -1, 1, 0])
            rotations[node] = [
                _quaternion(rest_local @ _axis_rotation(axis_local, math.radians(degrees * value)))
                for value in values
            ]
        animation = _append_clip(doc, binary, name, times, rotations)
        reports.append(
            {
                "name": name,
                "duration_seconds": duration,
                "fps": fps,
                "sample_count": len(times),
                "review_times_seconds": [duration * f for f in (0, 0.25, 0.5, 0.75, 1)],
                "author": "deterministic local diagnostic",
                "mapped_joints": [role for role, _, _ in spec],
                "channel_count": len(animation["channels"]),
                "hand_claim": "wrist rotation only; no finger or thumb articulation added",
            }
        )
    return reports


def clip_metrics(doc: Record, binary: Binary, clips: list[Record]) -> list[Record]:
    records = surfaces(doc, binary)
    rest = glb.worlds(doc)
    base = [rig_metrics.surface_points(r, rest) for r in records]
    names = {a.get("name"): a for a in doc.get("animations", [])}
    results = []
    for clip in clips:
        tracks = rig_metrics.decode_animation(doc, binary, names[clip["name"]])
        samples = []
        for at in clip["review_times_seconds"]:
            world = rig_metrics.sample_worlds(doc, tracks, at)
            posed = [rig_metrics.surface_points(r, world) for r in records]
            samples.append(
                {
                    "time_seconds": at,
                    "bounds": _bounds(np.vstack(posed)),
                    "max_vertex_displacement": max(
                        (
                            float(np.linalg.norm(a - b, axis=1).max())
                            for a, b in zip(posed, base, strict=True)
                        )
                    ),
                }
            )
        results.append({"name": clip["name"], "samples": samples, "finite": True})
    return results


def similarity(matrix: ArrayLike) -> tuple[FloatArray, float]:
    value = np.asarray(matrix, dtype=float)
    if (
        value.shape != (4, 4)
        or not np.isfinite(value).all()
        or (not np.allclose(value[3], [0, 0, 0, 1]))
    ):
        raise ValueError("Preservation transform must be a finite affine 4x4 matrix")
    gram = value[:3, :3].T @ value[:3, :3]
    scale = float(np.sqrt(np.trace(gram) / 3))
    if (
        scale <= 0
        or not np.allclose(gram, np.eye(3) * scale * scale, rtol=1e-05, atol=1e-10)
        or np.linalg.det(value[:3, :3]) <= 0
    ):
        raise ValueError("Preservation transform must be a positive uniform similarity")
    return (value, scale)


def _candidates(source: FloatArray, target: FloatArray, tolerance: float) -> Iterator[list[int]]:
    """Spatial hash correspondence; no quadratic all-pairs matrix or SciPy dependency."""
    origin = np.minimum(source.min(0), target.min(0))
    cells = defaultdict(list)
    for index, cell in enumerate(np.floor((source - origin) / tolerance).astype(np.int64)):
        cells[tuple(cell)].append(index)
    for point, cell in zip(
        target, np.floor((target - origin) / tolerance).astype(np.int64), strict=True
    ):
        indexes = [
            index
            for shift in itertools.product((-1, 0, 1), repeat=3)
            for index in cells.get(tuple(cell + shift), [])
        ]
        yield [index for index in indexes if np.linalg.norm(source[index] - point) <= tolerance]


def _surface_bundle(doc: Record, binary: Binary) -> Record:
    world, result, offset = (glb.worlds(doc), [], 0)
    points, uvs, normals, triangles = ([], [], [], [])
    for record in surfaces(doc, binary):
        node = doc["nodes"][record["node_index"]]
        primitive = doc["meshes"][node["mesh"]]["primitives"][record["primitive_index"]]
        attrs = primitive["attributes"]
        if "TEXCOORD_0" not in attrs:
            raise ValueError("Preservation correspondence requires UV coordinates")
        values = rig_metrics.surface_points(record, world)
        uv = glb.accessor(doc, binary, attrs["TEXCOORD_0"])
        if uv.shape != (len(values), 2):
            raise ValueError("UV coordinates do not match the surface")
        if record["skin"] is None:
            matrices = np.repeat(world[record["node_index"]][None], len(values), axis=0)
        else:
            bind = (
                np.asarray([world[index] for index in record["skin"]["joints"]])
                @ record["skin"]["inverse"]
            )
            matrices = sum(
                bind[record["joints"][:, slot]] * record["weights"][:, slot, None, None]
                for slot in range(record["joints"].shape[1])
            )
        raw_normals = glb.accessor(doc, binary, attrs["NORMAL"]) if "NORMAL" in attrs else None
        world_normals = None
        if raw_normals is not None:
            world_normals = np.einsum(
                "vij,vj->vi", np.linalg.inv(matrices[:, :3, :3]).transpose(0, 2, 1), raw_normals
            )
            lengths = np.linalg.norm(world_normals, axis=1)
            if np.any(lengths < 1e-10):
                raise ValueError("Degenerate normals cannot be restored")
            world_normals /= lengths[:, None]
        result.append(
            {
                "record": record,
                "attributes": attrs,
                "offset": offset,
                "count": len(values),
                "bind_matrices": matrices,
                "raw_normals": raw_normals,
            }
        )
        points.append(values)
        uvs.append(uv)
        normals.append(world_normals)
        triangles.append(record["triangles"] + offset)
        offset += len(values)
    return {
        "parts": result,
        "points": np.vstack(points),
        "uv": np.vstack(uvs),
        "normals": np.vstack(cast(list[FloatArray], normals))
        if all(x is not None for x in normals)
        else None,
        "triangles": np.vstack(triangles),
    }


def _axis_similarities(source: FloatArray, target: FloatArray) -> Iterator[FloatArray]:
    center_source = (source.min(0) + source.max(0)) * 0.5
    center_target = (target.min(0) + target.max(0)) * 0.5
    for permutation in itertools.permutations(range(3)):
        for signs in itertools.product((-1, 1), repeat=3):
            rotation = np.eye(3)[list(permutation)] * np.asarray(signs)[:, None]
            if np.linalg.det(rotation) < 0.5:
                continue
            rotated = source @ rotation.T
            a, b = (np.ptp(rotated, axis=0), np.ptp(target, axis=0))
            active = a > max(float(a.max()) * 1e-08, 1e-10)
            scale = float(np.dot(a[active], b[active]) / np.dot(a[active], a[active]))
            value = np.eye(4)
            value[:3, :3] = rotation * scale
            value[:3, 3] = center_target - value[:3, :3] @ center_source
            yield value


def _image_hashes(doc: Record, binary: Binary) -> list[str]:
    result = []
    for image in doc.get("images", []):
        view = doc["bufferViews"][image["bufferView"]]
        start = view.get("byteOffset", 0)
        result.append(hashlib.sha256(binary[start : start + view["byteLength"]]).hexdigest())
    return result


def _image_bytes(doc: Record, binary: Binary, index: int) -> bytes:
    view = doc["bufferViews"][doc["images"][index]["bufferView"]]
    start = view.get("byteOffset", 0)
    return bytes(binary[start : start + view["byteLength"]])


def _pixel_identity(data: bytes) -> Record:
    """Compare decoded RGBA samples, not PNG compression or ancillary metadata.

    Pillow serves the host tests/runtime; Blender's own decoder serves its
    dependency-isolated worker. Decoder identity is recorded with the sample hash.
    Images above eight bits per channel are refused instead of quantized.
    """
    if data.startswith(b"\x89PNG\r\n\x1a\n") and (len(data) < 26 or data[24] > 8):
        raise ValueError("Decoded preservation supports PNG up to eight bits per channel")
    try:
        from io import BytesIO

        from PIL import Image
    except ImportError:
        bpy = importlib.import_module("bpy")
        suffix = ".png" if data.startswith(b"\x89PNG") else ".jpg"
        with tempfile.TemporaryDirectory(prefix="texture-identity-") as temporary:
            path = Path(temporary) / ("image" + suffix)
            path.write_bytes(data)
            image = bpy.data.images.load(str(path), check_existing=False)
            try:
                width, height = image.size
                if not 0 < width * height <= 67108864 or image.is_float:
                    raise ValueError("Unsupported decoded texture size or floating point samples")
                image.colorspace_settings.name = "Non-Color"
                pixels = np.empty(width * height * 4, dtype=np.float32)
                image.pixels.foreach_get(pixels)
                pixels = np.rint(np.clip(pixels.reshape(height, width, 4)[::-1], 0, 1) * 255)
                payload = pixels.astype(np.uint8).tobytes()
                decoder = "blender_rgba8"
            finally:
                bpy.data.images.remove(image)
    else:
        with Image.open(BytesIO(data)) as image:
            width, height = image.size
            if (
                image.format not in {"PNG", "JPEG"}
                or not 0 < width * height <= 67108864
                or image.mode in {"I", "F", "I;16", "I;16B", "I;16L"}
            ):
                raise ValueError("Unsupported decoded texture format, size or sample precision")
            payload = image.convert("RGBA").tobytes()
            decoder = "pillow_rgba8"
    return {
        "width": width,
        "height": height,
        "rgba8_sha256": hashlib.sha256(payload).hexdigest(),
        "decoder": decoder,
    }


def _texture_appearance(
    source_doc: Record,
    source_binary: Binary,
    provider_doc: Record,
    provider_binary: Binary,
    match: Record,
    canonical: IntArray,
) -> Record:
    """Compare texture usage on corresponding triangles, independent of object ordering."""
    source_hashes = _image_hashes(source_doc, source_binary)
    target_hashes = _image_hashes(provider_doc, provider_binary)
    decoded: dict[str, Record] = {}
    identities: list[Record] = []

    def identity(doc: Record, binary: Binary, index: int, hashes: list[str]) -> str:
        raw = hashes[index]
        if raw in source_hashes:
            return raw
        if raw not in decoded:
            decoded[raw] = _pixel_identity(_image_bytes(doc, binary, index))
        for source_index, source_hash in enumerate(source_hashes):
            if source_hash not in decoded:
                decoded[source_hash] = _pixel_identity(
                    _image_bytes(source_doc, source_binary, source_index)
                )
            if decoded[raw] == decoded[source_hash]:
                return source_hash
        return "changed_pixels:" + raw

    for index, raw in enumerate(target_hashes):
        resolved = identity(provider_doc, provider_binary, index, target_hashes)
        identities.append({"provider_index": index, "raw_sha256": raw, "identity": resolved})

    def rounded(value: Any) -> Any:
        if isinstance(value, float):
            return float(format(value, ".6g"))
        if isinstance(value, dict):
            return {k: rounded(v) for k, v in value.items()}
        if isinstance(value, list):
            return [rounded(v) for v in value]
        return value

    def signature(
        doc: Record, material: Record, hashes: list[str], target: bool
    ) -> tuple[Record, Record]:
        slots: Record = {}
        unlit = "KHR_materials_unlit" in material.get("extensions", {})

        def walk(value: Any, path: tuple[str, ...] = ()) -> None:
            if not isinstance(value, dict):
                return
            for key, item in value.items():
                slot = (*path, key)
                if key.endswith("Texture") and isinstance(item, dict) and ("index" in item):
                    if unlit and slot != ("pbrMetallicRoughness", "baseColorTexture"):
                        continue
                    texture = doc["textures"][item["index"]]
                    if texture.get("extensions") or "source" not in texture:
                        raise ValueError("Unsupported texture source extension in preservation")
                    image_index = texture["source"]
                    transform = item.get("extensions", {}).get("KHR_texture_transform", {})
                    if set(item.get("extensions", {})) - {"KHR_texture_transform"}:
                        raise ValueError("Unsupported texture-coordinate extension in preservation")
                    sampler = (
                        doc.get("samplers", [])[texture["sampler"]] if "sampler" in texture else {}
                    )
                    slots["/".join(slot)] = {
                        "image": identities[image_index]["identity"]
                        if target
                        else hashes[image_index],
                        "texcoord": transform.get("texCoord", item.get("texCoord", 0)),
                        "offset": transform.get("offset", [0, 0]),
                        "scale": transform.get("scale", [1, 1]),
                        "rotation": transform.get("rotation", 0),
                        "sampler": {
                            "mag": sampler.get("magFilter", "unspecified"),
                            "min": sampler.get("minFilter", "unspecified"),
                            "wrap_s": sampler.get("wrapS", 10497),
                            "wrap_t": sampler.get("wrapT", 10497),
                        },
                    }
                else:
                    walk(item, slot)

        walk(material)
        paint = {
            "base_color": material.get("pbrMetallicRoughness", {}).get(
                "baseColorFactor", [1, 1, 1, 1]
            ),
            "alpha_mode": material.get("alphaMode", "OPAQUE"),
            "alpha_cutoff": material.get("alphaCutoff", 0.5)
            if material.get("alphaMode") == "MASK"
            else None,
        }
        return (rounded(slots), rounded(paint))

    def usage(
        doc: Record, binary: Binary, bundle: Record, target: bool
    ) -> tuple[
        Counter[tuple[tuple[int, ...], str]], Counter[tuple[tuple[int, ...], str, str, str]]
    ]:
        texture_counts: Counter[tuple[tuple[int, ...], str]] = Counter()
        appearance_counts: Counter[tuple[tuple[int, ...], str, str, str]] = Counter()
        for part in bundle["parts"]:
            record = part["record"]
            primitive = doc["meshes"][doc["nodes"][record["node_index"]]["mesh"]]["primitives"][
                record["primitive_index"]
            ]
            material = (
                doc.get("materials", [])[primitive["material"]] if "material" in primitive else {}
            )
            slots, paint = signature(
                doc, material, target_hashes if target else source_hashes, target
            )
            if any(slot["texcoord"] != 0 for slot in slots.values()):
                raise ValueError("Texture preservation requires verified TEXCOORD_0 usage")
            local = record["triangles"] + part["offset"]
            values = canonical[match["mapping"][local] if target else local]
            colors = None
            if "COLOR_0" in primitive["attributes"]:
                colors = glb.accessor(doc, binary, primitive["attributes"]["COLOR_0"])
                if colors.shape[1] == 3:
                    colors = np.column_stack((colors, np.ones(len(colors))))
            slot_key = json.dumps(slots, sort_keys=True)
            paint_key = json.dumps(paint, sort_keys=True)
            for row, triangle in zip(values, record["triangles"], strict=True):
                vertex_colors = colors[triangle] if colors is not None else np.ones((3, 4))
                entries = list(zip(row.tolist(), rounded(vertex_colors.tolist()), strict=True))
                best = min(range(3), key=lambda i: tuple(row[i:].tolist() + row[:i].tolist()))
                triangle_key = tuple(row[best:].tolist() + row[:best].tolist())
                color_key = json.dumps(entries[best:] + entries[:best])
                texture_counts[triangle_key, slot_key] += 1
                appearance_counts[triangle_key, slot_key, paint_key, color_key] += 1
        return (texture_counts, appearance_counts)

    source_usage = usage(source_doc, source_binary, match["source"], False)
    target_usage = usage(provider_doc, provider_binary, match["target"], True)
    return {
        "texture_binding_preserved": source_usage[0] == target_usage[0],
        "appearance_preserved": source_usage[1] == target_usage[1],
        "provider_images_pixel_identical_to_source": all(
            not x["identity"].startswith("changed_pixels:") for x in identities
        ),
        "image_identity": identities,
        "decoded_image_identity": decoded,
        "appearance_scope": (
            "Matched surface texture slots, sampled UV transforms, samplers"
            ", base-color tint, vertex colors and opacity; light response a"
            "nd normals are separate policies."
        ),
    }


def audit_preservation(
    source_doc: Record,
    source_binary: Binary,
    provider_doc: Record,
    provider_binary: Binary,
    policy: Record,
) -> tuple[Record, Record]:
    """Verify transform, positions, UVs and triangle winding before any normal transfer."""
    source, target = (
        _surface_bundle(source_doc, source_binary),
        _surface_bundle(provider_doc, provider_binary),
    )
    max_error = policy.get("max_position_error", 1e-05)
    uv_tolerance = policy.get("uv_tolerance", 1e-06)
    transform = np.asarray(policy["transform"], dtype=float) if "transform" in policy else None
    if transform is None:
        chosen = np.linspace(
            0, len(target["points"]) - 1, min(128, len(target["points"])), dtype=int
        )
        for candidate in _axis_similarities(source["points"], target["points"]):
            _, scale = similarity(candidate)
            transformed = glb.points(source["points"], candidate)
            valid = True
            for vertex, matches in zip(
                chosen,
                _candidates(transformed, target["points"][chosen], max_error * scale),
                strict=True,
            ):
                if not any(
                    np.max(np.abs(source["uv"][index] - target["uv"][vertex])) <= uv_tolerance
                    for index in matches
                ):
                    valid = False
                    break
            if valid:
                transform = candidate
                break
        if transform is None:
            raise ValueError(contract.PRESERVATION_REFUSALS["correspondence"])
    transform, scale = similarity(transform)
    transformed = glb.points(source["points"], transform)
    mapping, errors, ambiguous, resolved = ([], [], 0, 0)
    source_normals = source["normals"]
    target_normals = target["normals"]
    rotation_inverse = np.linalg.inv(transform[:3, :3])
    for vertex, candidates in enumerate(
        _candidates(transformed, target["points"], max_error * scale)
    ):
        candidates = [
            index
            for index in candidates
            if np.max(np.abs(source["uv"][index] - target["uv"][vertex])) <= uv_tolerance
        ]
        if not candidates:
            raise ValueError(contract.PRESERVATION_REFUSALS["geometry_uv"])
        best = min(
            candidates, key=lambda i: np.linalg.norm(transformed[i] - target["points"][vertex])
        )
        if source_normals is not None and len(candidates) > 1:
            dots = source_normals[candidates] @ source_normals[best]
            if np.min(dots) < math.cos(math.radians(1)):
                ambiguous += 1
                if target_normals is not None:
                    projected = source_normals[candidates] @ rotation_inverse
                    projected /= np.linalg.norm(projected, axis=1)[:, None]
                    scores = projected @ target_normals[vertex]
                    best = candidates[int(np.argmax(scores))]
                    resolved += 1
        mapping.append(best)
        errors.append(float(np.linalg.norm(transformed[best] - target["points"][vertex]) / scale))
    error_values = np.asarray(errors)
    if np.percentile(error_values, 95) > 1e-05:
        raise ValueError(contract.PRESERVATION_REFUSALS["p95"])
    canonical_indexes: list[int] = []
    for vertex, candidates in enumerate(_candidates(source["points"], source["points"], max_error)):
        canonical_indexes.append(
            min(
                index
                for index in candidates
                if np.max(np.abs(source["uv"][index] - source["uv"][vertex])) <= uv_tolerance
            )
        )
    canonical = np.asarray(canonical_indexes)

    def triangles(values: IntArray) -> Counter[tuple[int, ...]]:
        return Counter(
            min(tuple(row), tuple(np.roll(row, 1)), tuple(np.roll(row, 2))) for row in values
        )

    connectivity = triangles(canonical[source["triangles"]]) == triangles(
        canonical[np.asarray(mapping)[target["triangles"]]]
    )
    if not connectivity:
        raise ValueError(contract.PRESERVATION_REFUSALS["connectivity"])
    source_images, target_images = (
        _image_hashes(source_doc, source_binary),
        _image_hashes(provider_doc, provider_binary),
    )
    report = {
        "status": "surface_correspondence_verified",
        "source_to_provider": transform.tolist(),
        "registration": "explicit_verified_similarity"
        if "transform" in policy
        else "measured_axis_rotation_uniform_scale_translation",
        "position_error_source_units": rig_metrics.distribution(error_values),
        "uv_max_error": float(np.max(np.abs(source["uv"][mapping] - target["uv"]))),
        "triangle_connectivity_and_winding_equal": connectivity,
        "source_vertex_count": len(source["points"]),
        "provider_vertex_count": len(target["points"]),
        "ambiguous_normal_correspondence_vertices": ambiguous,
        "ambiguous_normals_resolved_by_provider_normal": resolved,
        "provider_image_payloads_all_from_source": all(
            value in source_images for value in target_images
        ),
        "source_image_sha256": source_images,
        "provider_image_sha256": target_images,
        "materials_json_equal": source_doc.get("materials") == provider_doc.get("materials"),
        "material_limit": (
            "Serialization inequality is not evidence of a visible material defect."
        ),
    }
    match = {
        "source": source,
        "target": target,
        "mapping": np.asarray(mapping),
        "transform": transform,
    }
    report.update(
        _texture_appearance(
            source_doc, source_binary, provider_doc, provider_binary, match, canonical
        )
    )
    return (report, match)


def restore_verified_normals(
    doc: Record, binary: bytearray, match: Record, audit: Record
) -> Record:
    unresolved = audit["ambiguous_normal_correspondence_vertices"] - audit.get(
        "ambiguous_normals_resolved_by_provider_normal", 0
    )
    if unresolved:
        raise ValueError(contract.PRESERVATION_REFUSALS["ambiguous_normals"])
    source, target = (match["source"], match["target"])
    if source["normals"] is None:
        raise ValueError("Original source has no complete normals")
    desired = source["normals"][match["mapping"]] @ np.linalg.inv(match["transform"][:3, :3])
    desired /= np.linalg.norm(desired, axis=1)[:, None]
    changed = []
    for part in target["parts"]:
        if "TANGENT" in part["attributes"]:
            raise ValueError(
                "Explicit tangent frames require a coordinated tangent restoration adapter"
            )
        start, count = (part["offset"], part["count"])
        normals = np.einsum(
            "vji,vj->vi", part["bind_matrices"][:, :3, :3], desired[start : start + count]
        )
        normals /= np.linalg.norm(normals, axis=1)[:, None]
        attrs = part["attributes"]
        old = attrs.get("NORMAL")
        attrs["NORMAL"] = glb.append_accessor(doc, binary, normals, 5126, "VEC3")
        changed.append(
            {
                "node_index": part["record"]["node_index"],
                "primitive_index": part["record"]["primitive_index"],
                "previous_normal_accessor": old,
                "normal_accessor": attrs["NORMAL"],
            }
        )
    return {
        "operation": "verified_normal_restoration_only",
        "changed_normals": changed,
        "geometry_uv_textures_materials_weights_joints_unchanged": True,
    }


def _native_capture(
    objects: Sequence[Any],
) -> tuple[dict[str, Record], dict[str, Record], list[Record]]:
    bpy = importlib.import_module("bpy")
    from _character_tools.worker import blender_io

    armatures = [obj for obj in objects if obj.type == "ARMATURE"]
    if len(armatures) != 1:
        raise ValueError("Native rig normalization requires exactly one provider armature")
    armature = armatures[0]
    armature.data.pose_position = "REST"
    bpy.context.view_layer.update()
    bones = {}
    for bone in armature.data.bones:
        matrix = TO_PUBLIC @ np.asarray(armature.matrix_world) @ np.asarray(bone.matrix_local)
        bones[bone.name] = {
            "parent": bone.parent.name if bone.parent else None,
            "head": matrix[:3, 3].tolist(),
            "world_matrix": matrix.tolist(),
        }
    meshes = {}
    for obj in objects:
        if obj.type != "MESH":
            continue
        if obj.data.shape_keys or any(modifier.type != "ARMATURE" for modifier in obj.modifiers):
            raise ValueError(
                "Native rig modifiers or morphs require an explicit preservation adapter"
            )
        matrix = TO_PUBLIC @ np.asarray(obj.matrix_world)
        positions = glb.points(np.asarray([v.co[:] for v in obj.data.vertices]), matrix)
        group_names = {group.index: group.name for group in obj.vertex_groups}
        weights = []
        for vertex in obj.data.vertices:
            row = {
                group_names[g.group]: float(g.weight)
                for g in vertex.groups
                if group_names[g.group] in bones and g.weight > 1e-08
            }
            total = sum(row.values())
            if total <= 1e-08 or len(row) > 4:
                raise ValueError("Native provider weights need one through four active influences")
            weights.append({name: value / total for name, value in row.items()})
        uv_values: list[set[tuple[float, float]]] = [set() for _ in obj.data.vertices]
        if not obj.data.uv_layers.active:
            raise ValueError("Native rig weight preservation requires UV coordinates")
        for loop, uv in zip(obj.data.loops, obj.data.uv_layers.active.data, strict=True):
            uv_values[loop.vertex_index].add((float(uv.uv.x), 1 - float(uv.uv.y)))
        obj.data.calc_loop_triangles()
        meshes[obj.name] = {
            "positions": positions,
            "weights": weights,
            "uv_values": uv_values,
            "triangles": len(obj.data.loop_triangles),
            "polygon_count": len(obj.data.polygons),
        }
    images = blender_io.image_inventory()
    if any(
        not image["packed"] or not image["has_data"] or min(image["width"], image["height"]) <= 1
        for image in images
    ):
        raise ValueError("Native provider textures are missing or not embedded")
    return (bones, meshes, images)


def _retain_native_weights(
    doc: Record, binary: bytearray, native_meshes: dict[str, Record]
) -> list[Record]:
    records = surfaces(doc, binary)
    world = glb.worlds(doc)
    checks = []
    for record in records:
        node = doc["nodes"][record["node_index"]]
        if node.get("name") not in native_meshes or record["skin"] is None:
            raise ValueError("Converted rig does not match the native weighted mesh")
        native = native_meshes[node["name"]]
        attrs = doc["meshes"][node["mesh"]]["primitives"][record["primitive_index"]]["attributes"]
        positions = rig_metrics.surface_points(record, world)
        uv = glb.accessor(doc, binary, attrs["TEXCOORD_0"])
        joint_names = [doc["nodes"][index].get("name", "") for index in record["skin"]["joints"]]
        lookup = {name: index for index, name in enumerate(joint_names)}
        joints = np.zeros((len(positions), 4), dtype=np.uint16)
        weights = np.zeros((len(positions), 4), dtype=np.float32)
        loss, errors = ([], [])
        for vertex, candidates in enumerate(_candidates(native["positions"], positions, 1e-05)):
            candidates = [
                i
                for i in candidates
                if any(
                    np.max(np.abs(np.asarray(value) - uv[vertex])) <= 1e-06
                    for value in native["uv_values"][i]
                )
            ]
            if not candidates:
                raise ValueError("Native weight preservation cannot match exported position and UV")
            old = {
                joint_names[int(j)]: float(w)
                for j, w in zip(record["joints"][vertex], record["weights"][vertex], strict=True)
                if w > 0
            }

            def error(index: int, native: Record = native, old: dict[str, float] = old) -> float:
                row = native["weights"][index]
                return float(
                    sum(abs(row.get(key, 0) - old.get(key, 0)) for key in set(row) | set(old))
                )

            match = min(candidates, key=error)
            row = native["weights"][match]
            if any(
                sum(
                    abs(native["weights"][i].get(k, 0) - row.get(k, 0))
                    for k in set(row) | set(native["weights"][i])
                )
                > 1e-05
                for i in candidates
            ):
                raise ValueError("Ambiguous coincident native weight assignment")
            errors.append(float(np.linalg.norm(positions[vertex] - native["positions"][match])))
            loss.append(error(match))
            for slot, (name, value) in enumerate(sorted(row.items(), key=lambda item: -item[1])):
                if name not in lookup:
                    raise ValueError("Native weighted joint disappeared during export")
                joints[vertex, slot], weights[vertex, slot] = (lookup[name], value)
        attrs["JOINTS_0"] = glb.append_accessor(doc, binary, joints, 5123, "VEC4")
        attrs["WEIGHTS_0"] = glb.append_accessor(doc, binary, weights, 5126, "VEC4")
        checks.append(
            {
                "mesh": node["name"],
                "native_vertices": len(native["positions"]),
                "exported_vertices": len(positions),
                "position_error": rig_metrics.distribution(errors),
                "exporter_weight_loss_before_preservation": rig_metrics.distribution(loss),
                "vertices_with_weight_loss_above_1e_5": int((np.asarray(loss) > 1e-05).sum()),
                "operation": (
                    "actual native normalized weights copied at verified position a"
                    "nd UV; no new weights authored"
                ),
            }
        )
    return checks


def normalize_asset(source: Path, output_dir: Path) -> tuple[Path, Record]:
    """Return an embedded provider rig control; caller supplies a fresh staging root."""
    source, output_dir = (Path(source), Path(output_dir))
    native_format = detect_format(source)
    source_sha = glb.sha(source)
    target = output_dir / "raw-normalized.glb"
    if target.exists():
        raise ValueError("Normalized provider control already exists")
    if native_format == "glb":
        doc, binary = glb.read_glb(source)
        if not doc.get("skins"):
            raise ValueError("Provider result has no rig")
        surfaces(doc, binary)
        shutil.copyfile(source, target)
        return (
            target,
            {
                "native_format": native_format,
                "source_sha256": source_sha,
                "normalized_sha256": glb.sha(target),
                "source_unchanged": True,
                "operation": "validated byte-exact GLB control",
                "byte_exact": True,
            },
        )
    bpy = importlib.import_module("bpy")
    from _character_tools.worker import blender_io

    imported = source
    if source.suffix.lower() != ".fbx":
        imported = output_dir / "native-format-input.fbx"
        shutil.copyfile(source, imported)
    objects, _ = blender_io.import_source(imported)
    if bpy.data.actions:
        raise ValueError("Animated native FBX requires an explicit clip-preservation adapter")
    before_bones, before_meshes, images = _native_capture(objects)
    for image in bpy.data.images:
        image.filepath_raw = ""
        if image.has_data:
            image.pack()
    bpy.context.preferences.filepaths.save_version = 0
    bpy.ops.wm.save_as_mainfile(filepath=str(output_dir / "native-import.blend"))
    intermediate = output_dir / "native-export.glb"
    bpy.ops.export_scene.gltf(
        filepath=str(intermediate),
        export_format="GLB",
        export_animations=False,
        export_materials="EXPORT",
        export_yup=True,
        export_def_bones=False,
    )
    doc, binary = glb.read_glb(intermediate)
    preservation = _retain_native_weights(doc, binary, before_meshes)
    glb.save_glb(target, doc, binary)
    objects, _ = blender_io.import_source(target)
    after_bones, after_meshes, _ = _native_capture(objects)
    if set(before_bones) != set(after_bones) or set(before_meshes) != set(after_meshes):
        raise ValueError("Native rig conversion changed the bone or mesh names")
    joint_errors = []
    for name, before in before_bones.items():
        if before["parent"] != after_bones[name]["parent"]:
            raise ValueError("Native rig conversion changed joint hierarchy")
        joint_errors.append(
            float(np.linalg.norm(np.asarray(before["head"]) - after_bones[name]["head"]))
        )
    if max(joint_errors) > 1e-05:
        raise ValueError("Native rig conversion moved rest joints beyond 0.01mm")
    verified = []
    for name, after in after_meshes.items():
        before = before_meshes[name]
        errors = []
        if before["triangles"] != after["triangles"]:
            raise ValueError("Native rig conversion changed triangle count")
        for vertex, candidates in enumerate(
            _candidates(before["positions"], after["positions"], 1e-05)
        ):
            if not candidates:
                raise ValueError("Reimported provider surface has no native match")
            row = after["weights"][vertex]
            errors.append(
                min(
                    sum(
                        abs(row.get(key, 0) - before["weights"][i].get(key, 0))
                        for key in set(row) | set(before["weights"][i])
                    )
                    for i in candidates
                )
            )
        if max(errors) > 1e-05:
            raise ValueError("Reimported provider weights differ from the native rig")
        verified.append({"mesh": name, "max_normalized_weight_l1_error": max(errors)})
    if glb.sha(source) != source_sha:
        raise ValueError("Native provider source changed")
    return (
        target,
        {
            "native_format": native_format,
            "source_sha256": source_sha,
            "normalized_sha256": glb.sha(target),
            "source_unchanged": True,
            "byte_exact": False,
            "operation": "embedded-only Blender FBX to GLB; actual native weights retained",
            "native_joint_count": len(before_bones),
            "native_bones": before_bones,
            "max_joint_position_error": max(joint_errors),
            "weight_preservation": preservation,
            "weight_reimport_checks": verified,
            "native_images": images,
            "limitations": [
                ("FBX materials are converted by Blender; native source remains authoritative."),
                "No generated rig or weights are substituted.",
            ],
        },
    )


def _motion_source(
    path: Path, options: Record, needed: Iterable[str]
) -> tuple[dict[str, FloatArray], list[dict[str, FloatArray]], FloatArray, Record]:
    """Sample only the declared motion file; no bundled or character-specific default."""
    fps = options["fps"]
    if detect_format(path) == "glb":
        doc, binary = glb.read_glb(path)
        mapping = semantic_mapping(
            doc,
            preset=options.get("source_mapping_preset"),
            mapping=options.get("source_mapping"),
            required=needed,
        )
        clips = doc.get("animations", [])
        selected = (
            [clip for clip in clips if clip.get("name") == options["source_clip"]]
            if "source_clip" in options
            else clips
        )
        if len(selected) != 1:
            raise ValueError("Motion input needs one unambiguous exact source clip")
        tracks = rig_metrics.decode_animation(doc, binary, selected[0])
        begin = min(float(track["times"][0]) for track in tracks)
        end = max(float(track["times"][-1]) for track in tracks)
        duration = end - begin
        if not 0 < duration <= 120:
            raise ValueError("Declared motion duration must be positive and at most 120 seconds")
        times = np.linspace(0, duration, round(duration * fps) + 1)
        rest_all = glb.worlds(doc)
        rest = {role: rest_all[node] for role, node in mapping.items()}
        poses = []
        for at in times:
            current = rig_metrics.sample_worlds(doc, tracks, begin + at)
            poses.append({role: current[node] for role, node in mapping.items()})
        return (
            rest,
            poses,
            times,
            {"source_clip": selected[0].get("name"), "source_format": "glb"},
        )
    bpy = importlib.import_module("bpy")
    swapped_attribute = importlib.import_module(__package__ + ".blender_io").swapped_attribute
    image_utils = importlib.import_module("bpy_extras.image_utils")
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.context.scene.render.fps = fps
    bpy.context.scene.render.fps_base = 1

    def placeholder(filepath: str, **_kwargs: Any) -> Any:
        image = bpy.data.images.new(
            "motion_embedded_" + hashlib.sha256(str(filepath).encode()).hexdigest()[:16],
            width=1,
            height=1,
        )
        image.source = "FILE"
        image.filepath_raw = ""
        return image

    with swapped_attribute(image_utils, "load_image", placeholder):
        bpy.ops.import_scene.fbx(filepath=str(path), use_image_search=False, use_custom_props=False)
    arms = [obj for obj in bpy.context.scene.objects if obj.type == "ARMATURE"]
    if len(arms) != 1:
        raise ValueError("Motion source needs one armature")
    armature = arms[0]
    names = [bone.name for bone in armature.data.bones]
    fake = {
        "nodes": [{"name": name} for name in names],
        "skins": [{"joints": list(range(len(names)))}],
    }
    mapping = semantic_mapping(
        fake,
        preset=options.get("source_mapping_preset"),
        mapping=options.get("source_mapping"),
        required=needed,
    )
    actions = list(bpy.data.actions)
    selected = (
        [action for action in actions if action.name == options["source_clip"]]
        if "source_clip" in options
        else actions
    )
    if (
        len(selected) != 1
        or not armature.animation_data
        or armature.animation_data.action != selected[0]
    ):
        raise ValueError("Native motion needs one selected active armature action")
    action = selected[0]
    begin, end = map(float, action.frame_range)
    source_fps = bpy.context.scene.render.fps / bpy.context.scene.render.fps_base
    duration = (end - begin) / source_fps
    if not 0 < duration <= 120:
        raise ValueError("Declared motion duration must be positive and at most 120 seconds")
    times = np.linspace(0, duration, round(duration * fps) + 1)

    def matrix(name: str, posed: bool) -> FloatArray:
        local = (
            armature.pose.bones[name].matrix if posed else armature.data.bones[name].matrix_local
        )
        return TO_PUBLIC @ np.asarray(armature.matrix_world) @ np.asarray(local)

    rest = {role: matrix(names[index], False) for role, index in mapping.items()}
    poses = []
    for at in times:
        frame = begin + at * source_fps
        bpy.context.scene.frame_set(math.floor(frame), subframe=frame % 1)
        bpy.context.view_layer.update()
        poses.append({role: matrix(names[index], True) for role, index in mapping.items()})
    return (
        rest,
        poses,
        times,
        {"source_clip": action.name, "source_format": "fbx", "source_fps": source_fps},
    )


def _anatomical_frame(direction: FloatArray, forward: FloatArray) -> FloatArray:
    y = _unit(direction)
    z = forward - y * np.dot(forward, y)
    if np.linalg.norm(z) < 0.1:
        z = np.array([0.0, 1.0, 0.0]) - y * y[1]
    z = _unit(z)
    x = _unit(np.cross(y, z))
    return np.column_stack((x, y, _unit(np.cross(x, y))))


def append_declared_motion(
    doc: Record, binary: bytearray, mapping: dict[str, int], source_path: Path, options: Record
) -> Record:
    required = {
        "pelvis",
        "head",
        *[
            f"{side}_{role}"
            for side in ("left", "right")
            for role in ("upper_arm", "forearm", "hand", "upper_leg", "lower_leg", "foot")
        ],
    }
    if not required <= mapping.keys():
        raise ValueError("Declared humanoid retarget lacks required target semantics")
    source_rest, poses, times, source_report = _motion_source(source_path, options, required)
    mapping = {role: node for role, node in mapping.items() if role in source_rest}
    target_world = glb.worlds(doc)
    target_frame = body_frame(target_world, mapping)
    source_roles = list(source_rest)
    source_frame = body_frame(
        [source_rest[role] for role in source_roles],
        {role: source_roles.index(role) for role in source_roles},
    )
    alignment = target_frame @ source_frame.T
    offsets = {}
    for role, node in mapping.items():
        target_rotation, source_rotation = (
            _rotation(target_world[node]),
            _rotation(source_rest[role]),
        )
        child = CHILD.get(role)
        if child in mapping and role != "pelvis":
            sf = _anatomical_frame(
                source_rest[child][:3, 3] - source_rest[role][:3, 3], source_frame[:, 2]
            )
            tf = _anatomical_frame(
                target_world[mapping[child]][:3, 3] - target_world[node][:3, 3], target_frame[:, 2]
            )
            offsets[role] = source_rotation.T @ sf @ tf.T @ target_rotation
        else:
            offsets[role] = source_rotation.T @ alignment.T @ target_rotation

    def leg_length(values: dict[str, FloatArray]) -> float:
        return np.mean(
            [
                np.linalg.norm(values[f"{s}_upper_leg"] - values[f"{s}_lower_leg"])
                + np.linalg.norm(values[f"{s}_lower_leg"] - values[f"{s}_foot"])
                for s in ("left", "right")
            ]
        )

    source_leg = leg_length({role: value[:3, 3] for role, value in source_rest.items()})
    target_leg = leg_length({role: target_world[node][:3, 3] for role, node in mapping.items()})
    if min(source_leg, target_leg) <= 1e-08:
        raise ValueError("Degenerate retarget leg measurements")
    scale = float(target_leg / source_leg)
    records = surfaces(doc, binary)
    all_points = np.vstack([rig_metrics.surface_points(record, target_world) for record in records])
    floor, height = (float(all_points[:, 1].min()), float(np.ptp(all_points[:, 1])))
    sole_records = []
    feet = {
        node for role, node in mapping.items() if role.endswith("_foot") or role.endswith("_toes")
    }
    for record in records:
        if record["skin"] is None:
            continue
        points = rig_metrics.surface_points(record, target_world)
        support = (
            record["weights"] * np.isin(record["skin"]["joints"][record["joints"]], list(feet))
        ).sum(1)
        mask = (points[:, 1] < floor + height * 0.08) & (support > 0.35)
        if mask.any():
            small = copy.copy(record)
            for key in ("positions", "joints", "weights"):
                small[key] = record[key][mask]
            sole_records.append(small)
    if not sole_records:
        raise ValueError("Actual provider rig has no supported sole vertices for motion grounding")
    parents = {
        child: index
        for index, node in enumerate(doc["nodes"])
        for child in node.get("children", [])
    }
    mapped_nodes = {node: role for role, node in mapping.items()}
    rotations: dict[int, list[FloatArray]] = {node: [] for node in mapped_nodes}
    translations = []
    hip = mapping["pelvis"]
    parent_world = target_world[parents[hip]] if hip in parents else np.eye(4)
    parent_inverse = np.linalg.inv(parent_world)
    source_feet = [
        min(pose.get(f"{side}_toes", pose[f"{side}_foot"])[1, 3] for side in ("left", "right"))
        for pose in poses
    ]
    source_floor = float(np.percentile(source_feet, 10))
    source_initial_hip = poses[0]["pelvis"][:3, 3].copy()
    corrections = []
    for index, pose in enumerate(poses):
        desired: dict[int, FloatArray] = {
            mapping[role]: alignment @ _rotation(pose[role]) @ offsets[role] for role in mapping
        }
        current_rotations: dict[int, FloatArray] = {}
        pose_doc: Record = {"nodes": copy.deepcopy(doc["nodes"])}

        def visit(
            node: int,
            current_rotations: dict[int, FloatArray] = current_rotations,
            desired: dict[int, FloatArray] = desired,
            pose_doc: Record = pose_doc,
        ) -> FloatArray:
            if node not in current_rotations:
                parent_rotation = visit(parents[node]) if node in parents else np.eye(3)
                if node in desired:
                    current_rotations[node] = desired[node]
                    quat = _quaternion(parent_rotation.T @ desired[node])
                    rotations[node].append(quat)
                    pose_doc["nodes"][node]["rotation"] = quat.tolist()
                else:
                    current_rotations[node] = parent_rotation @ _rotation(
                        glb.local(doc["nodes"][node])
                    )
            return current_rotations[node]

        for node in range(len(doc["nodes"])):
            visit(node)
        displacement = alignment @ (pose["pelvis"][:3, 3] - source_initial_hip) * scale
        hip_world = target_world[hip][:3, 3] + displacement
        local_hip = (parent_inverse @ np.r_[hip_world, 1])[:3]
        pose_doc["nodes"][hip]["translation"] = local_hip.tolist()
        current = glb.worlds(pose_doc)
        minimum = min(
            float(rig_metrics.surface_points(record, current)[:, 1].min())
            for record in sole_records
        )
        source_air = max(0.0, (source_feet[index] - source_floor) * scale - target_leg * 0.02)
        correction = floor + source_air - minimum
        local_hip += parent_inverse[:3, :3] @ np.array([0.0, correction, 0.0])
        translations.append(local_hip)
        corrections.append(correction)
    animation = _append_clip(doc, binary, options["clip"], times, rotations, {hip: translations})
    duration = float(times[-1])
    return {
        "name": options["clip"],
        "duration_seconds": duration,
        "fps": options["fps"],
        "sample_count": len(times),
        "review_times_seconds": np.linspace(0, duration, 13).tolist(),
        "author": "declared external motion adapted locally",
        "motion_source_sha256": glb.sha(source_path),
        "source": source_report,
        "anatomical_alignment": alignment.tolist(),
        "leg_length_scale": scale,
        "ground_correction": rig_metrics.distribution(corrections),
        "channel_count": len(animation["channels"]),
        "limitations": [
            "No horizontal foot locking",
            "No facial, finger or secondary hair animation added",
            ("Original source trajectory retained; endpoint continuity is not invented"),
        ],
    }


def build_provider_rig(
    source: Path,
    options: Record,
    output_dir: Path,
    *,
    preservation_source: Path | None = None,
    motion_source: Path | None = None,
) -> Record:
    """Process a real provider rig in a caller-confined fresh staging directory."""
    validate_options(options)
    source, output_dir = (Path(source), Path(output_dir))
    normalized, normalization = normalize_asset(source, output_dir)
    doc, binary = glb.read_glb(normalized)
    mapping = semantic_mapping(
        doc,
        preset=options.get("mapping_preset"),
        mapping=options.get("semantic_mapping"),
        required=options["required_joints"],
    )
    raw_inventory = inventory(doc, binary, mapping, options.get("part_roles"))
    contract.write_json(output_dir / "raw-external-rig.json", raw_inventory)
    preservation, restoration = (None, None)
    if "preservation" in options:
        if preservation_source is None:
            raise ValueError("Declared preservation source must be resolved by the caller")
        if glb.sha(preservation_source) != options["preservation"]["source"]["sha256"]:
            raise ValueError("Preservation source hash changed")
        sd, sb = glb.read_glb(preservation_source)
        preservation, match = audit_preservation(sd, sb, doc, binary, options["preservation"])
        if options["preservation"]["mode"] == "restore_normals":
            restoration = restore_verified_normals(doc, binary, match, preservation)
            glb.save_glb(output_dir / "normal-restored.glb", doc, binary)
        contract.write_json(output_dir / "preservation.json", preservation)
    material_changes = material_finish(doc, options.get("material_policy", "preserve"))
    before, original = (copy.deepcopy(doc), bytes(binary))
    clips = (
        append_diagnostics(doc, binary, mapping, options.get("fps", 24))
        if options.get("diagnostics", True)
        else []
    )
    if "motion" in options:
        if motion_source is None or glb.sha(motion_source) != options["motion"]["source"]["sha256"]:
            raise ValueError("Declared motion source is missing or its hash changed")
        clips.append(append_declared_motion(doc, binary, mapping, motion_source, options["motion"]))
    if (
        any(doc.get(key) != before.get(key) for key in PROTECTED)
        or bytes(binary[: len(original)]) != original
    ):
        raise ValueError("Motion adapter changed actual provider rig/appearance data")
    if doc.get("animations", [])[: len(before.get("animations", []))] != before.get(
        "animations", []
    ):
        raise ValueError("Motion adapter changed an existing provider animation")
    output_transform = (
        normalize_output_space(doc, binary, options["target_height"])
        if "target_height" in options
        else None
    )
    animated = output_dir / "animated.glb"
    glb.save_glb(animated, doc, binary)
    exported_doc, exported_binary = glb.read_glb(animated)
    final_inventory = inventory(exported_doc, exported_binary, mapping, options.get("part_roles"))
    if output_transform is not None:
        _output_postcondition(final_inventory["bounds"], options["target_height"])
        output_transform["after_bounds"] = final_inventory["bounds"]
        output_transform["height_after"] = final_inventory["bounds"]["dimensions"][1]
        output_transform["ground_after"] = final_inventory["bounds"]["min"][1]
        output_transform["verified_export_sha256"] = glb.sha(animated)
    contract.write_json(output_dir / "external-rig.json", final_inventory)
    metrics = clip_metrics(exported_doc, exported_binary, clips)
    contract.write_json(
        output_dir / "diagnostics.json",
        {
            "schema_version": 1,
            "clips": metrics,
            "semantic_acceptance": False,
            "rig_author": "external provider",
        },
    )
    if glb.sha(source) != normalization["source_sha256"]:
        raise ValueError("Provider source changed during processing")
    report = {
        "schema_version": 1,
        "operation": "provider_rig",
        "status": "completed_unreviewed",
        "code_version": CODE_VERSION,
        "source_sha256": normalization["source_sha256"],
        "native_format": normalization["native_format"],
        "source_unchanged": True,
        "provider_calls": 0,
        "coordinate_system": contract.COORDINATES,
        "output": {"path": "animated.glb", "sha256": glb.sha(animated)},
        "raw_normalized": {"path": "raw-normalized.glb", "sha256": glb.sha(normalized)},
        "normalization": normalization,
        "external_rig": final_inventory,
        "raw_external_rig": {
            "path": "raw-external-rig.json",
            "sha256": contract.digest(output_dir / "raw-external-rig.json"),
        },
        "geometry": {"bounds": final_inventory["bounds"], "meshes": final_inventory["meshes"]},
        "output_transform": output_transform,
        "clips": clips,
        "preservation": preservation,
        "normal_restoration": restoration,
        "material_policy": options.get("material_policy", "preserve"),
        "material_changes": material_changes,
        "motion_preservation": {
            "provider_fields_exact": [
                field
                for field in PROTECTED
                if output_transform is None or field not in {"nodes", "scenes"}
            ],
            "original_provider_nodes_exact": True,
            "output_transform_excluded_fields": ["nodes", "scenes"] if output_transform else [],
            "original_binary_prefix_exact": True,
            "existing_provider_animations_unchanged": True,
        },
        "visual_review": "required_separately",
        "limitations": [
            (
                "No replacement body rig, skin weights, grouped fingers, or ind"
                "ependent hair rig authored."
            ),
            ("Finite numerical samples do not establish anatomical or visual acceptance."),
        ],
    }
    contract.write_json(output_dir / "report.json", report)
    return report
