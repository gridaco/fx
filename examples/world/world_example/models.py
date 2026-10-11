"""Universe V1 contracts and deterministic evaluators.

Everything here is medium-free except where a medium contract is passed in
explicitly. Field names use lower_snake_case; there are no compatibility
aliases.
"""

from __future__ import annotations

import re
from collections import Counter
from typing import Annotated, ClassVar, Final, Literal, TypedDict

from pydantic import BaseModel, ConfigDict, Field, model_validator

from world_example.medium import MediumContract, forbidden_terms_present
from world_example.ontology import (
    CONCEPT_PURPOSES,
    DRY_WEATHER,
    MODES_BY_CLASS,
    WET_WEATHER,
    ConceptMode,
    ConceptPurpose,
    Energy,
    EntityClass,
    LineageKind,
    Population,
    RelationshipFamily,
    Scale,
    Setting,
    TimeOfDay,
    Vantage,
    Weather,
)

StableId = Annotated[str, Field(pattern=r"^[a-z][a-z0-9_]{1,95}$")]
Sha256 = Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
Text = Annotated[str, Field(min_length=1)]
Salience = Literal["major", "supporting", "minor"]
Grade = Literal["pass", "fail"]


class ContractModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class ProposalEvaluation(TypedDict):
    """What the deterministic proposal evaluator reports.

    Typed rather than a loose mapping because callers branch on ``errors``
    inside the structured service's retry owner: a mistyped key there is six
    burnt attempts, not a test failure.
    """

    status: str
    errors: list[str]
    metrics: dict[str, object]


class PlanEvaluation(TypedDict):
    """Set-level evaluation. ``warnings`` never block; they are advisory only."""

    status: str
    errors: list[str]
    warnings: list[str]
    metrics: dict[str, object]


# --- semantic proposal -------------------------------------------------------


class Fact(ContractModel):
    fact_id: StableId
    claim: Text
    lineage: LineageKind
    basis: Text
    evidence_ids: list[StableId]
    direction_requirement_ids: list[StableId]


class VisualObservation(ContractModel):
    observation_id: StableId
    category: Literal[
        "subject", "place", "thing", "system_cue", "art_grammar", "ambiguity", "promotional_element"
    ]
    claim: Text
    confidence: Literal["high", "medium", "low"]
    canonical_status: Literal["evidence", "uncertain", "exclude_from_canon"]


class Viewpoint(ContractModel):
    viewpoint_id: StableId
    display_name: Text
    summary: Text
    anchor_entity_ids: list[StableId]
    initially_known: Text
    entry_question: Text


class Entity(ContractModel):
    entity_id: StableId
    display_name: Text
    primary_class: EntityClass
    facets: list[EntityClass]
    entity_kind: Text
    salience: Salience
    summary: Text
    how_it_works_or_lives: Text
    present_tension: Text
    facts: list[Fact] = Field(min_length=2, max_length=4)


class Relationship(ContractModel):
    relationship_id: StableId
    relationship_family: RelationshipFamily
    relationship_kind: StableId
    source_entity_id: StableId
    target_entity_id: StableId
    summary: Text
    temporal_scope: Text
    perspective: Text
    lineage: LineageKind
    evidence_ids: list[StableId]
    direction_requirement_ids: list[StableId]


class IdentityMarker(ContractModel):
    marker_id: StableId
    owner_entity_id: StableId
    form: Text
    materials: Text
    applied_use: Text
    meaning: Text
    limits: Text
    historical_origin: Text
    lineage: LineageKind
    evidence_ids: list[StableId]
    direction_requirement_ids: list[StableId]
    independently_admitted_thing_id: StableId | None


class InstitutionalTension(ContractModel):
    tension_id: StableId
    summary: Text
    participant_entity_ids: list[StableId]
    fact_ids: list[StableId]
    material_stakes: Text
    competing_legitimate_needs: Text


class DirectionCoverage(ContractModel):
    requirement_id: StableId
    status: Literal["satisfied", "partial", "unmet"]
    evidence_ids: list[StableId]
    note: Text


class SourceConflict(ContractModel):
    conflict_id: StableId
    detail: Text
    status: Literal["unresolved", "resolved", "deliberate_extension"]
    resolution: Text
    evidence_ids: list[StableId]


class UniverseProposal(ContractModel):
    universe_id: StableId
    title: Text
    premise: Fact
    present_state: Fact
    physical_ecological_rules: list[Fact]
    institutional_tensions: list[InstitutionalTension]
    visual_observations: list[VisualObservation]
    viewpoints: list[Viewpoint]
    entities: list[Entity]
    relationships: list[Relationship]
    identity_markers: list[IdentityMarker]
    direction_coverage: list[DirectionCoverage]
    source_conflicts: list[SourceConflict]
    unresolved_questions: list[Text]

    def all_facts(self) -> list[Fact]:
        facts = [self.premise, self.present_state, *self.physical_ecological_rules]
        for entity in self.entities:
            facts.extend(entity.facts)
        return facts

    def entity(self, entity_id: str) -> Entity:
        for entity in self.entities:
            if entity.entity_id == entity_id:
                return entity
        raise KeyError(entity_id)


# --- gallery plan (set-level) -----------------------------------------------


class SceneRegister(ContractModel):
    scale: Scale
    time_of_day: TimeOfDay
    weather: Weather
    weather_justification: Text
    setting: Setting
    population: Population
    energy: Energy

    def key(self) -> tuple[str, str, str, str, str]:
        return (self.scale, self.time_of_day, self.weather, self.setting, self.population)


class SignatureMotif(ContractModel):
    """What a viewer would sketch in three strokes to tell this image from every other."""

    action_verb: StableId
    dominant_prop: StableId
    vantage: Vantage

    def key(self) -> tuple[str, str]:
        return (self.action_verb, self.dominant_prop)


class ConceptPlan(ContractModel):
    entity_id: StableId
    concept_mode: ConceptMode
    primary_purpose: ConceptPurpose
    lesson_key: StableId
    audience_question: Text
    visible_fact_ids: list[StableId]
    visible_relationship_ids: list[StableId]
    scene_premise: Text
    signature_motif: SignatureMotif
    in_frame_contrast: Text
    unique_contribution: Text
    scene_register: SceneRegister


class GalleryPlan(ContractModel):
    universe_id: StableId
    set_rationale: Text
    plans: list[ConceptPlan]

    def plan(self, entity_id: str) -> ConceptPlan:
        for plan in self.plans:
            if plan.entity_id == entity_id:
                return plan
        raise KeyError(entity_id)


# --- visual direction (medium-aware, text only) ------------------------------


class RegionPalette(ContractModel):
    region: Text
    palette: Text


class GlobalDirection(ContractModel):
    universe_id: StableId
    medium_id: StableId
    world_silhouette_language: Text
    architecture_grammar: Text
    costume_grammar: Text
    material_language: Text
    palette_by_region: list[RegionPalette] = Field(min_length=2)
    scale_anchors: Text
    technology_and_ecology_rules: Text
    forbidden_substitutions: list[Text] = Field(min_length=3)
    poster_observation_ids_used: list[StableId]


class ActionBeat(ContractModel):
    agent: Text
    goal: Text
    obstacle: Text
    intervention: Text
    visible_state_change: Text


class VisualIdentity(ContractModel):
    silhouette: Text
    proportions: Text
    construction_logic: Text
    materials: Text
    color_placement: Text
    scale_anchor: Text
    wear_and_history: Text
    characteristic_motion_or_use: Text
    forbidden_substitutions: list[Text] = Field(min_length=1)


class EntityDirection(ContractModel):
    entity_id: StableId
    primary_subject: Text
    action_beat: ActionBeat
    immediate_environment: Text
    composition_and_camera: Text
    visual_identity: VisualIdentity
    register_realization: Text
    continuity_notes: Text
    avoid: list[Text]

    def prose(self) -> str:
        beat = self.action_beat
        identity = self.visual_identity
        return "\n".join(
            (
                self.primary_subject,
                beat.agent,
                beat.goal,
                beat.obstacle,
                beat.intervention,
                beat.visible_state_change,
                self.immediate_environment,
                self.composition_and_camera,
                identity.silhouette,
                identity.proportions,
                identity.construction_logic,
                identity.materials,
                identity.color_placement,
                identity.scale_anchor,
                identity.wear_and_history,
                identity.characteristic_motion_or_use,
                self.register_realization,
                self.continuity_notes,
            )
        )


# --- reviews -----------------------------------------------------------------


class ReviewCheck(ContractModel):
    check_id: StableId
    status: Grade
    finding: Text


class SemanticReview(ContractModel):
    review_id: Literal["universe_independent_semantic_review"]
    reviewer_role: Literal["independent_semantic_reviewer"]
    verdict: Grade
    checks: list[ReviewCheck] = Field(min_length=6)
    blocking_findings: list[Text]
    advisory_findings: list[Text]
    conclusion: Text

    @model_validator(mode="after")
    def verdict_matches_checks(self) -> SemanticReview:
        failed = any(check.status == "fail" for check in self.checks)
        if failed != (self.verdict == "fail"):
            raise ValueError("semantic review verdict must be fail exactly when a check fails")
        if self.verdict == "fail" and not self.blocking_findings:
            raise ValueError("a failing semantic review must list blocking findings")
        return self


class ImageReview(ContractModel):
    review_id: Literal["universe_independent_image_review"]
    entity_id: StableId
    verdict: Literal["admit", "reject"]
    entity_identity: Grade
    action_legibility: Grade
    medium_fidelity: Grade
    register_fidelity: Grade
    readable_text_absent: Grade
    explanatory_form_absent: Grade
    technical_quality: Grade
    blocking_findings: list[Text]
    advisory_findings: list[Text]
    what_the_image_teaches: Text

    BLOCKING_GRADES: ClassVar[tuple[str, ...]] = (
        "entity_identity",
        "action_legibility",
        "medium_fidelity",
        "register_fidelity",
        "readable_text_absent",
        "explanatory_form_absent",
        "technical_quality",
    )

    @model_validator(mode="after")
    def verdict_matches_grades(self) -> ImageReview:
        failed = any(getattr(self, grade) == "fail" for grade in self.BLOCKING_GRADES)
        if failed != (self.verdict == "reject"):
            raise ValueError("image review verdict must be reject exactly when a grade fails")
        if self.verdict == "reject" and not self.blocking_findings:
            raise ValueError("a rejecting image review must list blocking findings")
        return self


# --- deterministic evaluators -----------------------------------------------


def _check_lineage(
    label: str,
    lineage: str,
    evidence_ids: list[str],
    direction_ids: list[str],
    *,
    synopsis_ids: set[str],
    observation_ids: set[str],
    requirement_ids: set[str],
    errors: list[str],
) -> None:
    evidence = set(evidence_ids)
    unknown = evidence - synopsis_ids - observation_ids
    if unknown:
        errors.append(f"{label}: unknown evidence ids {sorted(unknown)}")
    if evidence & requirement_ids:
        errors.append(f"{label}: direction requirement ids used as evidence")
    unknown_direction = set(direction_ids) - requirement_ids
    if unknown_direction:
        errors.append(f"{label}: unknown direction ids {sorted(unknown_direction)}")
    if not evidence:
        errors.append(f"{label}: evidence_ids must be non-empty")
    if lineage == "explicit_source":
        if evidence - synopsis_ids:
            errors.append(f"{label}: explicit_source must cite only synopsis paragraphs")
        if direction_ids:
            errors.append(f"{label}: explicit_source carries no direction rationale")
    elif lineage == "visual_observation":
        if not evidence & observation_ids:
            errors.append(f"{label}: visual_observation must cite a poster observation")
        if direction_ids:
            errors.append(f"{label}: visual_observation carries no direction rationale")
    elif lineage == "conservative_inference":
        if direction_ids:
            errors.append(f"{label}: conservative_inference carries no direction rationale")
    elif lineage == "generated_extension" and not direction_ids:
        errors.append(f"{label}: generated_extension requires a direction requirement id")


def _connected(entity_ids: set[str], edges: list[tuple[str, str]]) -> int:
    adjacency: dict[str, set[str]] = {entity_id: set() for entity_id in entity_ids}
    for source, target in edges:
        if source in adjacency and target in adjacency:
            adjacency[source].add(target)
            adjacency[target].add(source)
    seen: set[str] = set()
    components = 0
    for start in entity_ids:
        if start in seen:
            continue
        components += 1
        stack = [start]
        while stack:
            current = stack.pop()
            if current in seen:
                continue
            seen.add(current)
            stack.extend(adjacency[current] - seen)
    return components


def evaluate_proposal(
    proposal: UniverseProposal,
    *,
    universe_id: str,
    synopsis_ids: set[str],
    requirement_ids: set[str],
    min_entities: int,
    max_entities: int,
) -> ProposalEvaluation:
    errors: list[str] = []
    if proposal.universe_id != universe_id:
        errors.append(f"universe_id must be {universe_id}")
    entity_ids = [entity.entity_id for entity in proposal.entities]
    entity_set = set(entity_ids)
    if len(entity_ids) != len(entity_set):
        errors.append("entity ids must be unique")
    count = len(entity_ids)
    if not min_entities <= count <= max_entities:
        errors.append(f"entity count {count} outside [{min_entities}, {max_entities}]")
    observation_ids = {obs.observation_id for obs in proposal.visual_observations}
    bad_observation = sorted(o for o in observation_ids if not o.startswith("poster_"))
    if bad_observation:
        errors.append(f"poster observation ids must start with poster_: {bad_observation}")
    evidence_observations = {
        obs.observation_id
        for obs in proposal.visual_observations
        if obs.canonical_status == "evidence"
    }

    facts = proposal.all_facts()
    fact_ids = [fact.fact_id for fact in facts]
    fact_set = set(fact_ids)
    if len(fact_ids) != len(fact_set):
        errors.append("fact ids must be unique across the proposal")
    for fact in facts:
        _check_lineage(
            f"fact {fact.fact_id}",
            fact.lineage,
            fact.evidence_ids,
            fact.direction_requirement_ids,
            synopsis_ids=synopsis_ids,
            observation_ids=evidence_observations,
            requirement_ids=requirement_ids,
            errors=errors,
        )

    for entity in proposal.entities:
        if entity.primary_class in entity.facets:
            errors.append(f"entity {entity.entity_id}: primary class repeated in facets")
        if len(set(entity.facets)) != len(entity.facets):
            errors.append(f"entity {entity.entity_id}: duplicate facets")

    relationship_ids = [rel.relationship_id for rel in proposal.relationships]
    if len(relationship_ids) != len(set(relationship_ids)):
        errors.append("relationship ids must be unique")
    by_id = {entity.entity_id: entity for entity in proposal.entities}
    edges: list[tuple[str, str]] = []
    incident: Counter[str] = Counter()
    for rel in proposal.relationships:
        for endpoint in (rel.source_entity_id, rel.target_entity_id):
            if endpoint not in entity_set:
                errors.append(f"relationship {rel.relationship_id}: unknown entity {endpoint}")
        if rel.source_entity_id == rel.target_entity_id:
            errors.append(f"relationship {rel.relationship_id}: self relationship")
        if rel.relationship_family == "taxonomic":
            classes = {
                by_id[endpoint].primary_class
                for endpoint in (rel.source_entity_id, rel.target_entity_id)
                if endpoint in by_id
            }
            if "kind" not in classes:
                errors.append(
                    f"relationship {rel.relationship_id}: taxonomic edges need a kind endpoint"
                )
        edges.append((rel.source_entity_id, rel.target_entity_id))
        incident[rel.source_entity_id] += 1
        incident[rel.target_entity_id] += 1
        _check_lineage(
            f"relationship {rel.relationship_id}",
            rel.lineage,
            rel.evidence_ids,
            rel.direction_requirement_ids,
            synopsis_ids=synopsis_ids,
            observation_ids=evidence_observations,
            requirement_ids=requirement_ids,
            errors=errors,
        )
    isolated = sorted(entity_id for entity_id in entity_ids if incident[entity_id] == 0)
    if isolated:
        errors.append(f"entities without relationships: {isolated}")
    if count and len(proposal.relationships) < count:
        errors.append("relationship count must be at least the entity count")
    if count and len(proposal.relationships) > 3 * count:
        errors.append("relationship count must not exceed three times the entity count")
    components = _connected(entity_set, edges) if entity_set else 0
    if components > 1:
        errors.append(f"relationship graph has {components} components; expected 1")
    families = {rel.relationship_family for rel in proposal.relationships}
    if count and len(families) < 4:
        errors.append("relationships must span at least four families")

    marker_owner: Counter[str] = Counter()
    thing_ids = {e.entity_id for e in proposal.entities if e.primary_class == "thing"}
    for marker in proposal.identity_markers:
        if marker.owner_entity_id not in entity_set:
            errors.append(f"marker {marker.marker_id}: unknown owner {marker.owner_entity_id}")
        marker_owner[marker.owner_entity_id] += 1
        if (
            marker.independently_admitted_thing_id is not None
            and marker.independently_admitted_thing_id not in thing_ids
        ):
            errors.append(f"marker {marker.marker_id}: admitted thing is not a thing entity")
        _check_lineage(
            f"marker {marker.marker_id}",
            marker.lineage,
            marker.evidence_ids,
            marker.direction_requirement_ids,
            synopsis_ids=synopsis_ids,
            observation_ids=evidence_observations,
            requirement_ids=requirement_ids,
            errors=errors,
        )
    for entity in proposal.entities:
        needs_marker = entity.primary_class == "collective" or (
            entity.primary_class == "place" and entity.salience == "major"
        )
        if needs_marker and marker_owner[entity.entity_id] == 0:
            errors.append(f"entity {entity.entity_id}: collective or major place needs a marker")

    if not proposal.viewpoints:
        errors.append("at least one audience viewpoint is required")
    for viewpoint in proposal.viewpoints:
        unknown = set(viewpoint.anchor_entity_ids) - entity_set
        if unknown or not viewpoint.anchor_entity_ids:
            errors.append(f"viewpoint {viewpoint.viewpoint_id}: anchors must be known entities")

    if len(proposal.institutional_tensions) < 3:
        errors.append("at least three institutional tensions are required")
    for tension in proposal.institutional_tensions:
        if set(tension.fact_ids) - fact_set:
            errors.append(f"tension {tension.tension_id}: unknown fact ids")
        if set(tension.participant_entity_ids) - entity_set:
            errors.append(f"tension {tension.tension_id}: unknown participants")
        if len(tension.participant_entity_ids) < 2:
            errors.append(f"tension {tension.tension_id}: needs at least two participants")

    owned_ids = (
        entity_set
        | fact_set
        | set(relationship_ids)
        | {m.marker_id for m in proposal.identity_markers}
        | {t.tension_id for t in proposal.institutional_tensions}
        | {v.viewpoint_id for v in proposal.viewpoints}
        | observation_ids
    )
    covered = [item.requirement_id for item in proposal.direction_coverage]
    if sorted(covered) != sorted(requirement_ids):
        errors.append("direction_coverage must list every requirement id exactly once")
    for item in proposal.direction_coverage:
        if not item.evidence_ids or set(item.evidence_ids) - owned_ids:
            errors.append(f"coverage {item.requirement_id}: evidence must be proposal-owned ids")
    for conflict in proposal.source_conflicts:
        if set(conflict.evidence_ids) - owned_ids - synopsis_ids - observation_ids:
            errors.append(f"conflict {conflict.conflict_id}: unknown evidence ids")
    if not proposal.unresolved_questions:
        errors.append("at least one unresolved question is required")

    class_counts = Counter(entity.primary_class for entity in proposal.entities)
    return {
        "status": "pass" if not errors else "fail",
        "errors": errors,
        "metrics": {
            "entity_count": count,
            "class_counts": dict(sorted(class_counts.items())),
            "fact_count": len(facts),
            "generated_extension_fact_count": sum(
                fact.lineage == "generated_extension" for fact in facts
            ),
            "relationship_count": len(proposal.relationships),
            "relationship_family_count": len(families),
            "connected_components": components,
            "identity_marker_count": len(proposal.identity_markers),
            "institutional_tension_count": len(proposal.institutional_tensions),
            "viewpoint_count": len(proposal.viewpoints),
            "unresolved_question_count": len(proposal.unresolved_questions),
        },
    }


_CONTRAST_CLASSES: Final = frozenset({"system", "idea"})
_CONTRAST_MODES: Final = frozenset({"visible_system_instance", "practiced_or_contested_idea"})


def _needs_contrast(primary_class: str, concept_mode: str) -> bool:
    return primary_class in _CONTRAST_CLASSES or concept_mode in _CONTRAST_MODES


def _is_contrast(text: str) -> bool:
    stripped = text.strip().lower()
    return len(stripped) >= 20 and not stripped.startswith("none")


def evaluate_plan(plan: GalleryPlan, proposal: UniverseProposal) -> PlanEvaluation:
    errors: list[str] = []
    warnings: list[str] = []
    if plan.universe_id != proposal.universe_id:
        errors.append("plan universe_id must match the proposal")
    entity_ids = [entity.entity_id for entity in proposal.entities]
    planned = [item.entity_id for item in plan.plans]
    if sorted(planned) != sorted(entity_ids):
        errors.append("plan must contain exactly one entry per admitted entity")
    if len(planned) != len(set(planned)):
        errors.append("plan entity ids must be unique")
    by_entity = {entity.entity_id: entity for entity in proposal.entities}
    relationships = {rel.relationship_id: rel for rel in proposal.relationships}
    keys: Counter[tuple[str, str, str, str, str]] = Counter()
    weather: Counter[str] = Counter()
    time_of_day: Counter[str] = Counter()
    purposes: Counter[str] = Counter()
    scales: Counter[str] = Counter()
    settings: Counter[str] = Counter()
    population: Counter[str] = Counter()
    motifs: Counter[tuple[str, str]] = Counter()
    props: Counter[str] = Counter()
    verbs: Counter[str] = Counter()
    vantages: Counter[str] = Counter()
    for item in plan.plans:
        motif = item.signature_motif
        motifs[motif.key()] += 1
        props[motif.dominant_prop] += 1
        verbs[motif.action_verb] += 1
        vantages[motif.vantage] += 1
        population[item.scene_register.population] += 1
        entity = by_entity.get(item.entity_id)
        if entity is None:
            continue
        if _needs_contrast(entity.primary_class, item.concept_mode) and not _is_contrast(
            item.in_frame_contrast
        ):
            errors.append(
                f"plan {item.entity_id}: a {entity.primary_class} scene must stage an "
                "in_frame_contrast (two states of the mechanism visible in one frame)"
            )
        allowed_modes = set(MODES_BY_CLASS[entity.primary_class])
        for facet in entity.facets:
            allowed_modes.update(MODES_BY_CLASS[facet])
        if item.concept_mode not in allowed_modes:
            errors.append(
                f"plan {item.entity_id}: mode {item.concept_mode} invalid for "
                f"{entity.primary_class} with facets {entity.facets}"
            )
        entity_fact_ids = {fact.fact_id for fact in entity.facts}
        if not item.visible_fact_ids or set(item.visible_fact_ids) - entity_fact_ids:
            errors.append(f"plan {item.entity_id}: visible facts must be the entity's own facts")
        for rel_id in item.visible_relationship_ids:
            rel = relationships.get(rel_id)
            if rel is None or item.entity_id not in (rel.source_entity_id, rel.target_entity_id):
                warnings.append(
                    f"plan {item.entity_id}: relationship {rel_id} is not incident; ignored"
                )
        register = item.scene_register
        keys[register.key()] += 1
        weather[register.weather] += 1
        time_of_day[register.time_of_day] += 1
        purposes[item.primary_purpose] += 1
        scales[register.scale] += 1
        settings[register.setting] += 1
        if register.weather not in DRY_WEATHER and len(register.weather_justification) < 20:
            errors.append(f"plan {item.entity_id}: non-dry weather needs a scene justification")
        if "mood" in register.weather_justification.lower():
            errors.append(f"plan {item.entity_id}: weather may not be justified by mood")
    crowded = sorted(key for key, n in keys.items() if n > 2)
    if crowded:
        errors.append(f"scene registers used more than twice: {crowded}")
    repeated = sorted(key for key, n in keys.items() if n == 2)
    if repeated:
        warnings.append(f"scene registers used twice: {repeated}")
    total = len(plan.plans)
    if total:
        wet = sum(weather[w] for w in WET_WEATHER)
        if wet > total * 0.25:
            errors.append(f"rain and storm cover {wet}/{total} entries; limit is one quarter")
        if time_of_day["night"] > total / 3:
            errors.append("night covers more than one third of entries")
        if time_of_day["day"] < total * 0.2:
            errors.append("plain day covers less than one fifth of entries")
        top_purpose, top_count = purposes.most_common(1)[0]
        if top_count > total * 0.3:
            errors.append(
                f"purpose {top_purpose} covers {top_count}/{total}; limit is three tenths"
            )
        if total >= 18 and len(purposes) < 6:
            errors.append("a gallery this size must use at least six distinct purposes")
        if population["crowd"] > total * 0.4:
            errors.append(
                f"crowd covers {population['crowd']}/{total} entries; limit is two fifths"
            )
        if total >= 12 and population["solitary"] < total / 6:
            errors.append(
                f"solitary covers {population['solitary']}/{total} entries; floor is one sixth"
            )
        top_vantage, vantage_count = vantages.most_common(1)[0]
        if vantage_count > total / 2:
            errors.append(f"vantage {top_vantage} covers {vantage_count}/{total}; limit is half")
        if total >= 18 and len(vantages) < 4:
            errors.append("a gallery this size must use at least four vantages")
    repeated_motifs = sorted(key for key, n in motifs.items() if n > 1)
    if repeated_motifs:
        errors.append(f"signature motif (action_verb, dominant_prop) repeated: {repeated_motifs}")
    crowded_props = sorted(key for key, n in props.items() if n > 2)
    if crowded_props:
        errors.append(f"dominant_prop used by more than two entries: {crowded_props}")
    crowded_verbs = sorted(key for key, n in verbs.items() if n > 3)
    if crowded_verbs:
        errors.append(f"action_verb used by more than three entries: {crowded_verbs}")
    # Scale and setting coverage are set-level obligations the register prompt
    # states unconditionally. They were nested under the over-used-verb branch
    # in the spike, so a gallery only had to spread its scales once it had
    # already failed something else.
    if total >= 12:
        for scale in ("intimate", "human", "architectural", "landscape"):
            if scales[scale] == 0:
                errors.append(f"scale {scale} is unused")
        if settings["interior"] == 0:
            errors.append("no interior scene planned")
    unique = [item.unique_contribution.strip().lower() for item in plan.plans]
    if len(unique) != len(set(unique)):
        errors.append("unique_contribution statements must differ")
    lessons: Counter[str] = Counter(item.lesson_key for item in plan.plans)
    repeated_lessons = sorted(key for key, n in lessons.items() if n > 1)
    if repeated_lessons:
        errors.append(f"lesson_key repeated across entries: {repeated_lessons}")
    return {
        "status": "pass" if not errors else "fail",
        "errors": errors,
        "warnings": warnings,
        "metrics": {
            "entry_count": total,
            "weather": dict(sorted(weather.items())),
            "time_of_day": dict(sorted(time_of_day.items())),
            "purposes": dict(sorted(purposes.items())),
            "scales": dict(sorted(scales.items())),
            "settings": dict(sorted(settings.items())),
            "purpose_vocabulary_size": len(CONCEPT_PURPOSES),
            "population": dict(sorted(population.items())),
            "vantages": dict(sorted(vantages.items())),
            "distinct_dominant_props": len(props),
            "distinct_action_verbs": len(verbs),
        },
    }


WET_WORDS = ("rain", "storm", "downpour", "drizzle", "puddle", "soaked", "sodden")
NIGHT_WORDS = ("night", "moon", "lantern", "dark", "starlight", "torch")
_WET_ALTERNATION = (
    r"rain|rains|raining|rainfall|rainy|storm|storms|stormy"
    r"|downpour|drizzle|puddle|puddles|soaked|sodden"
)
_NEGATION_ALTERNATION = (
    r"no|not|without|never|absent|absence of|free of|rather than|instead of"
    r"|before|after|since|last|previous|earlier|old|dried|drying"
)
# Word boundaries, not substrings: "rain" matched inside "restrained" and
# "terrain" for a whole gallery run before this was anchored.
_WET_RE = re.compile(rf"\b({_WET_ALTERNATION})\b")
_NEGATED_WET_RE = re.compile(
    rf"\b({_NEGATION_ALTERNATION})(\s+\w+){{0,2}}\s+({_WET_ALTERNATION})\b"
    r"|\b(rain|storm)[- ]?(free|less)\b"
)


def wet_mentions(text: str) -> list[str]:
    """Positive mentions of wet weather, ignoring negations and past references."""

    stripped = _NEGATED_WET_RE.sub(" ", text.lower())
    return sorted({match.group(0) for match in _WET_RE.finditer(stripped)})


def evaluate_direction(
    direction: EntityDirection, plan: ConceptPlan, medium: MediumContract
) -> list[str]:
    """Hard acceptance: only the identity binding. Lexical checks are warnings."""

    if direction.entity_id != plan.entity_id:
        return ["direction entity_id must match the plan entry"]
    return []


def direction_warnings(
    direction: EntityDirection, plan: ConceptPlan, medium: MediumContract
) -> list[str]:
    """Lexical register and medium hints; negation-prone, so advisory only.

    The independent image review grades register and medium fidelity on the
    rendered image, which is the check that matters.
    """

    errors: list[str] = []
    prose = direction.prose()
    lowered = prose.lower()
    forbidden = forbidden_terms_present(medium, prose)
    if forbidden:
        errors.append(f"direction uses terms foreign to medium {medium.medium_id}: {forbidden}")
    register = plan.scene_register
    if register.weather in DRY_WEATHER:
        hits = wet_mentions(prose)
        if hits:
            errors.append(f"dry scene ({register.weather}) describes wetness: {hits}")
    elif register.weather in WET_WEATHER and not _WET_RE.search(lowered):
        errors.append(f"wet scene ({register.weather}) never names its weather")
    realization = direction.register_realization.lower()
    night_words = re.compile(r"\b(" + "|".join(NIGHT_WORDS) + r")\w*\b")
    if register.time_of_day == "night" and not night_words.search(realization):
        errors.append("night scene realization never establishes night")
    if register.time_of_day != "night" and re.search(r"\bnight\b", realization):
        errors.append(f"{register.time_of_day} scene realization describes night")
    for term in ("diagram", "map ", "label", "caption", "infographic", "turnaround"):
        if term in lowered and term not in " ".join(direction.avoid).lower():
            errors.append(f"direction stages an explanatory form: {term.strip()}")
    return errors


#: The schema file each structured step answers to, and the model its answer parses as.
SCHEMA_MODELS: dict[str, tuple[type[ContractModel], str]] = {
    "proposal.json": (UniverseProposal, "universe.propose"),
    "plan.json": (GalleryPlan, "universe.plan"),
    "review.json": (SemanticReview, "universe.review"),
    "global-direction.json": (GlobalDirection, "universe.direction.global"),
    "entity-direction.json": (EntityDirection, "universe.direction.entity"),
    "image-review.json": (ImageReview, "universe.review.image"),
}


def json_schemas() -> dict[str, dict[str, object]]:
    """The workflow's ``schemas/`` files, as these models state them."""

    return {
        name: {
            **model.model_json_schema(),
            "description": f"universe structured output for {operation}",
        }
        for name, (model, operation) in SCHEMA_MODELS.items()
    }
