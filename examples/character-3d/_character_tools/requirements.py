"""Offline validation of a profile's review and part requirements, and of an export's space."""

from __future__ import annotations

import math
from typing import Any

from _character_tools.profiles import (
    anthro_cat,
    articulation,
    diagnostic_samples,
    fixed_human,
    humanoid,
)
from _character_tools.studio import VIEWS

DEFAULT_REVIEW_VIEWS = ("front", "back", "left", "right", "three_quarter")
MAX_REVIEW_IMAGES = 60


def _compiled_motion_inventory(profile: dict[str, Any]) -> dict[str, Any]:
    """Ask the existing compiler for its authored clip set before any rig build."""
    compiler = articulation(profile)
    if compiler in (humanoid, fixed_human):
        return {"clips": compiler.motion_clips()}
    if compiler is anthro_cat:
        neutral_tail = {
            name: {"head": [0, 0, 0], "tail": [0, 0, 1]} for name in compiler.TAIL_BONES
        }
        articulation_settings = profile.get("articulation", {})
        if not isinstance(articulation_settings, dict) or "tail_wag" not in articulation_settings:
            raise ValueError("Cat profile must explicitly configure tail_wag")
        return {
            "clips": compiler.motion_clips(neutral_tail, tail_wag=articulation_settings["tail_wag"])
        }
    raise ValueError("Registered profile lacks an offline motion inventory adapter")


def _validate_evidence_limit(review: dict[str, Any], pose_count: int) -> None:
    count = len(review.get("required_views", DEFAULT_REVIEW_VIEWS)) * (
        pose_count + len(review.get("target_character_height_pixels", []))
    )
    if count > MAX_REVIEW_IMAGES:
        raise ValueError(
            "Mandatory evidence exceeds the bounded review image allowance before dispatch"
        )


def names(value: object, label: str) -> list[str]:
    if (
        not isinstance(value, list)
        or not value
        or any(not isinstance(item, str) or not item.strip() for item in value)
        or (len(set(value)) != len(value))
    ):
        raise ValueError(f"{label} must be a nonempty list of unique names")
    return value


def validate_requirements(profile: dict[str, Any], experiment: dict[str, Any]) -> None:
    if (
        experiment.get("rigging", {}).get("strategy") == "provider"
        and profile.get("profile_id") != "sd_human_fixed_hands_v1"
    ):
        raise ValueError("Provider rigging v1 supports only the fixed-hand SD human profile")
    if profile.get("profile_id") == "sd_human_fixed_hands_v1":
        target_height = profile.get("target_height")
        if (
            isinstance(target_height, bool)
            or not isinstance(target_height, int | float)
            or (not math.isfinite(target_height))
            or (target_height <= 0)
        ):
            raise ValueError("Fixed-hand profile target_height must be finite and positive")
        required_controls = names(
            profile.get("required_joint_semantics"), "Required joint semantics"
        )
        if set(required_controls) != set(fixed_human.PARENTS):
            raise ValueError("Fixed-hand profile control inventory differs from its version")
        if experiment.get("rigging", {}).get("strategy") != "provider":
            raise ValueError("Fixed-hand v1 requires the configured provider rig strategy")
    review = profile.get("review", {})
    if not isinstance(review, dict):
        raise ValueError("Profile review must be an object")
    for key in ("assembly_criteria", "rig_criteria", "required_views", "required_diagnostics"):
        if key in review:
            names(review[key], key)
    if set(review.get("required_views", [])) - VIEWS.keys():
        raise ValueError("Profile requests an unsupported review view")
    sizes = review.get("target_character_height_pixels", [])
    if "target_character_height_pixels" in review and (not isinstance(sizes, list) or not sizes):
        raise ValueError("Gameplay review sizes must be a nonempty unique list")
    for size in [*sizes, review.get("inspection_height_pixels", 512)]:
        if type(size) is not int or not 64 <= size <= 1024:
            raise ValueError("Review character height must be an integer from 64 through 1024")
    if len(set(sizes)) != len(sizes):
        raise ValueError("Gameplay review sizes must be a nonempty unique list")
    if "required_parts" in profile:
        required = set(names(profile["required_parts"], "required_parts"))
        if experiment.get("pipeline_mode") == "brief_to_rig":
            roles = required
        else:
            roles = {part["role"] for part in experiment["parts"]}
        if not required <= roles:
            raise ValueError(
                "Inputs are missing required part roles: " + ", ".join(sorted(required - roles))
            )
    _validate_evidence_limit(review, 1)
    if experiment.get("pipeline_mode", "assembly") == "assembly":
        return
    names(review.get("required_diagnostics"), "required_diagnostics")
    motion = review.get("required_motion")
    if not isinstance(motion, str) or not motion.strip():
        raise ValueError("Rig review needs one required_motion clip")
    optional = review.get("optional_diagnostics", [])
    if not isinstance(optional, list):
        raise ValueError("optional_diagnostics must be a list")
    if optional:
        names(optional, "optional_diagnostics")
    plan = _compiled_motion_inventory(profile)
    if motion not in {clip["name"] for clip in plan["clips"]}:
        raise ValueError("Profile requires an unimplemented motion clip: " + motion)
    poses = diagnostic_samples(profile, plan)
    _validate_evidence_limit(review, len(poses))


def validate_export_space(inventory: dict[str, Any], target_height: float) -> None:
    """The exported character stands at the profile's height with its feet on the ground."""

    bounds = inventory["bounds"]
    height = float(bounds["dimensions"][1])
    ground = float(bounds["min"][1])
    tolerance = max(1e-06, target_height * 1e-05)
    if (
        not math.isfinite(height)
        or not math.isfinite(ground)
        or abs(height - target_height) > tolerance
        or abs(ground) > tolerance
    ):
        raise ValueError("Exported provider rig violates profile height or ground placement")
