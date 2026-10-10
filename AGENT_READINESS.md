# Agent readiness

FX should let an agent translate a person's changing intent into understandable,
composable operations: discover, author, plan, execute, observe, revise and deliver.
The person should not need to know FX's internals to ask for a change. The agent
must be able to determine what is supported, what an operation changes, and what
to do next from the public interface and its results.

This is a living product and engineering harness. It records current behavior,
gaps and scenarios against which we design FX. It is not a claim that every
scenario below works today, nor approval to implement every proposed capability.

**Last source review: 2026-10-09.** Current entries were checked against the CLI,
SDKs and specifications. The cancellation delivery includes provider-free execution evidence from
`tools/check_control.py`; run listing and removal rest on the `runs` CLI tests, the
`runs-remove` and `cache-prune` conformance cases and the SDK tests; other capability status rests on the evidence
listed below. No live providers were used.
Recheck the installed version before applying them to a user's installation.

## Ownership and status

- [spec/](spec/) owns normative contracts. Ratify a contract there before changing
  execution, identity, lifecycle or public protocol semantics.
- This document owns agent-facing vocabulary, operation composition, capability
  gaps and acceptance scenarios. A scenario becomes supported only with an
  implemented public surface and evidence.
- [The installed skill](skills/grida-fx/SKILL.md) teaches agents how to use what
  ships. It must not teach a proposed command as if it exists.
- [The guide](docs/guide/) explains supported usage;
  [the overview](docs/wg/overview.md) records decisions and milestones.

Use these statuses when extending this document: **current** means implemented
in the reviewed source; **partial** means only part of the intent is supported;
**ratified design** means an agreed contract whose implementation is pending;
**proposed** means a use case whose mechanics and scope still need a decision.
Existing tests are starting points for evidence, not proof of every scenario here.

## Vocabulary agents should be able to use precisely

| Term | Meaning and distinction |
|---|---|
| Agent / caller | The external assistant or application operating FX. This is separate from an `agent.turn` node inside a workflow. |
| Workflow definition | Authored steps, inputs, dependencies and output declarations, supplied as YAML or through supported SDK authoring. |
| Plan | FX's resolved view of that definition and its inputs: known instances, dependencies, problems, prices and unresolved work. Planning is not execution approval. |
| Run | The recorded execution history in a selected run folder, bound to a plan. A fresh run is created by default; cached work can still be reused. An optional name labels this record. |
| Run name | A case-sensitive create-only label within one project and recorded workflow ID/source. It is presentation metadata, not a cache key or workflow version. |
| Invocation | One execution attempt of that run. Resuming appends a new invocation to the same history. |
| Step / instance | A step is authored; an instance is a concrete occurrence after repeats, takes and nesting. Use recorded identifiers instead of reconstructing them from UI labels. |
| Artifact | A recorded output with a content identity. A path alone is not proof of success or valid content. |
| Cache reuse | Reuse under FX's identity and validation rules. The engine decides which work matches; the agent does not manufacture cache keys. |
| Retry | Another engine-owned attempt within execution. It is not a new run, resume, or request for an artistic alternative. |
| Provider call / attempt / job | A call is the requested capability work; an attempt is one engine-accounted try; a job is a provider operation that may remain pending beyond a local request. An unknown job outcome is not permission to submit it again. |
| Resume | Execute the same recorded plan again in its run folder, preserving recorded work and reusing valid results. It does not authorize changing that plan in place. |
| Cancel | Ask the active invocation to stop. Completed work remains; local accounting can conservatively charge a full reservation without knowing the provider's final outcome. Exact-target CLI/Python/JavaScript control is implemented in source under spec/control.md. |
| Terminate | Proposed explicit force operation with weaker cleanup guarantees; stopping local execution cannot prove remote termination. |
| Pause | A future, explicitly defined suspension boundary from which work can continue. There is no pause operation today; cancellation is not a pause acknowledgment. |
| Revise | Change authored inputs or workflow choices and plan again. Today this generally needs a new run folder; valid unchanged work can still be reused from the same cache. |
| Take / reroll / pick | A take identifies an alternative. Reroll selects a later take for the next execution; pick selects a take. Neither command executes it immediately. |
| Observe / inspect | Read execution evidence or inspect a saved run; `--open` can present it in the browser. Neither is permission to mutate the run. |
| Service | A project-scoped process started by `start`, providing the dashboard, catalog and observation HTTP endpoints. Workflow execution has a separate owner and lifetime. |
| Standalone | Independent inspection with its own available port and foreground owner. It does not mean a fresh cache or different execution semantics. |
| Project / projection | `init` establishes the recommended `fx.yaml` project; standalone use remains supported. The unrelated `project <run>` command projects recorded events into state. |
| Remove a run | Delete a run's folder and its record. The cache keeps the results it used, so removing frees little disk and costs no reuse. Distinct from cancelling a run, and from pruning the cache. |
| Prune the cache | Delete from the cache what no remaining run of the project names, to get disk back. A later run that needs a removed result makes it again, and pays again for a removed paid answer. Refused while another project uses the cache or anything is still running or unreadable. |

The interfaces should preserve these distinctions. Avoid using one word such as
"restart" for resume, fresh generation and revision: they have different effects
on work, history and spending.

## Current operation map

Commands below use the npm launcher `grida-fx`. The Python equivalent is
`python -m grida.fx`; it runs the same engine. Users install the packages rather
than cloning the repository. Confirm `--version` and the relevant `--help` first.
`<target>` means a workflow file, workflow id, or supported Python builder target;
individual commands can have narrower target rules.

| Intent / tool | What an agent receives | Conditions and effects |
|---|---|---|
| Discover: `--version`, `--help`, `<verb> --help` | Installed version and human-readable command help | No common machine-readable capability manifest yet. |
| Establish project: `init [directory] --json` | Resolved project and initialization outcome | Creates minimal configuration without overwriting an existing file. No dependencies, service, credentials or execution. |
| Start service: `start --background [--project directory] [--port N] [--open] --json` | Ready service identity and actual URL | Native background process, port 8787 by default. Matching startup is idempotent; readiness checks project, instance and protocol. Does not execute workflows or install crash/login recovery. |
| Manage service: `status --json`, `logs --lines N`, `stop --json` | Service identity/status, bounded logs, shutdown outcome | Select the project explicitly when needed. Stop affects inspection only; runs and records remain. Plain `start` stays foreground. |
| Discover built-ins: `nodes [TYPE]` | Text descriptions, settings, ports and applicable routes | Lists built-in types, not a complete custom-node registry. Unknown types currently print nothing with exit 0. Listing a type does not prove every advertised capability can execute. |
| Discover workflow inputs: `schema <target>` | JSON Schema for authored inputs | May load author code to resolve a target. |
| Validate: `plan <target> ... --check --json` | Expanded graph including problems; nonzero validation status | No FX paid calls; may import code, invoke builders, execute local `at: plan` nodes and write cache results. `--json` alone does not make plan problems a failing exit status. |
| Inspect planned work: `expand`, `identity`, `price <target> ...` | JSON graph, instance identities, or phased estimates | Planning conditions apply. Unresolved work and estimates are not proof of final work or actual billing. |
| Diagnose: `doctor [target]` | Text prerequisite, tool, route and key-presence diagnostics | Let FX inspect credential configuration; agents must not read secret values. Missing keys alone do not make its exit status fail. It does not make provider calls and is not a provider health check. |
| Execute: `run <target> ... [--name NAME \| --run <folder>]` | Foreground execution, an early run URL when inspection is available, and final summary | Local code needs no `--live`; paid calls need admission and a ceiling, except cached answers. Choose a name or explicit folder when supervising asynchronously. `--name` refuses collisions; neither naming nor a fresh record forces fresh generation. Project run pages survive completion through the separately started service. `--open` requests browser launch; `--standalone` owns a temporary viewer instead; `--no-view` skips integration and conflicts with `--open`. |
| Observe: `observe <run> --snapshot`, `observe <run> --after <cursor>`, `project <run>`, `inspect <run> --json` | Consistent snapshot, bounded event batches, state projection or summary | Read recorded data without executing workflow code. Use a precise folder or `inspect <workflow-id>/<name>`; `inspect <workflow-id>` chooses the newest-created run, including failures, and refuses ambiguous recorded sources. Keep the emitted folder for observation commands. |
| Control: `inspect RUN --control --json`, `cancel RUN [--invocation ID] [--wait] [--timeout 30s] --json` | Selected invocation, verified availability, acceptance or local completion | Exact folder or WORKFLOW_ID/NAME only. No author code, service or provider access. Wait never forces; external completion stays unverified. Python inspect_control/cancel and JavaScript inspectControl/cancel preserve the same structured result. |
| Verify: `inspect <run> --verify --json` | Recorded summary with placed-file verification | Checks the recorded bytes, not artistic quality, external side effects, or the accuracy of a provider bill. |
| Open browser: `plan <target> --open`, `inspect <run> --open` | A selected plan/run page in the running project service | Explicit project startup required. Source targets plan once; saved runs read records. `--standalone` hosts independently until interrupted; projectless browser inspection does this by default. Run pages follow recorded events and resumes; plans stay static. `view` remains hidden compatibility syntax, including saved-plan files. |
| Resume: `run <target> ... --resume NAME`, or repeat `--run FOLDER` | Another invocation in the existing folder | A named resume requires an existing name; repeat current inputs/routes/takes/builder arguments explicitly. Requires the same plan digest and stand-in mode, and no other writer. A changed plan is refused rather than silently replacing history. Recorded spend and original creation time carry forward; resume does not reset the budget ledger or promote the record as newly created. |
| Choose alternatives: `reroll <run> <step-path>`, `pick <run> <step-path> <take>` | Takes-file mutation and advice | Affect a later plan/run, not the active scheduler; both refuse stand-in runs. Reroll's `--live` only changes printed advice. Picking may still require generation of the selected take or downstream work. |
| Maintain takes: `takes list <target>`, `takes mv <target> <old> <new>` | Text listing or takes-file edit | These targets are workflow files/ids, not builder targets. Moving refuses an existing destination entry. |
| Clean up runs: `runs list --json`; `runs remove RUN… [--yes] --json` or `runs remove --workflow ID --state STATE --before DATE [--yes] --json` | The project's runs with placement and recorded state; what a removal would do, or did | Listing reads only. Removal previews until `--yes`; filters take dated runs only, never named, `--run` or external ones. Refuses a selection that is not a run (exit 2, nothing removed), a run an invocation holds (`active`) and the last run holding a pick (`holds_pick`). Removed files are mostly links into the cache, which keeps them. Python list_runs/remove_runs and JavaScript listRuns/removeRuns take explicit runs. |
| Reclaim disk: `cache prune [--forget-user ID] [--yes] --json` | What would be (or was) removed per store, with the cost of the removed paid answers; or the refusals | Previews until `--yes`. Keeps everything a remaining run names; refuses (`shared_cache`, `job_outstanding`, `cache_in_use`, unreadable runs, interrupted older runs) rather than guess, and removes nothing then. `--forget-user` is the explicit statement that another project no longer uses the cache. CLI-only. |
| Diagnose jobs: `jobs`; reconcile: `jobs --forget <key>` | Text job records; explicit deletion of a job record | Forgetting can permit a fresh paid submission. Use only after reconciling the provider outcome, not as generic failure recovery. |
| Maintain node locks: `lock [where] --check` / `lock [where]` | Lock verification / lockfile updates | `--same` asserts unchanged behavior; it is not a way to silence a real identity change. |
| Deliver: `run ... --deliver output=path` or SDK result delivery | Copies of available declared outputs | CLI delivery happens after execution and reports missing outputs. Preserve immutable store/placed files; copy out before editing bytes. |

Python and JavaScript SDKs expose planning, named execution/resume, result files
and delivery. `load_run` / `loadRun` return a saved `RunRecord` with a pinned
folder and engine inspection; its snapshot/events/follow methods observe through
the public reader. Python provides sync/async reads and an async iterator;
JavaScript provides promises and an async iterator. JavaScript's existing `project`
and `inspect` exports remain. Check `RunResult.ok`, incomplete/stopped state and
failures: a failed step can return a result rather than throw. See
[the SDK guide](docs/guide/09-sdk-runs.md) and [contract](spec/sdk.md).

SDK coverage follows useful jobs, with language-native objects rather than a
wrapper for every CLI verb. `Plan` already exposes graph/problems/pricing;
results expose files/delivery. Project setup/service management/browser opening,
diagnostics, node locks and take/job maintenance intentionally remain CLI-only
(run listing and removal are SDK operations: `list_runs` / `listRuns` and
`remove_runs` / `removeRuns`, with explicit runs; pruning the cache stays CLI-only)
until a concrete programmatic use needs a native lifetime/error design. JS callable
stand-ins remain deferred; Python already supports them. Pause/active revision
stay separate contracts. Async completion alone supplies no readiness handle.

For CLI orchestration, status 0 describes the command's success, not every
possible user goal. `run` requires an ok run and successful requested delivery;
status 1 covers refusals, unsuccessful/incomplete runs and missing deliveries;
2 covers input/usage and fatal engine errors; 130 indicates interruption. Inspect partial records
when a run exists. Do not depend on an English error sentence as a stable error
code; machine-readable errors are not uniform across commands today.

## Composing operations from user intent

### "Make this, and let me inspect it"

Resolve the installed interface and inputs, author the workflow, validate and
price it, then execute within the user's authorized scope. An illustrative local
sequence, with paths replaced by the user's actual files, is:

```sh
grida-fx init
grida-fx start --background --json
grida-fx status --json
grida-fx schema workflows/example.yaml
grida-fx plan workflows/example.yaml --inputs inputs.yaml --check --json
grida-fx run workflows/example.yaml --inputs inputs.yaml --run runs/example --open
grida-fx inspect runs/example --verify --json
```

These are individual operations, not an unconditional script. Resolve validation
problems before running and inspect failures before claiming success. Project
initialization is safe to repeat, and background startup returns after verified
readiness. Preserve existing project settings rather than replacing them.

`run` is foreground and non-interactive. With a running project service, it reports
an actual usable run URL after initialization and before run-phase execution.
An agent can relay it immediately while supervising execution; planning-time code
precedes it. The service keeps that page available after completion and across
service restarts while the record is retained. `--open` requests browser launch;
otherwise report the printed URL. Do not synthesize a URL from filesystem paths.

Commands never start a background service implicitly. An absent project service
does not prevent execution; `--open` reports the startup prerequisite. For a probe,
`run --standalone` keeps the current independent viewer lifetime: the URL ends with
that invocation. Projectless runs use standalone behavior by default. This flag
does not clear caches or force fresh work. `inspect RUN --standalone --open`
independently serves the saved result until interrupted.

Use `stop` to end the project's inspection service. To stop execution, use
`cancel RUN`, optionally with `--wait` and an inspected invocation guard. Detached background operation is not automatic
crash restart or login/boot activation. `status --json` reports service liveness;
run events report execution evidence, and these are separate observations.

`observe RUN --snapshot` attaches to a consistent recorded prefix. Continue with
`observe RUN --after CURSOR`, applying the response before saving its returned
cursor. Catch up while `has_more` is true. A lost response can be replayed from
the prior cursor. `run_finished` terminates one invocation; keep following to see
a later resume. Unfinished evidence does not establish process liveness.

Python and JavaScript SDK runs suppress hosting and return after execution.
Their `RunRecord` supports independent live observation of an initialized folder:
load once, apply its snapshot, then follow after that exact cursor. Iteration
continues across terminal events and resumes until its consumer stops; cancelling
that observer does not cancel the separately owned run. Missing records and invalid
cursors are explicit errors. Inspection and snapshot are independent samples.
The SDK has no early execution/viewer readiness handle; use the CLI to provide
an early browser URL. Do not promise these source APIs in an older installation.

### "Stop here; continue later"

**Current source:** select an exact run folder or `WORKFLOW_ID/NAME`, inspect with
`inspect RUN --control --json`, retain its invocation, then request
`cancel RUN --invocation ID --wait --json`. Bare workflow IDs never select a run.
[Run control](spec/control.md) defines outcomes and the
[user guide](docs/guide/08-run-control.md) shows CLI and both SDKs.

Acceptance closes admission and persists `cancel_requested`; completion additionally
verifies local bookkeeping, owned host cleanup and writer release. Wait timeout or
interrupted control clients never grant force authority or retract acceptance.
Repeated requests are idempotent and a bound waiter never retargets a newer resume.
The viewer's cancelling label is recorded intent, not proof of owner liveness.

First SIGINT/SIGTERM and SDK run cancellation use the same path. No automatic
whole-runner force timer remains; a second explicit SIGINT is emergency force.
SDK task/AbortSignal cancellation retains ownership until engine cleanup, even if
its caller remains pending. Control-wait cancellation ends only the waiter.
Choose a run folder to supervise a pending SDK call through the separate wrappers.

Retain target, inputs and name/folder. Repeat the same plan with `--resume NAME`
or the same `--run FOLDER`; old input options are not restored automatically.
Recorded spend carries forward. An unknown paid outcome can settle conservatively
at the full reservation; local completion does not prove remote completion or a
final invoice. A later resume can repeat an uncertain plain synchronous call;
recorded resumable jobs retain their job-reconciliation protection.
`--yes-up-to` gates later phases only; it does not pause on demand.

**Deferred pause:** the agreed intent is to stop new steps, let active steps
finish, then persist a resumable boundary. Active steps may still spend while
draining. Define acknowledgment versus reaching that boundary and recovery after
process exit before implementation. OS process suspension is not durable pause.
Explicit force termination is a separate operation, also not implemented.

### "Change what has not started yet"

**Current:** wait for or cancel the invocation, change authored inputs/definition,
replan and inspect the impact, then use a new folder with the same project cache.
Matching valid results can be reused. A changed plan is refused in the old folder.

**Proposed active revision:** define dispatch races, accepted work, dependent
invalidations, identity and budget changes, and an attributable history. "Not yet
run" alone is insufficient when a scheduler can dispatch concurrently. No current
command mutates active parameters.

### "Keep this result; try another there"

**Current:** address the exact recorded step path, use `reroll` to choose its next
take or `pick` to select a take, replan, and run with the relevant spending approval.
These commands affect future execution; they do not launch work or silently change
an active invocation. A selected take may still require generation, and downstream
work may change. Preserve the original run as evidence.

### "Recover from failure"

Inspect the explicit run and failed/skipped instances before retrying. Distinguish
node failures, planning refusals, provider jobs, cancellation and invalid artifacts.
Resume the same folder when the plan is unchanged; revise into a new folder when
it changed. FX owns paid-call retries and settlement. Do not multiply retries in
node code or a loop around a paid command; recorded failure is not proof that a
provider charged nothing.

### "Tidy up the layout"

**Ratified design, 2026-10-09; Ship A in source, 2026-10-10, unreleased; Ship B not
implemented.** [Canvas layout](spec/layout.md) specifies it. Also cover requests such as "Place the comparison below the variants
so it makes more sense." User-driven canvas editing is deferred; the first consumer
is an external agent authoring presentation data.

The design has two ships. Ship A gives every view an automatic left-to-right grid
and stacks repeated instances into decks; it adds no file. Ship B adds the optional
adjacent `<workflow id>.layout.json` (`fx-layout-v1`) in the folder of the workflow's
takes file: declared steps placed by column and row, and each repeat drawn as a deck
or a grid. Cells are computed by the engine; a missing file means automatic layout.
Builders, imports, precedence, diagnostics and the layout report are settled in the
spec. Per-run files, per-instance keys, a layout command and typed SDK access to the
report are deferred: the layout route (and, from Ship B, `inspect`) carries the report,
and Python and JavaScript callers will receive it untyped inside the existing
`inspection` summary. In source, the scoped and standalone `api/layout` routes and a
run's `api/snapshot` serve the report with automatic cells only. Nothing is in an
installed release, and the file and `inspect` layout output arrive with Ship B: do not
advertise them before then.

The operation sequence is **inspect, edit, validate, refresh and verify**:

1. Read the saved graph/run and its layout report: supplying file, revision, state,
   cells by declaration path, decks and diagnostics. Resolve the intended workflow; v1
   has one shared file per workflow and no run-only override.
2. Edit the JSON through ordinary file tools. A key addresses a declared step and
   covers all its repeated instances; a key for one recorded instance is refused.
   Preserve unrelated entries and detect a conflicting file revision instead of
   overwriting another editor's changes.
3. Validate against the selected graph through the report's structured diagnostics
   (unknown, undrawn or imported steps, shared cells, backward edges, displaced
   slots, invalid files, refused locations). Validation must describe its scope:
   diagnostics alone do not establish that a person finds the result clear.
4. Refresh the view and report its actual URL and the report's `state` and `revision`:
   the edit is live only when `state` is `applied` and `revision` is the digest of the
   bytes written, since a refused file reports a revision too. Historical graphs may
   not match current source; expose unmatched entries and the fallback rather than
   silently claiming the request was satisfied.

The operation changes presentation only. It must work on saved evidence without
importing builders, replanning, executing steps or spending. Visual grouping must
not change execution groups or dependencies. Layout edits must preserve plan and
step identities, cache reuse, recorded history and same-plan resume. Run-status
updates should preserve the arrangement. Future portable snapshots should include
their selected layout so it does not depend on the author's checkout.

## Engineering acceptance scenarios

The identifiers below are stable references for future issues and tests. Add
evidence to a row when implementing it; do not turn a proposed scenario into a
promise through documentation alone.

| ID | Person's request | Current coverage | Evidence required for acceptance |
|---|---|---|---|
| AR-01 | "Tell me what you can do here." | Partial: version/help, built-in catalog, input schema and planning; no uniform capability discovery. | An installed agent distinguishes supported, planned and unknown operations/types without guessing flags or interpreting empty output as support. |
| AR-02 | "Check this before running." | Current: plan/check, graph and price JSON. | Agent uses `--check` or reads `problems` rather than treating `plan --json` exit 0 as validation; unresolved work and planning side effects stay explicit. |
| AR-03 | "Run it locally without AI." | Current: ordinary nodes and offline stand-ins. | A normal code-only workflow completes and its artifacts verify at $0; stand-in data stays separate from paid data. |
| AR-04 | "Show me while it runs." | Current in source: project service, early run URL, polling viewer and shared observation reader; standalone mode also remains. SDK runs return after completion. | Agent reports a verified URL; human and independent observer see the same record; project inspection survives run completion, while standalone lifetime stays explicit. |
| AR-05 | "Stop this run now." | Implemented in source: exact-target CLI/Python/JavaScript cancellation, invocation guard, optional verified wait and shared signal semantics. | Target and wait stay bound across resume; acceptance and finalization serialize; duplicate/lost requests are safe; waiter timeouts never force; verified local cleanup and unknown external outcomes remain separate. |
| AR-06 | "Continue that exact run." | Current in source: require-existing named resume and explicit-folder same-plan resume. | Finished valid work is reused; plan/mode mismatch and an existing writer are refused; history distinguishes invocations. |
| AR-07 | "Pause and wait for me." | Proposed; cancellation and later-phase thresholds only cover narrower intents. | Explicit request/acknowledgment/boundary semantics; active work and reservations accounted for; resume survives the documented lifetime. |
| AR-08 | "Change what has not started yet." | Partial: revise/replan/new run with cache reuse. Active mutation proposed. | Unchanged work is retained where identities match; an edit racing dispatch cannot silently change already accepted work; history and cost remain attributable. |
| AR-09 | "Keep this result; try another there." | Current takes, reroll and pick. | Selected recorded instance is addressed exactly; no command implies immediate execution; next-plan impact and possible cost are visible. |
| AR-10 | "Ask before the next expensive part." | Current later-phase `--yes-up-to`, within a separate ceiling. | Incomplete run can continue with an approved threshold; no claim of a general human-approval/pause system. |
| AR-11 | "You disconnected; catch up." | Current in source: opaque cursors, bounded replay and snapshot reattachment; process liveness remains unknown. | Retained events can be read from a valid cursor; duplicates, unknown liveness and later invocations are handled without launching another run. |
| AR-12 | "Put the result in my project." | Current output delivery and file verification. | Exact output destinations and missing/partial results are reported; immutable stored files remain intact. |
| AR-13 | "Do it from Python or JavaScript." | Current: planning, named execution/resume, result files, saved RunRecord inspection, snapshot/batch/async following, and exact-target control; authoring/hosting capabilities differ. | Each claimed job preserves engine semantics with native ergonomics. Recorded folders stay pinned; cursor attachment, bounded catch-up, errors, later resumes and observer cancellation have provider-free tests. Execution readiness/browser lifetime and CLI maintenance are intentionally excluded. |
| AR-14 | "Run exactly what I reviewed." | Partial: planning and execution are separate; SDK `run(plan)` retains options but the engine replans. No immutable approved-plan execution interface. | Changes between review and execution are detected or require renewed review; the agent does not mistake a saved plan or SDK object for a frozen execution snapshot. |
| AR-15 | "Keep my project available while I work." | Current in source: init, foreground/background service, status/logs/stop, persistent catalog and run URLs; OS supervision remains future work. | Repeat/parallel starts, readiness, port conflicts, project identity, independent execution, restart persistence, and standalone use all have provider-free lifecycle evidence. |
| AR-16 | "Name this deliverable; show my workflow history." | Current in source: create-only names, explicit same-plan resume, workflow-grouped index. | Concurrent name claims have one winner; collisions/missing names/plan mismatches/ambiguous sources refuse; latest includes failures, old active records remain visible, resume retains creation time/spend, and metadata changes no cache identity. |
| AR-17 | "Tidy up the layout; place X below Y." | Ratified design: [spec/layout.md](spec/layout.md). Ship A (automatic grid, decks, the layout report with automatic cells) in source, unreleased; Ship B (the adjacent `fx-layout-v1` file the scenario edits) not implemented; visual editing deferred. | Agent discovers exact scope/references, edits and validates layout, verifies the renderer loaded the intended revision, and reports the actual URL or actionable diagnostics. No builder execution, provider calls, execution-history changes or identity/resume changes; repeats, nested scopes and stale references have provider-free evidence. |
| AR-18 | "Clean up last week's exploratory runs, but keep my named ones." | Implemented in source: `runs list` and `runs remove` (CLI, Python, JavaScript), and `cache prune` (CLI). | Agent lists, previews and then removes exactly the previewed runs; named, `--run` and external runs survive filters; an active run, the last pick holder and a non-run selection are refused with a code and nothing else changes; catalog and run index forget removed folders; pruning keeps every remaining run resumable and refuses a shared, busy or unreadable cache, removing nothing; provider-free CLI, conformance and SDK evidence. |

For AR-04 and AR-11, [the observation checker](tools/check_observation.py) and
[observation v1](spec/observation.md) provide the source acceptance evidence.
For AR-15 and persistent AR-04 inspection, [the service contract](spec/service.md)
defines lifecycle and browser acceptance. The [public CLI service tests](crates/grida-fx/tests/service.rs)
and [execution integration tests](crates/grida-fx/tests/service_run.rs) exercise
lifecycle and run independence without providers; [the service demo](tools/demo_service.py)
prepares a fresh code-only project and prints ordinary user commands. Named-run
CLI/runtime tests and catalog/frontend grouping tests provide AR-16 evidence. Source
evidence does not imply that an older installed release contains these features.
Use [the viewer fixtures](fixtures/viewer/README.md),
[image-recolor](examples/image-recolor/README.md), SDK tests and
[conformance cases](conformance/) as the initial provider-free harness. Test the
public interface through an independent consumer, not only internal methods.
SDK record tests in [Python](python/tests/test_record.py) and
[JavaScript](js/fx/test/record.test.ts), plus their run tests, cover AR-13 naming,
resume and record observation through the real engine. A `RunRecord` pins its
folder, not one invocation: a later snapshot can include a resume. Existing
`RunResult` construction reads the folder after execution ends and is not atomic
with a concurrent external resume; do not promise an immutable invocation capture.
Add targeted evidence for each changed boundary instead of rerunning paid examples.

## Gaps that should guide design

- **Discovery and outcomes:** machine-readable capabilities, consistent structured
  refusals and early run identity are incomplete. When adding an operation,
  expose its inputs, preconditions, mutation/spend effects, acknowledgment versus
  completion, result and recoverable failure conditions.
- **Observation versus control:** public observation is implemented; local
  cancellation is implemented in source under [its contract](spec/control.md).
  Termination, pause, active revision and remote execution control remain later
  contracts. Keep their ownership distinct even if one client presents them.
- **Long-lived interaction:** project run URLs now survive execution and service
  restart while records are retained. Agents still need to distinguish server
  liveness, recorded run state, and unsupported execution control. Machine-wide
  aggregation and automatic service recovery remain separate design work.
- **Plan freshness:** a reviewed plan is not an execution token. Current SDK
  `run(plan)` retains its target/options, but execution plans again. Define how
  review is bound to actual inputs and code before promising exact reviewed-plan
  execution; meanwhile, recheck material changes before running.
- **Intent and authority:** a configured budget is not user authorization. Preserve
  existing authorized scope; when an operation changes that scope, make its
  effects concrete before asking for a decision. Agents must not expose secrets
  or implement their own provider retry loop to work around a missing tool.
- **Known correctness limits:** [ISSUES.md](ISSUES.md) tracks missing provider usage
  and an unexplained image-cost discrepancy. Do not claim complete cost explanation
  while those remain open. Unversioned node identities name their export in source
  (`<path>#<attr>@source:<digest>`). Engines that print a bare `source:<digest>`,
  including the 0.1.0 release, can give two unversioned exports of one module one
  cache key; there, keep distinct bodies in separate files or declare versions.

## Keeping this useful

For changes to the CLI, SDKs, lifecycle or public contracts, identify the relevant
scenario here and update its status and evidence alongside the change. For a new
user intent, add a scenario before selecting a transport or adding commands.
Record the supported path and a precise unsupported boundary, rather than
teaching agents a workaround that corrupts identity or history.

An operation is agent-ready when another process can discover it, satisfy its
preconditions, apply it to the right run, understand its outcome, and choose the
next operation from public evidence. The person should receive a clear result
or an actionable limitation without needing to understand FX's implementation.
