"""Recipe-owned partition plans, independent of mesh object names or bone counts."""

from __future__ import annotations

import copy
from typing import Any

from _character_tools.io import canonical_digest

CORE_REGIONS = ("head", "neck", "torso", "arms", "hands", "legs", "feet", "outfit")
PRESETS: dict[str, dict[str, tuple[str, ...]]] = {
    "whole": {"character": (*CORE_REGIONS, "hair")},
    "head_body_hair": {
        "body": ("neck", "torso", "arms", "hands", "legs", "feet", "outfit"),
        "head": ("head",),
        "hair": ("hair",),
    },
}


def partition_plan(preset: str) -> dict[str, Any]:
    if not isinstance(preset, str) or preset not in PRESETS:
        raise ValueError("Unsupported v1 partition; long hair, hands and animal parts are deferred")
    groups = {name: list(regions) for name, regions in PRESETS[preset].items()}
    body = {
        "schema_version": 1,
        "preset": preset,
        "generation_groups": groups,
        "rig_input_groups": list(groups),
        "assembly_stage": "orient_whole" if preset == "whole" else "before_rig",
        "post_rig_attachments": [],
        "scope": "short_haired_sd_human_fixed_hands",
        "qualification_status": "development_only",
    }
    return {**body, "plan_sha256": canonical_digest(body)}


def apply_partition(profile: dict[str, Any], preset: str) -> dict[str, Any]:
    if profile.get("profile_id") != "sd_human_fixed_hands_v1":
        raise ValueError("Partition override requires the explicit fixed-hand provider profile")
    result = copy.deepcopy(profile)
    plan = partition_plan(preset)
    result["partition"] = plan
    result["required_parts"] = list(plan["generation_groups"])
    result["part_binding"] = {name: "provider_skin" for name in result["required_parts"]}
    if preset == "whole":
        result["review"]["assembly_criteria"] = [
            "forward_orientation",
            "reference_proportions",
            "face_visibility",
            "texture_integrity",
        ]
    return result
