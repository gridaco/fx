# Working with runs from an SDK

Use Python or JavaScript to execute workflows, return to saved records, and observe
work running in another process. The pause, resume and attempt-history APIs
described here require FX 0.13 or later. Neither SDK needs a running FX service
for record access. [SDK run access](../../spec/sdk.md) defines their boundary.

## Give a result a name

Use a local `workflow.yaml` with no required inputs, or with defaults for its
inputs. Plan it and create a named run:

```python
from grida.fx import load_run, plan, run

planned = plan("workflow.yaml")
result = run(planned, name="character_1_rig_ready")
print(result.ok, result.run_dir)

record = load_run(result.run_dir, verify=True)
print(record.inspection["run"]["state"])
print(record.inspection["verification"])
```

```ts
import { loadRun, plan, run } from "@grida/fx";

const planned = await plan("workflow.yaml");
const result = await run(planned, { name: "character_1_rig_ready" });
console.log(result.ok, result.runDir);

const record = await loadRun(result.runDir, { verify: true });
console.log(record.inspection.run.state, record.inspection.verification);
```

Names are create-only. Repeating that creation fails. Omitting `name`, `resume`
and `run_dir` / `runDir` creates an independent run at attempt 1. Matching work
can still come from cache. Paid calls require explicit live admission and a
current ceiling where applicable.

## Pause, resume or rerun

Use `pause(run)` from a separate caller to let active steps finish and save a
durable pause. Use `resume(run)` to continue that attempt from its recorded
execution context. If a saved record lacks that context, supply the original
target and options to `run` with its explicit folder or the `resume` option.
See [Pause and resume at a step boundary](08-run-control.md#pause-and-resume-at-a-step-boundary).

Use `rerun(run)` to create another attempt using the verified context of a
saved run. A rerun keeps the run ID, opens fresh review requests, and can reuse
normal cached results. Use the attempt folder or pass `attempt` to choose a
source explicitly. Resume and rerun require current authorization for live work.

Passing an `inputs` mapping to the SDK uses a temporary file, so the run lacks
the saved context that automatic resume and rerun require. To resume such a run,
pass the original target and inputs to `run` with the saved attempt folder. To
make a run eligible for automatic rerun, use workflow defaults or launch it from
the CLI with a persistent `--inputs` file inside the project.

For example, after creating the local run above, rerun its exact attempt and
look up the first result from the history:

```python
from grida.fx import load_run, rerun, run_history

again = rerun(result.run_dir)
history = run_history(again.run_dir)
first = load_run(history.run_id, attempt=1)
print([entry.attempt_number for entry in history.attempts], first.run_dir)
```

```ts
import { rerun, runHistory } from "@grida/fx";

const again = await rerun(result.runDir);
const history = await runHistory(again.runDir);
const first = await loadRun(history.run_id, { attempt: 1 });
console.log(history.attempts.map((entry) => entry.attempt_number), first.runDir);
```

## Return to a saved run

Pass a run ID or `workflow_id/name` to load its latest attempt. Pass the exact
attempt folder or use `attempt` to open a particular result. A bare workflow ID
selects its newest independently created run, including failures, and refuses
ambiguous sources. The returned record stays pinned to the resolved attempt.

Python `load_run` and async `load_run_async`, or JavaScript `loadRun`, return a
`RunRecord` with an absolute folder and the engine's inspection document. It is a
saved summary, with recorded state, charges, step failures and relative placed-file
paths. It has no invocation process exit code. With verification enabled, inspect
the returned `verification.verified` and `problems`; a failed verification returns
evidence rather than pretending the artifacts are valid.
Load again to refresh the summary. Snapshot and event methods perform new reads.

For portable review or publication preparation, use the record's `export` method.
[Share a recorded run](11-exports.md) covers disclosure, omissions and immutable
content references. Exporting spends nothing and does not publish the files.

For a completed SDK execution, keep using its `RunResult` output file objects and
delivery API. Record inspection also works on active, failed and copied runs. A
copied record does not need its original cache or workflow source for inspection.
Existing `RunResult` construction reads the folder after execution ends; avoid
assuming an immutable invocation capture if another process immediately resumes it.

## Follow an existing run

Load the initialized record, apply a consistent snapshot, then follow from its
cursor. This works while a CLI or another process owns execution:

```python
import asyncio
from grida.fx import load_run_async

async def watch():
    record = await load_run_async("my_workflow/character_1_rig_ready")
    snapshot = await record.snapshot_async()
    for event in snapshot["events"]:
        print(event["invocation_id"], event["event"])
    async for batch in record.follow(after=snapshot["cursor"]):
        for event in batch["events"]:
            print(event["invocation_id"], event["event"])
        # Checkpoint batch["cursor"] only after applying the whole batch.

asyncio.run(watch())
```

```ts
import { loadRun } from "@grida/fx";

const record = await loadRun("my_workflow/character_1_rig_ready");
const snapshot = await record.snapshot();
for (const event of snapshot.events) {
  console.log(event.invocation_id, event.event);
}
for await (const batch of record.follow({ after: snapshot.cursor })) {
  for (const event of batch.events) {
    console.log(event.invocation_id, event.event);
  }
  // Checkpoint batch.cursor only after applying the whole batch.
}
```

Following continues through completion and later resumes. Break the iterator,
cancel its Python observer task, or abort its JavaScript observer signal to stop
watching. That affects only observation. Cancelling `run_async` or the signal
passed to `run` instead requests execution cancellation and waits for owned
cleanup. [Run control](08-run-control.md) covers exact-target cancellation.

A single-invocation consumer can stop after applying that invocation's terminal
event (`run_finished`, `run_cancelled`, or `run_paused`). Unfinished evidence does not prove a
process is alive; a crash may have no terminal event. A terminal event also does
not certify cleanup. Use control's verified completion when that guarantee matters.

For one-shot reads, use `record.events(after=cursor, limit=256)` in Python
(`events_async` inside an event loop), or `record.events({ after: cursor, limit: 256 })`
in JavaScript. The default replays from the beginning. Catch up while `has_more`
is true. Snapshot and inspection are independently captured samples.

`RunObservationError` retains a structured reader code and document. Invalid or
changed cursors require an explicit reattachment; no events are silently skipped.
Unknown event names/fields pass through so callers can keep their cursor advancing.
Validate recognized payloads before taking external actions.

SDK execution still returns after completion and suppresses viewer hosting. Use
the CLI for early browser URLs and project service lifecycle; observing a record
does not start either a service or a workflow.
