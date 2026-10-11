# Example: terrain atlas

The authoring source for the [terrain showcase](https://grida.co/fx/workflows/terrain/).
Paint one material over a locked 47-mask topology, validate the returned canvas, assemble
an atlas, and use its neighborhood lookup to render a deterministic map preview.

## Run it

From this folder, install and activate the example's Python environment:

```bash
uv sync
source .venv/bin/activate
python ../fetch_showcase_inputs.py .
grida fx plan workflow.py:build
```

The checksum-verified public download supplies the showcase's original meadow concept
and published atlas in ignored `.showcase-inputs/`; it makes no AI/provider call.
`direction.txt` owns the material instruction. To use your own reference, change the reference passed by
`build()` and revise that direction. Python-only users can replace `grida fx` with
`python -m grida.fx`.

With an explicitly chosen ceiling and `OPENAI_API_KEY` configured:

```bash
grida fx run workflow.py:build --live --max-usd 15 --no-view
```

`--no-view` runs without a project service. For inspection, run `grida fx start` in
another terminal, omit `--no-view`, and optionally add `--open`.

The outputs are `atlas`, `validation`, `preview`, `preview_manifest`, `cover`,
`cover_manifest`, and a packaged atlas/preview handoff. The atlas is 1440×480:
twelve columns by four rows of 120px cells. Its 47-mask lookup selects occupied-cell
neighbors in the order NW, N, NE, W, center, E, SW, S, SE.
The reserved placeholder stays transparent. Technical admission does not certify art.

The shared downloader is a source-checkout convenience, not an installed FX command.
A copied standalone project can instead bind a reference image you supply.

## Render a cover at $0

Use the already published atlas, or supply your own canonical atlas in an inputs YAML
file. This workflow makes no provider calls:

```bash
python ../fetch_showcase_inputs.py .
grida fx plan render-cover.yaml --inputs cover-inputs.yaml
grida fx run render-cover.yaml --inputs cover-inputs.yaml --no-view \
  --run .fixture/cover-showcase \
  --deliver cover=.fixture/cover.webp \
  --deliver cover_manifest=.fixture/cover-layout.json
```

`cover-inputs.yaml` binds `.showcase-inputs/atlas.png`. `cover.py` deterministically
builds the digits “47” standing on shared ground with the existing neighborhood
compositor. The entire silhouette is one connected terrain map, rather than text
painted over a picture. The ground extends beyond the left, right and bottom of
the frame; its physical outer edges stay offscreen. It returns a 1920×1200 WebP,
a JSON layout with the source
digest, map rows/coordinates and digit-cell placements, and the exact original
atlas. It does not repaint the tiles. The main generation workflow also returns
this cover while preserving its existing transparent preview.

The cover node follows source identity, including its imported helpers and declared
lookup resource. Editing its layout rerenders this local step. The atlas-producing
steps and their paid requests are unchanged.

To export a finished derivative recording for inspection or publication:

```bash
grida fx export .fixture/cover-showcase --out .fixture/cover-export --json
```

The export directory must be new. This records the local presentation and supplied
atlas; the original paid generation retains its separate recording.

### Optional animated cover

With FFmpeg on `PATH`, a headless Python simulation makes an 8-second, 960×600,
20fps MP4 at $0. One binary grid retains its state as the digits are removed
top to bottom and rebuilt bottom to top. Every frame selects tiles from the
current neighbors using the canonical 47-mask lookup. The ground cells stay
occupied, but their edges update as the digits disconnect and reconnect.
The complete “47” holds at both ends for a seamless temporal loop.

```bash
grida fx run render-cover-video.yaml --inputs cover-inputs.yaml --no-view \
  --run .fixture/cover-video-showcase \
  --deliver cover_video=.fixture/cover.mp4 \
  --deliver cover=.fixture/cover.webp
```

This is a separate local workflow. The main atlas workflow and still cover do not
require FFmpeg. Rendering needs no browser, JavaScript or provider; the website
only plays the resulting video. This does not clip or fade a finished image.
`python check_cover_motion.py` checks the evolving grid, neighbor selection,
loop endpoints and complete video workflow with synthesized input.

## Offline fixture

```bash
python make_fixture.py
grida fx plan workflow.py:build_fixture
grida fx run workflow.py:build_fixture --no-view
python check_cover.py
```

This uses locally drawn shapes in `.fixture/` and preserves the same graph and prompts.
The run packs the real topology template, then refuses the paid edit at $0. A fresh
fixture plan from the current engine prints:

```text
showcase-meadow-terrain  ·  1 phase
phase 1   17 steps   1–6 provider calls   $0.10 – $1.92
cached    0 of 1 known steps
estimate  $0.10 – $1.92   ceiling $15.00
```

`check_cover.py` instead runs the complete free cover workflow on a synthesized
atlas. It checks the WebP dimensions, occupied-cell coordinates, the shared ground,
layer boundaries and exact source-atlas passthrough, with credentials removed and
network disabled. Neither the still workflow nor this check requires FFmpeg.

`terrain_example/` contains this example's private assembly, validation, guide-detection
and composition helpers. It requires only the public FX SDK and its Pillow dependency.
The small, non-generated topology template is preserved because its exact pixels are
part of the paint instruction; see [TERRAIN-ATTRIBUTION.md](TERRAIN-ATTRIBUTION.md) for
its CC BY 3.0 provenance. Helper code retains its [source attribution](ATTRIBUTION.md).
Generated art, runs and caches stay outside Git.
