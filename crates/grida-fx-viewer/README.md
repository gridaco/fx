# FX viewer host

This crate hosts the bundled viewer for one explicitly selected existing run folder
or one schema-validated expanded graph supplied by the caller.
It is read-only: no workflow imports, provider credentials, execution, cache discovery,
project registry, daemon, snapshot export, or background launch. The CLI owns opening a
browser and its existing interruption handling. The listener is always `127.0.0.1`;
port `0` requests an unused port from the operating system.

The Vite production build must exist at `web/viewer/dist` before compiling. The same
assets are embedded in debug and release builds; missing assets are a build error.

## Bootstrap read API

- `GET /api/view` returns either the run projection below or the original
  [`fx-graph-v1`](../../spec/schemas/fx-graph-v1.schema.json) document. The response's
  `kind` distinguishes them; there is no additional envelope. Static plans remain
  immutable in memory and retain every instance, planner state, pending repeat,
  unresolved value, estimate and problem exactly as supplied. This host performs
  no planning or file/cache resolution for a plan.
- `GET /api/run` returns `fx-viewer-run-v1`, specified in
  [the schema](../../spec/schemas/fx-viewer-run-v1.schema.json). The browser may fetch it
  again to refresh. It combines the initial graph and readable event prefix, including
  instances discovered during execution. It leaves a torn final event line unchanged.
  This route returns 404 in plan mode.
- `GET` and `HEAD /api/artifacts/<digest>` serve only recorded, confined placed files
  whose size and SHA-256 match the record. They support a single HTTP byte range and
  digest ETags. HTML, SVG and unsupported file kinds are attachments with an inert MIME
  type and a sandbox policy. Files cannot execute with the viewer origin's privileges.
  Artifact routes return 404 in plan mode, even when the graph names file digests.
- Host, Origin and browser fetch-site checks restrict access to this loopback origin.
  There is no CORS allowance for other websites.

Inputs and outputs retain their FX source encoding. Event files are
`{"file":{"digest","kind","name","size"}}`; lists and collections retain their
wrapper. A plan's parameters may use its shown-value encoding until a start event
provides the execution values. `charged_usd` is the terminal invocation's recorded
charge, null before a terminal event; it is not an inferred provider invoice.

Optional node `ports` and `bindings` retain the engine's named connection metadata,
with `needs` and `judges` identifying control dependencies. Initial declarations come
from `plan.json`'s type entries. Later `node_started` metadata is authoritative,
including empty bindings and types/instances discovered during execution. Records
for instances blocked or failed before starting carry the same known metadata on
their skip/failure event without implying execution. Records
that predate these fields remain readable; the projection never imports author code
or derives source ports from matching values or file digests.

This is an inspection response, not a portable publication format. The bootstrap
resolves placed outputs in the selected run only. Cache-only inputs, absent files,
and files lost through a human-readable name collision are marked unavailable.
It never scans a cache or substitutes different bytes. The run response verifies
the files again on refresh; the initial UI therefore uses manual refresh. Artifact
GET/HEAD/range requests build only the reference inventory and verify their selected
digest, without opening unrelated artifacts.

The crate's tests synthesize records and bytes without providers, then exercise the
schema, event projection, read-only tail behavior, confinement, artifact integrity,
Host/Origin refusal, MIME isolation, HEAD, and byte ranges.
