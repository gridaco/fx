"""One admitted entity's way to its concept image: what its director and reviewer are shown,
the one instruction its image is drawn from, and the record a reader keeps beside it.
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

from world_example import models
from world_example.medium import medium_contract
from world_example.nodes.world import compact
from world_example.universe_prompts import image_prompt as compose_image_prompt

from grida.fx import Ctx, node

_ENTITY = {"entity_id": str}


def _other(relationship: models.Relationship, entity_id: str) -> str:
    if relationship.source_entity_id == entity_id:
        return relationship.target_entity_id
    return relationship.source_entity_id


def _world(ctx: Ctx) -> tuple[models.UniverseProposal, models.GalleryPlan]:
    universe = ctx.read.json("universe")
    return (
        models.UniverseProposal.model_validate(universe["proposal"]),
        models.GalleryPlan.model_validate(universe["plan"]),
    )


def _projection(
    proposal: models.UniverseProposal, plan: models.GalleryPlan, entity_id: str
) -> dict[str, Any]:
    entity = proposal.entity(entity_id)
    names = {item.entity_id: item.display_name for item in proposal.entities}
    return {
        "entity": entity.model_dump(mode="json"),
        "incident_relationships": [
            {
                "relationship_id": relationship.relationship_id,
                "kind": relationship.relationship_kind,
                "with": names.get(_other(relationship, entity_id), "?"),
                "direction": (
                    "outgoing" if relationship.source_entity_id == entity_id else "incoming"
                ),
                "summary": relationship.summary,
            }
            for relationship in proposal.relationships
            if entity_id in (relationship.source_entity_id, relationship.target_entity_id)
        ],
        "owned_identity_markers_text_only": [
            {"form": marker.form, "materials": marker.materials, "applied_use": marker.applied_use}
            for marker in proposal.identity_markers
            if marker.owner_entity_id == entity_id
        ],
        "plan": plan.plan(entity_id).model_dump(mode="json"),
    }


@node(
    "entity_context",
    inputs={"universe": "json"},
    params=_ENTITY,
    outputs={"text": "text/plain"},
    version=1,
)
def entity_context(ctx: Ctx) -> dict[str, Any]:
    """The entity, its relationships, its markers as text, and its sealed plan entry."""

    proposal, plan = _world(ctx)
    return {"text": ctx.out.text(compact(_projection(proposal, plan, ctx.params["entity_id"])))}


@node(
    "image_prompt",
    inputs={"direction": "json", "grammar": "json", "source": "json"},
    params=_ENTITY,
    outputs={"prompt": "text/plain"},
    version=1,
)
def image_prompt(ctx: Ctx) -> dict[str, Any]:
    """The one instruction an image is drawn from: the medium first and last, the scene
    word-budgeted between, so verbose direction cannot crowd the medium out."""

    direction = models.EntityDirection.model_validate(ctx.read.json("direction"))
    medium = medium_contract(ctx.read.json("source")["medium"]["medium_id"])
    prompt = compose_image_prompt(
        entity_id=ctx.params["entity_id"],
        direction=direction,
        global_direction=ctx.read.json("grammar"),
        medium=medium,
    )
    return {"prompt": ctx.out.text(prompt)}


def _direction_summary(direction: models.EntityDirection) -> dict[str, Any]:
    return {
        "primary_subject": direction.primary_subject,
        "action_beat": direction.action_beat.model_dump(mode="json"),
        "visual_identity": direction.visual_identity.model_dump(mode="json"),
        "register_realization": direction.register_realization,
    }


@node(
    "review_record",
    inputs={"universe": "json", "direction": "json"},
    params=_ENTITY,
    outputs={"text": "text/plain"},
    version=1,
)
def review_record(ctx: Ctx) -> dict[str, Any]:
    """What the image reviewer judges against: the entity, its plan entry, its direction."""

    proposal, plan = _world(ctx)
    projection = _projection(proposal, plan, ctx.params["entity_id"])
    direction = models.EntityDirection.model_validate(ctx.read.json("direction"))
    shown = {
        "entity": projection["entity"],
        "plan": projection["plan"],
        "direction_summary": _direction_summary(direction),
    }
    return {"text": ctx.out.text(compact(shown))}


@node(
    "record",
    inputs={"universe": "json", "direction": "json", "review": "json", "image": "image"},
    params=_ENTITY,
    outputs={"record": "json", "markdown": "text/markdown"},
    version=1,
)
def record(ctx: Ctx) -> dict[str, Any]:
    """The entity as a reader keeps it beside its image, admitted or rejected by its review.

    A rejected image is a result, not a failure: the gallery keeps it, marked, and a new
    take is drawn only by asking for one (``python -m grida.fx reroll``).
    """

    entity_id = ctx.params["entity_id"]
    proposal, plan = _world(ctx)
    review = models.ImageReview.model_validate(ctx.read.json("review"))
    if review.entity_id != entity_id:
        raise ctx.fail(f"the review is of {review.entity_id}, not {entity_id}")
    direction = models.EntityDirection.model_validate(ctx.read.json("direction"))
    picture = ctx.read.image("image")
    names = {item.entity_id: item.display_name for item in proposal.entities}
    status = "admitted" if review.verdict == "admit" else "rejected"
    document = {
        "kind": "universe-entity-record-v2",
        "universe_id": proposal.universe_id,
        "status": status,
        "entity": proposal.entity(entity_id).model_dump(mode="json"),
        "relationships": [
            {
                **relationship.model_dump(mode="json"),
                "other_entity_id": _other(relationship, entity_id),
                "other_display_name": names.get(_other(relationship, entity_id), "?"),
                "direction": (
                    "outgoing" if relationship.source_entity_id == entity_id else "incoming"
                ),
            }
            for relationship in proposal.relationships
            if entity_id in (relationship.source_entity_id, relationship.target_entity_id)
        ],
        "identity_markers": [
            marker.model_dump(mode="json")
            for marker in proposal.identity_markers
            if marker.owner_entity_id == entity_id
        ],
        "concept": plan.plan(entity_id).model_dump(mode="json"),
        "direction_summary": _direction_summary(direction),
        "image": {
            "path": f"entities/{entity_id}.png",
            "width": picture.width,
            "height": picture.height,
        },
        "review": review.model_dump(mode="json"),
        "publication_authorized": False,
    }
    ctx.fact("status", status)
    ctx.fact("display_name", names[entity_id])
    ctx.fact("primary_class", proposal.entity(entity_id).primary_class)
    return {
        "record": ctx.out.json(document),
        "markdown": ctx.out.text(entity_markdown(document), "text/markdown"),
    }


def entity_markdown(record: Mapping[str, Any]) -> str:
    """The readable half of an entity record: what a person reads beside the image."""

    entity = record["entity"]
    lines = [f"# {entity['display_name']}", ""]
    classes = [entity["primary_class"], *entity.get("facets", [])]
    lines.append(
        f"*{' / '.join(classes)}* · {entity['entity_kind']} · "
        f"{entity['salience']} · image {record['status']}"
    )
    lines += [
        "",
        entity["summary"],
        "",
        "## How it works or lives",
        "",
        entity["how_it_works_or_lives"],
        "",
        "## Present tension",
        "",
        entity["present_tension"],
        "",
        "## Facts",
        "",
    ]
    for fact in entity["facts"]:
        lines.append(f"- **{fact['fact_id']}** ({fact['lineage']}): {fact['claim']}")
    lines += ["", "## Relationships", ""]
    for relationship in record["relationships"]:
        arrow = "→" if relationship["direction"] == "outgoing" else "←"
        lines.append(
            f"- {arrow} {relationship['relationship_kind']} "
            f"**{relationship['other_display_name']}**: {relationship['summary']}"
        )
    if record["identity_markers"]:
        lines += ["", "## Identity markers", ""]
        for marker in record["identity_markers"]:
            lines.append(f"- {marker['form']}: {marker['meaning']} ({marker['materials']})")
    concept = record["concept"]
    motif = concept["signature_motif"]
    lines += [
        "",
        "## Concept image",
        "",
        f"Purpose: {concept['primary_purpose']}. Question: {concept['audience_question']}",
        "",
        f"Signature: {motif['action_verb']} / {motif['dominant_prop']} / {motif['vantage']}. "
        f"Contrast: {concept['in_frame_contrast']}",
        "",
        concept["scene_premise"],
        "",
        f"What this image alone teaches: {record['review']['what_the_image_teaches']}",
    ]
    if record["review"]["verdict"] == "reject":
        lines += ["", "Rejected: " + " ".join(record["review"]["blocking_findings"])]
    return "\n".join(lines) + "\n"
