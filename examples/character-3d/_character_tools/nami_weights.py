"""Hash-bound Nami lower-body skin-weight correction; never a general autorigger."""

from __future__ import annotations

import copy
import hashlib
import itertools
import json
from collections import defaultdict
from pathlib import Path

import numpy as np

from _character_tools.worker import provider_rig as rig
from _character_tools.worker import rig_glb as glb

SOURCE_SHA = "73cfe88743514eb1f3eb939314107d5b69d609a9ddd422058c1d98d7ec903dcc"
ARM_NAMES = tuple(
    "mixamorig:" + side + role
    for side in ("Left", "Right")
    for role in ("Shoulder", "Arm", "ForeArm", "Hand")
)
RECIPIENT_NAMES = (
    "mixamorig:Hips",
    "mixamorig:Spine",
    "mixamorig:Spine1",
    "mixamorig:LeftUpLeg",
    "mixamorig:RightUpLeg",
)


def selected_vertices(doc, binary):
    surfaces = rig.surfaces(doc, binary)
    if len(surfaces) != 1:
        raise ValueError("Exact candidate must have one skinned surface")
    surface = surfaces[0]
    points = rig.rig_metrics.surface_points(surface, glb.worlds(doc))
    use = points[:, 1] < 0.95
    parent = np.arange(len(points))

    def find(index):
        while parent[index] != index:
            parent[index] = parent[parent[index]]
            index = parent[index]
        return int(index)

    def union(a, b):
        a, b = find(a), find(b)
        if a != b:
            parent[b] = a

    # Static triangle adjacency and only coincident UV-seam positions. Arms join
    # the torso above this waist cut, so lower cuffs/hands remain separate.
    for a, b in surface["edges"]:
        if use[a] and use[b]:
            union(a, b)
    tolerance = 3e-6
    cells = defaultdict(list)
    for index in np.flatnonzero(use):
        cell = tuple(np.floor(points[index] / tolerance).astype(int))
        for shift in itertools.product((-1, 0, 1), repeat=3):
            for other in cells.get(tuple(a + b for a, b in zip(cell, shift, strict=True)), ()):
                if np.linalg.norm(points[index] - points[other]) <= tolerance:
                    union(index, other)
        cells[cell].append(index)
    roots = np.asarray([find(index) for index in range(len(points))])
    groups, counts = np.unique(roots[use], return_counts=True)
    central = use & (roots == groups[np.argmax(counts)])
    names = [doc["nodes"][int(index)]["name"] for index in surface["skin"]["joints"]]
    arm_slots = np.isin(np.asarray(names)[surface["joints"]], ARM_NAMES)
    arm_weight = np.where(arm_slots, surface["weights"], 0).sum(1)
    selected = np.flatnonzero(central & (arm_weight > 1e-8))
    if len(selected) != 514 or int(central.sum()) != 7111:
        raise ValueError("Pinned lower-body topology selection changed")
    remaining = np.where(arm_slots, 0, surface["weights"])[selected]
    if remaining.sum(1).min() < 0.67:
        raise ValueError("No safe existing body/upper-leg weight remainder")
    recipient_names = set(np.asarray(names)[surface["joints"][selected]][remaining > 0])
    if not recipient_names <= set(RECIPIENT_NAMES):
        raise ValueError("Unexpected recipient influence; reject instead of assigning new joints")
    return (
        surface,
        points,
        selected,
        arm_slots,
        names,
        {
            "method": (
                "static_rest_waist_cut_induced_triangle_component_with_coincident_uv_seam_weld"
            ),
            "waist_cut_y_world": 0.95,
            "seam_weld_tolerance_world": tolerance,
            "central_component_vertex_count": int(central.sum()),
            "selected_vertex_count": len(selected),
            "selected_bounds": {
                "min": points[selected].min(0).tolist(),
                "max": points[selected].max(0).tolist(),
            },
            "nonarm_weight_min_before": float(remaining.sum(1).min()),
            "vertex_indices": selected.tolist(),
            "vertex_indices_sha256": hashlib.sha256(selected.astype("<u4").tobytes()).hexdigest(),
            "scope": (
                "Central lower body including skirt and hidden thigh/skin; not a garment-only "
                "mask. Static selection never samples animation."
            ),
        },
    )


def apply(source: Path, output: Path, parameters: dict):
    if glb.sha(source) != SOURCE_SHA or parameters["source_sha256"] != SOURCE_SHA:
        raise ValueError("Correction applies only to exact recovered candidate")
    doc, binary = glb.read_glb(source)
    original_doc, original_binary = copy.deepcopy(doc), bytes(binary)
    surface, points, selected, arm_slots, names, selection = selected_vertices(doc, binary)
    if selection != parameters["selection"]:
        raise ValueError("Measured static selection differs from frozen parameters")
    if (
        parameters["redistribution"]
        != "remove_arm_chain_and_proportionally_renormalize_existing_body_upper_leg_weights"
    ):
        raise ValueError("Unsupported correction policy")
    attrs = doc["meshes"][doc["nodes"][surface["node_index"]]["mesh"]]["primitives"][
        surface["primitive_index"]
    ]["attributes"]
    if set(key for key in attrs if key.startswith("WEIGHTS_")) != {"WEIGHTS_0"}:
        raise ValueError("Only exact one-set skin layout supported")
    accessor = doc["accessors"][attrs["WEIGHTS_0"]]
    view = doc["bufferViews"][accessor["bufferView"]]
    if (
        accessor["componentType"] != 5126
        or accessor.get("normalized")
        or view.get("byteStride", 16) != 16
    ):
        raise ValueError("Pinned float32 weight layout changed")
    before = glb.accessor(doc, binary, attrs["WEIGHTS_0"])
    after = before.copy()
    retained = np.where(arm_slots[selected], 0, before[selected]).astype(float)
    retained *= before[selected].sum(1, dtype=float)[:, None] / retained.sum(1)[:, None]
    after[selected] = retained.astype("<f4")
    if np.any(after < 0) or np.max(np.abs(after.sum(1) - 1)) > 1e-6:
        raise ValueError("Corrected weights are not normalized")
    byte_offset = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
    packed = np.ndarray(before.shape, dtype="<f4", buffer=binary, offset=byte_offset)
    packed[selected] = after[selected]
    allowed_bytes = np.zeros(len(binary), dtype=bool)
    for index in selected:
        allowed_bytes[byte_offset + int(index) * 16 : byte_offset + (int(index) + 1) * 16] = True
    changed = np.frombuffer(original_binary, dtype="u1") != np.frombuffer(binary, dtype="u1")
    if np.any(changed & ~allowed_bytes) or doc != original_doc:
        raise ValueError("Correction modified data outside selected weight rows")
    # This runs all existing default surface/UV/connectivity/appearance gates.
    preservation, _ = rig.audit_preservation(
        original_doc, original_binary, doc, binary, {"transform": np.eye(4).tolist()}
    )
    output.mkdir(parents=True, exist_ok=False)
    glb.save_glb(output / "model.glb", doc, binary)
    audit = {
        "schema_version": 1,
        "source_sha256": SOURCE_SHA,
        "output_sha256": glb.sha(output / "model.glb"),
        "selection": selection,
        "changed_vertex_count": int(np.any(before != after, axis=1).sum()),
        "changed_weight_slot_count": int((before != after).sum()),
        "changed_joint_index_count": 0,
        "joint_weight_changes": [],
        "rest_surface_default_audit": preservation,
        "json_document_unchanged": doc == original_doc,
        "all_nonweight_binary_bytes_unchanged": True,
        "animation_tracks_and_amplitudes_unchanged": True,
        "mesh_positions_normals_uv_triangles_materials_textures_unchanged": True,
        "description": (
            "Explicit local lower-body weight correction of provider-authored rig; "
            "not an unmodified provider rig."
        ),
    }
    for joint, name in enumerate(names):
        old = np.where(surface["joints"][selected] == joint, before[selected], 0).sum(1)
        new = np.where(surface["joints"][selected] == joint, after[selected], 0).sum(1)
        if np.any(old != new):
            audit["joint_weight_changes"].append(
                {
                    "joint": name,
                    "changed_vertices": int((old != new).sum()),
                    "mass_before": float(old.sum()),
                    "mass_after": float(new.sum()),
                }
            )
    (output / "audit.json").write_text(json.dumps(audit, indent=2) + "\n")
    # Separate diagnostic visualization only, never the delivered corrected rig.
    mask_doc, mask_bin = copy.deepcopy(original_doc), bytearray(original_binary)
    colors = np.tile([0.28, 0.32, 0.38, 1.0], (len(points), 1))
    colors[selected] = [1.0, 0.03, 0.02, 1.0]
    mask_attrs = mask_doc["meshes"][mask_doc["nodes"][surface["node_index"]]["mesh"]]["primitives"][
        surface["primitive_index"]
    ]["attributes"]
    mask_attrs["COLOR_0"] = glb.append_accessor(mask_doc, mask_bin, colors, 5126, "VEC4")
    mask_doc["materials"] = [
        {
            "name": "diagnostic_selection_only",
            "doubleSided": True,
            "pbrMetallicRoughness": {
                "baseColorFactor": [1, 1, 1, 1],
                "metallicFactor": 0,
                "roughnessFactor": 1,
            },
            "extensions": {"KHR_materials_unlit": {}},
        }
    ]
    for mesh in mask_doc["meshes"]:
        for primitive in mesh["primitives"]:
            primitive["material"] = 0
    mask_doc["extensionsUsed"] = sorted(
        set(mask_doc.get("extensionsUsed", [])) | {"KHR_materials_unlit"}
    )
    mask_doc.pop("extensionsRequired", None)
    glb.save_glb(output / "selection-mask.glb", mask_doc, mask_bin)
    return audit
