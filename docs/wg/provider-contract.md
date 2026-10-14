# AI provider contract

Status: ratified by the owner, 2026-10-14. It is built in one pull request that closes
[gridaco/fx#69](https://github.com/gridaco/fx/issues/69) and covers
[gridaco/fx#79](https://github.com/gridaco/fx/issues/79) as far as Grida Models' schema allows.
Once it lands, [`spec/`](../../spec/) holds the contracts; this page keeps the decisions and
the record of the plan.

## Why

- New models and providers arrive faster than FX can ship adapters, so FX will always be
  behind. A user must be able to run any model offered by a provider FX already speaks
  (GG, OpenRouter, fal), and to add a provider FX does not know, without waiting for an FX
  release.
- FX must stay useful on its own, with its engine and its contracts, if its maintainers are away
  for weeks or months.
- FX is a general workflow engine with first-class AI support. The contract covers AI
  capabilities only. It is not a generic meter for paid HTTP calls: a user who needs one writes
  a node and meters it.
- The fix is the fundamental one: FX's own providers use the same contract that users build on.

## Decisions

Ratified 2026-10-14.

| | Decision |
|---|---|
| R1 | **Capabilities stay FX's.** The engine checks every request against its capability and the route's contract before any adapter sees it, and runs the capability's every-route checks on every answer. An adapter maps requests and answers, and adds only its route's own judgements. |
| R2 | **One adapter contract, two bindings.** FX's own adapters stay in Rust and in process, so AI calls need neither Python nor Node. A project's adapters run out of process over the node protocol, in Python or TypeScript. The contract (outcomes, long jobs, costs, reasons, handles, redaction) is its own spec; each FX adapter's wire details move to a reference kept beside its code. |
| R3 | **Network access belongs to the adapter.** A project adapter reaches the network however it likes: its own HTTP client or a vendor's SDK. The contract asks only for what FX's promises need: one send per attempt with retries off, an honest outcome (received or not, billed or not), and credentials named in its declaration, so the engine can redact them. The SDKs offer an optional `fetch` through the engine's transport, which adds the never-sent classification, response caps and offline replay. FX's own adapters keep the transport. |
| R4 | **Adapters are project code, like node modules.** They live in the project or in a package under `sources:`, are pinned by `fx.lock`, and are identified by a version or a source digest. |
| R5 | **Providers are declared.** A declaration names the provider's credential variables and how they are sent, its base-URL variable and default, and the hosts its results may be downloaded from. FX's providers become built-in declarations. Keys still come only from the environment or the project's `.env`, under the existing rules, now for the declared names. FX never writes credentials ([gridaco/fx#45](https://github.com/gridaco/fx/issues/45)). A declared provider bills dollars; only FX's own runtimes bill a plan. |
| R6 | **An OpenAI-compatible family by declaration alone**, for the chat capabilities (`structured.generate`, `agent.turn`). |
| R7 | **Routes.** `extends` lets a route take another route's price, features, limits and contract and restate only what differs. Namespaced `provider_options`, like the Vercel AI SDK's `providerOptions`, pass parameters FX does not expose; only the matching adapter reads them, and they are part of identity. The engine always supplies the serving adapter's identity to the route fingerprint: no built-in key moves, and a project route declared without a contract recomputes once. |
| R8 | **A route gives a price or a per-call cap.** A capped route is unpriced: each call holds the cap and is charged what the provider reports, else the whole cap. A route with neither is refused, and an empty price no longer means $0. |
| R9 | **Any model on a provider FX speaks.** A new model id, with `extends`, `provider_options` and a cap where needed, is enough to run a model FX does not list on GG, OpenRouter and fal. |
| R10 | **Grida Models is used when FX is built, never when it runs.** FX pins a released snapshot of [Grida Models](https://github.com/gridaco/models), keeps a generated projection of the facts Models' JSON Schema defines, and CI checks the built-in route table against it. A route's effective features are the provider's documented facts that its adapter implements; an unknown fact never enables one. A project-level pin is a follow-up. |
| R11 | **GG's planning prices are FX's allowances, and say so.** GG's image and video routes keep allowances taken from the same model elsewhere until GG's own rates can be planned from. A GG call is booked at GG's settled charge. |
| R12 | **Request fields become provider-neutral later,** in the one identity break before 1.0, with the naming review, so users migrate once. `provider_options` lands now, since it only adds a member. |
| R13 | **One definition per capability is deferred.** A capability is consolidated when it next changes. |
| R14 | **No model aliases.** The capability default in `fx.yaml` is already a one-line bump. |
| R15 | **Nothing is spent.** Everything is proven over replay fixtures, stand-ins and synthetic adapters. |

**Amendments to the [overview](overview.md)'s decisions:**

| Decision | Amendment |
|---|---|
| 1 | FX owns the adapter contract and its own adapters; users may add providers and adapters. "FX depends on no Grida package" gains: "It may pin Grida Models' public data, which it never needs in order to run." The decision predates Models' first release (2026-10-11). |
| 4 | Adapter code maps requests and answers; the checks stay in the engine (R1). |
| 9 | Scoped to FX's own adapters. The canonical request is what an adapter maps to the wire, not the wire body itself. |

## What users can extend

| Job | Needs |
|---|---|
| A model FX does not list, on a provider it speaks | a route: `extends` and the new model id, with a cap when it has no price |
| A parameter FX does not expose | `provider_options` |
| A provider that speaks the OpenAI chat wire | a declaration |
| Any other provider | a declaration and an adapter in Python or TypeScript |

## Working rules

- **One draft PR,** from the branch `provider-contract`. The owner reviews and merges it.
  Commits stay local and are pushed once, when the plan is done or the session must stop; the
  draft PR opens then.
- **Commits follow the milestones in order.** Each commit builds and passes the checks for what
  it touches, and each milestone ends with its gate green. A milestone ticks its boxes here in
  its last commit.
- **Spec first.** A milestone that changes a contract changes `spec/` before the code.
- **Users' surfaces move together.** Every user-visible change updates `llms.txt` in the same
  milestone, and public changes move `AGENT_READINESS.md` and both SDKs with them (AGENTS.md).
- **Never stop to ask.** A question this page does not answer takes the default under
  [Defaults](#defaults); anything left out becomes a follow-up issue, and the work goes on.
- **Code facts win.** Where this page and the code disagree on a fact, the page is corrected in
  the milestone that finds it.
- **No release.** The PR bumps no version; a release follows RELEASING.md separately.

## Milestones

### M0. Ratify

- [x] This page.
- [x] The overview: amend decisions 1, 4 and 9; add decision 15, this contract, ratified
  2026-10-14; drop "GG as a provider" and "The `grida fx` umbrella" from Later, since both shipped.
- [x] `AGENT_READINESS.md`: scenario AR-20, "Use a model or provider FX does not ship",
  Proposed.

Gate: the docs' relative links resolve; `check_spec`.

### M1. The adapter contract, split out (no behaviour change)

- [x] `spec/adapters.md`: the adapter contract, version 1, made from `providers.md`'s
  introduction and §1, §2 and §4 to §8, unchanged in substance.
- [x] `spec/providers.md`: providers, keys and endpoints; the built-in adapters as a normative
  table (provider, capability, contract `adapter`, `adapter_behavior` as exact JSON, reference);
  the built-in route table; renamed features. fal and Tripo get their behaviour named, and
  `background.remove` on fal gets one.
- [x] Each FX adapter's wire details move from §9 to
  `crates/grida-fx-providers/src/<provider>/README.md`, with its contract names and behaviour.
- [x] Every reference follows: spec cross-references, the guide's anchors (`#96-codex`,
  `#97-grida-gateway`, `#98-grok-build`, `#10-the-built-in-route-table`), `llms.txt`, Rust
  comments, and capabilities.md's pointer to the route's value checks (old §9).
- [x] The gates follow the text: `check_spec` (j), (k) and its forbidden-name roots;
  `tests/routes.rs`'s sentence and contract checks; `bind.rs`'s renamed-features anchor;
  `tests/capabilities.rs`. A new check: every "`<spec file>` §N" reference names a heading
  that exists.

Gate: `check_spec`, `cargo test`, `check_llms`.

### M2. The engine owns the checks (no behaviour change)

- [x] Spec: the engine checks the route's contract and the request's fit before dispatch, and
  runs the every-route checks after the adapter's own (`adapters.md` §3.4 and §4,
  `capabilities.md` §1, `protocol.md` §6.1 step 7). It checks inside the attempt, where an
  adapter's refusal comes (after the slot, the hold and a long job's `submitting` record), so
  events and records are unchanged, and never before a collect.
- [x] Runtime: each FX adapter names its contract, an `Identity`: `adapter`, and `adapter_behavior`
  as canonical JSON (none for fal and Tripo, which checked only `adapter` until M3). Around an
  adapter that names one, the engine makes the contract check, then the adapter's own judgement
  of its other contract members (`check_contract`: GG's and OpenRouter chat's), then
  `check_request`, before each send or submit; and `checks::every_route_with`, with the
  adapter's redactor, after the adapter's check of each answer. A test double that names none
  gets neither.
  - Adapters keep their guards for direct callers, since their tests call them directly, but each
    guard is built on the engine's functions (`checks::serves`, `checks::contract`), so there is
    one implementation. Called directly, fal and Tripo now refuse a contract that is not an
    object, and GG, OpenRouter chat and ElevenLabs accept a behaviour written `1.0`, as OpenRouter
    music did: the engine hands an adapter neither.
  - Stand-ins are unchanged: the request check only, and the adapter's check or the every-route
    check, never both (`protocol.md` §6.1, *Stand-in answers*).
- [x] Refusal sentences and their order are unchanged; every adapter test passes as written but
  one. The adapters' `ADAPTER_BEHAVIOR` and `CONTRACT_BEHAVIOR` constants become their
  `IDENTITY`, so `tests/routes.rs`'s contract test, which asserted five of them, now holds each
  identity that names a behaviour to the table. `tests/engine_checks.rs` holds each adapter's
  identity to `providers.md` §2, and the engine's refusals to each adapter's own, over every
  built-in contract and the members each judges.

Gate: `cargo test`, conformance.

### M3. Route identity and `extends`

- [x] Spec (`identity.md` §7): where a route's contract leaves out `adapter` or
  `adapter_behavior`, the engine supplies them from the built-in adapter table, for a
  capability and provider that has an adapter. Routes on pairs without one are untouched.
  `extends` is resolved before the fingerprint and is not part of it.
  - The fill is keyed by capability and provider, as the registry is, not by provider alone:
    `vision.review` on `gg` is untouched. A member set to `null` counts as left out. The
    behaviour is filled only when the contract's `adapter` is the table's, typed exactly as
    `providers.md` §2 writes it (a string for images, such as `"1"`, `"2"` for Codex and `"3"`
    for OpenRouter; the number `1` otherwise).
  - `kind` stays `fx-route-v1`, the one exception to the rule that an identity change bumps it
    (`identity.md`, `spec/README.md`): no built-in fingerprint moves. The migration note: a
    project route on a pair FX serves whose contract left out `adapter`, or named FX's adapter
    and left out `adapter_behavior`, recomputes once.
  - `extends` names a route of the same capability and provider: an earlier entry of the same
    table, else the entry in force in the catalog so far (which wins over a built-in one of the
    same route), else the built-in table's, even under `--routes`. The entry copies the base's
    price, features, `max_input_pictures`, `concurrency`, `requests_per_minute` and contract; a
    member it states replaces the base's whole, except `contract`, whose members merge one level
    deep. A missing base, a base of another capability, and one on another provider are refused
    while the table is read (`routes.<n>.extends: no route <base> serves <capability>`;
    `… <base> is not a <provider> route`), in `llms.txt`'s errors table.
- [x] `fx-routes-v1`: `extends`; `price` is not required beside it. It is an `if`/`then`, so a
  missing price is refused as before.
- [x] Core: a static (capability, provider) → identity table, promoted from `tests/routes.rs`;
  filling in `parse_route`; resolving `extends` over the bases `load_catalog` passes.
  - The table is `routes::BUILT_IN_ADAPTERS`, promoted from the test's table with its behaviours
    unchanged; fal's and Tripo's adapters, whose `Identity` named none, now name the ones
    `providers.md` §2 gives (fal images `"1"`, `fal-queue`, `fal-run-birefnet` and Tripo's `1`).
  - The fill is in `parse_route`, not `load_catalog`, since every reader reaches it
    (`RouteTable::from_document` reads the built-in table and bypasses `load_catalog`);
    `load_catalog` hands each table its bases, the catalog so far and then the built-in table
    (`read_table`, then `parse_table`; `RouteTable::from_document_over` does the same for a
    document already loaded). `tests/routes_spec.rs` proves both bases through `load_catalog`.
  - Core cannot see the providers crate, so a test there holds the table to every registered
    adapter's `Identity`, behaviour included, and to `providers.md` §2, in its order.
  - fal's and Tripo's adapters now name their behaviour in their `Identity`, so the engine
    refuses a route whose contract names another `adapter_behavior`, as for every other adapter.
    This is a deliberate behaviour change, for a project route that names another behaviour on
    fal or Tripo only; `llms.txt` and the two references say so. `Identity::behavior` is no
    longer optional. A fal video job already submitted is still collected whatever behaviour its
    route names: its collect checks the capability and `adapter` alone, as before, since the
    call's key fixes the contract (`adapters.md` §4), so a job submitted under 0.6.1 is not
    stranded.
- [x] A test pins all 29 built-in fingerprints before the change and proves that none moves.
- [x] `tools/digest.py` fills and resolves exactly as the engine does; vectors for a filled
  contract and an extended route.
  - digest.py keeps its own copy of the adapter table, which `check_spec` (j) holds to
    `providers.md` §2, and always takes the built-in table as a base: `check_spec` (h) passes
    `--builtin-routes` with `--routes` too.
  - The vectors are a new input form, `spec/vectors/identity/routes.json`
    (`fx-route-vectors-v1`): tables in, completed contracts and fingerprints out, and refusals
    by reason, with made-up models, on FX's own providers where a case needs an adapter of FX's
    and on `acme` otherwise. A hashed object alone cannot show a fill.
  - A conformance case, `route-extends`, plans extended routes through the command line, and
    `check_spec` (h) checks its fingerprints with digest.py. No expected file moved.
- [x] Drop `jpeg_output`, `webp_output` and `hosted_url_reference_input` from the vocabulary
  (capabilities.md, `features.json`, `capabilities.rs`) and from the routes. A step that
  requires one is refused as no feature of its capability, with no hint; `llms.txt`'s upgrade
  note says so.
- [x] `llms.txt`; guide pages 05 and 13.

Gate: `cargo test`, conformance with no expected file moved, `check_spec`, `pytest`.

### M4. Capped routes

- [ ] Spec: `price: {cap_usd: N}` stands alone; `price: {}` is refused; the hold is the cap
  (`protocol.md` §6.1); costs (`adapters.md`); decimals (`identity.md` §12).
- [ ] Schemas: `fx-routes-v1`; in `fx-graph-v1`, an instance's `price` gains an optional
  `capped: true`.
- [ ] Core: the route price's flag, the refusals in `parse_price`, and a plan note per capped
  route.
- [ ] Browser and SDKs: `web/fx-web` accepts `capped`; the viewer shows "up to $N"; the
  TypeScript document types.
- [ ] Tests: the parser, the plan's text and graph, and settlement with and without a reported
  cost. A conformance case plans a capped route.
- [ ] `llms.txt`; guide page 04.

Gate: `cargo test`, conformance, the viewer's checks, the SDKs' tests.

### M5. `provider_options`

- [ ] Spec: every paid capability takes an optional object `provider_options`, keyed by
  provider. An adapter reads only its route's provider's member; members FX sets itself are
  refused; it never holds a credential, since requests are recorded verbatim. A route's
  `contract.provider_options` is merged under the step's.
- [ ] Each paid built-in gets the optional param, with no default, so no key moves.
- [ ] Adapters merge their member into the body: OpenAI, OpenRouter, fal, GG, Tripo and
  ElevenLabs. Codex and Grok Build refuse a non-empty one.
- [ ] Replay tests per adapter, for the pass-through and for the refusal of members FX sets.
- [ ] `examples/movie-sprite`: its prompt-expansion setting goes through `provider_options`;
  README and pasted plan output updated.
- [ ] `llms.txt`; guide pages 03 and 13.

Gate: `cargo test`, conformance, `check_examples`, `check_llms`.

### M6. Any model on GG, OpenRouter and fal

- [ ] The registry keys adapters by capability, provider and contract `adapter`. A route
  without one takes its provider's default for the capability.
- [ ] OpenRouter images: a generic adapter, the default for routes FX does not ship, sends what
  the request and `provider_options` say, with no forced quality, moderation or size list. An
  answer that is not a PNG becomes one, as Grok Build's do. The built-in routes keep
  `fx-openrouter-image-v1` unchanged.
- [ ] fal images: the same, with fal's common request and answer shapes taken from the fal
  endpoint parameters that Models records. The GPT Image envelope stays with the GPT Image
  adapter.
- [ ] fal video (`fal-queue`): durations, resolutions and aspect ratios come from the route (its
  price units and tiers), not constants; the first frame is sent only when given. No byte it
  sends today changes.
- [ ] GG images: the picture limit comes from the route; the built-in GG image routes declare
  `max_input_pictures: 16`.
- [ ] A replay test per generic adapter with a made-up model; `doctor` and `plan` report the
  route as servable.
- [ ] `llms.txt`; guide page 13.

Gate: `cargo test`, conformance, `check_llms`.

### M7. Grida Models facts and OpenRouter transparency

- [ ] `tools/make_models.py --release <tag>`, run by hand with the network:
  - downloads `models.v1.json` and verifies its SHA-256;
  - validates it against that release's schema;
  - keeps only the schema-defined members that FX's routes use;
  - writes `crates/grida-fx-providers/routes/models.json` with a `source` block (repository,
    release, asset, bytes, SHA-256, schema `$id`).
  `--check` verifies it offline. The full release is never committed.
- [ ] Pinned to `v1.20261013.6`.
- [ ] `crates/grida-fx-providers/tests/models.rs` checks every route's public facts against the
  projection:
  - wire ids and endpoint ids;
  - features from documented parameters, intersected with adapter support, both ways: a
    documented and supported feature must be declared;
  - picture limits and size envelopes;
  - GG's 3D endpoints;
  - text token prices as bounds, never equality.

  Each exception sits in one table with its reason, and an exception no longer needed fails.
- [ ] OpenRouter GPT Image: `alpha` on both routes; the adapter sends `background:
  transparent`; replay tests for the body and the answer's check. `adapter_behavior` stays
  `"3"`, since no body it sent before changes.
- [ ] `providers.md`: GG's prices are said to be FX's allowances; GG's image picture limit.
- [ ] Drift: `llms.txt` and guide page 05 on masks, since fal's edit route takes one.
- [ ] CI: `make_models.py --check` in the `spec` job; AGENTS.md's verification list.

Gate: `cargo test`, `check_spec`, `make_models.py --check`, `check_llms`.

### M8. Provider declarations

- [ ] Spec (`providers.md`): a declaration names the provider (the route grammar, not a built-in
  name), its credential (variable, header, scheme), its base URL (variable and default; https
  unless loopback), its download hosts, and its family or adapter. Refused: a built-in
  provider's name; a variable a built-in or another declaration claims; `GRIDA_FX_*`, proxy and
  certificate variables.
- [ ] `fx-project-v1`: `providers:`.
- [ ] Built-in declarations replace the fixed key, variable, endpoint and provider lists.
  - Credential headers and prefixes are owned values.
  - The `.env` loader reads the declared names, and the redactor learns their values.
  - The environment scrubs (Codex and Grok Build children, the service, the browser opener) use
    the computed list.
  - Codex and Grok Build stay runtimes.
- [ ] One download-host allowlist helper, used by fal and Tripo.
- [ ] `doctor` lists declared providers with their key state and base-URL host.
- [ ] The tools' and tests' key lists read the engine's list, or stay in step with it.
- [ ] A conformance case declares `acme`: plan, `doctor`, and the refused declarations.
- [ ] `llms.txt` (it says "only these eleven variables"); guide pages 05 and 13.

Gate: the whole verification list.

### M9. OpenAI-compatible providers

- [ ] Spec: the `openai-compatible` family serves `structured.generate` and `agent.turn` over
  `{base}/chat/completions`.
  - Errors map through the default status table, never `Some(0)`.
  - Cost is `None`, so the hold or the cap is charged.
  - Contract adapters are `openai-compatible-structured` and `openai-compatible-tool-loop`,
    behaviour `1`.
- [ ] Registered from declarations, on the shared chat module.
- [ ] A replay test: a made-up declared `acme` answers both capabilities.
- [ ] An `llms.txt` recipe: a declared provider, planned offline.

Gate: `cargo test`, `check_llms`.

### M10. Project adapters in Python and TypeScript

- [ ] Spec: the out-of-process binding (`adapters.md`); in `protocol.md`, the methods
  `adapter.load`, `adapter.send`, `adapter.submit` and `adapter.collect`, and the host-to-engine
  `fetch`, added within protocol v1; their schemas.
- [ ] A declaration's `adapter: <path>#<attribute>`; `fx.lock` `adapters`. Identity: the contract
  `adapter` is the declared adapter, and `adapter_behavior` is its version, or `source:<digest>`.
- [ ] Engine:
  - an adapter host started lazily in a live run, outside the pools, and shut down with the run;
  - an RPC adapter for send, submit and collect;
  - a host lost mid-send is `Failed { retryable }`, mid-submit `Uncertain`, mid-collect
    `Unreachable`;
  - handles must be objects;
  - cancellation at the invocation;
  - dollars only;
  - declared credentials passed into the host's environment.
- [ ] `fetch`: the engine's transport, with the credential attached by the engine, the network
  switch, caps and deadlines.
- [ ] SDKs: an adapter authoring API in `grida.fx` and `@grida/fx`, with outcome types and
  concurrent calls; tests; READMEs; `protocol.md` §8.
- [ ] Conformance: `adapter-py` and `adapter-ts`, each a synthetic `acme` adapter run live with
  the network off: an answer, a refusal and a long job.
- [ ] `llms.txt`; guide pages 03 and 13.

Gate: the whole verification list, `check_host` included.

### M11. Docs, readiness and the PR

- [ ] Guide page 13, "Use a model or provider FX does not ship"; caps in page 04; routes in
  page 05.
- [ ] `llms.txt`: recipes for a new model and a declared provider; the errors table.
- [ ] `skills/grida-fx` and its onboarding reference.
- [ ] `AGENT_READINESS.md`: AR-20 Current, with its evidence.
- [ ] The follow-up issues filed, each linked from this page.
- [ ] The whole verification list; push; open the draft PR.

## Defaults

| Question | Default |
|---|---|
| Where a project declares providers | `providers:` in `fx.yaml` |
| A cap's form | `price: {cap_usd: N}`, alone in `price` |
| What `extends` may name | a route of the same capability and provider, from the built-in table (even under `--routes`) or an entry already read, which wins. It takes a snapshot and is never hashed |
| Route-level options | `contract.provider_options`, already in the fingerprint |
| Step-level options | an optional `provider_options` param on each paid built-in, without a version bump. If `identity.md` requires a bump, route-level options only, and a follow-up |
| The adapter of a route without a contract | its provider's default for the capability, from the built-in table (the generic one after M6) |
| The spec's layout | `spec/adapters.md` and `spec/providers.md`, renumbered, with every reference updated; references in `crates/grida-fx-providers/src/<provider>/README.md` |
| `adapter_behavior` bumps | none where no byte an adapter sent before changes |
| A generic adapter's wire | only what published facts support (Models' endpoint parameters, the provider's own published schema). Otherwise ship the selection and `provider_options` only, and file a follow-up |
| An answer that is not a PNG | converted to PNG in the adapter, as Grok Build's are |
| A declared provider's billing | dollars only; no declaration can claim a plan |
| A declared provider's errors | the default status table; never `Some(0)` |
| A declared provider's cost | none read; the hold or the cap is charged |
| New protocol methods | added within protocol v1; hosts answer unknown methods with -32601 |
| Project adapters in conformance | run live with the network off; the synthetic adapter makes no request |
| The Models file | one generated projection; the full release is never committed |
| Prices against Models | bounds, never equality |
| A test that resists | fix the code, not the test, unless the test pins wording this plan changes |

## Follow-ups

Filed as issues in M11, or as soon as a milestone leaves something out, with only verified facts.

**gridaco/fx:**
- a project-level Models pin and model import (#69's P5);
- the media routes' facts from Models, once its media prices are in its schema;
- the identity break before 1.0 (R12);
- OpenAI-compatible images;
- a generic adapter for OpenRouter music;
- pacing shared across a provider's routes: an extended GG image route gets its own slot;
- ElevenLabs speech's allowance for long texts: its high is $0.05 a call, and its answers
  report no cost;
- a live check that OpenRouter's transparent answers carry alpha, which needs the owner's go
  and a cap;
- Tripo's face limit: 25,000 in FX, 50,000 in Models.

**gridaco/models**, gaps found against `v1.20261013.6`:
- no card for Gemini Omni Flash;
- no endpoint facts on the OpenAI binding;
- no mask facts;
- GG's meters have no quantity per request;
- media prices sit outside the schema, and `token_pricing` cannot express image models;
- no ElevenLabs binding or US-dollar rate;
- `input_references` is typed as an integer on OpenRouter;
- no `size` on OpenRouter;
- `image_constraints` only on fal.
