# run-local-ts

[`run-local`](../run-local/)'s project with its node types written in TypeScript,
`in/nodes/local.ts`, which the engine runs in the TypeScript node host of `@grida/fx`
([protocol.md](../../spec/protocol.md) §1, §8.2). `in/package.json` depends on `@grida/fx`; the
runner installs the built package into the project's `node_modules`, and the bodies run on Node
24 or later.

## Against run-local

The six steps both cases share make the same files, so these expected files are run-local's,
byte for byte: `said.txt`, `report.json` (the engine writes `ctx.out.json`'s value in its own
JSON layout, whichever host sent it), `parts-0.txt`, `parts-1.txt`, `entries-first.txt`,
`entries-last.txt` and `marks.json` (the engine writes the marks that `ctx.annotate` reported).

What differs, and why:

- `project.json`: the plan digest, since the node types are other modules; and the two steps
  this case adds, `order` and `listing`, with their outputs.
- `inspect.txt`: 8 steps and 11 placed files, against 6 and 7.

## Keyed order

`order` returns its keyed output as a `Map` made in the order `sun`, `10`, `2`. A plain object
would put the integer-like keys first (`2`, `10`, `sun`), so the SDK refuses one on a keyed port
(`refusals-ts`). The order holds in `run_finished`'s collection (`project.json`), in the files
the run places (`order-sun.txt`, `order-10.txt`, `order-2.txt`), and in `listing`, which gets the
collection as a keyed input, a `ReadonlyMap`, and writes each key with its file's text in the
order it sees them (`keys.txt`).

## What the case pins

- A local run of every output shape from TypeScript bodies, with node facts (`letters`,
  `words`) and a judge's verdict (`check`).
- `inspect --verify` rehashes the 11 files the steps placed.
- The same plan run into `runs/two` is answered by the result cache: every instance in
  `project-two.json` is a `hit`, and its outputs are the same bytes.
