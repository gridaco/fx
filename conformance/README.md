# FX conformance

These cases hold an implementation of FX to the [spec](../spec/) through its command line only.
The runner never imports an engine: it runs a command, checks how it exits, and compares what it
prints or writes with what a person has reviewed. Any implementation (the Rust `grida-fx`, a
future one) can be held to the same cases.

The suite began in stage-gen, where FX comes from. Its first fourteen cases moved here with their
inputs renamed to FX's names. Their recorded outputs did not move, because every digest changes
under [identity.md](../spec/identity.md); see [Recording expected output](#recording-expected-output).

## Running

```sh
uv run --project python python conformance/run.py --command target/debug/grida-fx
uv run --project python python conformance/run.py --command target/debug/grida-fx linear numbers
```

- **The command** comes from `--command`, else the environment variable
  `GRIDA_FX_CONFORMANCE_COMMAND`, else `grida-fx` on `PATH`. It is split like a shell would split
  it, so it can carry arguments. A program path is made absolute from where you run the script.
  Every step runs in a temporary copy of its case's project, so a Cargo command must name the
  workspace's manifest by absolute path. Build first, because a step's timeout includes any build:

  ```sh
  cargo build --bin grida-fx
  uv run --project python python conformance/run.py \
    --command "cargo run -q --manifest-path '$PWD/Cargo.toml' --bin grida-fx --"
  ```

- **Python node bodies.** Most cases use project node types written in Python. The engine loads
  them through its Python host even to plan (`describe` reads the node types), so set
  `GRIDA_FX_PYTHON` to a Python that has the `grida` package installed, for example
  `GRIDA_FX_PYTHON=python/.venv/bin/python` after `uv sync --project python`. A value that is a
  path is made absolute from where you run the script, without following symbolic links (a
  virtual environment's `python` is one); a bare name such as `python3` is passed on unchanged.
  A stand-in case (one that runs `run --stand-in <file>.py#<function>`) needs it too, even
  without a `nodes/` folder: its stand-in is served by the same Python host. Only the cases
  whose `in/` has no `nodes/` folder and that name no stand-in run without it.
- **Options:** `--timeout SECONDS` limits each step (default 60). `--strict` fails a case that has
  no `expected/` yet. `--write` records output (below).
- **Results:** each case prints `PASS`, `FAIL` or `PENDING`, and the runner exits non-zero if any
  case fails.

## A case

A case is a folder:

| Path | What it is |
|---|---|
| `case.yaml` | the steps to run, in order |
| `in/` | an FX project: `fx.yaml`, a `routes.yaml` route table, workflow files, node modules, inputs |
| `expected/` | what the steps save, as reviewed by a person |
| `README.md` | optional: how a fixture was made, and anything a reviewer needs |

The runner copies `in/` to a fresh temporary folder (prefixed `fx-conformance-<case>-`) and runs
every step there, so a case may write anything into its project without touching the repository.

### case.yaml

`case.yaml` is written in the strict YAML subset of [yaml.md](../spec/yaml.md), like every FX
document. It starts with a comment that says what the case pins, so a reviewer can judge its
recorded output, and holds a mapping with these keys and no others:

| Key | Meaning |
|---|---|
| `steps` | required: a non-empty list of steps, run in order |
| `invalid_inputs` | optional: a list of project documents that are deliberately outside their schema, relative to `in/` (or to the case folder). The runner checks the paths exist; nothing else reads the list, so it documents intent. |

```yaml
# No fx.lock yet: `lock --check` fails and names the unlocked type; `lock` writes fx.lock,
# compared by meaning; then the workflow plans with three items.
steps:
- argv: [lock, --check]
  status: 1
  mentions: ["nodes/n.py#pinned@2"]
- argv: [lock]
- read: fx.lock
  save: fx.lock.json
  yaml: true
- files:
    inputs.yaml: "count: 3\n"
  argv: [expand, case, --routes, routes.yaml, --inputs, inputs.yaml]
  save: expand.json
  json: true
```

### Steps

Each step is a mapping with these keys and no others:

| Key | Value | Meaning |
|---|---|---|
| `argv` | list of strings | the command's arguments, after the command itself, run in the project folder |
| `status` | integer, default 0 | the exit status `argv` must end with |
| `mentions` | list of strings | strings that must each appear in the command's stdout or stderr |
| `stdin` | string | text fed to the command's standard input (default: none, and stdin is closed) |
| `files` | mapping of path to string | files written into the project as UTF-8, before `argv` runs |
| `read` | path | a project file to save instead of stdout: something the command wrote (`fx.lock`, a run's `outputs/…`) |
| `save` | file name | the file under `expected/` that this step's stdout (or its `read` file) is compared with; no `/`, and not starting with `.` |
| `json` | boolean | compare the saved text as JSON (below) |
| `jsonl` | boolean | compare the saved text as JSON Lines, such as a run's `events.jsonl` (below) |
| `yaml` | boolean | compare the saved text as YAML, by converting it to JSON (below) |
| `absent` | list of paths | project paths that must hold no file once the step is done: nothing is there, or only folders with no file anywhere below them (an empty folder is no record) |

- A step needs `argv`, `read`, `files` or `absent`. In one step, `files` are written first, then
  `argv` runs, then `read` is read, then every `absent` path is checked.
- `status`, `mentions` and `stdin` need `argv`. `read`, `json`, `jsonl` and `yaml` need
  `save`, and `save` needs `argv` or `read`. `json`, `jsonl` and `yaml` exclude each other.
- Paths in `files`, `read`, `absent` and `invalid_inputs` are POSIX and relative. They may not
  leave the project: no `..`, no absolute path, no backslash, no symbolic link out of it.
- An unknown key, or a value of the wrong type, fails the case before any step runs.

Two steps may save the same name. They must then produce the same bytes: that is how a case
says two different inputs mean the same thing (see `numbers` and `local-identity`).

### The command line the cases use

Every step runs in the project folder, and `<target>` is a workflow id (`case`) or a workflow
file (`workflows/case.yaml`).

| Command | What a case reads from it |
|---|---|
| `expand <target>`, `identity <target>`, `price <target>` | the expanded graph, each instance's identity, the price by phase, as JSON on stdout |
| `plan <target> [--json]` | whether the workflow plans (a workflow found by id, or not found); with `--json`, the expanded graph as `expand` prints it |
| `run <target> --run <folder> [--deliver <output>=<path>]… [--max-usd <n>]` | a local run in `<folder>`, or the continuation of the run that folder holds; its exit status and summary lines (`result    ok   spent $0.00`, `failed    <id>: <error>`, `stopped   <message>`, `refused: …`); the files it writes under the folder; with `--deliver`, the output files copied to `<path>` (`{key}` once per element) after the run. No case runs `--live`: one passes it only beside `--stand-in`, a usage error |
| `run <target> … --stand-in <file>.py#<function>` (or `--stand-in=…`) | a stand-in run: the paid calls the cache cannot answer go to `<function>` in the case's `<file>`, served by the Python host in the project folder, offline and for nothing; its summary has the line `stand-in  <source as typed>`, and it keeps its records in the stand-in store, `<cache>/stand-in/`. Its usage errors (with `--live` or `--yes-up-to`, a source of another form, a missing file, `--stand-in -` when standard input is not a socket) and a stand-in that cannot be loaded exit 2 |
| `project <folder>` | the run's record projected to its state, as JSON on stdout |
| `inspect <folder> --verify`, `inspect <folder> --json` | the run's summary (state, counts, spend, failures) and the check of every file it placed, on stdout; or the same as JSON |
| `jobs` | the project store's long jobs, one line each; nothing when it holds none |
| `reroll <folder> <step>` | the next take of a step path, written to the workflow's takes file; one line saying so and the next command, on stdout |
| `pick <folder> <step> <take>` | a take chosen for a step path, with its result's digest, written to the takes file; one line on stdout |
| `takes list <target>` | the takes file's entries, one line each (`<step>  take <n>[  <digest>]`), on stdout |
| `lock [--check] [--same <type>]` | `fx.lock`, written or checked |

Every case relies on these flags:

- **`--routes <file>`** adds a route table after the project's own. Giving any `--routes` leaves
  the built-in default table out ([identity.md](../spec/identity.md) §7). Every step that plans
  passes `--routes routes.yaml`, and no case's `fx.yaml` lists `route_tables`, so the catalog is
  exactly the case's `routes.yaml`: no case depends on the routes an engine ships.
- **`--inputs <file>`** reads the workflow's inputs from a file. Paths inside it are relative to
  it.
- **`--run <folder>`** names the run folder, relative to the current directory (the runner runs every step in the project), so that later steps can find
  it: `project runs/one`, `read: runs/one/outputs/said.txt`. A run writes each workflow output
  under the folder's `outputs/` as `<name><suffix>`, where the suffix is that of the output's
  kind, as [store.md](../spec/store.md), "Run folders", specifies: the text output `said` is
  `outputs/said.txt`. An output of several files, such as a list or a keyed collection, is a
  folder with one file per element: `outputs/parts/0.txt`, `outputs/entries/first.txt`.
- **`--deliver <output>=<path>`** copies an output after the run, to a path relative to the
  current directory; `{key}` in the path stands for each element's key (or position), made a
  safe path. An output the workflow does not declare is refused before anything runs.

A node body in a case may keep a counter in a file in the project folder, the node host's
working directory, so that a later step can read how often the body ran (`run-retry-engine`,
`run-timeout`). A case's stand-in does the same in its own working directory, which is the
engine's, the project folder: `count.txt` for how often it was asked and `asked.json` for what
(`stand-in-image/README.md`).

### Exit status

| Status | Meaning |
|---|---|
| 0 | success |
| 1 | the command read everything and refused or stopped: a planning problem the workflow author must fix (a problem in the plan, a stale `fx.lock`), a run refused before it started (a folder that holds another plan), or a run that is not ok (a failed or skipped step, such as a paid call without `--live`) |
| 2 | unreadable or invalid input: a document outside the strict YAML subset, an inputs file that does not fit the workflow's inputs, a command-line mistake (an output `--deliver` cannot name, a take out of bounds) |

A run a person interrupts exits 130; no case does.

### Comparing

- **Bytes.** Saved output is compared byte for byte, after the normalisation below. Standard
  error is never compared; a step pins what a message must say with `mentions`.
- **`json: true`.** The text is parsed as FX values ([identity.md](../spec/identity.md) §1): NaN,
  infinities and numbers that overflow are refused, and so is an integer literal that reading
  would round (one that is not the canonical form of the number it reads as:
  `9007199254740993`, but not `9007199254740992` or `10000000000000000`), a literal of more than
  21 digits by its length alone. Then:
  - `offset_ms`, `invocation_id`, `duration_ms` and `created_at` are dropped from run events (objects with an
    `event` member). The data a record carries is never touched, whatever its members are named:
    nothing under `with`, `inputs`, `outputs`, `request`, `data`, `facts`, `params`, `value` or
    `contract`.
    `created_at` is also omitted from the recorded run summary returned by `inspect`.
    Focused named-run tests check that creation time stays fixed across resumes.
  - An integral number below 1e21 is written as digits, as JCS writes it: `1`, `1.0` and `1e0`
    are one value, and `1e16` is `10000000000000000`.
  - The result is printed with sorted keys and an indent of 1.
- **`jsonl: true`.** The text is JSON Lines, such as a run's `events.jsonl`
  ([store.md](../spec/store.md) §8): each non-empty line is parsed and normalised as with
  `json: true`, and the lines are sorted (by their compact form with sorted keys) into one
  array, printed as JSON is. The events of instances that run at the same time may be written in
  any order, so a case compares which events a run wrote, not their order.
- **`yaml: true`.** The text is parsed as YAML and normalised as JSON is, so a written YAML
  document (`fx.lock`) is held to its meaning, not its layout.
- **Stale files.** A file under `expected/` that no step saves fails the case.

### Environment

Each step runs with a minimal environment, never the caller's:

- `PATH`, from the caller;
- `HOME`, a fresh empty folder for the case;
- `NO_COLOR=1`, `LANG=C.UTF-8` and `PYTHONDONTWRITEBYTECODE=1`;
- `GRIDA_FX_PYTHON`, passed through when set (a path made absolute, above);
- `RUSTUP_HOME` and `CARGO_HOME`, from the caller, else `~/.rustup` and `~/.cargo` when those
  folders exist, so that a `cargo run` command still finds its toolchain once `HOME` is replaced.

No provider key, `GRIDA_FX_TOOL_<NAME>` override or FX cache location can reach a case, and no
case spends: a case that needs a paid call's answer seeds the project's store with it
(`cache-replay`), or answers it with a stand-in (`stand-in-image`).

## Recording expected output

```sh
uv run --project python python conformance/run.py --command target/debug/grida-fx --write numbers
```

`--write` runs the cases and records whatever they save into `expected/`, removing stale files.
It never judges: the output is only as right as the implementation that printed it. A person
reviews every recorded file before it is committed, against the spec and the case's comments,
and checks what a case promises across files (for example, that `lock-drift` ends with the same
`fx.lock` that `lock-write` records). Never edit `expected/` by hand.

A case with no `expected/` folder yet is `PENDING`: its steps still run, and their exit statuses
and `mentions` are still checked, but nothing is compared and it does not fail the run unless
`--strict` is given. A case that saves nothing needs no `expected/` and passes on its statuses
alone.

## The cases

| Case | What it pins |
|---|---|
| `at-plan` | an `at: plan` step runs while planning, and a `for_each` over its output expands to keyed instances; its price |
| `at-plan-run` | an `at: plan` step in a run: its keyed `for_each` of local steps runs, collected into one output; the `at: plan` step is not run again |
| `cache-replay` | a paid call answered offline from a seeded call record, without `--live`; a second run answered from the result cache |
| `cache-replay-miss` | the same project without a store: a paid call without `--live` is refused (exit 1), naming the capability, the route and `--live` |
| `conditions-select` | `if:` on an input and on a judge's fact, `on_reject: continue`, and `fx/select@1` with `first_of` |
| `export-identity` | two unversioned exports of one module given the same value: one source digest, two type and step identities; a second run answers both from the cache, and each keeps its own output |
| `facts` | `facts()` of input files (an image's size, alpha and opacity; a text file's size and kind) in `if:` and in a prompt; a fact of a file not made yet leaves an identity null |
| `facts-media` | file facts of a WAV and an MP4 input (`duration`, `fps`, `frames`, `width`) in a free built-in step's `with:` and in `if:`, and the identities they give |
| `group-regenerate` | a group with `regenerate: {max, until}`: nested take ids (`build.draw#2.1`) and `maybe` states |
| `identity-once` | two steps with one step identity in one run: the second waits for the first, then the result cache answers it (one `miss`, one `hit`) |
| `judge-regenerate` | a judge's `on_reject: {regenerate: {max: 3, then: fail}}`, priced as up to three calls |
| `linear` | a file input's digest in a local step's identity, a pending value that leaves an identity null, and workflow outputs |
| `local-identity` | an unversioned project type's `<path>#<attr>@source:<digest>` identity over its module, its project imports and its declared resources; a versioned type's `<path>#<attr>@<version>` never moves |
| `lock-drift` | an `fx.lock` entry that no longer matches its source refuses the plan, `lock --check` and `lock`; `lock --same` confirms it |
| `lock-write` | `lock --check` without a lock fails; `lock` writes `fx.lock` for the versioned types only |
| `matrix` | a 2x2 `matrix` with dotted keys, collected into a `text{}` port |
| `numbers` | `1` and `1.0` give one identity; numbers render in text in their JCS form; a run's output at `outputs/<name><suffix>`; an integer literal that reading would round is refused (exit 2) |
| `phase` | a `for_each` over a step's output with `max:` makes a pending second phase, priced at its maximum |
| `refusals` | six planning problems in one plan (an assertion, a feature under its old name, a missing feature, `independent_of`, an unknown route, an unbounded repeat), exit 1 |
| `repeat-keyed` | a keyed `for_each` over an input list, collected into one step |
| `resource-missing` | a declared resource that is not in the project is a planning problem (exit 1), never a crash |
| `run-cache-hit` | the same plan run into a second folder: every step is a result-cache hit, with the same output bytes |
| `run-deliver` | `--deliver` of a one-file and a keyed output (keys made safe paths), left alone when unchanged; an undeclared output refused before the run (exit 2) |
| `run-failures` | `ctx.fail` keeps its node facts (in the run's events), an exception is a `node_error`, a reader of a failed step is skipped as blocked; the run goes on, lists its failures in the expansion's order and exits 1 |
| `run-local` | a local run of every output shape: text with node facts, JSON written by the engine, a list port, a keyed port, marks written as an `annotations` output, a judge's verdict; outputs and step files at their run-folder names, verified by `inspect --verify` |
| `run-project` | a local run, then `project` of its record: cache misses, output digests, nothing charged |
| `run-resume` | a second run into the same folder resumes it (`resumed: true`, nothing runs again); other inputs into that folder are refused (exit 1) |
| `run-retry-engine` | `retry="engine"`: a body that raises twice runs a third time and succeeds, and no more; a `node_retry` event for each failed attempt |
| `run-takes` | `reroll`, `pick` and `takes list` between runs: the takes file a run's plan names, the take each run draws, a picked result's digest checked by the next run |
| `run-timeout` | a step past its `timeout:` fails with `ran past 1 seconds` and is not run again, even under `retry="engine"` (exit 1) |
| `stand-in-agent` | a stand-in's `agent.turn` answers drive the engine's agent loop: a tool call, then a `submit`, and the transcript in the output; a turn whose data is not `{text, tool_calls}` and a structured answer its schema refuses are `call_failed` |
| `stand-in-errors` | what a stand-in's answer can end in: refused, failed, declined (`not_live`), a picture of the wrong size, an answer of the wrong shape (a file's kind, an extra file, no file, data), and a body's call the request check refuses before the stand-in is asked; the `code` of each `node_failed`; nothing of them recorded, so a new run asks again |
| `stand-in-flags` | how `--stand-in` is given: with `--live` or `--yes-up-to`, a malformed source, a missing file, a missing function, a file that fails to import, and `-` without a socket are usage errors (exit 2) that ask nothing; a stand-in that raises stops the run (exit 1) |
| `stand-in-image` | a stand-in run: the call answered offline for $0.00 under a ceiling below its hold, marked `stand_in` in `plan.json`, `run_started` and the `call` event, kept in the stand-in store (resumed, rerun and call-cache hits ask nothing; the project's own store gets nothing and still misses); the plan digest unchanged; each folder refuses the other mode; `inspect` names it; `pick` and `reroll` refuse it |
| `stand-in-job` | long jobs (`video.generate`, `mesh.generate`) answered at once with no job record: files sent without a kind take the capability's (the model's from its signature), and the engine writes `data` from the files, so the clip's file facts and the model's kind become node facts |
| `takes-pick` | `takes: 3` with `pick: first_accepted`, priced as three calls |
| `template-prompt` | a prompt file rendered with `vars` and inputs, `max:` from an input, and steps nested in a repeat |
| `tiered-price` | a route priced per second by a request setting: each take at its tier, and a take with no setting or a value no tier names at the route's whole range |
| `workflow-step` | a workflow used as a repeated step, with its own input defaults |
| `workflows-setting` | `workflows:` in `fx.yaml` replaces the folders searched for a workflow id (overlapping entries count a file once, a missing folder adds nothing, the root is always searched); a workflow in a nested project keeps its home's route defaults under the planning project's, and its run, store and takes file are the planning project's; an entry that is the project or holds it is refused (exit 2) |
| `yaml-strict` | the strict YAML subset in inputs, workflow, route, takes and project files: `on` as a key, quoted and plain strings (`y`, `0bad`) accepted; ambiguous scalars (a YAML 1.1 boolean in any letter case, a leading zero, a date, `1:30` and `16:9`, `.inf`), duplicate keys, anchors and tags refused (exit 2) |

## Writing a case

- Keep it small and offline: local node types, the shared `routes.yaml` prices, and made-up names
  (`acme`, `img-a`). Nothing in a case may hold a key, a private path or real media.
- Media is synthesized, never generated art; the case's `README.md` says how each file was made.
- Quote digests and other ambiguous scalars in YAML fixtures (`"0123…"`), as
  [yaml.md](../spec/yaml.md) requires.
- Digests are bare: 64 lowercase hexadecimal characters, never prefixed with `sha256:`. That
  holds for `fx.lock` entries and a takes file's `result` alike.
- Say what the case pins in a comment at the top of `case.yaml`, so a reviewer can judge its
  recorded output.
