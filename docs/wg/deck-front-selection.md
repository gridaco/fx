# Representative cards in collapsed decks

Status: ratified by the owner and implemented in source, 2026-11-11. This amends decision 14 in the
[overview](overview.md). [Canvas layout](../../spec/layout.md) owns the normative
rules. The existing priority order is preserved. Broader deck pairing and changes
to the relative priorities remain follow-up work.

## Decision

A collapsed deck uses its front card to represent several members in limited
space. Choosing that card is a presentation preference scoped to the collapsed
summary. It may favor a recorded chosen result or work requiring attention.
It establishes no universal ranking of workflow states or quality of results.

Canonical member order stays unchanged. Expansion exposes the members in that
order. Explicit inspection can bring any member forward, including an unused or
skipped one. The front card changes no workflow pick, output binding, verdict,
execution decision, identity, cache entry or recorded history.

## Rationale

Collapsing hides simultaneous content. A deck therefore needs a representative
even if the viewer uses a simple first- or last-member rule. Those rules are
defaults too, and can foreground unused planned work while obscuring the result
the workflow actually used.

Member order and front selection serve different purposes. Stable order keeps
alternatives addressable and preserves their relationship in the recorded
expansion. The front card makes the collapsed view useful to inspect. Keeping
these concerns separate lets the viewer improve the summary without rewriting
the sequence or hiding its alternatives permanently.

Execution status alone cannot settle representativeness. A succeeded node body
may produce a candidate that its judge rejects, or an output with no available
browser preview. A skipped node may reflect an unused take, a conditional path,
an unavailable prerequisite, or an operation that started but produced no
candidate. A blanket status ranking would imply more than these facts establish.

Recorded choices provide stronger evidence for some inspection jobs. If a
finished workflow uses take 2 and leaves take 3 unused, take 2 can represent the
result more usefully than the largest take number. During execution, active work
or a failure may deserve attention. The existing layout priorities address those
jobs; this decision does not settle a new relative order between them.

A useful default remains a default. The viewer must keep every member
inspectable, honor explicit inspection, and accurately show the front member's
own state and available preview. It must not borrow another member's image or
call the front member accepted or chosen without supporting evidence.

## Boundaries

- Front selection applies to collapsed decks. It does not sort expanded cards,
  change canonical member order, or move unrelated graph cells.
- Explicit inspection selection takes priority over the automatic default.
  Hover temporarily raises a member; leaving restores the resting selection or
  default. Inspection selection is separate from the workflow's `pick`.
- A preference for completed work over a member known to be unused is a bounded
  fallback. The `skipped` label alone does not prove that the member never ran,
  failed, or has no output.
- Runtime consumption requires recorded execution evidence. A declared edge or
  planned binding establishes possible wiring. A judge inspecting a candidate
  alone does not identify the result chosen for subsequent work.
- A confirmed read supports the representative preference without proving a
  unique chosen output. A consumer such as `fx/select` may inspect several
  candidates. The existing last-match rule resolves that preference without
  labelling its front member accepted or chosen.
- Missing or ambiguous evidence uses the deterministic fallback in the layout
  contract. The viewer must not reconstruct execution decisions by evaluating
  workflow expressions or invent a chosen take for an older recording.
- Media availability affects presentation, not workflow selection. An unsupported
  video, missing file or absent preview must retain its accurate availability
  message and recorded execution state.
- The same boundary applies to node, frame and imported-workflow decks, including
  members whose useful result is not an image.

## Cases the implementation must preserve

| Recorded situation | Required interpretation |
|---|---|
| Take 1 is rejected, take 2 accepted and used, take 3 unused | The recorded use supports take 2 as the result representative under the existing priorities. No image is attributed to take 3. |
| A person selects unused take 3 for inspection | Take 3 may remain in front with its own state and explicit empty preview. The workflow's choice stays unchanged. |
| Every candidate has a judge connection | Those connections alone do not establish which candidate subsequent work chose. |
| Some members are active or failed | Preserve the existing attention priorities while their relative order is reviewed separately. |
| Every member is skipped, or an older record lacks choice evidence | Preserve inspectability and deterministic fallback without claiming a winner or a skip reason. |
| A chosen result has no browser-playable media | Preserve the recorded choice and accurate preview feedback; decoding failure does not change the execution outcome. |
| A deck is expanded | All members remain available in canonical order, regardless of which was previously in front. |

## Implementation status and follow-up

The shared browser viewer separates display wiring from confirmed runtime
reads. Its private projection marks execution from positive recorded evidence
such as a start, duration, cache result or output; status alone does not establish
execution. A consumed connection also requires the consumer's recorded read and
excludes a judge examining its own subject. Inherited planned bindings that no
longer match recorded reads do not establish consumption. All wiring remains
visible. Plan-only records use deterministic fallback without claiming runtime
consumption.

Node draw/review decks link only when explicit judge relationships form a complete
one-to-one correspondence in the same frame, with matching recorded item keys
and full take arrays. The existing priorities run over the corresponding members;
explicit inspection and temporary hover bring their partners forward together.
Partial, ambiguous or unrelated decks remain independent. Pairing of frame and
imported-workflow decks remains deferred; those decks still use recorded reads
for their own representative selection.

The badge reports skipped members as `1 skipped`, avoiding an inference that they
never started. Skipped members without a preview keep the explicit empty state.
No member order, geometry, recorded state, workflow choice or preview attribution
changes when the front card changes.

Evidence includes the shared browser tests for consumption, scope projection and
deck selection. They cover accepted take 2 with unused take 3, an earlier consumed
take, explicit skipped inspection, attention priorities, ambiguous legacy evidence,
multiple judges, incomplete mappings and frame membership. Changes to the relative
priority of failures, active work and consumed results need a separate explicit
decision.

Local browser verification against the saved provider-free takes run confirms
take 2 in front of both decks by default, expansion in take order, explicit
inspection of skipped take 3 and its matching review, reverse selection through
the review deck, and restoration of the default pair after clearing selection.
Hover correspondence and restoration are covered by the pure selection tests.

The implementation introduces no engine operation, SDK API or workflow setting.
Showcase reruns and CDN publication remain separate work.
