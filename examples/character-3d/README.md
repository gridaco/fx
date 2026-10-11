# Example: 3D character

From an original brief, draw and review reference views, generate a textured mesh, assemble
it in Blender, and rig it. Independent agents review the references, mesh, assembly and
rig. Local checks verify the export and its diagnostic animation before admission.

This is the runnable source behind the [3D character showcase](https://grida.co/fx/workflows/character-3d/).
It uses real node bodies and FX's built-in routes. The separate `rigged-character` example
illustrates planned APIs; this project uses the public Python SDK available today.

```text
character-3d/
  workflow.yaml                    # the complete character pipeline
  fx.yaml                          # OpenRouter image/agent routes and Tripo mesh/rig routes
  inputs/meadow-caretaker/          # the original showcase brief and settings
  nodes/                           # setup, references, parts, assembly, rig and admission
  prompts/                         # producer and independent-review instructions
  profiles/                        # anatomy, joints, diagnostics and review criteria
  _character_harness.py             # private agent/Blender/review support
  _character_tools/                # private bounded Blender workers and numeric checks
  source-artwork/                   # independent text-to-transparent-image project
  calibration.yaml                # review an existing frozen, labelled rig subject
  make_fixture.py                  # synthesize a small offline input and textured box
```

## Plan and run

This is a Python-node project. From this folder, install its dependencies and use the Python
launcher's environment for both the command and its node host:

```bash
uv sync
uv run python -m grida.fx doctor workflow.yaml --inputs inputs/meadow-caretaker/inputs.yaml
uv run python -m grida.fx plan workflow.yaml --inputs inputs/meadow-caretaker/inputs.yaml
```

Install Blender 5.2 or later and make `blender` discoverable on PATH. If needed, set
`GRIDA_FX_TOOL_BLENDER` to its executable. `jsonschema` and `numpy` are this example's
Python dependencies; Blender uses its own bundled Python and NumPy. No Stage Gen package,
external source checkout or container is required.

The fresh plan with the supplied showcase settings prints:

```text
character-3d  ·  1 phase
phase 1   20 steps   45–93 provider calls   $2.60 – $128.90
cached    0 of 3 known steps
estimate  $2.60 – $128.90   ceiling $30.00   ⚠ the worst case exceeds the ceiling; the run stops before crossing it
```

Planning is offline. A run without `--live` refuses its first paid call at $0; it still
requires Blender because the workflow declares local Blender steps. Live execution needs
`OPENROUTER_API_KEY` and `TRIPO_API_KEY`, and an explicit spending ceiling:

```bash
uv run python -m grida.fx start --background
uv run python -m grida.fx run workflow.yaml --inputs inputs/meadow-caretaker/inputs.yaml \
  --name caretaker --live --max-usd 30 --open
```

The $30 ceiling is a limit, not a completion guarantee. The engine stops before crossing it.
A fresh checkout generates new references and meshes; it does not contain the website's
paid cache or reproduce its artwork byte for byte. The supplied brief uses one whole mesh
and fixed mitten hands, and does not promise articulated fingers, facial animation or
secondary motion. Pipeline admission is not game-engine qualification.

The workflow returns `character` (GLB), `result` (admission and findings), `references`
(canonical image), and `atlas` (rig-review evidence). With `review: none`, delivery is
explicitly unreviewed. `through: assembly` stops before rigging. Custom parts can replace the
brief through the declared `parts` input; the selected profile and partition decide the
required roles during planning.

## Independent source artwork

The website's original transparent caretaker image was generated from text in a separate
workflow. It is not the 3D workflow's reference input: the 3D pipeline draws its own views
from the brief.

```bash
uv run python -m grida.fx plan source-artwork/workflow.yaml
uv run python -m grida.fx run source-artwork/workflow.yaml --live --max-usd 1 --no-view
```

That nested project preserves its direct OpenAI route and requires `OPENAI_API_KEY` for a
live run. Its fresh plan is:

```text
meadow-traveler-source  ·  1 phase
phase 1   1 steps   1 provider calls   $0.16 – $0.24
cached    0 of 1 known steps
estimate  $0.16 – $0.24
```

## Offline fixture

```bash
uv run python make_fixture.py
uv run python -m grida.fx plan workflow.yaml --inputs inputs/fixture/inputs.yaml
# With Blender installed: local normalization and export validation, no provider calls.
uv run python -m grida.fx run inputs/fixture/normalize.yaml \
  --inputs inputs/fixture/normalize-inputs.yaml --no-view
```

The generated fixture is a flat geometric reference and a textured box, not character art.
It stays under ignored `inputs/fixture/`. The local check imports and re-exports its GLB,
verifying embedded textures, dimensions and mesh integrity with the same worker used by the
paid workflow. It does not validate artistic quality or the entire paid pipeline.

`calibration.yaml` accepts a caller-supplied `rig_review_subject_v2` JSON and its hash-bound
candidate GLB. It exercises the same rig reviewer without generating a character. Labels
and reference subjects are not bundled with this example.

Source attribution and the preserved BSD terms are in [NOTICE.md](NOTICE.md) and
[LICENSE.stage-gen](LICENSE.stage-gen). Generated artwork, run records, caches and old
runtime binaries are not included.
