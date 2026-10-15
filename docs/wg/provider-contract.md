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

- **One branch, one pull request.** Everything lands as commits on the branch
  `provider-contract`. It stayed local until the owner asked for the pull request (2026-10-15).
  The owner reviews and merges it.
- **Commits follow the milestones in order.** Each commit builds and passes the checks for what
  it touches, and each milestone ends with its gate green. A milestone ticks its boxes here in
  its last commit.
- **Spec first.** A milestone that changes a contract changes `spec/` before the code.
- **Users' surfaces move together.** Every user-visible change updates `llms.txt` in the same
  milestone, and public changes move `AGENT_READINESS.md` and both SDKs with them (AGENTS.md).
- **Never stop to ask.** A question this page does not answer takes the default under
  [Defaults](#defaults); anything left out becomes a follow-up, and the work goes on. The
  follow-ups below are filed as issues with the pull request (2026-10-15).
- **Code facts win.** Where this page and the code disagree on a fact, the page is corrected in
  the milestone that finds it.
- **The version.** The pull request carries the version bump to 0.7.0, as the owner asked
  (2026-10-15); publishing follows RELEASING.md once it merges.

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

- [x] `tools/make_models.py --release <tag>`, run by hand with the network:
  - downloads `models.v1.json` and verifies its SHA-256;
  - validates it against that release's schema;
  - keeps only the schema-defined members that FX's routes use;
  - writes `crates/grida-fx-providers/routes/models.json` with a `source` block (repository,
    release, asset, bytes, SHA-256, schema `$id`).
  `--check` verifies it offline. The full release is never committed.
  - The SHA-256 is verified against the release's own `SHA256SUMS`, for `models.v1.json`,
    `models.v1.schema.json` and `manifest.json`, whose `version` must be the tag and whose entries
    for `models.json` and `schemas/models.schema.json` must agree with both files. `--from <dir>`
    reads the four assets from a folder with the same checks. The run of 2026-10-14 downloaded
    them from github.com; the projection is 38,626 bytes.
  - `CARDS` maps each route id of the table to its model's card (section and id), or to none:
    Codex and Grok Build (and Gemini Omni Flash, until `v1.20261014.1` gave it a card). Writing holds it to the release: each card must bind
    its route's model on its provider, and no card may bind a route given none, so a card that
    appears is not missed. FX's `gg` is Models' `grida`.
  - The projection is `{source, routes}`: per route id `null`, or `{section, card}` and for a rig
    `rig_check`, the section's rig-check operation. A card keeps only what concerns the route's
    provider: its coverage, its binding, and the card's own endpoints when the provider is its
    `reference_provider` (Tripo's endpoints are there). `KEEP` lists the members kept of each
    schema object, checked against the release's schema when the file is written: every endpoint
    of the binding and every parameter are kept, so a feature documented on an endpoint FX does
    not call is still seen; URLs, qualifications, calculators, `token_pricing`'s flat cache rates
    and the members the schema does not define (a card's `pricing`, `sizes`, `constraints`, a
    binding's `references`, `transparent_background`) are not. Code fact: rates are kept by
    member, not by meter, so the cache meters of `token_pricing`'s tiers and of GG's
    `published_pricing` components stay; the price check reads only the input and output meters.
  - `source` also carries `license: MIT`, the catalogue data's. `--check` holds the file to its
    canonical text (two-space indents, sorted members), `source` to the pin, `routes` to the
    table's route ids and `CARDS`, every member to `KEEP`, and the release `providers.md` §3
    names to the pin.
- [x] Pinned to `v1.20261013.6`: `RELEASE` in the tool, which `--release` must name, so moving the
  pin is one reviewed line, the release `providers.md` §3 names, and a new projection
  (CONTRIBUTING.md says how). A new built-in route needs its card in `CARDS` and a new
  projection too.
  - Re-pinned on 2026-10-15 to `v1.20261014.1`, the latest release, with the same schema (byte
    for byte). Writing refused it until `CARDS` mapped the fal video route to its new card,
    `video.models/google/gemini-omni-1.1-flash`, whose fal endpoint is the route's id and whose
    parameters are `fal-queue`'s constants (3 to 10 whole seconds, `16:9` and `9:16`, `360p` to
    `4k`, a prompt of at most 20,000 characters, an end frame); the test exception for its
    missing card went with it. Of the gaps below, only that card is filled.
- [x] `crates/grida-fx-providers/tests/models.rs` checks every route's public facts against the
  projection:
  - wire ids and endpoint ids;
  - features from documented parameters, intersected with adapter support, both ways: a
    documented and supported feature must be declared;
  - picture limits and size envelopes;
  - GG's 3D endpoints;
  - text token prices as bounds, never equality.

  Each exception sits in one table with its reason, and an exception no longer needed fails.
  - Ids: a binding's id is the route's model, but on fal, whose binding id is one of its
    endpoints; a route sends to one of its provider's recorded endpoints: fal's
    `<model>/text-to-image` and `<model>/edit` (`fal::image::GENERATE_ENDPOINT`,
    `EDIT_ENDPOINT`), Tripo's paths (`tripo::mesh::PATH`, `tripo::rig::PATH`; Tripo has no
    binding, so its card's endpoints, whose `model` lists the route's), the GG 3D routes'
    `endpoint`. Code fact: GG's image and video routes post to `/images/generations`,
    `/images/edits` and `/videos`, never to an endpoint id, so they are held to a GG endpoint of
    that `interface`. ElevenLabs has no binding: its routes are held to the card id. Every route
    with a card has its provider `listed`.
  - GG's 3D endpoints: the mesh route's `endpoint` takes `multi_image`, the rig's `asset`, and its
    `check_endpoint` is the rig-check operation's GG endpoint; Tripo's rig route checks at the
    operation's path (`tripo::CHECK_PATH`).
  - Features: three hand tables in the test, the mapping to facts (`documented`), each adapter's
    support from its reference (`honoured`) and `EXCEPTIONS`. Chat features come from the
    binding's `capabilities`. Code fact: GG's text bindings record them too in `v1.20261013.6`,
    so they are checked as OpenRouter's are, not excepted. Image, video, 3D and audio features
    come from the inputs and parameters of the endpoints the adapter sends to. Three features no
    member of the schema can show are in `UNMAPPED`: `authored_prompt_passthrough`, `audio_tags`
    (`input.audio_tags` is not schema-defined) and `glb_input` (`input.formats` is not). A route
    that asks for a reasoning effort runs on a binding with `reasoning`. The check found one
    more feature documented and honoured but not declared: GG serves Wan 3.0's text operation,
    and `gg-video` already sent a request without a first frame, so `alibaba/wan-3.0@gg` now
    declares `text_to_video`. Features are not identity: no fingerprint moved.
  - Limits, FX's within the provider's: the declared input pictures; `wire::SIZE_ENVELOPE`
    (`check_size_envelope`'s literals lifted into a constant, its sentences unchanged), where
    the OpenAI and fal adapters hold a size to it, and the sizes a GPT Image route prices or
    serves, against `image_constraints` and fal's `image_size` edges (GG's routes are priced by
    size too, but leave sizes to GG); fal's prompt (32,000 characters); Tripo's face limit (48
    to 25,000, within 48 to 50,000, so no exception: the follow-up stays); a structured route's
    default `max_tokens` (16,000) within `max_output_tokens` (128,000). Code fact: an agent turn
    sends no `max_tokens` by default (`chat::agent_prompt`), so its routes are not held to it.
  - Prices, the formula decided: (a) a `structured.generate` route's high is at least the default
    `max_tokens` (16,000) times its highest published output rate; (b) within one capability and
    provider, a model whose every input and output rate is at least another's, at every input
    length either publishes a band from, has a low and a high at least the other's. Rates are
    OpenRouter's `token_pricing` tiers and GG's per-token components banded by
    `total_input_tokens`, in exact decimals (units of 10^-18 dollars: Models has 7 places, `Usd`
    6). Both hold: sol's structured high $0.60 ≥ 16,000 × $0.00003 = $0.48, astra's $1.50 ≥
    $1.20, and astra is priced above sol in all four pairs.
  - `EXCEPTIONS`: no card for Codex and Grok Build (runtimes with no provider facts) or Gemini
    Omni Flash; no endpoint on the OpenAI binding; no request parameters on GG's image and 3D
    endpoints, and no `image_constraints` on GG's image endpoints; no `size` or `output_format`
    on OpenRouter, and `image_constraints` only on fal; no mask parameter; no ElevenLabs binding.
    Each test fails on an exception it did not need.
- [x] OpenRouter GPT Image: `alpha` on both routes; the adapter sends `background:
  transparent`; replay tests for the body and the answer's check. `adapter_behavior` stays
  `"3"`, since no body it sent before changes.
  - The refusal `OpenRouter image generation does not support transparent backgrounds` is gone,
    the one sentence that changes. A generate and an edit pin their bodies; the adapter's check
    and the every-route check refuse an RGB answer and an opaque RGBA one, and take one with a
    transparent pixel. `tests/routes.rs` asserts `alpha` on both routes, and still keeps an
    example's OpenRouter route on the generic adapter from declaring it.
- [x] `providers.md`: GG's prices are said to be FX's allowances; GG's image picture limit.
  - §3: GG publishes its image, video and 3D rates in its own units (`fal_output_units`,
    `provider_credits`) with no quantity per request, so its routes take another provider's
    prices until those can be planned from; its text rates equal OpenRouter's. M6 had declared
    the limit (16, on the `image.edit` route only); §3 now calls it GPT Image's. §3 also says the
    table is checked against the projection, and that the GG video route declares
    `text_to_video`. The `default.yaml` header follows.
- [x] Drift: `llms.txt` and guide page 05 on masks, since fal's edit route takes one.
  - Also `llms.txt`'s upgrade note (OpenRouter's transparency, GG video's `text_to_video`) and
    page 05's video row.
- [x] CI: `make_models.py --check` in the `spec` job; AGENTS.md's verification list.
  - Also AGENTS.md's `tools/` row and CONTRIBUTING.md's checks.

Gate: `cargo test`, `check_spec`, `make_models.py --check`, `check_llms`.

### M8. Provider declarations

- [x] Spec (`providers.md`): a declaration names the provider (the route grammar, not a built-in
  name), its credential (variable, header, scheme), its base URL (an optional variable and a
  default; https unless loopback), its download hosts, and its family or adapter. Refused: a built-in
  provider's name; a variable a built-in or another declaration claims; `GRIDA_FX_*`, proxy and
  certificate variables.
  - §1 keeps its heading and gains the built-in declarations as a table (key, header, base URL
    and its variable, default base, download hosts), which a core test holds the code to, and the
    `providers:` member. Codex and Grok Build are runtimes, not declarations; the name check
    covers all eight names FX ships.
  - Code facts: Tripo has no base-URL variable, so `base_url.variable` is optional and
    `base_url.default` required; fal's queue host stays a constant (`setup::FAL_QUEUE`), since only
    its run host is configurable. Base-URL refusals name the provider by a label (`Grida Gateway`,
    `OpenAI`, …), a declared provider by its name.
  - `credential` is optional: a provider without one is sent no key and needs none (a server on
    this machine). `header` defaults to `authorization`, lowercase; `scheme` to `Bearer`, and `""`
    sends the key bare. A variable is the `.env` grammar, `[A-Za-z_][A-Za-z0-9_]*`. `family` is
    optional and `openai-compatible` is its one value; no adapter serves a family until M9, so a
    declared provider's routes plan and price, a live call is `no_route` and doctor says
    `no adapter`. `adapter` is M10's.
  - Refused while `fx.yaml` is read (exit 2), `fx.yaml: providers.<name>…: <sentence>`: `<name> is
    a provider FX ships`; `<VARIABLE> is already read for <provider>` (a built-in, an earlier
    declaration, or the same one's other variable), compared in any letter case, since Windows
    reads variables so; `<VARIABLE> is reserved for FX and the programs it starts` for `GRIDA_FX_*`,
    `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`, `SSL_CERT_FILE`, `SSL_CERT_DIR`, and
    also `PATH` and `HOME`, which the scrubs would otherwise take from the programs FX starts, in
    any letter case; a default base URL that does not normalize, with the base-URL sentences.
    Codex's and Grok Build's withheld extras (`CODEX_API_KEY`, `XAI_API_KEY`, …) are not read by
    FX, so a declaration may read them.
  - Declarations come from the planning project's `fx.yaml` (`store.md`'s list of the planning
    project's settings gains `providers`), are not identity and are not written to `plan.json`.
- [x] `fx-project-v1`: `providers:`.
  - The schema holds the shape (a name `[a-z0-9._-]+`, a lowercase header, a scheme token, a
    variable, a host or `*.` and a domain of two labels or more, the one family) and describes
    the refusals the parser makes, which come after it.
- [x] Built-in declarations replace the fixed key, variable, endpoint and provider lists.
  - Credential headers and prefixes are owned values.
  - The `.env` loader reads the declared names, and the redactor learns their values.
  - The environment scrubs (Codex and Grok Build children, the service, the browser opener) use
    the computed list. Node hosts inherit FX's environment as before (`protocol.md` §1), declared
    variables set there included; `providers.md` §1 says so.
  - Codex and Grok Build stay runtimes.
  - Core's `providers.rs`: `Declaration`, `built_in()` in doctor's order, `RUNTIMES`, and
    `Declarations` (FX's, then the project's; `variables()` gives the keys in order, then the base
    URLs, so the built-in list is the eleven in their old order). `normalize_base_url` and
    `is_loopback` moved there (with the `url` dependency) so a declared default is refused while
    the file is read; `wire` re-exports them. `Project::declarations()`.
  - `keys::ALLOWED`, `setup::BASE_URL_VARIABLES`, `live::PROVIDERS` and the fixed `Endpoints`
    fields are gone. `KeyName` stays as a typed handle on the built-in declarations, its
    variable read from them. `Environment::read` takes the declarations; `Keys` is keyed by
    provider and carries the declarations it was read for, so `Setup::new` and `live::servable`
    keep their signatures, and `Servable::NoKey` names the variable. `Endpoints` is a base per
    provider, plus fal's queue. `Client::credential()` takes no arguments: the header and scheme
    are the declaration's. No credential byte changed: every replay fixture passes unchanged.
  - Scrubs: `Setup::withheld()` reaches `CodexRuntime::new` and `GrokRuntime::new`;
    `codex::version`, `live::installed` (doctor's probe) and the commands take the declared
    variables. The service's child is built by `service_command` from the project's
    declarations. `launch_browser` takes the declarations of the project its caller works in:
    the planning project (`run`, `plan`, `view <target>`), the project `start` serves, and for
    `inspect` the working directory's; `view --run` and `view --plan` read no project, so theirs
    are the working directory's project's, else FX's own. `tests/open.rs` plans another
    project's workflow with `--open` and holds the opener to that project's declarations.
- [x] One download-host allowlist helper, used by fal and Tripo.
  - `wire::download_url(url, hosts)`, refusing with `OffHost::Malformed` or `Outside`, each caller
    keeping its sentences; `Client` carries its declaration's hosts. Code fact: fal's image and
    background-removal answers check hosts, its video adapter and GG take any `https` URL
    (unchanged; a follow-up).
  - One rule set, fal's: Tripo now refuses a model URL with a fragment and accepts a host with a
    trailing `.`; its reference says both, and `llms.txt`'s upgrade note the fragment, the one a
    user can meet.
- [x] `doctor` lists declared providers with their key state and base-URL host.
  - After the built-in `key` lines, which are unchanged: `provider  <name> <host> <VARIABLE>
    present (environment)` (or `present (.env)`, `missing`), or `… needs no key`, `<host>` with
    the port a base URL names, the default's when its variable is refused. Each base URL is read
    on its own (`Endpoints::each_from_environment`), so a refused variable moves no other
    provider's host, and each refused one has its `keys` line; a live run still stops at the first.
- [x] The tools' and tests' key lists read the engine's list, or stay in step with it.
  - Code fact: no command prints the list, and the examples' checks stand alone, so
    `crates/grida-fx/tests/key_lists.rs` holds the 17 lists (tools, examples, both SDKs' tests)
    to the engine's: the six keys, or for `check_llms.py` and `demo_viewer.py` all eleven
    variables. `cli.rs` takes the engine's list.
- [x] A conformance case declares `acme`: plan, `doctor`, and the refused declarations.
  - `providers-declared`: `acme` and a keyless `local`; the workflow on an `acme` route plans and
    expands to the same graph without the declarations (saved twice); `doctor`'s `provider` lines
    by `mentions`; six refusals. It binds no chat route on a family provider: M9 may. The
    README gains a `doctor` row.
- [x] `llms.txt` (it says "only the eleven FX key and base URL names are read"); guide pages 05
  and 13.
  - Code fact: "Only these eleven variables are read", which this item quoted, was
    `providers.md`'s; guide page 05 said "only those eleven names". Also `llms.txt`'s `fx.yaml`
    block, a declaration with its refusals in the errors table, and the upgrade note (Tripo's
    fragment); page 13's "A provider FX does not ship"; `AGENT_READINESS.md` (AR-20, with no SDK
    API: neither SDK writes `fx.yaml`); the Codex, Grok Build, fal and Tripo references;
    `adapters.md`'s download rule; the starter's `.env.example`.

Gate: the whole verification list. `tools/check_observation.py` resolves its scratch folder
before `prepare_output`, which refuses a path through a symbolic link: macOS's temporary folder
is under `/var`, a link to `/private/var`, so the gate needs it on macOS.

### M9. OpenAI-compatible providers

- [x] Spec: the `openai-compatible` family serves `structured.generate` and `agent.turn` over
  `{base}/chat/completions`.
  - Errors map through the default status table, never `Some(0)`.
  - Cost is `None`, so the hold or the cap is charged.
  - Contract adapters are `openai-compatible-structured` and `openai-compatible-tool-loop`,
    behaviour `1`.
  - `providers.md` §1 (the family serves the provider's two chat capabilities; only a `family`
    reaches a fingerprint) and §2, a third table, `| Family | Capability | …`, with the two
    adapters, each its capability's default on every provider the planning project declares with
    the family; `adapters.md` §1 (registration from declarations; a stand-in run's adapters are
    built for FX's declarations only, so a declared route gets its capability's every-route
    check, all the family checks); the `family` description in `fx-project-v1`. The reference is
    `crates/grida-fx-providers/src/openai_compatible/README.md`.
  - Code fact: the behaviour is the number `1`, as every route outside images carries it
    (`tests/routes.rs`); the string `"1"` is another fingerprint.
  - The fill extends to declared families (R7; the Default on a route without a contract):
    `identity.md` §7 step 2 completes a declared provider's route from its family's adapters,
    keyed on the planning project's declarations, never on a provider's name, so an undeclared
    `acme` route, in conformance and in `identity.md`'s worked examples, does not move. Core's
    `routes::FAMILY_ADAPTERS`; `complete_contract` and `parse_table` take the declarations
    (`load_catalog` passes the project's, `RouteTable::from_document` FX's own,
    `RouteTable::from_document_over` the caller's). `digest.py` keeps `FAMILY_ADAPTERS` and reads
    `fx.yaml`'s `providers:` for `--check-graph`; route vectors take a case's `providers`, with
    three new cases; `check_spec` (j) holds the family table, its reference and digest.py's copy.
    No built-in fingerprint moved.
  - The wire (the Default on a generic adapter's wire): only what OpenAI's Chat Completions
    reference names and FX's two capabilities need: `model`, `messages`, `max_tokens` (the
    request's, else 16,000 on a structured call), a strict `json_schema` response format or
    strict function tools, and `tool_choice`. Contract members: `adapter`, `adapter_behavior`,
    `provider_options`; structured pictures are always reduced (there is no `pictures:
    unchanged`), a turn's pictures go as they are, and no picture carries a `detail`. Owned:
    every body member, `max_completion_tokens` (it restates `max_tokens`), `stream`, `n`, and the
    other chat capability's members that would change what it reads in the answer (`tools` and
    `tool_choice` on structured output, `response_format` on a turn), as OpenRouter's.
  - Labels `<name> structured generation` and `<name> tool loop`; deadlines 1800 s and 600 s; a
    16 MiB response cap and a 200 MiB body cap (OpenRouter's chat precedent). A redirect names
    the declaration's base-URL variable, else `providers.<name>.base_url`. Token usage is
    recorded: GG's reader moved to `chat::usage`, a small extraction.
- [x] Registered from declarations, on the shared chat module.
  - Code fact: no registry site needed new plumbing. `live::adapters` reads the declarations the
    setup's `Keys` carry (M8), so the engine (`adapters_for`) and doctor's `uses` and `route`
    lines get the planning project's; the stand-in checks keep FX's own (above).
    `openai_compatible::register` adds both adapters per declaration of the family, after FX's.
    `Setup::declared_client` and `Client::optional_credential` (none for a keyless declaration).
  - The shared chat module gained `chat::completions_request`, `chat::structural` and
    `chat::usage` (OpenRouter's and GG's, moved, with no byte or sentence changed);
    `wire::with_request_id` is OpenRouter's. A new `classify` follows `adapters.md` §3.3 as
    written: no adapter had a shared default-table classifier.
- [x] A replay test: a made-up declared `acme` answers both capabilities.
  - `tests/openai_compatible.rs`: both answers from the registry a live run builds (exact bodies,
    cost `None`, usage), a keyless `local` sent no credential, the base-URL variable, refusals in
    order, provider options, every status and structural failure. The engine-check matrix
    (`tests/engine_checks.rs`) now registers a declared `acme`, so it covers the family. Also a
    retry-owner test (each attempt charged the whole cap, a reported cost ignored), a CLI test (a
    live run with the network off reaches the adapter; undeclared, `no adapter serves`), doctor
    (`no key (ACME_API_KEY)`, then `servable`), and `conformance/providers-declared`, which gains
    a chat workflow: its filled fingerprint (`expand-chat.json`, checked by digest.py through
    `check_spec` (h)) and the bare one once `acme` is no longer declared. No other expected file
    moved.
- [x] An `llms.txt` recipe: a declared provider, planned offline.
  - "A provider FX does not ship, from a declaration alone"; M11 does not repeat it. Also
    `llms.txt`'s routes section and errors table, guide page 13 ("An OpenAI-compatible
    provider"), `AGENT_READINESS.md` (AR-20, still Proposed) and the conformance README.

Gate: `cargo test`, `check_llms`, and, since the fill reaches identity, `check_spec`, conformance
and `pytest`.

### M10. Project adapters in Python and TypeScript

- [x] Spec: the out-of-process binding (`adapters.md`); in `protocol.md`, the methods
  `adapter.load`, `adapter.send`, `adapter.submit` and `adapter.collect`, and the host-to-engine
  `fetch`, added within protocol v1; their schemas.
  - The wire, within `fx-node-protocol-v1` and with no new error code: `adapter.load {path,
    attribute}` answers `{adapter: {version, capabilities: {<capability>: request | job}},
    closure}`; the calls carry `call_id`, the adapter, the route (`id`, `model`, `provider`, the
    completed `contract`), the declaration's `base_url`, the request, its `files` as `{digest,
    kind, size, path}` (a call's files have no name or facts), `take`, `key`, `attempt` and, to
    collect, `handle`. Their results are outcomes named by `outcome`, a new convention, since
    they share `reason` and `cost_usd`. `fetch` carries the call's `call_id`, never a `run_id`.
  - `adapter.load` is answered by the planning host too: the fingerprint, and so every call key,
    is taken while planning, and the engine must know a capability's kind (request or job) before
    its first call, so a plan, a dry run and an offline replay need the adapter's language's
    host, never an adapter host. A declared adapter that cannot be loaded refuses the command
    (exit 2), `fx.yaml: providers.<name>.adapter: <sentence>`.
  - Identity (`adapters.md` §9, `identity.md` §7): `adapter` is `<path>#<attribute>`,
    `adapter_behavior` the version or `source:<digest>`, the digest of the `fx-node-source-v1`
    object with no resources; `fx.lock`'s `adapters` and drift reported on the step, as a node
    type's.
  - The adapter host: one per declaration with an adapter, started on the first call of a live
    run (the network off included), outside the pools, shut down and verified with the run; the
    second exception to one job per host, and `protocol.md`'s "never talks to a provider" now
    excepts it. Its environment has its own declaration's variables, from `.env` too, and no other
    declaration's. A lost host, an adapter's error or an unreadable result: a send `Failed { cost:
    None, retryable: false }` (Codex's precedent; a retryable one could hold up to six times), a
    submit `Uncertain`, a collect `Unreachable`. Each call is refused when the host's load
    differs from the plan's.
  - The engine makes `adapters.md` §4 steps 1 to 3 (the key too), holds an answer to its shape as
    a stand-in's, then makes the every-route check; there is no `adapter.check`, so the adapter
    judges inside its send and collect. The adapter's reported cost is read: the adapter contract
    carries it (the Default below is the family's). A handle must be an object that holds no
    key's value, else the submit is `Uncertain`.
  - `fetch`: GET and POST (`transport::Method` has no other); deadlines and caps at most 300 s and
    64 MiB, `HttpRequest::new`'s defaults; the credential only when asked, on the provider and
    upload lanes, and only to the base URL's origin; downloads held to the declaration's
    download hosts, so a declaration that lists none downloads nothing through it; failures as
    `refused`, `not_sent` or `after_send`.
  - `providers.md` §1: `adapter` beside `family`, one or the other (`providers.<name>: a provider
    has a family or an adapter, not both`, the parser's, after the schema); §2's registration and
    fill; `fx-project-v1` (`adapter`, without `./` or `..`) and `fx-lock-v1` (`adapters`).
    `protocol.md` §8 gives both SDKs' API: `adapter(…)`, an `AdapterCall`, the outcomes
    (Python's `Answered`, `NotReceived`, `Refused`, `Failed`, `Accepted`, `Uncertain`, `Ended`,
    `Unreachable`; TypeScript's plain objects), `call.fetch` and `FetchError`.
  - After review: `protocol.md` §3.1 (an adapter call's files are not file refs), §5.7 and §5.8
    (a stand-in host, and a host with a run pending, refuse `adapter.load`), §6.1 step 7 and
    `capabilities.md` §1 (the engine checks a project adapter's request as it checks one for an
    adapter FX ships, and holds its answer to its shape, then to the every-route check), and
    §6.8, which now says what the code does: the declaration's own credential header is refused
    on every lane.
    `adapters.md` §9: a project adapter MUST keep the network switch for a client of its own,
    which the engine cannot turn off; `llms.txt`, guides 05 and 13 and both SDK READMEs no longer
    say the switch stops every request.
  - After the second review: `adapters.md` §9 and `protocol.md` §5.8 say that a send or a submit
    never written whole to a host whose connection had ended is `NotReceived`, sent again to a new
    host; `capabilities.md` §13 and `providers.md` §3 that a capability FX ships no adapter for is
    served by a project adapter that declares it.
  - After the third review: `fx-routes-v1`'s `contract` says that a project adapter completes a
    left-out `adapter` and `adapter_behavior` too, with its name and its version or source.
- [x] A declaration's `adapter: <path>#<attribute>`; `fx.lock` `adapters`. Identity: the contract
  `adapter` is the declared adapter, and `adapter_behavior` is its version, or `source:<digest>`.
  - Core's `providers::DeclaredAdapter` (`path`, `attribute`, `loaded`) and `LoadedAdapter`
    (version, each capability's kind, the closure's digests, drift). `adapters::load_declared`
    fills them in `make_planner`'s step 5, before the home's host is opened and the route tables
    are read, with the planning host pointed at the planning project; `NodeHost::load_adapter`
    (`FakeHost::with_adapter` for tests); `registry::closure_digests` hashes the closure. The
    drift is judged against the planning project's `fx.lock`, which is the home's only when the
    two are one project.
  - The fill: `routes::complete_contract` completes a declared provider's routes from its loaded
    adapter as from a family, for each capability the adapter declares. `Route::drift`, not
    hashed, carries a drifted adapter's sentence to every route whose contract names it, and
    binding reports it at `<step>.route`, the route still bound.
  - `fx.lock`: `LockFile.adapters`, written after `nodes` only when there is one (the header and
    `lock-write`'s file do not move). `grida-fx lock` loads every declared adapter and judges each
    versioned one as a node type (`unlocked  <key>: run grida-fx lock`, `changed   <path>#<attr>:
    …`, `--same <path>#<attribute>`). Code fact: no spec states `lock`'s count line; with an
    adapter in the lock it reads `fx.lock: <n> versioned node types, <m> versioned adapters`.
  - Code fact: the providers crate's `Identity` holds `&'static str`, and the registry keys its
    adapters by it, so a project adapter's identity is interned once per distinct text
    (`Identity::interned`) rather than changing every adapter FX ships.
  - `tools/digest.py` reads `fx.yaml`'s `adapter` and `fx.lock`'s `adapters`: a versioned
    adapter's routes are checked against the contract it completes, or as written (it cannot know
    which capabilities the adapter declares); an adapter `fx.lock` gives no single version for is a
    note. `python/tests/test_digest.py`.
  - After the third review: `fx.lock`'s version is a guess, since `grida-fx lock` never drops an
    entry and planning uses the adapter's own version, or its source, whatever the lock holds (a
    version bumped before `lock`, or one removed). A route that guess does not complete is a note,
    not a failure, unless `--current-lock` says the lock is current, as `check_spec` (h) does for
    the conformance cases, which commit it so.
  - Vectors: `spec/vectors/identity/routes.json` gains three cases of a declared adapter (its
    capabilities' routes filled, its stated members kept, an unversioned one known by its source),
    whose `adapters` member gives what loading told planning: version, capabilities and the
    source's file digests. `tests/vectors.rs` and `digest.py` both reproduce them.
- [x] Engine:
  - an adapter host started lazily in a live run, outside the pools, and shut down with the run;
  - an RPC adapter for send, submit and collect;
  - a host lost mid-send is `Failed { cost: None, retryable: false }`, mid-submit `Uncertain`,
    mid-collect `Unreachable`;
  - handles must be objects;
  - cancellation at the invocation;
  - dollars only;
  - declared credentials passed into the host's environment.
  - `crates/grida-fx-runtime/src/adapter_host.rs`: `register` (one `AdapterHost`, kept as a
    `Runtime`, and one `ProjectAdapter` per declared capability, a request adapter or a long job
    by its kind, its provider's default), `Launch` (`ProcessLaunch` over
    `HostProcess::start_accountable`; tests launch a peer on an in-memory connection; `NoLaunch`
    for doctor). The engine's checks wrap it as any adapter (`calls::checked`); the key is checked
    before a send or a submit; each call is `c<n>` on a task of its own, its drop sending
    `$/cancel`. `HostSpec::with_environment` withholds every other declared variable and gives the
    declaration's own, as `.env` or the environment gave them (`engine.rs`, `adapter_environment`).
  - Decisions: a host that cannot start, cannot load the adapter, or loads another one than the
    plan's is not started again in the run (Codex's refused runtime), so each send and submit is
    `Refused` and each collect `Unreachable` (`adapters.md` §9 now says so); an answer whose shape
    is refused is billed as a check's refusal (`Failed { retryable: true }`, a collected one
    `Ended`), at the reported cost; a broken protocol ends its calls as a lost host does (`the
    adapter host broke the protocol: …`); the planning host names the SDK that predates adapters
    with the version `initialize` reported.
  - `doctor`: an `adapter` line per declaration with one, loaded as planning loads it (`version
    <v>: <capability> (request), …`, or `cannot load: <sentence>`, exit 1), and the routes of its
    capabilities judged as served by it (`providers.md` §1 says so).
  - After review: a host whose start its call abandoned (a run stopped while the adapter loads),
    one that died and one that would not serve are each ended on a task of their own, which the
    run's cleanup waits for and counts; a host process dropped before it was reaped leaves the
    cleanup unverified (`HostProcess::start_accountable`, as the pools' start); and a host the
    engine ends tells the calls still waiting on it how it ended, rather than leaving them to the
    watcher it stopped. `tests/adapter_host.rs` pins both.
  - After the second review: a call whose request was never written whole, the host's connection
    having ended between the check that the host is alive and the write, is no longer taken for
    a lost pending call: `Connection::try_request_cancellable` says whether a request that got no
    result may have reached the host (`Unanswered::written`), and such a send or submit is
    `NotReceived`, which the engine sends again and `running()` gives a new host. Every reason
    built from the host's result (an unreadable one, a refused shape) passes the call's
    redactor, which knows the URLs it downloaded, not only the invocation's keys.
    `tests/adapter_host.rs` pins both, and `host::connection`'s tests the written flag.
  - After the third review: the module docs of `calls`, `calls::checked`, `calls::retry` and the
    providers crate's `checks` and `capabilities` say that the engine's checks wrap a project
    adapter as one FX ships, as the code does (`checked` wraps every adapter that names an
    identity).
- [x] `fetch`: the engine's transport, with the credential attached by the engine, the network
  switch, caps and deadlines.
  - Refused with `-32602`: an unknown `call_id`, a body on a GET, a header name that is not a
    lowercase token, a body that is not base64, `timeout_s` or `max_response_bytes` out of range;
    a dropped call's fetch is `cancelled`. Refused before it leaves: a credential on the download
    lane, to another origin (`wire::same_origin`), or whose key is not set (`protocol.md` §6.8 now
    says so); the declaration's own credential header set by the adapter; a download off the
    declaration's hosts; whatever the transport refuses (`Offline` with the network off).
    Downloads join the redactor of the call's reasons.
- [x] SDKs: an adapter authoring API in `grida.fx` and `@grida/fx`, with outcome types and
  concurrent calls; tests; READMEs; `protocol.md` §8.
  - Python, done: `grida/fx/_adapter.py` (`adapter()`, `Adapter`, `AdapterCall`, `AdapterRoute`,
    `AdapterFile`, `FetchResponse`, `FetchError`, the eight outcomes, the `Server` that answers
    calls), exported from `grida.fx`; the host answers `adapter.load` on any host but a stand-in
    host, and the calls on the body loop; `fetch` goes out without a `run_id`
    (`Session.request(None, …)`). Tests: `tests/test_adapter.py`, the adapter host in
    `tests/test_host.py`, a live run through the engine in `tests/test_adapter_engine.py`, and
    `crates/grida-fx-runtime/tests/python_host.rs` (`the_real_python_host_serves_a_project_adapter`).
  - Decisions: `$/cancel` answers the call `cancelled` at once, a coroutine function's task
    cancelled and a plain function's return discarded (a stand-in's waits for its coroutine to
    end); a fault ends only its own call (a stand-in's stops every later one); `Failed`'s
    `retryable` is a required keyword, as TypeScript's member is required; each outcome holds its
    scalar members when made, and its files, data and handle when sent; an answer's files take an
    `AdapterFile` too; `fetch` lowercases header names and raises what the engine would refuse
    with `-32602` before sending; `fetch` is a coroutine only, as `ctx.capability` is. A host
    refuses `adapter.load` on a stand-in host and while a run is pending (`-32600`), and a call
    for a capability its adapter does not serve with the method (`-32602`, `<path>#<attribute>
    does not serve <capability> with adapter.send`; `protocol.md` §5.8 now says so).
  - TypeScript, done: `js/fx/src/adapter.ts` (`adapter()`, `Adapter`, `AdapterCall`, a class
    generic over the capabilities its function serves and its handle, with `CollectCall` for
    `collect`; `FetchResponse`, `FetchError`; the outcome types `Answered`, `NotReceived`,
    `Refused`, `Failed`, `Accepted`, `Uncertain`, `Ended`, `Unreachable`, `SendOutcome`,
    `SubmitOutcome` and `CollectOutcome`, with `Answered.json`; the `AdapterServer` that answers
    calls) and `AdapterFile` in `files.ts`, exported from `@grida/fx`; `host.ts`'s
    `serveAdapters` answers `adapter.load` on any host but a stand-in host, and the calls on the
    host's event loop; `fetch` goes out without a `run_id` (`Session.request(null, …)`). Tests:
    `test/adapter.test.ts`, the adapter host in `test/host-adapter.test.ts`, a live run through
    the engine in `test/adapter-engine.test.ts`, the types in `test/types/adapter-types.ts`, and
    `crates/grida-fx-runtime/tests/typescript_host.rs`
    (`the_real_typescript_host_serves_a_project_adapter`).
  - Decisions: the declaration's types refuse what `node()`'s refuse for a body, where `tsc` can
    see it: no capability, `requests` without `send`, `jobs` without `submit` and `collect`, a
    field `adapter()` does not know, an outcome a function's method does not take, and a member
    an outcome does not have (`cost_usd` for `costUsd`, a `refused` with a `costUsd`), the last by
    a constraint on each function's inferred outcomes, since TypeScript checks no function's
    return for excess members; each function's `call.capability` is typed as the capabilities it
    serves, and `collect`'s `call.handle` as a `JobHandle`. Outcomes are plain objects, held to
    their members when sent (Python's hold their scalars when made): a member an outcome does not
    have is refused (`Refused holds outcome and reason, not 'costUsd'`), one whose value is
    `undefined` is left out, and an answer's `files` and `data` may be left out, as Python's
    defaults leave them. `fetch` takes a `URL` as well as a string, `headers` as an object or as
    pairs (a `Headers`, a `Map`), `json: null` as JSON's null (only `undefined` leaves the body
    out), refuses an option it does not know, and names its options as TypeScript writes them
    (`timeoutS is above 0 and at most 300`, `a body needs its contentType`). `$/cancel` aborts
    `call.signal` with a `Cancelled`. Every refusal of the declaration, the call's params,
    `fetch`'s options and an outcome is a `TypeError`, as `node()`'s and a stand-in's are. A
    stand-in's answer now leaves out a file whose value is `undefined`, as `protocol.md` §8.2
    says of every value the SDK sends; it was refused before. Each answer takes its own call's
    files, as Python's: a stand-in's an `InputFile`, an adapter's an `AdapterFile`
    (`protocol.md` §8.1 and §8.2 say so).
- [x] Conformance: `adapter-py` and `adapter-ts`, each a synthetic `acme` adapter run live with
  the network off: an answer, a refusal and a long job.
  - Each case declares `acme`, keyed, served by a request adapter, and the keyless `acme-jobs`,
    served by a long-job adapter of the same module (`adapters/acme.py#acme` and `#acme_jobs`;
    `adapters/acme.ts#acme` and `#acmeJobs`), both versioned, with the committed `fx.lock`'s
    `adapters`, so `check_spec` (h) recomputes each route's fingerprint through `digest.py` from
    `fx.yaml` and `fx.lock`. The adapters make no request: the one `fetch` is refused before it
    leaves. A run answers one call, submits and collects a job, and refuses one with a reason
    that holds the key, which reaches neither the terminal nor the record (`[redacted]`); a second
    run without the key is answered from the result cache and refuses the call before its
    adapter (`ACME_API_KEY is not set`). The cases also pin `doctor`'s `adapter` lines, the drift
    an edit to an imported module makes until `lock --same`, and three load refusals. The
    TypeScript case's bytes differ from the Python one's only through the adapters' names.
  - `conformance/run.py` gains a step's `env`: variables for that step's command alone, which
    may not be one the runner sets nor any `GRIDA_FX_*`, so a made-up key can prove redaction
    while the network stays off.
  - Each case also answers the adapters' routes with a stand-in (`stand_in.py`, `stand_in.ts`):
    a run without `--live` builds no adapter, so no adapter host starts, and the answers get the
    every-route check, which stores one and refuses one that breaks the schema.
- [x] `llms.txt`; guide pages 03 and 13.
  - `llms.txt`: an `adapter` bullet under Routes and providers, a recipe for a Python adapter
    (planned by `check_llms`) with its TypeScript form, Python and Node needed for adapters, and
    the errors table's rows for the load refusals, the family-or-adapter refusal, the SDK that
    predates adapters, an adapter's failure, a changed adapter, drift and `fetch`'s refusals.
    Guide page 13 gains "Your own adapter"; page 03 a section that places adapters beside node
    types and their versions. `lock`'s help now says it pins adapters too. `AGENT_READINESS.md`
    AR-20 gains the adapters' evidence and stays Proposed until M11.
  - After the second review: `llms.txt` and AR-20 say what the code does: planning and a dry
    run refuse an adapter that cannot load (exit 2), and `doctor` prints `adapter   <name>
    <path>#<attribute> cannot load: <sentence>` and exits 1, as `providers.md` §1 and guide 05
    say.
  - After the third review: guide 01 lists adapters among what Python and Node run, so a project
    of paid built-ins that declares a Python adapter needs Python, as `llms.txt` says;
    `python/README.md`, `CONTRIBUTING.md` and guide 05 name adapters beside nodes, builders and
    stand-ins.

Gate: the whole verification list, `check_host` included.

### M11. Docs, readiness and the PR

- [x] Guide page 13, "Use a model or provider FX does not ship"; caps in page 04; routes in
  page 05.
  - Page 13 keeps "A model FX does not list" (now with OpenRouter text) and "Parameters FX does
    not expose"; its "A provider FX does not ship" is a summary that names the term and links
    page 15, where the detail moved. Pages 01, 03 and 05 link page 15's sections for adapters,
    `fetch` and declarations, and so do the README and both SDK READMEs. Page 04 already said
    what M4 built; page 05 now says that a declared provider's routes are completed from its
    family or adapter, and points to page 15. The guide's chapter list gains page 15.
- [x] Guide page 15, "Community providers" (asked for by the owner, 2026-10-15): a provider a
  project declares, end to end, as the one place a user learns it. Choosing the path; the
  declaration and its refusals; routes; the OpenAI-compatible family; a complete adapter in
  Python and in TypeScript (the authoring API, request or long job, outcomes and what each costs,
  `fetch`, provider options, reasons and redaction, its version and `fx.lock`, the adapter host);
  checking it offline; identity and the cache; sharing it today (adapters in `sources:` packages
  planned); its limits. Its examples were planned, locked, run with the network off and with a
  stand-in through the engine, the adapters run against a fake of `acme` on loopback, and the
  TypeScript one type checked; the output it shows is the engine's.
  - Code fact: the engine passes `provider_options` to a project adapter as written (the route's
    in its contract, the step's in its request) and neither merges nor judges them, so the
    page's adapters merge them themselves; `llms.txt` says so.
  - Code fact: a live run on a declared provider needs a ceiling even when its routes are free
    (`usd: 0`): only Codex and Grok Build bill a plan.
- [x] `llms.txt`: a recipe for a new model (M9 wrote the declared provider's); the errors table.
  - The recipe, "A model FX does not list, on a provider FX speaks", plans a capped OpenRouter
    chat route with route options, a sibling that `extends` it, and a capped fal image route
    with a step's options. The term "community provider" and a link to page 15 join the routes
    section and the docs list. The errors table gains the sentences this branch added that it
    lacked: a zero cap, an options member that is not an object, a contract member an adapter
    does not take, an OpenAI-compatible server's statuses and redirect, the route's picture
    limit while planning, fal video's frame, length and tier refusals, the generic adapters'
    unreadable answers, three more load refusals, a broken protocol, and `fetch`'s credential
    refusals.
- [x] `skills/grida-fx` and its onboarding reference.
  - `SKILL.md` gains "A model or provider FX does not list" and its description the jobs;
    `references/onboarding.md` a community-provider path, doctor's `provider` and `adapter`
    lines and a route for an unlisted model; `references/authoring.md` points a node that needs
    another provider at a declaration and an adapter, never a direct call. They say FX 0.7.
- [x] `AGENT_READINESS.md`: AR-20 Current, with its evidence.
  - With the guide page, the recipes, and the tests by path; the vocabulary gains route, cap,
    provider options, community provider, provider declaration and adapter; the source review
    date moves to 2026-10-15. The overview's decision 15 says implemented, pending release;
    "What shipped" keeps released things only.
- [x] The follow-ups written up on this page, filed as issues with the pull request (below).
- [x] The whole verification list, green on the branch: 2,250 Rust tests in 74 binaries,
  1,612 Python tests, 508 TypeScript tests, 66 conformance cases (also through `grida fx`), and
  every check AGENTS.md lists, with `make_models.py --check` and `check_versions.py`.

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
| A declared provider's errors | the OpenAI-compatible family's: the default status table, never `Some(0)`; a project adapter's: the outcomes it returns, a `failed` or `ended` one's `cost_usd` of 0 included (M10) |
| A declared provider's cost | the OpenAI-compatible family's: none read, so the hold or the cap is charged; a project adapter's: the cost it reports, as the adapter contract says (M10) |
| New protocol methods | added within protocol v1; hosts answer unknown methods with -32601 |
| Project adapters in conformance | run live with the network off; the synthetic adapter makes no request |
| The Models file | one generated projection; the full release is never committed |
| Prices against Models | bounds, never equality |
| A test that resists | fix the code, not the test, unless the test pins wording this plan changes |

## Follow-ups

Listed here as each milestone leaves something out, with only verified facts, and filed as issues
with the pull request (the owner asked for it on 2026-10-15).

Filed, grouped by theme, on 2026-10-15:

| Issue | Covers |
|---|---|
| [gridaco/fx#91](https://github.com/gridaco/fx/issues/91) | Caps: say a call is capped beyond its instance, and flag a call charged more than it held |
| [gridaco/fx#92](https://github.com/gridaco/fx/issues/92) | provider_options: agent runs, a plan-time guard, and members FX owns whole |
| [gridaco/fx#93](https://github.com/gridaco/fx/issues/93) | Generic image adapters: picture count, exact sizes, and more generic adapters |
| [gridaco/fx#94](https://github.com/gridaco/fx/issues/94) | fal video: route-declared limits, text_to_video at planning, and a generic adapter |
| [gridaco/fx#95](https://github.com/gridaco/fx/issues/95) | Stand-in runs: route limits, declared providers and an adapter.check method |
| [gridaco/fx#96](https://github.com/gridaco/fx/issues/96) | OpenAI-compatible family: 408s, contract options, refusals and costs |
| [gridaco/fx#97](https://github.com/gridaco/fx/issues/97) | Project adapters: fetch, packages, route limits, deadlines and redaction |
| [gridaco/fx#98](https://github.com/gridaco/fx/issues/98) | Provider declarations: download hosts, the browser opener, headers and redirect sentences |
| [gridaco/fx#99](https://github.com/gridaco/fx/issues/99) | Grida Models: a project-level pin, media facts and one list of honoured features |
| [gridaco/fx#100](https://github.com/gridaco/fx/issues/100) | Identity break before 1.0: provider-neutral request fields and the naming review |
| [gridaco/fx#101](https://github.com/gridaco/fx/issues/101) | Route table: pacing shared across a provider's routes, and ElevenLabs speech's allowance |
| [gridaco/fx#102](https://github.com/gridaco/fx/issues/102) | Conformance runner: live-run events and files in never_mentions |
| [gridaco/fx#103](https://github.com/gridaco/fx/issues/103) | Tests that fail under load |
| [gridaco/fx#104](https://github.com/gridaco/fx/issues/104) | Rustdoc warnings |
| [gridaco/fx#105](https://github.com/gridaco/fx/issues/105) | A live run whose calls are all on free routes still needs a ceiling |
| [gridaco/models#21](https://github.com/gridaco/models/issues/21) | Gaps found while pinning v1.20261013.6 in Grida FX |

The list they were filed from:

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
  and a cap: Grida Models records `background: transparent` on the endpoint but no
  `output_format`, so a transparent answer in another format would fail the check, billed;
- one list of the features each adapter honours: `tests/models.rs` (`honoured`) and
  `tests/routes.rs` (`features_say_what_the_adapters_honour`) each keep their own by hand, since
  no adapter exports one;
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
  as before M6). `gg-video` does not judge it (the built-in GG video route declares it since
  M7);
- stand-in runs and route limits: a stand-in holds an answer to the adapter's check, which reads
  no route limit, so a stand-in takes a frameless request, a tier or a length that `fal-queue`
  refuses on a live submit (up to 0.6.1 its check refused a frameless request and every tier and
  length outside its constants), and a node body's generate with references on an OpenRouter
  route that does not declare `image_input`, which the generic adapter refuses on a live send;
- a generic fal video adapter: fal's video request shapes diverge by endpoint (Models
  `v1.20261014.1`, 22 endpoints): the first frame is `image_url` on 10 and `start_image_url` on
  1; `duration` is an integer on 8, a number on 2, a string `"4"` to `"30"` (or `auto`) on 4,
  `"4s"`, `"6s"` or `"8s"` on 6, and untyped on 2; an end frame (`end_image_url`) on 3.
  `fal-queue` fits only endpoints with the built-in route's shape;
- a count for the generic image adapters: they own `n` (OpenRouter) and `num_images` (fal), so a
  model that draws several pictures by default fails, billed and not drawn again (`returned no
  single image`); fal's published schemas default `num_images` to 1 where an endpoint takes it;
- fal image endpoints the generic adapter does not fit: Models `v1.20261014.1` records Ideogram
  4.5's edit taking one `image_url` and `reference_image_urls`, where `fal-image` sends
  `image_urls`, and pictures cannot go in `provider_options`, which hold JSON only;
- exact sizes on the generic image adapters: a model's own size parameter goes in
  `provider_options`, so the image check cannot hold the answer to it, and a step that asks for
  an exact `size` is refused there;
- download hosts for fal video and GG: `fal/video.rs` (`result_video`) and `gg/job.rs`
  (`result_file`) download from any `https` host, so their declarations list none (fal's lists
  `fal.media` for its image and background answers);
- the browser opener's declarations for a recorded run or plan: `inspect <run>`, `view --run
  <dir>` and `view --plan <file>` withhold those of the working directory's project, which is not
  the run's when its folder lies in another project, since a run's `plan.json` holds only
  project-relative paths;
- a declared credential header the transport also sets: a declaration may name `content-type` or
  `accept-encoding`, which `transport/http.rs` sets itself;
- the redirect sentences' variables: `gg::BASE_URL_VARIABLE`, `elevenlabs::BASE_URL_VARIABLE`,
  fal's `RUN_BASE_VARIABLE`, OpenAI's `BASE_URL_VARIABLE` and OpenRouter's literal restate the
  declarations' base-URL variables;
- a 408 from an OpenAI-compatible server: the family follows `adapters.md` §3.3, so a 408 is
  `NotReceived` and resent under the same attempt, unbilled, while FX's OpenAI and OpenRouter
  adapters treat their providers' 408 as a timeout after the request arrived (`Failed { cost:
  None, retryable: true }`); a server whose 408 means the same is resent without a bill;
- family contract options: the family's contract takes only `adapter`, `adapter_behavior` and
  `provider_options`, so a route cannot ask for a picture `detail`, a reasoning effort or
  `pictures: unchanged`, and the token limit is always `max_tokens`: the family owns
  `max_completion_tokens`, so a server that takes only that name cannot be given a limit;
- an OpenAI-compatible provider's refusals and costs: every 4xx but 408 and 429 costs `None`, so
  a server that refuses a request it never ran (a `json_schema` response format it does not take,
  say) is charged the whole hold or cap, and a reported cost (`usage.cost`) is never read; its
  declaration has no member to say either (a project adapter reports its own cost and outcome);
- stand-in checks for declared providers: `StandInChecks::new` builds its registry with
  `Keys::none()`, FX's declarations only, so a declared route's stand-in answer gets the
  every-route check, which is what the family checks (a project adapter has no check of its own:
  see the next item);
- an `adapter.check` method: a project adapter judges an answer only inside its send and collect
  (`adapters.md` §9), so a stand-in's answer on its route gets the every-route check alone;
- `fetch` methods beyond GET and POST: `transport::Method` has only those two;
- offline replay through `fetch`: the replay transport is test-only
  (`#[cfg(any(test, feature = "testing"))]` in `transport/mod.rs`);
- project adapters in packages under `sources:` (R4): a declaration names a module of the
  planning project, which may import one from a package, whose files then count in its closure;
- a route's limits for a project adapter: `adapter.send` carries no `max_input_pictures`, tiers,
  `max_units` or features (the providers crate's `RouteLimits`), so a project adapter cannot hold
  a request to them (`adapters.md` §4 step 4);
- a deadline on a project adapter's request: the engine sets none (`adapters.md` §9), so an
  adapter that never answers holds its call until the run is stopped;
- redacting an adapter host's standard error: a Python host's is inherited and a TypeScript
  host's copied line by line (`protocol.md` §1), neither redacted;
- private paths in a project adapter's reasons: `adapter_host.rs` redacts its reasons with the
  invocation's keys and the call's downloads only, never with the engine's private roots
  (`Engine::scrub`), and an `uncertain` submit's reason is kept as its job record's note;
- an adapter's identity for independent checkers: neither the graph nor `plan.json` records a
  declared adapter's capabilities, version or source digest, so `tools/digest.py` cannot check an
  unversioned adapter's routes, nor tell a capability the adapter serves from one it does not,
  and takes a versioned one's version from `fx.lock`, whose entry may be stale (`lock` never drops
  one): without `--current-lock`, a route that version does not complete is only a note;
- adapter drift in `doctor`: its `adapter` line does not say when a versioned adapter's source no
  longer matches `fx.lock`; planning reports it on the step;
- a live run's events in conformance: `budget_reserved` and `budget_settled` name the call's
  `node_id` as `<instance>/<invocation id>.<n>`, which `conformance/run.py` does not normalise,
  so `adapter-py` and `adapter-ts` pin the run's record through `project` instead of its
  `events.jsonl`;
- files in conformance's `never_mentions`: the runner checks only a command's output, so no case
  can show that a key is absent from a file the run writes (the events, a call record); the SDKs'
  engine tests (`python/tests/test_adapter_engine.py`, `js/fx/test/adapter-engine.test.ts`) read
  the files;
- `fx.lock`'s header: it says `the source behind each versioned node type` also when the file
  holds adapters, kept so that the lock fixtures do not move;
- `provider_options` in a project adapter: the engine passes the route's (in `call.route.contract`)
  and the step's (in `call.request`) as written, unmerged and unjudged, and neither SDK has a
  helper that merges this provider's member, the step's winning, and refuses a member the adapter
  sets, so each adapter writes its own (guide page 15's do);
- the cap note on a route whose adapter reads no cost: the plan's `note` for a capped
  OpenAI-compatible route says `… is charged what the provider reports, else the cap`, though the
  family never reads a cost, so each call is charged the cap;
- `download_hosts` takes an address: the schema and the parser accept `127.0.0.1`, which no
  download matches, since a download's host must be a name (a collect on a loopback fake that
  answered such a URL was `Unreachable`, `the download is not on acme's download hosts`);
- free routes and the ceiling: a live run whose calls are all on `usd: 0` routes, a model server
  on the user's machine say, is refused without a ceiling (`refused: a live run needs a ceiling:
  …`); only runs on Codex and Grok Build, which bill a plan, go without one (decision 5).

**gridaco/models**, gaps found against `v1.20261013.6`; `v1.20261014.1` fills the second (the
others still stand there, with the same schema):
- no answer shapes for fal's endpoints (only `grida` bindings have `result_schema_url`);
- no card for Gemini Omni Flash (filled in `v1.20261014.1`);
- no endpoint facts on the OpenAI binding;
- no mask facts (`v1.20261014.1` records one, on Ideogram 4.5's fal edit, none on FX's routes);
- GG's meters have no quantity per request;
- media prices sit outside the schema, and `token_pricing` cannot express image models;
- no ElevenLabs binding or US-dollar rate;
- `input_references` is typed as an integer on OpenRouter;
- no `size` on OpenRouter;
- `image_constraints` only on fal;
- no `references` (the number of input pictures) on 4 of its 17 OpenRouter image bindings;
- no `output_format` on OpenRouter;
- the rigging card's `input.formats` and a speech card's `input.audio_tags` are not
  schema-defined;
- no request parameters on the `grida` endpoints for images, video and 3D;
- Tripo's `/generation/multiview-to-model` is recorded with input `image`, where GG's multiview
  endpoint for the same model has `multi_image`.
