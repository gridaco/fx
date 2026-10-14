# FX contracts

These documents define FX independently of any implementation. The Rust engine, the SDKs and the conformance suite all follow them.

| Document | What it defines |
|---|---|
| [identity.md](identity.md) | Values, canonical JSON, and every digest: files, node types, routes, steps, calls, plans |
| [facts.md](facts.md) | File facts: what `facts(f)` gives for every file, image, WAV, MP4 and Matroska/WebM file |
| [yaml.md](yaml.md) | The strict YAML subset FX reads |
| [expressions.md](expressions.md) | Expressions: what counts as nothing, and the fallback `??`, including a missing field |
| [store.md](store.md) | The cache layout and its records |
| [observation.md](observation.md) | Run-observation v1: consistent snapshots, bounded events, cursors and errors |
| [exports.md](exports.md) | Portable terminal run inspection: disclosure, verified media, immutable manifests and offline export |
| [service.md](service.md) | Project initialization, local service lifecycle, stable run/plan URLs and standalone inspection |
| [layout.md](layout.md) | Canvas layout: the automatic grid and decks, and the optional `<workflow id>.layout.json` file (not implemented) |
| [control.md](control.md) | Local run control: exact invocation targeting, cancellation acceptance, verified local completion and force boundaries |
| [sdk.md](sdk.md) | SDK user jobs: named execution/resume, pinned saved records, independent observation and in-process stand-ins |
| [distribution.md](distribution.md) | The ways to run FX (`grida fx`, `grida-fx`, `python -m grida.fx`), the invoker, the host contract and the release order |
| [protocol.md](protocol.md) | The node protocol between the engine and a node host, and the Python and TypeScript hosts |
| [capabilities.md](capabilities.md) | Each paid capability's canonical request, its answer, and the features a route declares |
| [adapters.md](adapters.md) | The adapter contract, version 1: where adapters run, the transport, how provider answers become outcomes and bills, refusals, long jobs, downloads and reasons, and provider options |
| [providers.md](providers.md) | Provider keys and endpoints, the built-in adapters and their references, the built-in route table, and renamed features |
| [schemas/](schemas/) | JSON Schemas for every document and record |
| [vectors/](vectors/) | Test vectors: canonical JSON, YAML, identity examples, file facts |

A change to identity changes every cache. It needs a new `kind` version in [identity.md](identity.md), new vectors, and a migration note. The one exception, the route contract's completion ([identity.md](identity.md) §7), kept `fx-route-v1` because it moved no built-in fingerprint; it has its vectors and its migration note.
