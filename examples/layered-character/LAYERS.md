# Moving layers and deformation: design study

This studies extensions of the working full-face repaint recipe, not an FX
language contract. The first [workflow](workflow.yaml) exports native blink and
mouth patches. The newer [gaze workflow](workflows/gaze.yaml) implements the eye
stack described here. The [puppet workflow](workflows/puppet.yaml) adds a completed
neck and independent head/body layers for procedural pose deformation. Authored
keyforms, brows, separate hair motion and expressions remain proposed.

The target is a source-aligned anime puppet with gaze, eyelid movement, brows,
small head turns/nods, breathing and separately moving hair. Generate complete
portraits or contextual crops, then extract layers in source coordinates. The
generator supplies appearance; authored masks, landmarks and motion bounds supply
geometry and ownership.

## What the player actually accepts

This study reads LivLiv Player at
[42a483ae9205e342bda91d063601ce91d4319f84](https://github.com/livliv-dev/player/tree/42a483ae9205e342bda91d063601ce91d4319f84).
Its [format contract](https://github.com/livliv-dev/player/blob/42a483ae9205e342bda91d063601ce91d4319f84/FORMAT.md)
and [runtime](https://github.com/livliv-dev/player/blob/42a483ae9205e342bda91d063601ce91d4319f84/src/runtime/puppet.ts)
describe three different mechanisms:

| Mechanism | Suitable features | Actual contract |
|---|---|---|
| Procedural mesh deformation | Gaze, blinking, brows, head/torso movement, breathing | Native layer boxes, semantic roles, anchors, rig settings and shared mesh lattice |
| Spring-driven mesh deformation | Hair, cords, ornaments | Hair groups or named `rig.chains`, attached by layer `bones`; optional `rigid` attachment |
| Authored keyform deformation | Corrected head turns and nods | Optional `keyforms.json`: displacement grids for `angleX` and `angleY` |
| Texture switching and expression blending | Speech shapes and expressions | Discrete mouth `frames`/`visemes`, blended expression patches and talking variants |

These mechanisms can run together. A mouth frame is not a mesh morph. A keyform
warps existing texture; it cannot paint a newly visible ear or side of the nose.

Layers use native integer `x`, `y`, `w`, `h`; every associated image matches that
box and uses straight alpha. The `layers` array sets back-to-front drawing order;
`depth` controls parallax, not drawing order. Groups are `head`, `body`,
`hair_back`, `hair_side`, `hair_front` and `hair_inner`. `L`/`R` mean image-left/right.
The player makes meshes on one canvas-space lattice so overlapping pieces can
share deformation without drifting apart. See FORMAT sections 5–7 and Appendix B;
`puppet.ts` lines 749–779.
Consistent attachment also requires matching deformation groups, depth and fields
at shared boundaries. Mesh occupancy uses the main texture and mouth frames;
`mask`, `talk` and `brow_tex` do not expand it. Include their required support in
the mesh-producing images and keep every associated image in the same layer box.

## Extraction needs a backing surface

A replacement patch includes already-blended skin around a changed feature. It
works at one fixed offset. A moving cutout instead needs a foreground alpha matte
and the surface it will uncover. Difference masks are useful change diagnostics;
they do not identify foreground coverage or reconstruct hidden surfaces.

| Moving part | Extracted foreground | Required completed backing surface | Playback |
|---|---|---|---|
| Iris | Iris and pupil; initially keep highlights in this texture | Eye whites with the old iris removed, including its swept area | `iris_L/R`, `clip`, `range` |
| Open eyelid | Whites and original lash line, without iris | Skin replacing the original open eye | `eye_L/R`, lossless whites mask, pivot |
| Closed eyelid | Closed lash/skin state from a full-face repaint | Same eye skin | `lid_L/R`, beneath open eye |
| Brows | Original brow strokes with authored edge alpha | Skin or bangs with the baked-in brows removed | `brows` layer and brow anchors/lines |
| Mouth/jaw | Existing native speech patches; later a properly separated mouth | Neutral skin where the source mouth was removed | Mouth frames plus measured jaw anchors |
| Front/side hair | Original visible strands, completed where needed | Forehead, ear, neck, clothing or back hair behind each strand | Hair groups and optional chains |
| Head | Face, ears and associated layers with complete attachment edges | Neck/collar/torso exposed during motion | Shared head deformation and keyforms |
| Torso/accessories | Body or individual accessory | Completed surfaces underneath, where motion reveals them | Body rig or chains |

Every moving part must also lose its baked-in copy in the underlying drawing.
Keeping an iris in the original portrait and sliding a second iris over it would
produce a duplicate. The original stays the identity reference; the exported rig
uses a reconstructed layer stack, checked against that reference at rest.

Clean plates are generated as complete in-context repaints: for example, the same
face with only the irises removed, or the same portrait with one fringe removed.
After registration, only the authored repair region enters the backing surface.
Preserve the original visible foreground whenever possible. Repaint hidden
extensions only where the motion envelope needs them.
Separate visible-source coverage from generated hidden completion and record both
provenances. At soft cutout edges, solve/unmix foreground RGB against the selected
backing surface; retaining already-blended edge RGB and multiplying it by a new
matte would blend that boundary twice and produce a halo.

## Eye stack and gaze

For each eye, the recommended local drawing order is:

```text
clean eye skin
  → closed-eye layer (lid_L/R)
  → open eye with blank whites and lashes (eye_L/R)
  → iris/pupil/highlights (iris_L/R, clipped to eye_L/R)
```

The whites mask is separate lossless coverage, with the same crop as the open-eye
layer. It excludes lashes and surrounding skin. The iris uses its own native box,
a shared eyelid pivot and a measured horizontal/vertical travel in pixels.
`eyeBallX/Y` translate the iris; the eye and clipping coverage deform during
closure. The player uses stencil coverage at rest and soft coverage during gaze
or closure (`puppet.ts` lines 1388–1422, 1483–1540 and 1654–1709).
Positive `eyeBallY` means up. A `clip` reference alone is insufficient: the parent
eye must supply its mask, or the iris is not drawn. Crossing hair needs an actual
foreground layer and completed eye surfaces beneath its intended motion.

Use full-face look-left/right/up/down repaints as anatomical references for safe
travel and any missing iris surface, not as four independently fitted eye sprites.
Review diagonal gaze and half-closed eyes too. Keep highlights inside the iris
for the first implementation: separate `shine_*` layers have additional
chain-dependent movement in this player and need their own review.
Complete any iris surface currently hidden by the lids before allowing travel
that exposes it. A crop of only the currently visible iris is insufficient.
The existing closed-eye replacement patches also contain surrounding skin and
sometimes hair. Extract a reviewed lash boundary and pivot for the moving eye
stack; the player's automatic dark-pixel lash fitting can otherwise follow hair.

## Authoring information the workflow needs

Keep the existing source hash, native canvas and contextual crop. An extended
authoring input should additionally declare:

| Information | Purpose |
|---|---|
| Ordered layers, role/group and native box | Stable identity and drawing order |
| Foreground coverage matte | True cutout alpha, distinct from a replacement support mask |
| Visible-source and hidden-completion coverage | Identify preserved artwork and invented surfaces separately |
| Clean-plate donor, repair region and backing layer | Reconstruct what moving foreground uncovers |
| Clipping relation and clip coverage | Keep iris inside the eye opening |
| Semantic landmarks, pivot and attachment/root points | Preserve source anatomy and connected seams |
| Verified parameter bounds and swept region | Limit motion to completed, reviewed surfaces |
| Shared deformation group and pinned boundaries | Move attached features coherently |
| Pose donor and corresponding visible landmarks | Fit authored deformation without guessing feature placement |
| Reference hashes and extraction/reconstruction reports | Make each result inspectable and reproducible |

This is a proposed example-local input, not fields currently accepted by
[fern.geometry.json](inputs/fern.geometry.json). Its existing broad eye polygons
are replacement regions, not iris mattes or whites clipping masks.
The implemented eye subset lives separately in [fern.gaze.json](inputs/fern.gaze.json);
the head/body ownership subset lives in [fern.puppet.json](inputs/fern.puppet.json).
The broader part, occlusion and authored-keyform contract below is still a design.

## Repaints to head keyforms

The keyform [schema](https://github.com/livliv-dev/player/blob/42a483ae9205e342bda91d063601ce91d4319f84/schema/livliv-keyforms-v1.json)
stores row-major `(dx, dy)` int16 grids in base64, scaled by `units`. Fields address
`head`, `hair`, `torso` or a layer id. The grid is sampled in source canvas pixels.
The file describes total displacement from rest; the player subtracts its own
procedural pose to compute the correction it applies at runtime. Exporting the
correction instead would subtract the procedural pose twice (`puppet.ts` 650–709).
Cover the entire canvas and outer render-lattice vertices with the field grid.
A zero keyform mask retains procedural motion; it does not pin vertices to neutral.

Start with one modest negative and positive pose per axis, for example ±10° yaw
and ±8° pitch, subject to visual review. These are proposed test bounds, not
measured capabilities of the current art. Fit correspondence from persistent
semantic landmarks and reliable visible texture, then regularize a smooth field
with shared attachment constraints. Refuse ambiguous correspondences, folds,
uncompleted disocclusions and unsupported silhouette changes.

Registration differs by donor type:

- Same-pose clean plates and expressions: use unchanged texture outside repair
  regions, as the current recipe does.
- Head-turn/nod donors: remove only camera/framing drift using immutable torso,
  background or explicit frame anchors. Exclude the moving head, hair and neck
  transition from registration. Fitting the whole face back onto neutral would
  erase the deformation being measured.

Important limits of this player's keyforms:

- Public keys are `angleX` and `angleY`; this is not an arbitrary parameter-morph
  system for brows, gaze or mouth shapes.
- Weights add axis contributions rather than bracket a general pose lattice.
  Multiple keys on the same side of an axis would all contribute. Use one key
  on each side initially; optional Hermite interpolation smooths the center.
- X/Y corners add the two corrections and require visual checks. `diag` keys
  default to zero; their weights need internal caller control, not the ordinary
  public parameter API.
- Angle corrections can extrapolate beyond the authored key. The driving app
  must constrain parameters to verified bounds; a package does not enforce them.
- Fixed portrait/gaze packages have `tuning.turn = 0`; the procedural puppet enables
  turn and nod without keyforms. Enabling authored keyforms requires nonzero
  turn/nod tuning; suppress unwanted procedural motion through appropriate rig
  settings rather than suppressing all keyform weights.

See `puppet.ts` 701–708 and 1355–1370. Texture changes and newly revealed geometry
need completed layers or a separately designed multi-view renderer; they cannot
be recovered by stretching neutral pixels.

## Alpha and acceptance

The current fixed replacement recipe preserves original alpha in its offline
recomposed PNG states. The player's alpha-over blending of active patches can
already raise alpha within their support. The offline promise does not transfer
to a moving silhouette. A rig may change alpha inside
its authored swept regions; it must preserve pixels outside those regions.

The earlier generated portrait also has slightly translucent interior pixels
(alpha 251–254). Drawing several such layers with ordinary source-over blending
raises alpha. Admission must either solve the foreground/background mattes or
produce an explicitly named canonical render master with opaque interior regions
and preserved soft outer silhouette. If normalized, retain the original, report
the changed pixels and hashes, and name the canonical master as the neutral
reference. Never silently call it exact reconstruction of the original.

Before adding more motion, verify the actual exported stack at rest in the target
renderer, over black, white and checker backgrounds. Report RGB and alpha error
separately with a declared tolerance; current GPU edge quantization already makes
native byte equality and rendered equality different claims. Check iris travel,
half blinks, unilateral blinks, head/gaze combinations, brow/hair occlusion, jaw
attachment and hair at both motion extremes. Test interpolated states and
forward/reverse sweeps for folds, ghosts, seams and identity drift. Numbers support
these checks; they do not establish naturalness.

The first useful extension is a convincing two-eye gaze/blink stack, brows and one
isolated front-hair lock over a repaired forehead. Add small head-turn/nod fields
after neutral decomposition and those independent motions survive visual review;
then add torso breathing and more hair. Each stage retains inspectable full-face
donors, mask overlays, backing plates, native layers and motion previews.

## Related rigging conventions

Clipping a pupil by eye whites, deforming related pieces together, and
interpolating keyed shapes are established 2D puppet techniques. Live2D documents
[eye clipping](https://docs.live2d.com/en/cubism-editor-manual/clipping-mask/),
[shared warp deformers](https://docs.live2d.com/en/cubism-editor-manual/making-and-placement-of-warp-deformer/),
[parameter interpolation](https://docs.live2d.com/en/cubism-editor-manual/parameter/)
and [explicit diagonal corrections](https://docs.live2d.com/en/cubism-editor-manual/synthesize-corners/).
These are useful design references, not file compatibility. LivLiv v4 is a
project-specific documented format; Cubism exports its own
[model files](https://docs.live2d.com/en/cubism-editor-manual/export-moc3-motion3-files/).
The example should export LivLiv's published contract without treating its
keyforms as a general interchange standard.
