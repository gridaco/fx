# Example: interface kit

The authoring source for the [interface showcase](https://grida.co/fx/workflows/ui-kit/).
One visual reference and a style direction produce three independently validated
sheets: a nine-slice panel, four button states, and sixteen icons. The workflow renders
each geometry template, paints it, checks and normalizes it, produces visual evidence,
and requests an independent structured review.

## Run it

From this folder, install and activate the example's Python environment:

```bash
uv sync
source .venv/bin/activate
python ../fetch_showcase_inputs.py .
grida fx plan workflow.py:build
```

The download verifies the public meadow concept against `showcase-inputs.json` and
writes into ignored `.showcase-inputs/`, without an AI/provider call. `ui.json` selects
the roles, instructions and digest-bound reference; `style.txt` supplies their common
visual direction. To use your own reference, update both its path and SHA-256 in
`ui.json`. Python-only users can replace `grida fx` with `python -m grida.fx`.

After choosing a ceiling and configuring `OPENAI_API_KEY` and `OPENROUTER_API_KEY`:

```bash
grida fx run workflow.py:build --live --max-usd 15 --no-view
```

`--no-view` runs without a project service. To inspect it while running, start
`grida fx start` in another terminal, omit `--no-view`, and optionally add `--open`.

Each role returns an image, validation JSON, evidence image and review JSON. The
package bundles them for a consumer. The engine owns paid calls and retries; judges
can request a new billed painting within the ceiling. The independent model review
is retained as evidence and never substitutes for human visual or rights approval.

The shared downloader is a source-checkout convenience, not an installed FX command.
A copied standalone project can instead bind a reference image you supply.

## Offline fixture

```bash
python make_fixture.py
grida fx plan workflow.py:build_fixture
grida fx run workflow.py:build_fixture --no-view
```

The script draws a separate reference and digest binding into `.fixture/`. It leaves
the authored showcase input untouched. The run renders all three geometry templates
and review schemas, then refuses the paid edits at $0. A fresh fixture plan prints:

```text
showcase-meadow-ui  ·  1 phase
phase 1   49 steps   6–21 provider calls   $0.78 – $12.24
cached    0 of 6 known steps
estimate  $0.78 – $12.24   ceiling $15.00
```

`ui_example/` owns the private sheet geometry, alpha normalization, evidence renderers,
models and review shapes. Dependencies are the public FX SDK, its Pillow dependency,
and Pydantic. No game package or frozen runtime is required. The original algorithms
retain their [source attribution](ATTRIBUTION.md). Generated artwork and historical
run packages stay on the CDN, outside Git.
