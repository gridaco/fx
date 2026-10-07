# Getting started

FX builds generated assets the way a build system builds code:
- You describe the steps in a **workflow file**.
- FX **plans** the run offline, with a cost estimate.
- It **runs** only what changed.
- Every result is kept with a record of how it was made.

## Install

```bash
npm install -g @grida/fx   # the grida-fx command (or without installing: npx @grida/fx)
pip install grida         # the Python SDK, for nodes and builders you write in Python
grida-fx doctor           # checks the keys and tools your workflows need
```

Both packages are published as stable releases starting at `0.1.0` and carry the same engine.
No repository clone or Rust toolchain is needed. For source development, see
[CONTRIBUTING.md](../../CONTRIBUTING.md#build-from-source).
FX runs on macOS, and on Linux with glibc 2.28 or later, on x64 and arm64. Native Windows is not
supported, because the engine's runner uses Unix process groups and signals: use WSL 2
([the platforms](../../README.md#install)). Python users who don't want Node run
it as `python -m grida.fx <verb>` (`python -m grida.fx doctor`), or through the Python API
([Running](05-running.md#from-python)). This guide writes `grida-fx`.

FX reads each provider's key under its usual name: `OPENAI_API_KEY`, `OPENROUTER_API_KEY`,
`FAL_KEY`, `TRIPO_API_KEY`, `ELEVENLABS_API_KEY`. Set them in the environment, or put them in a
`.env` file in your project (one `NAME=value` per line, and keep the file out of version
control); the environment wins. Keys never go in `fx.yaml` or a workflow, and FX never prints
them: only a `--live` run uses them, and `grida-fx doctor` says which ones it found, where, and
which routes they make usable ([Running](05-running.md#keys-and-live-runs)).

### When FX needs Python

The engine itself runs the paid built-in types (`image.generate`, `structured.generate`, …),
`fx/select@1` and every file fact. Everything else runs on Python:
- your own Python nodes and builders, which import `from grida.fx import node, Ctx`;
- the built-in local types too: `image.resize`, `image.crop`, `image.pad`,
  `image.mirror_repeat`, `image.check_alpha`, `image.check_size`, `json.merge`, `files.copy` and
  `package`.

A project that uses any of them needs a Python with `grida` and Pillow installed
(`pip install grida`, which brings Pillow). FX uses `GRIDA_FX_PYTHON` when it is set, else
the `.venv` of the workflow's project, else the `.venv` of the project you run from, else the Python
that runs the SDK when you run through it ([Running](05-running.md#from-python)), else `python3`
on `PATH`. Only a project whose steps are all paid built-ins and `select` needs just the command.
The first workflow below uses `image.check_alpha`, so it needs Python.

## A project

A project is a folder with an `fx.yaml` in it:

```
my-assets/
  fx.yaml             # project settings
  workflows/          # your workflow files
  nodes/              # your own node types (optional)
  prompts/            # prompt templates (optional)
  inputs/             # files you feed in
  runs/               # every run, one folder each   (created by grida-fx)
  .fx/cache/          # content-addressed results    (created by grida-fx)
```

```yaml
# fx.yaml
fx: project/v1
budget:
  max_usd: 10                 # the ceiling for any one run; --max-usd overrides it
routes:                       # which model serves each capability by default
  image.generate: gpt-image-2.5-sunburst@openai
# workflows: [workflows, ../tools/art/workflows]   # where a workflow id is looked for
```

`grida-fx nodes image.generate` lists the routes that can serve a capability: FX's built-in
route table, plus the tables your project lists under `route_tables:` (read after it, so their
entries win).

**Where workflows are found.** A workflow named by its id (`grida-fx run icon`) is looked for among
the `.yaml` and `.yml` files at the project root, then in `workflows/` and every folder below it. A
monorepo that keeps its workflows elsewhere lists their folders under `workflows:`, searched in the
order given; each is relative to the project or absolute, and may lie outside the project. The list
replaces the default, so name `workflows` too if you keep workflows there as well. The project root
itself, or a folder above it, is refused, since it would search your runs and cache. A workflow in a
folder with an `fx.yaml` of its own keeps that project as its home: its `./` paths, node modules and
route defaults are that project's, with your `routes:` winning. The runs, the cache, the budget, the
route tables and the takes file (`<id>.takes.yaml` at your root) stay those of the project you run
from.

## Your first workflow

Generate one item icon on a transparent background. If the background isn't actually clean,
draw it again.

```yaml
# workflows/icon.yaml
fx: workflow/v1
id: icon
title: One item icon

inputs:
  name: { type: string, description: "What the item is" }

steps:
  draw:
    uses: fx/image.generate@1
    with:
      prompt: "A single ${{ inputs.name }} game icon, centered, no text."
      background: transparent
      size: 1024x1024

  clean:
    uses: fx/image.check_alpha@1           # a built-in, deterministic judge
    judges: draw
    with: { image: "${{ steps.draw.outputs.image }}", expect: transparent }
    on_reject:
      regenerate: { max: 3 }               # draw again, at most three takes

outputs:
  icon: ${{ steps.draw.outputs.image }}
```

## Plan, then run

```bash
grida-fx plan workflows/icon.yaml --name "copper lantern"
```

```
icon  ·  1 phase
phase 1   6 steps   1–3 provider calls   $0.21 – $0.87
cached    0 of 3 known steps
estimate  $0.21 – $0.87   ceiling $10.00
```

The plan counts every take regeneration may draw: three drawings and their three checks. The low
end is the one drawing that surely runs, at the low price of its size's tier; the high end is all
three, at the tier's high. The prices are the allowances of FX's built-in route table, in tiers by
size (`gpt-image-2.5-sunburst@openai` at `1024x1024`: $0.21 – $0.29 a call), which a project's own
tables can override.

```bash
grida-fx run workflows/icon.yaml --name "copper lantern" --live
```

`--live` is required whenever a run may call a paid provider. Without it nothing is spent: local
steps still run, a paid call the cache already answered is replayed, and any other paid call
fails its step (`image.generate on gpt-image-2.5-sunburst@openai is a paid call; run with --live`). A live
run also needs a ceiling, here the project's `budget:`. The run's folder holds its outputs;
`grida-fx inspect` summarises it.

## Run it again

Run the same command again and nothing is generated: every step is answered from the cache. Change
the name and only `draw` and `clean` run again. Rename a step, reorder your file, or change a
title, and nothing re-runs. [Cost, cache and takes](04-cost-and-cache.md) has the exact rule.

Don't like the icon? Ask for another take of that one step:

```bash
grida-fx reroll runs/icon/2026-10-02-1 draw     # take 2 from now on; take 1 stays in the cache
grida-fx run workflows/icon.yaml --name "copper lantern" --live    # draws take 2
grida-fx pick   runs/icon/2026-10-02-2 draw 1   # changed your mind: back to take 1, free
```

`reroll` and `pick` write your choice into `icon.takes.yaml` beside the workflow; every run
reads it.
