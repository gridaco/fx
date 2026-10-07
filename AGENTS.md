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
| `crates/grida-fx-viewer` | Read-only run projection and static plan/run loopback host for the embedded viewer. |
| `spec/` | The language-neutral contracts: schemas, identity, protocol, test vectors. Code follows `spec/`; a change to identity is a change to `spec/` first. |
| `conformance/` | Cases that hold any implementation to the spec through the command line only. |
| `python/` | The `grida` distribution; FX is `grida.fx`. |
| `js/fx/` | `@grida/fx`, the Node SDK and command launcher; per-platform engines are packaged separately. |
| `js/fx-web/`, `js/fx-react/` | Private browser-only contracts, TypeScript controllers/canvas, and thin React adapters. |
| `web/viewer/` | Vite/React/Tailwind client, built and embedded in the engine. |
| `examples/` | Projects written the way a user would. CI plans every one and runs every runnable one offline through the SDK, from a fresh build (rigged-character only plans). |
| `skills/` | Self-contained user-installable agent skills, distributed through `npx skills add gridaco/fx`; the README owns layout and authoring conventions. |
| `fixtures/viewer/` | Canonical provider-free development harness for viewer topology, media, cache, takes, failures and recorded in-progress states; generated data stays outside the source fixture. |
| `tools/` | What runs outside the engine: the spec gate, the independent digest checker, packaging, `build_engine.py` (the engine into a checkout's SDK) and `check_examples.py`. |

## Rules

- **The repository is public.** Everything committed must be original and brand-neutral. Test fixtures and conformance cases use made-up names (`acme`, `img-a`, …); examples use the built-in table's route names, with original settings and characters. Test media is synthesized by a committed script or snippet (named next to the media), never generated art.
- **Offline is the default, and nothing spends.** Tests and CI never call a provider and hold no keys. A live call needs the owner's explicit go and a cap.
- **The engine is the only place with engine logic.** Expressions, identity, prompt rendering, the agent loop and facts live in Rust. SDKs build documents, author nodes and host node bodies; they never compute an identity.
- **One retry owner: the engine.** Transports and adapters never retry. A request is resent only when it provably was not received; every other repeat is a new, recorded and settled attempt, at most 6 in all.
- **Never persist secrets.** No keys, authorization headers, signed URLs or private absolute paths in records, logs, caches or test fixtures. Provider keys come from the environment through the allowlisted loader; `.env` is optional and never printed.
- **Examples run.** Their node bodies are real code and their routes are the built-in table's, and the plan output in a README is pasted from the engine. An example that needs something FX does not have yet (a capability without an adapter, a planned API) only plans: it may keep an illustrative route of its own and placeholder bodies, and its README says so. A change that alters what an example prints updates its README.
- **Contracts use `lower_snake_case`.** Keep mandatory external vocabulary exactly (`$ref`, `$defs`, `additionalProperties`).
- **Identifiers, comments, logs and messages are in English.**
- **Nontrivial browser UI uses vanilla TypeScript classes.** Own canvas, viewport,
  interaction and viewer state in independently testable classes. React is a thin
  presentation/mount layer; use a React-specific framework only when it is the
  practical choice for a capability that cannot reasonably fit that boundary.
- **Use Lucide package icons for browser UI chrome.** Bundle the imported icons
  with the client so installed viewers work offline. Keep workflow geometry
  (connections, sockets and frames) owned by the canvas rather than the icon library.

## Verification

Run the checks for what you touched:

```sh
bun --no-env-file install --frozen-lockfile
bun --no-env-file run check:viewer && bun --no-env-file run test:viewer && bun --no-env-file run build:viewer
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
(cd python && uv run ruff check --config pyproject.toml . ../tools ../conformance ../examples ../fixtures && uv run ruff format --check --config pyproject.toml . ../tools ../conformance/run.py ../examples ../fixtures && uv run pytest)
(cd js/fx && bun --no-env-file run typecheck && bun --no-env-file test)
uv run --project python python tools/check_spec.py
uv run --project python python conformance/run.py --command target/debug/grida-fx
python3 tools/build_engine.py && uv run --project python python tools/check_examples.py
python3 tools/check_viewer.py --command "$PWD/python/src/grida/fx/_bin/grida-fx"
uv run --project python python tools/check_viewer_fixtures.py
```

Source builds embed `web/viewer/dist` and refuse a missing bundle. Bun is a build-time dependency;
installed npm and Python packages serve the bundled viewer without a frontend runtime.
