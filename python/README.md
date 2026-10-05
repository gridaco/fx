# grida

The Python SDK for [Grida FX](https://github.com/gridaco/fx). Import it as `grida.fx`.

It does three things: it authors node types and workflows (`node`, `Workflow`), it hosts node
bodies when the engine runs them (`python -m grida.fx.host`, started by the engine), and it drives
the `grida-fx` engine from Python. The engine is the only place with engine logic: planning,
identities, the cache and budgets all happen in `grida-fx`, never in this package.

## The engine from the command line

```sh
python -m grida.fx plan workflows/gallery.yaml --poster inputs/poster.png
python -m grida.fx run workflows/gallery.yaml --poster inputs/poster.png --live --max-usd 10
```

`python -m grida.fx <verb> …` takes the same arguments as `grida-fx` and exits with its status
(on POSIX it becomes the `grida-fx` process), so a Python installation needs no Node. The binary
is the first of:

1. `GRIDA_FX_BIN`, when set (a path; relative to the current directory);
2. the one shipped inside the `grida` wheel (`grida/fx/bin/grida-fx`);
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
  `outputs` and `steps`.
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
