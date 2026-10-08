# Authoring with the installed FX interface

Use YAML unless the user's existing code or programmatic composition benefits
from a builder. All supported authoring paths produce the same workflow contract.
The examples here make no provider calls.

## Project home

In a fresh project, use `init` when installed help lists it; it preserves existing
configuration. Older versions can create the same minimal `fx.yaml` manually:

```sh
python -m grida.fx init
```

```yaml
fx: project/v1
```

Keep workflows in `workflows/` and custom Python nodes in `nodes/`. This gives
local paths a consistent project home. The greeting example below is a small
end-to-end check after installing FX and its Python runtime.

For browser inspection, current source uses `start --background` once per project,
then `plan --open`, `run --open`, and `inspect --open`. Check installed help before
using those flags. Older releases can use `view TARGET` or `view --run DIR` with
the browser flags their help advertises; do not silently switch to a source build.

## Workflow rules that affect authoring

- Workflow documents begin with `fx: workflow/v1`, a stable `id`, `title`, `steps`, and
  public `outputs`. Schema and persisted fields use `lower_snake_case`.
- File inputs use `{ type: file, kind: image }`; JSON parameters use types such as
  `string`, `integer`, `number`, or `boolean`. `--inputs file.yaml` is repeatable;
  file paths inside it are relative to that input file. Direct input flags use
  kebab-case names and paths relative to the command's working directory.
- `uses: fx/image.resize@1` names a built-in, `uses: ./nodes/greet.py#greet` a
  Python node export, and `uses: ./workflows/child.yaml` an imported workflow.
  Import paths refer to local files, not remote workflow packages.
- Paths starting `./` or `../` in a workflow resolve from its home: the nearest
  enclosing `fx.yaml` directory, or the workflow directory when none exists.
  With a project home, `workflows/a.yaml` can refer to `./nodes/greet.py` at the
  project root. Run from the intended project so cache/run ownership is clear.
- Workflow IDs are discovered at the project root and under `workflows/` by
  default. A project's `workflows:` list replaces the default search folders.
- References establish data dependencies, so branches can run independently and
  joins wait for the values they read. Use `if:` for conditions, `for_each:` with
  stable `key:` expressions for repeats, and `matrix:` for Cartesian combinations.
  Runtime-dependent repeats may remain unresolved in a plan. Inline groups and
  imported workflows are supported; they are not ordinary node bodies.
- Expressions use `${{ ... }}`, not JavaScript or Python syntax evaluation.
  Check supported operators/functions before inventing an expression. Quote
  ambiguous YAML strings such as `"16:9"`, `"on"`, and dates. Anchors, aliases,
  duplicate keys, and tags are refused.
- Route defaults belong in `fx.yaml` under `routes:`, keyed by capability.
  Declare requirements such as `alpha` or `mask` with `requires:`; the planner
  rejects a route that cannot serve them.

Read the [workflow language guide](https://github.com/gridaco/fx/blob/main/docs/guide/02-workflow-file.md)
for exact matrix, repeat, condition, expression, and judge syntax. Read only the
sections needed for the user's workflow; check against the installed engine.

## Custom Python nodes

Python 3.11+ with `grida>=0.1.0` is required. In `nodes/greet.py`:

```python
from grida.fx import Ctx, node


@node("greet", params={"name": str}, outputs={"text": "text/plain"})
def greet(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(f"Hello, {ctx.params['name']}!\n")}
```

Use it in `workflows/greeting.yaml`:

```yaml
fx: workflow/v1
id: greeting
title: A local greeting
inputs:
  name: { type: string, default: world }
steps:
  greet:
    uses: ./nodes/greet.py#greet
    with: { name: "${{ inputs.name }}" }
outputs:
  text: ${{ steps.greet.outputs.text }}
```

From the project root:

```sh
python -m grida.fx plan workflows/greeting.yaml --name Ada --check
python -m grida.fx run workflows/greeting.yaml --run runs/greeting --deliver text=out/greeting.txt -- --name Ada
python -m grida.fx inspect runs/greeting --verify --json
```

This executes local code and records its output without provider credentials.
The `--` delimiter keeps the workflow's `name` input separate from the run label.
On a source version with named-run support, `--name baseline` before the delimiter
creates a named run; use `--resume baseline` with the same inputs to continue it.
With service support, open the retained run:

```sh
python -m grida.fx start --background
python -m grida.fx inspect runs/greeting --open
```

Declare file `inputs`, JSON `params`, exact `outputs`, file `resources`, external
`tools`, and any maximum paid `calls`. Read through `ctx.read`/`ctx.inputs` and
write artifacts through `ctx.out`; return every required output. Helpers can use
`ctx.work_path` for scratch work. Keep module import and builder execution free
of unrelated side effects.

For paid work, use FX capabilities such as `ctx.image_generate` or
`ctx.capability`, with declared `calls`. Do not implement your own provider retry,
cache, billing, or provenance layer. Direct HTTP calls escape FX's pricing and
spending controls. Read the [node guide](https://github.com/gridaco/fx/blob/main/docs/guide/03-nodes.md)
before introducing a capability or judge.

**Cache limitation of older engines, including the 0.1.0 release:** distinct
unversioned exports in one source module can share a type identity, so one step
can be answered with another's cached result. Check what `grida-fx expand <target>`
prints under `types`: a fixed engine names an unversioned type
`<path>#<attr>@source:<digest>`, an affected one prints a bare `source:<digest>`.
On an affected engine, put distinct unversioned node bodies in separate files, or
declare node versions and maintain `fx.lock` (bump a version when behavior
changes; use `lock --same` only when it does not). Changing step names or run
folders does not avoid it.

## Python builders and execution

A builder returns `Workflow`; it is invoked through `file.py:function`. For the
same project and `nodes/greet.py`, create `assets.py`:

```python
from grida.fx import Workflow


def build(name: str = "world") -> Workflow:
    workflow = Workflow("greeting-built", title="Greeting from a builder")
    greeting = workflow.step(
        "greet", uses="./nodes/greet.py#greet", with_={"name": name}
    )
    workflow.outputs(text=greeting.outputs.text)
    return workflow
```

```sh
python -m grida.fx plan assets.py:build --arg name=Ada --check
python -m grida.fx run assets.py:build --arg name=Ada --run runs/greeting-built
```

With the project service running, inspect the builder's materialized plan:

```sh
python -m grida.fx plan assets.py:build --arg name=Ada --open
```

For orchestration from Python:

```python
from grida.fx import plan, run

planned = plan("assets.py:build", arguments={"name": "Ada"})
if not planned.ok:
    raise RuntimeError(planned.problems)
result = run(planned)
if not result.ok:
    raise RuntimeError(result.failures)
result.deliver({"text": "out/greeting.txt"})
```

`run` accepts a target string or a `Plan`, not a `Workflow` object. A builder uses
`--arg`/`arguments`; workflow inputs use input flags/`inputs`. Preserve this
distinction. Async callers use `plan_async` and `run_async`.

## JavaScript and TypeScript

Install `@grida/fx` in the project (`npm install @grida/fx`). Use `workflow`, `expr`, and `toYaml` to
author a workflow, then write its YAML and give that path to the engine. This
example reuses `nodes/greet.py` above, so it also needs the Python runtime:

```javascript
import { writeFile } from "node:fs/promises";
import { expr, plan, run, toYaml, workflow } from "@grida/fx";

const document = workflow({
  id: "greeting-js",
  title: "A greeting authored in JavaScript",
  inputs: { name: { type: "string", default: "world" } },
  steps: {
    greet: {
      uses: "./nodes/greet.py#greet",
      with: { name: expr("inputs.name") },
    },
  },
  outputs: { text: expr("steps.greet.outputs.text") },
});
await writeFile("workflows/greeting-js.yaml", toYaml(document));
const planned = await plan("workflows/greeting-js.yaml", {
  inputs: { name: "Ada" },
});
if (!planned.ok) throw new Error(JSON.stringify(planned.problems));
const result = await run(planned, { runDir: "runs/greeting-js" });
if (!result.ok) throw new Error(JSON.stringify(result.failed));
```

SDK runtime options use JavaScript names (`maxUsd`, `runDir`, `inputFiles`, builder
`args`); workflow documents retain contract names. Custom TypeScript node bodies
and direct JavaScript builder hosting are not implemented. Do not infer a graph
by executing an arbitrary script as though it were a supported builder.

The [Python SDK reference](https://github.com/gridaco/fx/blob/main/python/README.md)
and [JavaScript SDK reference](https://github.com/gridaco/fx/blob/main/js/fx/README.md)
document result/file APIs and option names. Prefer installed help and matching
release documentation when the main-branch guide describes a newer feature.
