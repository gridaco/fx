# Example: illustrated world

The authoring source for the [world showcase](https://grida.co/fx/workflows/world/).
A supplied poster establishes the visual language; a synopsis establishes the facts.
The workflow proposes and checks a connected world, plans and reviews its gallery, then
fans out into one directed image and independent review per entity. Rejected image
studies remain visible in the delivered records.

## Run it

From this folder, install the example's Python dependencies and activate its interpreter:

```bash
uv sync
source .venv/bin/activate
python ../fetch_showcase_inputs.py .
grida fx plan workflow.yaml --inputs inputs/windward_meadow/inputs.yaml
```

The fetch command retrieves the exact public showcase poster, checks its SHA-256 and
size, and writes it into ignored `.showcase-inputs/`. It makes no AI/provider call.
The synopsis and expansion direction are tracked beside the input binding. Change
those files and the poster binding to author your own world. Python-only users can
replace `grida fx` with `python -m grida.fx`.

For a paid run, after choosing a ceiling and configuring `OPENROUTER_API_KEY`:

```bash
grida fx run workflow.yaml --inputs inputs/windward_meadow/inputs.yaml --live --max-usd 25 --no-view
```

`--no-view` runs without a project service. To inspect it while running, start
`grida fx start` in another terminal and omit `--no-view` (add `--open` to open the browser).
The ceiling stops the run before it would cross $25; it does not promise every possible
regeneration will fit. The model review is evidence, not publication or rights approval.

The shared downloader is a source-checkout convenience, not an installed FX command.
A copied standalone project can instead bind a reference image you supply.

## Offline fixture

```bash
python make_fixture.py
grida fx plan workflow.yaml --inputs .fixture/inputs.json
grida fx run workflow.yaml --inputs .fixture/inputs.json --no-view
```

The fixture draws simple shapes locally and never replaces the showcase poster. The
last command resizes that poster and then refuses the first paid call at $0. A fresh
fixture plan from the current engine prints:

```text
universe  ·  2 phases
phase 1   42 steps   4–19 provider calls   $0.08 – $11.40
phase 2   entity (up to 6)   ≤ $51.00   priced exactly when its list exists
cached    0 of 2 known steps
estimate  $0.08 – $62.40   ceiling $25.00   ⚠ the worst case exceeds the ceiling; the run stops before crossing it
```

## Source layout

- `workflow.yaml` is the complete declaration; `inputs/windward_meadow/` owns its story.
- `prompts/` and `schemas/` hold the exact generation and review instructions.
- `world_example/` owns the private models, medium rules, deterministic judges and nodes.
- `views/gallery.html` is the workflow's custom artifact gallery.

Only the public `grida.fx` SDK and Pydantic are required. There is no dependency on a
separate game or asset application, frozen engine, local cache, or historical run.
Source attribution is in [ATTRIBUTION.md](ATTRIBUTION.md). Published imagery and run
packages stay on the CDN; moving this authoring source does not regenerate them.
