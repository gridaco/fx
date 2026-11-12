# Recorded 3D artifact inspection

The read-only viewer can preview a recorded self-contained GLB 2.0 artifact on
its producing card and inspect it there or in step and run details. This is artifact presentation, separate from the
workflow-authored video selector in [previews.md](previews.md). It adds no
workflow setting, record field, execution operation or identity algorithm.

## Supported scope

Preview eligibility comes from the recorded file kind `model/gltf-binary`, not
its filename or the node that produced it. A supported GLB embeds its geometry,
images, material data, skinning and animation in the file, including permitted
data URIs. The browser renderer displays the original material settings under
neutral lighting. It does not rewrite all materials as unlit, repair the model,
convert the format or claim equivalence to every other renderer.

A mesh can be inspected without a rig or clips. A rig does not establish that
animation exists. When the artifact contains clips and renderable geometry, the
viewer discovers the clips from that artifact. Playback starts paused. An icon
button toggles play/pause; a compact native clip selector appears only when the
artifact has more than one clip. A model without clips has no playback controls.
The viewer does not invent motion or borrow clips from a different output.

FBX, JSON glTF, standalone motion and skeleton-only artifacts are outside this
first preview scope. The viewer does not infer a target character, retarget
animation or associate files by names, equal digests or sibling output ports.
An unsupported required glTF extension produces a preview limitation rather
than a silently incomplete model. Browser graphics support remains a separate
precondition. The implementation pins its required-extension support in the
[model validator](../web/fx-web/src/model.ts); upgrading the renderer requires
reviewing that allowlist and its validation tests.

## Resource boundary

The viewer reads only the selected artifact URL already supplied by its trusted
run or export reader. It checks the file's recorded size and SHA-256 digest
before handing bytes to the renderer. Files larger than 128 MiB are refused
before reading; reads are bounded independently of a server's reported length.
GLB headers and chunk boundaries are validated before model parsing. A preview
that has not loaded within 30 seconds stops with an explicit timeout; this
includes artifact loading and renderer startup.

GLB is a container, not a guarantee that resources are embedded. External
buffer and image references, including relative paths and HTTP(S) URLs, are
refused before the renderer receives the model. Unsupported required extensions
are refused before their resource behavior can be used. No resource URL found
inside an artifact is granted authority to access the project, service or
network. Inert base64 data URIs for supported buffers and texture formats remain
self-contained and are supported by the installed viewer's resource policy.
Selected artifact bytes and downloads remain unchanged.

The renderer is a separate lazy-loaded local asset. A visible GLB card or an opened
model in details loads it; pages without visible model previews do not. The renderer
includes its Meshopt decoder; both load on the first visible model preview.
Draco and KTX2 decoder resources are bundled with the viewer and fetched only
when a model needs those formats.
Renderer scripts, decoders and neutral lighting do not depend on a CDN. The
installed viewer supports this scope without external network access.

## Presentation and lifetime

### Canvas cards

A recorded card automatically previews its first GLB output when it has no available
image or video output. Authored video preview selection keeps its existing priority.
The preview belongs to that exact instance and take and comes from its model bytes,
never its reference image or another step's output. No poster, author code, provider
call or workflow setting is required.

The browser frames the model under neutral lighting and captures a static 16:9
thumbnail lazily when its card becomes visible and uncovered. Thumbnail generation
is bounded and serialized; a bounded browser-local cache reuses it by model digest
and rendering settings. Temporary renderers are released after capture. These are
presentation copies, not new artifacts or changes to the recording or call cache.
The cache retains at most 64 thumbnails. If all entries are still needed by visible
cards, further cards offer **Activate to preview this model** and retain their
inspection action; capture resumes automatically when panning frees a cache slot.
Failures remain marked for the current graph instead of automatically retrying in a loop.

An explicit **Inspect 3D** action activates one interactive model over its card's
frame without opening details or changing selection or execution picks. Orbit, pan,
zoom and embedded animation use the same minimal controls as details. The overlay
follows the card as the workflow camera moves. Close or Escape restores the still
and keyboard focus to the activating control while it remains available and
uncovered, otherwise to the canvas. Covered or offscreen controls leave the tab order. Covered
or removed cards cannot keep an active preview. Video and model interaction are
mutually exclusive within one canvas. Ordinary polling retains an unchanged active model.

Unavailable or omitted model bytes never start capture or interaction, including
when a thumbnail of the same digest was previously cached. Browser preview failures
stay distinct from node failure and source availability; original files remain
accessible from details when available. Missing or unsupported previews do not
change card geometry. Model output declarations reserve the preview area before
outputs arrive, just as image and video declarations do.

### Details

The preview is an inline neutral model frame in the existing details panel.
The model supports orbit, pan and zoom through the renderer's gestures, with
instructions available as a tooltip. Its existing surrounding details retain
the recorded run, producing instance, output port and take; the preview's
accessible name can include that context without repeating it as a caption.

A static model presents the frame and a quiet original-filename download link.
An animated model adds only the play/pause icon and, for multiple clips, the
native selector. Loading and failure feedback appear when needed; a ready
preview has no persistent instruction or no-clips message. Inspection stays
inline without a dialog or modal. Camera movement within a model must not pan
the workflow canvas. Ordinary run polling must not reset an unchanged model's
camera or playback state.

The square preview frame and one-row footer reserve their final dimensions
before artifact fetching, renderer loading and custom-element initialization.
Loading feedback overlays the frame; inserting the renderer and revealing
animation controls must not move the footer, original-file link or surrounding
details. The original-file link also retains its space for long filenames.

Switching artifacts or closing details cancels obsolete loads and releases
preview URLs and instance-owned renderer resources. Renderer model caching is
disabled, so repeated inspection does not accumulate retained models. A late
completion from an older selection must not replace the current selection. The viewer uses no unbounded collection
of active model renderers or animation loops for canvas cards.

Camera, playback and clip selection are local presentation state. They do not
change the workflow's execution pick, output bindings, judge verdict, recording,
identity, cache contents or provider accounting. Inspection never starts or
resumes generation. Cached and old recorded artifacts follow the same path as
new results. Python and JavaScript workflows use the same recorded file contract;
neither SDK needs a new operation. The viewer page API in [viewer.md](viewer.md)
continues to expose context and graph presentation, not model playback control.

## Failure and original files

Preview availability is distinct from artifact availability and node execution
state. The viewer explains unsupported format, missing external resources,
unsupported required extensions, invalid bytes, browser graphics failure,
resource limits and unavailable artifact bytes as preview limitations. It does
not mark a successful producing node as failed because a preview cannot load.

Every available model keeps an original-file download, including an unsupported
format or a failed preview. Missing or unverifiable bytes remain unavailable;
the browser does not recover them from a cache or rerun a node.

Portable exports already allow `model/gltf-binary` in [exports.md](exports.md).
This inspector changes neither inclusion policy nor disclosure. An omitted
artifact cannot be previewed; an included model does not authorize discovery of
additional files. A static consumer bundles the renderer resources alongside
its viewer and resolves the selected artifact through the trusted export reader.
