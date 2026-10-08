# Run control

**Status: cancellation v1 implemented in source, 2026-10-08; unreleased.**
The commands, events and result shapes below describe this checkout. Check an
installed version's help before assuming support. This document owns cancellation v1.
Termination and pause have agreed vocabulary only; their full contracts and
implementation remain separate work.

## 1. Intent and ownership

An agent or person should select a run, request an operation and verify its
outcome without discovering process IDs. Execution belongs to its invoking
runner. The project service and the viewer do not acquire execution ownership.
Cancellation must work with no service, with `--no-view`, and for projectless
and explicit run folders. It neither starts the service nor executes author
code, loads provider credentials, replans, resumes, or submits provider work.

| Person's intent | Operation | Scope |
|---|---|---|
| Stop this run. | `cancel RUN` | Request cancellation of one invocation. Ratified here. |
| Stop it, and tell me when it has ended. | `cancel RUN --wait` | Also verify local completion. Ratified here. |
| Force the stuck execution to end. | `terminate RUN` | Separate, explicit force operation; deferred. |
| Finish active steps, then wait for me. | `pause RUN` | Stop new steps, drain active steps, persist a suspension boundary; deferred. |
| Continue the recorded run. | `run TARGET --resume NAME` or `--run FOLDER` | Existing same-plan/mode behavior; repeat the original options. |
| Shut down the project service. | `stop` | Existing service operation; does not cancel runs. |

`grida-fx` is the installed executable; `python -m grida.fx` reaches the same
engine. No new `fx` executable or `run` subcommand hierarchy is introduced.
There is no implicit latest-run cancellation, bulk cancellation, rollback,
parameter mutation, provider-side job cancellation, or pause emulation.

## 2. Command contract

```sh
grida-fx inspect RUN --control [--json]
grida-fx cancel RUN [--invocation ID] [--wait] [--timeout DURATION] [--json]
```

`RUN` is an explicit run folder or an exact `WORKFLOW_ID/NAME`. Named lookup
uses recorded workflow ID/source within the selected planning project's runs
tree, with the existing bounded traversal and ambiguous-source refusal. An
explicit folder supports unnamed runs and folders outside that tree. A bare
workflow ID is refused even when only one run currently exists. Path spelling
follows existing inspect resolution; ambiguity between a path and a named
selector must be refused, with advice to use an absolute folder path.

`--invocation ID` is an expected-current guard. Without it, resolve the selected
record's current invocation once, then bind the request to that invocation and
its owner instance. If the owner changes before acceptance, refuse; never retry
against its successor automatically. With the guard, any newer invocation is an
`invocation_mismatch` even if the requested predecessor ended. Cancel targets
the latest initialized execution, not arbitrary historical invocations; the
guard also refuses when that newer invocation has already ended.

`inspect --control` uses this same exact target grammar. It reads saved
evidence and performs a bounded local owner handshake, returning the invocation,
recorded state, verified control availability and whether `cancel` is currently
supported. It does not change the ordinary portable `inspect --json` document.
It conflicts with browser/standalone-serving flags. It exposes no token, socket
path, PID, private absolute path or unverified claim of liveness. Agents retain
the target they supplied; initial `run` output must expose the selected folder
and invocation before run-phase work, independently of viewer availability.

The default `cancel` returns after a verified acceptance or a definite no-op;
it does not wait for all work to end. `--wait` waits for the selected invocation,
not for the whole folder to become idle. Its default timeout is 30 seconds.
`--timeout` requires `--wait`, accepts a positive integer followed by `s`, `m`
or `h`, and bounds the entire command's wait from invocation, including discovery
and acknowledgment. Local requests have their own bounded transport deadline.
A wait timeout stops waiting only. It does not retract cancellation, terminate
the runner, or silently select another invocation. Interrupting the waiting
client exits 130 and leaves any accepted request in effect. If interrupted
before acknowledgment, acceptance can remain unknown.

## 3. Request, acceptance and completion

The runner serializes cancellation acceptance with admission of new execution
and with commitment to finalization. There are three distinct boundaries:

1. **Requested:** a client attempted to contact the runner. A timeout or lost
   response does not establish whether the request was accepted.
2. **Accepted:** the runner closed admission for this invocation and appended
   and flushed one `cancel_requested` event before replying. The event has the
   existing invocation/plan envelope and a bounded `source` enum (`cli`,
   `signal`, `sdk`). It is intent evidence, not a terminal event.
3. **Completed locally:** the invocation has a terminal record, local call
   bookkeeping has finished, owned node hosts have been cleaned up within the
   supported process-ownership boundary, and its run writer ownership is released.
   It does not imply remote completion or prove the fate of detached processes
   outside that boundary.

At acceptance, no new step dispatch, provider attempt/resend, or agent turn may
be admitted. Admission and the acceptance transition need a shared synchronization
boundary, not unrelated checks of a flag. Work admitted before acceptance may
already be executing or still reach the provider. It is included in cancellation
and conservative accounting. Do not promise that no network bytes can leave
after acknowledgment, or that completed side effects are undone.

Already admitted node work receives the existing cooperative `$/cancel` and
five-second host grace; then uncooperative owned hosts are ended as specified
by the node protocol. This host deadline does not terminate the bookkeeping
owner. Valid results already committed remain usable; late results follow the
existing cancellation rules. There is no new successful workflow-output
publication for a cancelled invocation.

Finalization may win only after authored node execution and paid attempts have
ended and admission is closed. It is the non-executing output publication,
terminal commit and cleanup phase; it cannot hide active/retrying work behind a
`finishing` label to make it uncancellable.
If finalization wins before acceptance, return `finishing` or `already_terminal`;
do not falsely acknowledge cancellation. With `--wait`, observe that same
invocation's final result. If cancellation wins, normal completion becomes
`run_cancelled`; an engine/store failure can instead produce a failed terminal
record or no terminal record. At most one terminal event is committed per
invocation. Preserve the actual result; never synthesize cancelled from exit.

Concurrent or repeated cancellations of the same owner are idempotent. There
is one acceptance event, and later requests return `already_requested`.
If event persistence fails, close admission and attempt existing fatal cleanup,
but return `record_error`; do not claim durable acceptance. If the reply is lost,
inspection or a retry guarded by the same invocation can resolve the uncertainty;
owner loss or missing evidence can leave it unknown.
No general request queue or operation-history database is needed.

Completion must have positive evidence tied to the selected owner. A terminal
event alone precedes cleanup today; a disappeared socket or a released lock
alone can result from a crash. The implementation must publish a private
completion receipt after local cleanup and writer release, bound to the owner
instance and terminal event. A waiter already bound to invocation A can finish
from its receipt even if invocation B immediately resumes the folder. It must
never cancel or wait for B. Missing/expired receipts leave cleanup unverified;
they do not invalidate recorded terminal history or authorize a force action.

## 4. Structured outcomes

`--json` writes one bounded result to stdout for operational outcomes, including
errors. Human progress belongs on stderr. CLI parse/usage failures retain the
ordinary stderr diagnostic and exit 2; clients must check the exit status and
must not assume every invocation of an unsupported binary returns JSON.

Results use `kind: "fx-run-control-v1"` and lower_snake_case fields:

- `operation`: `inspect` or `cancel`.
- `invocation_id`: the selected invocation, or null before successful resolution.
- `outcome`: one of the values below.
- `request_status`: `accepted`, `not_accepted`, or `unknown`; includes prior
  acceptance only for this invocation, never a predecessor.
- `recorded_state`: existing coarse recorded state, or `unknown`; it is separate
  from live owner/control state.
- `cleanup`: `pending`, `complete`, or `unknown`.
- `external_completion`: `not_verified`. This version makes no general promise
  about provider completion or arbitrary node side effects.
- `code`: a stable error code on failures; `message`: concise human explanation.
- Inspection additionally reports `availability` (`available`, `unavailable`,
  `unsupported`, `unknown`) and `can_cancel` (boolean, conditional on the sampled
  owner; not a promise that a later request will succeed).

All execution fields describe `invocation_id`, never the newest state of the
folder at response time. A waiter for A returns A's terminal result even if B
has begun; it does not report B's unfinished state as A's.

| Outcome | Meaning | Cancel exit |
|---|---|---|
| `inspected` | Read-only control observation; no cancellation performed. | Inspection only, exit 0 |
| `accepted` | Newly accepted; cleanup may still be pending. | 0 without `--wait` |
| `already_requested` | This invocation already accepted cancellation. | 0 without `--wait` |
| `already_terminal` | A terminal record already exists; no cancellation performed. | 0 without `--wait` |
| `finishing` | The owner already committed to finishing; no cancellation performed. | 0 without `--wait` |
| `completed` | Matching terminal result and local completion receipt verified. The result may be succeeded, failed or cancelled. | 0 with `--wait` |
| `error` | Refused, unsupported, unknown outcome, or unverified completion. | See codes below |

Inspection exits 0 when it can report a valid observation, including known
unavailability; invalid target or unreadable evidence exits 2. Cancellation
errors include `invalid_target`, `ambiguous_target`, `invocation_mismatch`,
`unsupported_version`, `unauthorized`, `unavailable`, `record_error`,
`wait_timeout`, `owner_lost`, `acknowledgment_unknown` and
`completion_unverified`. Exit 1 covers `unavailable`, `wait_timeout`,
`owner_lost`, `acknowledgment_unknown` and `completion_unverified`. Exit 2
covers `invalid_target`, `ambiguous_target`, `invocation_mismatch`,
`unsupported_version`, `unauthorized` and `record_error`. Unknown future
outcomes/codes must not be interpreted as success.

An ended legacy run can return `already_terminal` with `cleanup: "unknown"`.
`--wait` must not turn that into verified completion: return a bounded
`completion_unverified` error if no compatible completion evidence exists.
Control-client exit 0 means the outcome described above, not that the workflow
succeeded. The cancelled execution command retains exit 130 on ordinary
cancellation. No command turns an unknown external charge into $0.

The [result schema](schemas/fx-run-control-v1.schema.json) validates these fields.
Control payloads are bounded to 16 KiB; invocation IDs to 256 characters and
human messages to 4,096 characters. A `completed` result requires a non-null
invocation, terminal recorded state and `cleanup: complete`. Both SDKs reject
contradictory outcomes as malformed rather than reporting completion.

## 5. Local discovery and trust

Use a runner-owned Unix-domain socket for the initially supported macOS/Linux
platforms, with private same-user discovery in
`/tmp/grida-fx-<effective_uid>/control-v1/`. Canonicalize the system `/tmp` root
first (on macOS it normally resolves to `/private/tmp`); do not choose a root
from caller-specific `TMPDIR`, SDK working directory or project configuration.
This is short-lived coordination, not a daemon, network API or portable
run record. No socket, credential or PID is stored in `plan.json`/`events.jsonl`.

The versioned directory is deterministic per effective user ID; selected-run
lookup uses a digest of the canonical local folder, not a machine-wide scan.
Discovery records bind that location to the `invocation_id`, a fresh random
owner nonce, a short socket name and a fresh local control token. Socket naming
must fit macOS path limits independently of project/folder length. Owners and
clients validate directory ownership, 0700 directory/0600 file permissions,
regular-file/socket kinds, bounded content, confinement and no symlink traversal
below the canonical temporary root. The handshake checks token, selected
record/plan, invocation and owner nonce before any mutation. No unchecked PID
signaling is allowed. A control client never replaces discovery state to repair
a mismatch. Only a runner that has acquired the selected canonical folder's
`run.lock` may atomically publish its fresh owner pointer, replacing a stale
predecessor pointer without reusing its nonce or token.

Publish readiness only after acquiring `run.lock`, flushing `run_started` and
binding the control endpoint. Register a fresh owner on every resume. Never
delete a successor's discovery record during old-owner cleanup. Private
per-invocation completion receipts bind the same identity and terminal-event
digest; they are temporary evidence, never a second authoritative run history.
Receipt retention/pruning must not change run records or claim completion when
evidence is unavailable. Restart/reboot may remove all control state; portable
inspection and normal resume remain available.

Failure to establish private control must be reported as unavailable while
ordinary execution can continue; existing signal interruption still applies.
No control operation may load workflow code or credentials to repair discovery.
Authenticated local control protects against other users/accidental stale
targeting; it is not isolation from malicious code already running as this user.

The project service may later adapt this interface for a viewer action with
explicit browser mutation protections. Observation HTTP endpoints remain
read-only; merely opening a run URL never authorizes cancellation. No browser
write endpoint, remote worker transport, broker or cloud supervisor is in v1.

## 6. External work, signals and SDKs

Whole-invocation cancellation currently cancels local in-flight sends/collection and settles an
unknown received outcome at the whole reserved amount. Long jobs can remain
`submitting` or `submitted` for reconciliation under [store.md](store.md#5-long-jobs).
This is conservative FX accounting, not evidence of the provider's final bill,
termination or refund. This corrects the older node-protocol wording that promised
waiting for every remote call to finish; no price change or provider cancellation
API is part of this decision. Never automatically resubmit an uncertain job.
This does not supply an exactly-once guarantee for plain synchronous calls:
without a resumable job record, a later explicit resume can submit work whose
earlier remote outcome was unknown. Keep that limitation visible when advising
resume. Individual node timeout and whole-invocation cancellation differ:
timeout alone can leave already-sent paid work running until its outcome;
whole-invocation cancellation ends local waiting with conservative settlement.

First Ctrl-C/SIGTERM and SDK cancellation must enter the same runner acceptance
path once run-phase execution exists. Planning/pre-initialization interruption
remains process-owned and cannot be targeted as a saved run. A control-ready
notification must be independent of the viewer callback, including SDK launches.

The former automatic whole-runner force escalation at 15 seconds in the CLI
and 10 seconds in SDK run cancellation is removed. Retain the five-second
node-host grace.
While local cleanup is pending, report that fact; ordinary waiting deadlines
never grant force authority. Keep a second explicit Ctrl-C/SIGINT as an
emergency force action with unverified-outcome semantics. Repeated SIGTERM or
repeated SDK task/AbortSignal cancellation must remain idempotent cancellation,
not implicitly become force authority. SDK cancellation must
retain cleanup ownership while its caller unwinds; do not silently detach an
unmanaged execution or claim completion because an await was cancelled.
Forced parent/OS exit can still interrupt cleanup and must remain distinguishable.
Non-run command interruption keeps its existing process-owned behavior.

The cancellation delivery includes public Python and JavaScript wrappers for
the same inspect-control and cancel operations, options, outcomes and errors.
Python exposes `inspect_control`, `cancel` and their `_async` variants; JavaScript
exposes `inspectControl` and `cancel`. Both expose `RunControlResult` and
`RunControlError`; neither wrapper computes identity or implements cancellation
itself. See [the user guide](../docs/guide/08-run-control.md) for options and examples.
Cancelling an SDK run task/AbortSignal retains ownership and waits for engine cleanup;
it may therefore remain pending if cleanup is stuck. Cancelling only an SDK
control wait ends that wait and leaves any accepted run cancellation effective.
Test and document these distinct lifetimes. Other CLI/SDK parity gaps remain
the separate backlog task, not prerequisites for this delivery.

This is an unreleased behavior change: CLI/Python/JavaScript parity tests cover
the new ownership lifetime. Include it in the next release notes. SDK run
cancellation sends SIGTERM once with the internal SDK source marker; repeated
task/AbortSignal cancellation never becomes second-SIGINT force authority.

## 7. Observation and compatibility

`cancel_requested` is included in the run-event schema.
The v1 envelope, cursor encoding, existing terminal events, identity/cache/plan
digests, resume checks and recorded creation/name metadata remain unchanged.
Older observation consumers ignore the new event under their existing v1
rule and continue advancing cursors. Strict consumers pinned to the older
event schema need its additive catalog update; do not claim they already
validate the new event.

The bundled client derives a cancelling indicator from `cancel_requested` in
the selected invocation until a terminal event. A later `run_started` resets
that indicator. It is recorded cancellation intent, not evidence the owner is
still alive. Existing `fx-viewer-run-v1.state` stays `unfinished` until the
existing terminal projection changes it; do not add a new value to its closed
enum or change its response shape. Use observed events for the new presentation.
The separate local control result owns verified availability and cleanup status.

## 8. Delivery and acceptance

1. Implement bounded discovery, owner handshake and the shared runner acceptance
   transition; publish control identity before executing run-phase work.
2. Add the event/response schemas, exact-target inspect/cancel adapters and wait
   completion evidence. Keep portable records free of private coordination data.
3. Reconcile signal and SDK cleanup lifetime/timers as part of the same delivery.
4. Project cancellation intent in the existing viewer; update installed usage
   docs/skill only once the surface is implemented and checked.
5. Independently review the behavior and run credential-free fixtures/stand-ins.

Required cases include simultaneous/lost-response/duplicate cancellation;
finalization-versus-cancel and paid-admission-versus-cancel races; stale owners,
reused process IDs, corrupt or forged discovery, symlink/permission refusal;
unnamed/external folders and no-service/`--no-view`/SDK execution; planning before
control readiness; finish before request; accepted request followed by disk or
engine failure; wait timeout/client interrupt without force; A-to-B resume during
discovery and during waiting; node-host cleanup; plain calls with unknown billing
and retained long-job handles; CLI/SDK cancellation parity; old events and strict
viewer schema compatibility. No provider calls are needed for these gates.

Pause needs a separate scheduler/state contract: active steps can still perform
work/spend while draining, so `pausing` and `paused` must differ. Persisted pause
must survive the documented process lifetime and define resume options. A new
`resume RUN` cannot silently reconstruct missing historical inputs or run changed
source. Terminate needs reliable process ownership/cleanup evidence and an honest
forced/unknown outcome; a hung runner cannot certify its own cleanup. Neither
operation is authorized for implementation merely by reserving its vocabulary.

## 9. Implementation and verification

- Runtime `run_control` serializes acceptance, work admission and finalization;
  `control` owns private local discovery, authentication and completion receipts.
- CLI `inspect --control` and `cancel` reuse bounded exact run selection without
  invoking the planner. Early `run` output reports the folder, invocation and
  control availability separately from its viewer URL.
  The existing run-folder line stays on stdout; invocation/control diagnostics
  go to stderr so the final stdout summary remains stable.
- Python `inspect_control[_async]` / `cancel[_async]` and JavaScript
  `inspectControl` / `cancel` forward the same engine operations. Both expose
  `RunControlResult` and `RunControlError`; result fields stay lower_snake_case.
- `fixtures/control` and `tools/check_control.py` exercise timeout without
  retraction, signals, cleanup, checkpoint resume, stale guards, legacy evidence
  and both SDKs at $0. Runtime tests cover admission/finalization and private
  transport races. SDK tests cover cleanup beyond the former force deadline.
- Saved evidence reads are bounded: plan 16 MiB, event log 64 MiB, event line
  4 MiB, and 65,536 events for control inspection. An over-limit or malformed
  record is refused; control does not repair it.
- Private receipts and pointers remain until replaced or system temporary
  storage is cleared. Bounded pruning is separate retention work; unavailable
  evidence never implies completed cleanup.

## 10. Precedents

Reviewed 2026-10-08; these inform intent, not additional FX promises:

- [Docker stop](https://docs.docker.com/reference/cli/docker/container/stop/)
  exposes graceful process shutdown followed by timed force. FX separates caller
  waiting deadlines from force authority because it owns recorded workflow work.
- [Docker pause](https://docs.docker.com/reference/cli/docker/container/pause/)
  freezes local processes; it does not establish an FX workflow boundary.
- [Temporal cancellation and termination](https://docs.temporal.io/encyclopedia/workflow/cancellation-and-termination)
  distinguish requested cooperative cleanup and forceful closure.
- [Argo suspension](https://argo-workflows.readthedocs.io/en/latest/walk-through/suspending/)
  prevents scheduling new steps until resume.
- [GitHub Actions cancellation](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-cancellation)
  addresses a workflow run while its runners own process signaling and escalation.
