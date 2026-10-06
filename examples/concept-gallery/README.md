# Example: concept gallery

**The ask:** "From my synopsis, a poster and a short direction, propose the people, places and
objects of this world. Have a different model check the proposal against the synopsis. Then draw
one concept image per entity, reviewed by another model. I'll reroll the ones I don't like."

Every call goes through OpenRouter, so one key serves the whole run: `OPENROUTER_API_KEY`.

```
concept-gallery/
  fx.yaml                             # the routes: one writer model, one image model
  workflows/concept-gallery.yaml      # the whole flow
  nodes/world_checks.py               # a deterministic judge: free, exact, run first
  nodes/reviews.py                    # two judges that ask a second model, criterion by criterion
  prompts/propose.md, admit.md, grammar.md, direct-entity.md, review-concept.md
  schemas/world.json, grammar.json, entity-direction.json
  inputs/tidebell.yaml                # the inputs below, as one file
  inputs/tidebell/{synopsis.md, direction.md, poster.png}   # poster.png: ../draw_placeholders.py
```

## Run it

From this folder:

```bash
grida-fx plan concept-gallery --synopsis inputs/tidebell/synopsis.md \
  --direction inputs/tidebell/direction.md --poster inputs/tidebell/poster.png
```

```
concept-gallery  ·  2 phases
phase 1   9 steps   3–5 provider calls   $0.09 – $4.80
phase 2   entity (up to 8)   ≤ $23.60   priced exactly when its list exists
cached    0 of 1 known steps
estimate  $0.09 – $28.40   ceiling $12.00   ⚠ the worst case exceeds the ceiling; the run stops before crossing it
```

- **Phase 1:** `propose` and `grammar` are one call each to the writer model
  (`openai/gpt-5.6-sol@openrouter`, $0.02 – $0.60 a call), and `admit` one to the reviewer
  (`openai/gpt-6-astra@openrouter`, $0.05 – $1.50). Each of `propose`'s two judges may ask for a
  second take of it, which `admit` reviews again: 3 – 5 calls.
- **Phase 2:** up to `max_entities` entities (8 unless you say) × (`direct` $0.60 + `draw` $0.85
  + `review` $1.50) at worst = $23.60, priced exactly once phase 1 has made the list. Until then
  an entity's size is unknown, so `draw` is priced at the route's whole range; then each is
  priced at its size's tier: $0.79 for a character or a place, $0.64 for an object.
- **The ceiling** is the project's $12 (`fx.yaml`), or `--max-usd`: the run stops before crossing
  it. The figures are each route's allowance, a worst case; what a call costs is settled from
  what the provider reports
  ([Cost, cache and takes](../../docs/guide/04-cost-and-cache.md#before-spending-the-plan)).

```bash
grida-fx run concept-gallery --inputs inputs/tidebell.yaml --live --max-usd 12
grida-fx reroll runs/concept-gallery/<date>-1 "entity['bellwright'].draw"
```

Without `--live`, the run resizes the poster and then refuses the first paid call, at $0.

## The judges the user wrote

`world_checks.py` checks what can be checked exactly, for free: unique ids, relationships that
name real entities, the entity count. It runs before the paid reviewer (`needs: [well_formed]`).

`reviews.py` holds the two judges that need a model. Each sends one `structured.generate` call,
on a route the workflow names (`route:`), and asks for one mark per criterion; the node, not the
model, gives the verdict: accepted only when every criterion passed.

```python
@node(
    "review",
    inputs={"image": "image"},
    params={"criteria": list},
    outputs={},
    judge=True,
    calls={"structured.generate": 1},
    resources=["prompts/review-concept.md"],
    version=1,
)
async def review(ctx: Ctx) -> dict:
    criteria = ctx.params["criteria"]
    answer = await ctx.structured_generate(
        prompt=ctx.prompt("prompts/review-concept.md", criteria=numbered(criteria)),
        schema=marks_schema(len(criteria)),
        context=[ctx.inputs["image"]],
    )
    decide(ctx, criteria, answer.json["marks"])
    return {}
```

`independent_of:` on each review step makes the plan refuse if the reviewer and the writer ever
resolve to the same model, whatever the routes say later.

## What the user did not have to write, compared with a hand-built pipeline

- **A "source lock" node.** Inputs are keyed by content automatically.
- **An executor with two phases and a hand-off file.** The repeat over `propose`'s entities is the
  phase boundary.
- **A reroll ledger.** That is `grida-fx reroll`, plus the takes file.
- **Record and close handlers, a manifest reducer, attempt ledgers.** That is `fx/package@1`
  with a manifest laid out in YAML, and the run record.

## Planned

FX's built-in review types, `fx/structured.review@1` and `fx/vision.review@1` (marks and masks,
[Annotations and judges](../../docs/guide/06-annotations-and-judges.md)), have no provider adapter
yet. When they do, `reviews.py` can give way to them.
