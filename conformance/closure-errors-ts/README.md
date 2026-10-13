# closure-errors-ts

What the TypeScript node host refuses about a module's source closure
([protocol.md](../../spec/protocol.md) §5.1, "For TypeScript"). Each workflow uses one module.

- `missing` uses `nodes/broken.ts`, which imports `./missing.ts`: there is no such file, so
  describing the module fails and the step is refused while planning. The message is
  `nodes/broken.ts failed to import: ` and Node's own error, `ERR_MODULE_NOT_FOUND: …`, with the
  project's path cut from it: `'nodes/missing.ts' imported from nodes/broken.ts`.
- `wrongcase` uses `nodes/wrongcase.ts`, which imports `../lib/Words.ts` for `lib/words.ts`. On a
  file system that ignores case, as macOS's usually does, Node loads the file anyway, and the host
  refuses the module: `lib/Words.ts is spelled lib/words.ts on disk`. Where case matters, as on
  Linux, Node finds no file and the message is Node's `ERR_MODULE_NOT_FOUND`. The case pins only
  what both share, `nodes/wrongcase.ts failed to import: `; the js/fx host tests hold the rest.
- `late` uses `nodes/late.ts`, whose body imports `lib/late.ts` by a name it computes when it
  runs. Planning cannot see that import, so the type is named without it; after the body ends,
  the host finds a source file the closure did not hold and fails the node: `the body loaded
  lib/late.ts at run time, outside its source closure: import it statically`, a `node_failure`.

`never_mentions: ["{project}"]` holds every message to project-relative paths.

The modules are deliberately wrong, so this project does not type-check.
