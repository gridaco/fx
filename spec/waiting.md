# Waiting steps

A workflow uses `fx/await@1` to declare a JSON value that an external caller can supply during a run. The [request schema](schemas/fx-await-request-v1.schema.json) defines the durable record. The request belongs to the selected run and expanded step instance, including its item and take. A workflow may open several requests. Independent steps continue while their dependencies are ready.

## Authoring

| Setting in `with` | Meaning |
|---|---|
| `schema` | Required JSON Schema for the submitted value. The engine validates it locally for every submission channel. |
| `message` | Optional nonempty text describing what to supply. A promptless request omits this field in its record. |
| `show` | Optional reviewed values or artifact references. |
| `choices` | Optional keyed values. Freeze the actual mapping when the request opens. |
| `choice_field` | For object answers, the top-level member naming the chosen key. With choices and no field, the answer is the key string itself. |
| `timeout_seconds` | Optional positive window measured from first opening the request. |
| `on_timeout` | `fail` (default) or `continue`. |

The minimal declaration is `with: { schema: { type: string } }`. A scalar `enum`
constrains the value directly. Use `choices` when a key binds the submission to a
specific candidate from this run. A submission must satisfy both constraints.
For a whole-answer key at least one offered key must satisfy the schema. For a
named choice field, reject directly detectable contradictions, such as requiring
a boolean where the key must be a string. Check known settings during planning
and check evaluated settings before persisting the request. Controls for selecting
or entering a value are a client decision.

An answered step exposes `outputs.answer`, and `outputs.selected` when a choice was offered. The latter carries the original selected value, including its file identity. `facts.outcome` is `answered`. False and schema-permitted null are actual answers. On expiry with `continue`, the step reports `facts.outcome == 'expired'` without an answer. A workflow can run a normal conditional fallback using the reviewed candidates. Its existing route, admission and budget rules apply. No paid reservation or node worker is required during the wait.

## Identity, scheduling and recovery

A repeated waiting step honors its declared `concurrency`. At most that many
of its requests can be open at once. Other ready steps continue to run.

The request's execution scope is its exact
[run attempt](store.md#run-references-attempts-and-invocations) and expanded step
instance. The same scope applies whether the runner is attached, has parked, or
has resumed under a new invocation.

Persist the schema, optional message and reviewed values before making a request available. Opening is idempotent across invocations of the same attempt. A rerun opens its own requests with fresh deadlines. Distinct occurrences and independent runs also open independent requests, even for identical settings. The supplied value belongs to the attempt. Downstream computation uses its normal content-based identity. Resume verifies the existing plan and execution mode. Planning never opens a request. A branch conditional on the outcome uses the existing conditional cost range. Before the answer, the lower bound can omit the branch and the upper bound includes what it could spend. After resolution the engine prices the eligible work.

When only outstanding requests block progress, the default invocation exits 1 (incomplete). Inspection reports `awaiting` and preserves completed work. With `run --wait-for-answers` the runner stays attached, dispatches eligible work on an answer or deadline, and remains cancellable. A foreground invocation owns its timer. After exit, the original deadline still governs new answers and the run continues on explicit resume. Reading the run or submitting a decision cannot start an invocation or change its spending authority.

One durable resolution wins per request. Validate against both schema and frozen choices before committing. A rejected input leaves the request open. Retry of the same accepted JSON is idempotent, including after the deadline. A conflicting answer or late first answer reports the winning state. Time and channel describe the submission, without asserting the identity of a named reviewer.

For the [planned passive-wait race](https://github.com/gridaco/fx/issues/161),
each race occurrence is scoped to one exact run attempt. Its participating waits
and recorded winner stay within that occurrence across invocations.

## Local interfaces

`inspect RUN --requests --json` selects an attempt by folder, run ID or workflow/name and returns `{requests}`, including the winner of a resolved request. Use the exact folder to keep a later response pinned to the listed attempt. Listing reads request state independently of the viewer service. Add `--viewer-url` to look up an existing viewer link, or `--register` to register the run in the local project catalog. These explicit forms return `{requests, viewer_url}`. The URL is null until a verified project service can serve the registered run.

`respond RUN REQUEST --json JSON` returns a JSON receipt. A successful first or duplicate answer exits 0. A refused answer exits 1 for `invalid_answer`, `answer_conflict`, `expired` or `not_found`. A malformed JSON input exits 2. Success returns `{request}`. A refusal returns `{code, message, request?}`, including the winning request on conflict or expiry. Use `--json=-` for standard input. Python and JavaScript expose the same operations. Passing an explicit run folder or name allows a second client to observe a pending SDK invocation.

The live local viewer submits to a run-scoped JSON POST and polls for changes. The CLI, SDKs and viewer require no sign-in or credential setup. Preserve the loopback HTTP boundary, verify the browser origin for writes, and keep private control credentials out of browser documents, URLs and exports. An external agent can ask in chat and submit a user's answer while the viewer remains available to the user. Both clients observe the same recorded winner.

## Storage

The run's authoritative request records are in `awaiting/REQUEST_ID.json`. Each file is an
`fx-await-request-v1` object with `kind`, `plan` (the saved plan digest), `run` (the first
`run_started` invocation ID) and `request`. The request contains `id`, `step` (the concrete
instance ID), `schema`, `show`, optional `message`, `choices`, `choice_field` and
`timeout_seconds`, `on_timeout`, `opened_at`, optional `deadline` and `state`. Times are
absolute UTC RFC 3339 timestamps recorded at millisecond precision. The `deadline` is the
recorded opening time plus `timeout_seconds`. `state.status` is `pending`, `answered` or `expired`.
The evaluated request settings and the submitted JSON answer each have a 1 MiB limit.
Answers follow the same value rules as other workflow data. The engine rejects
[reserved runtime markers](identity.md#3-the-plain-projection) at any depth
and values nested beyond the run event's 509-level limit before recording an answer.
Pass large media as reviewed artifact references.
An answer stores `answer` (including explicit null), `channel` (`cli`, `sdk` or `viewer`)
and `at`. An expiry stores `at` equal to the deadline.

The opaque ID is the SHA-256 digest of the canonical JSON array
`["fx-await-request-v1", plan, run, step]`. It identifies a request within the exact run and
does not grant authority to answer it. The schema is Draft 2020-12 with no external
resource retrieval. Reviewed values and candidate mappings use the portable run-event value
encoding for typed references, with an `object` form for typed nested members and optional
`verdicts` on collections. Explicit null is `{"value":null}`. Reopening under the same ID
checks the frozen schema, optional message, reviewed values, choices and timeout policy. A mismatch
is refused.

`awaiting.lock` is an empty per-run operating-system lock. Within it, opening, answering
and expiring take one snapshot of the saved request, arbitrate by the engine clock and
atomically replace that request's file. A valid first response may be accepted only if the
engine sampled time before the deadline while holding the lock. It acknowledges after flushing
the file and publishing the replacement. Expiry and new answers serialize. Answer inputs use
the [FX JSON value profile](identity.md#1-values), and the engine compares retries using
canonical JSON. An equivalent response to an already answered request returns the saved
answer and original channel regardless of the current time. Listing requests can report an
elapsed deadline based on the current wall clock. Until a writer records expiry, that read-only
projection follows any system clock correction. Once a writer commits a resolution it is final.

The runner retains its separate `run.lock`. A responder does not acquire `run.lock`, so it
can answer an attached or exited run. Run removal acquires `run.lock` before `awaiting.lock`.
Invocations alone append `waiting_request` events with the instance ID, request ID, observed
request status and optional deadline. That summary omits the message, schema, choices and
answer. A detached answer is visible to request inspection immediately and to invocation
events when an invocation next observes it. A quiescent invocation records its blocking
request IDs in `run_finished.awaiting`. Cancellation ends the invocation, leaving request
state and the deadline intact for the same recorded run. A response cannot resume an ended
invocation.

An open request can remain outstanding when the operator pauses the run. Its
original deadline continues to apply, and the engine can accept a response
while the runner is away. A resumed invocation observes the saved result before
dispatching dependent work. [Durable pause](control.md#11-durable-pause-and-resume)

Run-folder copies can retain request records alongside the plan and event log. The public
export does not copy the request files or make requests answerable. Any separately recorded
workflow values remain subject to the existing [export disclosure rules](exports.md).
For local review, the runner snapshots reviewed file bytes that might lack an ordinary output
placement and the viewer resolves them by recorded digest. Cache pruning keeps the digests
referenced by frozen reviewed values and choices.
