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
| A model FX does not list, on a provider it speaks | a route: the new model id, with a cap when it has no price, and `extends` where a built-in route's adapter fits the model |
| A parameter FX does not expose | `provider_options` |
| A provider that speaks the OpenAI chat wire | a declaration |
| Any other provider | a declaration and an adapter in Python or TypeScript |

## Working rules

- **Local work, one branch.** Everything lands as commits on the branch `provider-contract` and
  stays local: nothing is pushed and no pull request or issue is opened until the owner asks
  (2026-10-14). The owner reviews and merges the pull request.
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

- [x] Spec: `price: {cap_usd: N}` stands alone; a price that states no amount is refused; the
  hold is the cap (`protocol.md` §6.1); costs (`adapters.md` §5); decimals (`identity.md` §12).
  - Not only `price: {}`: any price without `usd`, `low_usd`, `high_usd` or `cap_usd` was $0
    (`{unit: second, max_units: 10}` too), and is refused. An explicit $0 (`usd: 0`, or the Codex
    and Grok Build routes' `low_usd: 0, high_usd: 0`) stays valid, and `high_usd` alone still
    means a low of 0.
  - `cap_usd` is more than 0 (`usd: 0` already means free). A capped call's low is $0 and its
    high, the hold, is the cap. A reported cost above the cap is charged in full (R8: charged
    what the provider reports), so the cap is the hold, not a limit the provider honours, and a
    run can end above its ceiling by the difference, as on a priced route.
  - Under `extends`, a restated `price` replaces the base's whole (M3's rule), so a cap over a
    tiered base keeps no tier, and an entry that states no price inherits its base's cap.
- [x] Schemas: `fx-routes-v1`; in `fx-graph-v1`, an instance's `price` gains an optional
  `capped: true`.
  - Both refusals are described in the schema and enforced by the parser, like the range rule.
    The schema also checks that a cap stands alone, with an `if`/`then` (`maxProperties: 1`),
    not an `anyOf`, whose refusal the engine's messages would collapse; so the engine prints
    `routes.<n>.price: {…} has more than 1 property` for it, and the parser's own sentence for a
    price that states no amount.
  - An instance is `capped: true` when any of its calls is capped: one instance may call several
    capabilities on several routes, so its `low_usd` and `high_usd` may mix prices and caps.
    `capped` is written only when true, so no expected graph moved.
- [x] Core: the route price's flag, the refusals in `parse_price`, and a plan note per capped
  route.
  - `RoutePrice::capped` and `CallPrice::capped`, and `Instance::capped()`. The runtime needed
    no change: the hold is the route's high (`route.cost`), and settlement charges the reported
    cost, else the hold.
  - The note is one per capability and capped route of the live instances, sorted, after the
    takes notes: `<capability> on <route> has no price: each call holds its cap, $N, and is
    charged what the provider reports, else the cap`. Notes are in the plan's text only; `plan
    --json` carries only `capped`. They are `Plan::capped_notes`, apart from `Plan::warnings`,
    since a stand-in run's plan text leaves them out: it holds and charges nothing, as it already
    leaves out the ceiling warning.
  - A capped route on Codex or Grok Build is no longer priced at $0, so a live run on it needs
    a ceiling, as for any route that is not free.
- [x] Browser and SDKs: `web/fx-web` accepts `capped`; the viewer shows "up to $N"; the
  TypeScript document types.
  - The label is `priceLabel` in `web/fx-web`, with its tests, so the viewer's React code only
    calls it. The Python SDK hands the graph back as plain dicts and needs no change.
- [x] Tests: the parser, the plan's text and graph, and settlement with and without a reported
  cost. A conformance case plans a capped route.
  - Settlement is tested with a reported cost under the cap, one above it, none, and a ceiling
    below the cap (`tests/calls.rs`); the case is `capped-route`, with the refusals.
- [x] `llms.txt`; guide page 04.
  - Guide page 05 says a stand-in run notes no cap. The README's "a live run never crosses its
    ceiling" now says a live run reserves each call against its ceiling, since a call charged
    more than it held is charged in full. `llms.txt`'s summary ("never spends past a ceiling"),
    guide page 04's opening ("never spends money you didn't plan for") and page 02's "no paid
    call is made that could cross it" moved with it, and so did the prose of the
    `character-3d` and `concept-gallery` examples' READMEs ("stops before crossing it"), but
    not their pasted plan output. Page 04's cap example gives a made-up model id that extends
    OpenRouter's built-in image route, the job a cap is for.
    `conformance/README.md` lists `capped-route`.

Gate: `cargo test`, conformance, the viewer's checks, the SDKs' tests.

### M5. `provider_options`

- [x] Spec: every paid capability takes an optional object `provider_options`, keyed by
  provider. An adapter reads only its route's provider's member; members FX sets itself are
  refused; it never holds a credential, since requests are recorded verbatim. A route's
  `contract.provider_options` is merged under the step's.
  - `capabilities.md` §1 and every member table (the row after the last param, before the input
    ports; last for `agent.turn`); a new `adapters.md` §8, which §4 step 4 points to;
    `identity.md` §7; `providers.md` §2; `protocol.md` §6.2; `fx-routes-v1` describes
    `contract.provider_options` as an object of objects, so a bad shape is refused while the table
    is read (`routes.<n>.contract.provider_options.<p>: … is not of type "object"`).
  - `contract.provider_options` has the step's shape, an object by provider. The merge is
    shallow: the route's parameters first, a step's replacing one of the same name in place, the
    step's others after; all after FX's own members (`serde_json` keeps insertion order), so with
    none no byte changes and no `adapter_behavior` moves. Under `extends`, a stated
    `provider_options` replaces the base's whole, as every contract member does.
  - Each adapter owns a fixed list, stated in its reference and exported as its `OWNED`: the
    members it sets, whether or not a call sends them, those it never sends by design (OpenAI's
    never-sent members, the mask GG refuses), those that would change what it reads in the answer
    (`stream`, `response_format`, `n`, `sync_mode`), and those that restate a member it sets
    (`max_tokens` beside GG's `max_completion_tokens`); `adapters.md` §8 names the four kinds.
    Refusals, the first of step 4, the route's before the step's: `<where> is an object`,
    `<where>.<name> is a member FX owns`, and a file value anywhere in the member,
    `<where>.<name> holds a file: provider options are JSON only` (`<where> holds a file: …` when
    the member is one). They come before any byte leaves, Tripo's uploads and GG's rig check
    included. One helper, `wire::provider_options`, keyed by the route's provider. A file value
    is the engine's, `{"file": <digest>}` (`protocol.md` §3.2), so `{"file": "a.png"}` is data
    and is passed on.
  - Code facts: the request check holds `provider_options` to an object only, so a stand-in run,
    which runs no adapter, takes a provider member that is not an object. An adapter cannot keep a
    credential out of the records, since with-values are recorded before any adapter sees them,
    so the spec states the rule and FX does not look. GG's video wire already has a member
    `providerOptions`; it is FX's whole.
  - A project route whose contract already named `provider_options` now sends its provider's
    member, or is refused, under an unchanged fingerprint. Up to 0.6.1 the member meant nothing:
    GG's adapters and OpenRouter's chat adapters refused the contract (`the contract takes no
    member provider_options`), and OpenAI, OpenRouter images and music, fal, Tripo, ElevenLabs,
    Codex and Grok Build ignored it (`checks::serves` judges only `adapter` and
    `adapter_behavior`). No `adapter_behavior` is bumped, a departure from the Defaults row on
    bumps: the member had no meaning before, and a bump would move the fingerprint of every
    built-in route those adapters serve, since each states its behaviour. A call cached before
    replays without the options, as `identity.md` §7 and `llms.txt`'s upgrade note say.
- [x] Each paid built-in gets the optional param, with no default, so no key moves.
  - All 13 capability-body built-ins, the three without an adapter included, with no version
    bump: `identity.md` ties no param to a built-in's `version`. No identity, plan or key digest
    moved; the type listings did: 38 conformance expected files (`ports.params`, additions only)
    and two `cli.rs` pins.
- [x] Adapters merge their member into the body: OpenAI, OpenRouter, fal, GG, Tripo and
  ElevenLabs. Codex and Grok Build refuse a non-empty one.
  - Multipart bodies (OpenAI's and GG's edits) take them as text fields after FX's, before the
    pictures; GG's jobs in their paid `input` JSON, never the rig check's, which stays `{}`.
  - OpenRouter's structured and agent contracts, and every GG contract, now admit
    `provider_options`.
  - Codex and Grok Build refuse only their own member (`provider_options.codex`, `.grok`, the
    route's or the step's): `Codex takes no provider_options`. Other providers' members are not
    read, so a step may carry them for the routes it falls back to.
  - `agent.turn`: the engine's agent loop builds each turn's request, so an agent run's turns
    take only their route's; a body's own `agent.turn` call may carry its own.
- [x] Replay tests per adapter, for the pass-through and for the refusal of members FX sets.
  - Each adapter's test file pins the merged body (route and step options, another provider's
    member ignored): its exact bytes, or for OpenRouter's chat bodies its members' order and the
    options' values. It holds the members its bodies send to `OWNED`, and refuses each owned
    member over a transport that sends nothing. `wire.rs` unit tests cover the merge and the
    refusals; `routes_spec.rs` the route table's shape and `extends`.
- [x] `examples/movie-sprite`: its prompt-expansion setting goes through `provider_options`;
  README and pasted plan output updated.
  - Its route's contract carries `provider_options: {fal: {enable_prompt_expansion: false}}`.
    The name is fal's published input schema for `fal-ai/wan/v2.7/image-to-video` (a boolean,
    default true), checked 2026-10-14 and cited in `routes.yaml`; Grida Models
    `v1.20261013.6` records it only for Wan 3.0's fal endpoints. The route's fingerprint moves,
    as a project route's does when its contract changes; the pasted plan output does not.
- [x] `llms.txt`; guide pages 03 and 13.
  - Also `AGENT_READINESS.md` (AR-20, with the deferral of options for agent runs) and each
    adapter's reference.

Gate: `cargo test`, conformance, `check_examples`, `check_llms`. `check_examples` checks only
movie-sprite's links (self-contained); planning and running it are
`tools/check_showcase_examples.py`'s.

### M6. Any model on GG, OpenRouter and fal

- [x] The registry keys adapters by capability, provider and contract `adapter`. A route
  without one takes its provider's default for the capability.
  - `Adapters::register` makes an adapter its pair's default and names it by its `Identity`;
    `register_named` adds one only a route that names it gets. `serving` reads the contract's
    `adapter` as a string, else takes the default. A name no adapter of the pair has is served by
    none: `no_route` (`no adapter serves …`) and doctor's `no adapter`, so a contract naming an
    unknown adapter is no longer `capability_refused` (`adapters.md` §1, §4 step 1;
    `protocol.md` §6.1). The engine's contract check now guards `adapter_behavior` only in
    practice. `live::servable` takes the route, contract included.
  - `providers.md` §2 keeps one row per pair, its default, and gains a second table of the
    adapters a route must name: GPT Image's on OpenRouter and fal. Core's `BUILT_IN_ADAPTERS`
    gains a `default` flag (30 rows, in the spec's order). The fill (`identity.md` §7) gives a
    left-out `adapter` the default's name and a left-out `adapter_behavior` the behaviour of the
    pair's adapter the contract names, so `adapter: fx-openrouter-image-v1` alone still gets
    `"3"`. digest.py, `check_spec` (j) and two new route vectors follow. No built-in fingerprint
    moved (the 29-pin test); M3's vectors used no OpenRouter or fal image pair, so none was
    regenerated. A project OpenRouter image route whose contract states `adapter_behavior: "3"`
    without an `adapter`, which GPT Image served up to 0.6.1, now names the generic default and
    is refused when a call is sent; `providers.md` §2 and `llms.txt`'s upgrade note say so, and a
    vector pins its completion. On fal the one behaviour, `"1"`, is both adapters'.
  - A route's limits reach its adapter as `RouteRef::limits` (`RouteLimits`:
    `max_input_pictures`, `price.max_units` of a route priced per second, `price.by` with its
    tier names, and the features `image_input` and `text_to_video`), never identity. So an
    adapter holds a request to them before sending only, never when a job already sent is
    collected (`adapters.md` §4 step 4 and §6).
- [x] OpenRouter images: a generic adapter, the default for routes FX does not ship, sends what
  the request and `provider_options` say, with no forced quality, moderation or size list. An
  answer that is not a PNG becomes one, as Grok Build's do. The built-in routes keep
  `fx-openrouter-image-v1` unchanged.
  - Contract `openrouter-image`, behaviour `"1"`. The wire is what published facts support: Grida
    Models `v1.20261013.6` records request parameters only on OpenRouter's OpenAI bindings, and
    `references.max` on 13 of its 17 image bindings. So the body is `{model, prompt,
    input_references?}`, then the options. What it cannot send faithfully is refused before
    sending with a sentence naming `provider_options`: a mask, an exact size, `background:
    transparent`; `auto` and `opaque` are not sent (the image check holds an opaque request).
    It owns `model`, `prompt`, `input_references`, `n`, `response_format` and `stream`.
  - A generate's references are sent only on a route that declares `image_input`, else refused
    before sending with the plan's sentence (`<route> does not support image_input`), as
    `capabilities.md` §2 says: a node body's call and a step whose list only a run produces are
    judged only then. `RouteLimits` carries the feature, as it does `text_to_video`.
  - "Returned no single image" is `Failed { cost, retryable: false }`: FX asks for no count. The
    PNG conversion is `wire::to_png`, shared with Grok Build (its sentences unchanged), on a
    blocking thread.
- [x] fal images: the same, with fal's common request and answer shapes taken from the fal
  endpoint parameters that Models records. The GPT Image envelope stays with the GPT Image
  adapter.
  - Contract `fal-image`, behaviour `"1"`. It posts to `{run}/<model>` as given: text and edit
    endpoint ids differ, even in namespace, so nothing is appended. The body is `{prompt,
    image_urls?}`, then the options: Models records `prompt` on all 23 fal image endpoints and
    `image_urls` on all 11 that take pictures. It owns `prompt`, `image_urls`, `num_images` and
    `sync_mode`, and refuses references on a generate.
  - Code fact: Models records no answer shapes for fal (no fal endpoint has a
    `result_schema_url`). The answer shape, `images`, a list of files each with a `url`, is
    fal's own published schemas' (checked 2026-10-14, as was `num_images` defaulting to 1), and
    the answer is read as GPT Image's (`fal/output.rs`), then made a PNG.
- [x] fal video (`fal-queue`): durations, resolutions and aspect ratios come from the route (its
  price units and tiers), not constants; the first frame is sent only when given. No byte it
  sends today changes.
  - Code fact: a route table holds only the most seconds and the tiers. The route's resolutions
    are its price's tiers when it is priced `by: resolution` (else the adapter's four), and its
    most seconds its per-second `max_units`, rounded down (else 10). The minimum (3), whole
    seconds, the two aspect ratios, the defaults (`720p`, `9:16`) and the short sides stay
    `fal-queue` constants. The check judges a clip's size only where it knows the tier's short
    side.
  - A route draws from the prompt alone only when it declares a new `video.generate` feature,
    `text_to_video` (`capabilities.md` §6, `features.json`, `capabilities.rs`); any other refuses
    a request without a first frame at $0, as every route did (`this route draws from a first
    frame`), so the built-in image-to-video route still does. A last frame without a first frame
    is refused (`last_frame needs first_frame`, `capabilities.md` §6). Every request sent before
    had a first frame and a built-in tier, so none of its bytes changes.
  - The check of a collected clip reads the request with the defaults and none of the route's
    limits (`clip(request, None)`), since they are not identity: a job sent at a tier, a length or
    without a frame that its route's table no longer takes is still collected and checked
    (`adapters.md` §4 step 4 and §6; M3 kept a submitted job collectable for the same reason).
- [x] GG images: the picture limit comes from the route; the built-in GG image routes declare
  `max_input_pictures: 16`.
  - Code fact: only the `image.edit` route declares it, since `image.generate` on GG takes no
    references. A route that declares none leaves the count to GG.
- [x] A replay test per generic adapter with a made-up model; `doctor` and `plan` report the
  route as servable.
  - `tests/openrouter_generic_image.rs` and `tests/fal_generic_image.rs` (`acme/img-a@openrouter`,
    `acme/img-a@fal`, `acme/img-a/edit@fal`), a JPEG answer converted to PNG included; the
    stand-in and engine-check matrices cover every registered adapter, named ones included.
  - Code fact: `plan` says nothing of servability. A CLI test plans made-up models on OpenRouter,
    fal (images and video) and GG (exit 0), and doctor's `uses` lines say `servable`, and `no
    adapter` for a contract naming one FX does not have. A made-up model was already `servable`
    before M6; the unknown adapter is what is new.
- [x] `llms.txt`; guide page 13.
  - Also guide pages 03 and 05, `AGENT_READINESS.md` (AR-20), and the OpenRouter, fal and GG
    references.

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
- [ ] The follow-ups written up on this page, ready to file when the owner asks.
- [ ] The whole verification list, green on the branch.

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

Listed here as each milestone leaves something out, with only verified facts, and filed as issues
when the owner asks.

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
- Tripo's face limit: 25,000 in FX, 50,000 in Models;
- caps in a repeat whose list only a run produces: the graph's `pending` entry carries only
  `high_usd`, never `capped`, and the plan notes no capped route that only such a repeat binds,
  since `Plan::capped_notes` reads the expanded instances;
- caps beyond the instance: `price --json`, the graph's `estimate` and the viewer's run estimate
  carry no `capped`, so the viewer shows a capped plan's total as "est. $low – $high";
- a call charged more than it held, a cap or a priced route's high: it is charged in full and
  can take the run past its ceiling by the difference, and nothing in the run's output flags it;
  the plan's ceiling warning still ends "the run stops before crossing it";
- step-level `provider_options` for agent runs (`agent.run`, `protocol.md` §6.2, both SDKs'
  `ctx.agent`): the engine builds each of an agent run's `agent.turn` requests, so its turns take
  only their route's;
- a plan-time guard for `provider_options`: a credential (FX cannot tell one from a parameter,
  and with-values are recorded in `node_started.with` and the plan before any adapter sees them),
  and a member an adapter owns, which is refused only when a call is sent (core's
  `BUILT_IN_ADAPTERS` knows no adapter's owned members). The request check holds
  `provider_options` to an object only, so a stand-in run takes a provider member that is not an
  object;
- provider options inside members FX owns whole: OpenRouter's `provider` (routing) on structured
  output, agent turns and GPT Image (music and the generic image adapter send none, so their
  options may give one), GG video's `providerOptions` and ElevenLabs' `voice_settings` cannot be
  added to;
- route-declared video limits: `fx-routes-v1` has no member for a route's aspect ratios, minimum
  duration, default tier or a tier's size, so `fal-queue` keeps them as constants (`9:16` and
  `16:9`, 3 seconds, `720p` at `9:16`, short sides 360 to 2160) and its check does not judge
  the size of another tier. A capped route's price (`cap_usd` alone) has neither tiers nor
  `max_units`, so a capped fal video route, the case a cap is for, is held to the adapter's
  defaults (`360p`, `720p`, `1080p`, `4k`; 3 to 10 seconds);
- `text_to_video` while planning: a step without a `first_frame` implies no feature, so on a
  route that does not declare `text_to_video` it is refused only when the call is sent (at $0,
  as before M6). The built-in GG video route does not declare it, and `gg-video` does not judge
  it;
- stand-in runs and route limits: a stand-in holds an answer to the adapter's check, which reads
  no route limit, so a stand-in takes a frameless request, a tier or a length that `fal-queue`
  refuses on a live submit (up to 0.6.1 its check refused a frameless request and every tier and
  length outside its constants), and a node body's generate with references on an OpenRouter
  route that does not declare `image_input`, which the generic adapter refuses on a live send;
- a generic fal video adapter: fal's video request shapes diverge by endpoint (Models
  `v1.20261013.6`, 12 endpoints): the first frame is `image_url` on 6 and `start_image_url` on
  1; `duration` is an integer on 3, a string `"4"` to `"30"` (or `auto`) on 3, and `"4s"`,
  `"6s"` or `"8s"` on 6; no endpoint records an end frame. `fal-queue` fits only endpoints
  with the built-in route's shape;
- a count for the generic image adapters: they own `n` (OpenRouter) and `num_images` (fal), so a
  model that draws several pictures by default fails, billed and not drawn again (`returned no
  single image`); fal's published schemas default `num_images` to 1 where an endpoint takes it;
- exact sizes on the generic image adapters: a model's own size parameter goes in
  `provider_options`, so the image check cannot hold the answer to it, and a step that asks for
  an exact `size` is refused there.

**gridaco/models**, gaps found against `v1.20261013.6`:
- no answer shapes for fal's endpoints (only `grida` bindings have `result_schema_url`);
- no card for Gemini Omni Flash;
- no endpoint facts on the OpenAI binding;
- no mask facts;
- GG's meters have no quantity per request;
- media prices sit outside the schema, and `token_pricing` cannot express image models;
- no ElevenLabs binding or US-dollar rate;
- `input_references` is typed as an integer on OpenRouter;
- no `size` on OpenRouter;
- `image_constraints` only on fal;
- no `references` (the number of input pictures) on 4 of its 17 OpenRouter image bindings.
