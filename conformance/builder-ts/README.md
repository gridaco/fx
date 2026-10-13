# builder-ts

A workflow builder written in TypeScript ([protocol.md](../../spec/protocol.md) §5.2):
`workflows/build.ts` exports `build`, which the TypeScript node host calls with one object of the
`--arg` strings, `{count: "3"}`, and which returns the document `workflow()` makes. The workflow,
`built`, has one step per line, `line_1` to `line_3`, each of the TypeScript node type `make` in
`nodes/make.ts`, whose text names the take that made it.

## What the case pins

- `plan` and `run` take the target `workflows/build.ts:build` with `--arg count=3`, and the run
  succeeds (`line-1.txt` …).
- The same target and argument run into `runs/two` find every step in the result cache
  (`project-two.json`): a builder's workflow is planned like any other.
- `reroll runs/one line_2` writes the takes file next to the builder,
  `workflows/built.takes.yaml`: a TypeScript builder's takes anchor is always its own file, since
  JavaScript has no reliable caller frame. Nothing is written at the project's root. A run of a
  builder records no command that repeats it, so the `next` line advises running it again
  (`reroll.txt`).
- `runs/three` reads the takes file: `line_2` is take 2 (`line-2-three.txt`) and the other two
  steps are cache hits.
- `workflows/build.ts:steps` returns an array, so it is refused with `steps returned an array, not
  a workflow document`; `workflows/build.ts:nope` names no export (`workflows/build.ts has no
  function nope`); and a `.tsx` builder has no node host. Each exits 2.
