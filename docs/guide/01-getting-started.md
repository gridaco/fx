# Getting started

FX builds generated assets the way a build system builds code:
- You describe the steps in a **workflow file**.
- FX **plans** the run offline, with a cost estimate.
- It **runs** only what changed.
- Every result is kept with a record of how it was made.

## Install

```bash
npm install -g grida   # the grida command, with FX as grida fx (Node.js 24 or later)
pip install grida      # the Python SDK, for nodes and builders you write in Python
npm install @grida/fx  # in a project: the SDK for nodes and builders you write in TypeScript
grida fx doctor        # checks the keys and tools your workflows need
```

`grida` carries the FX version it was released with; updating `grida` updates FX. No repository
clone or Rust toolchain is needed. FX runs on macOS, and on Linux with glibc 2.28 or later, on x64
and arm64. On Windows, use WSL 2. Install the SDK of the language you write your own nodes in:
one language per project is the usual choice, though a project may mix them.

This guide writes `grida fx`. You may also meet FX as `grida-fx` or `python -m grida.fx`: these are
the same FX, started another way, and take the same commands (`grida-fx plan` is `grida fx plan`).

- `grida-fx` is FX on its own, from the `@grida/fx` package, which is also the JavaScript and
  TypeScript SDK and the host of TypeScript nodes. Projects that depend on `@grida/fx`, and servers
  or CI images that need only FX, use it.
- `python -m grida.fx` is FX from the Python package, for Python users who don't want Node
  ([Running](05-running.md#from-python)).

They share project files, the cache and run records. `--version` prints the FX version under any
of them, so you can tell two installations apart. Setting them up, and source development, are in
[CONTRIBUTING.md](../../CONTRIBUTING.md#ways-to-run-fx).

**One key for everything: Grida Gateway.** Create a key at
[console.grida.co](https://console.grida.co/_/ai-gateway) and set it as `GG_API_KEY`. It serves every route on
Grida Gateway (`@gg`): text, agent turns, images, video, 3D models and rigs, billed to your
organization's Grida AI credits, so there is no provider account to open for each model. Set it in
the environment, or put it in a `.env` file in your project (`GG_API_KEY=…`, and keep the file
out of version control); the environment wins. Keys never go in `fx.yaml` or a workflow, and FX
never prints them: only a `--live` run uses them, and `grida fx doctor` says which ones it found,
where, and which routes they make usable ([Running](05-running.md#keys-and-live-runs)).

Already have provider accounts? FX also reads each provider's own key under its usual name
(`OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `FAL_KEY`, `TRIPO_API_KEY`, `ELEVENLABS_API_KEY`) for the
routes on that provider; sound, speech and music need one, since GG has no route for them yet. No
key at all? With Codex signed in to your ChatGPT account, images can come from
[your own Codex](12-codex.md). [AI providers](13-providers.md) compares the ways.

### When FX needs Python

The engine itself runs the paid built-in types (`image.generate`, `structured.generate`, …),
`fx/select@1` and every file fact. Your own nodes and builders run in their language: TypeScript
ones on Node ([below](#when-fx-needs-node)), Python ones on Python. Python runs:
- your own Python nodes and builders, which import `from grida.fx import node, Ctx`;
- the built-in local types, whatever language your own nodes are in: `image.resize`,
  `image.crop`, `image.pad`, `image.mirror_repeat`, `image.check_alpha`, `image.check_size`,
  `json.merge`, `files.copy` and `package`.

A project that uses any of them needs Python 3.11 or later with `grida` and Pillow installed
(`pip install grida`, which brings Pillow). FX uses `GRIDA_FX_PYTHON` when it is set, else
the `.venv` of the workflow's project, else the `.venv` of the project you run from, else the Python
that runs the SDK when you run through it ([Running](05-running.md#from-python)), else `python3`
on `PATH`. An older Python is refused before FX starts it (macOS's own `/usr/bin/python3` is
3.9): run `uv sync` or `pip install grida` in the project, or set `GRIDA_FX_PYTHON`. A project
whose steps are all paid built-ins, `select` and nodes of your own in TypeScript needs no Python.
The first workflow below uses `image.check_alpha`, so it needs Python.

### When FX needs Node

Nodes, builders and [stand-ins](05-running.md#stand-ins-testing-without-a-provider) you write in
TypeScript (or JavaScript) run on Node, in the node host that `@grida/fx` carries. A project with
any of them needs:
- **Node 24 or later.** Node runs your `.ts` files as they are, stripping their types, and tells
  FX which files each node imports ([Nodes](03-nodes.md#writing-a-node-type-in-typescript)). Bun
  and Deno cannot host nodes.
- **`@grida/fx` in the project** (`npm install @grida/fx`), whose `node` your modules import. FX
  starts the host from the `@grida/fx` the workflow's project would import (its `node_modules`, or
  a parent folder's), else the one the project you run from would import; without either, from
  the `@grida/fx` that started FX, such as the one `grida fx` carries.

FX uses `GRIDA_FX_NODE` when it is set, else the Node that runs `grida fx` or the JavaScript SDK
([Running](05-running.md#from-javascript)), else `node` on `PATH`. An older Node is refused before
FX starts it:

```
/usr/local/bin/node is Node 22.12.0; FX's TypeScript node host needs Node 24 or later: install it, or set GRIDA_FX_NODE
```

Started as `python -m grida.fx`, FX has no Node of its own to fall back on: it uses
`GRIDA_FX_NODE` or `node` on `PATH`, and only a project's own `@grida/fx`. `grida fx doctor`
names the Node it would use on its `node` line ([Running](05-running.md#keys-and-live-runs)).

## A project

A project is a folder with an `fx.yaml` in it. Start a new one with:

```sh
npx grida fx init my-assets
cd my-assets
grida fx start --background
grida fx status --json
```

`init` writes a starter project into a new or empty folder: `fx.yaml`, a `README.md` that says how
to install the CLI and connect an AI provider, an `AGENTS.md` and the FX skills
(`.agents/skills/`) for coding agents, a `.gitignore` that keeps runs, the cache, `.env` and
`node_modules/` out of git, an `.env.example` listing the key names, and `workflows/icon.yaml`, a
one-step workflow that draws an icon once you pick a route. It installs nothing and starts nothing.
On a terminal it asks which template to start from; `--template NAME` names one without asking, and
`grida fx init --list-templates` lists them. A template is one of the
[examples](https://github.com/gridaco/fx/tree/main/examples), downloaded from the FX release you
have, with the same agent files added. In a folder that already has an `fx.yaml`, it checks that file and
writes nothing. It refuses any other folder that has files in it: to make an existing folder (a
game repository, say) a project, write its `fx.yaml` yourself, holding `fx: project/v1`.

`start` serves the bundled
dashboard at `http://127.0.0.1:8787/`. Add `--open` to open it, or omit
`--background` to keep the service in your terminal. `grida fx stop` stops the
service without stopping workflows or deleting their records. Background mode
survives the launching command; it does not install crash or login supervision.
These commands need FX 0.2 or later; `grida fx --version` names yours.

As you author workflows and run them, a project can look like this:

```
my-assets/
  fx.yaml             # project settings
  workflows/          # your workflow files
  nodes/              # your own node types (optional)
  prompts/            # prompt templates (optional)
  inputs/             # files you feed in
  package.json        # "type": "module", and @grida/fx   (TypeScript nodes only)
  tsconfig.json       # for type checking                 (TypeScript nodes only)
  node_modules/       # what npm installs                 (TypeScript nodes only)
  runs/               # every run, one folder each   (created by FX)
  .fx/cache/          # content-addressed results    (created by FX)
  .fx/service/        # local service state and catalog (created by FX)
```

The starter leaves routes and budgets commented out for you to choose. For the paid example
below, set them:

```yaml
# fx.yaml
fx: project/v1
budget:
  max_usd: 10                 # the ceiling for any one run; --max-usd overrides it
routes:                       # which model serves each capability by default
  image.generate: openai/gpt-image-2.5-sunburst@gg
# workflows: [workflows, ../tools/art/workflows]   # where a workflow id is looked for
```

`grida fx nodes image.generate` lists the routes that can serve a capability: FX's built-in
route table, plus the tables your project lists under `route_tables:` (read after it, so their
entries win).

**Where workflows are found.** A workflow named by its id (`grida fx run icon`) is looked for among
the `.yaml` and `.yml` files at the project root, then in `workflows/` and every folder below it. A
monorepo that keeps its workflows elsewhere lists their folders under `workflows:`, searched in the
order given; each is relative to the project or absolute, and may lie outside the project. The list
replaces the default, so name `workflows` too if you keep workflows there as well. The project root
itself, or a folder above it, is refused, since it would search your runs and cache. A workflow in a
folder with an `fx.yaml` of its own keeps that project as its home: its `./` paths, node modules and
route defaults are that project's, with your `routes:` winning. The runs, the cache, the budget, the
route tables and the takes file (`<id>.takes.yaml` at your root) stay those of the project you run
from.

Keep `runs/`, `.fx/` and `node_modules/` out of source control. A project file is recommended,
not required for execution. For a one-off workflow, call `run` directly; use
`--standalone` inside an existing project to request an independent viewer for
that invocation. See [Viewing](07-viewing.md#standalone-inspection).

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
grida fx plan workflows/icon.yaml --name "copper lantern"
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
size (`openai/gpt-image-2.5-sunburst@gg` at `1024x1024`: $0.21 – $0.29 a call), which a project's own
tables can override.

```bash
grida fx run workflows/icon.yaml --live -- --name "copper lantern"
```

`--live` is required whenever a run may call a paid provider. Without it nothing is spent: local
steps still run, a paid call the cache already answered is replayed, and any other paid call
fails its step (`image.generate on openai/gpt-image-2.5-sunburst@gg is a paid call; run with --live`). A live
run also needs a ceiling, here the project's `budget:`. The run's folder holds its outputs;
`grida fx inspect` summarises it.

With the local service running, add `--open` to a plan or run to open its canvas.
The run URL stays available after execution finishes. A missing service does not
prevent execution; start it explicitly when you want browser inspection.

## Run it again

Run the same command again and nothing is generated: every step is answered from the cache. Change
the workflow input `name` (after `--`) and only `draw` and `clean` run again. Rename a step, reorder your file, or change a
title, and nothing re-runs. [Cost, cache and takes](04-cost-and-cache.md) has the exact rule.

Don't like the icon? Ask for another take of that one step:

```bash
grida fx reroll runs/icon/2026-10-02-1 draw     # take 2 from now on; take 1 stays in the cache
grida fx run workflows/icon.yaml --live -- --name "copper lantern"    # draws take 2
grida fx pick   runs/icon/2026-10-02-2 draw 1   # changed your mind: back to take 1, free
```

`reroll` and `pick` write your choice into `icon.takes.yaml` beside the workflow; every run
reads it.
