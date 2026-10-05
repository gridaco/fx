# FX contracts

These documents define FX independently of any implementation. The Rust engine, the SDKs and the conformance suite all follow them.

| Document | What it defines |
|---|---|
| [identity.md](identity.md) | Values, canonical JSON, and every digest: files, node types, routes, steps, calls, plans |
| [yaml.md](yaml.md) | The strict YAML subset FX reads |
| [store.md](store.md) | The cache layout and its records |
| [protocol.md](protocol.md) | The node protocol between the engine and a node host |
| [schemas/](schemas/) | JSON Schemas for every document and record |
| [vectors/](vectors/) | Test vectors: canonical JSON, YAML, identity examples |

A change to identity changes every cache. It needs a new `kind` version in [identity.md](identity.md), new vectors, and a migration note.
