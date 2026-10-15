# Authoring with the installed FX interface

Use YAML unless the user's existing code or programmatic composition benefits
from a builder. All supported authoring paths produce the same workflow contract.
The examples here make no provider calls.

## Project home

In a new, empty folder, use `init` when installed help lists it; recent releases
write a starter project there and refuse a non-empty folder without `fx.yaml`.
In an existing folder, or with an older version, create the minimal `fx.yaml`
manually:

```sh
grida fx init
```

```yaml
fx: project/v1
```

Keep workflows in `workflows/` and custom nodes in `nodes/`, written in Python or
TypeScript; follow the language the project already uses, and keep to one unless
it already mixes them. This gives local paths a consistent project home. The
greeting examples below are small end-to-end checks after installing FX and the
runtime of the nodes' language.

For browser inspection, FX 0.2 and later use `start --background` once per project,
then `plan --open`, `run --open`, and `inspect --open`. Older releases can use `view TARGET` or `view --run DIR` with
the browser flags their help advertises; do not silently switch to a source build.

## Workflow rules that affect authoring

- Workflow documents begin with `fx: workflow/v1`, a stable `id`, `title`, `steps`, and
  public `outputs`. Schema and persisted fields use `lower_snake_case`.
- File inputs use `{ type: file, kind: image }`; JSON parameters use types such as
  `string`, `integer`, `number`, or `boolean`. `--inputs file.yaml` is repeatable;
  file paths inside it are relative to that input file. Direct input flags use
  kebab-case names and paths relative to the command's working directory.
- `uses: fx/image.resize@1` names a built-in, `uses: ./nodes/greet.py#greet` a
  Python node export, `uses: ./nodes/greet.ts#greet` a TypeScript one, and
  `uses: ./workflows/child.yaml` an imported workflow.
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

Python 3.11+ with `grida>=0.2.0` (which brings Pillow) is required. In `nodes/greet.py`:

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
spending controls. To reach a provider FX does not ship, declare it in `fx.yaml`
and give it routes and, unless it speaks the OpenAI Chat Completions API, an
adapter (`adapter(...)` from `grida.fx` or `@grida/fx`): nodes then reach it
through `ctx.capability` like any provider
([Community providers](https://github.com/gridaco/fx/blob/main/docs/guide/15-community-providers.md)).
Read the [node guide](https://github.com/gridaco/fx/blob/main/docs/guide/03-nodes.md)
before introducing a capability or judge.

**Cache limitation of older engines, including the 0.1.0 release:** distinct
unversioned exports in one source module can share a type identity, so one step
can be answered with another's cached result. Check what `grida fx expand <target>`
prints under `types`: a fixed engine names an unversioned type
`<path>#<attr>@source:<digest>`, an affected one prints a bare `source:<digest>`.
On an affected engine, put distinct unversioned node bodies in separate files, or
declare node versions and maintain `fx.lock` (bump a version when behavior
changes; use `lock --same` only when it does not). Changing step names or run
folders does not avoid it.

## Custom TypeScript nodes

Node 24 or later and `@grida/fx` in the project are required, as the main skill
says; add the type checker:

```sh
npm install @grida/fx
npm install --save-dev typescript @types/node
```

The project's `package.json` sets `"type": "module"`. For `npx tsc`, a
`tsconfig.json` sets `module: "nodenext"` and turns on `strict`, `noEmit`,
`allowImportingTsExtensions`, `erasableSyntaxOnly` and `verbatimModuleSyntax`.

In `nodes/greet.ts`:

```ts
import { node } from "@grida/fx";

export const greet = node(
  { name: "greet", params: { name: "string" }, outputs: { text: "text/plain" } },
  (ctx) => ({ text: ctx.out.text(`Hello, ${ctx.params.name}!\n`) }),
);
```

Use it from the greeting workflow above with `uses: ./nodes/greet.ts#greet` in
place of the Python export. The same commands plan and run it, with `grida fx` in
place of `python -m grida.fx`:
`grida fx plan workflows/greeting.yaml --name Ada --check`. Check types with
`npx tsc`; FX itself never type checks.

Node runs these files with its type stripping, so keep to its dialect:
- erasable syntax only: no `enum`, `namespace` or parameter properties;
- imports name the `.ts` file (`./helper.ts`), and `import type` brings types;
- `#` subpath imports (package.json `imports`) replace tsconfig `paths`;
- node modules are `.ts`, `.mts`, `.js` or `.mjs`; `.tsx`, `.cts` and `.cjs` are refused.

Declare nodes and use `ctx` as for Python, with camelCase names
(`ctx.workPath`, `ctx.imageGenerate`). Declarations type the body: one input
file is an `InputFile`, `kind[]` a readonly array, `kind{}` a `ReadonlyMap` (keep
its order), and the body returns exactly the declared outputs. `ctx.read` has no
image decoding; `ctx.out.png` takes encoded PNG bytes. Import what a body uses
with literal paths spelled as on disk (`../lib/x.ts`): they count toward the
node's identity, and a project file loaded by a computed path fails the node.
List files that a package, a worker thread or a child process loads under
`resources`. The [node guide](https://github.com/gridaco/fx/blob/main/docs/guide/03-nodes.md#writing-a-node-type-in-typescript)
has the rest.

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

## TypeScript builders

A TypeScript builder is an exported function, sync or async, that returns
`workflow(…)`'s document; it is invoked through `file.ts:function` (`.mts`, `.js`
and `.mjs` files too). Its `--arg` values arrive as one object of strings. For the
same project and `nodes/greet.ts`, create `assets.ts`:

```ts
import { expr, workflow } from "@grida/fx";

export function build(args: { name?: string }) {
  return workflow({
    id: "greeting-built",
    title: "Greeting from a builder",
    steps: { greet: { uses: "./nodes/greet.ts#greet", with: { name: args.name ?? "world" } } },
    outputs: { text: expr("steps.greet.outputs.text") },
  });
}
```

```sh
grida fx plan assets.ts:build --arg name=Ada --check
grida fx run assets.ts:build --arg name=Ada --run runs/greeting-built
```

## JavaScript and TypeScript SDK

Install `@grida/fx` in the project (`npm install @grida/fx`; the SDK itself runs
on Node 18 or later). Use `workflow`, `expr`, and `toYaml` to author a workflow,
then write its YAML and give that path to the engine, or name a builder as the
target. This example reuses `nodes/greet.ts` above, so it needs Node 24 or later:

```javascript
import { writeFile } from "node:fs/promises";
import { expr, plan, run, toYaml, workflow } from "@grida/fx";

const document = workflow({
  id: "greeting-js",
  title: "A greeting authored in JavaScript",
  inputs: { name: { type: "string", default: "world" } },
  steps: {
    greet: {
      uses: "./nodes/greet.ts#greet",
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
`args`, `standIn`); workflow documents retain contract names. A builder target is
`plan("assets.ts:build", { args: { name: "Ada" } })`, and `run(target, { standIn })`
answers paid calls with a function in the caller's process, offline. Do not infer a
graph by executing an arbitrary script as though it were a supported builder.

The [Python SDK reference](https://github.com/gridaco/fx/blob/main/python/README.md)
and [JavaScript SDK reference](https://github.com/gridaco/fx/blob/main/js/fx/README.md)
document result/file APIs and option names. Prefer installed help and matching
release documentation when the main-branch guide describes a newer feature.
