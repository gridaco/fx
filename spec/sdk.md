# SDK run access

Python and JavaScript expose useful user jobs through language-native APIs. CLI
verb coverage is not a goal. This additive SDK contract changes no execution,
identity, cache, spending, run-record or observation wire semantics.

## Execute and resume

Python `run`/`run_async` and JavaScript `run` accept `name` and `resume` as run
options, including when given a previously made `Plan`. They conflict with each
other and the explicit-folder option (`run_dir` / `runDir`). SDK checks for
conflicts, invalid types, empty strings and NUL run before planning. The engine
owns full name grammar, atomic claims, existence, plan/mode compatibility,
writer exclusion and accounting, under [store.md](store.md).
SDK preplanning can execute local code before the engine's remaining run checks.

Omitting all three selectors creates a fresh record. `name` is create-only; `resume` requires an
existing named run. An explicit folder retains its existing create/resume behavior.
Resuming repeats current target/input/route/builder options; old options are not
restored. A name does not force generation or alter any cache identity.

SDKs start `run` with `GRIDA_FX_CANCEL_SOURCE=sdk` in the engine's environment. Besides naming
the source of a signal's cancellation, it tells the engine that the invocation's inputs file is
temporary: its `run_started` records no `repeat` ([store.md](store.md) §8, "Repeating a run"),
so `reroll` and `pick` on its folder advise running it again rather than print a command. An
SDK caller repeats a run by calling `run` again with the same request.

Run calls retain execution ownership until completion, including cancellation
cleanup under [control.md](control.md). They return execution results. They do not
start a viewer, return an early URL, or offer a readiness handle.
Existing result construction reads the folder after the child exits; it is not
an atomic invocation capture when another process immediately resumes that folder.

## Load a record

Python `load_run`/`load_run_async` and JavaScript `loadRun` invoke engine inspection
and return a `RunRecord`. Targets follow `inspect`: explicit folders, workflow/name,
or a workflow ID for its newest-created record, with ambiguity refused. The
emitted folder is resolved once against the inspection call's working directory.
Subsequent observation uses that absolute folder, never a fresh latest-run lookup.

The record exposes `inspection`, the full engine summary and optional verification
result, and `run_dir` / `runDir`. It has no process exit code or execution ownership.
A verification failure returns its documented unsuccessful verification result;
input/engine errors raise the SDK's `FxError`. Inspection does not load author
code, run a workflow, or contact providers. Stored/placed files remain immutable.

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

Project initialization, service management/browser opening, diagnostics, node locks,
take maintenance and job forgetting stay CLI operations in this pass. Add native
SDK support when a concrete embedding job justifies its lifetime and errors.
Pause, active revision and cloud control remain separate contracts.
