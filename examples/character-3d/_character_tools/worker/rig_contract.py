"""Strict, provider-free plans for arbitrary explicitly measured skeletons."""

from __future__ import annotations

import copy
import math
import re
from collections.abc import Collection
from typing import Any


def fields(value: object, required: Collection[str], optional: Collection[str] = ()) -> None:
    if (
        not isinstance(value, dict)
        or not set(required) <= value.keys()
        or value.keys() - set(required) - set(optional)
    ):
        raise ValueError(f"Expected fields {sorted(required)}; optional {sorted(optional)}")


def number(value: object) -> float:
    if isinstance(value, bool) or not isinstance(value, (float, int)) or (not math.isfinite(value)):
        raise ValueError("Expected finite numeric value")
    return float(value)


def vector(value: object) -> list[float]:
    if not isinstance(value, list) or len(value) != 3:
        raise ValueError("Expected three-component vector")
    return [number(item) for item in value]


def direction(value: object) -> list[float]:
    result = vector(value)
    if sum(item * item for item in result) < 1e-16:
        raise ValueError("Direction cannot be zero")
    return result


def label(value: object) -> str:
    if not isinstance(value, str) or not 1 <= len(value) <= 160 or any(ord(c) < 32 for c in value):
        raise ValueError("Expected nonempty bounded label without control characters")
    return value


def bone_list(value: object, bones: Collection[str]) -> None:
    if (
        not isinstance(value, list)
        or not value
        or len(set(value)) != len(value)
        or any(name not in bones for name in value)
    ):
        raise ValueError("Expected nonempty unique existing bone names")


def validate_plan(plan: dict[str, Any]) -> dict[str, Any]:
    fields(
        plan,
        {
            "schema_version",
            "source_sha256",
            "coordinate_system",
            "bones",
            "part_roles",
            "bindings",
            "materials",
            "clips",
        },
        {"grounding"},
    )
    if type(plan["schema_version"]) is not int or plan["schema_version"] != 1:
        raise ValueError("Unsupported rig plan version")
    if not isinstance(plan["source_sha256"], str) or not re.fullmatch(
        "[a-f0-9]{64}", plan["source_sha256"]
    ):
        raise ValueError("Rig plan requires exact lowercase source SHA-256")
    if plan["coordinate_system"] != "gltf_y_up_z_front":
        raise ValueError("Rig coordinates must be gltf_y_up_z_front")
    bones = plan["bones"]
    if not isinstance(bones, dict) or not 1 <= len(bones) <= 256:
        raise ValueError("Expected one through 256 explicit bones")
    for name, bone in bones.items():
        label(name)
        fields(bone, {"parent", "head", "tail"}, {"forward_direction", "palm_direction"})
        if bone["parent"] is not None and bone["parent"] not in bones:
            raise ValueError("Bone parent is missing")
        head, tail = (vector(bone["head"]), vector(bone["tail"]))
        if sum(((a - b) ** 2 for a, b in zip(head, tail, strict=True))) <= 1e-16:
            raise ValueError("Bone has zero length")
        for key in ("forward_direction", "palm_direction"):
            if key in bone:
                direction(bone[key])
    ordered: dict[str, dict[str, Any]] = {}
    while len(ordered) < len(bones):
        additions = {
            name: spec
            for name, spec in bones.items()
            if name not in ordered and (spec["parent"] is None or spec["parent"] in ordered)
        }
        if not additions:
            raise ValueError("Bone hierarchy contains a cycle")
        ordered.update(additions)
    roles = plan["part_roles"]
    if not isinstance(roles, dict) or not roles:
        raise ValueError("Every mesh node needs an explicit role")
    for name, role in roles.items():
        label(name)
        label(role)
    if not isinstance(plan["bindings"], dict) or set(plan["bindings"]) != set(roles.values()):
        raise ValueError("Bindings must cover exactly the declared roles")
    for binding in plan["bindings"].values():
        fields(binding, {"method", "bones"}, {"regions"})
        method = binding["method"]
        bone_list(binding["bones"], bones)
        if method not in {"rigid", "heat", "chain"}:
            raise ValueError("Unsupported binding method")
        if method == "rigid" and len(binding["bones"]) != 1:
            raise ValueError("Rigid binding requires exactly one bone")
        if method == "chain":
            if len(binding["bones"]) < 2:
                raise ValueError("Chain binding requires at least two bones")
            for left, right in zip(binding["bones"][:-1], binding["bones"][1:], strict=True):
                if (
                    bones[right]["parent"] != left
                    or sum(
                        (
                            (a - b) ** 2
                            for a, b in zip(bones[left]["tail"], bones[right]["head"], strict=True)
                        )
                    )
                    > 1e-12
                ):
                    raise ValueError("Chain must have connected sequential bones")
        regions = binding.get("regions", [])
        if not isinstance(regions, list) or len(regions) > 128:
            raise ValueError("Too many binding regions")
        for region in regions:
            fields(
                region,
                {"name", "bounds", "bones"},
                {"fade_width", "whole_components", "blend_axis"},
            )
            label(region["name"])
            bone_list(region["bones"], bones)
            bounds = region["bounds"]
            if not isinstance(bounds, list) or len(bounds) != 2:
                raise ValueError("Region needs lower and upper bounds")
            lower, upper = (vector(bounds[0]), vector(bounds[1]))
            if any((a >= b for a, b in zip(lower, upper, strict=True))):
                raise ValueError("Region bounds must have positive dimensions")
            if number(region.get("fade_width", 0)) < 0:
                raise ValueError("Region fade must be nonnegative")
            if type(region.get("whole_components", False)) is not bool:
                raise ValueError("whole_components must be boolean")
            if "blend_axis" in region:
                axis = region["blend_axis"]
                fields(axis, {"axis", "start", "end", "reverse"})
                if (
                    type(axis["axis"]) is not int
                    or axis["axis"] not in {0, 1, 2}
                    or type(axis["reverse"]) is not bool
                ):
                    raise ValueError("Invalid region blend axis")
                if number(axis["end"]) <= number(axis["start"]):
                    raise ValueError("Blend transition must be increasing")
    materials = plan["materials"]
    fields(materials, {"default"}, {"by_role"})
    policies = {"preserve", "matte", "unlit"}
    if materials["default"] not in policies:
        raise ValueError("Unsupported material policy")
    overrides = materials.get("by_role", {})
    if (
        not isinstance(overrides, dict)
        or overrides.keys() - set(roles.values())
        or any(value not in policies for value in overrides.values())
    ):
        raise ValueError("Invalid role material override")
    clips = plan["clips"]
    if not isinstance(clips, list) or not 1 <= len(clips) <= 16:
        raise ValueError("Expected one through 16 clips")
    names = set()
    for clip in clips:
        fields(clip, {"name", "duration_seconds", "fps", "tracks"})
        name = label(clip["name"])
        if name in names:
            raise ValueError("Clip names must be unique")
        names.add(name)
        duration = number(clip["duration_seconds"])
        if not 0 < duration <= 120 or type(clip["fps"]) is not int or (not 1 <= clip["fps"] <= 120):
            raise ValueError("Clip duration/FPS exceeds supported limits")
        if duration * clip["fps"] > 7200:
            raise ValueError("Clip sample count exceeds 7200")
        if not isinstance(clip["tracks"], list) or not 1 <= len(clip["tracks"]) <= 512:
            raise ValueError("Expected bounded nonempty rotation tracks")
        for track in clip["tracks"]:
            fields(track, {"bone", "keyframes"}, {"axis_world", "kind"})
            if track["bone"] not in bones or ("axis_world" in track) == ("kind" in track):
                raise ValueError("Track needs existing bone and exactly one rotation mode")
            if "axis_world" in track:
                direction(track["axis_world"])
            elif track["kind"] != "curl" or "palm_direction" not in bones[track["bone"]]:
                raise ValueError("Curl requires an explicitly labeled palm direction")
            if track.get("kind") == "curl":
                bone = bones[track["bone"]]
                axis = [b - a for a, b in zip(bone["head"], bone["tail"], strict=True)]
                palm = bone["palm_direction"]
                cross = [
                    axis[1] * palm[2] - axis[2] * palm[1],
                    axis[2] * palm[0] - axis[0] * palm[2],
                    axis[0] * palm[1] - axis[1] * palm[0],
                ]
                direction(cross)
            keys = track["keyframes"]
            if not isinstance(keys, list) or not 2 <= len(keys) <= 256:
                raise ValueError("Track requires two through 256 explicit keyframes")
            previous = -1.0
            for key in keys:
                if not isinstance(key, list) or len(key) != 2:
                    raise ValueError("Keyframe is [seconds, degrees]")
                time, angle = (number(key[0]), number(key[1]))
                if not 0 <= time <= duration or time <= previous or abs(angle) > 720:
                    raise ValueError("Invalid keyframe time or angle")
                previous = time
            if keys[0][0] != 0 or abs(keys[-1][0] - duration) > 1e-08:
                raise ValueError("Tracks must explicitly cover the complete clip")
    if "grounding" in plan:
        ground = plan["grounding"]
        fields(
            ground,
            {
                "root_bone",
                "support_bones",
                "support_roles",
                "height_fraction",
                "min_weight",
                "ground_height",
                "max_correction_fraction",
            },
        )
        if ground["root_bone"] not in bones or bones[ground["root_bone"]]["parent"] is not None:
            raise ValueError("Ground translation requires a root bone")
        bone_list(ground["support_bones"], bones)
        for name in ground["support_bones"]:
            current = name
            while bones[current]["parent"] is not None:
                current = bones[current]["parent"]
            if current != ground["root_bone"]:
                raise ValueError("Support bones must descend from the translated root")
        if (
            not isinstance(ground["support_roles"], list)
            or not ground["support_roles"]
            or any(role not in roles.values() for role in ground["support_roles"])
        ):
            raise ValueError("Grounding requires existing support roles")
        if (
            not 0 < number(ground["height_fraction"]) <= 0.5
            or not 0 < number(ground["min_weight"]) <= 1
        ):
            raise ValueError("Invalid support selection thresholds")
        number(ground["ground_height"])
        if not 0 < number(ground["max_correction_fraction"]) <= 1:
            raise ValueError(
                "Ground correction cap must be positive and no more than source height"
            )
    result = copy.deepcopy(plan)
    result["bones"] = copy.deepcopy(ordered)
    return result
