# @grida/fx

Grida FX for JavaScript and TypeScript: the `grida-fx` command, and an SDK to write workflow
files and to plan, price and run them. Grida FX is a workflow engine for generative asset
pipelines: write a workflow file, and FX plans it, prices it before anything is spent, caches
every paid call by its request, and records the run. It works with your own provider keys.

This is a **preview** (`0.x` pre-releases under the npm tag `next`).

```sh
npm install @grida/fx     # or: npm install -g @grida/fx
npx grida-fx --help
```

The engine is a native binary. npm installs it with this package, as an optional dependency for
your machine:

| Platform | Engine package |
|---|---|
| macOS on Apple silicon | `@grida/fx-darwin-arm64` |
| macOS on Intel | `@grida/fx-darwin-x64` |
| Linux on x64, glibc 2.28 or later | `@grida/fx-linux-x64-gnu` |
| Linux on arm64, glibc 2.28 or later | `@grida/fx-linux-arm64-gnu` |

**Windows is not in the preview**: the engine's runner uses Unix process groups and signals. Use
WSL 2, which runs the Linux packages. Linux with musl (Alpine) is not supported either. Node 18 or
later.

`grida-fx` reports what is missing when it finds no engine. If you install with
`--omit=optional`, or need a binary of your own, set `GRIDA_FX_BIN` to the path of a `grida-fx`
binary: the command and the SDK use it first.

Some steps run on Python in the preview (your own Python nodes, and the built-in local types such
as `image.resize`): those projects also need `pip install grida`, or `GRIDA_FX_PYTHON`
pointing at a Python that has them. A project whose steps are all paid built-ins and `select`
needs only this package. See [Getting started](https://github.com/gridaco/fx/blob/main/docs/guide/01-getting-started.md).

## The SDK

Every function drives the engine and reads the JSON it prints. Planning never spends; a run
spends only with `live: true`, within a ceiling.

```ts
import { plan, run } from "@grida/fx";

const planned = await plan("workflows/icon.yaml", { inputs: { name: "copper lantern" } });
console.log(planned.ok, planned.estimate); // { lowUsd, highUsd, ceilingUsd }

const result = await run(planned, { live: true }); // the plan's own target, inputs and ceiling
console.log(result.ok, result.chargedUsd, result.failed);
const icon = result.outputs.icon; // a RunFile: digest, kind, path, read()
```

- `plan(target, options)`: the expanded graph (`plan.graph`, fx-graph-v1) and its price
  (`plan.price`), with `ok`, `problems`, `estimate` and `phases`.
- `expand`, `identity`, `price`: the documents of `grida-fx expand`, `identity` and `price`.
- `run(target | plan, options)`: plans first (a plan with problems rejects with `PlanRefused`),
  runs, and reads the run back: `ok`, `incomplete`, `chargedUsd`, `failed`, `outputs`, `steps`.
  A failed step does not reject.
- `project(runDir)` and `inspect(runOrWorkflowId, { verify })`: a run's record and summary.
- `binary()` and `engineVersion()`: the engine in use.

A target is a workflow file, a workflow id, or a builder `file.py:function`. Options:
`inputs` (values), `inputFiles`, `routes`, `args` (a builder's arguments), `maxUsd`, `cwd`,
`env` (for example `GRIDA_FX_PYTHON`) and `signal` (an `AbortSignal`); `run` also takes `live`,
`yesUpTo`, `runDir` and `deliver`. Relative paths start at `cwd`. The engine's errors reject with
`FxError`, which carries its `exitCode` and `stderr`.

## Writing workflows

```ts
import { writeFile } from "node:fs/promises";
import { expr, toYaml, workflow } from "@grida/fx";

const icon = workflow({
  id: "icon",
  title: "One item icon",
  inputs: { name: { type: "string", description: "What the item is" } },
  steps: {
    draw: {
      uses: "fx/image.generate@1",
      with: { prompt: `A single ${expr("inputs.name")} game icon`, background: "transparent" },
    },
  },
  outputs: { icon: expr("steps.draw.outputs.image") },
});
await writeFile("workflows/icon.yaml", toYaml(icon));
```

`toYaml` writes the strict YAML subset the engine reads, as the engine writes it: every string
that would read back as something else (`on`, `017`, `2026-10-05`, a digest) is quoted.

The [guide](https://github.com/gridaco/fx/tree/main/docs/guide) covers workflow files, nodes,
cost and cache, and running.
