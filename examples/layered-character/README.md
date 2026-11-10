# Registered portrait repaints

A source-preserving foundation for a LivLiv character. One canonical portrait stays
unchanged. Codex repaints the **complete face** for each state; local steps align those
images and extract only agent-authored feature masks at the original coordinates.
There is no disconnected facial-parts atlas, mirroring, independent feature resizing,
or guessed anatomical placement.

The first workflow supports native blink and mouth replacement. The optional
[gaze workflow](workflows/gaze.yaml) adds separate, clipped irises and completed
eye surfaces for looking left, right, up and down. The optional
[puppet workflow](workflows/puppet.yaml) adds a source-owned head over a completed
neck and independent torso for procedural turns, nods, tilts and breathing.
Separate hair motion, brows, expressions and authored head keyforms remain future work.

[Moving layers and deformation](LAYERS.md) studies the player's exact layer,
clipping and keyform contracts and designs that extension: completed backing
surfaces, source-aligned cutouts, brows, hair and authored head turns. Gaze and the
procedural head/body split now have implementations; authored keyforms and the
remaining separately moving parts are still design work.

## Workflow

```text
master portrait + source-bound feature polygons
→ one native full-face working crop
→ five complete-face repaints: blink, half/open/round/wide mouth
→ bounded whole-face registration using unchanged texture
→ masked replacement patches at native integer offsets
→ exact neutral and outside-mask pixel checks
→ LivLiv v4 package
```

Every repaint includes the entire same face. The original source supplies open eyes
and the resting mouth. Only skin and changed features within the declared masks enter
the exported patches. Differences outside a mask diagnose provider drift; they never
authorize expanding the mask. A failed registration refuses the export.

The processing contract follows the working
[portrait-motion recipe](https://github.com/gridaco/fx/tree/main/examples/portrait-motion):
blend once in native pixels, preserve original alpha, and apply patches by integer
offset. [registered_patches.py](nodes/registered_patches.py) is self-contained code
adapted from that recipe. It requires NumPy, SciPy and Pillow, not a paid vision route.

## Run

Install `grida fx` with `npm install -g grida`, then:

```sh
uv sync
export GRIDA_FX_PYTHON="$PWD/.venv/bin/python"
# Optional: draw a master on your own Codex. Review it before authoring masks.
grida fx run workflows/master.yaml --live --max-usd 0 --deliver portrait=out/master.png
```

Author a geometry JSON for that exact portrait. [fern.geometry.json](inputs/fern.geometry.json)
shows the format: native source size and SHA-256, one working crop, polygons for the
image-left eye, image-right eye and mouth, and package anchors. These particular masks
are bound to the earlier generated Fern portrait, which is excluded from Git; they are
not a generic face detector. Trace masks around the original and repainted feature
extents with a boundary of unchanged skin. Keep unrelated features and hair outside.

Create `out/input.yaml`, with paths relative to that file:

```yaml
portrait: master.png
geometry: geometry.json
```

Then:

```sh
grida fx plan layered-character --inputs out/input.yaml
grida fx run layered-character --inputs out/input.yaml --live --max-usd 0   --deliver archive=out/character.livliv   --deliver preview=out/neutral.png   --deliver states=out/states/{key}.png   --deliver patches=out/patches/{key}.png   --deliver mask_overlay=out/mask-overlay.png   --deliver report=out/report.json
```

The five repaints draw on the user's signed-in Codex plan. FX books $0 and the project
has a $0 ceiling; no provider API fallback is configured. Codex chooses image size.
Only a whole-canvas normalization and bounded registration map a donor back to the
native crop; individual eyes or mouths are never fitted independently. Generated
artwork and saved runs remain in ignored `out/`, `.fx/` or `runs/`.

## What the package does

The base layer contains the original portrait at its original resolution. Two closed-eye
patches crossfade over it during a blink. The mouth slots share one native crop. The
resting slot is transparent, leaving the original mouth visible in the base without
adding alpha over it. The exported neutral must reconstruct the original RGBA pixels
exactly. No fresh neutral face is generated.

LivLiv's mouth renderer applies a small vertical easing to talking frames; blink
patches crossfade rather than producing a continuous anatomical eyelid deformation.
Changed patches can also increase alpha within their support when the player blends
them over the base. Native still-state RGB replacement preserves original alpha;
that guarantee is separate from player blending. This is a
[LivLiv v4](https://github.com/livliv-dev/player/blob/42a483ae9205e342bda91d063601ce91d4319f84/FORMAT.md)
package, not a Live2D Cubism or VRM model. No upstream character artwork is copied.

## Add four-direction gaze

[workflows/gaze.yaml](workflows/gaze.yaml) keeps the five expression repaints and
adds three complete-face donor images: skin beneath the eyes, whites with the
original irises removed, and a view completing the iris surfaces hidden by lids.
The images remain in the same source coordinate system. Local code creates the
eye backing and moves separate irises behind authored aperture masks; the
original pupils are removed from the base image rather than duplicated beneath
moving copies.

[fern.gaze.json](inputs/fern.gaze.json) shows the additional source-bound geometry:
one aperture, eye support, iris center/radii, pivot and travel range for each eye.
Completion landmarks describe the actual aligned donor; visible iris masks and
hole masks control which original pixels are reused and replaced. `L` and `R`
mean image-left and image-right, not the character's anatomical sides. Trace
these coordinates on your own master and inspect the masks at native size.

Add the gaze geometry to an inputs file, with paths relative to that file:

```yaml
portrait: master.png
geometry: geometry.json
gaze_geometry: gaze.json
```

Then plan and run the complete gaze workflow:

```sh
grida fx plan workflows/gaze.yaml --inputs out/gaze-input.yaml
grida fx run workflows/gaze.yaml --inputs out/gaze-input.yaml --live --max-usd 0 \
  --deliver archive=out/gaze/character.livliv \
  --deliver preview=out/gaze/neutral.png \
  --deliver states=out/gaze/states/{key}.png \
  --deliver report=out/gaze/report.json
```

The gaze stack has its own explicit canonical neutral. Eye-layer support regions
become opaque for compositing; the report records native RGB and alpha differences
from the untouched master. This is distinct from the first workflow's exact
original neutral. Original RGBA pixels outside the admitted eye regions stay
unchanged. Inspect the canonical neutral, iris rims, gaze extremes and closed
lashes before accepting artwork; a valid package or responsive pupil is not an
artistic pass.

## Add a procedural head and independent torso

[workflows/puppet.yaml](workflows/puppet.yaml) builds on the gaze package and makes
one additional full-portrait repaint: the head and hair are removed while the
hidden neck is completed in context. Only RGB inside an authored neck-fill mask
is imported. The original head, including its hair silhouette, becomes a separate
source-aligned layer; the torso retains the original clothing outside the repair.
Eyes, irises, lids and mouth follow the head rather than rotating with the jacket.

[fern.puppet.json](inputs/fern.puppet.json) shows the source-bound ownership mask,
completed-neck region, anchors and candidate pose ranges. Trace the jaw boundary
and retain the hair silhouette without assigning neck or shirt pixels to the head.
An optional neck-shadow polygon recovers a separate source-derived shadow against
the completed neck. An optional `neck_texture_polygon_xy` selects clean registered
skin inside the neck fill; local color extension from that interior excludes donor
outlines without resizing or moving the neck. Review the exposed neck and shadow
at each pose extreme.

Add this fourth input to your existing gaze inputs file:

```yaml
portrait: master.png
geometry: geometry.json
gaze_geometry: gaze.json
puppet_geometry: puppet.json
```

```sh
grida fx plan workflows/puppet.yaml --inputs out/puppet-input.yaml
grida fx run workflows/puppet.yaml --inputs out/puppet-input.yaml --live --max-usd 0 \
  --deliver archive=out/puppet/character.livliv \
  --deliver preview=out/puppet/neutral.png \
  --deliver report=out/puppet/report.json
```

The player supplies the rounded head volume and separate body deformation. This
is a restrained frontal 2D puppet, not a reconstructed 3D head or authored side
view. No measured head-turn keyforms are exported. `pose_bounds` are candidate
review ranges in the report; the package cannot enforce caller parameters.
Drive the player within ranges accepted through its actual pose sweeps.

The report separately measures neutral differences from the gaze master, including
opaque head-interior canonicalization and neck/shadow reconstruction. Original
soft silhouette alpha is retained. The Python assembler checks neutral composition;
head movement, attachment quality and breathing must be tested in the destination
renderer. Separate hair physics and expression morphs are not enabled.

## Check without a provider

```sh
uv run python check.py
uv run python check_gaze.py
uv run python check_puppet.py
```

[stand_in.py](stand_in.py) synthesizes a textured geometric portrait and full-face
repaints in place. Checks cover exact neutral reconstruction, unchanged pixels outside
masks, original alpha, registration/refusal, portable package references, $0 planning,
paid-call refusal, offline stand-ins and cache reuse. These are technical guarantees;
they do not judge anatomy, natural expressions or artistic acceptance.

[check_gaze.py](check_gaze.py) builds original synthetic eye surfaces and tracks
unique pupil pixels in both eyes. It checks all four travel directions, clipping,
removal of baked pupils, canonical neutral reconstruction, source hash/bounds
refusals, and the eight-call gaze workflow with offline stand-ins and cache reuse.

[check_puppet.py](check_puppet.py) checks independent head/torso ownership, a completed
neck without a baked face, exact synthetic neutral reconstruction, pose/hash/package
guards, and the full nine-call workflow with offline stand-ins and cache reuse. It
also checks that registration can exclude a head touching the image boundary while
requiring valid donor coverage only over the imported neck. It does not simulate
the player's pose engine or grant artistic acceptance.

[check_player.mjs](check_player.mjs) separately loads an export using a local upstream
player build. With Node, Bun and optionally FFmpeg:

```sh
git clone https://github.com/livliv-dev/player /tmp/fx-player
git -C /tmp/fx-player checkout 42a483ae9205e342bda91d063601ce91d4319f84
(cd /tmp/fx-player && bun build ./src/index.ts --outdir ./dist --target browser)
npm install --prefix /tmp/fx-player-tools playwright-core
node /tmp/fx-player-tools/node_modules/playwright-core/cli.js install chromium
node check_player.mjs --player-root /tmp/fx-player   --playwright /tmp/fx-player-tools/node_modules/playwright-core   --package out/character.livliv --out out/player-check   --profile portrait --neutral-reference out/master.png
```

The browser reports native first-frame source differences separately from technical
playback and saves a difference image. Pixel response alone does not establish quality.
Naturalness and likeness require visual review of the original and all composited
states. Mask overlays, registration metrics and each native state remain inspectable.
Use `--profile gaze` for a gaze package; `--profile portrait` exercises the first
workflow's blink/mouth controls without requiring iris layers.
Use `--profile puppet` for the head/torso package and its pose combinations.
