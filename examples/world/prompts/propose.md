Create one complete expanded universe proposal for "${{ inputs.title }}" from the attached reference poster, the synopsis, and the expansion direction that follow.

V0 UNIVERSE ONTOLOGY (stable semantic classes; no franchise-menu roots)
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

IDENTITY MARKERS are text records attached to their owner (flag, sigil, livery, knot pattern, color pairing, material motif, gesture, script, sound). A marker becomes a thing only when it has independent history, custody, or conflict. A marker may appear as a subordinate diegetic detail inside its owner's scene; it never gets a standalone image.

CENSUS
- Set universe_id exactly to ${{ inputs.universe_id }}. Emit between ${{ inputs.census.min_entities }} and ${{ inputs.census.max_entities }} entities. The distribution across classes must be irregular and honest: choose what this world needs, not a quota. A large universe still needs several systems, at least two events, at least two ideas, and at least two kinds unless the world truly lacks them.
- Every entity gets two to four concise lineaged facts, a summary a cold reader understands, a how_it_works_or_lives paragraph, and a present_tension sentence. Every entity takes part in at least one relationship. Emit at least as many relationships as entities and at most three times as many, spanning at least four relationship families. The graph must be ONE connected component: before returning, walk the relationships and confirm every entity can reach every other; a small cluster linked only to itself (for example one actor and the collective they belong to) fails. Every actor connects to a place and to at least one non-actor class; every collective connects to a place or a system it operates in.
- Give every collective and every major place a culturally grounded identity marker record with correct lineage. A marker whose form is invented uses generated_extension and cites a direction id.
- Emit one or more audience viewpoints, at least three institutional tensions with material stakes on each side, typed poster observations (ids prefixed poster_), source conflicts where the poster and synopsis disagree, direction coverage for every requirement id exactly once, citing evidence_ids copied byte-for-byte from ids this proposal emits (entity, fact, relationship, marker, tension, viewpoint, or poster observation ids; never a field name such as relationships, never a paraphrased or re-prefixed id), and unresolved questions the source leaves open.
- Fact and relationship evidence_ids may contain only synopsis paragraph ids and poster observation ids with canonical_status evidence. Direction requirement ids appear only in direction_requirement_ids.
- Elaboration changes lineage: a fact is explicit_source only when every specific in its claim is stated by the cited paragraph. The moment a claim adds a category, mechanism, custom, practice, number, or name the synopsis does not state, the whole fact becomes conservative_inference (if the synopsis implies it) or generated_extension (if the direction motivates it). Split a sourced core and an invented elaboration into two facts rather than blending them under explicit_source.
- Lineage honesty: a detail you invent for texture (a marker's colour, a custom, a tool's construction) is generated_extension with a direction id, or conservative_inference when the synopsis implies it; it is never explicit_source. A causal relationship_kind (caused, resulted_in, prevents) requires the synopsis to state that cause; where the synopsis leaves a cause open, use depends_on or a hedged summary with conservative_inference and keep the open cause in unresolved_questions.
- One record per practice: never emit both a collective and a separate system for the same craft or trade (a guild and its craft, a gleaner band and gleaning, riggers and rigging). Choose one: either the collective, whose how_it_works_or_lives explains the practice, or the system, whose practitioners appear as relationships to actors, collectives, and places. A physical infrastructure (a water system, a road network) and the workforce that runs it may both exist only when the workforce record is about politics, livelihood, and authority and never restates the infrastructure's operation.
- Taxonomic relationships (instance_of, variant_of, descended_from) link an individual or subtype to a kind; an actor belonging to a collective is member_of, never instance_of. An assembly or installation is not an instance of the kind of its parts.
- Every relationship must read as a true sentence "<source> <relationship_kind> <target>" with the endpoints' classes: a system is not carried, made, or owned; a place does not use a tool; a kind does not act. If the sentence is false or awkward, choose another kind or drop the edge.
- A bounded event made central by the synopsis must state when it happens, what participates, what physically changes and what remains unresolved; never import names, festivals or history from another world.
- Do not add gameplay statistics, decorative creatures, generic magic, or franchise-shaped filler. Do not describe any image; concept planning happens later.
Return only the strict schema object.

${{ vars.source.section }}
