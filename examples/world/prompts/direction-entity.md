Compile exactly one text-only concept direction for the bound entity. This request has zero image references and zero masks.

${{ vars.source.medium.compile_guidance }}

- primary_subject: the one primary entity as it will be seen, presence-only language.
- action_beat: agent, goal, obstacle, intervention, and visible_state_change of the single moment. For an event this is its decisive causal beat; for anything else it is the consequential micro-action that makes the entity legible.
- immediate_environment: only what is physically present in this one continuous scene.
- composition_and_camera: framing, scale placement, and depth in the medium's own terms. Realize the sealed signature_motif: the vantage is the camera's vantage, the dominant_prop is the largest legible mass in the frame, and the action_verb is what the primary subject is visibly doing. If the plan states an in_frame_contrast, place both states in the same frame where the eye reads them together. Never an isometric, orthographic, catalog-plate, or cutaway view; the scene is seen from a place a person or bird could stand.
- Lighting follows the register's time of day in value, not only in colour: a day scene is high-key with most of the frame above mid value and shadows that stay open; dusk and dawn keep a lit sky; only night and storm may be low-key.
- visual_identity: silhouette, proportions, construction_logic, materials, color_placement, scale_anchor, wear_and_history, characteristic_motion_or_use, and forbidden_substitutions. This is the medium-neutral identity a viewer must recognize.
- register_realization: one paragraph that establishes the sealed scene register (scale, time of day, weather, setting, population, energy) as concrete visible conditions. A dry scene stays dry; a night scene must show how it is lit.
- continuity_notes: what must agree with the global grammar.
- avoid: concrete things this scene must not contain. Always include explanatory staging and readable text.
- Identity markers are text records; do not stage a flag, crest, seal, badge, or logo. A subordinate culturally grounded applied marking on the owner's own materials is acceptable only when nothing can be read from it.
- Do not name or imitate any franchise, studio, artist, or performer. Do not use words foreign to the medium.
Return only the strict schema object.

Bound entity_id: ${{ vars.entity_id }}

GLOBAL VISUAL GRAMMAR
${{ vars.grammar }}

ENTITY, RELATIONSHIPS, AND SEALED PLAN ENTRY
${{ vars.context }}
