# Showcase maintenance

This is private tooling for FX's website, not a public workflow packaging API.
The reusable recorded-run contract is [`fx-run-export-v1`](../../spec/exports.md).
See the [first-time setup and CDN policy](../README.md#cdn-assets) for browser delivery.

## Content boundaries

```text
content/showcases/lock.json    Exact release and recording pins
content/showcases/manifests/   Frozen small demo documents and release inventory
content/showcases/recordings/  Frozen public export manifests
src/lib/showcases/             Website schemas, parsers, resolver and JSON loader
scripts/showcases/             Approval checks, preparation, receipts and adapters
.showcases/delivery/           Ignored immutable media/data staging
```

Releases use `showcases/<manifest-sha256>/<slug>/...`. The release manifest lists
selected paths, SHA-256 digests, sizes, MIME types and native-demo adapter kinds.
Its internal paths omit the release digest to avoid self-reference. Recorded
canvases use `runs/<export-manifest-sha256>/...` and may select multiple labeled
runs. Generation and finishing graphs stay distinct.

Overview presents native website demos and authored usage content. Workflow uses
the shared FX export reader and the installed viewer's canvas. These records are
frozen, without polling or local lifecycle/file actions. Omissions remain
explicit; the inspector distinguishes withheld values from empty ones. The
Overview's demo manifest is a website projection, not the recording's run record.
Custom generation workflows and nodes shown here are not installed by FX.

Every showcase's complete authoring project lives under `examples/`, including its
private node helpers, prompts, schemas and authored settings:

| Page | Maintained source |
|---|---|
| Product promo clip | [`examples/product-promo/`](../../examples/product-promo/), `workflow.yaml` |
| Character performance | [`examples/character-performance/`](../../examples/character-performance/), `cast.yaml`; single-character utility in `workflow.yaml` |
| Parallax | [`examples/looping-parallax/`](../../examples/looping-parallax/), `workflows/meadow.yaml` |
| World | [`examples/world/`](../../examples/world/), `workflow.yaml` |
| Movie sprite | [`examples/movie-sprite/`](../../examples/movie-sprite/), `workflow.yaml` |
| Pet | [`examples/pet/`](../../examples/pet/), `workflows/pet.yaml`; cover from `workflows/reel.yaml` |
| Portrait motion | [`examples/portrait-motion/`](../../examples/portrait-motion/), `workflow.yaml` |
| 3D character | [`examples/character-3d/`](../../examples/character-3d/), `workflow.yaml`; independent `source-artwork/` |
| Terrain | [`examples/terrain/`](../../examples/terrain/), `workflow.py:build` |
| UI kit | [`examples/ui-kit/`](../../examples/ui-kit/), `workflow.py:build` |

`src/lib/showcases/sources.ts` binds each page's source link and CLI commands to
these projects; tests check their targets and inputs exist. Generated inputs stay
on the CDN, with checksum-verified download metadata in each consuming example.
[`examples/fetch_showcase_inputs.py`](../../examples/fetch_showcase_inputs.py) supports
an explicit `--base-url` when identical pinned bytes move to another host. Parallax
retains its single-input download command. Neither downloader calls a provider.

Examples are maintained and checked against the current engine; website recordings
remain immutable evidence of the particular saved runs. Editing source does not silently
replace a recording or regenerate artwork. Fresh live runs use the caller's provider
account and can produce different results. Movie finishing can replay the recorded paid
take locally, while its source-bound face pin must not be reused for new footage.

Before/after comparisons are authored in `src/lib/showcases/comparisons.ts`.
Each pair selects exact node outputs (or keyed collection entries) from a frozen
recording, then resolves its included image files directly on the configured CDN.
Missing recordings, outputs or included-image references fail the website build. Keep both images on
the same registered canvas; do not stretch different layouts into a comparison.
The reusable image slider owns only presentation and needs no new content export.
For a repeat seam, its wrap view places the source's tail beside its head and
crops both images identically. Ticks locate the join without covering it; dashed
bounds show this run's fixed repaint band. This shows the assembled repair, not
an unrecorded provider conditioning image or an adaptive cut path.

The main parallax demo's **Show joins** toggle draws red repeat joins and repaint
boundaries over the moving layers. Its annotations live in
`src/lib/showcases/parallax-guides.ts` and are bound to the frozen release,
recording and selected image digests at build time. Loaded native dimensions are
also checked before enabling the overlay. The current run repainted only the far
layer; the mirrored middle and already-looping foreground have repeat markers
only. A new run requires reviewing these authored annotations, without adding an
engine or export feature just for the website.

The combined video-example release retains all five parallax files and the exact
`meadow-01` recording from the previous release. Its guide release pin was refreshed
after comparing all existing demo inventories and recording selections; the geometry
and digest checks remain in place. An unrelated release addition still requires
this explicit review before changing the guide pin.

## Prepare reviewed content

### Reference-driven video examples

Product promo and character performance share the website-owned `video-demo-v1`
projection. This presents one exact finished clip, its static poster, the supplied
visual inputs, recorded stage outputs and scoped limitations. It does not contain
workflow execution logic. The Workflow tab uses the same frozen FX run export as
the other examples.

The product page's selected command runs `workflow.yaml` with
`replay-segments.yaml` at a zero-dollar ceiling. It reuses the original source and
brief with a saved edit containing explicit chronological source intervals and
caller-revised camera beat times. The intervals omit processing waits and retime
the retained footage independently; the report binds source and output intervals.
`showcase.yaml` remains the fresh model-planning path, and `replay.yaml` retains the
earlier uniformly retimed result. Keep model proposals and caller revisions clear
in the stage descriptions.

Each complete reviewed bundle contains `manifest.json`, `cover.webp`,
`assets/final.mp4`, `assets/poster.webp`, `assets/report.json` and every resource
named by its inputs and stages. The product page's prompt attachment is
`assets/source.mp4`; the character page's are `assets/character.png` (Riko) and
`assets/nami.png` (Nami). Keep these
files identical to their selected source artifacts. A motion comparison may use
an independently reviewed still or comparison sheet when the original footage
is acquired separately. Its label and description must explain what it shows.

The character page uses `cast.yaml` with `inputs/shared-cast.yaml`: two instances of the
single-character `workflow.yaml`, followed by the free `reel.yaml` composition.
Riko and Nami each use a separately prepared identity image and the same four-second
silent driving interval at 24 fps. Preparation and reel composition make no provider
calls. Each child workflow generates and independently reviews its own take, and
the reel accepts only clips whose digests match their accepted reports. Finishing
decodes every native frame and applies a conservative plain-studio border check;
an independent review compares eight paired samples per take. Clear borders are
a pixel-based framing proxy and do not certify continuous anatomy or every
transition.

The optional `first_region` and `second_region` inputs are normalized
`[x, y, width, height]` rectangles. Their default preserves the whole frame. The
canonical reel uses one author-selected common region to remove empty upper background
and hold the fitted stage fixed across its identity cut. Its foreground bounds were
measured across all 96 native frames per take, with a requested 48-pixel margin
constrained by the native image boundary. This region belongs to the exact accepted
clips; measure every frame again when selecting framing for fresh takes. This is
an explicit framing decision rather than automatic content
cropping. Local composition aspect-fits each complete chosen region into a silent,
four-second 1080 × 1920 presentation at 30 fps; the supplied performance clips
remain native 24 fps. With `mode: shared_timeline` and `switch_at: 2`, it uses
Riko's first two seconds and Nami's final two seconds. This mode requires matching
driving-motion digests and equal source timeline lengths. `poster_at: 3.2` selects
an actual output frame. Its report records the chosen regions, exact output-frame
intervals and inherited child review scope. The cut and poster have a separate
bounded visual review; they are an editorial identity change, not a seamless morph
or loop. Fresh takes require a new cut and poster review. The default sequence mode
and `inputs/cast.yaml` retain the eight-second comparison of both complete takes,
included as an alternate stage output.

The source commands download FX's character artwork, acquire the selected stock
clip from its original source, then prepare both identity plates and the shared
motion interval locally. Keep the raw and transcoded stock MP4s out of public
bundles and recorded output placements. The page may show composed comparison
evidence with clear source attribution; its reusable inputs remain distinct from
the larger cast presentation. New character identities and driving footage need
their own review.

The manifest shape is:

```json
{
  "kind": "video-demo-v1",
  "title": "A bounded performance",
  "summary": "One finished clip from supplied visual inputs.",
  "result": {
    "file": "assets/final.mp4",
    "poster": "assets/poster.webp",
    "media_type": "video/mp4",
    "width": 1080,
    "height": 1920,
    "duration_seconds": 4
  },
  "inputs": [{
    "id": "identity",
    "label": "Character artwork",
    "role": "identity",
    "file": "assets/character.png",
    "media_type": "image/png",
    "description": "The supplied identity image."
  }],
  "stages": [{
    "id": "finish",
    "label": "Finish",
    "description": "Export the selected take locally.",
    "outputs": [{
      "label": "Delivery report",
      "file": "assets/report.json",
      "media_type": "application/json"
    }]
  }],
  "limitations": ["A single bounded performance."]
}
```

Input roles are `source`, `identity`, `motion` and `direction`. Inputs and stage
outputs can be images, MP4/WebM clips, JSON or plain text/Markdown downloads.
Every video needs its own static poster. Relative paths, media declarations,
positive dimensions and duration, unique input/stage IDs and complete resources
are validated before rendering. Runtime manifest loading verifies the pinned
hash and size before parsing. Native controls retain authored result audio;
reference clips begin muted. Clips load when visible, pause offscreen and keep
their poster for reduced motion until the visitor deliberately starts playback.

After independent content review, export the selected saved FX runs. Their root
outputs must include `video` and `poster`, byte-identical to the bundle's final
clip and poster. `cover.webp` is an exact copy of that WebP poster. Create an
ignored local preparation document with the two reviewed selections:

```json
{
  "kind": "fx-video-showcases-preparation-v1",
  "entries": [
    {
      "slug": "product-promo",
      "bundle": "/absolute/path/to/product-promo-bundle",
      "recording": {
        "id": "product-promo-01",
        "label": "Selected product reveal",
        "source": "/absolute/path/to/product-promo-export"
      }
    },
    {
      "slug": "character-performance",
      "bundle": "/absolute/path/to/character-performance-bundle",
      "recording": {
        "id": "character-performance-01",
        "label": "Selected character performance",
        "source": "/absolute/path/to/character-performance-export"
      }
    }
  ]
}
```

```sh
bun --no-env-file run --cwd www prepare:video \
  /absolute/path/to/ignored-video-preparation.json
bun --no-env-file run --cwd www verify:showcases --media --recordings
bun --no-env-file run check:www
bun --no-env-file run test:www
bun --no-env-file run build:www
```

`prepare:video` validates the exact result/output bindings, stages both demos in
one immutable release, appends their selected recordings, and updates the frozen
metadata and content lock together. The existing release bytes must be present
in `.showcases/delivery/`. Existing demos, recordings and their order are
preserved. A later run may update one already-added video example. The tool also
generates `content/showcases/video-recordings.ts` with explicit imports of the
selected frozen recordings; covers resolve their named root outputs at build
time. It never executes a workflow, infers semantic approval or uploads media.
Run the media verification and direct-CDN browser checks before publishing the
new immutable `showcases/<sha256>/` and `runs/<sha256>/` paths.

### Earlier campaign preparation

Preparation accepts an independently reviewed campaign containing `publish/`,
`publish-inventory.json` and its exact `review-approval.json`, plus exported saved
runs. The first curated release's run names are mapped in `prepare.ts`.

`campaign.ts` verifies approval bound to the exact inventory, its complete bundle
set, sizes and tree digests without copying or publishing old HTML bundles.
Preparation selects required media and positively projects website demo JSON.
It does not execute workflows or import node code. Historical approval flags,
review screenshots and old page shells do not become public assets automatically.
Review additional intermediate media selected by generic run exports separately:
the campaign approval covers only its explicitly inventoried original bundles.

```sh
# From the FX repository root; export saved runs separately with grida-fx export.
bun --no-env-file run --cwd www prepare:showcases \
  /absolute/path/to/reviewed-campaign /absolute/path/to/run-exports
bun --no-env-file run --cwd www verify:showcases --media --recordings
bun --no-env-file run build:www
PORT=3101 bun --no-env-file run preview:www
```

Preparation creates `.showcases/delivery/`, updates the small frozen metadata and
content lock, and copies local preview bytes into ignored `public/showcases/`
and `public/runs/`. It refuses different existing immutable content. Large media,
raw runs, approvals, staging directories and verification receipts stay outside Git.
Small frozen documents let source checks/builds run without media or a CDN.

### Add a recorded preview

A workflow can keep its reusable assets as named outputs and optionally render
another output for a cover. Select the recording and its image or video/poster outputs in
`src/lib/showcases/covers.ts`; missing or unavailable outputs fail the build.
The shared cover displays native CDN images or videos, defers video loading until
visible, pauses offscreen, and uses the poster for reduced motion. It does not
reimplement each workflow's renderer in the index.

The parallax recording `meadow-forward-01` adds a local FFmpeg rendering step to
the saved layer workflow. Its predecessor results were reused from the original
cache, without provider calls. The 16-second video scrolls forward continuously:
the far, middle and foreground layers complete 1, 1 and 3 whole repeats. This
retimes the preview to close its temporal loop; original images and runtime
scroll factors remain unchanged. At the 960-pixel preview width, the layer speeds
are 60, 120 and 180 pixels per second. The recording retains the original factors
and reports its actual preview timing separately. Consumers can disable the
`render_preview` input and use the layer images alone. Earlier recordings remain
available.

The animated terrain cover uses the free `render-cover-video.yaml` workflow in
[`examples/terrain/`](../../examples/terrain/). It reads the exact published atlas
and maintains a real binary tile grid in Python. Cells are removed top to bottom
and rebuilt bottom to top; each frame selects tiles from its current neighbors
using the canonical 47-mask lookup. Ground occupancy stays fixed while its tile
edges change where digits disconnect and reconnect. The ground continues past
the frame's left, right and bottom edges. The complete “47” holds at
both ends of the loop. Rendering needs no browser or provider; the website only
plays the MP4 with its WebP poster. The still layout records the completed grid.
Each changing silhouette is composed from its current grid state. Its `terrain-cover-04`
recording is separate from the original generation; the cover resolver selects
its named `cover_video` and `cover` outputs. The still-only `render-cover.yaml`
does not need FFmpeg. The original atlas, demo and approved media release stay
unchanged.

The portrait cover selects the native animated WebP from `nami-face-01`'s
`animation` output. It fills the card with a centered landscape crop.
The published static cover is its native `picture` reduced-motion fallback.
The image loads lazily; unlike videos, an animated WebP cannot pause offscreen.
This selection reuses existing reviewed media and needs no new generation.

The pet cover is a separate free run of the pet example's `reel` workflow on the
recorded sheet and `sprite.json`. It plays the states on the previews' backdrop and encodes a
960x600 MP4 with its first frame as a WebP poster. `coral-reel-01` selects its `video` and
`poster` outputs. The jump is left out because its row is drawn smaller to fit the jump in a
cell; the page's player shows every row as recorded. The recorded run, `coral-01`, keeps its
generated strips and pose counts. Its reference picture and `sprite.json` are in the demo
bundle, because exports carry neither input pictures nor JSON.

After separately reviewing a new exported recording and any added media, append
it without rebuilding or replacing the approved media release:

```sh
bun --no-env-file run --cwd www prepare:recording \
  parallax meadow-forward-01 "Layers and forward-loop preview" \
  /absolute/path/to/exported-run --primary
```

This single-writer tool verifies export hashes and portability, stages immutable
bytes, then atomically updates the content lock. A recording ID cannot be reused
for different evidence. `--primary` selects the default Workflow recording while
retaining earlier recordings. Existing comparison and annotation bindings remain
explicitly pinned to their original recording. Add the frozen document to the
cover resolver when selecting new root outputs. Run the media and build checks,
then explicitly publish the new immutable `runs/<sha256>/` namespace. Raw runs,
large media and review receipts remain outside Git.

To replace one reviewed interactive demo, supply a complete website-owned bundle
containing `manifest.json`, `cover.webp`, its referenced resources and page
downloads:

```sh
bun --no-env-file run --cwd www prepare:demo \
  portrait-motion /absolute/path/to/reviewed-demo-bundle
```

This single-writer tool needs the current release's bytes in
`www/.showcases/delivery`. It verifies those bytes and the frozen metadata,
validates the new bundle against the current readers and page downloads, then stages a new immutable
release and updates its frozen metadata and content lock. All other demos and
every recording retain their content and order, including the selected default
Workflow recording. It does not append a run or publish assets. Append a new
recording separately when needed, then run the media and build checks and
explicitly publish the new `showcases/<sha256>/` namespace before deployment.
Earlier immutable release directories remain available for rollback; generated
artwork and raw run data stay outside Git. Page filenames may change with a demo:
the predecessor still passes its frozen metadata and media integrity checks, while
new page references must all exist in the candidate before the lock can change.

### Shared character across examples

The transparent sprite loop, facial animation patches and rigged 3D character
showcases form one character family: **Nami**. Use the same approved character
identity and retain each representation's source lineage. The rig-ready SD
adaptation may simplify hair and clothing for deformation; a different character
must not become its cover, source attachment or demo fallback.

The facial example may use a separate high-resolution crop of Nami. When facial
patches are demonstrated over the finished sprite, they must share its registered
2D source: use that sprite's exact canonical first frame and regenerate or refit patches against it. Patches made
for a different image are not interchangeable because the character name matches.
Retain the source illustration, finished canonical frame and derived rig as
distinct artifacts rather than replacing their provenance with a common label.

All three demos use independently reviewed Nami material. The sprite
uses a six-second 1080p landscape take of the original adult rabbit-hood
illustration. Its source-bound trim keeps frames [12, 132), producing four seconds
at the native 30 fps without a face hold. Generated blinks, small facial changes
and foot adjustments remain visible. Independent scoped review passed, and the
owner approved its publication. The quiet sprite and its earlier recordings
remain available: `nami-sprite-02` selects [12, 228),
plays that 7.2-second interval over eight seconds, and holds source-bound face,
adjacent head and finger regions.
The facial patches use a separate high-resolution registered Nami portrait.
The 3D example uses a hood-free, short-haired SD adaptation, followed by measured
surface-preserving finishing and an explicit lower-body weight correction.
Its runtime qualification remains pending. Earlier character recordings remain
available as historical evidence, without becoming the current demos.

The interactive 3D exhibit displays a flat, texture-only preview. The browser
fetches each original GLB directly from the configured asset base, then adds
`KHR_materials_unlit` to a temporary in-memory copy. All binary chunks remain
byte-identical, including geometry, skinning, animation and embedded images.
The bundled viewer skips lighting and tone mapping for these unlit materials;
painted highlights already in a texture remain. Temporary blob URLs are cached
per stage and revoked on disposal. The cheer plays at two-thirds speed, matching
the rendered cover's timing; other diagnostic clips keep their native speed.
Recorded GLBs, downloads, source textures and their hashes remain unchanged.
The animated cover uses the same flat texture-only style, rendered headlessly from
the unchanged corrected rig. Its six-second, 24 fps cheer keeps the original fixed
camera and framing. `nami-rig-cover-02` selects the new video and still fallback;
the earlier lit cover remains a historical recording. This free cover-only run
does not replace the primary rig recording or require paid generation.

The covers use actual sprite idle, facial patch animation and a headlessly
rendered rig cheer clip, reviewed together for identity, framing and motion.
Keep reduced-motion posters, offscreen video pausing and direct CDN delivery.
A new run requires updating the demo, primary recording, source and download
pins, and cover together, then independently reviewing those exact bytes.
Cover polishing is website work and free local rendering, not a reason to rerun
paid generation.

## Validate clients and content

Every website build checks the catalog against current readers. This is offline
and does not regenerate content. A successful build binds the exact static output,
content lock and configured delivery base; a lock change during build is refused.
Explicit `verify:showcases --media` checks all selected disk bytes. Runtime JSON
loaders verify pinned hashes, supported kinds, references and geometry before
rendering. Failure is visible instead of becoming an empty or changed canvas.

```sh
# Use the same base that was supplied to the successful website build.
bun --no-env-file run --cwd www candidate:showcases \
  https://assets.example.com/root
```

This also needs the prepared delivery bytes on disk. It refuses stale output or
a different base and writes `.showcases/candidate.json` with browser acceptance
pending. Inspect that exact output following the README's direct-CDN checks;
qualify the receipt with explicit review evidence before deployment. A new build
invalidates earlier acceptance. Preparation, receipts and object upload do not
route or publish the production Grida website.

## Publish to an object host

Showcase recordings are FX exports: no credentials, call records or local paths. Their
media keep what providers embedded, such as content credentials and generation
labels, so content review covers that too.

The prepared delivery tree is CDN-agnostic. Copy it to a public HTTPS host,
preserving paths and bytes. Validate public SHA-256 bytes, MIME, CORS, HEAD and
video ranges; a successful PUT or an S3 ETag alone is insufficient.

An optional Tigris adapter uses the already authenticated local CLI, invoked from
the system temporary directory rather than a project dotenv scope:

```sh
# Explicit publication only, after content review and public bucket setup.
bun --no-env-file run --cwd www publish:tigris \
  my-public-bucket https://my-public-object-host.example --configure-cors
```

The adapter requires an existing public bucket and an explicit HTTPS base. A
path in the base is the bucket folder it publishes under:
`https://my-public-object-host.example/showcase` writes `showcase/showcases/…` and
`showcase/runs/…`. Access and CORS belong to the whole bucket, so share one only
with other public, read-only media. The adapter checks the local closure, refuses
conflicting remote bytes, uploads media before manifests and verifies public
delivery. `--configure-cors` explicitly replaces the bucket's existing CORS rules.
It neither creates buckets nor changes DNS.
Publication writes `.showcases/delivery-receipt.json` with the verified inventory.
The adapter assumes one publisher owns the immutable namespace; distributed
publisher locks are outside this tooling's scope.

## Change and roll back

- Client-only changes validate the existing lock and frozen documents; no rerun.
- Packaging corrections export saved evidence again and explicitly update pins.
- A breaking required meaning gets a new format kind. Retain old readers while
  the active catalog needs them, or explicitly repackage affected recordings.
  Never migrate or rewrite content on read.
- New generated results require a deliberate run, review and content release.
- CDN migration copies identical bytes, changes `FX_SHOWCASE_ASSET_BASE` and the
  examples' `delivery_base` pins, and rebuilds the website. Workflow reruns are
  unnecessary. Only the closure the current lock and the examples' pins reference
  needs to move: earlier website builds keep reading the old host, so retire it
  only once rolling back to them is no longer needed.

Keep old releases while links refer to them. Rollback deploys the previously
checked website output and content lock together; no mutable `latest` object is
needed. CI and website builds never regenerate workflows or publish content.

### Local preview and content promotion

Local preview media may be copied from the verified delivery closure into ignored
`public/showcases/` and `public/runs/`, then built with
`FX_SHOWCASE_ASSET_BASE=/fx`. Do not deploy this local output. Promoting new
material requires publishing the reviewed immutable closure, merging its verified
input pin, moving local preview media out of `public/`, and rebuilding
with the CDN base as described above. The current Nami motion take is pinned in
`examples/movie-sprite/showcase-inputs.json` for free finishing replay. Earlier
immutable recordings remain available. Local preparation never replaces an
existing CDN object.
