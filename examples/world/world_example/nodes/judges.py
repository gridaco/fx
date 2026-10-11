"""The deterministic judges: what a structured answer must satisfy before anything reads it.

Each rejects with the exact errors, as facts, and its step's ``on_reject`` draws the answer
again; warnings never reject. Lexical register and medium checks stay warnings, because a
hint the model cannot correct itself from only burns takes.
"""

from __future__ import annotations

from typing import Any, TypeVar

from pydantic import ValidationError
from world_example import models
from world_example.medium import forbidden_terms_present, medium_contract

from grida.fx import Ctx, node

T = TypeVar("T", bound=models.ContractModel)


def _verdict(ctx: Ctx, errors: list[str], warnings: list[str] | None = None) -> dict[str, Any]:
    ctx.fact("errors", errors)
    if warnings is not None:
        ctx.fact("warnings", warnings)
    ctx.fact("verdict", "reject" if errors else "accept")
    return {}


def _parsed(ctx: Ctx, name: str, model: type[T]) -> T | None:
    try:
        return model.model_validate(ctx.read.json(name))
    except ValidationError as error:
        ctx.fact("errors", [f"{name}: {e['msg']} at {e['loc']}" for e in error.errors()][:20])
        ctx.fact("verdict", "reject")
        return None


@node(
    "proposal_ok",
    inputs={"proposal": "json", "source": "json"},
    params={"universe_id": str, "min_entities": int, "max_entities": int},
    judge=True,
    version=1,
)
def proposal_ok(ctx: Ctx) -> dict[str, Any]:
    """Ids unique and resolvable, evidence from the synopsis only, the census honoured, one
    connected world."""

    proposal = _parsed(ctx, "proposal", models.UniverseProposal)
    if proposal is None:
        return {}
    source = ctx.read.json("source")
    evaluation = models.evaluate_proposal(
        proposal,
        universe_id=ctx.params["universe_id"],
        synopsis_ids={item["id"] for item in source["paragraphs"]},
        requirement_ids={item["id"] for item in source["requirements"]},
        min_entities=ctx.params["min_entities"],
        max_entities=ctx.params["max_entities"],
    )
    ctx.fact("metrics", evaluation["metrics"])
    return _verdict(ctx, evaluation["errors"])


@node("plan_ok", inputs={"plan": "json", "proposal": "json"}, judge=True, version=1)
def plan_ok(ctx: Ctx) -> dict[str, Any]:
    """One entry per entity, unique lessons and motifs, registers spread across the set."""

    plan = _parsed(ctx, "plan", models.GalleryPlan)
    proposal = _parsed(ctx, "proposal", models.UniverseProposal)
    if plan is None or proposal is None:
        return {}
    evaluation = models.evaluate_plan(plan, proposal)
    ctx.fact("metrics", evaluation["metrics"])
    return _verdict(ctx, evaluation["errors"], evaluation["warnings"])


@node(
    "grammar_ok",
    inputs={"grammar": "json", "universe": "json", "source": "json"},
    judge=True,
    version=1,
)
def grammar_ok(ctx: Ctx) -> dict[str, Any]:
    """The global grammar names this world and medium, speaks the medium's own words, and
    cites only admitted poster observations."""

    grammar = _parsed(ctx, "grammar", models.GlobalDirection)
    if grammar is None:
        return {}
    proposal = models.UniverseProposal.model_validate(ctx.read.json("universe")["proposal"])
    medium = medium_contract(ctx.read.json("source")["medium"]["medium_id"])
    errors: list[str] = []
    if grammar.medium_id != medium.medium_id:
        errors.append(f"medium_id must be {medium.medium_id}")
    if grammar.universe_id != proposal.universe_id:
        errors.append("universe_id must match")
    prose = "\n".join(
        (
            grammar.world_silhouette_language,
            grammar.architecture_grammar,
            grammar.costume_grammar,
            grammar.material_language,
            grammar.scale_anchors,
            grammar.technology_and_ecology_rules,
            *(entry.palette for entry in grammar.palette_by_region),
        )
    )
    foreign = forbidden_terms_present(medium, prose)
    if foreign:
        errors.append(f"the global direction uses terms foreign to the medium: {foreign}")
    observed = {observation.observation_id for observation in proposal.visual_observations}
    if set(grammar.poster_observation_ids_used) - observed:
        errors.append("poster_observation_ids_used must be admitted observation ids")
    return _verdict(ctx, errors)


@node(
    "direction_ok",
    inputs={"direction": "json", "universe": "json", "source": "json"},
    params={"entity_id": str},
    judge=True,
    version=1,
)
def direction_ok(ctx: Ctx) -> dict[str, Any]:
    """The direction is for its own entity; register and medium hints are warnings."""

    direction = _parsed(ctx, "direction", models.EntityDirection)
    if direction is None:
        return {}
    plan = models.GalleryPlan.model_validate(ctx.read.json("universe")["plan"])
    entry = plan.plan(ctx.params["entity_id"])
    medium = medium_contract(ctx.read.json("source")["medium"]["medium_id"])
    return _verdict(
        ctx,
        models.evaluate_direction(direction, entry, medium),
        models.direction_warnings(direction, entry, medium),
    )
