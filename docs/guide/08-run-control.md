# Stopping and continuing a run

`cancel` stops one invocation without losing completed work. It works independently
of the project service, including SDK launches, `--no-view` and projectless runs.
`stop` shuts down the inspection service; it does not stop workflow execution.

## Select, request, verify

```sh
grida-fx inspect gallery/baseline --control --json
grida-fx cancel gallery/baseline
grida-fx cancel gallery/baseline --wait --timeout 30s --json
```

The target is an explicit run folder, such as `runs/example`, or an exact
`WORKFLOW_ID/NAME`. A bare workflow ID cannot select an implicit latest run.
Use an absolute folder path if a relative path could also identify a named run.

`inspect --control` reports the current invocation, recorded state, verified local
control availability and `can_cancel`. It never runs author code or calls providers.
It is separate from ordinary `inspect --json` and cannot serve or open a browser.

For agents, retain the inspected invocation and guard the request:

```sh
grida-fx cancel gallery/baseline --invocation INVOCATION_ID --wait --json
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
Cancellation is not durable pause. `pause` and `terminate` remain deferred.

The [provider-free harness](../../fixtures/control/) demonstrates timeout and resume.
The [normative contract](../../spec/control.md) defines outcomes, error codes,
completion evidence and local trust boundaries.
