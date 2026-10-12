# Examples

Runnable FX projects with original settings and characters. Each folder's README gives
its inputs, dependencies and commands. The website's authoring projects live here too:
source stays in Git, while generated artwork and saved runs stay outside it.

| Example | What it shows | What runs |
|---|---|---|
| [hello](hello/) | custom nodes, local steps, a judge, keyed repeats, result collections and cache | entirely offline at $0 |
| [image-recolor](image-recolor/) | image inputs, algorithmic recoloring, delays and live run viewing | entirely offline at $0 |
| [looping-parallax](looping-parallax/) | reusable repeating layers and the website's complete concept-to-parallax graph | mirrored layers offline; separation and seam repainting paid; optional video uses FFmpeg |
| [world](world/) | synopsis, poster and direction into reviewed illustrated entities | free preparation offline; structured proposals and illustrations paid |
| [movie-sprite](movie-sprite/) | an image into a video take, then transparent loop finishing | generation paid; downloaded or supplied footage finishes locally at $0 with FFmpeg |
| [portrait-motion](portrait-motion/) | independent face states, registration, native patches and combinations | state generation paid; synthesized stand-ins exercise both branches at $0 |
| [character-3d](character-3d/) | reviewed reference views, mesh generation, Blender assembly, rigging and admission | current runnable graph; local steps need Blender, generation and review paid |
| [terrain](terrain/) | Python builder, fixed 47-mask atlas, validation and deterministic map preview | preparation and preview local; atlas generation and review paid |
| [ui-kit](ui-kit/) | Python builder, nine-slice panels, button states, icons and measured geometry | preparation and validation local; artwork and review paid |
| [concept-gallery](concept-gallery/) | structured answers, independent judges, phases and takes | paid generation and review |
| [game-build](game-build/) | a Python builder reading its application's level files | sky mirrors offline; ground, icons and repaints paid |
| [rigged-character](rigged-character/) | an illustrative graph with APIs FX has not implemented | plans only; use character-3d for the runnable pipeline |

Every example plans offline. A run without `--live` executes free steps and refuses paid
calls at $0. Live execution uses your keys, within the ceiling supplied with
`--max-usd` ([Running](../docs/guide/05-running.md#keys-and-live-runs)): the routes on Grida
Gateway (`@gg`) all take one key, `GG_API_KEY`, and the others their provider's. A
ceiling limits spending; it does not guarantee completion or artistic quality.

## Use an example

`npm install -g grida` installs `grida fx`. Python-node projects also need the Python
SDK and their own processing libraries. Copy the example and follow its README:
most showcase projects use `uv sync`, with dependencies declared in their own
`pyproject.toml`. These application helpers are not installed with the FX SDK.

To use the checkout's SDK and engine, follow
[the source build instructions](../CONTRIBUTING.md#build-from-source). Small examples
can then use this alias from the checkout root:

```bash
alias grida-fx="uv run --project '$PWD/python' python -m grida.fx"
cd examples/hello && grida-fx run hello --inputs inputs/badges.yaml
```

The alias uses the checkout's Python environment. Larger projects need the additional
libraries listed in their READMEs. Rebuild the engine after changing its source.

## Images and recorded inputs

Small fixture pictures are deterministic shapes drawn by
[`draw_placeholders.py`](draw_placeholders.py). Recreate them from the repository root:

```bash
uv run --project python python examples/draw_placeholders.py
```

The seven website showcase projects include their workflow declarations, private helper
code, prompts, schemas and authored settings. Generated reference images and the original
movie take are optional public CDN downloads, pinned by SHA-256 and byte size. For example:

```bash
# From the repository root; no provider call.
python examples/fetch_showcase_inputs.py examples/world
# After a CDN migration, identical bytes can come from another base.
python examples/fetch_showcase_inputs.py examples/world --base-url https://assets.example.com
```

Parallax retains its dedicated `fetch_showcase_input.py`. Downloads go into ignored
`.showcase-inputs/` folders and never replace conflicting files. The download metadata
records the inputs' origin; it grants no additional media rights. The small terrain topology
template is an authored source resource with its original attribution, not generated art.

A maintained example and a frozen website recording have different lifetimes. Source
changes do not regenerate or replace recordings. Fresh paid runs can differ from the
published artwork. Movie finishing can replay its original take at $0; its exact face pin
is deliberately separate from the settings for generating a new take.

## What CI checks

[`tools/check_examples.py`](../tools/check_examples.py) plans the small examples and runs
their free paths on fresh copies using the current checkout's SDK and embedded engine.
Paid calls are refused at $0; hello also proves cache reuse. The complete parallax graph
plans and refuses its paid calls using synthesized input.

[`tools/check_showcase_examples.py`](../tools/check_showcase_examples.py) checks the other
six website projects with synthesized inputs, without CDN access, provider keys or dotenv
loading. It plans the actual graphs, checks paid-call refusal, finishes a synthetic movie,
and exercises both portrait branches with stand-ins. The 3D graph always plans; actual local
normalization requires Blender and is separately documented in that example.

For these larger examples, install FFmpeg and the example-only processing dependencies
into the checking environment after building the engine:

```bash
uv pip install --python python/.venv/bin/python -r examples/requirements-ci.txt
python/.venv/bin/python tools/check_showcase_examples.py
```

These checks prove executable wiring and structural outputs, not artistic acceptance.
They never generate or publish paid artwork.
