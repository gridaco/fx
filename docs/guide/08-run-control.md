# Stopping and continuing a run

`cancel` stops one invocation without losing completed work. It works independently
of the project service, including SDK launches, `--no-view` and projectless runs.
`stop` shuts down the inspection service; it does not stop workflow execution.

## Control a run in the viewer

On a local project run page, open **Run actions**. Choose **Pause** to let
started steps finish, **Resume** to continue the paused attempt, **Rerun** to
start another attempt from the selected record, or **Cancel** to stop the
selected active invocation. The confirmation dialog for **Rerun** and
**Cancel** shows which attempt the action affects. When the rerun starts, the
viewer opens the new attempt in the same tab.
Use the browser's Back button to return to the source attempt.

Execution continues when you close the tab. When a viewer-started attempt
requests a review, answering lets that same invocation continue.
If a start needs live spending
approval or FX cannot reconstruct its execution context, follow the CLI guidance
in the viewer. Use attempt history to open an earlier result.

## Pause and resume at a step boundary

```sh
grida fx pause gallery/baseline --wait --json
grida fx resume gallery/baseline
```

Pause closes admission to new steps. Steps already underway can finish,
including calls covered by their existing spending authorization. A reply
with `pausing` means FX has accepted the request. A reply with `paused` and
`cleanup: complete` verifies that the invocation reached the durable pause
boundary and released the run. `cancel` can interrupt the drain if active work
is taking too long. Any pending workflow request retains its deadline and can
accept a reply while the runner is away.

Resume starts a new invocation of the same run attempt. FX plans from the
recorded working directory and verifies the same plan, mode and any files
bound by input defaults. For a live run,
pass `--live` with the applicable spending approval as on `run`. Include
`--require-estimates` to require forecast coverage for the resumed work. If the record
lacks execution context, repeat the original options with `run TARGET --resume
NAME` or `run TARGET --run FOLDER`. Python and JavaScript expose `pause` and
`resume` for the same jobs. Python also has `pause_async` and `resume_async`.

## Select, request, verify

```sh
grida fx inspect gallery/baseline --control --json
grida fx cancel gallery/baseline
grida fx cancel gallery/baseline --wait --timeout 30s --json
```

The target is an explicit attempt folder, a run ID or `WORKFLOW_ID/NAME`. An ID
or name selects that run's latest attempt. Pass the exact folder to keep control
bound to an earlier attempt. A bare workflow ID cannot select an implicit
latest run. Use an absolute folder path if a relative path could also identify
a named run.

`inspect --control` reports the current invocation, recorded state, verified local
control availability, `can_cancel` and `can_pause`. It never runs author code or calls providers.
It is separate from ordinary `inspect --json` and cannot serve or open a browser.

For agents, retain the inspected invocation and guard the request:

```sh
grida fx cancel gallery/baseline --invocation INVOCATION_ID --wait --json
```

A newer invocation refuses this guard, including when it has already finished.
Without the guard, the command binds the current invocation once and never retargets
a later resume. `run` prints its folder and invocation before run-phase work starts,
even when no viewer is available.

## Acceptance and completion differ

Without `--wait`, `accepted` means admission is closed and cancellation intent has
been persisted. Existing steps receive cooperative cancellation; an unresponsive
owned node host has a five-second grace. No new steps, provider attempts or agent
turns are admitted. Already admitted work can still reach a provider.

With `--wait`, `completed` and `cleanup: complete` verify the selected invocation's
terminal record, local bookkeeping, owned host cleanup and released writer ownership.
The recorded result can be cancelled, succeeded or failed. Cancellation that loses
the finalization race reports `finishing` or `already_terminal` instead of acceptance.

Wait defaults to 30 seconds. `--timeout` requires `--wait` and takes a positive
integer with `s`, `m` or `h`. A timeout or interrupted waiter ends only that wait;
accepted cancellation remains effective. Reissue the guarded request to check again.
Repeated requests are idempotent. A copied or older run can have terminal history
without local cleanup evidence: `--wait` reports `completion_unverified`.

`external_completion` always remains `not_verified`. FX conservatively settles an
unknown paid outcome at its full reservation. It cannot promise remote cancellation,
a refund or the provider's final invoice. An explicit resume may repeat an uncertain
plain synchronous call; recorded resumable jobs have separate reconciliation rules.

## Signals and SDKs

First Ctrl-C or SIGTERM follows the same acceptance path once execution is initialized.
There is no automatic whole-runner force timer. Repeated SIGTERM remains idempotent.
A second explicit Ctrl-C is an emergency force exit and can leave cleanup unverified.
Planning interruption and non-run SDK calls keep their process-owned behavior.

Python exposes synchronous and asynchronous operations:

```python
from grida.fx import RunControlError, cancel, inspect_control

current = inspect_control("gallery/baseline")
if current.invocation_id is None:
    raise RuntimeError("No initialized invocation to target")
try:
    result = cancel("gallery/baseline", invocation=current.invocation_id,
                    wait=True, timeout="30s")
    print(result.recorded_state, result.cleanup)
except RunControlError as error:
    print(error.code, error.result.request_status)
```

Use `inspect_control_async` and `cancel_async` inside an event loop. JavaScript:

```ts
import { cancel, inspectControl, RunControlError } from "@grida/fx";

const current = await inspectControl("gallery/baseline");
if (current.invocation_id === null)
  throw new Error("No initialized invocation to target");
try {
  const result = await cancel("gallery/baseline", {
    invocation: current.invocation_id, wait: true, timeout: "30s",
  });
  console.log(result.recorded_state, result.cleanup);
} catch (error) {
  if (error instanceof RunControlError)
    console.log(error.code, error.result.request_status);
  else throw error;
}
```

Control results retain the wire contract's `lower_snake_case` fields in both SDKs.
Wrappers raise `RunControlError` for structured operational errors and ordinary
`FxError` for unsupported binaries or malformed results. Check installed versions
before using new commands; source support does not upgrade an older installed package.

Cancelling a Python `run_async` task or aborting a JavaScript `run` sends one graceful
SIGTERM and waits for engine cleanup before unwinding. It may remain pending if
cleanup is stuck; repeated SDK cancellation cannot grant force authority. Aborting
only an SDK control waiter ends that waiter and leaves an accepted request in effect.
To control a pending SDK run, choose `run_dir`/`runDir`, retain it and call the control
operation separately. There is no live SDK viewer handle.

## Continue later

Repeat the original target, inputs and options with `--resume NAME` or the same
`--run FOLDER`. FX requires the same plan and execution mode, preserves recorded
spend and reuses valid saved work. It restores no old source or input options.

The [provider-free harness](../../fixtures/control/) demonstrates timeout and resume.
The [normative contract](../../spec/control.md) defines outcomes, error codes,
completion evidence and local trust boundaries.
