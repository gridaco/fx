# run-timeout-ts

[`run-timeout`](../run-timeout/) for TypeScript bodies (`in/nodes/slow.ts`), each under a step
`timeout: 1` and each waiting 30 seconds. After a second the engine sends `$/cancel`, which the
TypeScript node host turns into an abort of the body's `ctx.signal`, with an `Error` whose message
is `the engine cancelled run` as its reason ([protocol.md](../../spec/protocol.md) §5.6).

- `nap` never looks at the signal, as run-timeout's Python body never looks at `ctx.cancelled`.
  The engine ends its node host 5 seconds later.
- `watch` waits on the signal: it writes the reason to `aborted.txt` and throws `ctx.fail`.

Both steps fail with `ran past 1 seconds`, the engine's own failure, not the body's, and with
`code: null` in their `node_failed` events: a timeout carries no error code. Neither runs again,
although each type declares `retry: "engine"`: `naps.txt` and `watches.txt`, which each body
writes in the project folder before it waits, say `1`.

## Against run-timeout

`naps.txt` is run-timeout's, byte for byte, and so is `nap`'s entry in `project.json`. The plan
digest differs, as does everything else in `project.json` and `events.json` that names the node
type, and `watch` is this case's own.
