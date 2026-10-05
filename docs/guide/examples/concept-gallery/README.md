# Example: concept gallery

**The ask:** "From my synopsis, a poster and a short direction, propose the people, places and
objects of this world. Have a different model check the proposal against the synopsis. Then draw
one concept image per entity, reviewed by another model. I'll reroll the ones I don't like."

**Status:** the project plans and prices today. Its admission and image reviews use
`structured.review` and `vision.review`, which are planned, so those steps run once their bodies
land. Until then, the same flow runs with `structured.generate` answers that judges you write read
([Nodes](../../03-nodes.md#judges-you-write)).

```
concept-gallery/
  fx.yaml
  workflows/concept-gallery.yaml      # the whole workflow: no Python needed for the flow
  nodes/world_checks.py               # one deterministic judge (~30 lines)
  prompts/propose.md, admit.md, grammar.md, direct-entity.md, review-concept.md
  schemas/world.json, grammar.json, entity-direction.json
  inputs/tidebell/{synopsis.md, direction.md, poster.png}   # poster.png: ../draw_placeholders.py
```

## Run it

From this folder (`fx.yaml` lists [`../routes.yaml`](../routes.yaml) under `route_tables`, so the
prices below are the same everywhere):

```bash
grida-fx plan concept-gallery --synopsis inputs/tidebell/synopsis.md \
  --direction inputs/tidebell/direction.md --poster inputs/tidebell/poster.png
```

```
concept-gallery  ·  2 phases
phase 1   9 steps   3–5 provider calls   $0.01 – $0.10
phase 2   entity (up to 48)   ≤ $16.80   priced exactly when its list exists
cached    0 of 1 known steps
estimate  $0.01 – $16.90   ceiling $12.00   ⚠ the worst case exceeds the ceiling; the run stops before crossing it
```

- **Phase 1:** `propose`, `admit` and `grammar` are one $0.002 – $0.02 call each, and
  regeneration may draw a second take of `propose` and `admit`: 3 – 5 calls, $0.006 – $0.10.
- **Phase 2:** up to 48 entities × (`direct` $0.02 + `draw` $0.30 + `review` $0.03) at worst
  = $16.80, priced exactly once phase 1 has made the list.
  [Cost, cache and takes](../../04-cost-and-cache.md#before-spending-the-plan) walks through it.

```bash
grida-fx run concept-gallery --inputs inputs/tidebell.yaml --live --max-usd 12
grida-fx reroll runs/concept-gallery/2026-10-02-1 "entity['bellwright'].draw"
```

## The node the user wrote

```python
# nodes/world_checks.py
"""Deterministic checks of a proposed world: free, exact, and run before the paid reviewer."""

from grida.fx import Ctx, node


@node(
    "well_formed",
    inputs={"world": "json"},
    params={"max_entities": int},
    outputs={},
    judge=True,
    version=1,
)
def well_formed(ctx: Ctx) -> dict:
    world = ctx.read.json("world")
    ids = [entity["id"] for entity in world["entities"]]
    problems = []
    if len(ids) != len(set(ids)):
        problems.append("two entities share an id")
    if not 1 <= len(ids) <= ctx.params["max_entities"]:
        problems.append(f"{len(ids)} entities, outside 1..{ctx.params['max_entities']}")
    known = set(ids)
    for relation in world["relationships"]:
        for end in (relation.get("from"), relation.get("to")):
            if end not in known:
                problems.append(f"a relationship names {end}, which is not an entity")
    ctx.fact("problems", problems)
    ctx.fact("verdict", "reject" if problems else "accept")
    return {}
```

## What the user did not have to write, compared with a hand-built pipeline

- **A "source lock" node.** Inputs are keyed by content automatically.
- **A proxy node before each review.** `vision.review` reviews a reduced copy by itself.
- **An executor with two phases and a hand-off file.** The repeat over `propose`'s entities is the
  phase boundary.
- **A reroll ledger.** That is `grida-fx reroll`, plus the takes file.
- **Record and close handlers, a manifest reducer, attempt ledgers.** That is `fx/package@1`
  with a manifest laid out in YAML, and the run record.
