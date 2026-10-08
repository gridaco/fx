# Running: CLI, Python, agents

## CLI

The command is `grida-fx` (from `npm install -g @grida/fx`, or `npx @grida/fx`). Without Node,
the `grida` Python package runs the same engine: `python -m grida.fx <verb>` takes the same
arguments. Install it with `pip install grida`. Both packages are published as stable
releases; [the source setup](../../CONTRIBUTING.md#build-from-source) is for development.

| Command | |
|---|---|
| `grida-fx init [directory] [--json]` | establish a project without overwriting existing configuration |
| `grida-fx start [--project directory] [--port N] [--background] [--open] [--json]` | serve the project's dashboard and recorded runs; foreground by default |
| `grida-fx status [--project directory] [--json]` / `stop [--project directory] [--json]` | inspect or stop that project's service |
| `grida-fx logs [--project directory] [--lines N]` | read a bounded tail of service logs |
| `grida-fx inspect RUN --control [--json]` | observe an exact invocation's local control availability |
| `grida-fx cancel RUN [--invocation ID] [--wait] [--timeout DURATION] [--json]` | request cancellation; optionally verify local completion ([Run control](08-run-control.md)) |
| `grida-fx plan <target> [inputs] [--routes file]… [--max-usd N] [--check] [--expect-cached] [--json] [--open] [--standalone]` | expand, check, price; optionally open the plan canvas. Never spends. |
| `grida-fx run <target> [inputs] [--routes file]… [--live] [--max-usd N] [--yes-up-to N] [--deliver out=path]… [--name NAME \| --resume NAME \| --run folder] [--stand-in file.py#function] [--open \| --no-view] [--standalone]` | run; `--live` admits paid calls, needs a ceiling and reads the provider keys; `--name` creates a named run, `--resume` continues it, `--run` selects a folder; `--stand-in` answers paid calls with your function instead, offline ([below](#stand-ins-testing-without-a-provider)) |
| `grida-fx reroll <run> <step-path> [--live]` / `grida-fx pick <run> <step-path> <take>` | takes ([Cost](04-cost-and-cache.md#takes)) |
| `grida-fx takes list <target>` / `grida-fx takes mv <target> <old> <new>` | inspect or repair a takes file |
| `grida-fx jobs [--forget <key>]` | the cache's long provider jobs, `settled` ones included ([Resuming](04-cost-and-cache.md#resuming)); `--forget` clears one once you have checked the provider |
| `grida-fx inspect <folder or workflow id or workflow id/name> [--verify] [--json] [--open] [--standalone]` | summary; a workflow ID selects its newest-created run; `--verify` re-checks every placed file; `--open` opens its canvas |
| `grida-fx nodes [type]` | built-in and project node types, with settings and routes |
| `grida-fx schema <target>` | the JSON Schema its `inputs:` compile to |
| `grida-fx doctor [target]` | the keys, routes and tools a workflow needs ([below](#keys-and-live-runs)) |
| `grida-fx lock [where] [--same <node>]… [--check]` | node version locks; repeat `--same` to confirm several |
| `grida-fx expand`, `identity`, `price <target> [inputs]` | the expanded graph, each instance's identity, the price by phase, as JSON |
| `grida-fx project <run>` | a run's record projected to its state, as JSON |
| `grida-fx observe <run> [--after cursor] [--limit 256]` / `observe <run> --snapshot` | consistent snapshot or bounded recorded events, as JSON ([contract](../../spec/observation.md)) |

`grida-fx --help` lists what your installation has. These service commands describe
current source behavior; installed packages gain it when that source is released.
Use `init`, then `start --background` for normal project work. The service uses
loopback port 8787 by default and keeps run pages available after execution exits.
`run` prints an available run URL after initialization and before run-phase steps;
`--open` requests browser launch. Commands never start a background service implicitly.
Execution still works when the service is stopped.

## Name a run or continue it

```sh
grida-fx run character-rig
grida-fx run character-rig --name character_1_rig_ready
grida-fx run character-rig --resume character_1_rig_ready
grida-fx inspect character-rig --open
grida-fx inspect character-rig/character_1_rig_ready --open
```

Without a name, each command creates a fresh run record; matching cached results
can still make it finish immediately. `--name` creates once and refuses an existing
name. `--resume` requires that name and the same current plan and execution mode:
supply the same inputs, routes, takes and builder arguments again. It appends an
invocation, retains recorded work and spending, and never replaces history.
`--run PATH` remains the explicit create-or-resume folder interface, including
for SDK users. The three options are mutually exclusive; there is no overwrite.

Names are case-sensitive, start with a letter or digit, and contain at most 64
letters, digits, underscores or hyphens. They are scoped to one project and
recorded workflow ID/source. Inspection refuses an ID shared by several sources;
use an actual run folder to select one precisely. A changed workflow definition
or inputs needs a fresh run, and stays under the same workflow in the index.

Put workflow inputs that overlap CLI options after `--`, or use `--inputs FILE`:
`grida-fx run greeting --name baseline -- --name Ada`. Plan commands still accept
`grida-fx plan greeting --name Ada` as an authored input. Check installed help:
named runs, like the project service, describe current source until released.

For independent tests and probes, `run --standalone` hosts its own viewer on an
available port until execution ends. Projectless runs use this model by default.
`plan --standalone` and `inspect --standalone` host their selected data until
interrupted. `--standalone` does not disable cache reuse. `--no-view` skips run
viewer integration and conflicts with `--open`; SDK runs use it. JSON plan/inspect
output cannot be combined with browser-serving flags. See
[Viewing](07-viewing.md) for the lifecycle and independent observation commands.

**Exit status.** `0` when the command did what it was asked: for `run`, the run is ok and every
`--deliver` found its output. `1` when it read everything and refused or stopped: a plan with
problems, a run refused before it started (a live run without a ceiling, a folder that holds
another plan), a run that is not ok, an output that `--deliver` did not find. `2` for unreadable
input or a command-line mistake, with `grida-fx: <message>` on stderr. `130` for
process-owned interruption, including planning, or accepted run cancellation
(Ctrl-C, SIGTERM or `cancel`): owned node hosts are cleaned up and completed work
is kept ([Resuming](04-cost-and-cache.md#resuming)). If finalization already won,
the execution can preserve its actual terminal result and normal exit status.

Control commands have their own [outcomes and exit codes](08-run-control.md).
First Ctrl-C/SIGTERM requests graceful run cancellation, with no automatic runner
force timer. A second explicit Ctrl-C is emergency force; repeated SIGTERM is
idempotent. Cancelling a separate waiter never stops waiting by forcing the run.

**Route tables.** Routes and their prices come from FX's built-in table, then the tables `fx.yaml`
lists under `route_tables`, then each `--routes` file in the order given. A later table's entry
for the same capability and route replaces an earlier one, and giving any `--routes` file leaves
the built-in table out ([identity.md §7](../../spec/identity.md#7-route-fingerprint)).

The built-in table ships with the command. Its prices are planning allowances in US dollars, not
provider quotes: what a call costs is settled from what the provider reports, else from the whole
amount held for it ([providers.md §10](../../spec/providers.md#10-the-built-in-route-table)).

| Capability | Built-in routes | Key |
|---|---|---|
| `image.generate` | `gpt-image-2.5-sunburst@openai` (a call, by `size`: $0.09 – $0.70), `openai/gpt-image-2.5-sunburst@openrouter` ($0.13 – $0.70), `openai/gpt-image-2.5/sunburst@fal` ($0.09 – $0.70) | each provider's |
| `image.edit` | `gpt-image-2.5-sunburst@openai` (a call, by `size`: $0.09 – $0.85), `openai/gpt-image-2.5-sunburst@openrouter` ($0.13 – $0.85), `openai/gpt-image-2.5/sunburst@fal` ($0.09 – $0.85) | each provider's |
| `structured.generate` | `openai/gpt-5.6-sol@openrouter` ($0.02 – $0.60), `openai/gpt-6-astra@openrouter` ($0.05 – $1.50) | `OPENROUTER_API_KEY` |
| `agent.turn` | `openai/gpt-5.6-sol@openrouter` ($0.003 – $0.10), `openai/gpt-6-astra@openrouter` ($0.01 – $1.50) | `OPENROUTER_API_KEY` |
| `music.generate` | `google/lyria-3-pro-preview@openrouter` ($0.05 – $0.50) | `OPENROUTER_API_KEY` |
| `video.generate` | `google/gemini-omni-flash/v1.1/image-to-video@fal` (per second, by `resolution`, at most 10 s: $0.03 – $0.375) | `FAL_KEY` |
| `mesh.generate`, `mesh.rig` | `P2-20260801@tripo` ($1.20 – $2.50), `v1.0-20240301@tripo` ($0.25 – $0.50) | `TRIPO_API_KEY` |
| `sound.generate`, `speech.generate` | `eleven_text_to_sound_v2@elevenlabs` ($0.001 – $0.10), `eleven_v3@elevenlabs` ($0.001 – $0.05) | `ELEVENLABS_API_KEY` |

An image call is priced by its `size`. A size the route's tiers list is priced at its tier:
`1024x1024` is $0.21 – $0.29 for a generate on `gpt-image-2.5-sunburst@openai`, and
$0.21 – $0.43 for an edit. `auto`, no size, or a size no tier lists is priced at the whole
range above ([providers.md §10](../../spec/providers.md#10-the-built-in-route-table)).

A project picks one with `routes:` in `fx.yaml` (or a step's `route:`). To change a price or add
a route, list a table under `route_tables:`: its entry for the same capability and route replaces
the built-in one. A route on a provider FX has no adapter for still plans and prices, but a live
call on it is refused (`no adapter serves …`). `grida-fx nodes <type>` lists the routes that
serve a type; `grida-fx doctor` says which of them a live run could serve.

`<target>` is one of:
- a workflow file (`workflows/gallery.yaml`);
- a workflow id (`concept-gallery`), looked for among the `.yaml` files at the project root, then
  in the folders `fx.yaml` lists under `workflows:`, `workflows/` by default
  ([Getting started](01-getting-started.md#a-project));
- a Python builder (`level_art.py:build`), whose arguments are passed with `--arg name=value`.

## Keys and live runs

**Keys.** A live run (`--live`) reads each provider's key under its usual name:
`OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `FAL_KEY`, `TRIPO_API_KEY` and `ELEVENLABS_API_KEY`
(`OPENAI_BASE_URL`, `OPENROUTER_BASE_URL`, `FAL_BASE_URL` and `ELEVENLABS_BASE_URL` point a
provider at another address, such as a proxy). A key that is missing refuses only the calls that
need it, before anything is sent and for nothing: the step fails, refused with `OPENAI_API_KEY is
not set`. Without `--live` no key is used.

**Proxies.** A live run sends `https` requests through the proxy your environment names
(`HTTPS_PROXY` or `ALL_PROXY`, minus the hosts in `NO_PROXY`), tunnelled, so the proxy never reads
a key. Requests to `localhost` or a loopback address, the only hosts a plain `http` base URL may
name, are never proxied: they go straight there, and `HTTP_PROXY` is not used
([providers.md §2](../../spec/providers.md#2-the-transport)).

**The `.env` file.** A variable set (and not blank) in the environment wins. Otherwise FX reads it
from the `.env` file of the project you run from, if there is one, and only those nine names: a
line naming anything else is skipped without being read. A line is `NAME=value` (an `export`
before it is fine), with the value unquoted, in `'single quotes'` taken as written, or in `"double
quotes"` with JSON escapes; blank lines and `#` comments are skipped. A file FX cannot use stops a
live run before anything is planned (exit 2), with a message that names the variable and the line
and never the value: a symlink or anything but a plain file, text that is not UTF-8, a name given
twice, an empty value, a quote out of place, or a control character in the value. Keys are never
printed, logged or recorded. `GRIDA_FX_DISABLE_DOTENV=1` turns the file off, so only the
environment counts (credential-free checks set it).

**Trying `--live` without the network.** With `GRIDA_FX_NETWORK=off`, a live run builds its
adapters as usual, but nothing leaves the machine: every provider request is refused before it is
sent, and the step fails with a message that names `GRIDA_FX_NETWORK`, for nothing. Tests of live
paths use it.

**Checking keys and routes.** `grida-fx doctor` prints, after the engine and Python lines, where
each key comes from and which routes a live run could serve, for the project you run from (the
built-in table and its `route_tables`); with a target, the tools and node types the workflow needs
follow:

```
engine    grida-fx 0.1.0
python    .venv/bin/python (grida 0.1.0)
key       OPENAI_API_KEY present (.env)
key       OPENROUTER_API_KEY missing
key       FAL_KEY present (environment)
key       TRIPO_API_KEY missing
key       ELEVENLABS_API_KEY missing
route     image.generate gpt-image-2.5-sunburst@openai servable
route     image.generate img-a@acme no adapter
route     mesh.rig v1.0-20240301@tripo no key (TRIPO_API_KEY)
…
```

A route is `servable`, has `no key` (and names the variable to set), or has `no adapter` (no
provider of FX serves it). Missing keys don't change doctor's exit status; a `.env` or base URL
that a live run would refuse does: doctor prints `keys      <why>` and exits 1.

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
  files/             # every step's results: one folder per step path and take
```

- **Where:** `runs/<workflow id>/<date>-<n>/` in the project you run from, a new folder each
  time; `--run <folder>` names one instead, relative to where you are.
- **`files/` has one folder per take:** `files/<step>/` holds take 1, and any other take has a
  folder of its own, its take numbers after `#` (`files/draw#2/`, `files/entity__ada__.draw#1.3/`
  for a take inside a regenerating group). Every take a run made is there, and
  `inspect --verify` checks each one.
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

`run` prints the plan, then its selected folder and invocation before executing.
Its result summary follows execution, each label in a column of ten:

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
  on <route>, take <n>`. A `submitting` job FX could not confirm ends with why, naming the
  provider's job id when it returned one. `--forget <key>` removes one, so the next run submits
  its call anew.

## Delivering outputs

From the [game-build](../../examples/game-build/) example's `kitewharf/assets/`:

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
  failed step doesn't: check `result.ok`, then `result.failures`. It holds, by instance id
  (`draw#1`), each step that failed or was skipped: its `path`, `message`, `code` (the error's
  name, such as `node_failure` or `capability_refused`; `None` for a timeout, an assertion that
  did not hold, or a skip), `facts` and `skipped`. `result.stopped` says why a run stopped before
  it was done, and is `None` otherwise.
- **Stand-ins:** `stand_in=answer` answers the run's paid calls with your function, offline and
  for nothing ([below](#stand-ins-testing-without-a-provider)); `result.stand_in` says whether a
  run had one.
- **Your Python runs the nodes when nothing else is chosen:** the SDK (and `python -m grida.fx`)
  tells the engine the Python it runs in, and node bodies, built-ins with Python bodies and a
  `--stand-in` file run in it when neither `GRIDA_FX_PYTHON` nor a project `.venv` chooses one.
  Set `GRIDA_FX_PYTHON` to choose another.

## Stand-ins (testing without a provider)

A **stand-in run** answers each paid call with a function you write instead of a provider:
offline, for nothing, with the answers your test chooses, as a mock model does in an AI SDK. The
rest is a real run: every step runs, your nodes and judges see the answers, and the run is
recorded. A call answered before is replayed from the stand-in's own cache (below); the function
is asked only the rest.

**Two ways to give one:**
- **From Python,** `grida.fx.run(..., stand_in=answer)` (or `run_async`) calls `answer` in your
  own process, so closures, counters and lists of scripted answers work, and each run may get
  another function. The SDK hands the engine a socket on its standard input (`--stand-in -`, for
  SDKs rather than for typing), so this needs macOS or Linux.
- **From the command,** `--stand-in <file>.py#<function>`, with the file relative to where you are;
  it may lie outside the project. FX loads it in a Python of its own: `GRIDA_FX_PYTHON` when it is
  set, else the `.venv` of the project you run from, else the SDK's Python, else `python3` on
  `PATH`. Unlike your nodes,
  it never uses the `.venv` of a workflow's own project when that is another one
  ([Getting started](01-getting-started.md#when-fx-needs-python)). The file's folder comes first
  on `sys.path`, as `python <file>` would have it. A file that is missing or does not load, or a
  function it lacks, is refused before the run starts (exit 2).

  ```bash
  grida-fx run icon --stand-in tests/stand_in.py#answer -- --name "copper lantern"
  ```

  ```
  run       runs/icon/2026-10-06-1
  stand-in  tests/stand_in.py#answer
  result    ok   spent $0.00
  ```

**Never with `--live` or `--yes-up-to`** (exit 2): a stand-in run spends nothing and runs every
phase. `--max-usd` is accepted and changes nothing, since nothing is reserved: the plan shows the
ceiling but never warns that the run stops before crossing it. `--deliver` works as in any run. `plan` and the other planning commands take no stand-in: planning never makes
a paid call.

**The function** takes one call, and may be `async`:

| `call.` | |
|---|---|
| `capability`, `route.id`, `key` | what is asked, of which route, and the call key |
| `take`, `takes` | the take being drawn (`2` for a second take), and the whole take list |
| `instance.id`, `.path`, `.step` | the instance making the call: its id (`entity['ada'].draw#2`), its step path with repeat keys (`entity['ada'].draw`) and its declared step (`entity.draw`) |
| `request` | the request, with each file in it, however deep (`references`, an agent's `messages[*].images`), as an `InputFile`: `.path`, `.read_bytes()`, `.facts` |
| `files` | every file of the request, as an `InputFile`, by digest |

What it returns or raises decides the call:

| `answer(call)` | The call |
|---|---|
| returns an `Answer` | is answered: checked (below), stored and recorded, at `cost_usd: 0` |
| returns `DECLINE` | goes on as in a run without a stand-in, and fails `not_live`: `… is a paid call the stand-in declined`. Made again, it is asked again |
| raises `CallRefused("…")` | is refused (`capability_refused`), as when a provider refuses before sending |
| raises `CallFailed("…")` | fails (`call_failed`), as when every attempt failed |
| raises anything else, or returns `None` or another value | stops the run (below) |

`Answer(files={name: file}, data=None)` takes each file as bytes, an `Output(kind=…, data=…)`, an
`InputFile` (to answer with a file the call was given) or a `Path`. Bytes take the kind the
capability gives that name: `image` is `image/png`, `audio` is `audio/mpeg`, `video` is
`video/mp4`. `data` is what the capability returns
([capabilities.md](../../spec/capabilities.md)), with shortcuts:
- `Answer.json(value)`: a `structured.generate` answer;
- `Answer.turn(text, tool_calls)`: one agent turn, each tool call a mapping with `name` and
  `arguments` (and an `id`, made up when it has none);
- `Answer.submit(**arguments)`: an agent turn that submits its answer.

`video.generate` and `mesh.generate` answer `data=None`: FX reads the clip's size, frame rate and
duration, or the model's kind, from the file itself.

**Faults stop the run.** The function is part of your test, so it never fails quietly. Any other
exception (an `AssertionError` among them), an answer FX cannot read, or a stand-in host that
exits stops the whole run, rather than failing one call the workflow would then route around:
`stopped   the stand-in failed: AssertionError: …`, exit 1. The function is not called again
after its first fault, and from Python, `run()` raises that exception once the engine has ended,
noting the run folder. A step's `timeout:` and a stopped run abandon a call the function is still
answering, and its late answer is discarded.

**Answers are checked** as a provider's are, in this order:
1. **The request,** before the function is called: a request that does not fit its capability
   (`image.generate needs prompt`) is refused, `capability_refused`, and the stand-in is not asked.
2. **The answer's shape:** the files the capability returns, by name and kind, and its `data`
   (`the answer holds no image`, `image.generate returns no file named mask`,
   `structured.generate returns its data as {"json": <value>}`).
3. **The route's check:** the check of FX's adapter for the route's provider when FX has one (built
   with no key, so it sends nothing), else the check the capability makes on every route. For a
   picture: a PNG of the exact `size`, with an alpha channel when `transparent` was asked and no
   transparent pixel when `opaque` was (`the image is 32x32, not 64x64`). For
   `structured.generate`, its schema; for audio, an MP3.
4. **The data:** its round trip through canonical JSON, and an agent turn's shape.

The first refusal fails the call: `call_failed`, `<capability> on <route> failed: <reason>`. A call
that was refused or failed, by the stand-in or a check, is not asked again by the same command, and
nothing of it is recorded, so the next command asks again, whether it resumes the folder or starts
a new one. A picture of the wrong size is therefore a
failed call, not a picture your judge sees: to test a judge, answer a picture that passes these
checks and that the judge rejects.

**Not checked yet:** a route's own refusals of values, which its adapter makes just before sending:
a blank prompt, a size outside the route's range, an unknown `background`, too many pictures. A
stand-in may answer a request that a live run would refuse there.

**Kept apart from your cache:**
- **The stand-in store.** A stand-in run keeps its input files, calls, results and outputs in
  `stand-in/` inside the project's cache (`.fx/cache/stand-in/`), and reads nothing of the cache
  itself. What your stand-in is asked never depends on what paid runs left in the cache, and a
  stand-in's answer never reaches a run without one.
- **Answers are kept** there as a provider's are in the cache: running again, into the same folder
  or a new one, asks only what has not been answered, whatever the function. A test can check
  that a second run calls nothing, or that a resumed run asks only for what is left. Each test
  should therefore start from a scratch copy of the project, as below, or delete `stand-in/` first.
- **One mode per folder.** The run folder is placed and named as any other. Its `plan.json` has
  `"stand_in": true` and its `run_started` events `stand_in: true`, as has each `call` event the
  stand-in answered; the plan digest is the same as without a stand-in. Resuming the folder without
  `--stand-in`, or a plain run's folder with one, is refused (exit 1): `refused: runs/one holds a
  stand-in run; resume it with --stand-in, or choose a new folder`.
- **The takes file is not touched.** `reroll` and `pick` refuse a stand-in run (exit 1), and a
  stand-in run writes nothing outside its folder and the stand-in store, apart from what
  `--deliver` copies out. A test of picks writes the takes file itself.
- **Read like any run.** `inspect` adds `stand-in` to its header, `inspect --json` has
  `run.stand_in`, and `project` shows the `run_started` event as it is.

**A test.** With pytest, for the icon workflow of
[Getting started](01-getting-started.md#your-first-workflow): the first drawing is opaque
everywhere, which its `image.check_alpha` judge rejects, so a second take is drawn.

```python
# tests/test_icon.py
import io
import shutil
from collections import Counter
from pathlib import Path

import pytest
from PIL import Image

import grida.fx as fx
from grida.fx import DECLINE, Answer, CallRefused

PROJECT = Path(__file__).resolve().parents[1]
LANTERN = {"name": "copper lantern"}


def png(width, height, background_alpha):
    """An orange square on a background of the given alpha."""
    picture = Image.new("RGBA", (width, height), (32, 24, 16, background_alpha))
    picture.paste((200, 120, 40, 255), (width // 4, height // 4, width * 3 // 4, height * 3 // 4))
    out = io.BytesIO()
    picture.save(out, "PNG")
    return out.getvalue()


@pytest.fixture
def project(tmp_path):
    """A scratch copy of the project, with a cache of its own and no takes: nothing starts
    answered or picked."""
    root = tmp_path / "project"
    skip = shutil.ignore_patterns(".fx", ".venv", ".env", "runs", "tests", "*.takes.yaml")
    shutil.copytree(PROJECT, root, ignore=skip)
    return root


def test_a_rejected_icon_is_drawn_again(project):
    asked = Counter()

    def answer(call):
        assert "copper lantern" in call.request["prompt"]
        asked[call.take] += 1
        width, height = map(int, call.request["size"].split("x"))
        # Take 1 is opaque everywhere, which the judge rejects; take 2 has a clear background.
        return Answer(files={"image": png(width, height, 255 if call.take == 1 else 0)})

    result = fx.run("icon", inputs=LANTERN, cwd=project, stand_in=answer)
    assert result.ok and result.stand_in and result.cost == 0
    assert asked == {1: 1, 2: 1}
    assert result.steps["clean"].facts["verdict"] == "accept"

    again = fx.run("icon", inputs=LANTERN, cwd=project, stand_in=answer)
    assert again.ok and asked == {1: 1, 2: 1}  # answered from the stand-in store


def test_a_refused_drawing_fails_its_step(project):
    def answer(call):
        raise CallRefused("no lanterns today")

    result = fx.run("icon", inputs=LANTERN, cwd=project, stand_in=answer)
    assert not result.ok
    assert result.failures["draw#1"].code == "capability_refused"
    assert "no lanterns today" in result.failures["draw#1"].message
    assert result.failures["clean#1"].skipped


def test_a_declined_drawing_is_not_live(project):
    result = fx.run("icon", inputs=LANTERN, cwd=project, stand_in=lambda call: DECLINE)
    assert result.failures["draw#1"].code == "not_live"
```

- **The scratch copy** gives each test an empty stand-in store, and no takes: a `reroll` or `pick`
  you made while following [Getting started](01-getting-started.md) would change which take is
  drawn. A test of picks writes the takes file itself.
- **The test's own Python** runs the judge, a built-in whose body is Python: the scratch copy has
  no `.venv`, so `grida.fx.run` falls back to the Python it runs in, which has `grida` and Pillow.
- **A failed `assert` in the function** fails the test with its own message: the run stops, and
  `run()` raises the `AssertionError`.
- **One call at a time:** FX calls the function for one call after another, in the order the
  engine asks, never for two at once, so plain counters and `list.pop(0)` scripts need no lock.
  Steps that run at the same time may still ask in either order.

## Agents

A coding agent drives FX through the same command. It can write a workflow file and `plan` it:
a plan is free, so an agent can iterate on a workflow before spending anything. `grida-fx schema`
gives it the JSON Schema of a workflow's inputs, and `plan --json`, `inspect --json`, `expand` and
`price` give it JSON to read. A run that may spend still needs `--live` and a ceiling.
