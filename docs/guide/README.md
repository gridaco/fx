# The Grida FX guide

Grida FX builds generated assets the way a build system builds code: you describe the steps in a
workflow file, FX plans the run offline with its price, runs only what changed, and keeps every
result with a record of how it was made.

This guide describes FX for the people who use it. The contracts are in [`spec/`](../../spec/),
which is normative: where this guide and `spec/` disagree, `spec/` wins. Where the guide describes
something designed but not built yet, it says **planned**. Stable FX releases start at `0.1.0`;
[the overview](../wg/overview.md) records shipped milestones and future work.

The local FX service gives a project a stable address, a run/plan index, and a
read-only node canvas. Start with `grida-fx init` and `grida-fx start --background`;
standalone workflows remain supported. These service commands describe current
source behavior: check your installed version's help before using them.
Custom step pages and static snapshot exports remain planned.

## Chapters

1. [Getting started](01-getting-started.md)
2. [The workflow file](02-workflow-file.md)
3. [Nodes: built-in, and your own in Python](03-nodes.md)
4. [Cost, cache and takes](04-cost-and-cache.md)
5. [Running: CLI, Python, agents](05-running.md)
6. [Annotations and judges](06-annotations-and-judges.md): marks as artifacts, verdicts as decisions
7. [Viewing workflows and runs](07-viewing.md): the bundled local node canvas
8. [Stopping and continuing a run](08-run-control.md): exact targeting, cancellation and verified local cleanup

## Example projects

The [examples](../../examples/) are complete FX projects, written from scratch the way a user
would write them, with original settings and characters. Their routes are FX's built-in ones
(rigged-character adds one illustrative route, for a capability FX does not serve yet), so their
plans price at the built-in allowances, and a live run uses your own keys. Each plans
offline; without `--live` a run does every free step and refuses each paid call at $0.

| Project | What it shows | What runs |
|---|---|---|
| [hello](../../examples/hello/) | your own nodes, built-in local steps, a judge, a keyed repeat, the cache | everything, offline, at $0 |
| [looping-parallax](../../examples/looping-parallax/) | plan-time facts, fallbacks, a paid call inside your own node | offline when every layer mirrors; a repaint is live |
| [concept-gallery](../../examples/concept-gallery/) | propose a world, review it, one reviewed image per entity: YAML plus judges you write | live |
| [game-build](../../examples/game-build/) | a game repo that builds its art with FX from its own level files: Python builder | the sky offline; the rest live |
| [rigged-character](../../examples/rigged-character/) | agents, Blender, parts × review rounds, a recovery loop, resume | plans and prices; its reviews use `vision.review`, which has no adapter yet, and its Blender steps use tool scripts (`ctx.tool(...).script`) and version constraints, which are planned |

Every example has Python nodes or built-in local types, so each needs a Python with `grida` and
Pillow ([Getting started](01-getting-started.md#when-fx-needs-python)). The
[examples' README](../../examples/README.md) says how to run them from a clone of the repository.
