"""Reviewer quality bar: one labeled atlas at the height that decides the verdict.

SD characters are consumed at mobile gameplay scale, so the reviewer judges one
atlas whose cells are rendered at the bar's character height. `low` means the
character is usable in a game: nothing broken at the smallest declared gameplay
height. `medium` uses the largest declared gameplay height and a more explicit
per-boundary policy. `high` is declared for later close-up work and refused
before any spend. Each level needs its own calibration; none transfers.
"""

from __future__ import annotations

from collections.abc import Sequence
from typing import Any

JsonObject = dict[str, Any]
LEVELS = ("low", "medium", "high")
DEFAULT_LEVEL = "low"
EVIDENCE_KIND = "labeled_atlas_rows_v2"
PRESENTATION_SCALE = 2
#: The torso images' character height: the worker's largest, so a one-sided hem flap of a
#: few gameplay pixels is tens of pixels here.
TORSO_HEIGHT_PIXELS = 1024


def quality_bar_level(experiment: JsonObject) -> str:
    level = experiment.get("review_quality_bar", DEFAULT_LEVEL)
    if not isinstance(level, str) or level not in LEVELS:
        raise ValueError("review_quality_bar must be one of " + ", ".join(LEVELS))
    if level == "high":
        raise ValueError(
            "review_quality_bar 'high' is declared but not implemented for "
            "3D characters; use low or medium"
        )
    return level


def verdict_height(profile: JsonObject, level: str) -> int:
    review = profile.get("review", {})
    gameplay = sorted(int(size) for size in review.get("target_character_height_pixels", []))
    if not gameplay:
        raise ValueError("Quality bars need declared target_character_height_pixels")
    return gameplay[0] if level == "low" else gameplay[-1]


def quality_bar(experiment: JsonObject, profile: JsonObject) -> JsonObject:
    """Host-authored facts and policy handed to every reviewer episode.

    Cells are presented at twice the verdict height, capped at the inspection
    height: a vision model needs more pixels than a player to see the same
    defect. The verdict rule itself stays at the gameplay height.
    """
    level = quality_bar_level(experiment)
    height = verdict_height(profile, level)
    inspection = int(profile.get("review", {}).get("inspection_height_pixels", 512))
    presented = min(height * PRESENTATION_SCALE, max(inspection, height))
    return {
        "schema_version": 4,
        "level": level,
        "verdict_character_height_pixels": height,
        "cell_character_height_pixels": presented,
        "declared_review_heights_pixels": [height],
        "evidence_kind": EVIDENCE_KIND,
        "torso_character_height_pixels": TORSO_HEIGHT_PIXELS,
        "policy": _policy(level, height, presented),
    }


def _policy(level: str, height: int, presented: int) -> str:
    common = [
        f"Quality bar '{level}': the verdict is decided at {height} px character height.",
        ("You receive one labeled image per pose sample, a face strip, then one torso image"),
        "per pose sample.",
        ("In each pose image every cell is one required view of the same exported model."),
        f"Cells are rendered at {presented} px so you can see what a player sees at {height} px.",
        f"Judge as a player would at {height} px: what you can see here, a player can see.",
        ("The face strip shows the rest face at inspection height, front and three-quarter."),
        ("Use the face strip only for the face: smeared, missing, duplicated, misplaced or"),
        ("asymmetric eyes or facial paint fail rest_shape at every bar, because the face is"),
        "what a player looks at even when the figure is small.",
        f"Each torso image shows chest to mid-thigh at {TORSO_HEIGHT_PIXELS} px character",
        "height, front and back, of one pose sample: rest, then every motion sample.",
        ("Use the torso images only to compare the character's left and right sides: a hem,"),
        ("flap or panel that juts out past the body's outline on one side only as a joint"),
        f"moves is a spike; report it at {height} px, because a spike fails at every bar.",
        ("A crease, fold, pinch or dark shading line inside the outline is not a spike, and"),
        "anything else you can see only in the torso images is a minor issue.",
        ("There are no other renders and no second turn: submit your complete verdict now."),
        "Every issue states smallest_visible_character_height_pixels.",
        f"That value is {height} for anything you can see in the atlas.",
        ("Required-weight gaps and numeric rig findings block at every quality bar."),
        "Compare the left and right sides of the body in every row.",
        ("A pointed or angular protrusion on a shoulder, hip, sleeve or garment that exists on"),
        "one side only is a spike, not a design.",
        ("So is a thin point, longer than it is wide, that grows out of a surface that was"),
        "smooth at rest as the joint moves.",
        ("Cloth follows the limb it is attached to: a sleeve, cuff or jacket hem near the arm"),
        ("may lift or flare on both sides as the arm rises, and that is at most minor."),
        ("A garment on the hips or legs, such as a skirt or trousers, that widens, lifts or"),
        ("tilts when only the arms move is weighted to the wrong bones and fails"),
        "shoulder_deformation, even when both sides move together.",
        ("A spike fails shoulder_deformation, accessory_attachment or rest_shape at every bar."),
    ]
    if level == "low":
        specific = [
            "Low means usable in a game at this size.",
            ("Fail a criterion only for a defect a player would notice at this height:"),
            ("a detached or floating part, a collapsed or spiked joint, a head off its neck,"),
            ("a hand or foot separated from its limb, scrambled textures, or wrong proportions."),
            ("A seam, cuff or collar that merely looks imperfect when you know where to look"),
            "is a minor issue, not a failure.",
            ("For texture_integrity, blocking means scrambled, missing or wrong-object texture:"),
            ("UV garbage, a blank or missing image, or a large region in the wrong color."),
            ("An off-color patch, streak or small invented detail that still reads as plausible"),
            ("surface detail, such as a hair clip, a seam or a shading band, is minor at this bar"),
            "even when the reference does not show it.",
        ]
    else:
        specific = [
            "Medium is stricter than usable.",
            ("Compare each attachment boundary between its rest cell and its motion cells."),
            ("Fail the criterion when a motion cell shows a newly opened gap, exposed interior"),
            ("or stretched boundary that the rest cell does not, if visible at this height."),
            ("For head_neck_attachment, inspect the neck-to-body and collar boundary"),
            "as well as how the head is seated.",
            ("For shoulder_deformation and accessory_attachment, inspect sleeve, cuff and"),
            "strap boundaries in the raised-arm cells.",
            "Judge every criterion on its own evidence.",
            ("A failure in one criterion does not establish that the remaining criteria pass,"),
            "and a pass in one cell does not excuse a defect in another.",
        ]
    return " ".join(common + specific)


def upstream_policy(experiment: JsonObject, profile: JsonObject) -> JsonObject:
    """The bar as it applies before export: reference, raw-part and assembly reviews.

    These stages judge whether inputs are usable for the next stage, at the same
    level the final character will be judged. At low, brief-fidelity nuance is
    not a reason to refuse; a defect must break the next stage or be player-visible.
    """
    bar = quality_bar(experiment, profile)
    height = bar["verdict_character_height_pixels"]
    common = [
        f"Quality bar '{bar['level']}': the final character is judged at {height} px.",
        ("Judge these inputs as usable for the next stage at that bar, not for exact"),
        "fidelity to every word of the brief.",
        ("Blocking means the next stage would produce a broken or unusable character:"),
        ("a view that shows a different character, a duplicated front posing as a back,"),
        ("cropped or missing anatomy, an extra body part, scrambled or missing texture,"),
        ("a face with smeared, missing, duplicated or misplaced features, or a one-sided"),
        ("protrusion. Report everything else as a minor issue and pass the criterion."),
    ]
    if bar["level"] == "low":
        specific = [
            ("Proportion drift within the SD range, about two to four heads tall, is minor."),
            ("Small differences in head-to-body ratio between the canonical and a part view"),
            "are minor when identity, outfit and palette clearly agree.",
            ("Shading or lighting cues that merely look ambiguous, such as knee or elbow"),
            ("highlights on a back view, are minor when the view is otherwise a real back."),
        ]
    else:
        specific = [
            ("Proportion drift beyond about half a head from the canonical is blocking."),
            "A back view whose surface cues contradict its label is blocking.",
        ]
    return {
        "schema_version": 1,
        "level": bar["level"],
        "verdict_character_height_pixels": height,
        "policy": " ".join(common + specific),
    }


def numeric_tools(tools: Sequence[Any]) -> tuple[Any, ...]:
    """Keep only tools that return numbers, never images; the atlas is the picture."""
    kept = tuple(tool for tool in tools if tool.name == "inspect_asset")
    if not kept:
        raise ValueError("Atlas review needs the read-only inspect_asset tool")
    return kept


def check_issue_heights(value: JsonObject, bar: JsonObject) -> None:
    """Host self-consistency: severities and criterion failures respect the bar."""
    height = bar["verdict_character_height_pixels"]
    heights = set(bar["declared_review_heights_pixels"])
    for issue in value["issues"]:
        visible = issue["smallest_visible_character_height_pixels"]
        if visible not in heights:
            raise ValueError("Issue visibility must name a declared review height")
        if issue["severity"] == "blocking" and visible > height:
            raise ValueError("Blocking issues must be visible at the verdict height or smaller")
    failed = [item["criterion"] for item in value["criteria"] if not item["passed"]]
    if failed and (not any(issue["severity"] == "blocking" for issue in value["issues"])):
        raise ValueError("Failed criteria need at least one blocking issue at the verdict height")
