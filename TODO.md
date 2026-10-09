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
- Share browser components for the future website and design portable snapshots
  that work with a local file server or any selected storage/CDN provider.

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

**Status: agreed direction, 2026-10-08; schema and operations remain proposed,
not implemented or ratified in `spec/`.** First serve requests made through an
external agent: "Tidy up the layout" or "Place the comparison below the variants."
Interactive node dragging and other visual editing are deferred. See the scenario
and AR-17 in [Agent readiness](AGENT_READINESS.md#tidy-up-the-layout).

#### Authoring and ownership

- Use an optional adjacent **JSON** file, for example `gallery.layout.json`
  beside `gallery.yaml`. Treat it as shareable, version-controlled authored
  presentation data. Start with one format; YAML/JSONC support is not required.
  Missing layout metadata means automatic layout.
- Define a versioned JSON Schema with a small, engine-independent vocabulary for
  grouping, ordering and supported placement constraints. Computed coordinates
  are derived data. Explicit labels/descriptions should survive agent and future
  visual-editor round trips; arbitrary executable layout code is out of scope.
- Keep layout separate from the workflow definition, execution dependencies,
  scheduling, cache identity and recorded history. The current
  [plan digest](spec/identity.md#10-plan-digest) includes authored workflow
  documents: adding inline layout fields would affect resume compatibility unless
  identity semantics were deliberately revised. A visual edit must not require a
  new run or invalidate resume.
- Specify predictable adjacent-file discovery and an explicit layout-file option
  for YAML, supported builders and saved records. Settle builder naming and
  imported-workflow composition before implementing discovery. Target recorded
  source/entry-point identity, not a display title or workflow ID alone.
- Address authored steps within a scope; distinguish rules for all repeated
  instances from an override for one recorded instance. Use recorded identifiers
  rather than deriving them from labels. Visual groups must not create execution
  groups. Keep authored rules stable across changing matrix/repeat cardinality.
- The adjacent file is the workflow default. Design explicit per-run overrides
  only where needed, with visible scope and precedence; avoid a broad inheritance
  system initially. Future portable snapshots should carry the selected layout.
  Historical runs can differ from current source: report unmatched rules and
  provide a usable viewing fallback without rewriting execution evidence.

#### Agent operations and bounded implementation

1. Inspect a saved graph/run and its effective layout, returning addressable nodes,
   scopes, supplying file and layout revision. This path must not execute author
   code, replan, start workflow work or call providers.
2. Let agents edit the JSON file with ordinary file tools. Validate schema,
   references and supported constraints through structured diagnostics; exact CLI
   syntax remains a contract decision, not a currently supported command.
3. Refresh the existing view and expose which layout revision was loaded, the
   actual viewer URL, and renderer diagnostics. Distinguish structural validation,
   successful geometric layout and human visual acceptance. If the solver runs in
   browser JavaScript, native CLI validation alone cannot claim it ran that solver.
4. Preserve arrangement during status-only updates. Prototype against branching,
   imported scopes, inline groups, matrix/repeat changes and variable preview sizes.
   Prove that layout-only edits preserve plan/step identities and resume, and that
   stale or conflicting rules produce actionable results without disturbing runs.

Start with one schema, discovery convention, explicit selection, validation and
viewer refresh. Prefer a testable TypeScript adapter around an existing layout
engine; defer mouse editing and a command for every individual arrangement action.
Evaluate **ELK layered** as the leading candidate for ports, nested graphs,
partitions and ordering. It is not a general solver for every relative constraint;
prototype the exact supported combinations before ratifying the vocabulary.
Full relational constraints, position pins and rules over arbitrary node sets
remain later candidates rather than prerequisites.

References from the design discussion:

- [Blender frames](https://docs.blender.org/manual/en/latest/interface/controls/nodes/types/layout/frame.html)
  for visual grouping, and [Houdini arrangement](https://www.sidefx.com/docs/houdini/network/layout.html)
  for targeted branch layout that preserves the surrounding arrangement.
- [ELK layered](https://eclipse.dev/elk/reference/algorithms/org-eclipse-elk-layered.html)
  and [elkjs](https://github.com/kieler/elkjs) for the initial engine evaluation.
- [WebCola constraints](https://github.com/tgdwyer/WebCola/wiki/Constraints)
  and [fCoSE](https://github.com/iVis-at-Bilkent/cytoscape.js-fcose) for richer
  alignment/relative-placement alternatives; [SetCoLa](https://idl.uw.edu/papers/setcola)
  for applying rules to sets, potentially useful for repeated instances.

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
Local here describes where the agent process runs, not offline inference.

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

**Status: high-priority proposal, 2026-10-08; not implemented or runtime-proven.**
This is a separate product capability from the general local-agent node above.
Eligible users already signed into Codex could try supported FX image workflows
using their included image-generation allowance, without supplying a provider
API key or entering a credit card for a separate API billing account. This is
an important onboarding opportunity: some otherwise paid showcase workflows
could have no additional API charge from those users' perspective. Do not
promise unlimited free execution, account eligibility or remaining allowance.
Initial scope is optional local Codex use, not CI or servers. Other agents are
outside this initial scope; that is not a claim that none can generate images.

### Minimal research and proof still needed

Official [image-generation documentation](https://learn.chatgpt.com/docs/image-generation)
explicitly supports Codex CLI image generation and invocation through
`$imagegen`. It currently identifies the built-in model as `gpt-image-2`, not
`gpt-image-latest`, and says image generation counts toward general Codex usage
limits, consuming included limits 3–5 times faster on average than similar
non-image turns depending on quality and size. Treat model identity and usage
rules as version-dependent evidence, not a permanent FX contract.
[Authentication](https://learn.chatgpt.com/docs/auth) distinguishes ChatGPT
subscription access from separately billed API-key access.
[Non-interactive mode](https://developers.openai.com/codex/noninteractive)
documents `codex exec`, JSONL events and schema-constrained final responses.
These establish useful integration pieces; they do not establish a stable
headless image-file return contract. Verify `$imagegen` availability under
`codex exec` for supported versions/accounts and identify the actual generated
file/tool result before advertising FX support. No inference was invoked for
this research, and no authentication files were read.

### Proposed integration

- Organize this as an explicitly selected image-generation backend beneath the
  proposed AI node family (`fx/ai/image.generate@1`, spelling still provisional),
  rather than requiring showcase authors to disguise image generation as a
  generic agent task. Review route/backend configuration with the AI namespace
  proposal; do not force subscription execution into direct-API price semantics.
- Wrap the authored prompt with a versioned instruction, for example:
  "Use $imagegen to generate an image with the prompt below." Keep the user's
  prompt distinct from wrapper instructions, request the declared image count
  and output location, and prohibit API fallback or unrelated tool work.
  A prompt alone cannot guarantee tool selection or successful generation.
- Prefer structured tool results or generated files plus a validated manifest
  over prose/terminal parsing. A schema-constrained answer containing a path is
  only a locator, not proof of an artifact. Copy only verified image outputs
  from a confined task workspace; reject missing files, traversal, symlink
  escapes, unrelated files and text/code returned in place of generated images.
  Validate decoding, media kind, count, dimensions and required alpha/content
  constraints before atomically admitting outputs to the FX store.
- Advertise only controls the adapter can actually honor. Reference inputs,
  editing, exact sizes, transparency and other requirements need individual
  verification; reject unsupported requirements while planning. Prompt-based
  guidance must not masquerade as exact provider API parameters.
- Use existing external-agent lifecycle boundaries: explicit execution opt-in,
  user-managed login, bounded permissions, cancellation, timeout and clear
  missing-tool/auth/quota failures. Never silently switch to API-key billing,
  purchase credits, retry uncertain generations or report unknown external
  usage as a settled $0. FX's dollar cap does not enforce Codex allowance.
- Include the backend and prompt-wrapper version in identity/provenance under
  the ratified spec. Record actual supported model/tool evidence when available;
  never infer a hidden model from an alias or claim direct-provider equivalence.
  Reuse the public artifact/run/view contracts for downstream steps and previews.

First prove extraction and lifecycle with fake CLI events/files and invalid
output cases. Then perform one separately authorized, bounded real Codex CLI
image generation to establish headless support, output delivery and observed
usage. Only after that proof should supported showcase workflows advertise the
included-allowance option. No implementation, live generation or release is
authorized by this TODO.

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
