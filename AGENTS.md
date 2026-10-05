# AGENTS.md

Guardrails for working in this repository. [README.md](README.md) says what FX is; [docs/wg/overview.md](docs/wg/overview.md) holds the direction, the decisions and the plan.

## Layout

| Path | What it is |
|---|---|
| `crates/grida-fx-core` | The offline core: strict YAML, canonical JSON, expressions, documents, expansion, identity, planning, pricing. No I/O beyond reading a project. |
| `crates/grida-fx-protocol` | The node protocol: JSON-RPC 2.0 over stdio, framed as in LSP. |
| `crates/grida-fx-runtime` | Running: the store, the call cache, budgets, the retry owner, facts, the agent loop, node hosts. |
| `crates/grida-fx-providers` | Provider adapters behind an injected transport. |
| `crates/grida-fx` | The `grida-fx` command. |
| `spec/` | The language-neutral contracts: schemas, identity, protocol, test vectors. Code follows `spec/`; a change to identity is a change to `spec/` first. |
| `conformance/` | Cases that hold any implementation to the spec through the command line only. |
| `python/` | The `grida` distribution; FX is `grida.fx`. |
| `js/` | `@grida/fx` and its per-platform engine packages. |
| `tools/` | Checks that run outside the engine: the spec gate and the independent digest checker. The comparison with stage-gen's engine arrives with the offline core. |

## Rules

- **The repository is public.** Everything committed must be original and brand-neutral. Examples and fixtures use made-up names (`acme`, `img-a`, …). Test media is synthesized by a committed script or snippet (named next to the media), never generated art.
- **Offline is the default, and nothing spends.** Tests and CI never call a provider and hold no keys. A live call needs the owner's explicit go and a cap.
- **The engine is the only place with engine logic.** Expressions, identity, prompt rendering, the agent loop and facts live in Rust. SDKs build documents, author nodes and host node bodies; they never compute an identity.
- **One retry owner: the engine.** Transports and adapters never retry. A request is resent only when it provably was not received; every other repeat is a new, recorded and settled attempt, at most 6 in all.
- **Never persist secrets.** No keys, authorization headers, signed URLs or private absolute paths in records, logs, caches or test fixtures. Provider keys come from the environment through the allowlisted loader; `.env` is optional and never printed.
- **Contracts use `lower_snake_case`.** Keep mandatory external vocabulary exactly (`$ref`, `$defs`, `additionalProperties`).
- **Identifiers, comments, logs and messages are in English.**

## Verification

Run the checks for what you touched:

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
(cd python && uv run ruff check . && uv run ruff format --check . && uv run pytest)
(cd js/fx && bun run typecheck && bun test)
uv run --project python python tools/check_spec.py
uv run --project python python conformance/run.py --command target/debug/grida-fx
```
