# Example: hello-ts

**The ask:** "Draw a badge for each entry in my list, make sure each has a clear background,
scale them down, and give me one sheet with all of them."

[hello](https://github.com/gridaco/fx/tree/main/examples/hello), with every node written in
TypeScript. Nothing in it is paid: it runs offline, at $0, with no keys. It needs only Node 24 or
later for its nodes
([When FX needs Node](https://github.com/gridaco/fx/blob/main/docs/guide/01-getting-started.md#when-fx-needs-node)).

```
hello-ts/
  fx.yaml
  package.json               # ES modules, the #lib/* imports, @grida/fx
  tsconfig.json              # for tsc: FX runs the .ts files as they are
  workflows/hello.yaml       # the whole flow
  nodes/badges.ts            # four nodes you write: draw, judge, scale down, lay out
  lib/png.ts                 # pictures to PNG bytes and back, with Node's own zlib
  inputs/badges.yaml         # three badges
```

## Run it

From this folder:

```bash
npm install
grida fx plan hello --inputs inputs/badges.yaml
grida fx run  hello --inputs inputs/badges.yaml
```

```
hello  ·  1 phase
phase 1   10 steps   0 provider calls   $0.00
cached    0 of 3 known steps
estimate  $0.00
run       runs/hello/<date>-1
result    ok   spent $0.00
```

- **10 steps:** for each of the three badges, `draw`, `clear` and `small`; then `sheet`. Only the
  three `draw`s are known before the run: everything else reads what they make.
- **The result** is in the run folder's `outputs/`: `sheet.png` and `badges/<id>.png`.

Run it again and every step comes from the cache. Change one badge's colour in
`inputs/badges.yaml` and only that badge is drawn, checked and scaled again, then the sheet.
`npx tsc` checks the nodes' types; FX itself never type checks.

## What this example shows

- **Your own nodes, in TypeScript.** `node(declaration, body)` declares a type, and the body's
  types follow from the declaration: `ctx.params.shape` is `"circle" | "square" | "diamond"`, so
  `tsc` checks that `badge` draws every shape, and `ctx.inputs.badges` is a `ReadonlyMap` in the
  order of the input list
  ([Writing a node type in TypeScript](https://github.com/gridaco/fx/blob/main/docs/guide/03-nodes.md#writing-a-node-type-in-typescript)).
- **The source closure.** FX hashes each node's module and every project file it imports, so
  editing `nodes/badges.ts` or `lib/png.ts` re-runs exactly the steps that use them.
- **A judge you write.** `clear` declares `judge: true` and reports a `verdict` fact: a badge
  without a clear background fails, and `small` waits for the verdict
  ([Annotations and judges](https://github.com/gridaco/fx/blob/main/docs/guide/06-annotations-and-judges.md)).
- **A keyed repeat.** `badge` runs once per list entry, keyed by `id`, and
  `steps.badge.*.small.outputs.image` collects the results by key.
- **The cache.** Every step's result is kept by what went into it, so a second run is free and
  instant, and a change re-runs only what it touches
  ([Cost, cache and takes](https://github.com/gridaco/fx/blob/main/docs/guide/04-cost-and-cache.md)).
