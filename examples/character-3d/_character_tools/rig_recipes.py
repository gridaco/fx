"""Immutable named edits to a registered recipe, never direct mesh-weight edits."""

from __future__ import annotations

from copy import deepcopy
from typing import Any, Protocol

from _character_tools.io import canonical_digest
from _character_tools.worker import rig_contract


class RecipeCompiler(Protocol):
    def compile_plan(
        self,
        asset: dict[str, Any],
        landmarks: list[dict[str, Any]],
        regions: list[dict[str, Any]],
        profile: dict[str, Any],
    ) -> dict[str, Any]: ...


PRESERVATION = (
    "Untouched recipe fields are preserved. The worker rebuilds hea"
    "t weights, so exported weights and deformation outside the edi"
    "ted region are not guaranteed unchanged. The new export requir"
    "es full independent review."
)


def recipe_inputs(plan: dict[str, Any]) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Recover authorable inputs while retaining region order and omitted defaults."""
    rig_contract.validate_plan(plan)
    landmarks = [
        {
            "joint": name,
            "head": deepcopy(bone["head"]),
            "tail": deepcopy(bone["tail"]),
            "palm_direction": deepcopy(bone.get("palm_direction")),
        }
        for name, bone in plan["bones"].items()
    ]
    regions = [
        {"role": role, **deepcopy(region)}
        for role, binding in plan["bindings"].items()
        for region in binding.get("regions", [])
    ]
    keys = [(item["role"], item["name"]) for item in regions]
    if len(keys) != len(set(keys)):
        raise ValueError("Named recipe editing requires unique region names within each role")
    return (landmarks, regions)


def revise_plan(
    base: dict[str, Any],
    patch: dict[str, Any],
    *,
    compiler: RecipeCompiler,
    source: dict[str, Any],
    profile: dict[str, Any],
) -> dict[str, Any]:
    """Merge an explicit patch and refuse any incidental compiler changes."""
    rig_contract.fields(
        patch,
        {
            "base_asset_id",
            "expected_plan_sha256",
            "landmark_updates",
            "region_updates",
            "region_additions",
            "region_removals",
        },
    )
    if canonical_digest(base) != patch["expected_plan_sha256"]:
        raise ValueError("Base rig plan digest differs from the requested revision")
    if base["source_sha256"] != source["source"]["sha256"]:
        raise ValueError("Base recipe does not bind the admitted assembly")
    landmarks, regions = recipe_inputs(base)
    if compiler.compile_plan(source, landmarks, regions, profile) != base:
        raise ValueError("Base recipe differs from the active profile; use a fresh build")
    for field in ("landmark_updates", "region_updates", "region_additions", "region_removals"):
        if not isinstance(patch[field], list) or len(patch[field]) > 64:
            raise ValueError("Recipe updates must be bounded lists")
    if not any(
        patch[field]
        for field in ("landmark_updates", "region_updates", "region_additions", "region_removals")
    ):
        raise ValueError("Recipe revision needs at least one explicit change")
    indexed_landmarks = {item["joint"]: item for item in landmarks}
    seen_joints = set()
    for item in patch["landmark_updates"]:
        rig_contract.fields(item, {"joint", "head", "tail", "palm_direction"})
        name = item["joint"]
        if not isinstance(name, str) or name not in indexed_landmarks or name in seen_joints:
            raise ValueError("Landmark update names an unknown or duplicate joint")
        seen_joints.add(name)
        indexed_landmarks[name] = deepcopy(item)
    indexed_regions = {(item["role"], item["name"]): item for item in regions}
    seen_regions = set()
    for operation in ("region_updates", "region_additions", "region_removals"):
        for item in patch[operation]:
            if operation == "region_removals":
                rig_contract.fields(item, {"role", "name"})
            else:
                rig_contract.fields(
                    item,
                    {"role", "name", "bounds", "bones"},
                    {"fade_width", "whole_components", "blend_axis"},
                )
            role, name = (item["role"], item["name"])
            rig_contract.label(role)
            rig_contract.label(name)
            key = (role, name)
            if role not in base["bindings"] or key in seen_regions:
                raise ValueError("Region edit names an unknown role or repeats an edit target")
            if (key in indexed_regions) == (operation == "region_additions"):
                raise ValueError("Region addition must be new; update/removal must already exist")
            seen_regions.add(key)
            if operation == "region_removals":
                del indexed_regions[key]
            else:
                indexed_regions[key] = deepcopy(item)
    if len(indexed_regions) > 64:
        raise ValueError("Merged recipe exceeds the agent region limit")
    result = compiler.compile_plan(
        source, list(indexed_landmarks.values()), list(indexed_regions.values()), profile
    )
    rig_contract.validate_plan(result)
    expected = deepcopy(base)
    for name in seen_joints:
        expected["bones"][name] = deepcopy(result["bones"][name])
    for role in base["bindings"]:
        if any(key[0] == role for key in seen_regions):
            expected["bindings"][role]["regions"] = [
                {
                    key: deepcopy(value)
                    for key, value in item.items()
                    if key != "role" and value is not None
                }
                for item in indexed_regions.values()
                if item["role"] == role
            ]
    if result != expected:
        raise ValueError("Compiler changed untouched recipe fields; use a fresh build")
    if result == base:
        raise ValueError("Recipe revision makes no change")
    return result
