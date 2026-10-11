"""Required provider controls and clip inventory; no body-rig authoring recipe."""

from typing import Any, NoReturn

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
            f"{side}_upper_leg": "pelvis",
            f"{side}_lower_leg": f"{side}_upper_leg",
            f"{side}_foot": f"{side}_lower_leg",
            f"{side}_toes": f"{side}_foot",
        }
    )


def motion_clips() -> list[dict[str, Any]]:
    """Required inventory only; the motion adapter authors and reports actual tracks."""
    return [
        {"name": name, "duration_seconds": 4 if name == "cheer" else 2, "fps": 24}
        for name in ("rest", "shoulder_raise", "elbow_bend", "knee_bend", "wrist_bend", "cheer")
    ]


def compile_plan(*args: object, **kwargs: object) -> NoReturn:
    raise ValueError("The fixed-hand provider profile requires an external provider rig")
