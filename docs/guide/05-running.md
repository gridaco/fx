# Running: CLI, Python, agents

## CLI

The command is `grida-fx` (from `npm install -g @grida/fx`, or `npx grida-fx`). Without Node,
the `grida` Python package runs the same engine: `python -m grida.fx <verb>` takes the same
arguments.

| Command | |
|---|---|
| `grida-fx plan <target> [inputs] [--routes file]… [--max-usd N] [--check] [--expect-cached] [--json]` | expand, check, price. Never spends. |
| `grida-fx run <target> [inputs] [--routes file]… [--live] [--max-usd N] [--yes-up-to N] [--deliver out=path]… [--run folder]` | run; `--live` admits paid calls and needs a ceiling; `--run` names the run folder |
| `grida-fx reroll <run> <step-path> [--live]` / `grida-fx pick <run> <step-path> <take>` | takes ([Cost](04-cost-and-cache.md#takes)) |
| `grida-fx takes list <target>` / `grida-fx takes mv <target> <old> <new>` | inspect or repair a takes file |
| `grida-fx jobs [--forget <key>]` | the cache's long provider jobs, `settled` ones included ([Resuming](04-cost-and-cache.md#resuming)); `--forget` clears one once you have checked the provider |
| `grida-fx inspect <run or workflow id> [--verify] [--json]` | summary; `--verify` re-checks every placed file against its record |
| `grida-fx nodes [type]` | built-in and project node types, with settings and routes |
| `grida-fx schema <target>` | the JSON Schema its `inputs:` compile to |
| `grida-fx doctor [target]` | the keys, tools and routes a workflow needs |
| `grida-fx lock [where] [--same <node>]… [--check]` | node version locks; repeat `--same` to confirm several |
| `grida-fx expand`, `identity`, `price <target> [inputs]` | the expanded graph, each instance's identity, the price by phase, as JSON |
| `grida-fx project <run>` | a run's record projected to its state, as JSON |

`grida-fx --help` lists what your installation has.

**Exit status.** `0` when the command did what it was asked: for `run`, the run is ok and every
`--deliver` found its output. `1` when it read everything and refused or stopped: a plan with
problems, a run refused before it started (a live run without a ceiling, a folder that holds
another plan), a run that is not ok, an output that `--deliver` did not find. `2` for unreadable
input or a command-line mistake, with `grida-fx: <message>` on stderr. `130` when you interrupt a
run (Ctrl-C): what finished is kept ([Resuming](04-cost-and-cache.md#resuming)).

**Route tables.** Routes and their prices come from FX's built-in table, then the tables `fx.yaml`
lists under `route_tables`, then each `--routes` file in the order given. A later table's entry
for the same capability and route replaces an earlier one, and giving any `--routes` file leaves
the built-in table out ([identity.md §7](../../spec/identity.md#7-route-fingerprint)).

`<target>` is one of:
- a workflow file (`workflows/gallery.yaml`);
- a workflow id (`concept-gallery`);
- a Python builder (`level_art.py:build`), whose arguments are passed with `--arg name=value`.

## Inputs

**Ways to pass them:**
- **Flags:** the kebab-case form of each input (`--poster inputs/poster.png`,
  `--max-entities 24`).
- **Files:** `--inputs file.yaml`, repeatable and merged in order.
- **Both together,** with flags winning.

**Paths and lists:**
- **Paths inside an inputs file** are relative to that file.
- **List and map inputs** need a file. A `files` input also takes a glob: `--sprites 'art/*.png'`.

```yaml
# inputs/harbor.yaml, from the looping-parallax example
canvas: { width: 640, height: 360 }
layers:
  - { id: far_cliffs, file: ../art/far_cliffs.png, order: 0, parallax: 0.2 }
  - { id: near_masts, file: ../art/near_masts.png, order: 1, parallax: 0.7, repeat: repaint }
```

Both layers leave out `offset_y`, and `far_cliffs` leaves out `repeat`: those fields have
defaults (`0` and `mirror`). `order` has none, so every layer gives it.

## Runs

Each run is a folder:

```
runs/concept-gallery/2026-10-02-1/
  events.jsonl       # everything that happened, in order (the record)
  plan.json          # the plan the run started from: workflow, inputs, every step instance
  outputs/           # the declared outputs, by name and key
  files/             # every step's results, by step path
```

- **Where:** `runs/<workflow id>/<date>-<n>/` in the project you run from, a new folder each
  time; `--run <folder>` names one instead, relative to where you are.
- **`events.jsonl` is the source of truth** (an `fx-run-events-v1` log); `inspect` and `project`
  are built from it. `plan.json` is the same `fx-graph-v1` document `grida-fx expand` prints, and
  names the takes file `reroll` and `pick` write.
- **Deleting is safe:** results live in the cache.
- **Copying is safe:** a run folder can be inspected on another machine.

A failed step does not stop the run: its body failed, a paid call was refused (the ceiling, no
`--live`), or an assertion did not hold. The steps that need it are skipped, everything else
runs, and the run ends *failed*, naming each failure. A run that stops before it is done (a phase
past `--yes-up-to`, an interruption) is *incomplete*. Everything a run finished is kept and
shown, and running the same command into the same folder (`--run`) continues from there; a
folder that holds a run of another workflow or other inputs is refused.

`run` prints the plan, then a summary, each label in a column of ten:

```
run       runs/one
result    failed   spent $0.00
failed    nope#1: refused on purpose
failed    after#1: something it reads failed
failed    bang#1: RuntimeError: kaboom y
```

`result` is `ok`, `incomplete` or `failed`; a `failed` line follows for each step that failed or
was skipped, in the plan's order; `stopped` says why an incomplete run stopped. A run refused
before it starts prints `refused: <why>` instead (exit 1). A body that raises fails with
`<exception type>: <message>`; a step past its `timeout:` with `ran past <n> seconds`, and it is
not run again, even under `retry="engine"`.

**After a run.** These commands never run a step or spend anything:

- **`grida-fx inspect <run>`:** the run's state and a count of its steps' states, what it spent,
  and each failure. Given a workflow id instead of a folder, it reads that workflow's newest run.
  `--verify` rehashes every file its steps placed and exits 1 if one differs from its record;
  `--json` prints the same summary as JSON, each step with its files.
- **`grida-fx project <run>`:** the record projected to its state, as JSON: each instance's
  `state` (`running`, `succeeded`, `failed` or `skipped`), with `cache` and `facts` when it
  succeeded or `error` when it did not, and the latest `run_started`, `run_finished` and
  `run_cancelled` events.
- **`grida-fx reroll <run> <step-path>`:** writes the take after the latest one this run drew of
  that step into the takes file the run's `plan.json` names, and prints the next command:

  ```
  icon.takes.yaml: draw uses take 2 from now on
  next      grida-fx run icon
  ```

- **`grida-fx pick <run> <step-path> <take>`:** writes that take into the takes file, with the
  digest of the first port holding one file in the outputs that take finished with in this run
  (a take this run never finished is written without one). A later run fails the step if the
  take no longer produces that file ([Cost](04-cost-and-cache.md#rerolls-and-picks)).
- **`grida-fx takes list <target>`:** one line per entry of a workflow's takes file,
  `<step-path>  take <n>[  <digest>]`; `takes mv <target> <old> <new>` moves an entry to a step's
  new path and refuses to overwrite another entry. Neither takes a builder target.
- **`grida-fx jobs`:** the cache's long provider jobs, one line each: `<key>  <state>  <capability>
  on <route>, take <n>`. `--forget <key>` removes one, so the next run submits its call anew.

## Delivering outputs

From the [game-build](examples/game-build/) example's `kitewharf/assets/`:

```bash
grida-fx run level_art.py:build --arg level=../levels/docks.toml --live \
  --deliver plate=../game/art/docks/ground.png \
  --deliver icons=../game/art/docks/icons/{key}.png
```

- **Names are checked before running:** an output the workflow does not declare is refused (exit
  2) before anything is planned or run.
- **Delivery copies the declared outputs that exist,** after the run, to paths relative to where
  you are. `{key}` stands for each element of a list or keyed output (its key, else its
  position), made a safe path; an output of one file takes a path without it. It is idempotent:
  unchanged files aren't rewritten.
- **It lists anything missing** (a skipped instance, an incomplete run), and the command then exits
  non-zero.
- **Delivering a partial result is safe:** every delivered file was verified against its record.

## From Python

The Python SDK (`pip install grida`) drives the same engine as the command, and needs no Node.

```python
from grida.fx import run

result = run(
    "workflows/gallery.yaml",  # a workflow file, an id, a builder "file.py:function", or a plan
    inputs={"synopsis": "inputs/synopsis.md", "poster": "inputs/poster.png"},
    live=True,
    max_usd=10,
)
print(result.ok, result.cost, result.incomplete)
for key, image in result.outputs["images"].items():  # keyed collection
    verdict = result.steps[f"entity['{key}'].review"].facts["verdict"]
    print(key, image.path, verdict)
result.deliver({"images": "out/{key}.png"})
```

- **Targets** are what the command takes: a workflow file, an id, or a builder
  `"file.py:function"` with its arguments as `arguments={"level": "levels/docks.toml"}`. A
  `Workflow` object a builder returns is not accepted yet (`TypeError`): name the builder instead.
- **Planning first:** `grida.fx.plan(...)` returns the plan, with `.estimate()`, `.phases()` and
  `.problems`. `grida.fx.run(plan, live=True)` runs it with the target, inputs and ceiling it was
  planned with (`live`, `yes_up_to` and `run_dir` may still be given).
- **Async:** `await grida.fx.run_async(...)`.
- **A refused plan raises** `PlanRefused`, and a run refused before it starts raises `FxError`. A
  failed step doesn't: check `result.ok` and `result.failed`.

## Agents

A coding agent drives FX through the same command. It can write a workflow file and `plan` it:
a plan is free, so an agent can iterate on a workflow before spending anything. `grida-fx schema`
gives it the JSON Schema of a workflow's inputs, and `plan --json`, `inspect --json`, `expand` and
`price` give it JSON to read. A run that may spend still needs `--live` and a ceiling.
