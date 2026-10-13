# Grida FX: overview

> Working draft, 2026-10-07. Only decisions marked **ratified** have been agreed; everything else is a proposal. The contracts live in [`spec/`](../../spec/); where this overview and `spec/` disagree, `spec/` wins.

Grida FX is a workflow engine for generative asset pipelines. You write a workflow file whose steps are nodes, and FX does five things with it:

- expands it into a graph;
- gives every step a content identity;
- prices the plan before anything is spent;
- caches every paid call by its request;
- records the run.

FX is a standalone product, made by Grida. Think of the Vercel AI SDK without its gateway: FX talks to providers (fal, OpenAI, OpenRouter, Tripo, ElevenLabs, …) with the user's own keys. Grida's gateway (GG) is one more provider, the recommended start: one key, `GG_API_KEY`, serves FX's text, image, video and 3D routes ([providers.md](../../spec/providers.md) §9.7). The Grida ecosystem will get first-class support. Neither is needed for FX to work.

## Decisions

[Agent readiness](../../AGENT_READINESS.md) tracks the user intents, vocabulary,
operation sequences and acceptance scenarios used to evaluate these designs.
It distinguishes implemented behavior, ratified contracts and proposed capabilities.

| | Decision | Status |
|---|---|---|
| 1 | **FX is standalone.** It owns its provider adapters, and users bring their own keys from the environment (`GG_API_KEY`, `OPENROUTER_API_KEY`, `FAL_KEY`, …). FX depends on no Grida package. | ratified |
| 2 | **The engine is written in Rust.** TypeScript users must not need Python, and Python users must not need Node. A native binary is the one runtime both can carry. | ratified |
| 3 | **A protocol, not FFI.** There are no napi, PyO3 or WASM bindings.<br>• SDKs and node bodies talk to the engine through a node protocol: JSON-RPC 2.0 over stdio, framed as in LSP.<br>• An `initialize` handshake carries the protocol version, because a project's SDK and the installed engine will drift. | ratified |
| 4 | **The engine is the only place with engine logic.** Expressions, identity, prompt rendering, the agent loop and file facts all live in the binary. SDKs only build documents, author nodes and host node bodies, so every SDK produces the same cache keys for the same request. | proposal |
| 5 | **Paid work needs a priced plan and a ceiling.** A workflow file can spend the user's money, so nothing paid runs without a priced plan and a `--max-usd` ceiling. A run whose every call is on the user's own plan (the Codex and Grok Build routes, which bill no dollars) is not paid work in this sense: it runs without one, under a ceiling of $0 that refuses any other route (proposed with gridaco/fx#46; awaiting ratification). | ratified |
| 6 | **The workflow registry copies; it never references.**<br>• This repo later doubles as the default registry: `grida-fx add <item>` copies a workflow or node into the user's project (like shadcn registries or `npx skills`) and stamps where it came from.<br>• Nothing is fetched while a run is being planned. | ratified direction |
| 7 | **The Grida umbrella comes last.** `grida fx` (in the `grida` CLI, with this repo as a submodule) hands off to the FX engine, and GG becomes a provider. | ratified |
| 8 | **The standard library's node bodies are in Python** (`grida.fx.std`, with Pillow). A Rust image resize that differs from Pillow's by one byte changes every request downstream of it, so the bodies stay in Python until Rust bodies come, with a planned rekey. The engine still owns `select` and every fact. | in the approved plan |
| 9 | **No third-party OpenAI client.** Adapters are thin clients on FX's own injected transport. The canonical request then *is* the wire body, and every exchange can be replayed in tests. | in the approved plan |
| 10 | **Document versions start at v1 in the `fx` namespace** (`fx: workflow/v1`, `fx-graph-v1`, …). Authored YAML documents carry `fx: <doc>/v1`; machine-written JSON carries `"kind": "fx-<doc>-v1"`. | in the approved plan |
| 11 | **Initial publication waited for trusted publishing; that bootstrap is complete.**<br>• The original decision required the owner to set up trusted publishing (OIDC) before publication. npm sets that up only on a package that already exists, so [RELEASING.md](../../RELEASING.md) records the first-publish bootstrap. Stable releases, from `0.1.0`, publish to both registries.<br>• For source development, `tools/build_engine.py` builds the engine into a checkout's Python SDK, and [examples/](../../examples/) prove that path in CI. | the owner's direction (2026-10-06) |
| 12 | **No cache replay.** A cache is never carried across an identity change by replaying old runs against it. Records keep the canonical request, so a rekey can compute the new keys from them (decision 8). | the owner's decision (2026-10-06) |
| 13 | **Run observation is a public contract.** FX will expose versioned, read-only execution events for independent consumers; the built-in viewer must use the same observation boundary. Recorded events remain authoritative. Ordered reads, cursor/replay semantics, compatibility and observer independence are specified before a follow interface is advertised as stable. [Run observation](../../spec/observation.md) records the wire interface, guarantees, lifecycle and verification. | v1 ratified and implemented (2026-10-08); released in 0.2.0 |
| 14 | **Canvas layout is engine-computed presentation.** The viewer lays a workflow out as a left-to-right grid of cells and stacks repeated instances into decks. An optional `<workflow id>.layout.json` (`fx-layout-v1`) beside the workflow's takes file places declared steps by column and row. The engine computes the cells and the member order and serves them on the scoped layout route; the browser places its own projection's cards in them, so `inspect` (once it reads the layout file) and the browser agree. The file never enters planning, identities, caches or resume. [Canvas layout](../../spec/layout.md) holds the design. | ratified design (2026-10-09); the grid and decks released in 0.2.0; the layout file not implemented |

The [local service contract](../../spec/service.md) is ratified and implemented
(2026-10-08; released in 0.2.0): optional project initialization, a native foreground or
background FX service, fixed-port project browsing, persistent run/plan routes,
and explicit standalone inspection. Workflow processes retain execution ownership.
OS supervision and machine-wide aggregation remain later work.

The [run-control contract](../../spec/control.md) is implemented
(2026-10-08; released in 0.2.0). It follows user intent: inspect an exact invocation, request
cancellation, and optionally verify local completion. Acceptance is distinct from
completion and remote provider outcomes; timeouts on waiting do not grant force
authority. Runner-owned control works without the project service. Signal and
Python/JavaScript SDK cancellation share these semantics. Cancellation is independent
of viewer hosting; pause and explicit terminate remain deferred.
Explicit termination and durable scheduler pause remain separate follow-ups.

The [SDK run-access contract](../../spec/sdk.md) aligns useful Python/JavaScript
jobs: named creation/resume, loading a pinned saved record, and observing its
consistent snapshots and bounded event batches. Async following owns only its
reader and can continue across resumes. The SDKs leave service/browser
lifetimes and diagnostic/lock/take/job maintenance to the CLI; verb mirroring is
not required. See [the SDK guide](../guide/09-sdk-runs.md).

The TypeScript node host ([protocol.md](../../spec/protocol.md) §1, §5.1, §8.2) is
implemented (2026-10-13; released in 0.6.0): `@grida/fx` hosts node bodies, workflow builders
and stand-ins written as `.ts`, `.mts`, `.js` and `.mjs` modules on Node 24 or later,
with the Python host's whole protocol, and the JavaScript SDK answers a run's paid
calls with an in-process stand-in. A project may mix both languages; the engine keeps
one planning host and one pool of hosts per language. This is half of decision 2: a
TypeScript project needs no Python unless it uses a built-in local type, whose bodies
are still Python (decision 8).

## Names

| Thing | Name | Status |
|---|---|---|
| Product | Grida FX | ratified |
| Command | `grida fx`, from the npm `grida` package, which runs `grida-fx`. A bare `fx` is taken: on npm it is antonmedv's JSON viewer, and the name is taken on PyPI and crates.io. | ratified |
| JS SDK | `@grida/fx`: light, with the engine in per-platform optional packages (`@grida/fx-darwin-arm64`, …), as esbuild does | ratified |
| Python SDK | import `grida.fx`, fixed from day one. The distribution is `grida` (stable releases from `0.1.0`; prereleases use PEP 440, such as `0.2.0a1`). Its per-platform wheels carry the engine binary as package data, with no console command: Python users run the engine as `python -m grida.fx <verb>` or through the API, so they never need Node. | shipped |
| Rust crates | `grida-fx`, `grida-fx-std`, `grida-fx-protocol`, … (free on crates.io) | proposal |
| Project files | `fx.yaml`, `fx.lock`, `.fx/` | proposal |
| Environment | `GRIDA_FX_PYTHON`, `GRIDA_FX_NODE`, `GRIDA_FX_TOOL_<NAME>`; provider keys under their providers' usual names | proposal |
| Built-in type namespace, schema ids | `fx/…`, `fx-…-v1` | proposal |

## What shipped

- **Contracts:** the documents and the node protocol as JSON Schema, identity (below), the
  conformance suite, parameterised by command, and the guide.
- **The offline core in Rust:** workflow parsing, expressions, expansion, identity, planning,
  pricing, the routes document, and `describe` and `build`.
- **The runner, the node hosts and the SDKs:** the store and atomic publish, the call cache, budget
  reservation and settlement, the retry owner, the event log, the agent loop, prompt rendering
  and facts, and `run`; the Python and TypeScript node hosts; `grida.fx` and `@grida/fx`, with
  stand-in answers that answer a run's paid calls offline with a test's function
  ([protocol.md](../../spec/protocol.md) §5.7).
- **Provider adapters:** text and agent calls go through FX's own thin client (decision 9), and
  cache keys come from FX's canonical request. FX ships a default route table with prices and
  capabilities, which users can override; actual cost is settled from what the provider
  reports. Each adapter is checked against an injected transport and synthetic exchange
  fixtures: CI has no keys and spends nothing.
- **Packaging and releases:** `@grida/fx` with its per-platform engine packages on npm, and
  `grida` wheels, one per platform, on PyPI, published through trusted publishing. Stable
  releases start at `0.1.0` ([RELEASING.md](../../RELEASING.md)).

## Later

- The workflow registry.
- GG as a provider.
- The `grida fx` umbrella.
- Views: the [canvas layout](../../spec/layout.md) file (decision 14), [custom views and
  optional OS supervision](../../TODO.md#workflow-and-run-viewer). Already released: the
  [local workflow-plan and run viewer](../guide/07-viewing.md), the read-only node canvas with
  its automatic grid and decks, live observation, the persistent project service,
  [portable recorded-run exports](../../spec/exports.md) and the shared static browser reader.
- Standard-library node bodies in Rust, with a planned rekey (decision 8). This lets a TypeScript project that uses them run without Python, the other half of decision 2, and also makes their outputs the same bytes on every platform. Today Pillow's Linux x86-64 build writes other PNG bytes than its macOS arm64 build for some pictures, so a cache filled on one platform misses on the other for every paid call downstream of a std picture output.

## Identity

The normative definitions are [`spec/identity.md`](../../spec/identity.md) and
[`spec/yaml.md`](../../spec/yaml.md). In outline:

- **Canonical JSON** is [RFC 8785 (JCS)](https://www.rfc-editor.org/rfc/rfc8785) with I-JSON
  numbers: no NaN or infinity, integers within ±2^53, and `1` = `1.0`. Implementations exist for
  Rust, Python and JS.
- **YAML** is a strict YAML 1.2 core subset. Duplicate keys, timestamps, collections as keys and
  ambiguous scalars are errors; plain keys are always strings.
- **The plan digest** hashes the authored workflow documents, the inputs, the takes, the type
  identities and the route fingerprints (`spec/identity.md` §10).
- **A step's identity** includes upstream file digests only: content addressing keeps everything
  downstream cached when an upstream rerun writes the same bytes (`spec/identity.md` §13).
- **File facts** are computed by the engine. Each one is specified and pinned by media fixtures.
- **Records keep the canonical request**, so a future rekey can be computed. Requests never carry
  secrets: keys travel in the transport.

## Node protocol

Node bodies run out of the engine's process, over the protocol that
[`spec/protocol.md`](../../spec/protocol.md) specifies. In outline:

- **Session:** `initialize` exchanges protocol versions; `shutdown` and `exit` end it.
- **Engine to host:**
  - `describe`: the host returns node specs, plus its source closure as `(label, path)` pairs. The engine reads and hashes the files.
  - `build`: a Python or TypeScript builder returns a workflow document.
  - `run`: carries the instance, its take, the params, staged read-only input files and a work dir. Outputs come back by path.
  - `tool.invoke`, `agent.check` and `$/cancel`.
- **Host to engine:** requests with ids, each answered by the engine:
  - `capability`;
  - `agent.run` (the engine runs the model loop);
  - `fact`, `annotate` and `progress`;
  - `prompt.render`, using the engine's expression language;
  - `file.put`, which stores bytes or a JSON value and returns a file reference.

## Retry and billing (ratified)

Every attempt that may have billed is counted:

- **One retry owner:** the engine. Transports never retry.
- **When a request may be resent:** only when it provably was not received. That means the connection or send failed, or the provider says it took nothing.
- **Every other repeat is a new attempt.** It is reserved, recorded and settled, at most 6 in all.
- **Checks after an answer** (signature, opacity) fail the attempt as billed.
- **Long jobs never resubmit.** They collect by their recorded handle.

## Open decisions

| # | Decision | Default |
|---|---|---|
| D1 | YAML | the strict subset (not exact PyYAML compatibility) |
| D2 | Python distribution | `grida` itself (the alternative is `grida-fx`, which `grida` would later depend on) |
| D3 | Crate names | the `grida-fx-*` prefix |

## Verification

- **This repo:** `cargo fmt`, `clippy` and `test`; the conformance suite against `grida-fx`;
  adapter fixtures; and an installed-package check per platform.
- **Spending:** none in tests or CI. A live call needs the owner's go and a cap.
