"""Fixed-coordinate native or clay diagnostic renders with numeric cameras."""

from __future__ import annotations

import itertools
from importlib import import_module
from pathlib import Path
from typing import TYPE_CHECKING, Any

import numpy as np
from numpy.typing import ArrayLike, NDArray

bpy = import_module("bpy")
Matrix = import_module("mathutils").Matrix
Vector = import_module("mathutils").Vector
if TYPE_CHECKING:
    from _character_tools.worker import contract, geometry
    from _character_tools.worker.blender_io import (
        TO_BLENDER,
        TO_PUBLIC,
    )
else:
    try:
        from _character_tools.worker import contract, geometry
        from _character_tools.worker.blender_io import (
            TO_BLENDER,
            TO_PUBLIC,
        )
    except ImportError:
        from blender_io import TO_BLENDER, TO_PUBLIC

        from _character_tools.worker import contract as contract
        from _character_tools.worker import geometry as geometry


def apply_matte_policy_preview(objects: list[Any]) -> None:
    """Preview the declared matte export finish on the source materials.

    Mirrors the provider-rig export policy: metallic 0, roughness 1, specular 0.08,
    normal strength at most 0.15, no coat or sheen. Base color textures stay. Raw
    provider gloss is never shipped, so reviewers should not see it as the product.
    """
    seen: set[str] = set()
    for obj in objects:
        if obj.type != "MESH":
            continue
        for slot in obj.material_slots:
            material = slot.material
            if material is None or material.name in seen or (not material.use_nodes):
                continue
            seen.add(material.name)
            tree = material.node_tree
            for node in tree.nodes:
                if node.type == "BSDF_PRINCIPLED":
                    fixed = {
                        "Metallic": 0,
                        "Roughness": 1,
                        "Specular IOR Level": 0.08,
                        "Specular": 0.08,
                        "Coat Weight": 0,
                        "Sheen Weight": 0,
                    }
                    for link in list(tree.links):
                        if link.to_node == node and link.to_socket.name in fixed:
                            tree.links.remove(link)
                    for name, value in fixed.items():
                        if name in node.inputs:
                            node.inputs[name].default_value = value
                elif node.type == "NORMAL_MAP":
                    strength = node.inputs["Strength"]
                    strength.default_value = min(strength.default_value, 0.15)


def camera_matrix(
    center: NDArray[Any], direction: ArrayLike, up: ArrayLike, distance: float
) -> NDArray[Any]:
    direction, up = (np.asarray(direction, dtype=float), np.asarray(up, dtype=float))
    if np.linalg.norm(direction) < 1e-10 or np.linalg.norm(up) < 1e-10:
        raise ValueError("Camera direction and up cannot be zero")
    outward = direction / np.linalg.norm(direction)
    right = np.cross(up, outward)
    if np.linalg.norm(right) < 1e-08:
        raise ValueError("Camera direction and up cannot be parallel")
    right /= np.linalg.norm(right)
    screen_up = np.cross(outward, right)
    matrix = np.eye(4)
    matrix[:3, :3] = np.stack([right, screen_up, outward], axis=1)
    matrix[:3, 3] = center + outward * distance
    return matrix


def fit_character_height(
    points: NDArray[Any], matrix: NDArray[Any], resolution: list[int], target_height: int
) -> tuple[NDArray[Any], float, float]:
    """Frame actual vertices at a requested size; no AABB corner substitution."""
    width, height = resolution
    projected = (points - matrix[:3, 3]) @ matrix[:3, :3]
    lower, upper = (projected.min(axis=0), projected.max(axis=0))
    span = upper - lower
    if span[1] <= 1e-12:
        raise ValueError("Visible geometry has no measurable projected height in this view")
    vertical_extent = float(span[1] * height / target_height)
    horizontal_extent = vertical_extent * width / height
    if span[0] > horizontal_extent * (1 + 1e-08):
        raise ValueError(
            "Requested character pixel height would crop its width; lower t"
            "he target or widen the canvas"
        )
    matrix = matrix.copy()
    midpoint = (lower + upper) / 2
    matrix[:3, 3] += matrix[:3, 0] * midpoint[0] + matrix[:3, 1] * midpoint[1]
    return (matrix, horizontal_extent, vertical_extent)


def projected_geometry(
    points: NDArray[Any], matrix: NDArray[Any], frame: NDArray[Any], resolution: list[int]
) -> dict[str, Any]:
    """Continuous pixel-edge coordinates using the actual exported camera frame."""
    inverse = np.linalg.inv(matrix)
    local = points @ inverse[:3, :3].T + inverse[:3, 3]
    lower, upper = (frame.min(axis=0), frame.max(axis=0))
    width, height = resolution
    pixels = np.column_stack(
        (
            (local[:, 0] - lower[0]) / (upper[0] - lower[0]) * width,
            (upper[1] - local[:, 1]) / (upper[1] - lower[1]) * height,
        )
    )
    pixel_min, pixel_max = (pixels.min(axis=0), pixels.max(axis=0))
    return {
        "pixel_bounds": {"min": pixel_min.tolist(), "max": pixel_max.tolist()},
        "height_pixels": float(pixel_max[1] - pixel_min[1]),
        "width_pixels": float(pixel_max[0] - pixel_min[0]),
        "point_count": len(points),
        "fully_in_frame": bool(
            np.all(pixel_min >= -0.001) and np.all(pixel_max <= np.array([width, height]) + 0.001)
        ),
        "measurement": (
            "Triangle-referenced vertices of unhidden evaluated meshes, in "
            "continuous top-left-origin pixel-edge coordinates. Transparenc"
            "y and occlusion are not raster-tested; the diagnostic ground i"
            "s excluded."
        ),
    }


def render(
    objects: list[Any], arrays: dict[str, dict[str, Any]], options: dict[str, Any], output: Path
) -> dict[str, Any]:
    hidden = set(options.get("hide_meshes", []))
    if hidden - arrays.keys():
        raise ValueError("Hidden mesh selector does not exist")
    visible = {name: data for name, data in arrays.items() if name not in hidden}
    if not visible:
        raise ValueError("Cannot render with every mesh hidden")
    projected_parts = [
        data["positions"][np.unique(data["triangles"])]
        for data in visible.values()
        if len(data["triangles"])
    ]
    if not projected_parts:
        raise ValueError("Visible meshes contain no triangle-referenced vertices")
    projected_points = np.concatenate(projected_parts)
    for obj in objects:
        if obj.type == "MESH":
            obj.hide_render = obj.name in hidden
        elif obj.type in {"CAMERA", "LIGHT"}:
            obj.hide_render = True
    measured = geometry.bounds(np.concatenate([data["positions"] for data in visible.values()]))
    framing = options.get("focus_bounds", measured)
    lower, upper = (np.asarray(framing["min"]), np.asarray(framing["max"]))
    center, dimensions = ((lower + upper) / 2, upper - lower)
    radius = max(float(np.linalg.norm(dimensions)), 0.0001)
    corners = np.array(list(itertools.product(*zip(lower, upper, strict=True))))
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    scene.cycles.samples = options.get("samples", 16)
    scene.cycles.use_denoising = True
    width, height = options.get("resolution", [512, 512])
    scene.render.resolution_x, scene.render.resolution_y = (width, height)
    scene.render.resolution_percentage = 100
    scene.render.pixel_aspect_x = scene.render.pixel_aspect_y = 1
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGB"
    scene.render.film_transparent = False
    scene.view_settings.view_transform = "Standard"
    scene.view_settings.look = "None"
    scene.view_settings.exposure = 0
    scene.view_settings.gamma = 1
    world = bpy.data.worlds.new("Worker neutral world")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs[0].default_value = (0.72, 0.74, 0.76, 1)
    world.node_tree.nodes["Background"].inputs[1].default_value = 0.65
    scene.world = world
    for name, offset, energy in (
        ("Key", (2, 3, 3), 70),
        ("Fill", (-3, 1, 2), 40),
        ("Back", (0, 2, -3), 50),
    ):
        light = bpy.data.lights.new(name, "AREA")
        light.energy, light.shape, light.size = (energy * radius * radius, "DISK", radius * 2)
        obj = bpy.data.objects.new(name, light)
        scene.collection.objects.link(obj)
        obj.location = TO_BLENDER @ Vector(center + np.asarray(offset) * radius)
        target = TO_BLENDER @ Vector(center)
        obj.rotation_euler = (target - obj.location).to_track_quat("-Z", "Y").to_euler()
    ground = options.get("ground_height")
    if ground is not None:
        bpy.ops.mesh.primitive_plane_add(size=radius * 20, location=(center[0], -center[2], ground))
        bpy.context.object.name = "Worker fixed ground"
    if options.get("material_mode", "native") == "clay_diagnostic":
        material = bpy.data.materials.new("Worker clay diagnostic")
        material.use_nodes = True
        shader = material.node_tree.nodes.get("Principled BSDF")
        shader.inputs["Base Color"].default_value = (0.38, 0.4, 0.42, 1)
        shader.inputs["Roughness"].default_value = 1
        for obj in objects:
            if obj.type == "MESH":
                obj.data = obj.data.copy()
                obj.data.materials.clear()
                obj.data.materials.append(material)
    elif options.get("material_mode", "native") == "matte_policy":
        apply_matte_policy_preview(objects)
    data = bpy.data.cameras.new("Worker numeric camera")
    camera = bpy.data.objects.new("Worker numeric camera", data)
    scene.collection.objects.link(camera)
    scene.camera = camera
    data.type = "ORTHO"
    data.clip_start, data.clip_end = (radius * 0.001, radius * 10)
    records = []
    for spec in options.get("views", list(contract.AXES)):
        if isinstance(spec, str):
            name, direction = (spec, contract.AXES[spec])
            up = (0, 0, -1) if abs(direction[1]) == 1 else (0, 1, 0)
        else:
            name, direction, up = (spec["name"], spec["direction"], spec["up"])
        matrix = camera_matrix(center, direction, up, radius * 3)
        projected = (corners - center) @ matrix[:3, :3]
        span = np.ptp(projected, axis=0)
        aspect = width / height
        vertical_extent = max(span[1], span[0] / aspect) * options.get("margin", 1.15)
        horizontal_extent = vertical_extent * aspect
        target_height = options.get("character_height_pixels")
        if target_height is not None:
            matrix, horizontal_extent, vertical_extent = fit_character_height(
                projected_points, matrix, [width, height], target_height
            )
        data.ortho_scale = max(horizontal_extent, vertical_extent)
        camera.matrix_world = TO_BLENDER @ Matrix(matrix.tolist())
        bpy.context.view_layer.update()
        frame = np.asarray([list(corner) for corner in data.view_frame(scene=scene)])
        actual_extents = np.ptp(frame, axis=0)
        if not np.allclose(actual_extents[:2], [horizontal_extent, vertical_extent], rtol=1e-05):
            raise ValueError("Camera projection differs from declared pixel-ray extents")
        actual_matrix = np.asarray(TO_PUBLIC @ camera.matrix_world, dtype=float)
        projected_report = projected_geometry(
            projected_points, actual_matrix, frame, [width, height]
        )
        if target_height is not None and (
            abs(projected_report["height_pixels"] - target_height)
            > max(0.0001, target_height * 1e-05)
            or not projected_report["fully_in_frame"]
        ):
            raise ValueError(
                "Actual camera does not achieve the requested complete character pixel height"
            )
        path = output / (name + ".png")
        scene.render.filepath = str(path)
        bpy.ops.render.render(write_still=True)
        records.append(
            {
                "name": name,
                "image": path.name,
                "width": width,
                "height": height,
                "camera_matrix_world": matrix.tolist(),
                "projection": "orthographic",
                "horizontal_extent": horizontal_extent,
                "vertical_extent": vertical_extent,
                "clip_near": data.clip_start,
                "clip_far": data.clip_end,
                "camera_local_frame": frame.tolist(),
                "actual_camera_matrix_world": actual_matrix.tolist(),
                "requested_character_height_pixels": target_height,
                "projected_geometry": projected_report,
                "framing_policy": "requested_character_pixel_height"
                if target_height is not None
                else "existing_bounds_and_margin",
                "view_direction_from_target": list(direction),
                "requested_up": list(up),
                "pixel_ray": (
                    "Pixel center (u,v), top-left origin: origin=C.translation+C.x*"
                    "((u+0.5)/width-0.5)*horizontal_extent+C.y*(0.5-(v+0.5)/height)"
                    "*vertical_extent; direction=-C.z. Intersect scene to recover d"
                    "epth."
                ),
            }
        )
    return {
        "images": records,
        "measured_visible_bounds": measured,
        "framing_bounds": framing,
        "fixed_ground_height": ground,
        "hidden_meshes": sorted(hidden),
        "material_mode": options.get("material_mode", "native"),
        "color_management": {
            "view_transform": "Standard",
            "look": "None",
            "exposure": 0,
            "gamma": 1,
        },
        "source_file_materials_modified": False,
        "render_semantic_verdict": "unreviewed",
    }
