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

## Intentional boundaries

Plans already expose graph/problems/pricing, and execution results expose output
files and delivery. Independent wrappers for every planning verb are optional
conveniences rather than required parity. Existing JS wrappers remain supported.

Project initialization, service management/browser opening, diagnostics, node locks,
take maintenance and job forgetting stay CLI operations in this pass. Add native
SDK support when a concrete embedding job justifies its lifetime and errors.
Python callable stand-ins remain supported; a JS callable transport needs separate
design. Pause, active revision and cloud control remain separate contracts.
