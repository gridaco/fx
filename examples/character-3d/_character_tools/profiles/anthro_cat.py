"""Explicit biped cat articulation; all character landmarks arrive from inspection."""

from __future__ import annotations

import copy
import math
from itertools import pairwise
from typing import Any

from _character_tools.worker import rig_contract

PROFILE_ID = "sd_cat_grouped_paws_v1"
PARENTS = {
    "pelvis": None,
    "spine_lower": "pelvis",
    "spine_upper": "spine_lower",
    "chest": "spine_upper",
    "neck": "chest",
    "head": "neck",
}
for _side in ("left", "right"):
    PARENTS.update(
        {
            f"{_side}_shoulder": "chest",
            f"{_side}_upper_arm": f"{_side}_shoulder",
            f"{_side}_forearm": f"{_side}_upper_arm",
            f"{_side}_paw": f"{_side}_forearm",
            f"{_side}_paw_1": f"{_side}_paw",
            f"{_side}_paw_2": f"{_side}_paw_1",
            f"{_side}_upper_leg": "pelvis",
            f"{_side}_lower_leg": f"{_side}_upper_leg",
            f"{_side}_foot": f"{_side}_lower_leg",
            f"{_side}_toes": f"{_side}_foot",
        }
    )
PARENTS.update({"tail_1": "pelvis", "tail_2": "tail_1", "tail_3": "tail_2"})
TAIL_BONES = ("tail_1", "tail_2", "tail_3")
PAW_CURL_BONES = tuple(f"{side}_paw_{index}" for side in ("left", "right") for index in (1, 2))
MOTION_SEMANTICS = {
    "shoulder_raise": (
        "Bilateral forelimb raise for shoulder attachment and deformation inspection."
    ),
    "elbow_bend": "Forelimb bend for elbow deformation inspection.",
    "knee_bend": "Biped hindlimb bend for knee deformation inspection.",
    "paw_curl": (
        "Soft grouped paw squeeze toward the observed palm; no opposing thumb is required."
    ),
    "cheer": ("Restrained biped cheer with soft paw squeeze and optional tail wag."),
    "tail_wag": ("Optional attached tail-chain wag; inspect root and curved surface stability."),
}


def _vector(value: object, label: str) -> list[float]:
    if (
        not isinstance(value, list)
        or len(value) != 3
        or any(type(item) not in {int, float} or not math.isfinite(item) for item in value)
    ):
        raise ValueError(f"{label} must contain three finite coordinates")
    return list(value)


def _cross(left: list[float], right: list[float]) -> list[float]:
    return [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]


def landmarks_to_bones(
    landmarks: list[dict[str, Any]], *, height: float
) -> dict[str, dict[str, Any]]:
    if type(height) not in {int, float} or not math.isfinite(height) or height <= 0:
        raise ValueError("Source height must be positive and finite")
    if not isinstance(landmarks, list) or len(landmarks) != len(PARENTS):
        raise ValueError("Provide exactly one landmark for every required cat joint")
    result: dict[str, dict[str, Any]] = {}
    for entry in landmarks:
        if not isinstance(entry, dict) or set(entry) != {"joint", "head", "tail", "palm_direction"}:
            raise ValueError("Landmarks require joint, head, tail and nullable palm_direction")
        name = entry["joint"]
        if not isinstance(name, str) or name not in PARENTS or name in result:
            raise ValueError("Unknown or duplicate cat joint")
        head = _vector(entry["head"], name + " head")
        tail = _vector(entry["tail"], name + " tail")
        if not height * 0.001 <= math.dist(head, tail) <= height * 0.8:
            raise ValueError(f"Joint {name} has implausible length relative to measured height")
        bone: dict[str, Any] = {
            "parent": PARENTS[name],
            "head": head,
            "tail": tail,
            "forward_direction": [0, 0, 1],
        }
        if name in PAW_CURL_BONES:
            palm = _vector(entry["palm_direction"], name + " observed palm direction")
            direction = [end - start for start, end in zip(head, tail, strict=True)]
            cross = _cross(direction, palm)
            if (
                math.hypot(*palm) <= 1e-08
                or math.hypot(*cross) <= math.hypot(*direction) * math.hypot(*palm) * 1e-06
            ):
                raise ValueError(f"Joint {name} needs an observed nonparallel palm direction")
            bone["palm_direction"] = palm
        elif entry["palm_direction"] is not None:
            raise ValueError("Only grouped paw curl controls use palm_direction in this profile")
        result[name] = bone
    for side in ("left", "right"):
        for child, parent in (
            ("forearm", "upper_arm"),
            ("paw", "forearm"),
            ("paw_1", "paw"),
            ("paw_2", "paw_1"),
            ("lower_leg", "upper_leg"),
            ("foot", "lower_leg"),
            ("toes", "foot"),
        ):
            if (
                math.dist(result[f"{side}_{child}"]["head"], result[f"{side}_{parent}"]["tail"])
                > height * 0.01
            ):
                raise ValueError(
                    f"Cat chain {side}_{parent} to {side}_{child} must meet within 1% of height"
                )
    for parent, child in pairwise(TAIL_BONES):
        if math.dist(result[parent]["tail"], result[child]["head"]) > 1e-06:
            raise ValueError("Tail chain endpoints must meet within 1e-6 scene units")
    return {name: result[name] for name in PARENTS}


def _tail_wag_axis(bone: dict[str, Any]) -> list[float]:
    direction = [end - start for start, end in zip(bone["head"], bone["tail"], strict=True)]
    length = math.hypot(*direction)
    unit = [item / length for item in direction]
    target: list[float] = [1, 0, 0] if abs(unit[0]) < 0.95 else [0, 0, 1]
    axis = _cross(unit, target)
    magnitude = math.hypot(*axis)
    return [item / magnitude for item in axis]


def motion_clips(
    bones: dict[str, dict[str, Any]], *, tail_wag: bool = True
) -> list[dict[str, Any]]:
    """Modest diagnostic motion; tail axes follow the supplied measured segments."""
    if type(tail_wag) is not bool:
        raise ValueError("tail_wag must be boolean")

    def track(
        bone: str,
        angle: float,
        *,
        duration: float = 2.0,
        axis: list[float] | None = None,
        curl: bool = False,
    ) -> dict[str, Any]:
        item: dict[str, Any] = {
            "bone": bone,
            "keyframes": [
                [0, 0],
                [duration * 0.25, angle],
                [duration * 0.75, angle],
                [duration, 0],
            ],
        }
        item.update({"kind": "curl"} if curl else {"axis_world": axis})
        return item

    def clip(name: str, tracks: list[dict[str, Any]], duration: float = 2.0) -> dict[str, Any]:
        return {"name": name, "duration_seconds": duration, "fps": 24, "tracks": tracks}

    shoulders, elbows, knees, paws, cheer = ([], [], [], [], [])
    for side, sign in (("left", 1), ("right", -1)):
        shoulders.append(track(f"{side}_upper_arm", sign * 45, axis=[0, 0, 1]))
        elbows.append(track(f"{side}_forearm", -35, axis=[1, 0, 0]))
        knees.append(track(f"{side}_lower_leg", 35, axis=[1, 0, 0]))
        cheer.extend(
            [
                track(f"{side}_upper_arm", sign * 55, duration=4, axis=[0, 0, 1]),
                track(f"{side}_forearm", -18, duration=4, axis=[1, 0, 0]),
            ]
        )
        for index, angle in ((1, 16), (2, 20)):
            paws.append(track(f"{side}_paw_{index}", angle, curl=True))
            cheer.append(track(f"{side}_paw_{index}", angle, duration=4, curl=True))
    cheer.append(
        {
            "bone": "chest",
            "axis_world": [0, 0, 1],
            "keyframes": [[0, 0], [1, 3], [2, -3], [3, 3], [4, 0]],
        }
    )
    wag = []
    if tail_wag:
        for index, angle in enumerate((8, 10, 8)):
            name = TAIL_BONES[index]
            wag.append(
                {
                    "bone": name,
                    "axis_world": _tail_wag_axis(bones[name]),
                    "keyframes": [[0, 0], [1, angle], [2, -angle], [3, angle], [4, 0]],
                }
            )
        cheer.extend(copy.deepcopy(wag))
    clips = [
        clip("shoulder_raise", shoulders),
        clip("elbow_bend", elbows),
        clip("knee_bend", knees),
        clip("paw_curl", paws),
        clip("cheer", cheer, 4),
    ]
    if wag:
        clips.append(clip("tail_wag", wag, 4))
    return clips


def compile_plan(
    asset: dict[str, Any],
    landmarks: list[dict[str, Any]],
    regions: list[dict[str, Any]],
    profile: dict[str, Any],
) -> dict[str, Any]:
    if profile.get("profile_id") != PROFILE_ID:
        raise ValueError("Cat compiler requires its matching profile_id")
    if profile.get("coordinate_system") != "gltf_y_up_z_front":
        raise ValueError("Cat profile requires canonical glTF coordinates")
    roles = asset["part_roles"]
    expected_roles = {"body", "head", "tail"}
    if set(roles.values()) != expected_roles or set(profile["required_parts"]) != expected_roles:
        raise ValueError("This cat profile requires exactly body, head and tail roles")
    if profile["part_binding"] != {
        "body": "deform",
        "head": "head_attachment",
        "tail": "tail_chain",
    }:
        raise ValueError("Unsupported cat part-binding policy")
    height = asset["inventory"]["bounds"]["dimensions"][1]
    bones = landmarks_to_bones(landmarks, height=height)
    body_bones = [name for name in bones if name != "head" and name not in TAIL_BONES]
    region_bones = {
        "body": set(body_bones),
        "head": {"head", "neck", "chest"},
        "tail": set(TAIL_BONES),
    }
    grouped: dict[str, list[dict[str, Any]]] = {role: [] for role in sorted(expected_roles)}
    if not isinstance(regions, list) or len(regions) > 128:
        raise ValueError("Binding regions must be a bounded list")
    for region in regions:
        if not isinstance(region, dict) or region.get("role") not in grouped:
            raise ValueError("Binding region names an undeclared part role")
        role = region["role"]
        if not isinstance(region.get("bones"), list) or any(
            name not in region_bones[role] for name in region["bones"]
        ):
            raise ValueError(
                "Binding region crosses the profile's body/head/tail ownership boundary"
            )
        grouped[role].append(
            copy.deepcopy(
                {key: value for key, value in region.items() if key != "role" and value is not None}
            )
        )
    surfaces = profile["surface_policy"]
    plan = {
        "schema_version": 1,
        "source_sha256": asset["source"]["sha256"],
        "coordinate_system": "gltf_y_up_z_front",
        "bones": bones,
        "part_roles": copy.deepcopy(roles),
        "bindings": {
            "body": {"method": "heat", "bones": body_bones, "regions": grouped["body"]},
            "head": {"method": "rigid", "bones": ["head"], "regions": grouped["head"]},
            "tail": {"method": "chain", "bones": list(TAIL_BONES), "regions": grouped["tail"]},
        },
        "materials": {
            "default": surfaces["default"],
            "by_role": {role: surfaces[role] for role in grouped if role in surfaces},
        },
        "clips": motion_clips(bones, tail_wag=profile["articulation"]["tail_wag"]),
        "grounding": {
            "root_bone": "pelvis",
            "support_bones": ["left_foot", "left_toes", "right_foot", "right_toes"],
            "support_roles": ["body"],
            "height_fraction": 0.1,
            "min_weight": 0.3,
            "ground_height": 0,
            "max_correction_fraction": 0.15,
        },
    }
    return rig_contract.validate_plan(plan)
