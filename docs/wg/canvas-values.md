# Canvas values

**Status: approved design and local implementation notes for
[#81](https://github.com/gridaco/fx/issues/81), 2026-11-16.**
The normative contract is [recorded canvas values](../../spec/values.md).

## User outcome

A reader understands a workflow's prompt, reference image, shared configuration and
result from its canvas. Opening details is for full inspection rather than discovering
every value. Workflow authors keep using ordinary FX data declarations; presentation
does not require extra executable steps or copies of data in Markdown notes.

## Grounding before implementation

Notes (#80), media previews/fullscreen, file delegation and automatic layout already
provide much of the presentation infrastructure. The data feature had these gaps:

- Step cards show settings names. Recorded effective `with` values are available in
  details but are dropped from the canvas model.
- Imported workflow input/output cards show port names and types, not values. Root
  workflow boundary cards do not exist.
- Root inputs, tables and named expressions lack first-class presentation provenance.
  Existing output/fact tracing can follow some aliases and imported interfaces.
- Saved plan `inputs` contains supplied values, not the effective defaults-filled input
  environment. Renaming that field to “effective inputs” would be incorrect.
- `let` is evaluated lazily in the referencing frame. One declaration can yield different
  values for different items, matrices or takes. It is not necessarily a global constant.
- Existing event encoding collapses null and missing. New presentation evidence must
  preserve their distinction at capture time, without changing execution encodings.
- Plan route selection is recorded, but the run viewer projection currently drops it.

## References and the design judgment

[ComfyUI's primitive implementation](https://github.com/Comfy-Org/ComfyUI_frontend/blob/main/src/extensions/core/widgetInputs.ts)
uses a virtual node to propagate a shared widget value. Its widgets also keep parameters
near their consumers. The useful UX is visible values and explicit shared sources. FX
already has named inputs/tables/expressions and should project those declarations.

[n8n's expression guidance](https://docs.n8n.io/build/work-with-data/expressions-versus-data-nodes.md)
favours expressions for simple parameter references and immediate computed-value previews.
Its [Edit Fields node](https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.set/)
also distinguishes fixed values from expressions; that node actually changes workflow
data. FX needs the distinction and readable results, while its existing language already
owns how values are produced.

The recommendation is automatic value presentation using the existing language. Do not
introduce `const`, `show`, a custom-view framework or new editor controls for this delivery.

## Authoring stays the same

This is ordinary current FX syntax; the proposal changes how its recorded data is shown:

```yaml
fx: workflow/v1
id: reference-variation
title: A variation from a reference
inputs:
  prompt: {type: string, default: A small ceramic lantern}
  reference: {type: file, kind: image}
tables:
  canvas: {size: 1024x1024, longest_side: 512}
let:
  brief: "${{ inputs.prompt }}. Soft daylight."
steps:
  draw:
    uses: fx/image.edit@1
    with:
      image: ${{ inputs.reference }}
      prompt: ${{ let.brief }}
      size: ${{ tables.canvas.size }}
  thumbnail:
    uses: fx/image.resize@1
    with:
      image: ${{ steps.draw.outputs.image }}
      longest_side: ${{ tables.canvas.longest_side }}
outputs:
  image: ${{ steps.thumbnail.outputs.image }}
```

No provider is called to populate a card. The caller supplies the reference through the
existing input mechanism and chooses routes through existing project configuration.

## Canvas experience

| Surface | Default presentation |
| --- | --- |
| Workflow inputs | Named input values packed in one Inputs frame on the left: readable prompt excerpt, reference thumbnail, compact scalar rows. Group short scalar fields rather than adding a large card for each. |
| Shared tables | Effective fields are available at consuming ports; captured sources remain available for explicit inspection. No standalone card by default. |
| Named expressions | Captured results are available at consuming ports. Context-dependent occurrences stay distinct in the evidence, without standalone cards by default. |
| Executable steps | Compact inline summaries for unconnected settings and recorded route/model. Connected values and outputs are inspected from their socket dots or labels. |
| Workflow outputs | Named result values/media packed in one Outputs frame on the right, with existing preview, fullscreen and external file actions. |
| Notes | Authored explanation, retaining the distinct note appearance and static semantics from #80. |

Source evidence is driven by actual recorded use; unused `let` declarations are not
evaluated to decorate the canvas. Explicit `reveal_value` can materialize one selected
source occurrence until selection changes. Default wiring follows recorded typed links
through hidden sources without crossing a closed import's private boundary. Short scalars
can share compact cards/rows, while long text and media have their own readable space.
Logical value references do not depend on
how rows are grouped. Grouping is renderer policy, not workflow syntax.
Each grouped row owns its named socket and activation target; selecting/revealing it uses
that row's recorded value ID. Grouping creates no aggregate value identity.
The displayed workflow's boundary frames pack their cards independently of processing
row heights, including when the displayed workflow is an opened import. They introduce
no new execution scope or authoring setting. Empty sides have no frame. This grouping
keeps intermediate source cards out of the overview. Boundary connections are hidden at
rest and revealed for the individual value or connected step being hovered, keyboard-focused or selected.
Selection keeps those connections visible. Frame headings/padding do not activate the
whole group, and hidden wires have no hit region. Processing connections remain visible.

Hovering a socket dot or label at 75% zoom or above opens a recorded-value
popover. Below 75%, hover opens no popover and adds no port-row tint. Explicit click/tap
and keyboard focus work at any zoom, with active preview feedback. The preview
highlights only that port's connections.
Click/tap pins it; Escape or an outside press closes it. Pan, zoom and navigation
dismiss it. Recorded images show one thumbnail or a compact image collection using
only verified recorded artifact inventory and existing viewer URLs. Pinned image previews
use the shared fullscreen control. Other media show metadata here and retain existing
cards/details for media inspection. Missing bytes and pending values keep explicit
fallbacks. The popover changes no card footprint, evaluates nothing, generates no content
and discovers no provider data; planned outputs may remain pending. This is
presentation behavior without new workflow syntax or viewer configuration. Descriptions
come only from captured node parameter JSON Schema: optional literal text, omitted when
absent, with no invented annotation or current-source lookup.

Values use literal text, not Markdown. Text has a bounded excerpt and an explicit full
inspection action. Objects/lists show a few fields/items plus a count. The existing details
panel provides full recorded content. Reuse typed file previews and actions; a filename or
an arbitrary JSON object shaped like a file is not evidence of previewable media.

Measure text before layout, as for notes; reserve media bounds before loading bytes. Later
execution may introduce new evidence, but image/model loading must not cause layout jumps.
Keep a user's camera stable when pending values resolve.

## Recording design

Add optional presentation evidence alongside existing graph fields, on both supported
graph families and corresponding run/expansion projections. The following JSON illustrates
the intended shape; the schemas and reference validation are defined by
[values.md](../../spec/values.md):

```json
{
  "value_views": [
    {
      "id": "value:<structural-reference-sha256>",
      "scope_id": null,
      "kind": "input",
      "name": "prompt",
      "state": "known",
      "data": {"value": "A small ceramic lantern"}
    }
  ],
  "value_links": [
    {
      "source": {"kind": "value", "value_id": "value:<structural-reference-sha256>"},
      "target": {"kind": "value", "value_id": "value:<brief-reference-sha256>"}
    }
  ]
}
```

- `kind` identifies input, table, let or workflow output. IDs are opaque portable references
  emitted by the engine; the client never derives them by parsing display paths.
- Capture effective bound inputs, including defaults and typed files. Preserve the existing
  supplied-input field and its identity meaning.
- A `let` record identifies the actual evaluation context as well as its declaration/owner.
  Contexts with different item/matrix/take environments cannot share one resolved value.
  Multiple consumers in the same semantic context may share a source. Do not eagerly
  evaluate an unused expression, expand work or read another source just for display.
- Links address recorded values or instance ports using typed references. Extend the
  existing evaluation trace to retain named sources while preserving current leaf lineage.
  Links say which recorded source contributed to a value, not that a transformation is
  invertible. Unknown/dynamic member attribution produces no invented precise link.
- A plan-time evaluation is not proof that a node executed. Retain the existing distinction
  between planned lineage and reads by executed consumers. Missing metadata proves neither.
- Bound presentation capture work and payloads independently of visible excerpts: per-value
  size/nesting, occurrence count and total snapshot size. Large tables and many contextual
  evaluations cannot create unbounded display tracing. Omitted presentation evidence is
  explicit, produces no fabricated links and does not fail executable work; it is distinct
  from a merely shortened canvas excerpt. Ratify limits in the normative spec.
- State distinguishes known (including genuine null), pending, absent, unavailable and
  withheld. Capture distinctions before lossy legacy encoding. A known file's bytes may
  separately be unavailable through the existing artifact inventory.
- File data reuses the explicit recorded file encoding and inventory. No absolute paths,
  extra remote fetches, source-path serving or heuristic file detection are introduced.
- New recording evidence is display-only. It does not enter scheduling, retries, node/call
  keys or the executable plan digest. Exact materialized evidence/export hashes still
  reflect the new evidence, as they do for notes.
- Actual input/table/expression edits keep their ordinary execution/identity effects.
  This feature does not extend notes' identity exemption to real workflow data.

Snapshot this evidence into new plans/runs and update it through recorded expansion and
execution events. Resume and inspection consume recorded evidence; they do not reopen current
source files to reconstruct historical values. Older records remain readable with available
effective step summaries; absent defaults/provenance/state are not reconstructed by guessing.

## Layout, embedding and disclosure

- Extend the layout report with separate value placement, input and output bands. Keep
  executable `cells` and member `order` unchanged. Source links do not create execution
  edges or require the deferred authored-layout file.
- Share root/imported-workflow behavior. Group/repeat occurrences retain recorded owner
  and evaluation context; a source outside a repeat is not copied into every child as though
  independently authored. Do not expose a closed imported workflow's private internals in
  its parent view.
- Add `kind: value` selection and `reveal_value` using recorded IDs, through an explicit
  viewer client opt-in following notes. Root uses `scope_id: null`; do not silently widen
  the existing closed `scope_boundary` selection type. Context messages carry references,
  never value bodies, file bytes or private paths.
- Reuse the shared Values/ArtifactPreview path for full inspection and file delegation.
  No parallel host file contract is needed.
- Apply export `--include-values` and file-disclosure choices to every new value position,
  schema default, expression body and nested structure. Structure may remain inspectable
  while content is explicitly withheld. Export normalization must not mistake arbitrary
  JSON for an encoded file. Freeze the same presentation as the local viewer.

## Implementation order and acceptance

1. **Ratify the contract and fixture.** Add `spec/values.md`; update graph v1/v2, expansion,
   viewer-run and layout-report schemas, plus the viewer, export and identity documentation.
   Extend the existing notes fixture with a consumed prompt, reference input, shared table,
   named expression and result. Use synthetic local media and free bodies.
2. **Capture the missing evidence in Rust.** Effective root/scoped inputs; table/let
   evaluation occurrences; root output lineage; selected routes in run projections. Retain
   pending/absent/null distinctions and snapshot/resume behavior. Keep SDK builders thin.
3. **Render one coherent read-only experience.** Step summaries, root/import input/output
   cards, shared sources/wires, details, stable layout and opt-in focus/reveal. Implement
   state and projection in vanilla TypeScript; React stays the mount/presentation adapter.
4. **Hold the boundaries and document it.** Targeted tests/conformance for unchanged
   execution identities, lazy unused expressions, aliases/fallbacks, scoped repeats/takes,
   late expansion, typed vs file-shaped JSON, old recordings, disclosure and static exports.
   Browser evidence covers long prompts, unavailable media, no load flicker, full inspection
   and host file interception. Prove YAML and both SDK authoring paths; update `llms.txt`,
   the guide, skill, harness README and `AGENT_READINESS.md` for delivered behavior.

The first UI checkpoint should make the simple workflow above legible without opening a
sidebar. The second proves scope/context cases. No canvas editing, reruns, arbitrary custom
views, general table editor or authored-field/default diff is part of this delivery.
