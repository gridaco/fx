# Contributing to Grida FX

Thanks for helping improve FX. Start with [AGENTS.md](AGENTS.md) for repository
boundaries and guardrails. The [specifications](spec/) own public contracts;
[the overview](docs/wg/overview.md) records the design and planned work.

[Agent readiness](AGENT_READINESS.md) is the living design harness for how external
agents discover and compose FX operations. Use its scenarios when changing the
CLI, SDKs, lifecycle or public contracts, and update supported behavior and evidence
alongside those changes.

## Build from source

Source development needs Git, Rust/Cargo ([rustup.rs](https://rustup.rs)),
[Bun](https://bun.sh) 1.4.0, [uv](https://docs.astral.sh/uv/), and Python 3.11+.
Installed npm packages and Python wheels already carry the engine and viewer;
users do not need this setup.

```sh
git clone https://github.com/gridaco/fx
cd fx
uv sync --project python
bun --no-env-file install --frozen-lockfile
python3 tools/build_engine.py
uv run --project python python -m grida.fx --version
```

`tools/build_engine.py` builds the production viewer, embeds it in a release
engine, and places that engine at `python/src/grida/fx/_bin/grida-fx` (gitignored).
Use `--debug` for a development engine build. Run the helper again after pulling
or switching commits so the engine matches the SDK beside it. Repeat the frozen
Bun installation after dependency changes.

Direct Cargo builds require `bun --no-env-file run build:viewer` first; a missing
bundle fails the engine build. Source execution through `uv run --project python
python -m grida.fx` uses the checkout's SDK and Python environment.

### Use the checkout from another project

Install the checkout's `python/` package editable. With uv, declare the `grida`
dependency and point its source at the checkout:

```toml
[tool.uv.sources]
grida = { path = "<checkout>/python", editable = true }
```

That project's `grida.fx` uses the engine built into the checkout, without a
separate engine-path setting. Rebuild it when the checkout changes.

## Run examples

[examples/README.md](examples/README.md) lists the projects and their commands.
Every example plans offline. `hello` and `image-recolor` run entirely at $0 with
no provider keys. Examples that need a capability FX does not yet implement say
so and only plan.

The examples' fixture pictures are code-drawn. Recreate them from the repository
root with `uv run --project python python examples/draw_placeholders.py`.

## Verify your changes

Run the checks for the boundaries you changed; [AGENTS.md](AGENTS.md#verification)
has the full command list. All tests and CI remain provider-free. Use injected
transports or stand-ins for paid calls rather than making real requests.

Common checks from the repository root:

```sh
# Rust engine
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace

# Browser viewer
bun --no-env-file run check:viewer
bun --no-env-file run test:viewer
bun --no-env-file run build:viewer

# Contracts and runnable examples
uv run --project python python tools/check_spec.py
cargo build --locked -p grida-fx
uv run --project python python conformance/run.py --command target/debug/grida-fx
uv run --project python python tools/check_examples.py
```

Build the debug engine before using `target/debug/grida-fx`; the source-build
helper defaults to the release profile. Use the Python and JavaScript checks in
AGENTS.md when changing either SDK. Changes to public schemas, identities, or
protocol behavior update their specification and conformance evidence together.

## Viewer development

`web/viewer` builds the client embedded in the engine. Shared browser contracts
and independently testable TypeScript classes live in `js/fx-web`; React adapters
live in `js/fx-react`. Nontrivial UI state and interaction belong in classes, with
React kept as a thin presentation layer.

The [canonical fixture suite](fixtures/viewer/README.md) creates real, provider-free
plans and recorded runs covering branching, joins, matrices, repeats, nesting,
media, cache reuse, takes, and failures. Generated data stays outside source fixtures.
See the fixture README for setup, generation, and viewer commands.

## Releases and safety

[RELEASING.md](RELEASING.md) owns packaging, installed-package verification,
platform targets, and publication. Publishing requires explicit owner authorization.

Keep tests offline and never persist credentials, authorization headers, or signed
URLs. Any live provider work needs explicit authorization and a dollar ceiling.
Keep committed examples and media original, with a documented rights basis for
referenced inputs. Follow AGENTS.md for the remaining artifact and contract rules.

### Observation development

The shared contract is [spec/observation.md](spec/observation.md). After building the
engine and viewer, run the credential-free integration harness:

```sh
uv run --project python python tools/check_observation.py --command target/debug/grida-fx
```

It creates fresh code-only fixture projects and verifies snapshot attachment,
CLI/HTTP replay, slow consumers, resume, artifacts, cancellation, failure and
standalone server lifetime. `cargo test -p grida-fx --test service --test service_run` covers project
service readiness, ownership, restart and execution independence. Generated
evidence stays outside Git. See the
[fixture guide](fixtures/viewer/README.md#live-observation) for manual viewing.

### Project service development

[spec/service.md](spec/service.md) defines project ownership, local catalog,
readiness and standalone behavior. After building, prepare an isolated project:

```sh
uv run --offline --no-sync --project python python tools/demo_service.py
```

The helper creates only source fixtures and prints ordinary user commands to
start FX, plan, run and stop it. It never starts a service or executes a workflow.
Use its fresh project to inspect uncached waits; repeated identical runs correctly
reuse their cache. No Docker, frontend dev server or provider account is needed.
