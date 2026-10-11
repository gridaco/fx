"""Blender import/export and coordinate adapters with embedded-only dependencies."""

from __future__ import annotations

import hashlib
import json
import math
import struct
from collections.abc import Iterator
from contextlib import contextmanager
from importlib import import_module
from pathlib import Path
from typing import TYPE_CHECKING, Any

import numpy as np
from numpy.typing import NDArray

bpy = import_module("bpy")


@contextmanager
def swapped_attribute(target: Any, name: str, replacement: Any) -> Iterator[None]:
    """Temporarily replace one attribute; the importer reads it during the block."""
    original = getattr(target, name)
    setattr(target, name, replacement)
    try:
        yield
    finally:
        setattr(target, name, original)


Matrix = import_module("mathutils").Matrix
Vector = import_module("mathutils").Vector
BVHTree = import_module("mathutils.bvhtree").BVHTree
KDTree = import_module("mathutils.kdtree").KDTree
if TYPE_CHECKING:
    from _character_tools.worker import contract, geometry
else:
    try:
        from _character_tools.worker import contract, geometry
    except ImportError:
        from _character_tools.worker import contract as contract
        from _character_tools.worker import geometry as geometry
TO_BLENDER = Matrix(contract.COORDINATES["gltf_to_blender"])
TO_PUBLIC = TO_BLENDER.inverted()


def glb_preflight(path: Path) -> dict[str, Any]:
    content = path.read_bytes()
    if len(content) < 28 or struct.unpack_from("<4sII", content) != (b"glTF", 2, len(content)):
        raise ValueError("Invalid GLB header")
    size, kind = struct.unpack_from("<II", content, 12)
    if kind != 1313821514 or 20 + size + 8 > len(content):
        raise ValueError("Invalid GLB document chunk")
    doc = json.loads(content[20 : 20 + size])
    if any("uri" in item for item in doc.get("buffers", []) + doc.get("images", [])):
        raise ValueError("Only embedded GLB resources are allowed")
    if len(doc.get("buffers", [])) != 1:
        raise ValueError("Expected a single embedded GLB buffer")
    return {
        "mesh_count": len(doc.get("meshes", [])),
        "primitive_count": sum(len(m["primitives"]) for m in doc.get("meshes", [])),
        "vertex_records": sum(
            doc["accessors"][p["attributes"]["POSITION"]]["count"]
            for m in doc.get("meshes", [])
            for p in m["primitives"]
        ),
        "skin_count": len(doc.get("skins", [])),
        "animation_names": [
            a.get("name", f"animation_{i}") for i, a in enumerate(doc.get("animations", []))
        ],
        "extensions_used": doc.get("extensionsUsed", []),
    }


def import_source(
    path: Path, fps: int = 30, reset: bool = True
) -> tuple[list[Any], dict[str, Any] | None]:
    if reset:
        bpy.ops.wm.read_factory_settings(use_empty=True)
    existing = set(bpy.context.scene.objects)
    bpy.context.scene.render.fps = fps
    bpy.context.scene.render.fps_base = 1
    bpy.context.scene["worker_requested_fps"] = fps
    raw = None
    if path.suffix.lower() == ".glb":
        raw = glb_preflight(path)
        bpy.ops.import_scene.gltf(filepath=str(path), disable_bone_shape=True)
    elif path.suffix.lower() == ".fbx":
        if not path.read_bytes()[:23].startswith(b"Kaydara FBX Binary"):
            raise ValueError("Only binary FBX is supported")
        image_utils = import_module("bpy_extras.image_utils")

        def embedded_placeholder(filepath: str | Path, **_kwargs: object) -> Any:
            name = hashlib.sha256(str(filepath).encode()).hexdigest()[:20]
            image = bpy.data.images.new("embedded_" + name, width=1, height=1)
            image.source = "FILE"
            image.filepath_raw = ""
            return image

        with swapped_attribute(image_utils, "load_image", embedded_placeholder):
            bpy.ops.import_scene.fbx(
                filepath=str(path), use_image_search=False, use_custom_props=False
            )
    else:
        raise ValueError("Worker accepts embedded GLB or binary FBX only")
    objects = [obj for obj in bpy.context.scene.objects if obj not in existing]
    if not any(obj.type == "MESH" for obj in objects):
        raise ValueError("Source contains no meshes")
    for image in bpy.data.images:
        image.filepath_raw = ""
    return (objects, raw)


def animation_owners(objects: list[Any]) -> list[Any]:
    owners = {}
    for obj in objects:
        candidates = [obj, obj.data, getattr(obj.data, "shape_keys", None)]
        for slot in getattr(obj, "material_slots", []):
            candidates.extend([slot.material, getattr(slot.material, "node_tree", None)])
        for value in candidates:
            if value is not None and getattr(value, "animation_data", None):
                owners[value.as_pointer()] = value
    return list(owners.values())


def clips(objects: list[Any]) -> list[dict[str, Any]]:
    found: dict[str, list[dict[str, Any]]] = {}
    for owner in animation_owners(objects):
        for track in owner.animation_data.nla_tracks:
            for strip in track.strips:
                if strip.action is not None:
                    bounds = list(map(float, strip.action.frame_range))
                    found.setdefault(track.name, []).append(
                        {"owner": owner.name, "frame_range": bounds}
                    )
    return [{"name": name, "bindings": values} for name, values in sorted(found.items())]


def select_pose(objects: list[Any], pose: dict[str, Any] | None) -> dict[str, Any]:
    owners = animation_owners(objects)
    available = clips(objects)
    for obj in objects:
        if obj.type == "ARMATURE":
            obj.data.pose_position = "POSE" if pose else "REST"
    bindings: list[dict[str, Any]] = []
    for owner in owners:
        animation = owner.animation_data
        selected: list[Any] = []
        for track in animation.nla_tracks:
            track.mute = True
            track.is_solo = False
            if pose and track.name == pose["clip"]:
                selected.extend(strip for strip in track.strips if strip.action is not None)
        animation.action = None
        animation.use_nla = False
        if not selected:
            continue
        if len(selected) != 1:
            raise ValueError("Clip must have one action binding per animated owner")
        strip = selected[0]
        animation.action = strip.action
        if hasattr(strip, "action_slot"):
            animation.action_slot = strip.action_slot
        animation.action_blend_type = "REPLACE"
        animation.action_influence = 1
        animation.action_extrapolation = "HOLD"
        bindings.append(
            {
                "owner": owner.name,
                "action": strip.action.name,
                "frame_range": list(map(float, strip.action.frame_range)),
            }
        )
    if pose:
        if not bindings:
            raise ValueError("Exact requested clip is unavailable")
        start = min(b["frame_range"][0] for b in bindings)
        end = max(b["frame_range"][1] for b in bindings)
        effective_fps = bpy.context.scene.render.fps / bpy.context.scene.render.fps_base
        frame = start + pose["time_seconds"] * effective_fps
        if frame > end + 1e-05:
            raise ValueError("Requested time exceeds actual imported clip duration")
    else:
        start = end = frame = 0
    integer = math.floor(frame)
    bpy.context.scene.frame_set(integer, subframe=frame - integer)
    bpy.context.view_layer.update()
    return {
        "clip": pose["clip"] if pose else None,
        "time_seconds": pose["time_seconds"] if pose else 0,
        "requested_fps_before_import": bpy.context.scene["worker_requested_fps"],
        "effective_fps_after_import": bpy.context.scene.render.fps
        / bpy.context.scene.render.fps_base,
        "fps_base": bpy.context.scene.render.fps_base,
        "frame": frame,
        "frame_range": [start, end],
        "bindings": bindings,
        "available_clips": available,
    }


def mesh_arrays(obj: Any) -> dict[str, Any]:
    graph = bpy.context.evaluated_depsgraph_get()
    evaluated = obj.evaluated_get(graph)
    mesh = evaluated.to_mesh(preserve_all_data_layers=True, depsgraph=graph)
    try:
        mesh.calc_loop_triangles()
        local = np.array([vertex.co[:] for vertex in mesh.vertices], dtype=float)
        matrix = np.asarray(TO_PUBLIC @ evaluated.matrix_world)
        positions = local @ matrix[:3, :3].T + matrix[:3, 3]
        triangles = np.array(
            [triangle.vertices[:] for triangle in mesh.loop_triangles], dtype=np.int32
        ).reshape(-1, 3)
        if not len(triangles) or not np.isfinite(positions).all():
            raise ValueError("Expected finite indexed triangle geometry")
        loops = np.array([triangle.loops[:] for triangle in mesh.loop_triangles], dtype=np.int32)
        uv_layers = {
            layer.name: np.array([item.uv[:] for item in layer.data], dtype=float)
            for layer in mesh.uv_layers
        }
        data = {
            "positions": positions,
            "triangles": triangles,
            "triangle_loops": loops,
            "triangle_polygon_indices": np.array(
                [triangle.polygon_index for triangle in mesh.loop_triangles], dtype=np.int32
            ),
            "triangle_material_indices": np.array(
                [
                    mesh.polygons[triangle.polygon_index].material_index
                    for triangle in mesh.loop_triangles
                ],
                dtype=np.int32,
            ),
            "uv_layers": uv_layers,
            "native_polygon_counts": {
                "triangles": sum(len(p.vertices) == 3 for p in mesh.polygons),
                "quads": sum(len(p.vertices) == 4 for p in mesh.polygons),
                "ngons": sum(len(p.vertices) > 4 for p in mesh.polygons),
            },
        }
        return data
    finally:
        evaluated.to_mesh_clear()


def scene_arrays(objects: list[Any]) -> dict[str, dict[str, Any]]:
    return {obj.name: mesh_arrays(obj) for obj in objects if obj.type == "MESH"}


def image_inventory() -> list[dict[str, Any]]:
    entries = []
    for image in bpy.data.images:
        packed = image.packed_file
        entries.append(
            {
                "name": image.name,
                "width": image.size[0],
                "height": image.size[1],
                "has_data": bool(image.has_data),
                "packed": packed is not None,
                "packed_sha256": hashlib.sha256(packed.data).hexdigest() if packed else None,
                "color_space": image.colorspace_settings.name,
            }
        )
    return entries


def inventory(objects: list[Any], arrays: dict[str, dict[str, Any]]) -> dict[str, Any]:
    meshes: list[dict[str, Any]] = []
    bones: list[dict[str, Any]] = []
    for obj in objects:
        if obj.type == "ARMATURE":
            for bone in obj.data.bones:
                matrix = TO_PUBLIC @ obj.matrix_world
                bones.append(
                    {
                        "armature": obj.name,
                        "name": bone.name,
                        "parent": bone.parent.name if bone.parent else None,
                        "head": list(matrix @ bone.head_local),
                        "tail": list(matrix @ bone.tail_local),
                        "deform": bone.use_deform,
                    }
                )
        if obj.type != "MESH":
            continue
        data = arrays[obj.name]
        weighted: dict[str, int] = {}
        for vertex in obj.data.vertices:
            for group in vertex.groups:
                if group.weight > 0:
                    name = obj.vertex_groups[group.group].name
                    weighted[name] = weighted.get(name, 0) + 1
        meshes.append(
            {
                "name": obj.name,
                "vertices": len(data["positions"]),
                "triangles": len(data["triangles"]),
                "native_polygon_counts": data["native_polygon_counts"],
                "bounds": geometry.bounds(data["positions"]),
                "materials": [
                    material.name if material else None for material in obj.data.materials
                ],
                "uv_layers": [
                    {"name": name, "corner_count": len(values)}
                    for name, values in data["uv_layers"].items()
                ],
                "shape_keys": [block.name for block in obj.data.shape_keys.key_blocks]
                if obj.data.shape_keys
                else [],
                "modifiers": [
                    {"name": modifier.name, "type": modifier.type} for modifier in obj.modifiers
                ],
                "weighted_vertex_groups_source_mesh": weighted,
            }
        )
    return {
        "meshes": meshes,
        "mesh_count": len(meshes),
        "vertices": sum(m["vertices"] for m in meshes),
        "triangles": sum(m["triangles"] for m in meshes),
        "bounds": geometry.bounds(np.concatenate([data["positions"] for data in arrays.values()])),
        "images": image_inventory(),
        "bones": bones,
        "clips": clips(objects),
        "index_space": (
            "evaluated_blender_mesh_vertices_and_loop_triangles_not_raw_glb_vertex_records"
        ),
    }


def nearest_queries(
    arrays: dict[str, dict[str, Any]], queries: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    trees = {
        name: BVHTree.FromPolygons(
            data["positions"].tolist(), data["triangles"].tolist(), all_triangles=True
        )
        for name, data in arrays.items()
    }
    result = []
    for query in queries:
        choices = [query["mesh"]] if "mesh" in query else list(arrays)
        if set(choices) - arrays.keys():
            raise ValueError("Nearest query names an unavailable mesh")
        best = None
        for name in choices:
            location, normal, index, distance = trees[name].find_nearest(Vector(query["point"]))
            if location is not None and (best is None or distance < best["distance"]):
                face = arrays[name]["triangles"][index]
                best = {
                    "mesh": name,
                    "point": list(location),
                    "normal": list(normal),
                    "distance": distance,
                    "triangle_index": index,
                    "vertex_indices": face.tolist(),
                }
        result.append({"name": query["name"], "query_point": query["point"], "nearest": best})
    return result


def assert_static(objects: list[Any]) -> None:
    for obj in objects:
        if obj.type not in {"MESH", "EMPTY"} or obj.animation_data is not None:
            raise ValueError("Normalization refuses rigs, animation and non-mesh scene assets")
        if obj.type == "MESH" and (obj.data.shape_keys is not None or obj.modifiers):
            raise ValueError("Normalization refuses to discard modifiers or shape keys")
    if bpy.data.actions:
        raise ValueError("Normalization refuses any animation actions")


def normalize(objects: list[Any], options: dict[str, Any], output: Path) -> dict[str, Any]:
    assert_static(objects)
    images = image_inventory()
    if any(not image["packed"] or not image["has_data"] for image in images):
        raise ValueError("Normalization requires every referenced image to be embedded and loaded")
    if options.get("require_textures") and (not images):
        raise ValueError("Textured normalization requested but no images exist")
    transform = np.asarray(options.get("transform", np.eye(4).tolist()), dtype=float)
    linear = transform[:3, :3]
    scale2 = np.trace(linear.T @ linear) / 3
    if (
        not np.allclose(transform[3], [0, 0, 0, 1])
        or scale2 <= 0
        or np.linalg.det(linear) <= 0
        or (not np.allclose(linear.T @ linear, np.eye(3) * scale2, rtol=1e-06, atol=1e-09))
    ):
        raise ValueError(
            "Normalization accepts only proper rigid transforms and positive uniform scale"
        )
    converted = TO_BLENDER @ Matrix(transform.tolist()) @ TO_PUBLIC
    for obj in objects:
        if obj.parent is None:
            obj.matrix_world = converted @ obj.matrix_world
    bpy.context.view_layer.update()
    before_arrays = scene_arrays(objects)
    before = inventory(objects, before_arrays)
    for image in bpy.data.images:
        if image.packed_file is None:
            image.pack()
    bpy.context.preferences.filepaths.save_version = 0
    bpy.ops.wm.save_as_mainfile(filepath=str(output / "native.blend"))
    bpy.ops.object.select_all(action="DESELECT")
    for obj in objects:
        obj.select_set(True)
    pending = output / "model.pending.glb"
    bpy.ops.export_scene.gltf(
        filepath=str(pending),
        export_format="GLB",
        use_selection=True,
        export_animations=False,
        export_extras=True,
        export_yup=True,
        export_materials="EXPORT",
    )
    after_objects, _ = import_source(pending)
    select_pose(after_objects, None)
    after_arrays = scene_arrays(after_objects)
    after = inventory(after_objects, after_arrays)
    if before["triangles"] != after["triangles"] or before["mesh_count"] != after["mesh_count"]:
        raise ValueError("Normalization changed mesh or triangulated surface count")
    before_materials = {mesh["name"]: mesh["materials"] for mesh in before["meshes"]}
    after_materials = {mesh["name"]: mesh["materials"] for mesh in after["meshes"]}
    if before_materials != after_materials:
        raise ValueError("Normalization changed mesh names or material slot associations")
    before_uv_counts = {mesh["name"]: len(mesh["uv_layers"]) for mesh in before["meshes"]}
    after_uv_counts = {mesh["name"]: len(mesh["uv_layers"]) for mesh in after["meshes"]}
    if before_uv_counts != after_uv_counts:
        raise ValueError("Normalization changed UV layer counts")
    left = np.concatenate([data["positions"] for data in before_arrays.values()])
    right = np.concatenate([data["positions"] for data in after_arrays.values()])

    def maximum_distance(source: NDArray[Any], target: NDArray[Any]) -> float:
        tree = KDTree(len(target))
        for index, point in enumerate(target):
            tree.insert(Vector(point), index)
        tree.balance()
        distance: float = max(tree.find(Vector(point))[2] for point in source)
        return distance

    distance = max(maximum_distance(left, right), maximum_distance(right, left))
    tolerance = max(float(np.linalg.norm(np.ptp(left, axis=0))) * 2e-06, 1e-07)
    if distance > tolerance:
        raise ValueError("Normalization moved the surface beyond the explicit transform")
    model = output / "model.glb"
    pending.replace(model)
    return {
        "applied_transform": transform.tolist(),
        "before_export": before,
        "after_reimport": after,
        "checks": {
            "triangle_count_preserved": True,
            "mesh_count_preserved": True,
            "material_slot_associations_preserved": True,
            "uv_layer_counts_preserved": True,
            "maximum_bidirectional_vertex_distance": distance,
            "position_tolerance": tolerance,
            "uvs_and_materials": "reported_not_promised_bit_identical_after_format_conversion",
            "native_polygons": "retained_in_native_blend_glb_triangulates",
        },
        "model": "model.glb",
        "native_blend": "native.blend",
    }
