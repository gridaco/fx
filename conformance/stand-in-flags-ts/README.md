# stand-in-flags-ts

[`stand-in-flags`](../stand-in-flags/) for what only a TypeScript stand-in file can get wrong. The
TypeScript node host loads the file ([protocol.md](../../spec/protocol.md) §5.7), and each of
these is a usage error (exit 2), refused before anything is planned, run or asked:

- `stand_in.ts#nope`: the file has no export `nope`;
- `broken.ts#answer`: the module throws while it loads, `broken.ts failed to import: Error:
  broken on purpose`;
- `legacy/stand_in.js#answer`: `legacy/package.json` says `"type": "commonjs"`, so Node would
  load it as CommonJS, and the host refuses it before its code runs, which would have written
  `ran.txt`.

The suffixes no host runs (`stand_in.tsx#answer`) and a missing `.ts` file are stand-in-flags'
own steps: they never reach a host.

Then `stand_in.ts#answer` counts the call and throws an `Error`: a fault of the stand-in, which
stops the run (exit 1). The host logs the error's stack with project-relative paths. `count.txt`
is stand-in-flags'; `events.json` is stand-in-flags' but for the error's name, `Error` where
Python's is `RuntimeError`.
