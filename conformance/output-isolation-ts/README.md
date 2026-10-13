# output-isolation-ts

A TypeScript body's output never reaches the node protocol ([protocol.md](../../spec/protocol.md)
§1, "Output streams"). Before any other module loads, the TypeScript node host opens the
protocol's pipe as a descriptor of its own and opens its standard error anew as descriptor 1; it
gives descriptor 0 the null device, so no child reads protocol bytes either. The host's standard
error is not the engine's: on Linux and macOS the engine gives the host a pipe of its own and
copies what arrives on it, in whole lines, to its own standard error, which is where the markers
arrive.

`in/nodes/noisy.ts` writes one marker line each way a body can write to standard output:

| Marker | How |
|---|---|
| `marker-module` | `console.log` while the module loads (once for the planning host, once for the run's) |
| `marker-console`, `marker-info` | `console.log`, `console.info` |
| `marker-write` | `process.stdout.write` |
| `marker-sync` | a child spawned with `spawnSync` and `stdio: "inherit"` |
| `marker-async` | a child spawned with `spawn` and `stdio: "inherit"`, awaited |
| `marker-grandchild` | a child's child: `sh -c 'sh -c "echo …"; true'`, where `; true` keeps the outer shell from handing its process to the inner one |
| `marker-stdin` | the body's report of what a child read from its inherited standard input: 0 bytes |

## What the case pins

- Every marker reaches the engine's standard error (`mentions`).
- None reaches the engine's standard output: `run.txt` is the run's summary and nothing else.
- The protocol stays intact: the run succeeds, and the body's output is its text (`said.txt`).

The case runs on Linux and macOS in CI (the `conformance` job, and `host` through `grida fx`).
