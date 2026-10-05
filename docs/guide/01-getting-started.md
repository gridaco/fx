# Getting started

FX builds generated assets the way a build system builds code:
- You describe the steps in a **workflow file**.
- FX **plans** the run offline, with a cost estimate.
- It **runs** only what changed.
- Every result is kept with a record of how it was made.

## Install

```bash
npm install -g @grida/fx   # the grida-fx command (or run it without installing: npx grida-fx)
pip install grida          # the Python SDK, for nodes and builders you write in Python
grida-fx doctor            # checks the keys and tools your workflows need
```

Both packages are previews, and both carry the same engine. Python users who don't want Node run
it as `python -m grida.fx <verb>` (`python -m grida.fx doctor`), or through the Python API
([Running](05-running.md#from-python)). This guide writes `grida-fx`.

FX reads each provider's key from the environment: `OPENROUTER_API_KEY`, `OPENAI_API_KEY`,
`FAL_KEY`, `TRIPO_API_KEY`, `ELEVENLABS_API_KEY`. It never needs a `.env` file, and keys never go
in a project file.

### When FX needs Python

The engine itself runs the paid built-in types (`image.generate`, `structured.generate`, …),
`fx/select@1` and every file fact. Everything else runs on Python:
- your own Python nodes and builders, which import `from grida.fx import node, Ctx`;
- in the preview, the built-in local types too: `image.resize`, `image.crop`, `image.pad`,
  `image.mirror_repeat`, `image.check_alpha`, `image.check_size`, `json.merge`, `files.copy` and
  `package`.

A project that uses any of them needs a Python with `grida` and Pillow installed
(`pip install grida pillow`). FX uses `GRIDA_FX_PYTHON` when it is set, else the project's
`.venv`, else `python3` on `PATH`. Only a project whose steps are all paid built-ins and `select`
needs just the command. The first workflow below uses `image.check_alpha`, so it needs Python.

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
  image.generate: gpt-image-2@openai
```

`grida-fx nodes image.generate` lists the routes that can serve a capability: FX's built-in
route table, plus the tables your project lists under `route_tables:` (read after it, so their
entries win).

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
phase 1   6 steps   1–3 provider calls   $0.04 – $0.90
cached    0 of 3 known steps
estimate  $0.04 – $0.90   ceiling $10.00
```

The plan counts every take regeneration may draw: three drawings and their three checks. The low
end is the one drawing that surely runs, at the route's lowest price; the high end is all three,
at its highest. The prices in this guide are the illustrative ones in
[`examples/routes.yaml`](examples/routes.yaml) (`gpt-image-2@openai`: $0.04 – $0.30 a call);
your installation's route table has its own.

```bash
grida-fx run workflows/icon.yaml --name "copper lantern" --live
```

`--live` is required whenever a run may call a paid provider. Without it, FX refuses before
spending anything. The run's folder holds its outputs; `grida-fx inspect` summarises it.

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
