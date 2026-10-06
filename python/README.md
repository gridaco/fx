# grida

The Python SDK for [Grida FX](https://github.com/gridaco/fx). Import it as `grida.fx`.

It does three things: it authors node types and workflows (`node`, `Workflow`), it hosts node
bodies when the engine runs them (`python -m grida.fx.host`, started by the engine), and it drives
the `grida-fx` engine from Python. The engine is the only place with engine logic: planning,
identities, the cache and budgets all happen in `grida-fx`, never in this package.

## Install

```sh
pip install --pre grida
```

The preview is a pre-release (`0.1.0aN`), so it needs `--pre` (`uv pip install --prerelease=allow
grida`). Each wheel carries the engine for its platform: macOS on Apple silicon (11 or later) and
on Intel (10.12 or later), and Linux with glibc 2.28 or later on x86_64 and aarch64. Windows is not
in the preview: the engine's runner uses Unix process groups and signals. There is no source
distribution, and the wheels install no console command: the engine runs as `python -m grida.fx`.

From a checkout of the repository, `python3 tools/build_engine.py` builds the engine and puts it
where a wheel carries it: `python/src/grida/fx/_bin/grida-fx` in the checkout (ignored by git). An
editable install of the checkout's `python/` folder (`uv sync --project python`, or a path
dependency in another project) then finds it. Run it again after every pull.

## The engine from the command line

```sh
python -m grida.fx plan workflows/gallery.yaml --poster inputs/poster.png
python -m grida.fx run workflows/gallery.yaml --poster inputs/poster.png --live --max-usd 10
```

`python -m grida.fx <verb> …` takes the same arguments as `grida-fx` and exits with its status
(on POSIX it becomes the `grida-fx` process), so a Python installation needs no Node. The binary
is the first of:

1. `GRIDA_FX_BIN`, when set (a path; relative to the current directory);
2. the one inside the package (`grida/fx/_bin/grida-fx`): shipped in the wheel, or built into a
   checkout by `tools/build_engine.py`;
3. `grida-fx` on `PATH`.

With none of them, the command prints `grida-fx: …` naming `GRIDA_FX_BIN` and exits 2.

## Planning and running from Python

```python
from grida.fx import plan, run

planned = plan("workflows/gallery.yaml", inputs={"poster": "inputs/poster.png"}, max_usd=10)
print(planned.ok, planned.problems, planned.estimate())  # estimate: (low, high) in US dollars
for phase in planned.phases():
    print(phase["phase"], phase["steps"], phase["high_usd"])

result = run(planned, live=True)  # runs the plan with the target and options it was planned with
print(result.ok, result.cost, result.incomplete, result.failed)
for key, image in result.outputs["images"].items():  # a keyed collection
    verdict = result.steps[f"entity['{key}'].review"].facts["verdict"]
    print(key, image.path, verdict)
missing = result.deliver({"images": "out/{key}.png"})
```

- **Targets.** A workflow file, a workflow id, a builder `file.py:function` (its arguments as
  `arguments={"level": "docks"}`, the command line's `--arg`), or a `Plan`. A `Workflow` object
  is refused with `TypeError`: name the builder that returns it.
- **Options.** `inputs` (a mapping, written to a temporary inputs file in `cwd`, so relative
  paths in it mean what they mean on the command line), `input_files` (`--inputs`), `routes`
  (`--routes`), `max_usd` (`--max-usd`); `run` adds `live`, `yes_up_to` and `run_dir`. `cwd` is
  the engine's working directory (default: the current one); every relative path is relative to
  it. A `Plan` takes no planning options when it runs: plan again to change them.
- **`plan`** asks the engine for the plan (`grida-fx plan --json`) and its price (`grida-fx
  price`). `Plan.document` is the fx-graph-v1 document; `ok`, `problems` (`where`, `message`),
  `estimate()` and `phases()` read it. Planning never spends.
- **`run`** plans first, and raises `PlanRefused` (`the plan is refused:` and one line per
  problem) before any run folder exists. A failed step does not raise: check `result.ok` and
  `result.failed`. The result is read from the run folder's `events.jsonl`: `ok`, `incomplete`,
  `cost` (the folder's charge over every invocation, in US dollars), `run_dir`, `failed`,
  `failures`, `stopped`, `stand_in`, `outputs` and `steps`.
- **Failures.** `result.failures` holds a `Failure` for each failed instance id: its `path`,
  `message`, `code` (the protocol's name of the error, such as `node_failure` or `call_failed`;
  `None` for a timeout, an assertion or a skip), the node `facts` it reported, and `skipped` (it
  never ran, because something it reads failed). `result.stopped` says why a run stopped early.
- **Files.** An output file has `path` (its copy in the planning project's store,
  `<cache>/files/…`: the project above the workflow file for a `.yaml` target, else above `cwd`;
  read it, never write it), `digest`, `kind`, `name`, `size`, `key`, `read_bytes()` and
  `copy_to()`.
  A keyed collection is a `dict`, a list a `list`, an absent output `None`.
- **Delivering.** `result.deliver({output: path})` copies output files out of the run; `{key}`
  in the path names each element of a collection or list (keys are made safe as run folders make
  them). A file that already holds the same bytes is left alone. It returns the outputs that hold
  no file.
- **Errors.** The engine's own errors (exit status 2: a workflow it cannot read, a bad option)
  raise `FxError` with its message; so does a run it refuses to start (`refused: …`).
- **Async.** `plan_async` and `run_async` do the same inside an event loop; `plan` and `run`
  cannot be called inside a running one. Cancelling `run_async` stops the run as Ctrl-C would.

## Stand-ins: running without a provider

A test runs a workflow's paid calls offline by answering them itself:

```python
from collections import Counter
from pathlib import Path

from grida.fx import DECLINE, Answer, CallRefused, run

asked = Counter()


def answer(call):  # a plain function, or an async def
    asked[call.capability] += 1
    if call.capability == "image.generate":
        return Answer(files={"image": Path(f"tests/pictures/{call.request['size']}.png")})
    if call.capability == "structured.generate":
        return Answer.json({"title": "The lighthouse"})
    if call.capability == "agent.turn":
        return Answer.submit(caption="a lighthouse at dusk")
    if call.capability == "image.edit":
        raise CallRefused("no edits in this test")
    return DECLINE


result = run("workflows/gallery.yaml", run_dir="runs/test", stand_in=answer)
assert asked["image.generate"] == 2
assert result.failures["touch_up#1"].code == "capability_refused"
```

- **The call.** `answer` gets a `StandInCall` for each paid call the cache cannot answer:
  `capability`, `route.id` and `route.fingerprint`, `key`, `takes` and `take`, `instance.id`,
  `.path` and `.step`, `request` (every file in it an `InputFile`: `digest`, `path`,
  `read_bytes()`, `facts`), `files` (the same files by digest) and `params` (the call as the
  engine sent it, files as `{"file": digest}`).
- **The answer.** `Answer(files={name: …}, data=…)`: a file is `bytes` (it takes the kind the
  capability names), an `Output` of bytes or an `InputFile` (its own kind), or a path (the kind of
  its suffix). `Answer.json(value)`, `Answer.turn(text, tool_calls)` and
  `Answer.submit(**arguments)` build the data of `structured.generate` and `agent.turn`. Return
  `DECLINE` to leave a call unanswered (it fails `not_live`); raise `CallRefused` or `CallFailed`
  to refuse or fail it. The engine checks every answer as it checks a provider's: an image of the
  wrong size fails the call.
- **One at a time.** The function runs in this process, so counters and scripted answers work.
  Calls are answered one at a time, in the order they come.
- **Faults.** Anything else it raises, or a return of `None`, stops the run; `run` then raises
  that exception, noting the run folder.
- **Nothing is spent, and the real store is left alone.** A stand-in run is never `live` and
  takes no `yes_up_to`; `run` does not plan it first. It keeps everything in the store's
  `stand-in/` folder, where its result's files are read, so a later stand-in run, or a resumed
  one, replays its answers; delete that folder to forget them. A folder resumes only in the mode
  it was started in, and `reroll` and `pick` refuse it.
- From the command line: `grida-fx run … --stand-in tests/stand_in.py#answer`.

## Standard bodies

`grida.fx.std` holds the Python bodies of FX's free standard node types. The engine declares
them, and runs them in this package's node host when a workflow uses one (`fx/image.resize@1`,
…); nobody imports them to use them.

| Type | What it does |
|---|---|
| `fx/image.mirror_repeat@1` | mirrors a picture onto itself on `x`, `y` or both, at most 16384 px a side |
| `fx/image.check_alpha@1` | judge: `transparent` wants a fully transparent pixel, anything else every pixel opaque |
| `fx/image.check_size@1` | judge: the picture's `width` and `height` against the ones asked for |
| `fx/image.resize@1` | `longest_side`, or `width` and `height`; Lanczos |
| `fx/image.crop@1` | a `box` in fractions, or the first box mark of a `region`, grown by `padding` |
| `fx/image.pad@1` | centres a picture on a transparent canvas |
| `fx/json.merge@1` | a shallow merge of JSON objects, a later key winning |
| `fx/files.copy@1` | the same bytes and kind |
| `fx/package@1` | lays files out by destination path (`{key}` takes a keyed collection) with a manifest |

The bodies are ported byte for byte from FX's predecessor, and every picture they write comes
from Pillow's own PNG encoder, so `pillow` is pinned to one exact version: another build may write
other bytes, which would change every identity and request downstream of them.
`grida.fx.std.body_of("fx/<name>@1")` returns a body; `grida.fx.std.pictures` holds the Pillow
reads and writes behind `ctx.read.image` and `ctx.out.png`.
