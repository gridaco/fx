# Example: 3D character

From an original brief, draw and review reference views, generate a textured mesh, assemble
it in Blender, and rig it. Independent agents review the references, mesh, assembly and
rig. Local checks verify the export and its diagnostic animation before admission.

This is the runnable source behind the [3D character showcase](https://grida.co/fx/workflows/character-3d/).
The current showcase is **Nami**: a frozen no-hood SD adaptation of the same original
character used by the facial and sprite examples. It keeps a short blonde-to-pink bob,
magenta eyes and the charcoal/pink outfit. The existing meadow-caretaker preset remains
an alternative original brief.
It uses real node bodies and FX's built-in routes. The separate `rigged-character` example
illustrates planned APIs; this project uses the public Python SDK available today.

```text
character-3d/
  workflow.yaml                    # the complete character pipeline
  fx.yaml                          # OpenRouter image/agent routes and Tripo mesh/rig routes
  inputs/nami/                     # current showcase canonical-image brief and settings
  inputs/meadow-caretaker/          # alternative original brief and settings
  showcases/nami-replay.yaml        # free replay of the exact retained rig finishing
  showcases/nami-cover.yaml         # free headless capture of the corrected rig clip
  showcase-inputs.json              # hash-pinned canonical and replay inputs on the CDN
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
uv run python ../fetch_showcase_inputs.py .
uv run python -m grida.fx doctor workflow.yaml --inputs inputs/nami/inputs.yaml
uv run python -m grida.fx plan workflow.yaml --inputs inputs/nami/inputs.yaml
```

Install Blender 5.2 or later and make `blender` discoverable on PATH. If needed, set
`GRIDA_FX_TOOL_BLENDER` to its executable. `jsonschema`, `numpy` and `Pillow` are this example's
Python dependencies; Blender uses its own bundled Python and NumPy. No external source checkout
or container is required.

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
uv run python -m grida.fx run workflow.yaml --inputs inputs/nami/inputs.yaml \
  --name nami --live --max-usd 30 --open
```

The $30 ceiling is a limit, not a completion guarantee. The engine stops before crossing it.
A fresh run generates new front/back references and meshes from the downloaded canonical
image; it does not contain the website's paid cache or reproduce its artwork byte for byte.
The supplied brief uses one whole mesh and fixed mitten hands, and does not promise articulated fingers, facial animation or
secondary motion. Pipeline admission is not game-engine qualification.

The workflow returns `character` (GLB), `result` (admission and findings), `references`
(canonical image), and `atlas` (rig-review evidence). With `review: none`, delivery is
explicitly unreviewed. `through: assembly` stops before rigging. Custom parts can replace the
brief through the declared `parts` input; the selected profile and partition decide the
required roles during planning.

## Replay Nami at $0

Nami's recorded paid trial produced the reference views, mesh, assembly and provider rig.
Its first automatic finishing attempt rejected preservation correspondence. A separate
recorded local finish supplied a measured global similarity transform and passed the
unchanged surface, UV, topology, winding, texture and material gates. Independent motion
review then found arm influences pulling the lateral skirt upwards.

The final showcased derivative explicitly corrects **514 exact lower-body vertices**:
remove the measured arm-chain weights and proportionally renormalize the existing
hips/torso/upper-leg weights. It adds no new joint assignment and changes no positions,
normals, UVs, triangles, textures, materials or animation tracks. This is an exact-asset
repair, not a general-purpose autorigger or a promise that a fresh generation needs it.
The original failed trial remains separate evidence.

Download the pinned canonical and retained source artifacts, then run the maintained free
finishing graph. Install Blender, FFmpeg and ffprobe on PATH; the cover uses a browser-free
Blender capture and Pillow WebP poster.

```bash
uv run python ../fetch_showcase_inputs.py .
uv run python -m grida.fx plan showcases/nami-replay.yaml \
  --inputs showcases/nami-inputs.yaml --max-usd 0
uv run python -m grida.fx run showcases/nami-replay.yaml \
  --inputs showcases/nami-inputs.yaml --name nami-replay --max-usd 0 --no-view
```

The fresh plan prints:

```text
nami-rig-replay  ·  1 phase
phase 1   7 steps   0 provider calls   $0.00
cached    0 of 3 known steps
estimate  $0.00   ceiling $0.00
```

The replay verifies the exact corrected model against the retained independent sampled
low-bar review, renders the original diagnostic amplitudes, and captures its actual cheer
clip at two-thirds playback speed. One fixed three-quarter camera frames all sampled poses;
there is no camera orbit or decorative scaling. The cover uses flat, unlit base-color textures
with their imported color factors and alpha, without added lights or surface shading. This
changes only the rendered preview, not the GLB's materials, geometry or animation.
Outputs include the corrected `character`,
qualified `record`, `result`, references, intermediate rigs, correction audit, selection,
`atlas`, and optional `cover_video`/`cover`/`capture`. Set `render_cover: false` in a later
inputs file to skip capture and FFmpeg. The separate `showcases/nami-cover.yaml` accepts a
finished GLB as `model` and captures only the cover.

The retained review passes the declared low bar at a 120-pixel gameplay character height.
An extra rear blossom remains a canonical-fidelity limitation. Fixed hands, no finger or
facial rig, no cloth/hair simulation, no continuous-motion or runtime qualification.
The CDN manifest pins bytes and can use `--base-url` to move the same content to another host.

The exact-asset replay was checked on macOS arm64 with Python 3.12.12, NumPy 2.5.3,
Pillow 12.3.0, Blender 5.2.0 LTS and FFmpeg 9.0.1. Blender inspects and renders the
rig; the local Python tools preserve and edit its GLB directly. Byte-identical
results across other builds are not promised. A source or result digest mismatch
requires explicit investigation and review; keep the strict checks intact.

## Independent source artwork

The previous showcase's transparent caretaker image was generated from text in a separate
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

Generated artwork, run records and caches are not included.
