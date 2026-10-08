# Run observation

**Status: v1 interface ratified, 2026-10-08. Implementation and verification are recorded below.**

FX exposes versioned, read-only execution events for independent consumers.
The built-in viewer must consume the same public observation contract. The wire interface below establishes the compatibility boundary; cursor encoding
and reader internals remain implementation details.

## 1. Existing foundation

[store.md §8](store.md#8-run-folders) and
[fx-run-events-v1](schemas/fx-run-events-v1.schema.json) already define the run
record. `events.jsonl` holds ordered, complete JSON lines. Resuming the same run
folder appends events under a new `invocation_id`. Placed step artifacts precede
their `node_finished` event; placed workflow outputs precede `run_finished`.

`project` and `inspect --json` expose summaries. `observe` and the loopback
viewer expose the v1 observation interface below. SDK run methods return their
result after execution; they do not expose a live event iterator or viewer handle.

Node-host `progress` notifications currently print to stderr; they are not run
events. They acquire no persistence or replay guarantee through this decision.

## 2. Ratified requirements

- **One engine-owned event contract.** Observation uses the recorded event
  vocabulary and meanings. Browser views may project events into display state,
  but do not define a second execution-event language. Observation does not load
  author code, execute a workflow, or open provider credentials.
- **Versioned compatibility.** Published event meanings and required fields are
  compatibility commitments. Breaking changes need a new contract version and
  a migration note. Additive changes must define how older observers continue
  safely. The exact unknown-field and unknown-event rules must be specified and
  tested before the new interface is advertised as stable.
- **Ordered, resumable reads.** A consumer can continue from an opaque position
  in one selected run's retained log. Cursor validity, duplicate delivery and
  replay behavior must be explicit. A timestamp is not a cursor: `offset_ms` is
  invocation-relative and need not be unique. No global ordering across runs or
  exactly-once external side effects is promised.
- **Recorded events before notification.** Only complete events successfully
  appended and flushed under the store contract become observable. Readers
  ignore an unfinished tail and never repair the log. Notification adds no
  stronger power-loss durability guarantee than the underlying store.
- **Independent observers.** A slow, disconnected or failed observer must not
  block scheduling or fail execution. Bounded buffering and catch-up from the
  retained log belong to delivery; execution must not await consumer callbacks.
- **Explicit execution lifetime.** `run_finished` ends an invocation; the same
  run folder may later resume. A process crash may leave no terminal event.
  Recorded unfinished state is not proof that a process is alive. The interface
  must distinguish recorded termination from unfinished or unknown state and
  define when following stops; a reader need not prove that a process crashed.
- **Shared consumer boundary.** The viewer and an independent process must be
  able to observe the same execution evidence. Snapshot attachment and continued
  observation must use a consistent position so events cannot disappear between
  the initial snapshot and subsequent reads.

Existing record schemas and execution semantics remain authoritative. This
decision changes no node identity, cache key, provider behavior or billing rule.

## 3. Scope

The first interface observes one explicitly selected local run. Its design does
not require a machine-wide service, project registry, message broker, remote
access, webhooks, command/control RPC, or a general automation system. The
engine-to-node JSON-RPC protocol remains a separate execution boundary.

The first adapters are a one-shot JSON command and cursor-based HTTP polling.
The browser polls about once a second. A later SSE adapter can reuse the same
reader and cursor without changing event meanings. High-frequency progress,
provider usage detail and custom user events need their own scoped decisions.

The [local service contract](service.md) owns project discovery, startup, catalog
identity, stable run/plan URLs and independent viewer lifetime. Observation itself
requires neither a running service nor a project: `observe RUN` reads a selected
record directly.

A CLI run plans and validates, initializes its record and flushes `run_started`,
then announces an available viewing URL before run-phase execution. Planning may
execute declared local `at: plan` nodes before that URL exists. In project mode,
the separately started FX service hosts the run page and survives execution.
If it is absent, execution still proceeds; commands never start it implicitly.

`--standalone` retains the independent OS-assigned loopback server, even inside a
project; a projectless run uses it by default. That server ends with its invoking
run. Standalone saved-plan/run inspection stays foreground until interrupted.
Neither mode changes event meanings, cache rules or execution ownership.

`--open` requests a best-effort browser launch after a usable URL is printed.
Browser or serving failures do not change the run result. `--no-view` skips run
viewer integration and conflicts with `--open`; SDK run methods pass it because
they capture output until completion. No viewer is advertised for a refused or
uninitialized run. Stopping any inspection service never stops a workflow in
another process; slow observers never block scheduling.

## 4. Wire interface v1

### Requests

```sh
grida-fx observe RUN --snapshot
grida-fx observe RUN --after CURSOR --limit 256
```

`observe RUN` without `--after` reads from the beginning. The default limit is
256 events; accepted limits are 1–1024. Each request exits: 0 with one JSON result,
2 with one structured reader-error JSON on stdout. CLI usage/parse failures use
the usual stderr diagnostics and exit 2. There is no implicit follow loop.
`--snapshot` excludes `--after` and an explicitly supplied `--limit`.

A standalone loopback host for a run provides the same reader through:

- `GET /api/snapshot`: consistent snapshot, plus an additive `view` field containing
  the existing `fx-viewer-run-v1` projection of exactly that prefix.
- `GET /api/events?after=CURSOR&limit=256`: bounded events strictly after the cursor.
  Omit `after` to replay from the beginning.

The project service places these same endpoints under the selected run's
`/p/PROJECT_ID/runs/RUN_ID/` prefix. Its root `/api/view` is the project catalog;
a run's scoped `/api/view` is its projection. Artifact URLs share that run scope.

Responses are `Cache-Control: no-store`. Existing `/api/view` and `/api/run` remain
available. Static plan hosts have no run observation source. Artifact references
remain recorded values; browser serving URLs are supplied by the view projection.
The browser has no run-folder or cache-path assumptions.

### Responses and attachment

The normative schemas are [snapshot](schemas/fx-run-snapshot-v1.schema.json),
[event batch](schemas/fx-run-event-batch-v1.schema.json) and
[error](schemas/fx-run-observation-error-v1.schema.json).

A snapshot has `kind: "fx-run-snapshot-v1"`, `plan`, `events`, and `cursor`. Its plan
is the saved `fx-graph-v1`; its events are the complete readable prefix in recorded
order. The cursor acknowledges exactly that prefix, even if execution appends
more events while the response is being constructed. Attach by displaying that
snapshot, then requesting events after its cursor.

A batch has `kind: "fx-run-event-batch-v1"`, `events`, `cursor`, and `has_more`.
Its cursor acknowledges only events returned in that batch, not a later frontier.
An empty batch preserves the acknowledged position. `has_more` means another
complete event was readable when the batch was captured; it says nothing about
future execution. Drain batches while it is true, then poll again. If a response
is lost, retry the previous cursor: the same prefix may be delivered again.
Apply and persist an acknowledged cursor together. External side effects require
the consumer's own idempotency policy.

Cursors are opaque, versioned, local-source handles. Do not decode or synthesize
them. CLI and HTTP readers of the same local folder accept the same cursor.
Moving/copying a folder to a different source can invalidate it; attach again.
A cursor binds the selected source, its plan, and its acknowledged log prefix.
Wrong-source, truncated or changed-prefix cursors fail, rather than silently
skipping data. Appending a resumed invocation preserves existing cursors.
Byte-identical acknowledged content is the same evidence, even if replaced.

Read only complete lines containing valid JSON and a supported, typed event
envelope; ignore an unfinished final line. An
unreadable or blank complete line returns `invalid_record`; the observation interface never
acknowledges corruption. Legacy summary readers may still show a readable prefix.
Readers never repair the log.
`run_finished` and `run_cancelled` terminate an invocation, not observation of the
folder. There is no stream-closed or process-alive promise. Consumers decide when
to stop, or remain attached to observe a later resume.

### Compatibility and errors

Known event payloads retain [their existing schema](schemas/fx-run-events-v1.schema.json).
The reader validates JSON and required envelope fields, including the saved plan
digest; it does not schema-check each known payload. Consumers taking external actions based on a
recognized payload must validate its required fields first. The built-in viewer
retains the existing tolerant, read-only display projection; omitted payload
evidence is not an execution-control or artifact-verification guarantee.
The observation envelope requires `kind`, `event`, `invocation_id`, `plan`, and
`offset_ms`. Within v1, consumers ignore unknown fields and unknown event names,
while still advancing their cursor. They must reject unsupported envelope/wire
versions; ignoring an unknown event is not permission to reinterpret its payload.
Older saved v1 records remain readable without rewriting them.

Errors have `kind: "fx-run-observation-error-v1"`, `code`, and `message`. No error
contains a private absolute source path. HTTP maps `invalid_cursor` and
`run_changed` to 409 (reattach), `invalid_limit`, `invalid_request` and `unsupported_version` to 400, and other reader failures
to 422. Unsupported versions require a compatible reader. Unknown future error
codes are generic observation failures, never successful delivery. Transport
failure leaves the last acknowledged cursor intact; retry or reattach explicitly.

### Implementation limits

The local stateless reader verifies the acknowledged prefix with a streaming hash
before returning new events. It avoids reparsing old events and bounds returned
batch size, but polling I/O grows with retained history. Snapshots materialize the
whole prefix; they are not a replacement for bounded catch-up on large runs.
This is the initial local implementation, not a storage-service scalability claim.
An optimized reader or future storage adapter must preserve the same guarantees.

## 5. Implementation and verification

The implementation followed the ratified order:

1. Record the ownership and guarantees before choosing adapters.
2. Specify v1 wire schemas, cursor scope, errors and invocation/server lifetimes.
3. Add the shared runtime reader and one-shot CLI adapter.
4. Prove the boundary with an independent provider-free consumer.
5. Add HTTP adapters and class-owned viewer polling with stable canvas state.
6. Add the initialized-run readiness hook and independent CLI hosting; the later
   [service contract](service.md) adds persistent project hosting.
7. Independently review, reconcile implementation/schema differences and verify.

Source ownership:

- `grida-fx-runtime::observation`: read-only prefix capture, cursor integrity and
  bounded replay; `runner::run_with_ready`: initialization before run-phase work.
- `grida-fx observe`: machine-readable CLI adapter.
- `grida-fx-viewer`: loopback snapshot/event adapters and prefix-consistent display
  projection; artifact serving still verifies recorded bytes.
- `js/fx-web`: wire parsing and independently testable polling controller;
  React remains presentation. Saved plan sessions never poll.

The meaningful verification gate is:

```sh
uv run --project python python tools/check_observation.py --command target/debug/grida-fx
```

It uses [the gated observation fixture](../fixtures/viewer/README.md#live-observation)
to cover mid-run attachment, CLI/HTTP agreement, bounded catch-up and replay,
invalid cursors, a slow consumer, artifact digests, resumed invocations, cancellation,
failure, opt-out and standalone server closure. Service lifetime and restart
coverage belong to the separate [service acceptance contract](service.md#6-verification-and-delivery). Runtime unit tests additionally cover torn
tails, crash-like unfinished records, wrong-source/changed-prefix cursors, schema
versions, forward fields/event names and corrupt/blank complete records. HTTP tests
cover consistent projected prefixes, safe structured errors and live cost settlement.
Frontend tests cover no-overlap polling, retry, reattachment, disposal and preserved
selection/scope navigation. A browser check verifies running-to-finished states and
image previews without manual refresh, with selection and zoom preserved.

These are source capabilities. No package release or provider run is part of this
implementation. Pause, active parameter revision, high-frequency progress events,
SDK observation handles and cloud execution remain separate work.

## 6. Precedents

References checked 2026-10-08; they support the observation pattern, not identical
delivery guarantees or a requirement to adopt their server architectures.

- [Prefect's event subscriber](https://reference.prefect.io/prefect/events/clients/#prefect.events.clients.PrefectEventSubscriber)
  and [event-stream command](https://reference.prefect.io/prefect/cli/events/)
  expose workflow events to independent programs and JSON consumers.
- [Docker events](https://docs.docker.com/reference/cli/docker/system/events/)
  exposes lifecycle events with filters and JSON Lines output. Its retained
  history is bounded; that is not an indefinite replay promise.
- [Temporal event history](https://docs.temporal.io/encyclopedia/event-history)
  makes recorded workflow execution history a documented product concept.

These are precedents for the public boundary. FX must establish its own guarantees
with its own implementation and conformance cases.
