"""One explicit SD humanoid articulation profile; contains no character coordinates."""

from __future__ import annotations

import math
from typing import Any

PARENTS = {
    "pelvis": None,
    "spine_lower": "pelvis",
    "spine_upper": "spine_lower",
    "chest": "spine_upper",
    "neck": "chest",
    "head": "neck",
}
for side in ("left", "right"):
    PARENTS.update(
        {
            f"{side}_shoulder": "chest",
            f"{side}_upper_arm": f"{side}_shoulder",
            f"{side}_forearm": f"{side}_upper_arm",
            f"{side}_hand": f"{side}_forearm",
            f"{side}_fingers_1": f"{side}_hand",
            f"{side}_fingers_2": f"{side}_fingers_1",
            f"{side}_thumb_1": f"{side}_hand",
            f"{side}_thumb_2": f"{side}_thumb_1",
            f"{side}_upper_leg": "pelvis",
            f"{side}_lower_leg": f"{side}_upper_leg",
            f"{side}_foot": f"{side}_lower_leg",
            f"{side}_toes": f"{side}_foot",
        }
    )


def landmarks_to_bones(
    landmarks: list[dict[str, Any]], *, height: float
) -> dict[str, dict[str, Any]]:
    if len(landmarks) != len(PARENTS) or {entry["joint"] for entry in landmarks} != set(PARENTS):
        raise ValueError("Provide each required profile joint exactly once: " + ", ".join(PARENTS))
    result: dict[str, dict[str, Any]] = {}
    for entry in landmarks:
        name = entry["joint"]
        head, tail = (entry["head"], entry["tail"])
        if any(
            type(value) not in {int, float} or not math.isfinite(value) for value in (*head, *tail)
        ):
            raise ValueError("Landmark coordinates must be finite")
        length = math.dist(head, tail)
        if not height * 0.001 <= length <= height * 0.8:
            raise ValueError(f"Joint {name} has implausible length relative to source height")
        bone: dict[str, Any] = {
            "parent": PARENTS[name],
            "head": head,
            "tail": tail,
            "forward_direction": [0, 0, 1],
        }
        palm = entry["palm_direction"]
        if "fingers" in name or "thumb" in name:
            if palm is None:
                raise ValueError(f"Joint {name} requires an observed palm direction")
            bone["palm_direction"] = palm
        result[name] = bone
    for side in ("left", "right"):
        for child, parent in (
            ("forearm", "upper_arm"),
            ("hand", "forearm"),
            ("lower_leg", "upper_leg"),
            ("foot", "lower_leg"),
            ("fingers_2", "fingers_1"),
            ("thumb_2", "thumb_1"),
        ):
            if (
                math.dist(result[f"{side}_{child}"]["head"], result[f"{side}_{parent}"]["tail"])
                > height * 0.01
            ):
                raise ValueError(
                    f"Profile chain {side}_{parent} → {side}_{child} must meet within 1% of height"
                )
    return result


def motion_clips() -> list[dict[str, Any]]:
    """Neutral diagnostics and a restrained cheer; no imported dance is substituted."""

    def track(
        bone: str,
        angle: float,
        *,
        duration: float = 2.0,
        axis: list[float] | None = None,
        curl: bool = False,
    ) -> dict[str, Any]:
        value: dict[str, Any] = {
            "bone": bone,
            "keyframes": [
                [0, 0],
                [duration * 0.25, angle],
                [duration * 0.75, angle],
                [duration, 0],
            ],
        }
        value.update({"kind": "curl"} if curl else {"axis_world": axis})
        return value

    def clip(name: str, tracks: list[dict[str, Any]], duration: float = 2.0) -> dict[str, Any]:
        return {"name": name, "duration_seconds": duration, "fps": 24, "tracks": tracks}

    shoulder, elbows, knees, hands = ([], [], [], [])
    for side, sign in (("left", 1), ("right", -1)):
        shoulder.append(track(f"{side}_upper_arm", sign * 55, axis=[0, 0, 1]))
        elbows.append(track(f"{side}_forearm", -65, axis=[1, 0, 0]))
        knees.append(track(f"{side}_lower_leg", 55, axis=[1, 0, 0]))
        for part, angle in (("fingers_1", 35), ("fingers_2", 40), ("thumb_1", 20), ("thumb_2", 25)):
            hands.append(track(f"{side}_{part}", angle, curl=True))
    cheer = []
    for side, sign in (("left", 1), ("right", -1)):
        cheer.extend(
            [
                track(f"{side}_upper_arm", sign * 65, duration=4, axis=[0, 0, 1]),
                track(f"{side}_forearm", -25, duration=4, axis=[1, 0, 0]),
            ]
        )
    cheer.append(
        {
            "bone": "chest",
            "axis_world": [0, 0, 1],
            "keyframes": [[0, 0], [1, 5], [2, -5], [3, 5], [4, 0]],
        }
    )
    return [
        clip("shoulder_raise", shoulder),
        clip("elbow_bend", elbows),
        clip("knee_bend", knees),
        clip("hand_curl", hands),
        clip("cheer", cheer, 4),
    ]


def compile_plan(
    asset: dict[str, Any],
    landmarks: list[dict[str, Any]],
    regions: list[dict[str, Any]],
    profile: dict[str, Any],
) -> dict[str, Any]:
    height = asset["inventory"]["bounds"]["dimensions"][1]
    bones = landmarks_to_bones(landmarks, height=height)
    grouped: dict[str, list[dict[str, Any]]] = {
        role: [] for role in set(asset["part_roles"].values())
    }
    for region in regions:
        if region["role"] not in grouped:
            raise ValueError("Binding region names an undeclared part role")
        spec = {key: value for key, value in region.items() if key != "role" and value is not None}
        grouped[region["role"]].append(spec)
    bindings = {}
    for role in grouped:
        policy = profile["part_binding"][role]
        if policy == "deform":
            bindings[role] = {
                "method": "heat",
                "bones": [name for name in bones if name != "head"],
                "regions": grouped[role],
            }
        elif policy == "head_attachment":
            bindings[role] = {"method": "rigid", "bones": ["head"], "regions": grouped[role]}
        else:
            raise ValueError("This humanoid profile does not implement that part binding")
    surfaces = profile["surface_policy"]
    return {
        "schema_version": 1,
        "source_sha256": asset["source"]["sha256"],
        "coordinate_system": "gltf_y_up_z_front",
        "bones": bones,
        "part_roles": asset["part_roles"],
        "bindings": bindings,
        "materials": {
            "default": surfaces["default"],
            "by_role": {role: surfaces[role] for role in grouped if role in surfaces},
        },
        "clips": motion_clips(),
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
