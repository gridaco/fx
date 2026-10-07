# Grida FX: overview

> Working draft, 2026-10-07. Only decisions marked **ratified** have been agreed; everything else is a proposal. The contracts live in [`spec/`](../../spec/); where this overview and `spec/` disagree, `spec/` wins.

Grida FX is a workflow engine for generative asset pipelines. You write a workflow file whose steps are nodes, and FX does five things with it:

- expands it into a graph;
- gives every step a content identity;
- prices the plan before anything is spent;
- caches every paid call by its request;
- records the run.

FX is a standalone product, made by Grida. Think of the Vercel AI SDK without its gateway: FX talks to providers (fal, OpenAI, OpenRouter, Tripo, ElevenLabs, …) with the user's own keys. Grida's gateway (GG) will later be one more provider, and the Grida ecosystem will get first-class support. Neither is needed for FX to work.

## Where it comes from

FX grew out of **gnode**, the Python engine inside [softmarshmallow/stage-gen](https://github.com/softmarshmallow/stage-gen). Milestone 2 moved stage-gen onto FX and deleted gnode; stage-gen `bb784832` is its last state. The table covers gnode as FX began: the Python code (about 21,500 lines) and the material around it.

| What | Where it was in stage-gen | Lines |
|---|---|---|
| Workflow engine: documents, expansion, planning, runner, store, command line | `src/gnode/workflow/` | 10,100 |
| Capability specs and services (image, video, audio, mesh, structured, tool loop, …) | `src/gnode/modalities/` | 3,700 |
| Provider adapters (openrouter, openai, fal, tripo, elevenlabs) | `src/gnode/providers/` | 2,900 |
| Retry, atomic writes, encoding | `src/gnode/reliability/` | 1,600 |
| Routes, binding, ledger, records, provenance | `src/gnode/*.py`, `src/gnode/contracts/` | 2,200 |
| Standard node library | `src/gnode_std/` | 900 |
| Document schemas (9) | `src/gnode/schemas/` | |
| Conformance suite (14 cases) | `tests/conformance/` | |
| Engine tests (about 10,400 lines with conformance) | `tests/unit/gnode/`, `tests/unit/gnode_std/` | |
| User guide and 4 example projects | `docs/guide/` | |
| Rings rule and decision 0072 | `docs/spec/gnode-rings.md`, `docs/decisions/0072-*` | |

## Decisions

| | Decision | Status |
|---|---|---|
| 1 | **FX is standalone.** It owns its provider adapters, and users bring their own keys from the environment (`OPENROUTER_API_KEY`, `FAL_KEY`, …; later `GG_API_KEY`). FX depends on no Grida package. | ratified |
| 2 | **The engine is written in Rust.** TypeScript users must not need Python, and Python users must not need Node. A native binary is the one runtime both can carry. | ratified |
| 3 | **A protocol, not FFI.** There are no napi, PyO3 or WASM bindings.<br>• SDKs and node bodies talk to the engine through a node protocol: JSON-RPC 2.0 over stdio, framed as in LSP.<br>• An `initialize` handshake carries the protocol version, because a project's SDK and the installed engine will drift. | ratified |
| 4 | **The engine is the only place with engine logic.** Expressions, identity, prompt rendering, the agent loop and file facts all live in the binary. SDKs only build documents, author nodes and host node bodies, so every SDK produces the same cache keys for the same request. | proposal |
| 5 | **Paid work needs a priced plan and a ceiling.** A workflow file can spend the user's money, so nothing paid runs without a priced plan and a `--max-usd` ceiling. | ratified |
| 6 | **The workflow registry copies; it never references.**<br>• This repo later doubles as the default registry: `grida-fx add <item>` copies a workflow or node into the user's project (like shadcn registries or `npx skills`) and stamps where it came from.<br>• Nothing is fetched while a run is being planned.<br>• This comes after the first milestone. | ratified direction |
| 7 | **The Grida umbrella comes last.** `grida fx` (in the `grida` CLI, with this repo as a submodule) hands off to the FX engine, and GG becomes a provider. | ratified |
| 8 | **The standard library's node bodies stay in Python for milestone 1** (`grida.fx.std`, ported from gnode with Pillow). The reason was milestone 2's planned replay: it could keep a cached paid call only if FX built the same request gnode built, and a Rust image resize that differs from Pillow's by one byte changes every request downstream of it. The replay was dropped (decision 12), and the bodies stay in Python until Rust bodies come, with a planned rekey. The engine still owns `select` and every fact. | in the approved plan |
| 9 | **No third-party OpenAI client.** Adapters are thin clients on FX's own injected transport. The canonical request then *is* the wire body, and every exchange can be replayed in tests. (This settles D4.) | in the approved plan |
| 10 | **Document versions restart at v1 in the `fx` namespace** (`fx: workflow/v1`, `fx-graph-v1`, …). "Identity v3" and "protocol v2" below are design names relative to gnode. Authored YAML documents carry `fx: <doc>/v1`; machine-written JSON carries `"kind": "fx-<doc>-v1"`. | in the approved plan |
| 11 | **Initial publication waited for trusted publishing; that bootstrap is complete.**<br>• The original decision required the owner to set up trusted publishing (OIDC) before publication. npm sets that up only on a package that already exists, so [RELEASING.md](../../RELEASING.md) records the first-publish bootstrap. Stable `0.1.0` is now published on both registries.<br>• For source development, `tools/build_engine.py` builds the engine into a checkout's Python SDK, and [examples/](../../examples/) prove that path in CI.<br>• stage-gen takes this repository as a submodule at `third_party/fx`, with a path dependency on its `python/`, and FX can be changed in place there. Detaching later changes that dependency line, not stage-gen's imports. | the owner's direction (2026-10-06) |
| 12 | **No cache replay.** stage-gen moves onto FX with an empty cache, and gnode is deleted in the same step, with no period of running both engines. This replaces the replay milestone 2 first planned; [milestone 2](#milestone-2-stage-gen-moves-onto-fx) says why. | the owner's decision (2026-10-06) |

## Names

| Thing | Name | Status |
|---|---|---|
| Product | Grida FX | ratified |
| Command | `grida-fx`; later `grida fx` hands off to it. A bare `fx` is taken: on npm it is antonmedv's JSON viewer, and the name is taken on PyPI and crates.io. | ratified |
| JS SDK | `@grida/fx`: light, with the engine in per-platform optional packages (`@grida/fx-darwin-arm64`, …), as esbuild does | ratified |
| Python SDK | import `grida.fx`, fixed from day one. The distribution is `grida` (stable releases from `0.1.0`; prereleases use PEP 440, such as `0.2.0a1`). Its per-platform wheels carry the engine binary as package data, with no console command: Python users run the engine as `python -m grida.fx <verb>` or through the API, so they never need Node. | shipped |
| Rust crates | `grida-fx`, `grida-fx-std`, `grida-fx-protocol`, … (free on crates.io) | proposal |
| Project files | `fx.yaml`, `fx.lock`, `.fx/` | proposal |
| Environment | `GRIDA_FX_PYTHON`, `GRIDA_FX_TOOL_<NAME>`; provider keys under their providers' usual names | proposal |
| Built-in type namespace, schema ids | `fx/…`, `fx-…-v1` | proposal |

## Milestone 1: FX standalone, published as a preview

This milestone works only in this repo. The Python gnode stays in stage-gen, frozen and untouched, until milestone 2. Nothing from it is moved here as code: this repo gets the contracts, the conformance suite and the guide, plus new code (the Rust engine and the thin SDKs).

**1. Contracts.**
- The documents and node protocol v2 as JSON Schema.
- Identity v3 (below).
- The conformance suite, moved from stage-gen and parameterised by command.
- The guide, moved from stage-gen.
- New conformance cases for what is unpinned today:
  - unversioned local node types and `fx.lock`;
  - declared resources;
  - `facts()`;
  - number and YAML edge cases;
  - JCS vectors.
- **Status:** done (`7aa75a2`).

**2. The offline core in Rust, with the Python describe host.**
- **Scope:** workflow parsing, expressions, expansion, identity, planning, pricing, the routes document, and protocol v2 `describe` and `build`. Most conformance cases and every stage-gen workflow load Python node modules even to plan, so the Python host's `describe` and `build` arrive here, not in step 3.
- **Gate:** checked against gnode, the Python engine, with `tools/compare_gnode.py` (deleted with gnode in milestone 2). With digests removed, the expanded graphs, prices and projections must match the Python output exactly. A small standalone script checks the digest formula, and the published JCS vectors check the canonical JSON.
- **Status:** done (`315f378`).

**3. The runner, the Python node host and the `grida.fx` SDK.**
- **Scope:**
  - the store and atomic publish;
  - the call cache;
  - budget reservation and settlement;
  - the retry owner;
  - the event log;
  - the agent loop, prompt rendering and facts;
  - protocol v2 `run`.
- **The SDK** is the authoring surface stage-gen uses today: `node`, `Ctx`, `tool`, `ToolReply`, `Group`, `StepRef`, `Workflow`, and `plan`, `run` and `run_async`, rehosted over the protocol. An import census of stage-gen settles the rest.
- **Gate:**
  - every conformance case passes;
  - stage-gen's workflows and game pipelines plan and dry-run on the Rust engine, with their Python nodes and builders unchanged in shape.
- **Status:** done (`403760e`, `3fafd28`).

**4. Provider adapters.**
- **Scope:** only the routes our workflows use: openrouter (images, structured output, agent turns, music), openai (images with native alpha), fal (images, video, background removal), tripo (mesh, rig) and elevenlabs (speech, sound effects).
- **Text and agent calls** go through FX's own thin client (decision 9). Cache keys come from FX's canonical request.
- **Prices:** FX ships a default route table with prices and capabilities, which users can override. Actual cost is settled from what the provider reports.
- **Tests cost nothing to run:** each adapter is checked against an injected transport and synthetic exchange fixtures. CI has no keys and spends nothing.
- **Status:** done (`6b84fd8`, `6205da0`).

**5. Packaging and a preview publish.**
- **npm:** `@grida/fx` plus the per-platform engine packages.
- **PyPI:** `grida` wheels, one per platform.
- **The preview gate is complete:** milestone 2 and the published-preview installed checks passed. Stable releases start at `0.1.0`, using npm's `latest` tag and ordinary PyPI installation; explicit prereleases remain available through npm's `next` tag and PyPI prerelease selection ([RELEASING.md](../../RELEASING.md#checking-the-published-release)).
- **Status:** published. The npm packages were bootstrapped at `0.1.0-alpha.1`; `v0.1.0-alpha.2` then published all five npm packages and all four `grida` wheels (`0.1.0a2`) through GitHub Actions trusted publishing, with provenance and attestations. All four targets passed their installed-package checks in CI; fresh registry installations on Apple Silicon also passed all 39 conformance cases and the embedded-viewer checks ([RELEASING.md](../../RELEASING.md)).

## Milestone 2: stage-gen moves onto FX

stage-gen is the acceptance test. Its workflow files and Python node bodies stayed as they were, but for the names below.

**Status:** stage-gen runs on FX, and gnode is deleted (FX `f4e7dea`, stage-gen `f499c35d`). Every workflow and game planned on FX at gnode's prices, with only digests moved, and its tests run whole workflows offline with stand-ins. The live smoke runs took place on 2026-10-07 (below). Since then the image routes are priced in tiers by size ([providers.md](../../spec/providers.md) §10), so an image step no longer plans at gnode's image price.

What changed:

- FX is a submodule at `third_party/fx` (decision 11): `grida` is a path dependency on `third_party/fx/python`, stage-gen's gates build the engine with `tools/build_engine.py` (its pre-push hook from the pinned commit), and its own linters skip it;
- imports are `from grida.fx import …`;
- project files are `fx.yaml` and `fx.lock`. stage-gen's root `fx.yaml` finds its workflows by id through `workflows:`, and each workflow home and game keeps its own routes;
- workflow files start with `fx: workflow/v1`, and built-in types are `fx/<name>@1`;
- the command is `python -m grida.fx`, since the `grida` distribution has no console command (Names);
- feature names in `requires:`, in workflow files and in builder code, moved to FX's vocabulary ([capabilities.md](../../spec/capabilities.md)): `transparent_background` → `alpha`, `masked_edit` → `mask`, `reference_images` → `image_input` and `data_url_reference_input` → `image_input` ([providers.md](../../spec/providers.md) §11). The planner refuses the old names and names FX's;
- a builder's takes file is renamed from `<builder module>.takes.yaml` to `<workflow id>.takes.yaml`, in the folder of the module that constructed the `Workflow` ([protocol.md](../../spec/protocol.md) §10);
- the direct provider calls outside a workflow went: a game's voice preparation is an FX workflow with one `speech.generate` step per line; another game's live design command was dead, since its pipeline already ran that loop as workflow steps, and is deleted; and the concept tool is now a skill that users install, which draws with a one-step FX workflow on the user's own key;
- FX gained what stage-gen's tests and layout need, each a public feature that makes sense without it: **stand-in answers** (`grida-fx run --stand-in`, `grida.fx.run(…, stand_in=…)`), which answer a run's paid calls offline with a test's function, held to a provider's checks and billed at nothing ([protocol.md](../../spec/protocol.md) §5.7); **workflow search folders** (`workflows:` in `fx.yaml`), so a project finds by id the workflows kept in nested projects ([store.md](../../spec/store.md)); and **`RunResult.failures`**, each failed or skipped instance with its message and error code.

**Where gnode's names went.** Leaving out gnode's own tests, stage-gen imported 159 names from gnode, and `grida.fx` had 17 of them. The other 142, imported in 116 files, had no FX equivalent:

| Names | Where they went |
|---|---|
| Plain utilities: contract models, atomic writes and path confinement, media signature checks and inspection, provenance sidecars | Kept, and moved into stage-gen: `stage_gen.contracts`, `stage_gen.fs`, `stage_gen.media`, `stage_gen.provenance` |
| Provider services, requests, backends, retries and cancellation (about 60 names) | Deleted. FX's adapters and its engine, the one retry owner, do that work. |
| The route-policy layer and stage-gen's gnode plugin: a model policy snapshot, re-checks of a chosen route, model overrides from the environment | Deleted. FX's built-in route table and each project's own routes (in `fx.yaml`, or a routes document) choose the route, and the planner refuses a route that lacks a required feature. |
| Run records, run views and the dashboard (`gnode view`) | Moved into stage-gen, which projects FX run folders into its own run-view contract; `scripts/view.py` replaces `gnode view`. |
| The engine test harness: injected services, and stand-ins for runs and call records | Replaced by stand-in answers. |
| The catalog, graph contracts and documented commands (`describe`, planning internals, the command-line parser) | stage-gen helpers over `grida.fx.plan(…).document`, and `python -m grida.fx --help`. |

gnode's tests, conformance suite, guide and schemas left with it; the suite and the guide had moved here in milestone 1. stage-gen's live provider tests went too, since they tested the Python adapters FX replaces.

**The paid cache is not migrated** (decision 12). The plan was a replay: run each recorded workflow against gnode's cache, compute each call's old key with gnode and its new key with FX, and copy each hit under the new key at $0. The owner decided against it, and FX started with an empty cache:

- gnode's call records keep no request, so a replay needed gnode running beside FX, and gnode could go only after it. Without one, the switch was a single step, with no period of running both engines.
- What a replay could keep was small: about $20–30 of game image calls, billed again when those games are next rebuilt, and that rebuild draws new art anyway.
- The largest block, $108 of one workflow's spike calls, could not move at all: they begin with agent turns, whose keys change shape, and every later call depends on them.
- gnode's runs stay in stage-gen's `out/`, and its run viewer no longer lists them.

**Smoke runs:** one budgeted live run per provider, each with the owner's go and a cap, took place on 2026-10-07:

| Provider | Capabilities checked |
|---|---|
| OpenAI | `image.edit` |
| OpenRouter | `structured.generate`, `image.generate`, `agent.turn`, `music.generate` |
| fal | `video.generate` |
| Tripo | `mesh.generate`, `mesh.rig` |
| ElevenLabs | `sound.generate`, `speech.generate` |

Milestone 2 has passed. The published preview `v0.1.0-alpha.2` has also passed its checks as installed, satisfying the publication gate for leaving preview. The stable release `v0.1.0` is published on npm and PyPI; npm stable releases use `latest`, while pre-releases use `next`.

## Later

- The workflow registry.
- The TypeScript node host for `@grida/fx`.
- GG as a provider.
- The `grida fx` umbrella.
- Views: a [local workflow-plan and run viewer](../guide/07-viewing.md), with a
  read-only node canvas, is implemented;
  [project browsing, custom views, snapshots, and lifecycle decisions](../../TODO.md#standalone-workflow-and-run-viewer) remain later work.
- Standard-library node bodies in Rust, with a planned rekey (decision 8). This also makes their outputs the same bytes on every platform. Today Pillow's Linux x86-64 build writes other PNG bytes than its macOS arm64 build for some pictures, so a cache filled on one platform misses on the other for every paid call downstream of a std picture output.

## Identity v3

A read-only audit of gnode found that its identity encoded how Python happens to behave. Paths below were in stage-gen's gnode (last at `bb784832`). The normative definitions are [`spec/identity.md`](../../spec/identity.md) and [`spec/yaml.md`](../../spec/yaml.md).

| gnode | v3 |
|---|---|
| Canonical JSON relies on Python's number formatting (`values.py:24-31`). It writes `NaN` as invalid JSON, and treats `1` and `1.0` as different values. Expressions collapse whole floats to integers and print numbers into prompts with `str()` (`expr.py:660-682`). | [RFC 8785 (JCS)](https://www.rfc-editor.org/rfc/rfc8785) with I-JSON numbers: no NaN or infinity, integers within ±2^53, and `1` = `1.0`. Implementations exist for Rust, Python and JS. |
| PyYAML parses YAML 1.1: `on` and `yes` become true, `017` becomes 15, dates become date objects, and the last of two duplicate keys silently wins. | A strict YAML 1.2 core subset. Duplicate keys, timestamps, collections as keys and ambiguous scalars are errors; plain keys are always strings. |
| The graph digest hashes a pydantic dump (`run.py:126-137`). | The plan digest hashes the authored workflow documents, the inputs, the takes, the type identities and the route fingerprints (`spec/identity.md` §10). |
| A step's identity includes upstream file digests only (`expand.py:1112-1123`). | Kept: content addressing is already a Merkle structure over content, and it keeps everything downstream cached when an upstream rerun writes the same bytes. (The first draft proposed chaining upstream identities instead; see `spec/identity.md` §13.) |
| File facts come from PIL, and from ffprobe if it happens to be installed, and they feed identity (`gnode_std/facts.py`). | The engine computes facts. Each one is specified and pinned by media fixtures. |
| Call records keep no request (`store.py:240-250`). | Records keep the canonical request, so a future rekey can be computed. Requests never carry secrets: keys travel in the transport. |

## Node protocol v2

gnode's v1 schema existed, but nothing serialized to it: a body's `ctx` held live Python objects (`host.py:339-367`). v2 is the out-of-process version, specified in [`spec/protocol.md`](../../spec/protocol.md). In outline:

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

gnode's capability layer sent a paid request again up to 6 times and settled only the last attempt's cost, so earlier attempts that billed were never counted (`retry.py:36`, `host.py:934-939`). In FX:

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

## Verification, every step

- **This repo:**
  - `cargo fmt`, `clippy` and `test`;
  - the conformance suite against `grida-fx`;
  - the comparison against gnode (milestone 1, steps 2–3; its tool left with gnode);
  - adapter fixtures (step 4);
  - an installed-package check per platform (step 5).
- **stage-gen (milestone 2):** `uv run python scripts/check.py`, its `VERIFICATION.md` gates and the clean-worktree pre-push hook. Every workflow and game plans on FX, at gnode's prices until the image routes were priced by size, and runs whole with stand-ins.
- **Spending:** none in milestone 1. Paid runs in milestone 2 each need the owner's go and a cap.
