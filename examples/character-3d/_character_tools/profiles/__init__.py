"""Replaceable character requirements and articulation recipes, outside the workflow engine."""

from types import ModuleType
from typing import Any

from _character_tools.profiles import anthro_cat, fixed_human, humanoid


def articulation(profile: dict[str, Any]) -> ModuleType:
    """Resolve an explicit profile before any model call; never guess anatomy."""
    compilers = {
        "sd_human_grouped_hands_v1": humanoid,
        "sd_cat_grouped_paws_v1": anthro_cat,
        "sd_human_fixed_hands_v1": fixed_human,
    }
    try:
        return compilers[profile["profile_id"]]
    except KeyError:
        raise ValueError("No articulation compiler is registered for this profile") from None


def diagnostic_samples(
    profile: dict[str, Any], plan: dict[str, Any]
) -> list[tuple[str | None, float]]:
    """Select declared diagnostics from clips actually compiled into the export."""
    durations = {clip["name"]: clip["duration_seconds"] for clip in plan["clips"]}
    review = profile["review"]
    required = [*review["required_diagnostics"], review["required_motion"]]
    optional = review.get("optional_diagnostics", [])
    samples: list[tuple[str | None, float]] = []
    for name in dict.fromkeys([*required, *optional]):
        if name == "rest":
            samples.append((None, 0))
        elif name in durations:
            samples.append((name, durations[name] * 0.375))
        elif name in required:
            raise ValueError(f"Profile requires an unimplemented diagnostic clip: {name}")
    return samples
