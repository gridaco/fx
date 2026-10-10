# FX contracts

These documents define FX independently of any implementation. The Rust engine, the SDKs and the conformance suite all follow them.

| Document | What it defines |
|---|---|
| [identity.md](identity.md) | Values, canonical JSON, and every digest: files, node types, routes, steps, calls, plans |
| [facts.md](facts.md) | File facts: what `facts(f)` gives for every file, image, WAV, MP4 and Matroska/WebM file |
| [yaml.md](yaml.md) | The strict YAML subset FX reads |
| [store.md](store.md) | The cache layout and its records |
| [observation.md](observation.md) | Run-observation v1: consistent snapshots, bounded events, cursors and errors |
| [service.md](service.md) | Project initialization, local service lifecycle, stable run/plan URLs and standalone inspection |
| [layout.md](layout.md) | Canvas layout: the automatic grid and decks (Ship A, in source, unreleased) and the optional `<workflow id>.layout.json` file (Ship B, not implemented) |
| [control.md](control.md) | Local run control: exact invocation targeting, cancellation acceptance, verified local completion and force boundaries |
| [sdk.md](sdk.md) | SDK user jobs: named execution/resume, pinned saved records and independent observation |
| [protocol.md](protocol.md) | The node protocol between the engine and a node host |
| [capabilities.md](capabilities.md) | Each paid capability's canonical request, its answer, and the features a route declares |
| [providers.md](providers.md) | The transport, provider keys, how provider answers become outcomes and bills, long jobs, downloads, and the built-in route table |
| [schemas/](schemas/) | JSON Schemas for every document and record |
| [vectors/](vectors/) | Test vectors: canonical JSON, YAML, identity examples, file facts |

A change to identity changes every cache. It needs a new `kind` version in [identity.md](identity.md), new vectors, and a migration note.
