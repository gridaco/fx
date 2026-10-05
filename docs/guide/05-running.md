# Running: CLI, Python, agents

## CLI

The command is `grida-fx` (from `npm install -g @grida/fx`, or `npx grida-fx`). Without Node,
the `grida` Python package runs the same engine: `python -m grida.fx <verb>` takes the same
arguments.

| Command | |
|---|---|
| `grida-fx plan <target> [inputs] [--routes file]… [--max-usd N] [--check] [--expect-cached] [--json]` | expand, check, price. Never spends. |
| `grida-fx run <target> [inputs] [--routes file]… [--live] [--max-usd N] [--yes-up-to N] [--deliver out=path ...]` | run; `--live` admits paid calls |
| `grida-fx reroll <run> <step-path> [--live]` / `grida-fx pick <run> <step-path> <take>` | takes ([Cost](04-cost-and-cache.md#takes)) |
| `grida-fx takes list <target>` / `grida-fx takes mv <target> <old> <new>` | inspect or repair a takes file |
| `grida-fx jobs [--forget <key>]` | long provider jobs whose submission has an unknown outcome; `--forget` clears one once you have checked the provider |
| `grida-fx inspect <run or workflow id> [--verify] [--json]` | summary; `--verify` re-checks every file against its record |
| `grida-fx nodes [type]` | built-in and project node types, with settings and routes |
| `grida-fx schema <target>` | the JSON Schema its `inputs:` compile to |
| `grida-fx doctor [target]` | the keys, tools and routes a workflow needs |
| `grida-fx lock [where] [--same <node>]… [--check]` | node version locks; repeat `--same` to confirm several |
| `grida-fx expand`, `identity`, `price <target> [inputs]` | the expanded graph, each instance's identity, the price by phase, as JSON |
| `grida-fx project <run>` | a run's record projected to its state, as JSON |

`grida-fx --help` lists what your installation has.

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

- **`events.jsonl` is the source of truth** (an `fx-run-events-v1` log); `inspect` and `project`
  are built from it. `plan.json` is the same `fx-graph-v1` document `grida-fx expand` prints.
- **Deleting is safe:** results live in the cache.
- **Copying is safe:** a run folder can be inspected on another machine.

A run that stops early (a failed step with `on_reject: fail`, a refused assertion, the ceiling) is
*incomplete*. Everything it finished is kept and shown, and re-running continues from there.

## Delivering outputs

From the [game-build](examples/game-build/) example's `kitewharf/assets/`:

```bash
grida-fx run level_art.py:build --arg level=../levels/docks.toml --live \
  --deliver plate=../game/art/docks/ground.png \
  --deliver icons=../game/art/docks/icons/{key}.png
```

- **Delivery copies the declared outputs that exist,** after the run. It is idempotent: unchanged
  files aren't rewritten.
- **It lists anything missing** (a skipped instance, an incomplete run), and the command then exits
  non-zero.
- **Delivering a partial result is safe:** every delivered file was verified against its record.

## From Python

The Python SDK (`pip install grida`) drives the same engine as the command, and needs no Node.

```python
from grida.fx import run

result = run(
    "workflows/gallery.yaml",  # a workflow, an id, a builder's Workflow, or a plan
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

- **Planning first:** `grida.fx.plan(...)` returns the plan, with `.estimate()`, `.phases()` and
  `.problems`. `grida.fx.run(plan, ...)` runs it.
- **Async:** `await grida.fx.run_async(...)`.
- **A refused plan raises.** A failed step doesn't: check `result.ok` and `result.failed`.

## Agents

A coding agent drives FX through the same command. It can write a workflow file and `plan` it:
a plan is free, so an agent can iterate on a workflow before spending anything. `grida-fx schema`
gives it the JSON Schema of a workflow's inputs, and `plan --json`, `inspect --json`, `expand` and
`price` give it JSON to read. A run that may spend still needs `--live` and a ceiling.
