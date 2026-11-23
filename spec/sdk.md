# SDK run access

Python and JavaScript expose useful user jobs through language-native APIs. The
engine owns execution, run identity, cache, spending and observation.

## Prepare a plan

Python and JavaScript plan through the same engine path as the CLI. For a selected missing
image-model binding on a supported provider, the engine may make a bounded, read-only
metadata lookup before offline core planning ([model-selection.md](model-selection.md) §3).
This needs neither `live` nor spending approval and never generates an asset. Network-off,
authored-route precedence, price admission and lookup failures are identical through
every invoker. Importing an SDK does not make a lookup.

A `Plan` retains the engine's normalized discovered routes as a preparation snapshot,
independently of the optional metadata cache. Creating a new run with `run(plan)` sends that
snapshot back to the engine instead of discovering those model bindings again. Snapshot
transport uses private temporary storage and does not require a writable project source root. The engine
validates the snapshot and current authored configuration. SDKs compute no route identity. This adds no public resolver
or synchronization API. Preparing a new plan still resolves current configuration and
provider metadata under the normal freshness policy.

## Execute and resume

Python `run`/`run_async` and JavaScript `run` accept `name` and `resume` as run
options, including when given a previously made `Plan`. They conflict with each
other and the explicit-folder option (`run_dir` / `runDir`). SDK checks for
conflicts, invalid types, empty strings and NUL run before planning. The engine
owns full name grammar, atomic claims, existence, plan/mode compatibility,
writer exclusion and accounting, under [store.md](store.md).
SDK preplanning can execute local code before the engine's remaining run checks.

Omitting all three selectors creates an independent run at attempt 1. `name` is
create-only. `resume` requires an existing named run and selects its latest
attempt. An explicit folder selects the exact record it contains.
Each resume starts a new invocation of the same
[run attempt](store.md#run-references-attempts-and-invocations).
Resuming through `run` uses the target, inputs, routes and builder options supplied
by the client. The name labels the run record independently of cache identity.

For a named resume or an existing explicit run folder, SDKs invoke the engine without
preplanning or injecting a `Plan`'s preparation snapshot. The engine loads the run's recorded
model facts before planning, retaining the usual current-configuration and plan-digest
checks. Deleting the optional metadata cache or removing a provider key does not force
rediscovery before a resume that can use recorded facts and cached results. An engine
refusal before execution raises `FxError`; SDKs do not repeat discovery to diagnose it.

Pass `recover=True` in Python or `{ recover: true }` in JavaScript with `live` and
an existing `resume` or explicit folder. This enables [recovery-only execution](store.md#5-long-jobs),
including collection of settled jobs whose handles are still in the store.
It can replay cached answers, run local steps and contact the original provider
to collect. It preserves the prior recorded ceiling and sends no new managed work.
Ordinary live execution also retries collection for jobs already known to have finished.

Python `resume(run)` / `resume_async(run)` and JavaScript `resume(run, options)`
select an attempt by exact folder, run ID or `WORKFLOW_ID/NAME` and continue it.
They return the same `RunResult` as `run` and use the engine's recorded-context
resume path. They accept the normal live authorization, ceiling, estimate and
later-phase approval options, and can wait for external answers. The engine reports
missing context, a plan mismatch or an authorization refusal as `FxError`.
Supply the original execution options to `run` when the saved record needs
explicit reconstruction.

## Rerun and inspect history

Python `rerun(run)` / `rerun_async(run)` and JavaScript `rerun(run, options)`
create a numbered attempt under the same run ID from a verified saved source.
They accept the execution options of `resume`, including current live and
spending approval, and return a `RunResult` for the new attempt. Pass `attempt`
to select a source by number, or pass its folder directly. A caller retrying a
start can use the same `request_id` / `requestId` to recover the attempt already
accepted for that request. See [full rerun](store.md#run-references-attempts-and-invocations).

Python `run_history` / `run_history_async` and JavaScript `runHistory` read a
`RunHistory` through engine inspection. Its `run_id` identifies the run and
`attempts` contains each attempt's `attempt_number` and folder. Use the native
`run_dir` / `runDir` of an entry for exact operation targeting. `RunResult` and
`RunRecord` expose the recorded `run_id` / `runId` and `attempt_number` /
`attemptNumber` when available.

## Execution ownership and results

SDKs mark signals from owned `run`, `resume` and `rerun` processes with
`GRIDA_FX_CANCEL_SOURCE=sdk`. When opening a run, they report whether they wrote
temporary inputs with `GRIDA_FX_SDK_TEMP_INPUTS` (value `1` for temporary inputs
or `0` for persistent inputs). `run_started.repeat` can record eligible persistent
input paths. A temporary inputs file or the absence of this marker on an older SDK's
`run` leaves `repeat` absent. See [repeating a run](store.md#8-run-folders).

Run, resume and rerun calls retain execution ownership until completion, including
cleanup under [control.md](control.md). A terminal pause returns a `RunResult`
with `paused` and `incomplete` set to true and the available workflow outputs.
These calls do not start a viewer, return an early URL, or offer a readiness handle.
Existing result construction reads the folder after the child exits; it is not
an atomic invocation capture when another process immediately resumes that folder.
`RunResult.call_recoveries` / `RunResult.callRecoveries` contains the recorded recovery events.
The event's optional `original_charged_usd` is the amount FX booked for the original task.
Older records can lack the evidence needed to identify that amount, even when the run's
accumulated spend is available. Recovery adds zero to that accumulated spend.

## Pause a run

Python `pause` / `pause_async` and JavaScript `pause` request that one exact
invocation stop admitting new steps and drain admitted work. Callers can supply
an expected invocation ID and wait for verified local completion with the same
`wait` and `timeout` options as cancellation. The result reports acceptance
separately from the recorded outcome. A stopped client wait leaves an accepted
request in effect. Structured refusals raise `RunControlError` with the engine's
code and result. `inspect_control` / `inspectControl` report `can_pause` when
available and distinguish `pausing` and `paused`. See [run control](control.md).

## Answer external requests

Use `list_requests` / `listRequests` with an exact attempt directory, a run ID or
`workflow/name`. An ID or name follows the latest attempt. Retain the exact
attempt directory when responding later.
Listing reads the run's request records without discovering a viewer. They include
the frozen schema and any authored message, reviewed values, choices, original
deadline and current state. Use `respond(run, request_id, answer)` for a durable
receipt. An engine refusal raises `AnswerError` with a machine-readable `code`.
Before submission, the SDK checks that it can encode the value. Python raises
`ValueError` for invalid JSON values, including a mapping with non-string keys.
JavaScript raises `TypeError` for values outside its SDK profile, including
integral numbers with magnitude greater than 2⁵³.
A conflict or expiry supplies the winning `request`. Python also supports the
`_async` forms. See [waiting steps](waiting.md).

Pass `wait_for_answers=True` / `waitForAnswers: true` and an explicit run directory
or name before starting an asynchronous run. Once initialized, a second client can
inspect and respond while the execution call is pending. `inspect_requests` /
`inspectRequests` returns `{requests, viewer_url}`. Requesting registration
(`register=True` / `{register: true}`) makes an SDK-started run available to an
already running local project viewer. The URL is null when no verified viewer
is available. Neither inspection nor registration starts a server.

## Provider-native calls and diagnostics

A project node may declare and call a custom capability through `ctx.capability`;
its project adapter declares it as a request or job. Standard capability schemas
are not applied to unknown capabilities. Canonical data, file references,
routing, holds, caching and job recovery still use the engine's common path.
[The native harness](../fixtures/native/README.md) exercises arbitrary fields,
files, cancellation, resume and independent call-cache replay in both SDKs.

Python failure outcomes accept a keyword-only `diagnostic=AdapterDiagnostic(...)`;
TypeScript outcomes accept `diagnostic: {source, phase, ...}` with camelCase
`httpStatus`, `requestId`, `taskId`, `threadId` and `turnId`. Evidence is optional, independent of the
outcome and sanitized by the engine. Uncaught capability errors retain it in
`EngineError.data.diagnostic`. Recorded fields use lower_snake_case:
Python `RunResult.failures[id].diagnostic`, JavaScript
`RunResult.steps[id].diagnostic`, inspection and observation expose the same data.
Older records may have no diagnostic. Cancellation still raises `FxError` with
exit status 130; load the saved record to inspect and later resume it.

## Load a record

Python `load_run`/`load_run_async` and JavaScript `loadRun` invoke engine inspection
and return a `RunRecord`. Targets follow `inspect` and accept an exact folder, run
ID, workflow/name, or a workflow ID for its newest independently created run.
Use the `attempt` option to select an earlier attempt by number. The SDK resolves
the emitted folder against the inspection call's working directory. Subsequent
observation uses that absolute folder.

The record exposes `inspection`, the full engine summary and optional verification
result, and `run_dir` / `runDir`. Where recorded, `inspection.run.call_attempt_failures`
contains the sanitized call failure events, including earlier attempts the run recovered from.
`inspection.run.call_recoveries` contains the recovery events, when present.
The record has no process exit code or execution ownership.
A verification failure returns its documented unsuccessful verification result;
input/engine errors raise the SDK's `FxError`. Inspection does not load author
code, run a workflow, or contact providers. Stored/placed files remain immutable.

## Discover and resolve files privately

Python `list_files(target, project=None, instance_id=None, cwd=None)` and JavaScript
`listFiles(target, {project, instanceId, cwd, env, signal})` enumerate recorded file
targets through the engine's [file access contract](files.md). `target` is an
explicit run folder or saved plan JSON file. `RunRecord.list_files` / `listFiles`
use the record's pinned folder; all Python jobs also have `_async` counterparts.

Return a list of `FileDescriptor` values: `target` retains the portable viewer
target shape, `role` describes its recorded association, and optional
`instance_id` / `instanceId` and `scope_id` / `scopeId` identify its context.
Discovery loads neither workflow builders nor node modules.

Python `resolve_file(target, reference, project=None, source_root=None, cwd=None)`
and JavaScript `resolveFile(target, reference, {project, sourceRoot, cwd, env, signal})`
resolve a complete captured viewer file reference (`subject` and `target`). The
record methods `resolve_file` / `resolveFile` again use the pinned run folder.
The engine owns subject checks, recorded-target admission, source binding,
confinement and artifact verification. SDKs do not infer identities or derive a
filesystem location from an artifact URL or portable source label.

The native `FileResolution` result contains `target`, `local_path` / `localPath`,
`source_match` / `sourceMatch`, and `reason`. Python's available `local_path` is a
`Path`; JavaScript's `localPath` is an absolute path string. Unavailable local
copies return `None` / `null` and a reason. These paths belong only to the trusted
native caller: never copy them into browser documents, viewer messages or portable
exports.

Artifacts resolve to verified recorded copies. Source resolution opens today's
editable file; `source_match` is `match`, `changed` or `unknown` when applicable,
and changed source remains openable. Source resolution needs an explicit local
binding: `project` selects the planning project and defaults the source root;
`source_root` / `sourceRoot` supplies the owning source root when it differs.
Closure-only helper labels require explicit `source_root` / `sourceRoot`, even
when `project` is supplied, because their base may be a declared source package.
The binding resolves the complete recorded label; FX does not infer package
ownership from today's project configuration or discover installed packages.
Neither the current directory nor the run folder silently supplies that binding.
Native resolution does not execute author code, contact providers or open the
file itself. The host owns its file-tab, OS-opening and other presentation choices.

Structured refusals raise `FileAccessError` with `code` and the complete error
`document`, including unknown future codes. Unsupported engines, malformed
responses and transport failures raise ordinary `FxError`. JavaScript's signal
and Python task cancellation stop only the reader command, never the workflow.

## Observe independently

Record methods `snapshot` and `events` call the existing engine `observe` reader;
Python also provides their `_async` forms. Documents retain
[observation v1](observation.md) wire names and opaque cursors. Snapshot is a
consistent plan/event prefix. The earlier inspection and later snapshot are
independent samples; they are not one atomic state read.

`events` accepts an optional `after` cursor and a limit of 1–1024 (default 256).
SDKs validate response envelopes and reject incompatible wire versions. Unknown
fields and event names pass through. Payload validation before external actions
remains the caller's job. Structured reader failures raise `RunObservationError`
with the engine's code and document, including unknown future error codes;
malformed responses and transport errors raise `FxError`.

`follow` is an async iterator of nonempty event batches. It starts from `after`,
or replays from the beginning when omitted. It buffers one bounded batch, waits
for consumer demand, drains while `has_more`, then polls about once a second.
Apply a whole batch before checkpointing its cursor. Cursor and `has_more` values
are captured before handing the document to caller code.

Following continues across terminal events and later resumes until the consumer
breaks or cancels it. It never infers owner liveness or cleanup from recorded
events. It does not retry missing records, reset invalid cursors, reattach silently,
or cancel execution when observation ends. Consumers needing a single invocation
decide when to stop using its recorded invocation ID and terminal event; a crash
may leave no terminal event. Follow requires no running project service.

## List and remove runs

Python `list_runs(workflow=None, cwd=None)` and JS `listRuns({workflow, cwd})` return the
planning project's runs as `runs list --json` gives them ([store.md](store.md) §9), each with
its folder resolved against the call's working directory (`run_dir` / `runDir`), which a
removal accepts as it is. Python `remove_runs(runs, preview=False, cwd=None)` and JS
`removeRuns(runs, {preview, cwd})` remove the named runs as `runs remove --yes` does, or with
`preview` say what would be removed. Pruning the cache (`cache prune`) stays a CLI
operation: it is an operator's decision about a whole store, which may be shared. The SDKs take explicit runs only: a caller filters the
list itself, so what is removed is exactly what it chose. A run refused (`active`,
`holds_pick`, `changed`, `not_removable`) or left `partial` is reported in the result, not raised; a
selection that is not a run raises an error carrying its code, and nothing is removed. A
result of an applied removal that cannot be read raises an error that says the removal may
have taken effect. Neither result is bounded in size, since a project may hold thousands of
runs.

## Export recorded evidence

Python `RunRecord.export(destination, include_values=False,
include_diagnostics=False)` and its `export_async` counterpart, and JavaScript
`RunRecord.export(destination, {includeValues, includeDiagnostics, signal})`,
invoke the engine's [portable export](exports.md). The saved source folder stays
pinned. Relative destinations resolve against the caller's current working
directory at the method call, not the earlier inspection location.
For viewer display defaults, a loaded record retains the resolved `cwd` supplied to
`load_run`/`loadRun` (or the loading caller's directory when omitted). The engine reads
that publishing project's current `fx.yaml`; it never infers a historical project from
the saved run. A directly constructed Python `RunRecord` uses the export caller's context.

The engine requires terminal, inactive evidence and a new destination. It owns
locking, disclosure projection, media selection, verification, atomic placement
and export identity. SDKs never compute identities or interpret a refusal as
permission to resume, regenerate or publish. Values and diagnostics are withheld
by default; media and labels still need the author's publication review.

Return the `fx-run-export-result-v1` receipt with wire field names preserved:
`manifest_sha256`, `output`, `files`, `bytes`, and `omitted_artifacts`. SDKs validate
the receipt envelope and retain unknown optional fields. The counts cover copied
artifact files, excluding `manifest.json`. Structured engine refusals raise
`RunExportError` carrying `code` and `document`; transport or malformed-response
failures raise `FxError`. JavaScript's signal stops the export command, not any
workflow. This operation has no provider calls, project-service requirement or
CDN-account handling.

## Answer paid calls with a stand-in

Python `run`/`run_async` (`stand_in=`) and JavaScript `run` (`standIn`) answer a run's
paid calls with a function in the caller's own process ([protocol.md](protocol.md) §5.7,
§8). Each invocation gives the engine one end of a connected pair of Unix-domain stream
sockets as its standard input, with `--stand-in -`, and serves `initialize`,
`stand_in.answer`, `$/cancel`, `shutdown` and `exit` on the other end until the engine
exits. Python makes the pair with `socketpair()`. JavaScript, which has none, accepts a
connection on a socket in a fresh private folder, then closes the listener and removes
the folder before the engine starts. Both answer one call at a time, in the order the
requests arrive, so the function's state (counters, scripted answers, the requests it
saw) is the caller's, and each invocation may get another function.

A stand-in run is never live and runs every phase. Before anything starts, the SDKs
refuse `live` and `yes_up_to` / `yesUpTo` with a stand-in, a stand-in that is not a
function, and a system that is not POSIX. JavaScript's refusals read:

- `a stand-in run is never live: pass live: false, or no standIn`;
- `a stand-in run runs every phase: yesUpTo does not apply`;
- `standIn is a function of one StandInCall, not <type>`;
- `stand-ins need a POSIX system`.

A stand-in that faults makes the engine stop the run, and once the engine has ended,
`run` raises the stand-in's first error (JavaScript: rejects with the value it threw).
`RunResult.stand_in` / `standIn` says whether a run was a stand-in run.
`grida-fx run --stand-in <file>#<function>` serves the same function from a stand-in
host of the file's language.

## Intentional boundaries

Plans already expose graph/problems/pricing, and execution results expose output
files and delivery. Independent wrappers for every planning verb are optional
conveniences rather than required parity. Existing JS wrappers remain supported.

Project initialization and template installation (`init` and `add`), service
management/browser opening, diagnostics, node locks,
take maintenance and job forgetting stay CLI operations. Add native
SDK support when a concrete embedding job justifies its lifetime and errors.
Active revision and cloud control remain separate contracts.

## Optional cost forecasts

The CLI, Python and JavaScript planning and execution jobs accept `--require-estimates`,
`require_estimates=True` and `requireEstimates: true`, respectively. Ordinary planning admits
compatible unpriced routes. Strict forecast coverage refuses uncached paid work without an
applicable estimate; explicit reservation caps do not satisfy it. Free stand-in runs are exempt.
Planning checks potential calls of uncached steps; it cannot prove arbitrary node requests
will be served entirely by the managed-call cache.

Prepared graphs may use `fx-graph-v2` with null total amounts, known numeric subtotals and
`unpriced_calls`. SDK estimate accessors return absent amounts for unknown totals; `forecast`
exposes known subtotals and the unpriced count. They never coerce unknown totals to zero.
Existing numeric `fx-graph-v1` records remain readable.
