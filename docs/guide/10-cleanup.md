# Cleaning up runs

Every `grida-fx run` without `--run` or `--name` makes a new folder, so a workflow you are
still shaping leaves dozens of exploratory runs behind. `runs list` shows what a project holds;
`runs remove` takes away the runs you no longer need. Neither calls a provider or runs a node.

## See what you have

```sh
grida-fx runs list
grida-fx runs list --workflow rig --json
```

```text
2026-10-09 14:02  succeeded   rig/character_1_rig_ready  $1.20
2026-10-09 13:40  failed      runs/rig/2026-10-09-3  $0.40
2026-10-09 13:10  incomplete  runs/rig/2026-10-09-2  $0.00
```

One line per run, newest first: when it was created, its state, the run (`WORKFLOW_ID/NAME` for
a named run, else its folder), what it spent, and `stand-in` for a stand-in run. The list
includes runs placed anywhere in the `runs` folder with `--run`, and runs outside it, such as an
SDK's run folder, which FX records as they start (`.fx/runs/`, private like `.fx/service/`).

The state is what the run's record says, as `inspect` reads it, except that a run stopped at an
approval (`--yes-up-to`) is `incomplete` rather than `failed`: running it again finishes it.
`unfinished` only means no invocation ended it; a killed run stays `unfinished`, so it is not a
sign that something is running. `empty` is a run folder that holds no run (claimed, or refused
before it started) and `removing` is a removal that could not finish. Folders the list cannot
read, and a `plan.json` that is not FX's, are reported on stderr as skipped.

Listing reads only: it takes no lock and writes nothing.

## Remove what you no longer need

Name the runs, or select them with filters. Nothing is removed without `--yes`: first the
command says what it would do.

```sh
grida-fx runs remove --workflow rig --state failed
grida-fx runs remove --workflow rig --state failed --yes
grida-fx runs remove runs/rig/2026-10-09-3 rig/old_baseline --yes
```

```text
would remove  runs/rig/2026-10-09-3  (failed)
would remove  runs/rig/2026-10-08-1  (failed)
passed    1 named (filters select dated runs only; name a run to remove it)
frees     1.2 kB; 48.3 MB more stays in the cache, which shares it
next      add --yes to remove them
```

- **Filters** (`--workflow ID`, `--state STATE`, repeatable, and `--before YYYY-MM-DD`, a local
  date) select the dated runs FX made for you, `runs/<id>/<date>-<n>`. A named run, a run you
  placed with `--run`, and a run outside the `runs` folder are only ever removed when you name
  them, so a production run called `character_1_rig_ready` survives any filter.
- **A run you name** is a run folder, or `WORKFLOW_ID/NAME`. Anything else, such as the project
  folder, the `runs` folder or a folder that holds other runs, stops the command before it removes
  anything (exit 2). A run you pointed at a folder of your own (`--run renders`) is refused while
  that folder holds files FX did not write: move them out first.
- **Some runs stay.** A run an invocation is running is refused (`active`), and so is the last
  run holding a [pick](04-cost-and-cache.md#takes) (`holds_pick`): pick again, or keep it. A run
  that changed since the runs were read (`changed`) is left alone too; list again.
- A filter selects again when you confirm, so when it matters, pass the exact folders the preview
  showed.

Removing an allocated run lets the next run that day reuse its folder name; the viewer's links
never point a removed run's URL at the new one.

## Disk space

A run folder's files are links to the [cache](04-cost-and-cache.md)'s copies, so removing a run
frees only its own record (`plan.json`, `events.jsonl`) and any file that had to be copied. The
removal says how much stays in the cache, which keeps every result so that the next run reuses it
instead of paying again.

## From an SDK

The SDKs list runs and remove the ones you choose; filtering is yours, so what goes is exactly
what you picked.

```python
from grida.fx import list_runs, remove_runs

failed = [run.run_dir for run in list_runs(workflow="rig") if run.state == "failed"]
if failed:
    preview = remove_runs(failed, preview=True)
    removal = remove_runs(failed)
    for run in removal.runs:
        if run.outcome != "removed":
            print(run.folder, run.code, run.message)
```

```ts
import { listRuns, removeRuns } from "@grida/fx";

const failed = (await listRuns({ workflow: "rig" }))
  .filter((run) => run.state === "failed")
  .map((run) => run.runDir);
if (failed.length > 0) {
  const removal = await removeRuns(failed);
}
```

A refused or partial run is reported in the result; a selection that is not a run raises an
error and removes nothing.

The contracts are in [the store specification](../../spec/store.md#9-listing-and-removing-runs)
and the [list](../../spec/schemas/fx-run-list-v1.schema.json) and
[removal](../../spec/schemas/fx-run-removal-v1.schema.json) schemas. These commands are new in
source: check your installed version's help before using them.
