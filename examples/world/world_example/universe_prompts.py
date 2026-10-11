"""The one instruction a universe concept image is drawn from, composed from its direction.

The structured steps' instructions are prompt templates in ``prompts/``; this one is built in
Python because it word-budgets the direction it composes. The prose is tuned: fifteen recorded
runs moved these words, and most of the rules in them exist because a specific run failed
without them.
"""

from __future__ import annotations

# The instruction blocks below are tuned prose. Rewrapping them to satisfy the
# line limit would risk changing the bytes a model is sent, which is a
# correctness question, not a style one.
# ruff: noqa: E501
import re
from typing import TYPE_CHECKING

from world_example.medium import (
    SHARED_LOCAL_WEATHER,
    SHARED_OUTPUT_FORM,
    SHARED_STAGING_RULE,
    MediumContract,
)
from world_example.models import EntityDirection

if TYPE_CHECKING:
    from collections.abc import Mapping


def compact_words(value: str, *, max_words: int) -> str:
    compacted = " ".join(value.split())
    words = compacted.split(" ")
    if len(words) <= max_words:
        return compacted
    selected: list[str] = []
    used = 0
    for sentence in re.split(r"(?<=[.!?])\s+", compacted):
        n = len(sentence.split(" "))
        if used + n > max_words:
            break
        selected.append(sentence)
        used += n
    if selected:
        return " ".join(selected)
    prefix = " ".join(words[:max_words]).rstrip(",;:-")
    return prefix if prefix.endswith((".", "!", "?")) else f"{prefix}."


def image_prompt(
    *,
    entity_id: str,
    direction: EntityDirection,
    global_direction: Mapping[str, object],
    medium: MediumContract,
) -> str:
    """Assemble the one instruction an image node sends.

    Medium first and medium last: the render contract opens the prompt and the
    negative block closes it, because a renderer that speaks one medium will
    otherwise be overruled by scene prose that speaks another. Everything
    between is word-budgeted, so a verbose direction cannot crowd the contract
    out of the model's attention.
    """

    beat = direction.action_beat
    identity = direction.visual_identity
    w = compact_words
    forbidden = global_direction["forbidden_substitutions"]
    substitutions = list(forbidden) if isinstance(forbidden, list) else [forbidden]
    world = "\n".join(
        (
            f"Silhouette language: {w(str(global_direction['world_silhouette_language']), max_words=60)}",
            f"Architecture: {w(str(global_direction['architecture_grammar']), max_words=60)}",
            f"Costume: {w(str(global_direction['costume_grammar']), max_words=50)}",
            f"Materials: {w(str(global_direction['material_language']), max_words=50)}",
            f"Scale anchors: {w(str(global_direction['scale_anchors']), max_words=30)}",
            "Never substitute: " + "; ".join(str(x) for x in substitutions[:6]) + ".",
        )
    )
    scene = "\n".join(
        (
            f"Primary subject: {w(direction.primary_subject, max_words=70)}",
            f"The moment: {w(beat.agent, max_words=25)} {w(beat.goal, max_words=25)} {w(beat.obstacle, max_words=25)} {w(beat.intervention, max_words=30)} {w(beat.visible_state_change, max_words=30)}",
            f"Environment: {w(direction.immediate_environment, max_words=70)}",
            f"Composition: {w(direction.composition_and_camera, max_words=60)}",
            f"Identity: silhouette {w(identity.silhouette, max_words=30)} Proportions {w(identity.proportions, max_words=25)} Construction {w(identity.construction_logic, max_words=30)} Materials {w(identity.materials, max_words=30)} Color placement {w(identity.color_placement, max_words=30)} Scale anchor {w(identity.scale_anchor, max_words=20)} Wear {w(identity.wear_and_history, max_words=25)} Motion or use {w(identity.characteristic_motion_or_use, max_words=25)}",
            f"Conditions: {w(direction.register_realization, max_words=60)}",
            "Avoid: " + "; ".join(direction.avoid[:8]) + ".",
        )
    )
    return "\n\n".join(
        (
            medium.render_block,
            SHARED_STAGING_RULE,
            f"WORLD GRAMMAR\n{world}",
            f"ENTITY SCENE ({entity_id})\n{scene}",
            SHARED_LOCAL_WEATHER,
            SHARED_OUTPUT_FORM,
            medium.negative_block,
        )
    )


__all__ = ["compact_words", "image_prompt"]
