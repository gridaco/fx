# The Grida FX guide

Grida FX builds generated assets the way a build system builds code: you describe the steps in a
workflow file, FX plans the run offline with its price, runs only what changed, and keeps every
result with a record of how it was made.

This guide describes FX for the people who use it. The contracts are in [`spec/`](../../spec/),
which is normative: where this guide and `spec/` disagree, `spec/` wins. Where the guide describes
something designed but not built yet, it says **planned**. FX is in preview;
[the overview](../wg/overview.md) has the plan.

Views (custom HTML pages for a step or a whole run, a dashboard over your runs, and static
exports of them) come later: they are on the overview's Later list, and this guide leaves them out
until then.

## Chapters

1. [Getting started](01-getting-started.md)
2. [The workflow file](02-workflow-file.md)
3. [Nodes: built-in, and your own in Python](03-nodes.md)
4. [Cost, cache and takes](04-cost-and-cache.md)
5. [Running: CLI, Python, agents](05-running.md)
6. [Annotations and judges](06-annotations-and-judges.md): marks as artifacts, verdicts as decisions

## Example projects

Each project is a complete FX project, written from scratch the way a user would write it, with
original settings and characters. They plan and price offline against illustrative routes,
[`examples/routes.yaml`](examples/routes.yaml). Every project's `fx.yaml` lists that file under
`route_tables`, which FX reads after its built-in route table, so the examples' entries win and
each project prices the same on every installation
([identity.md §7](../../spec/identity.md#7-route-fingerprint)). The commands in each project's
README work as written from the folder it names.

| Project | What it shows | Status |
|---|---|---|
| [concept-gallery](examples/concept-gallery/) | propose a world, review it, one reviewed image per entity: YAML plus one Python judge | plans and prices; its reviews use `vision.review` and `structured.review`, which are planned |
| [looping-parallax](examples/looping-parallax/) | plan-time facts, fallbacks, local Python nodes | plans and prices; its node bodies elide the seam and preview math, so it is read, not run |
| [rigged-character](examples/rigged-character/) | agents, Blender, parts × review rounds, a recovery loop, resume | plans and prices; its reviews use `vision.review`, and its Blender steps use tool scripts (`ctx.tool(...).script`) and version constraints, which are planned |
| [game-build](examples/game-build/) | a game repo that builds its art with FX from its own level files: Python builder | plans and prices; its node bodies elide the same math as looping-parallax |

Every example has Python nodes or built-in local types, so each needs a Python with `grida` and
Pillow ([Getting started](01-getting-started.md#when-fx-needs-python)).

The example pictures (the concept gallery's poster and the two parallax layers) are flat
placeholder shapes, drawn by [`examples/draw_placeholders.py`](examples/draw_placeholders.py).
It draws the same pixels on every run and writes every copy the examples read. From the
repository root:

```bash
cd python && uv run --with pillow python ../docs/guide/examples/draw_placeholders.py
```
