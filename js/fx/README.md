# @grida/fx

Grida FX for JavaScript and TypeScript: an SDK to write workflow files, node types and builders,
and to plan, price and run them; the host that runs your TypeScript nodes; and the standalone
`grida-fx` command. Grida FX is a workflow engine for generative asset pipelines: write a workflow
file, and FX plans it, prices it before anything is spent, caches every paid call by its request,
and records the run. It works with one Grida Gateway key (`GG_API_KEY`), or your own provider
keys.

To run FX from a terminal, install the Grida CLI: `npm install -g grida`, then `grida fx --help`.
It pins this package. Install `@grida/fx` itself for the SDK, or for `grida-fx` on its own (in a
service you host, say):

```sh
npm install @grida/fx     # or: npm install -g @grida/fx
npx grida-fx --help
```

Stable releases use npm's default `latest` tag. Prereleases use `next`.

The engine is a native binary. npm installs it with this package, as an optional dependency for
your machine:

| Platform | Engine package |
|---|---|
| macOS on Apple silicon | `@grida/fx-darwin-arm64` |
| macOS on Intel | `@grida/fx-darwin-x64` |
| Linux on x64, glibc 2.28 or later | `@grida/fx-linux-x64-gnu` |
| Linux on arm64, glibc 2.28 or later | `@grida/fx-linux-arm64-gnu` |

**Native Windows is not supported yet**: the engine's runner uses Unix process groups and signals. Use
WSL 2, which runs the Linux packages. Linux with musl (Alpine) is not supported either. The SDK and
`grida-fx` need Node 18 or later; hosting TypeScript nodes, builders and stand-in files needs
Node 24 or later ([Writing node types](#writing-node-types)).

For the `grida` command, this package exports its launcher as `@grida/fx/cli` (`launch`,
`HOST_CONTRACT`); see [spec/distribution.md](https://github.com/gridaco/fx/blob/main/spec/distribution.md).

`grida-fx` reports what is missing when it finds no engine. If you install with
`--omit=optional`, or need a binary of your own, set `GRIDA_FX_BIN` to the path of a `grida-fx`
binary: the command and the SDK use it first.

Some steps run on Python: Python nodes, and the built-in local types such as `image.resize`, in
a TypeScript project too. Those projects also need `pip install grida`, or `GRIDA_FX_PYTHON`
pointing at a Python that has it. A project whose steps are all paid built-ins, `select` and
TypeScript nodes needs only this package (and Node 24 or later for those nodes). See
[Getting started](https://github.com/gridaco/fx/blob/main/docs/guide/01-getting-started.md#when-fx-needs-python).

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
- `loadRun(target, { verify })`: a `RunRecord` with a pinned absolute `runDir` and
  engine `inspection`. Its `snapshot()`, `events({ after, limit })` and async
  `follow({ after, pollIntervalMs, signal })` methods use the same public reader as
  the CLI. Following yields nonempty bounded batches until broken or aborted,
  including across resumes. It owns no execution process or exit code.
- `inspectControl(run)` and `cancel(run, { invocation, wait, timeout })`: exact-target local
  control, independent of viewer/service lifetime. Results keep their `lower_snake_case` wire
  fields; structured failures raise `RunControlError` with `code` and `result`.
- `listRuns({ workflow })`: the project's runs, newest first, each with its `folder`, a `runDir`
  resolved against `cwd`, and `lower_snake_case` wire fields (`state`, `placement`,
  `charged_usd`, …). `removeRuns(runs, { preview })`: removes exactly the runs given (filter
  `listRuns` yourself); `preview: true` removes nothing. A refused or partial run is an entry's
  `outcome`, not a rejection; a RUN that is not a run raises `RunRemovalError` with `code` and
  `run`, before anything is removed.
- `binary()` and `engineVersion()`: the engine in use.

A target is a workflow file, a workflow id, or a builder: `file.ts:function`
([Builders](#builders)) or `file.py:function`. Options: `inputs` (values), `inputFiles`,
`routes`, `args` (a builder's arguments), `maxUsd`, `cwd`, `env` (for example `GRIDA_FX_PYTHON`
or `GRIDA_FX_NODE`) and `signal` (an `AbortSignal`); `run` also takes `live`, `yesUpTo`,
`deliver`, `standIn` (a function that answers the run's paid calls, [Stand-ins](#stand-ins)), and
one of `name` (create-only), `resume` (require-existing), or `runDir` (explicit-folder
create/resume). Relative paths start at `cwd`. The engine's errors reject with `FxError`, which
carries its `exitCode` and `stderr`.

The SDK and `grida-fx` start the engine with your environment plus `GRIDA_FX_SDK_NODE`, the Node
running them (not set under Bun or Deno), and `GRIDA_FX_SDK_NODE_HOST`, this package's own node
host; an `env` you pass wins. The engine hosts TypeScript nodes on `GRIDA_FX_NODE` when it is set,
else on that Node, else on `node` on `PATH`, and with the `@grida/fx` of the workflow's project
when it has one, else with that of the project you run from, else with this one. When this one
hosts them, run records and messages name its folder `@grida/fx`, never its absolute path.

Aborting a `run` requests graceful cancellation once with SIGTERM and waits for engine cleanup
before rejecting. There is no automatic runner force timer; stuck cleanup can keep the promise
pending. Aborting only a control waiter ends that wait without retracting accepted cancellation.
See [Stopping and continuing a run](../../docs/guide/08-run-control.md) for guarded requests,
wait timeouts, local completion and unknown remote outcomes. These APIs describe current source;
check your installed version for availability.

Observation abort stops only its reader. Apply an entire batch before checkpointing
its opaque cursor; `RunObservationError` keeps a reader's `code` and `document`.
Inspection and a later snapshot are independent samples. See
[Working with runs from an SDK](../../docs/guide/09-sdk-runs.md) for saved/named
records and observing another process. SDK execution still suppresses viewer
hosting and returns after completion.

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

## Writing node types

A node type is an export declared with `node`. Node 24 or later runs the project's `.ts` files as
they are, stripping their types: there is no build step, and the engine runs each node in this
package's node host. [hello-ts](https://github.com/gridaco/fx/tree/main/examples/hello-ts) is a
whole project of them.

```ts
// nodes/greet.ts
import { node } from "@grida/fx";

export const greet = node(
  { name: "greet", params: { name: "string" }, outputs: { text: "text/plain" } },
  (ctx) => ({ text: ctx.out.text(`Hello, ${ctx.params.name}!\n`) }),
);
```

A workflow uses it as `uses: ./nodes/greet.ts#greet`. The
[node guide](https://github.com/gridaco/fx/blob/main/docs/guide/03-nodes.md#writing-a-node-type-in-typescript)
covers declarations, `ctx`, paid calls, judges, agents and versions.

- **Declarations type the body**, through `const` type parameters: an input of one file is an
  `InputFile`, a list (`image[]`) a readonly array, a keyed collection (`image{}`) a
  `ReadonlyMap` in the collection's order; params take their declared types (`"integer"` is a
  `number`, `["x", "y"]` is `"x" | "y"`, a JSON Schema its type, and `"array"`, `"object"` or a
  schema that says nothing narrower JSON values you can pass on to a request, a fact or
  `ctx.out.json`); optional ones are optional properties, with a default or without. The body
  returns exactly the declared outputs: an `Output` from `ctx.out`, an `InputFile`, or arrays
  and `Map`s of them. `tsc` refuses anything else, and a declaration field `node()` doesn't
  know. A member set to `undefined` is left out (`{ type: "integer", default: undefined }` has no
  default).
- **Params are checked when the node runs**, since a step may bind one to an expression: before
  the body starts, each value is checked against its declaration, and the first that doesn't fit
  fails the node (`param side: "up" is not one of ["left","right"]`). A `null` for an optional
  param (bound to an input nobody gave) leaves it out, as if the step hadn't set it.
- **`ctx`** has `inputs`, `params`, `instance`, `read` (`bytes`, `text`, `json`; no image
  decoding), `out` (`png` of encoded bytes, `bytes`, `text`, `json`, `path`, `file`), `workPath`,
  `state`, `fact`, `annotate`, `progress`, `prompt`, `capability` (and `imageGenerate`,
  `imageEdit`, `structuredGenerate`), `agent`, `tool`, `signal` (an `AbortSignal`) and `fail`.
  `ctx.read` takes an `InputFile`, or the name of an input of one file that isn't optional; one
  absent anyway fails the node with `input <name> is absent`.
- **Programs** run with `ctx.tool(name).run(argv, { cwd, timeoutS, env, check })` in a session of
  their own. At `timeoutS` seconds (`null` or left out: no limit; anything but a number above 0
  rejects with a `TypeError`), when `ctx.signal` aborts, and when the run ends, the program is
  killed with everything it started.
- **Agent tools** are `tool({ name, description, parameters }, handler)`, with `parameters` a
  JSON Schema written in place (or `as const`) that types the handler's arguments; a handler
  returns text, a JSON value or a `ToolReply`.
- **Errors:** `throw ctx.fail("…")` fails the node on purpose. What the engine refuses reaches the
  body as an `EngineError` (`CallRefused`, `CallFailed`, `OverBound`, `NotLive`, …), which it may
  catch.
- **Values sent to the engine are checked**: `NaN`, infinities, integers beyond 2^53,
  `undefined` in an array or as a whole value, and objects that aren't plain are refused with a
  sentence that says where, rather than coerced as `JSON.stringify` would. An object member that
  is `undefined` is left out, as `JSON.stringify` and `toYaml` leave it out. The types say the
  same: what the SDK takes is a `JsonInput` (readonly arrays and objects, members that may be
  `undefined`), and a capability request or a prompt's variables a `RequestObject`, whose
  `RequestValue`s may hold `InputFile`s and `Output`s; what it hands out is a `JsonValue`.

The project is ES modules, in Node's type-stripping dialect: `"type": "module"` in `package.json`;
erasable syntax only (no `enum`, `namespace` or parameter properties); imports name the `.ts` file;
`#` subpath imports (package.json `imports`) instead of tsconfig `paths`. Node modules are `.ts`,
`.mts`, `.js` or `.mjs`. For type checking (`npm install --save-dev typescript @types/node`):

```json
{
  "compilerOptions": {
    "module": "nodenext",
    "strict": true,
    "noEmit": true,
    "allowImportingTsExtensions": true,
    "erasableSyntaxOnly": true,
    "verbatimModuleSyntax": true
  }
}
```

Node strips no types under `node_modules`, so a package of node types you publish ships
JavaScript and `.d.ts`, as this one does. Bun runs the SDK, but cannot host nodes: it has no
`module.registerHooks`, which the host uses to learn which files each node imports.

## Builders

A builder is an exported function, sync or async, that returns `workflow(…)`'s document. The
engine calls it with one object of the `--arg` strings:

```ts
// workflows/badges.ts
import { expr, workflow } from "@grida/fx";

export function build(args: { count?: string }) {
  const count = Number(args.count ?? "2");
  const steps = Object.fromEntries(
    Array.from({ length: count }, (_, i) => [`badge_${i}`, { uses: "./nodes/badge.ts#badge" }]),
  );
  return workflow({
    id: "badges",
    title: `${count} badges`,
    steps,
    outputs: { first: expr("steps.badge_0.outputs.image") },
  });
}
```

```sh
npx grida-fx plan workflows/badges.ts:build --arg count=3
```

From the SDK, `plan("workflows/badges.ts:build", { args: { count: 3 } })`. Its takes file,
`badges.takes.yaml`, goes beside the builder's file.
[item-icons](https://github.com/gridaco/fx/tree/main/examples/item-icons) builds its workflow
from a web game's own item table, and answers its paid calls offline with a stand-in.

## Stand-ins

`run(target, { standIn })` answers the run's paid calls with your function, in your own process:
offline, for nothing, with the answers your test chooses. Everything else is a real run.

```ts
import { Answer, DECLINE, run, type StandInCall } from "@grida/fx";

function answer(call: StandInCall) {
  if (call.capability !== "structured.generate") return DECLINE;
  return Answer.json({ caption: "a copper lantern" });
}

const result = await run("workflows/caption.yaml", { standIn: answer });
console.log(result.ok, result.standIn, result.chargedUsd); // true true 0
```

- **The call** has `capability`, `route`, `key`, `take`, `takes`, `instance`, `request` (its
  files as `InputFile`s), `files` (a `ReadonlyMap` by digest), `params` and `signal`, aborted when
  the engine abandons the call. Calls come one at a time, in the order the engine asks.
- **An answer** is `{ files: { name: file }, data }`, each file a `Uint8Array`, `{ bytes, kind }`,
  an `InputFile` or a file `URL`; `Answer.json`, `Answer.turn` and `Answer.submit` build the
  common ones. `DECLINE` leaves the call to fail as not live; `isDecline(value)` tells it from an
  answer, for a stand-in that asks another first (`value === DECLINE` does not narrow the type).
- **Throw** `new CallRefused("…")` or `new CallFailed("…")` to refuse or fail the call. Anything
  else thrown, or `undefined` returned, stops the run, and `run` rejects with what was thrown.
- **A stand-in run is never live** and runs every phase: `live: true` and `yesUpTo` are refused.
  `result.standIn` says whether a run had a stand-in.
- **macOS and Linux:** the SDK gives the engine a Unix-domain socket on its standard input
  (`--stand-in -`). This needs no Node 24, and works under Bun too.

The command takes a stand-in from a file instead:
`grida-fx run … --stand-in tests/stand-in.ts#answer`, the function an export of that file, hosted
on Node 24 or later. The
[stand-in guide](https://github.com/gridaco/fx/blob/main/docs/guide/05-running.md#stand-ins-testing-without-a-provider)
has the whole contract.

The [guide](https://github.com/gridaco/fx/tree/main/docs/guide) covers workflow files, nodes,
cost and cache, and running.
