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
| Parallax | [`examples/looping-parallax/`](../../examples/looping-parallax/), `workflows/meadow.yaml` |
| World | [`examples/world/`](../../examples/world/), `workflow.yaml` |
| Movie sprite | [`examples/movie-sprite/`](../../examples/movie-sprite/), `workflow.yaml` |
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

## Prepare reviewed content

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
validates the new bundle against the current readers, then stages a new immutable
release and updates its frozen metadata and content lock. All other demos and
every recording retain their content and order, including the selected default
Workflow recording. It does not append a run or publish assets. Append a new
recording separately when needed, then run the media and build checks and
explicitly publish the new `showcases/<sha256>/` namespace before deployment.
Earlier immutable release directories remain available for rollback; generated
artwork and raw run data stay outside Git.

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

Current migration status: the facial showcase uses Nami; the sprite and 3D
showcases still use their earlier characters. Nami's sprite replacement is
blocked by the current video provider, and its corrected hood-free rig remains
under visual review. Do not mark the family migration complete or replace only
a cover while its page still demonstrates another character.

Once the replacements are reviewed, update each demo, primary Workflow recording,
source attachments, download metadata and cover together. Review the three covers
as a set: consistent neutral backing and visual weight, deliberate positioning,
and gentle motion that demonstrates the output. Use the sprite's actual idle,
the face's actual patch animation, and a rendered rig clip; avoid decorative
floating or scaling. Fill the existing cover frame and compose the rendered
media to preserve the subject. Keep reduced-motion posters, offscreen video
pausing and direct CDN delivery. Cover polishing is website work and free local
rendering, not a reason to rerun paid generation.

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

The adapter requires an existing public bucket and an explicit HTTPS base. It
checks the local closure, refuses conflicting remote bytes, uploads media before
manifests and verifies public delivery. `--configure-cors` explicitly replaces the
bucket's existing CORS rules. It neither creates buckets nor changes DNS.
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
- CDN migration copies identical bytes, changes `FX_SHOWCASE_ASSET_BASE` and
  rebuilds the website. Workflow reruns are unnecessary.

Keep old releases while links refer to them. Rollback deploys the previously
checked website output and content lock together; no mutable `latest` object is
needed. CI and website builds never regenerate workflows or publish content.
