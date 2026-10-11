"""Headless cover capture of an existing GLB, with fixed framing and native materials."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

import bpy
import numpy as np
from mathutils import Matrix, Vector

from _character_tools.worker.blender_io import (
    TO_BLENDER,
    import_source,
    mesh_arrays,
    select_pose,
)
from _character_tools.worker.rendering import camera_matrix, projected_geometry


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fps", type=int, default=24)
    parser.add_argument("--duration", type=int, default=6)
    parser.add_argument("--width", type=int, default=960)
    parser.add_argument("--height", type=int, default=600)
    parser.add_argument("--preview", action="store_true")
    args = parser.parse_args(sys.argv[sys.argv.index("--") + 1 :])
    if (
        args.fps != 24
        or args.duration != 6
        or args.width != 960
        or args.height != 600
        or args.model.is_symlink()
        or args.output.exists()
    ):
        raise ValueError(
            "Cover capture requires a regular GLB and a fresh output at 960x600, 24fps, 6s"
        )
    digest = hashlib.sha256(args.model.read_bytes()).hexdigest()
    objects, _ = import_source(args.model, fps=24)
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    scene.cycles.samples = 8
    scene.cycles.use_denoising = True
    scene.render.resolution_x, scene.render.resolution_y = args.width, args.height
    scene.render.resolution_percentage = 100
    scene.render.pixel_aspect_x = scene.render.pixel_aspect_y = 1
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGB"
    scene.view_settings.view_transform = "Standard"
    scene.view_settings.look = "None"
    scene.view_settings.exposure = 0
    scene.view_settings.gamma = 1
    # The floor and background match the site's neutral backing. Native mesh,
    # texture and matte materials are never rewritten or replaced by emission.
    world = bpy.data.worlds.new("Cover neutral world")
    world.use_nodes = True
    tree = world.node_tree
    illumination = tree.nodes["Background"]
    illumination.inputs[0].default_value = (0.72, 0.74, 0.76, 1)
    illumination.inputs[1].default_value = 0.65
    backdrop = tree.nodes.new("ShaderNodeBackground")
    backdrop.inputs[0].default_value = (0.9131, 0.9131, 0.9131, 1)
    backdrop.inputs[1].default_value = 1
    light_path = tree.nodes.new("ShaderNodeLightPath")
    mix = tree.nodes.new("ShaderNodeMixShader")
    tree.links.new(light_path.outputs["Is Camera Ray"], mix.inputs[0])
    tree.links.new(illumination.outputs[0], mix.inputs[1])
    tree.links.new(backdrop.outputs[0], mix.inputs[2])
    tree.links.new(mix.outputs[0], tree.nodes["World Output"].inputs[0])
    scene.world = world
    clip_duration = 4.0
    count = args.fps * args.duration
    direction = np.array([0.65, 0.18, 1.5])
    positions = []
    for phase in np.linspace(0, 1, 97):
        select_pose(objects, {"clip": "cheer", "time_seconds": float(phase * clip_duration)})
        positions.append(
            np.concatenate([mesh_arrays(obj)["positions"] for obj in objects if obj.type == "MESH"])
        )
    all_points = np.concatenate(positions)
    lower, upper = all_points.min(0), all_points.max(0)
    center = (lower + upper) / 2
    radius = float(np.linalg.norm(upper - lower))
    matrix = camera_matrix(center, direction, [0, 1, 0], radius * 3)
    projected = (all_points - matrix[:3, 3]) @ matrix[:3, :3]
    lo, hi = projected.min(0), projected.max(0)
    midpoint = (lo + hi) / 2
    matrix[:3, 3] += matrix[:3, 0] * midpoint[0] + matrix[:3, 1] * midpoint[1]
    vertical_extent = max(
        (hi[1] - lo[1]) / 0.84, (hi[0] - lo[0]) / (args.width / args.height * 0.84)
    )
    horizontal_extent = vertical_extent * args.width / args.height
    camera_data = bpy.data.cameras.new("Cover fixed three-quarter camera")
    camera = bpy.data.objects.new("Cover fixed three-quarter camera", camera_data)
    scene.collection.objects.link(camera)
    camera_data.type = "ORTHO"
    camera_data.ortho_scale = horizontal_extent
    camera_data.clip_start, camera_data.clip_end = radius * 0.001, radius * 10
    camera.matrix_world = TO_BLENDER @ Matrix(matrix.tolist())
    scene.camera = camera
    for name, offset, energy in (
        ("Key", [2, 3, 3], 150),
        ("Fill", [-3, 2, -1], 90),
        ("Rim", [1, 2, -3], 120),
    ):
        light = bpy.data.lights.new(name, "AREA")
        light.energy, light.shape, light.size = energy, "DISK", radius * 2
        obj = bpy.data.objects.new(name, light)
        scene.collection.objects.link(obj)
        obj.location = TO_BLENDER @ Vector(center + np.asarray(offset))
        obj.rotation_euler = (
            (TO_BLENDER @ Vector(center) - obj.location).to_track_quat("-Z", "Y").to_euler()
        )
    frame = np.asarray([list(corner) for corner in camera_data.view_frame(scene=scene)])
    framing = projected_geometry(all_points, matrix, frame, [args.width, args.height])
    if not framing["fully_in_frame"]:
        raise ValueError("The fixed camera crops the sampled clip")
    args.output.mkdir(parents=True)
    # Slower playback of the real diagnostic clip; no invented body movement,
    # camera orbit, floating or scaling. Exclude the duplicate loop endpoint.
    for index in [24] if args.preview else range(count):
        select_pose(objects, {"clip": "cheer", "time_seconds": clip_duration * index / count})
        scene.render.filepath = str(args.output / f"{index:04d}.png")
        bpy.ops.render.render(write_still=True)
    if hashlib.sha256(args.model.read_bytes()).hexdigest() != digest:
        raise ValueError("Cover rendering changed the original GLB")
    (args.output / "capture.json").write_text(
        json.dumps(
            {
                "schema": "character_rig_cover_v1",
                "model_sha256": digest,
                "renderer": "Blender Cycles CPU",
                "material_policy": "native_exported_matte",
                "background_rgb": [245, 245, 245],
                "width": args.width,
                "height": args.height,
                "fps": args.fps,
                "frame_count": count,
                "duration_seconds": args.duration,
                "source_clip": "cheer",
                "source_clip_duration_seconds": clip_duration,
                "playback_rate": clip_duration / args.duration,
                "camera_motion": False,
                "geometry_and_glb_unchanged": True,
                "framing": framing,
                "fixed_camera_matrix_gltf": matrix.tolist(),
                "semantic_review_status": "capture_pending_independent_review",
                "single_frame_lighting_preview": args.preview,
            },
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
