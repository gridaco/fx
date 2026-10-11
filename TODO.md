# TODO

Pending work and open design questions only. Supported behavior belongs in the
[guide](docs/guide/), [specifications](spec/) and [agent-readiness harness](AGENT_READINESS.md).

## Workflow and run viewer

- Improve layout for larger graphs, scalar and keyed-collection inspection, and
  judge/take history using the [canonical viewer harness](fixtures/viewer/README.md).
- Render workflow `view` metadata and custom workflow views. Specify any missing
  viewer contract before implementation, keeping presentation separate from
  execution and identity under [the store contract](spec/store.md).
- Define precise incomplete-state evidence where inspection needs more than the
  recorded contracts provide; the client must not infer missing execution metadata.
- Preserve imported-scope membership for unused planned takes in recorded views.
  Some skipped descendants currently appear beside the closed workflow because
  its recorded membership omits them. Keep that evidence intact until the engine
  supplies ownership; do not guess from display paths in the export client.

**Prior implementation reference only:**
[Stage Gen's viewer](https://github.com/softmarshmallow/stage-gen/tree/main/web/viewer)
and its run-view projection (`src/stage_gen/runview/`) demonstrate graph and artifact
inspection. Use them to inform the UX; FX's viewer must work independently, with no
dependency on that application's packages, catalog, branding or view-contract namespace.

### Observation and SDK readiness

- Consider SSE if polling becomes insufficient, building on
  [observation v1](spec/observation.md) and its
  [provider-free harness](fixtures/viewer/README.md#live-observation).
- Optimize retained-prefix validation for large histories.
- Design early SDK execution/viewer readiness for applications that need to
  report a running workflow's URL before their execution call returns.
- Ratify cloud-worker and remote-control boundaries if those uses become concrete.

### Project service

- Consider optional native OS supervision for crash restart and login activation
  under [the service contract](spec/service.md).
- Evaluate machine-wide aggregation, recent projects and project switching if
  demand grows.

### Agent-authored canvas layout

**Status: ratified design, 2026-10-09; Ship A in source, 2026-10-10, unreleased.**
[Canvas layout](spec/layout.md) owns the design; [AR-17](AGENT_READINESS.md#tidy-up-the-layout)
tracks the agent scenario. Remaining work, in order:

- **Ship A, what is left** ([layout.md §12](spec/layout.md#12-implementation-order)): run-view
  placeholders for absent, empty and pending slots; merged edges on a closed deck; linked
  decks; take decks nested inside item cards; nested expansion; double-click as one expand;
  accessible deck and card names; keyboard order inside an expanded deck; patching unchanged
  layouts in place and tracks that never shrink; the automatic refit and camera anchor; a
  browser check.
- **Ship B:** the adjacent `fx-layout-v1` file: reader, discovery, confinement and
  diagnostics; the report's file fields and `inspect --json` output; recorded matrix axis
  values; grid spreads and matrix tables; the conformance identity case; the guide, the
  skill and AR-17 evidence.

Deferred ([spec/layout.md §11](spec/layout.md#11-deferred)): per-instance keys; `defaults`,
`wrap` and `clusters` keys; pins; labels and descriptions; visual groups; an imported
workflow's own file; per-run files; a CLI option or URL parameter; a URL that selects one
member; a write or rename command; visual
editing; a minimap; typed SDK access to the layout report (callers receive it untyped in
`inspection`); ELK or any other router.

## SDK invocation result capture

Define invocation-pinned execution-result capture if needed. Existing SDK
`RunResult` construction reads the saved folder after the child exits, so an
immediate external resume can change the latest invocation before that read.
Keep this distinct from a saved `RunRecord`, which deliberately follows its
folder across invocations. [SDK run access](spec/sdk.md) documents the boundary.

## Caller-defined metadata on FX entities

**Status: idea; entity scope, API shape, and storage are undecided.** Consider
optional caller-defined metadata for associating FX entities with application
context. For example, a customer-facing service starts a workflow for a customer
request, then associates its run and result with a customer ID or request ID.
Applications can already maintain that association themselves; this would be an
ergonomic convenience for more flexible integration, not a required FX concept.

Defer whether metadata lives only in memory on SDK objects, is persisted with
records, or is also accessible through the CLI. Also defer field naming (such as
`user_metadata`), supported entities, and propagation between runs and results.
Keep correlation metadata distinct from execution inputs; define its lifetime
and visibility before introducing persistence or exposure in exported records.

## Run control: next operations

- Ratify contracts for explicit `terminate`, durable `pause`, and active parameter
  revision, building on [run control](spec/control.md) and its
  [user guide](docs/guide/08-run-control.md).
- Design bounded pruning for private temporary control receipts without removing
  evidence from an active bound waiter. Receipts currently last until system
  temporary storage is cleared.

## Run cleanup and retention

Listing and removing runs and pruning the cache are implemented in source
([store.md](spec/store.md) §9, [guide](docs/guide/10-cleanup.md)). Still open:

- **Pruning a shared cache.** A cache other projects use is refused until they are
  forgotten; pruning it for all of them at once (each project's runs as roots) is not
  designed.
- **Retention policies.** Keeping the newest N runs of a workflow, or defaults per
  project, are not designed; selection stays explicit.
- **Saved plans.** Catalog plan entries have no removal path.

## CLI update notices (npm and PyPI)

**Status: researched proposal, 2026-10-08; not implemented or ratified.**
Let installed CLI users know a newer suitable release exists, with a short update
hint. Notification only: no installation, downloads of executable code, version
changes, or automatic migration. Both launchers should follow the same policy.

### Research and recommended approach

- **Cached checks are established practice.** Node's
  [update-notifier](https://github.com/sindresorhus/update-notifier#how-it-works)
  checks npm in an independent background process, persists the result, and uses
  a configurable interval (one day by default). It supports TTY-only notices,
  CI suppression and opt-out. Treat it as a reference; evaluate its dependency
  footprint and process behavior before adopting it wholesale.
- **Python uses the same basic pattern.**
  [pip's self-check](https://github.com/pypa/pip/blob/main/src/pip/_internal/self_outdated_check.py)
  caches the result for seven days, compares parsed versions and avoids yanked
  or prerelease candidates. Pip's implementation is a precedent, not a public
  API to import or a requirement to copy its synchronous fetch behavior.
- **Keep installation knowledge in the launchers.** `js/fx/src/cli.ts` knows the
  npm package; `python/src/grida/fx/__main__.py` knows the Python environment.
  SDK imports/calls, node hosts, browser code and the Rust execution engine
  should not perform update checks. Python uses `os.execve` on POSIX: show a
  cached notice before handoff, and preserve existing signals and exit status.
- **Proposed first policy:** preserve FX's offline default. Start with explicit
  opt-in for automatic registry checks; once enabled, refresh at most once per
  24 hours. Whether interactive installations should eventually default to
  opt-out requires an explicit product decision. Registry checks are unrelated
  to paid-provider admission and must not depend on `--live`.

### Release selection

- **npm:** query public registry metadata for `@grida/fx`, use the `latest`
  dist-tag, compare with SemVer, and reject prereleases for the initial stable
  channel. A dist-tag is a publisher-controlled channel, not necessarily the
  numerically highest version; do not select the largest version string.
  [npm dist-tags](https://docs.npmjs.com/adding-dist-tags-to-packages/)
  and [registry API](https://github.com/npm/registry/blob/main/docs/REGISTRY-API.md).
  Do not recommend a release until its matching platform engine package is
  available; publication can temporarily expose the wrapper before all engines.
- **PyPI:** use `GET https://pypi.org/simple/grida/` with the JSON Index API
  accept header. Select the highest stable, non-development PEP 440 version
  with a non-yanked wheel compatible with the current interpreter/platform and
  `requires-python`. FX's platform wheels make availability relevant. The
  project JSON API's deprecated `releases` mapping and a bare `info.version`
  are unsuitable shortcuts for this selection.
  [PyPI Index API](https://docs.pypi.org/api/index-api/),
  [JSON API deprecations](https://docs.pypi.org/api/json/),
  [Python version parsing](https://packaging.pypa.io/en/stable/version.html),
  [wheel compatibility tags](https://packaging.pypa.io/en/stable/tags.html).
  Use supported version/tag parsers; do not import pip internals or handwrite
  version ordering. Evaluate the small runtime `packaging` dependency explicitly.
- Query the registry for the distribution actually launched. Keep npm and PyPI
  caches separate; releases may arrive at different times. Source checkouts,
  local/development versions and explicit engine overrides should skip notices
  unless their installation relationship is known. Prerelease channels are later
  work rather than an inferred subscription from a version suffix.

### Runtime and UX requirements

- Read a small per-user cache at launch; do not await registry access before
  starting the engine. A stale cache may schedule one bounded background helper
  only when checks are enabled. The next invocation uses the saved result.
  Proposed limits: two-second request timeout and five-second total helper
  lifetime; no service, persistent daemon or background retry loop.
- Cache registry/package/channel, installed environment context, candidate,
  check time and notification time. Use atomic writes and a nonblocking lock
  to avoid concurrent command storms. Bound stale notices (proposed: seven days),
  validate cache data and handle clock changes. If cache persistence is unavailable,
  skip automatic checks rather than request on every invocation.
- HTTP/TLS, DNS, malformed metadata, timeouts, unwritable caches and helper
  failures must never fail execution or change its exit status. Bound response
  size; require HTTPS; use no registry credentials or provider keys. Pass only
  the helper's required environment. Registry requests disclose ordinary network
  metadata such as the caller's IP; describe that behavior, not as zero telemetry.
- A compact notice goes to stderr only in interactive CLI use, at most once
  per day for the same candidate. Suppress both fetches and notices for CI,
  tests, SDK execution, machine-readable commands, help/version and explicit
  offline mode (`GRIDA_FX_NETWORK=off`). Support a shared opt-out such as
  `NO_UPDATE_NOTIFIER`; exact FX setting/flag names remain to be decided.
- Show an update command only when installation context is known: npm global
  versus project dependency, Python interpreter/environment, pipx or uv tool.
  When uncertain, provide a documentation link instead of guessing a global
  upgrade command. Do not invoke a package manager, read authentication files,
  or probe project/provider configuration to discover this context.

### Implementation order and acceptance

1. Settle enablement, configuration names, supported installation contexts and
   stable-channel rules. Keep this proposal separate from implemented support.
2. Prototype launcher adapters with injected registry responses and clock/cache;
   compare a small implementation with `update-notifier` for npm. No live registry
   access in tests and no notifier on SDK imports or node-host execution.
3. Verify cache hit/miss/expiry, concurrent starts, missing/read-only/corrupt cache,
   clock rollback, network failure, malformed/oversized metadata, version ordering,
   yanked/incompatible/missing-platform releases and unequal registry publication.
4. Prove stdout/JSON unchanged, no command-start network wait, existing
   cancellation/signals/exit codes preserved, helper bounded and no child leak.
   Test notice throttling, opt-out, offline/CI suppression and truthful update hints.
5. Update installed-user documentation and agent capability guidance only after
   implementation passes those checks. No release or publishing action is implied.

## Explicit AI node namespace before 1.0

**Status: proposal, 2026-10-08; naming decision and migration not yet ratified.**
Review built-in `uses` names before the first major release. Names such as
`fx/image.generate@1` should make AI-backed behavior explicit, while ordinary
code-only image/media operations keep neutral names. An AI namespace does not
change live admission, prices, provider selection or cache-reuse behavior.

**Candidate:** `uses: fx/ai/image.generate@1`, with the same convention applied
consistently to the relevant AI generation/editing node types. Keep the required
node-major suffix; the namespace change and node major are separate decisions.

- Check established namespace conventions and usability before choosing nested
  slashes versus an `ai` prefix within the existing dotted name. These are FX
  node identifiers, not npm package names or filesystem paths; conventions must
  be evaluated at the correct boundary rather than assumed interchangeable.
- Current built-in resolution accepts `fx/<dotted_name>@<major>` and does **not**
  accept an additional slash in that name (`crates/grida-fx-core/src/registry.rs`,
  `BUILTIN`). Therefore the candidate requires an intentional grammar/resolver
  change, not merely renamed examples. Review schemas, catalog lookup, CLI
  discovery, SDK authoring, recorded types and viewer display together.
- Inventory the built-ins and define exactly which belong under `ai`; do not
  label every media operation AI or leave generation/editing families inconsistent.
- Specify old-name aliases/deprecation or an explicit breaking migration before
  implementation. Type names enter step and plan identity: assess cache reuse,
  saved-run resume, takes, locks and portable records under the identity contract.
  Do not silently rewrite historical records or promise aliases preserve every
  authored-plan digest. Update the spec before changing identity behavior.
- Update canonical examples, installed skills, user documentation and conformance
  cases together; test old and new spellings according to the chosen policy.
  Settle and implement this naming review **before FX 1.0**, so the first major
  release has a deliberate, stable vocabulary. No rename is authorized by this TODO.

## Optional local agent nodes (ACP, Codex, Claude)

**Status: proposal, 2026-10-08; first-class support not implemented.**
Support an explicitly configured workflow step that delegates to a locally
installed agent, through a child process or an ACP adapter. Codex and Claude
are initial candidates. This is optional local-user functionality; ordinary
workflows must not assume an agent is installed, authenticated or has quota.
Initial support is not intended for CI, servers or unattended deployment.
Local here describes where the agent process runs, not offline inference. The Codex image
route (below) already speaks `codex app-server` from Rust; an agent node can reuse that client.

### Purpose and existing extension path

- Tasks range from complex agent work in the middle of a workflow to a single
  classification, structured answer or yes/no decision. Do not require a heavy
  autonomous task or grant filesystem/tool access for a simple question.
- Users can already implement a small custom node around a declared external
  tool (`tools` and `ctx.tool(...).run(...)`; see
  [node authoring](docs/guide/03-nodes.md)). A basic subprocess wrapper is short;
  first-class support should standardize configuration, lifecycle, validation,
  failure reporting and observation rather than invent another workflow engine.
- Keep this distinct from FX's existing engine-owned `agent.run`/`agent.turn`
  capability and its provider adapters. This proposal delegates execution to an
  external agent runtime with its own authentication, tools and usage limits.
- A user's included subscription allowance may avoid separate per-token API
  charges, depending on the selected agent, account and supported authentication.
  It does not reduce tokens inherently, guarantee savings or make the task free.
  Remaining credits and eligibility must not be assumed.

### First-class contract to design

- Explicit agent/adapter selection, executable/version discovery, prompt and
  inputs, expected output schema, timeout and narrowly scoped workspace/tool
  permissions. Keep node names provisional and coordinate with the AI namespace
  review above. Do not silently substitute another agent or paid API provider.
- Offline planning may describe dependencies and configuration but must not
  launch an inference task, consume quota, sign in, or request account changes.
  Execution needs explicit external-agent admission; choosing its exact flag or
  configuration is design work. Ordinary offline runs must not unexpectedly
  start a networked agent. Optional checks must not expose credentials.
- Missing executable/adapter, authentication required, expired session,
  unavailable model, quota exhaustion, permission request, timeout and invalid
  output need clear, distinguishable failures. Presence of a binary or successful
  preflight cannot prove usable quota at execution time. Users perform login;
  FX does not read, copy, log or store agent credential files.
- Define headless versus user-assisted permission behavior. Never silently bypass
  approval controls or wait indefinitely for an invisible prompt. Simple
  classification/yes-no use should default to no write/tool permissions. Establish
  local execution eligibility explicitly; CI environment heuristics alone cannot
  identify every server or establish user consent.
- Bound process/session lifetime, cancel with the FX invocation, clean up child
  processes and reject partial answers as successful outputs. Prefer supported
  structured output or ACP events over parsing decorative terminal text. Capture
  only useful, redacted task evidence; do not persist raw absolute paths or
  credential-bearing transcripts.
- External agent retries, side effects and inference are not owned by FX's paid
  call retry/ledger today. Ratify admission, external usage reporting and retry
  rules before claiming support: `--max-usd` must not be advertised as enforcing
  another runtime's subscription/API spend. Unknown external usage stays unknown,
  not a booked $0. Do not automatically repeat a quota/auth failure or retry a
  partially executed agent task without an explicit idempotency policy.
- Specify identity and caching honestly: agent version/configuration, declared
  inputs and allowed external context affect reproducibility. Do not reuse a
  result merely because its prompt matches when the agent read mutable files,
  conversation state or network resources. Session reuse/resume is later scope
  unless separately specified.

### References and implementation sequence

References checked 2026-10-08:
[ACP overview](https://agentclientprotocol.com/protocol/v1/overview) defines
initialization, authentication, sessions, updates, permission requests and
cancellation. It is a separate external protocol from FX's node-host JSON-RPC;
preserve ACP's own wire vocabulary at an explicit adapter boundary.
[Codex non-interactive mode](https://developers.openai.com/codex/noninteractive)
and [Claude programmatic use](https://code.claude.com/docs/en/headless) provide
CLI integration references. ACP wrappers such as
[codex-acp](https://github.com/zed-industries/codex-acp) and
[claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp) are
separate dependencies, not proof that every installed CLI speaks ACP natively.
Recheck supported adapter versions, account modes and usage semantics before
implementation; installation alone does not authorize invocation.

1. Ratify the minimal local-agent node contract and external-usage boundary.
2. Prove a simple classification and a bounded agent task with fake CLI/ACP
   processes, including missing auth/quota, permission handling, malformed output,
   cancellation and timeout. No real account access or token spend in tests.
3. Ship one minimal adapter, then add the other through the same contract. Choose
   direct CLI versus ACP based on reliable structured output and lifecycle needs;
   do not build a universal agent orchestrator as the first step.
4. Expose declared outputs and honest external execution/usage status through the
   existing run/view contracts. Update installed docs and agent-readiness guidance
   only for supported behavior. Real-agent validation requires separate explicit
   authorization and a bounded usage/spending allowance.

## Codex-backed image generation for local workflows

**Status: in review, 2026-10-12** ([gridaco/fx#46](https://github.com/gridaco/fx/pull/46)), as
the route `imagegen@codex` of `image.generate`
([spec/providers.md §9.6](spec/providers.md#96-codex)). Users signed into Codex with ChatGPT pick
it in `fx.yaml` and run with no provider key. `codex` is an *included provider*: a live run whose
calls are all on it needs no `--max-usd`, runs under a ceiling of $0 and refuses any other
provider. The adapter speaks `codex app-server` JSON-RPC (its typed `imageGeneration` item
carries the image), admits only a ChatGPT sign-in (`account/read`), turns off the user's MCP
servers, apps, plugins, hooks and memories, pins low effort and the standard tier, runs Codex in
its own process group that FX always ends, and never resends a call. Verified with fake
app-server processes in tests and real runs on Codex 0.160.1: transparent and opaque backgrounds
are honoured; exact sizes are not (Codex draws about 1.57 MP in a shape it picks), so they are
refused.

How this meets the local-agent requirements above, and what it defers:
- **Admission (owner decision).** `--live` alone admits Codex; the skill asks agents to get the
  user's go. An explicit opt-in (`--allow codex`, or a project setting) is the alternative.
- **Usage is unknown, not $0.** Answers report no cost (`None`); the $0 hold is charged and the
  record says *not reported*. Usage evidence (tokens, `account/rateLimits` `usedPercent`) and
  provenance (Codex version, the model Codex chose) have no record field yet; `data` cannot hold
  them (`image.generate` answers `data: null`).
- **Attempts (owner decision).** The adapter never asks for a resend, but a failed answer check
  is still a new engine attempt, and each is a real image on the user's plan. A per-provider
  attempt policy (included providers: at most 2) needs the retry owner's spec.
- **Identity.** The route's model `imagegen` names the tool; the cache key ignores the Codex
  version, the model and the quality Codex chose. Documented as a moving target.
- **Decision 5** (docs/wg/overview.md) carries the included-provider exception, awaiting
  ratification.

Follow-ups:
- **An external-runtime adapter kind.** Codex is fitted into the route/adapter seam meant for HTTP
  clients billed in dollars. A runtime kind (engine-owned process lifecycle, `External` billing
  with usage evidence, its own admission and attempt policy, a runtime-chosen model with reported
  provenance) is the general shape; the ACP/agent node above should share it.
- **References / `image.edit`.** The app-server takes `localImage` inputs; prove that `$imagegen`
  edits from them before declaring `image_input` or serving `image.edit`.
- **Plan-time feature derivation.** A `WxH` size implies `exact_size` and references imply
  `image_input`; today they are refused only when the call is made (every route would gain).
- **Provider-level concurrency.** One Codex per machine and account; route concurrency is
  per route and per run.
- **Windows.** npm installs `codex.cmd`, which `find_program` does not look for; process groups
  are Unix-only; the tests use shell fakes. A launcher trait with an in-process JSON-RPC fake
  would make the tests portable and let the runner itself be tested (check retries, cancel).
- **A missing Codex is refused per call**; refusing it before the run, like a declared tool, is
  kinder.
- **Showcases.** Advertise the Codex option in examples whose image steps need no exact size or
  references (most set `size`).
- **Shape hints.** Codex draws a fixed budget of about 1.57 megapixels in a shape the prompt
  steers (1254x1254, 1024x1536, 1774x887, 1672x941; its tool has no size argument:
  openai/codex#19175, #28723). An orientation or aspect setting could map onto that honestly;
  `image.generate` has none today. Revisit if Codex adds a size argument.

### Deferred backends: Grok Build and Cursor

Keep Codex as the first implementation target. Grok Build and Cursor are
explicitly deferred candidates for the same local image-backend contract,
not approved implementation scope. Research checked 2026-10-08:

- **Grok Build:** its official [CLI changelog](https://x.ai/changelog/build)
  documents built-in image and video generation tools. The
  [CLI overview](https://docs.x.ai/build/overview) documents browser sign-in,
  headless execution and streaming JSON output. Verify image-tool availability
  in headless mode, reliable artifact extraction, account eligibility and usage
  accounting before promising a no-BYOK/included-allowance backend. Video support
  is evidence of a capability, not additional FX implementation scope here.
- **Cursor:** image generation is a native Agent tool, with project files as
  outputs; no separate image extension or provider API-key setup is documented
  in its [announcement](https://cursor.com/changelog/page/12) or
  [Agent overview](https://cursor.com/docs/agent/overview). Its
  [CLI changelog](https://cursor.com/docs/cli/changelog) also documents image
  generation failures. Verify headless invocation, tool permissions, output
  delivery and image-specific plan/billing behavior. Remaining credits alone do
  not guarantee successful generation or establish exact billing semantics.

Design the initial contract to allow explicit backend selection without
hard-coding Codex as the only possible implementation. Add these adapters only
after the Codex integration is proven and each candidate meets the same
lifecycle, capability, validation and honest usage-reporting requirements.
