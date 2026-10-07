# Examples

FX projects, written from scratch the way a user would write them, with original settings and
characters. Each folder is a project: its README's commands work as written from the folder it
names. All of them run but rigged-character, which needs what FX does not have yet and only
plans.

| Example | What it shows | What runs |
|---|---|---|
| [hello](hello/) | your own nodes, built-in local steps, a judge, a keyed repeat, a folder of results, the cache | everything, offline, at $0, with no keys |
| [image-recolor](image-recolor/) | an image input, an algorithmic recolor, a five-second delay, plan/run viewing and the cache | ordinary Python code, offline, at $0, with no keys |
| [looping-parallax](looping-parallax/) | plan-time facts and assertions, fallbacks, a paid call inside your own node | offline when every layer mirrors; a repaint needs `OPENAI_API_KEY` |
| [concept-gallery](concept-gallery/) | structured answers, judges you write on a second model, two phases, takes | live, with `OPENROUTER_API_KEY` |
| [game-build](game-build/) | a game that builds its art from its own level files: a Python builder | the sky mirrors offline; the ground plate, icons and repaints need `OPENAI_API_KEY` |
| [rigged-character](rigged-character/) | agents, Blender tools, review rounds, a long job, resume | plans and prices only: FX serves no `vision.review` yet, and its Blender steps use tool scripts, which are planned |

Every example plans and prices offline. A run without `--live` runs every free step and refuses
each paid call at $0 (`… is a paid call; run with --live`). A live run uses your own keys, within
the ceiling you give with `--max-usd` ([Running](../docs/guide/05-running.md#keys-and-live-runs)).

## From a clone

For source development, run FX from a checkout of this repository. This needs git, cargo
([rustup.rs](https://rustup.rs)), [Bun](https://bun.sh) 1.4 and [uv](https://docs.astral.sh/uv/).

```bash
git clone https://github.com/gridaco/fx && cd fx
uv sync --project python          # the Python SDK and Pillow, in python/.venv
bun --no-env-file install --frozen-lockfile
python3 tools/build_engine.py     # the engine, built into that SDK
alias grida-fx="uv run --project '$PWD/python' python -m grida.fx"
cd examples/hello && grida-fx run hello --inputs inputs/badges.yaml
```

- **`tools/build_engine.py`** builds the viewer, embeds it into `grida-fx`, and puts the binary where the SDK looks
  for its engine (`python/src/grida/fx/_bin/`, ignored by git). Run it again after every pull, so
  the engine always matches the SDK beside it. After dependency changes, repeat the frozen Bun install.
  Direct Cargo builds also need `bun --no-env-file run build:viewer` first; a missing bundle fails the build.
- **The alias** runs the engine through the SDK, in `python/.venv`. That folder's `python3` is
  also the Python the engine starts your node bodies with, so they find `grida` and Pillow.
- **Another project** uses the same checkout through a path dependency on its `python/` folder,
  installed editable (with uv: `grida = { path = "<checkout>/python", editable = true }` under
  `[tool.uv.sources]`). Its `grida.fx` then drives the engine of that checkout, with no setting.

## From the packages

`npm install -g @grida/fx` installs the stable `grida-fx` command, and
`pip install grida` installs the Python SDK with the same engine inside
([Getting started](../docs/guide/01-getting-started.md#install)).

## The pictures

The pictures the examples read (the concept gallery's poster and the parallax layers) are flat
placeholder shapes, drawn by [`draw_placeholders.py`](draw_placeholders.py). It draws the same
pixels on every run. From the repository root:

```bash
uv run --project python python examples/draw_placeholders.py
```

## What CI checks

[`tools/check_examples.py`](../tools/check_examples.py) plans every example and runs every one
but rigged-character offline, on a copy, through the SDK, after `tools/build_engine.py`, as a
fresh clone would: `hello` runs whole and then entirely from the cache, `looping-parallax`
mirrors its layers, `game-build` mirrors its sky, and every paid call is refused at $0.
