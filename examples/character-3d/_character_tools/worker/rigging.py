"""Blender binding and sampled motion from explicit, anatomy-neutral rig plans.

The caller confines source/output roots and atomically admits the complete output
directory. This module does no provider work and makes no semantic quality claim.
"""

from __future__ import annotations

import copy
import hashlib
import json
import math
from collections.abc import Sequence
from importlib import import_module
from pathlib import Path
from typing import Any

import numpy as np
from numpy.typing import ArrayLike, NDArray

from _character_tools.worker import geometry, rig_contract
from _character_tools.worker import rig_glb as glb
from _character_tools.worker.contract import write_json

bmesh = import_module("bmesh")
bpy = import_module("bpy")
Matrix = import_module("mathutils").Matrix
Quaternion = import_module("mathutils").Quaternion
Vector = import_module("mathutils").Vector
KDTree = import_module("mathutils.kdtree").KDTree


def to_blender(point: Sequence[float] | NDArray[Any]) -> Any:
    return Vector((point[0], -point[2], point[1]))


def frame(head: ArrayLike, tail: ArrayLike, forward: ArrayLike) -> Any:
    y = (Vector(tail) - Vector(head)).normalized()
    z = Vector(forward) - y * y.dot(Vector(forward))
    if z.length < 1e-06:
        fallback = min(
            (Vector(axis) for axis in ((1, 0, 0), (0, 1, 0), (0, 0, 1))),
            key=lambda axis: abs(axis.dot(y)),
        )
        z = fallback - y * y.dot(fallback)
    z.normalize()
    result = Matrix((y.cross(z).normalized(), y, z)).transposed().to_4x4()
    result.translation = Vector(head)
    return result


def build_armature(bones: dict[str, dict[str, Any]]) -> Any:
    data = bpy.data.armatures.new("MeasuredSkeleton")
    rig = bpy.data.objects.new("MeasuredRig", data)
    bpy.context.scene.collection.objects.link(rig)
    bpy.context.view_layer.objects.active = rig
    rig.select_set(True)
    bpy.ops.object.mode_set(mode="EDIT")
    for name, spec in bones.items():
        bone = data.edit_bones.new(name)
        bone.head, bone.tail = (to_blender(spec["head"]), to_blender(spec["tail"]))
        if spec["parent"]:
            bone.parent = data.edit_bones[spec["parent"]]
    bpy.ops.object.mode_set(mode="OBJECT")
    return rig


def compact_weights(dense: NDArray[Any]) -> tuple[NDArray[Any], NDArray[Any]]:
    if not np.isfinite(dense).all() or np.any(dense < -1e-08):
        raise ValueError("Binding returned nonfinite or negative influences")
    dense = np.maximum(dense, 0)
    selected = np.argsort(dense, axis=1)[:, -min(4, dense.shape[1]) :][:, ::-1]
    values = np.take_along_axis(dense, selected, axis=1)
    if np.any(values.sum(1) <= 1e-12):
        raise ValueError("Binding left vertices without an influence")
    values /= values.sum(1, keepdims=True)
    pad = 4 - values.shape[1]
    return (
        np.pad(selected, ((0, 0), (0, pad))).astype(np.uint16),
        np.pad(values, ((0, 0), (0, pad))).astype(np.float32),
    )


def fallback_weights(
    points: NDArray[Any], bones: dict[str, dict[str, Any]], allowed: Sequence[str]
) -> NDArray[Any]:
    """Segment-distance field restricted to caller-labeled eligible bones."""
    names = list(bones)
    distances, lengths = ([], [])
    for name in allowed:
        start, end = (np.asarray(bones[name]["head"]), np.asarray(bones[name]["tail"]))
        axis = end - start
        length = float(np.linalg.norm(axis))
        along = np.clip((points - start) @ axis / (length * length), 0, 1)
        distances.append(np.linalg.norm(points - start - along[:, None] * axis, axis=1))
        lengths.append(length)
    radius = max(float(np.median(lengths)) * 0.12, 1e-10)
    values = 1 / (np.column_stack(distances) ** 2 + radius**2) ** 2
    values /= values.sum(1, keepdims=True)
    dense = np.zeros((len(points), len(bones)))
    dense[:, [names.index(name) for name in allowed]] = values
    return dense


def heat_weights(
    points: NDArray[Any],
    triangles: NDArray[Any],
    rig: Any,
    bones: dict[str, dict[str, Any]],
    height: float,
    allowed: Sequence[str],
) -> tuple[NDArray[Any], dict[str, Any]]:
    for bone in rig.data.bones:
        bone.use_deform = bone.name in allowed
    mesh = bpy.data.meshes.new("TemporaryHeatProxy")
    mesh.from_pydata([to_blender(point) for point in points], [], triangles.tolist())
    mesh.update()
    bm = bmesh.new()
    bm.from_mesh(mesh)
    bmesh.ops.remove_doubles(bm, verts=list(bm.verts), dist=height * 1e-06)
    bm.to_mesh(mesh)
    bm.free()
    proxy = bpy.data.objects.new("TemporaryHeatProxy", mesh)
    bpy.context.scene.collection.objects.link(proxy)
    bpy.ops.object.select_all(action="DESELECT")
    proxy.select_set(True)
    rig.select_set(True)
    bpy.context.view_layer.objects.active = rig
    failure = None
    try:
        bpy.ops.object.parent_set(type="ARMATURE_AUTO")
    except RuntimeError:
        failure = "bone_heat_runtime_error"
    names = list(bones)
    groups = {
        group.index: names.index(group.name)
        for group in proxy.vertex_groups
        if group.name in allowed
    }
    dense = np.zeros((len(mesh.vertices), len(bones)))
    tree = KDTree(len(mesh.vertices))
    for vertex in mesh.vertices:
        tree.insert(proxy.matrix_world @ vertex.co, vertex.index)
        for group in vertex.groups:
            if group.group in groups:
                dense[vertex.index, groups[group.group]] = max(group.weight, 0)
    tree.balance()
    matches = [tree.find(to_blender(point)) for point in points]
    maximum = max(match[2] for match in matches)
    if maximum > height * 5e-06:
        raise ValueError("Heat proxy no longer corresponds to source positions")
    dense = dense[[match[1] for match in matches]]
    missing = dense.sum(1) < 1e-08
    if np.any(missing):
        dense[missing] = fallback_weights(points[missing], bones, allowed)
    dense /= np.maximum(dense.sum(1, keepdims=True), 1e-20)
    report = {
        "method": "bone_heat_proxy_with_explicit_segment_fallback",
        "heat_failure": failure,
        "welded_proxy_vertices": len(mesh.vertices),
        "fallback_vertex_records": int(missing.sum()),
        "maximum_proxy_distance": maximum,
        "eligible_bones": allowed,
    }
    bpy.data.objects.remove(proxy, do_unlink=True)
    bpy.data.meshes.remove(mesh)
    for bone in rig.data.bones:
        bone.use_deform = True
    return (dense, report)


def chain_weights(
    points: NDArray[Any], bones: dict[str, dict[str, Any]], chain: Sequence[str]
) -> tuple[NDArray[Any], dict[str, Any]]:
    starts = np.asarray([bones[name]["head"] for name in chain])
    ends = np.asarray([bones[name]["tail"] for name in chain])
    lengths = np.linalg.norm(ends - starts, axis=1)
    arcs = np.r_[0, np.cumsum(lengths)]
    distances, parameters = ([], [])
    for index, (start, end) in enumerate(zip(starts, ends, strict=True)):
        axis = end - start
        t = np.clip((points - start) @ axis / np.dot(axis, axis), 0, 1)
        distances.append(np.linalg.norm(points - start - t[:, None] * axis, axis=1))
        parameters.append(arcs[index] + t * lengths[index])
    closest = np.argmin(np.column_stack(distances), axis=1)
    along = np.column_stack(parameters)[np.arange(len(points)), closest]
    centers = (arcs[:-1] + arcs[1:]) * 0.5
    right = np.clip(np.searchsorted(centers, along), 1, len(chain) - 1)
    left = right - 1
    fraction = np.clip((along - centers[left]) / (centers[right] - centers[left]), 0, 1)
    fraction = fraction * fraction * (3 - 2 * fraction)
    dense = np.zeros((len(points), len(bones)))
    names = list(bones)
    dense[np.arange(len(points)), [names.index(chain[index]) for index in left]] = 1 - fraction
    dense[np.arange(len(points)), [names.index(chain[index]) for index in right]] += fraction
    return (
        dense,
        {
            "method": "measured_chain_projection",
            "chain": chain,
            "centerline_length": float(arcs[-1]),
            "maximum_centerline_distance": float(np.column_stack(distances).min(1).max()),
            "limitation": (
                "Non-self-crossing measured centerline assumed; visual bend review required."
            ),
        },
    )


def apply_regions(
    dense: NDArray[Any],
    points: NDArray[Any],
    triangles: NDArray[Any],
    bones: dict[str, dict[str, Any]],
    regions: Sequence[dict[str, Any]],
    height: float,
) -> list[dict[str, Any]]:
    reports, membership = ([], None)
    for region in regions:
        lower, upper = np.asarray(region["bounds"])
        mask = np.all((points >= lower) & (points <= upper), axis=1)
        if region.get("whole_components"):
            if membership is None:
                _, membership = geometry.topology(points, triangles, height * 1e-06)
            eligible = [
                index
                for index in np.unique(membership)
                if index >= 0 and np.all(mask[membership == index])
            ]
            mask = np.isin(membership, eligible)
        selected = points[mask]
        factor = np.ones(len(selected))
        blend_axis = region.get("blend_axis")
        if blend_axis:
            factor = np.clip(
                (selected[:, blend_axis["axis"]] - blend_axis["start"])
                / (blend_axis["end"] - blend_axis["start"]),
                0,
                1,
            )
            factor = factor * factor * (3 - 2 * factor)
            if blend_axis["reverse"]:
                factor = 1 - factor
        fade_width = region.get("fade_width", 0)
        if fade_width > 0:
            axes = [axis for axis in range(3) if not blend_axis or axis != blend_axis["axis"]]
            margin = np.minimum(
                selected[:, axes] - lower[axes], upper[axes] - selected[:, axes]
            ).min(1)
            fade = np.clip(margin / fade_width, 0, 1)
            factor *= fade * fade * (3 - 2 * fade)
        if mask.any():
            replacement = fallback_weights(selected, bones, region["bones"])
            dense[mask] = dense[mask] * (1 - factor[:, None]) + replacement * factor[:, None]
        reports.append(
            {
                "name": region["name"],
                "selected_vertex_records": int(mask.sum()),
                "changed_weight_records": int(np.sum(factor > 0)),
                "bones": region["bones"],
                "whole_components": region.get("whole_components", False),
            }
        )
    return reports


def append_skeleton(
    doc: dict[str, Any], bones: dict[str, dict[str, Any]]
) -> tuple[dict[str, int], dict[str, Any]]:
    existing_names = {node.get("name") for node in doc["nodes"]}
    root = len(doc["nodes"])
    root_name = f"RigRoot_{root}"
    while root_name in existing_names or root_name in bones:
        root_name += "_"
    doc["nodes"].append({"name": root_name, "children": []})
    doc["scenes"][doc.get("scene", 0)].setdefault("nodes", []).append(root)
    frames = {
        name: frame(
            spec["head"],
            spec["tail"],
            spec.get("forward_direction", spec.get("palm_direction", [0, 0, 1])),
        )
        for name, spec in bones.items()
    }
    node_map = {}
    for name, spec in bones.items():
        parent = spec["parent"]
        matrix = frames[parent].inverted() @ frames[name] if parent else frames[name]
        location, rotation, scale = matrix.decompose()
        node_map[name] = len(doc["nodes"])
        doc["nodes"].append(
            {
                "name": name,
                "translation": list(location),
                "rotation": [rotation.x, rotation.y, rotation.z, rotation.w],
                "scale": list(scale),
            }
        )
        doc["nodes"][node_map[parent] if parent else root].setdefault("children", []).append(
            node_map[name]
        )
    return (node_map, frames)


def material_finish(
    doc: dict[str, Any], roles: dict[int, str], policy: dict[str, Any]
) -> list[dict[str, Any]]:
    reports, cache = ([], {})
    for node_index, role in roles.items():
        choice = policy.get("by_role", {}).get(role, policy["default"])
        if choice == "preserve":
            continue
        for primitive in doc["meshes"][doc["nodes"][node_index]["mesh"]]["primitives"]:
            source = primitive.get("material")
            key = (source, choice)
            if key not in cache:
                before = (
                    copy.deepcopy(doc.get("materials", [])[source]) if source is not None else {}
                )
                material = copy.deepcopy(before)
                pbr = material.setdefault("pbrMetallicRoughness", {})
                pbr.update(metallicFactor=0, roughnessFactor=1)
                pbr.pop("metallicRoughnessTexture", None)
                extensions = material.setdefault("extensions", {})
                for name in (
                    "KHR_materials_unlit",
                    "KHR_materials_clearcoat",
                    "KHR_materials_sheen",
                    "KHR_materials_specular",
                ):
                    extensions.pop(name, None)
                if choice == "unlit":
                    extensions["KHR_materials_unlit"] = {}
                else:
                    extensions["KHR_materials_specular"] = {"specularFactor": 0.08}
                if "normalTexture" in material:
                    material["normalTexture"]["scale"] = min(
                        material["normalTexture"].get("scale", 1), 0.15
                    )
                index = len(doc.setdefault("materials", []))
                material["name"] = f"rig_material_{index}_{choice}"
                doc["materials"].append(material)
                cache[key] = index
                reports.append(
                    {
                        "source_material": source,
                        "output_material": index,
                        "policy": choice,
                        "before": before,
                        "after": material,
                    }
                )
            primitive["material"] = cache[key]
    used = set(doc.get("extensionsUsed", []))
    for material in doc.get("materials", []):
        used.update(material.get("extensions", {}))
    if used:
        doc["extensionsUsed"] = sorted(used)
    return reports


def track_axis(track: dict[str, Any], bones: dict[str, dict[str, Any]]) -> Any:
    if "axis_world" in track:
        return Vector(track["axis_world"]).normalized()
    bone = bones[track["bone"]]
    return (
        (Vector(bone["tail"]) - Vector(bone["head"]))
        .cross(Vector(bone["palm_direction"]))
        .normalized()
    )


def posed_points(record: dict[str, Any], joint_world: Sequence[NDArray[Any]]) -> NDArray[Any]:
    matrices = np.asarray(joint_world) @ record["inverse_bind"]
    homogeneous = np.column_stack((record["positions"], np.ones(len(record["positions"]))))
    result = np.zeros_like(homogeneous)
    for influence in range(4):
        matrix = matrices[record["joints"][:, influence]]
        result += (
            np.einsum("vij,vj->vi", matrix, homogeneous) * record["weights"][:, influence, None]
        )
    return result[:, :3]


def append_clips(
    doc: dict[str, Any],
    binary: bytearray,
    plan: dict[str, Any],
    node_map: dict[str, int],
    frames: dict[str, Any],
    support: Sequence[dict[str, Any]],
    height: float,
) -> list[dict[str, Any]]:
    bones, reports = (plan["bones"], [])
    rest_nodes = copy.deepcopy(doc["nodes"])
    for clip in plan["clips"]:
        duration = float(clip["duration_seconds"])
        times = np.linspace(0, duration, math.ceil(duration * clip["fps"]) + 1)
        names = list(dict.fromkeys(track["bone"] for track in clip["tracks"]))
        channels: dict[str, list[NDArray[Any]]] = {name: [] for name in names}
        compiled = [
            (track, track_axis(track, bones), np.asarray(track["keyframes"]))
            for track in clip["tracks"]
        ]
        translations, corrections = ([], [])
        ground = plan.get("grounding")
        for time in times:
            nodes = copy.deepcopy(rest_nodes)
            rotations = {
                name: Quaternion(
                    (nodes[node_map[name]]["rotation"][3], *nodes[node_map[name]]["rotation"][:3])
                )
                for name in names
            }
            for track, axis, keys in compiled:
                name = track["bone"]
                local_axis = frames[name].to_quaternion().inverted() @ axis
                rotations[name] @= Quaternion(
                    local_axis, math.radians(float(np.interp(time, keys[:, 0], keys[:, 1])))
                )
            for name, rotation in rotations.items():
                rotation.normalize()
                values = np.asarray([rotation.x, rotation.y, rotation.z, rotation.w])
                if channels[name] and np.dot(values, channels[name][-1]) < 0:
                    values = -values
                channels[name].append(values)
                nodes[node_map[name]]["rotation"] = values.tolist()
            if ground:
                world = glb.worlds({"nodes": nodes})
                joint_world = [world[node_map[name]] for name in bones]
                minimum = min(posed_points(record, joint_world)[:, 1].min() for record in support)
                correction = float(ground["ground_height"] - minimum)
                if abs(correction) > ground["max_correction_fraction"] * height:
                    raise ValueError("Ground correction exceeds the declared cap")
                value = np.asarray(
                    rest_nodes[node_map[ground["root_bone"]]]["translation"]
                ) + np.asarray([0, correction, 0])
                translations.append(value)
                corrections.append(correction)
        animation: dict[str, Any] = {"name": clip["name"], "samplers": [], "channels": []}
        timeline = glb.append_accessor(
            doc, binary, times[:, None], 5126, "SCALAR", {"min": [0], "max": [duration]}
        )

        def channel(
            name: str,
            path: str,
            values: ArrayLike,
            kind: str,
            animation: dict[str, Any] = animation,
            timeline: int = timeline,
        ) -> None:
            output = glb.append_accessor(doc, binary, np.asarray(values), 5126, kind)
            animation["channels"].append(
                {
                    "sampler": len(animation["samplers"]),
                    "target": {"node": node_map[name], "path": path},
                }
            )
            animation["samplers"].append(
                {"input": timeline, "output": output, "interpolation": "LINEAR"}
            )

        for name, samples in channels.items():
            channel(name, "rotation", samples, "VEC4")
        if ground:
            channel(ground["root_bone"], "translation", translations, "VEC3")
        doc.setdefault("animations", []).append(animation)
        reports.append(
            {
                "name": clip["name"],
                "duration_seconds": duration,
                "fps": clip["fps"],
                "samples": len(times),
                "rotation_bones": names,
                "ground_correction": glb.distribution(corrections) if corrections else None,
                "axis_semantics": (
                    "Rotation axes use the anatomical rest world frame; parent motion carries them."
                ),
            }
        )
    return reports


def build_rig(
    source_glb: str | Path, plan_json: dict[str, Any] | str | Path, out_dir: str | Path
) -> dict[str, Any]:
    """Build into a caller-confined staging directory; return the saved report."""
    source, out = (Path(source_glb), Path(out_dir))
    raw_plan = plan_json if isinstance(plan_json, dict) else json.loads(Path(plan_json).read_text())
    plan = rig_contract.validate_plan(raw_plan)
    source_sha = glb.sha(source)
    if source_sha != plan["source_sha256"]:
        raise ValueError("Rig plan does not bind the exact source hash")
    plan_sha = hashlib.sha256(
        json.dumps(raw_plan, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
    ).hexdigest()
    doc, binary = glb.read_glb(source)
    if doc.get("skins") or doc.get("animations"):
        raise ValueError("Expected an unrigged static source")
    if len(doc.get("scenes", [])) != 1:
        raise ValueError("Rigging currently requires exactly one explicit source scene")
    original, original_binary = (copy.deepcopy(doc), bytes(binary))
    initial_world = glb.worlds(doc)
    roles, points_by_primitive, used_meshes, seen_names = ({}, {}, set(), set())
    for index, node in enumerate(doc["nodes"]):
        if "mesh" not in node:
            continue
        name = node.get("name")
        if name in seen_names or name not in plan["part_roles"]:
            raise ValueError("Every mesh node needs a unique explicitly mapped name")
        seen_names.add(name)
        if node["mesh"] in used_meshes:
            raise ValueError(
                "Instanced meshes require an explicit de-instancing operation before rigging"
            )
        used_meshes.add(node["mesh"])
        roles[index] = plan["part_roles"][name]
        for pi, primitive in enumerate(doc["meshes"][node["mesh"]]["primitives"]):
            if (
                primitive.get("mode", 4) != 4
                or primitive.get("extensions")
                or primitive.get("targets")
                or ("indices" not in primitive)
            ):
                raise ValueError(
                    "Expected decoded static indexed triangle primitives without morph targets"
                )
            if any(key.startswith(("JOINTS_", "WEIGHTS_")) for key in primitive["attributes"]):
                raise ValueError("Source already contains skin attributes")
            points = glb.points(
                glb.accessor(doc, binary, primitive["attributes"]["POSITION"]), initial_world[index]
            )
            indices = glb.accessor(doc, binary, primitive["indices"])
            if (
                indices.dtype.kind not in "iu"
                or indices.size % 3
                or (not indices.size)
                or (indices.min() < 0)
                or (indices.max() >= len(points))
            ):
                raise ValueError("Invalid triangle index buffer")
            points_by_primitive[index, pi] = (points, indices.reshape(-1, 3).astype(int))
    if seen_names != set(plan["part_roles"]):
        raise ValueError("Part-role plan contains absent mesh nodes")
    if not points_by_primitive:
        raise ValueError("No supported geometry")
    height = float(np.ptp(np.vstack([points for points, _ in points_by_primitive.values()])[:, 1]))
    if height <= 1e-08:
        raise ValueError("Source has no positive measured height")
    out.mkdir(parents=True, exist_ok=True)
    for name in ("animated.glb", "working.blend", "report.json", "plan.json"):
        if (out / name).exists() or (out / name).is_symlink():
            raise ValueError("Output artifacts already exist; use a fresh operation directory")
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bones = plan["bones"]
    rig = build_armature(bones)
    node_map, frames = append_skeleton(doc, bones)
    doc["skins"] = []
    bindings, support = ([], [])
    names = list(bones)
    ground = plan.get("grounding")
    for node_index, role in roles.items():
        node = doc["nodes"][node_index]
        inverse = np.asarray(
            [np.linalg.inv(np.asarray(frames[name])) @ initial_world[node_index] for name in bones]
        )
        inverse_index = glb.append_accessor(
            doc, binary, inverse.transpose(0, 2, 1).reshape(-1, 16), 5126, "MAT4"
        )
        node["skin"] = len(doc["skins"])
        node.setdefault("extras", {})["anatomy_part"] = role
        doc["skins"].append(
            {
                "name": node["name"] + "_skin",
                "joints": list(node_map.values()),
                "inverseBindMatrices": inverse_index,
            }
        )
        binding = plan["bindings"][role]
        for pi, primitive in enumerate(doc["meshes"][node["mesh"]]["primitives"]):
            points, triangles = points_by_primitive[node_index, pi]
            if binding["method"] == "rigid":
                dense = np.zeros((len(points), len(bones)))
                dense[:, names.index(binding["bones"][0])] = 1
                record = {"method": "rigid", "bone": binding["bones"][0]}
            elif binding["method"] == "chain":
                dense, record = chain_weights(points, bones, binding["bones"])
            else:
                dense, record = heat_weights(
                    points, triangles, rig, bones, height, binding["bones"]
                )
            record["regions"] = apply_regions(
                dense, points, triangles, bones, binding.get("regions", []), height
            )
            joints, weights = compact_weights(dense)
            primitive["attributes"]["JOINTS_0"] = glb.append_accessor(
                doc, binary, joints, 5123, "VEC4"
            )
            primitive["attributes"]["WEIGHTS_0"] = glb.append_accessor(
                doc, binary, weights, 5126, "VEC4"
            )
            record.update(
                node=node["name"],
                role=role,
                primitive=pi,
                vertex_records=len(points),
                weighted_bones=[
                    name for ji, name in enumerate(names) if np.any((joints == ji) & (weights > 0))
                ],
                maximum_weight_sum_error=float(np.max(np.abs(weights.sum(1) - 1))),
            )
            bindings.append(record)
            if ground and role in ground["support_roles"]:
                referenced = np.unique(triangles)
                foot_weight = np.sum(
                    weights
                    * np.isin(joints, [names.index(name) for name in ground["support_bones"]]),
                    axis=1,
                )
                mask = (
                    (
                        points[:, 1]
                        <= points[referenced, 1].min() + height * ground["height_fraction"]
                    )
                    & (foot_weight >= ground["min_weight"])
                    & np.isin(np.arange(len(points)), referenced)
                )
                if np.any(mask):
                    support.append(
                        {
                            "positions": glb.accessor(
                                doc, binary, primitive["attributes"]["POSITION"]
                            )[mask],
                            "joints": joints[mask],
                            "weights": weights[mask],
                            "inverse_bind": inverse,
                        }
                    )
    if ground and (not support):
        raise ValueError("No sole/support vertices matched explicit grounding requirements")
    materials = material_finish(doc, roles, plan["materials"])
    clips = append_clips(doc, binary, plan, node_map, frames, support, height)
    doc.setdefault("asset", {}).setdefault("extras", {})["character_rig"] = {
        "schema_version": 1,
        "source_sha256": source_sha,
        "plan_sha256": plan_sha,
        "authorship": (
            "Explicit agent-authored plan executed by local Blender tools; semantic review pending."
        ),
    }
    target = out / "animated.glb"
    glb.save_glb(target, doc, binary)
    check, checked_binary = glb.read_glb(target)
    if bytes(checked_binary[: len(original_binary)]) != original_binary:
        raise ValueError("Source binary prefix changed")
    for key in ("images", "textures"):
        if check.get(key) != original.get(key):
            raise ValueError("Original embedded texture description changed")
    for old_mesh, new_mesh in zip(original["meshes"], check["meshes"], strict=True):
        for old, new in zip(old_mesh["primitives"], new_mesh["primitives"], strict=True):
            if old["indices"] != new["indices"] or any(
                (new["attributes"].get(name) != value for name, value in old["attributes"].items())
            ):
                raise ValueError("Original geometry/UV/index attributes changed")
    if glb.sha(source) != source_sha:
        raise ValueError("Source changed during rig operation")
    from _character_tools.worker.blender_io import import_source

    import_source(target, fps=plan["clips"][0]["fps"])
    for image in bpy.data.images:
        if image.has_data:
            image.pack()
    bpy.context.preferences.filepaths.save_version = 0
    bpy.ops.wm.save_as_mainfile(filepath=str(out / "working.blend"))
    report = {
        "schema_version": 1,
        "status": "structurally_checked_visual_review_pending",
        "source_sha256": source_sha,
        "plan_sha256": plan_sha,
        "source_height": height,
        "outputs": {
            "glb": "animated.glb",
            "glb_sha256": glb.sha(target),
            "blend": "working.blend",
            "blend_sha256": glb.sha(out / "working.blend"),
        },
        "joint_count": len(bones),
        "bindings": bindings,
        "clips": clips,
        "material_edits": materials,
        "provider_calls": 0,
        "preservation": {
            "source_unchanged": True,
            "original_binary_prefix_exact": True,
            "geometry_uv_indices_exact": True,
            "images_textures_exact": True,
        },
        "limitations": [
            "Numerical binding is not visual acceptance.",
            ("No anatomy, fitting, collision or seam repair is inferred by this worker."),
            "Whole-component constraints are evaluated per primitive.",
            ("Grounding is vertical support correction, not horizontal foot locking."),
        ],
    }
    write_json(out / "plan.json", raw_plan)
    write_json(out / "report.json", report)
    return report
