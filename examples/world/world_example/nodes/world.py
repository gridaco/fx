"""The world's sources, read once; what the planners are shown of it; and its admission.

The synopsis's paragraphs and the direction's requirements get stable ids here, the only
ids a proposal may cite as evidence or rationale. The medium's contract travels with them,
so every prompt that needs it reads the same text the judges hold it to.
"""

from __future__ import annotations

import json
import re
from typing import Any

from world_example import models
from world_example.medium import medium_contract

from grida.fx import Ctx, node


def synopsis_paragraphs(text: str) -> list[tuple[str, str]]:
    """Blank-line-separated blocks, headings skipped, numbered in reading order."""

    paragraphs: list[tuple[str, str]] = []
    for block in re.split(r"\n\s*\n", text.strip()):
        lines = [line.strip() for line in block.splitlines() if line.strip()]
        if not lines or lines[0].startswith("#"):
            continue
        paragraphs.append((f"synopsis_p{len(paragraphs) + 1:02d}", " ".join(lines)))
    if not paragraphs:
        raise ValueError("the synopsis has no paragraphs")
    return paragraphs


def direction_requirements(text: str) -> list[tuple[str, str]]:
    """``- <requirement_id>: <text>`` bullets, two-space continuations folded in."""

    items: list[tuple[str, str]] = []
    current: list[str] | None = None
    for line in text.splitlines():
        match = re.match(r"^- ([a-z][a-z0-9_]{1,95}): (.+)$", line)
        if match:
            if current:
                items.append((current[0], " ".join(current[1:])))
            current = [match.group(1), match.group(2).strip()]
        elif current and line.startswith("  ") and line.strip():
            current.append(line.strip())
        elif current:
            items.append((current[0], " ".join(current[1:])))
            current = None
    if current:
        items.append((current[0], " ".join(current[1:])))
    ids = [item[0] for item in items]
    if not ids or len(ids) != len(set(ids)):
        raise ValueError("the expansion direction must list unique requirement ids")
    return items


def compact(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


@node(
    "source",
    inputs={"synopsis": "text", "direction": "text"},
    params={"medium": str},
    outputs={"source": "json"},
    version=1,
)
def source(ctx: Ctx) -> dict[str, Any]:
    """Number the synopsis and the direction, and carry the medium's contract with them."""

    synopsis, direction = ctx.read.text("synopsis"), ctx.read.text("direction")
    try:
        paragraphs = synopsis_paragraphs(synopsis)
        requirements = direction_requirements(direction)
        medium = medium_contract(ctx.params["medium"])
    except (KeyError, ValueError) as error:
        raise ctx.fail(str(error)) from error
    section = "\n".join(
        (
            "SYNOPSIS PARAGRAPHS (explicit source; the only synopsis evidence ids)",
            *(f"- {pid}: {text}" for pid, text in paragraphs),
            "",
            "EXPANSION DIRECTION REQUIREMENTS (rationale ids, never evidence)",
            *(f"- {rid}: {text}" for rid, text in requirements),
            "",
            "EXPANSION DIRECTION (full text)",
            direction,
        )
    )
    return {
        "source": ctx.out.json(
            {
                "paragraphs": [{"id": pid, "text": text} for pid, text in paragraphs],
                "requirements": [{"id": rid, "text": text} for rid, text in requirements],
                "section": section,
                "medium": {
                    "medium_id": medium.medium_id,
                    "display_name": medium.display_name,
                    "compile_guidance": medium.compile_guidance,
                    "render_block": medium.render_block,
                    "negative_block": medium.negative_block,
                    "review_criteria": medium.review_criteria,
                    "forbidden_direction_terms": list(medium.forbidden_direction_terms),
                },
            }
        )
    }


def _proposal(ctx: Ctx) -> models.UniverseProposal:
    return models.UniverseProposal.model_validate(ctx.read.json("proposal"))


@node("projection", inputs={"proposal": "json"}, outputs={"text": "text/plain"}, version=1)
def projection(ctx: Ctx) -> dict[str, Any]:
    """What the gallery planner is shown: the entities, their facts and relationships."""

    proposal = _proposal(ctx)
    shown = {
        "universe_id": proposal.universe_id,
        "premise": proposal.premise.claim,
        "present_state": proposal.present_state.claim,
        "entities": [
            {
                "entity_id": entity.entity_id,
                "display_name": entity.display_name,
                "primary_class": entity.primary_class,
                "facets": entity.facets,
                "salience": entity.salience,
                "summary": entity.summary,
                "how_it_works_or_lives": entity.how_it_works_or_lives,
                "present_tension": entity.present_tension,
                "facts": [{"fact_id": fact.fact_id, "claim": fact.claim} for fact in entity.facts],
            }
            for entity in proposal.entities
        ],
        "relationships": [
            {
                "relationship_id": relationship.relationship_id,
                "kind": relationship.relationship_kind,
                "source": relationship.source_entity_id,
                "target": relationship.target_entity_id,
                "summary": relationship.summary,
            }
            for relationship in proposal.relationships
        ],
    }
    return {"text": ctx.out.text(compact(shown))}


@node("grammar_projection", inputs={"universe": "json"}, outputs={"text": "text/plain"}, version=1)
def grammar_projection(ctx: Ctx) -> dict[str, Any]:
    """What the global visual grammar is compiled from: the admitted world, briefly."""

    proposal = models.UniverseProposal.model_validate(ctx.read.json("universe")["proposal"])
    shown = {
        "universe_id": proposal.universe_id,
        "title": proposal.title,
        "premise": proposal.premise.claim,
        "present_state": proposal.present_state.claim,
        "physical_ecological_rules": [fact.claim for fact in proposal.physical_ecological_rules],
        "poster_observations": [
            observation.model_dump(mode="json")
            for observation in proposal.visual_observations
            if observation.canonical_status == "evidence"
        ],
        "entities": [
            {
                "entity_id": entity.entity_id,
                "display_name": entity.display_name,
                "primary_class": entity.primary_class,
                "entity_kind": entity.entity_kind,
                "summary": entity.summary,
            }
            for entity in proposal.entities
        ],
        "identity_markers": [
            {"owner": marker.owner_entity_id, "form": marker.form, "materials": marker.materials}
            for marker in proposal.identity_markers
        ],
    }
    return {"text": ctx.out.text(compact(shown))}


@node(
    "admit",
    inputs={"proposal": "json", "plan": "json", "review": "json"},
    params={"medium": str},
    outputs={"universe": "json"},
    version=1,
)
def admit(ctx: Ctx) -> dict[str, Any]:
    """Admit the world the independent review passed; a failed review stops the run here.

    Admission authorizes the gallery and nothing else: it is never publication approval.
    """

    review = models.SemanticReview.model_validate(ctx.read.json("review"))
    if review.verdict != "pass":
        raise ctx.fail(
            "the independent semantic review rejected the universe: "
            + "; ".join(review.blocking_findings)
        )
    proposal = _proposal(ctx)
    plan = models.GalleryPlan.model_validate(ctx.read.json("plan"))
    if {entry.entity_id for entry in plan.plans} != {e.entity_id for e in proposal.entities}:
        raise ctx.fail("the gallery plan does not hold exactly one entry per entity")
    return {
        "universe": ctx.out.json(
            {
                "kind": "universe-admitted-v2",
                "universe_id": proposal.universe_id,
                "title": proposal.title,
                "medium_id": ctx.params["medium"],
                "proposal": proposal.model_dump(mode="json"),
                "plan": plan.model_dump(mode="json"),
                "review": review.model_dump(mode="json"),
                "publication_authorized": False,
            }
        )
    }
