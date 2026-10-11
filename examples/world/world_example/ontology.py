"""V0 universe ontology vocabulary as prompt text and closed literal sets.

This is the checked-in taxonomy (docs/spec/universe/taxonomy-v0.md) projected
into the words a proposal, plan, or review model is told. It is medium-free.
"""

from __future__ import annotations

# The prompt blocks below are tuned prose carried over verbatim. Rewrapping them
# to satisfy the line limit would risk changing the bytes a model is sent, which
# is a correctness question, not a style one.
# ruff: noqa: E501
from typing import Final, Literal

EntityClass = Literal["actor", "collective", "place", "thing", "kind", "system", "event", "idea"]
ENTITY_CLASSES: Final[tuple[str, ...]] = (
    "actor",
    "collective",
    "place",
    "thing",
    "kind",
    "system",
    "event",
    "idea",
)

RelationshipFamily = Literal[
    "spatial",
    "social_political",
    "material_functional",
    "taxonomic",
    "causal_historical",
    "symbolic",
]

LineageKind = Literal[
    "explicit_source",
    "visual_observation",
    "conservative_inference",
    "generated_extension",
]

ConceptPurpose = Literal[
    "orient",
    "identify",
    "differentiate",
    "manifest",
    "connect",
    "historicize",
    "humanize",
    "immerse",
    "invite",
]
CONCEPT_PURPOSES: Final[tuple[str, ...]] = (
    "orient",
    "identify",
    "differentiate",
    "manifest",
    "connect",
    "historicize",
    "humanize",
    "immerse",
    "invite",
)

ConceptMode = Literal[
    "environmental_identity_portrait",
    "in_world_action",
    "representative_public_manifestation",
    "establishing_environment",
    "inhabited_environment",
    "hero_object_in_use",
    "representative_specimen_in_context",
    "visible_system_instance",
    "witnessed_event_moment",
    "practiced_or_contested_idea",
]

#: Which concept modes a primary class may use. Facets add context, not modes.
MODES_BY_CLASS: Final[dict[str, tuple[str, ...]]] = {
    "actor": ("environmental_identity_portrait", "in_world_action"),
    "collective": (
        "representative_public_manifestation",
        "in_world_action",
        "inhabited_environment",
    ),
    "place": (
        "establishing_environment",
        "inhabited_environment",
        "environmental_identity_portrait",
    ),
    "thing": ("hero_object_in_use",),
    "kind": ("representative_specimen_in_context",),
    "system": ("visible_system_instance",),
    "event": ("witnessed_event_moment",),
    "idea": ("practiced_or_contested_idea",),
}

#: Native raster sizes for the bound image route, chosen by concept mode.
WIDE_SIZE: Final = "2560x1440"
STANDARD_SIZE: Final = "2560x1712"
PORTRAIT_SIZE: Final = "1712x2560"
SIZE_BY_MODE: Final[dict[str, str]] = {
    "environmental_identity_portrait": PORTRAIT_SIZE,
    "in_world_action": STANDARD_SIZE,
    "representative_public_manifestation": WIDE_SIZE,
    "establishing_environment": WIDE_SIZE,
    "inhabited_environment": STANDARD_SIZE,
    "hero_object_in_use": PORTRAIT_SIZE,
    "representative_specimen_in_context": STANDARD_SIZE,
    "visible_system_instance": STANDARD_SIZE,
    "witnessed_event_moment": WIDE_SIZE,
    "practiced_or_contested_idea": STANDARD_SIZE,
}

# Scene register: the set-level vocabulary the gallery plan must spread across.
Scale = Literal["intimate", "human", "architectural", "landscape"]
TimeOfDay = Literal["dawn", "day", "dusk", "night"]
Weather = Literal["clear", "overcast", "wind", "fog", "rain", "storm", "snow"]
Setting = Literal["interior", "exterior", "threshold"]
Population = Literal["solitary", "few", "crowd"]
Energy = Literal["still", "working", "urgent"]
Vantage = Literal["eye_level", "low_angle", "high_angle", "aerial", "over_shoulder", "from_within"]

WET_WEATHER: Final[frozenset[str]] = frozenset({"rain", "storm"})
DRY_WEATHER: Final[frozenset[str]] = frozenset({"clear", "overcast", "wind"})

ONTOLOGY_PROMPT: Final = """V0 UNIVERSE ONTOLOGY (stable semantic classes; no franchise-menu roots)
- actor: an individual subject with agency, intention, experience, or viewpoint. Named creatures, machines, or artifacts with agency qualify. A species is a kind, not an actor.
- collective: a plurality of actors with shared identity, authority, purpose, or coordinated action: guilds, councils, crews, towns-as-polities, movements, families acting as institutions.
- place: a locus that can contain subjects, be entered or observed, and take part in spatial relationships: regions, cities, districts, buildings, rooms, mobile interiors, overlapping layers.
- thing: a bounded material or informational subject whose identity matters: tools, vehicles, documents, garments, resources, structures treated as objects.
- kind: a reusable category whose members share world-significant traits: species, creature types, plant kinds, tool classes, structure classes.
- system: a repeatable rule, process, infrastructure, or circulation with recognizable consequences: ecology, water, law, trade, craft practice, calendar, signalling. A system must connect to something that visibly demonstrates it.
- event: a bounded change in time that alters the world's state or interpretation. It must record what participated and what changed.
- idea: a belief, doctrine, value, taboo, law, prophecy, or interpretation that influences the world. It must be held, contested, enforced, embodied, or suffered by actors or collectives.

FACETS: each entity declares exactly one primary_class and may add facets when the subject crosses a boundary (a freighter that is also a home: thing + place facet; a forest that is an organism and a moving territory: place + kind or system facet). Never repeat the primary class inside facets. Never duplicate one subject into two records.

RELATIONSHIP FAMILIES AND KINDS (relationship_kind is a lower_snake_case verb phrase):
- spatial: located_in, contains, adjacent_to, connected_to, reachable_from, overlaps, hidden_within, moves_through, mirrors, replaces, originates_from, travels_through
- social_political: member_of, leads, serves, governs, represents, allied_with, opposes, protects, exploits, employs, trains, certifies
- material_functional: owns, uses, created, requires, produces, consumes, made_from, powers, damages, maintains, carries
- taxonomic: instance_of, variant_of, descended_from, related_to
- causal_historical: caused, participated_in, changed_by, preceded, resulted_in, prevents, depends_on, survived
- symbolic: represents, identified_by, sacred_to, forbidden_by, commemorates

LINEAGE (per fact and per relationship):
- explicit_source: stated by the synopsis; cite synopsis paragraph ids.
- visual_observation: literally visible in the poster; cite poster observation ids.
- conservative_inference: needed to connect supplied evidence; cite the evidence it connects.
- generated_extension: admitted because the expansion direction asked for it; cite the evidence it extends AND at least one direction requirement id. Direction ids are rationale, never evidence.

CONCEPT PURPOSES (what one image is for): orient (where is it, how is it reached), identify (how do I recognize it), differentiate (how is it unlike related subjects), manifest (what observable form makes it physically legible), connect (what it affects, uses, opposes, or depends on), historicize (why is it like this now), humanize (how it is experienced in ordinary life), immerse (what it feels like to encounter), invite (what question makes the audience continue).

CONCEPT MODES BY CLASS: actor -> environmental_identity_portrait or in_world_action; collective -> representative_public_manifestation or in_world_action; place -> establishing_environment or inhabited_environment; thing -> hero_object_in_use; kind -> representative_specimen_in_context; system -> visible_system_instance; event -> witnessed_event_moment; idea -> practiced_or_contested_idea. An entity may also use a mode allowed by one of its declared facets.

IDENTITY MARKERS are text records attached to their owner (flag, sigil, livery, knot pattern, color pairing, material motif, gesture, script, sound). A marker becomes a thing only when it has independent history, custody, or conflict. A marker may appear as a subordinate diegetic detail inside its owner's scene; it never gets a standalone image."""

REGISTER_PROMPT: Final = """SCENE REGISTER (set-level diversity vocabulary; every plan entry picks one value per axis)
- scale: intimate (hands, faces, one object), human (one or a few full figures), architectural (a structure or street with people for scale), landscape (terrain, sky, the forest as a mass)
- time_of_day: dawn, day, dusk, night
- weather: clear, overcast, wind, fog, rain, storm, snow. Wet weather must be justified by what the scene is about; it is never a mood.
- setting: interior, exterior, threshold (doorway, gate, canopy edge, bridge, wall breach)
- population: solitary, few, crowd
- energy: still, working, urgent
The set must spread across every axis. The deterministic validator rejects a plan where: any (scale, time_of_day, weather, setting, population) combination is used by more than two entries; rain and storm together exceed one quarter of entries; night exceeds one third; plain day is under one fifth; crowd exceeds two fifths; solitary is under one sixth; any single purpose exceeds three tenths; fewer than six purposes are used; any scale value is unused; no interior scene exists; a non-dry weather (fog, rain, storm, snow) lacks a full-sentence physical justification; a justification mentions mood; or two unique_contribution sentences are identical.

SIGNATURE MOTIF (composition vocabulary; every plan entry declares one)
A scene register spreads light and weather, but two images with different registers can still be the same picture: a crowd, some ropes, a timber frame, and a grey wall. The signature motif names what a viewer would sketch in three strokes to tell this image from every other one in the set.
- action_verb: one lower_snake_case verb naming the primary visible action (hauling, cutting, reading, waiting, burying, climbing, pouring). Never a state such as standing or existing.
- dominant_prop: one lower_snake_case noun naming the single largest legible object mass in the frame, specific to this scene (peg_beam, kite_line, anchor_column, root_heart, mooring_capstan). Never a generic such as rope, wall, tree, or crowd.
- vantage: eye_level, low_angle (looking up at the subject), high_angle (looking down from a height), aerial (far above, terrain reads as a map of masses), over_shoulder (past a foreground figure), from_within (inside a structure, root arch, or vehicle looking out).
The validator rejects a plan where: two entries share the same action_verb and dominant_prop; any dominant_prop is used by more than two entries; any action_verb is used by more than three entries; any vantage covers more than half the entries; or fewer than four vantages are used."""
