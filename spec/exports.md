# Portable run exports

The public, read-only inspection format is `fx-run-export-v1`. The directory
contains `manifest.json` and selected files under `artifacts/`. It is independent
of the service, website catalog, CDN and live observation protocol. It contains no
executable workflow, local cursor, credentials or source location. An export is
not an execution backup or resumable run folder.

## 1. Capture and identity

`grida-fx export RUN --out DIR [--include-values] [--include-diagnostics] [--json]`
reads exactly the selected saved run without importing author code, opening a
cache, loading a project or credentials, or executing providers. It opens the
existing, non-symlink `run.lock` without creating it, acquires its exclusive OS
lock without waiting, and verifies the opened lock is still the folder's lock.
A missing lock is `unavailable`; a held lock is `busy`. The lock remains held
through capture and copying. The source is never modified or repaired.

The observation reader captures the plan and complete event prefix. The latest
invocation must start with `run_started` and end with `run_finished` or
`run_cancelled`, and no unreadable or incomplete event tail may follow it. Refuse
unfinished, crashed, malformed and actively resumed records. Failed terminal
runs are valid inspection sources. A terminal recording never appears live.

`source.plan_sha256` hashes the exact saved UTF-8 `plan.json` bytes.
`source.events_sha256` hashes the RFC 8785 canonical JSON encoding of the captured
**event array**, with no trailing newline. `source.event_count` is its length.
Fingerprints identify evidence, not signatures or proof of execution. They
change neither existing plan identity nor cache identity. The manifest is written
as FX's UTF-8 JSON file form (`identity.md` §5); `manifest_sha256` hashes its exact
bytes. Consumers pin that digest externally; it is not inside the manifest.

## 2. Manifest

All seven fields are required: `kind`, `producer`, `source`, `disclosure`, `view`,
`layout`, `artifacts`.

```json
{
  "kind": "fx-run-export-v1",
  "producer": {"version": "0.1.0"},
  "source": {"plan_sha256": "<64 lower-case hex>", "events_sha256": "<64 lower-case hex>", "event_count": 4},
  "disclosure": {"values": false, "diagnostics": false, "artifacts": "media"},
  "view": {"kind": "fx-viewer-run-v1"},
  "layout": {"kind": "fx-layout-report-v1"},
  "artifacts": []
}
```

The example abbreviates `view` and `layout`; the complete required structures
are the frozen copies in `fx-run-export-v1.schema.json`, not whatever a future
local viewer returns. Export readers ignore unknown optional fields. Missing or
invalid required fields, unsupported major kinds and broken references fail
explicitly. Changed essential semantics require a new major kind; the producer
version is diagnostic only.

### Positive inspection projection

The export view contains the following fields only. Identifiers, display labels
and media may themselves disclose information; export is deliberate sharing and
cannot certify arbitrary author text or file bytes public-safe.

- Run: `kind`, workflow `id`/`title` (and `description` only with values),
  `run_name`, `state`, `stand_in`, `charged_usd`, estimate numeric
  `low_usd`/`high_usd`/`ceiling_usd`, `inputs`, `outputs`, `nodes`, `scopes`,
  `pending`, `artifacts`, `warnings`. Workflow `file` is always absent.
- Node: `id`, `path`, `step`, `take`, `key`, `title`, `uses`, `state`, `reads`,
  `ports`, `bindings`, `interface_bindings`, `needs`, `judges`, `with`, `outputs`,
  `cache`, `error`, `duration_ms`, `started_at`, `interrupted` where present in
  the recorded projection. Required fields stay present. Textual values in
  `with`, `inputs` and `outputs` obey disclosure below.
- Scope: `id`, `parent`, `kind`, `path`, `step`, `take`, `title`, `source` (always
  null), `ports`, `input_bindings`, `output_bindings`, `nodes`, `pending`.
  Pending repeats: `path`, `step`, `max`, `phase`, `high_usd`, `waiting_on`.
- Bindings: `source`, `source_port`, `target_port`, `source_kind` only.
- Ports: declared inputs/outputs and parameter schemas. With values withheld,
  schemas permit only `type`, standard primitive `format`, recursively sanitized
  `properties`, `items`, boolean/schema `additionalProperties`, `required`,
  `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`,
  `minLength`, `maxLength`, `minItems`, `maxItems`, `uniqueItems`, `minProperties`,
  `maxProperties`, and recursively sanitized `allOf`/`anyOf`/`oneOf`/`not`.
  `default`, `examples`, `enum`, `const`, descriptions, titles, patterns, references
  and arbitrary extensions are withheld. With values included, recorded schemas
  are included except source/transport fields; authors review that disclosure.
- Artifact view entries: `digest`, `kind`, sanitized basename `name`, `size`,
  `available` (included bytes only), `url` (always null). Consumers resolve the
  inventory by digest at their explicit static boundary, never through the local
  API URL validator.

Every original value position that is withheld becomes `{"withheld":true}`,
including explicit null/empty/scalar values. Original parameter/output names stay
present. Encoded `file`, `list` and `collection` values preserve their structure
and media references; file objects contain only `digest`, `kind`, basename
`name`, `size` and optional `key`. Collection keys retain their identifying role.
With `--include-values`, recorded values are retained through this same file
projection. Saved-plan inputs and not-yet-executed node parameters are plain JSON,
so the exporter first normalizes each to the existing `{"value": original_json}`
encoding before projection or artifact discovery. Recorded event values already
have an explicit encoding and stay encoded. An ordinary authored object whose
keys happen to include `file`, `list`, `collection` or `value` therefore remains
opaque JSON, with every member preserved; it cannot become an artifact reference.
The captured source plan and its fingerprint are unchanged by this normalization.
The marker means withheld, not an FX execution value. Values that
were originally absent remain absent. `error` stays null and `warnings` stays
empty unless `--include-diagnostics` is selected. Explicitly included diagnostic
strings are author-controlled disclosure, not automatic redaction.

Withholding hides content, not shape. A withheld file value keeps its `digest`,
`size`, basename `name` and `key`: the name of a recorded input is its original
local filename, and the digest is an unsalted SHA-256 of the bytes, so it can
confirm a guessed text or JSON file. `source.plan_sha256` likewise lets someone
who can rebuild the exact `plan.json` confirm guessed inputs. Diagnostics may
name provider request ids, routes and, for a job of unknown outcome, its call key,
itself an unsalted digest of the request ([identity.md](identity.md) §9). An
allocated `run_name` is the run folder's name, which holds the local date
([store.md](store.md) §8); an explicit `--run` folder name is kept as given.

The layout keeps `kind`, `state`, `cells`, `order`; `cursor`, `file`, and `revision`
are always null. Diagnostics are empty unless explicitly included. Cell sources
and order preserve engine layout semantics. No service path or observation
cursor can enter an export. Export readers validate node/scope/member references
and layout coverage before displaying the recording:

- Every displayed node, scope and pending-repeat address has a cell. Additional
  cells can retain slots from the saved plan or an earlier expansion, including
  previously pending repeats; they are not extra executed nodes.
- `order` is unique and every entry names a recorded node or scope. Every scope
  and non-skipped node is represented. A planned-unused node retained as
  `skipped`, with null cache/duration, no outputs and no `started_at`, may be
  absent from member order. The local projection retains such inspection
  records while the layout excludes their absent members (`layout.md` §3–§4).
- Preserve those cells and that order. Consumers must not append invented
  execution members or reconstruct an expanded graph from editorial prose.

## 3. Artifact inventory and copying

Every recorded artifact appears once, sorted by digest, with exactly:
`digest`, `media_type`, basename `name`, `size`, `path`, `omitted_reason`.
Included entries have `path: "artifacts/<digest>.<extension>"` and
`omitted_reason: null`; omitted entries have `path: null` and reason
`policy_excluded` (not allowed media) or `unplaced_input` (recorded bytes were
never placed in this run). A selected, placed artifact whose size/hash/path fails
verification fails the whole export with `artifact_unavailable`; it is never an
omission. No cache or arbitrary input path is searched. All referenced digests
must exist in the inventory; availability in the view agrees with inclusion.

The v1 allowlist and deterministic suffixes are:

| Media type | Extension |
| --- | --- |
| image/png | png |
| image/jpeg | jpg |
| image/webp | webp |
| image/gif | gif |
| image/avif | avif |
| video/mp4 | mp4 |
| video/webm | webm |
| audio/wav | wav |
| audio/mpeg | mp3 |
| audio/ogg | ogg |
| audio/flac | flac |
| model/gltf-binary | glb |

Selected bytes are copied exactly as recorded, which is how they are verified.
Metadata embedded in a file travels with it: EXIF (including a photo's
location), PNG text chunks, content credentials (C2PA) and provider labels that
name a generation, container tags and creation times. FX neither reads nor
strips it; a node that re-encodes a file before it is recorded makes new bytes.

Paths contain no scheme, host, absolute path, backslash, traversal, query or
fragment. Consumers use a chosen trusted base URL and never auto-discover
external references in HTML, JSON, glTF or arbitrary attachment bytes. The first
canvas previews images and offers links for other included media; HTML is never
executed automatically.

Copy each selected artifact from the same confined, digest-verified file handle
used by the viewer, and hash/size-check the copied bytes again before committing.
Build in a temporary sibling directory. Write the manifest last and place the
complete directory atomically; an existing output, including symlinks, is
`destination_exists`. Never replace another export. The destination may not be
inside the source run. Failed preparation leaves no destination.

## 4. Results and failures

Success exits 0. `--json` emits:

```json
{"kind":"fx-run-export-result-v1","manifest_sha256":"<64 lower-case hex>","output":"./review","files":1,"bytes":123,"omitted_artifacts":[{"digest":"<64 lower-case hex>","reason":"unplaced_input"}]}
```

`files` and `bytes` count included artifact files only, excluding the manifest.
`output` is the caller's destination, a local command result, never portable
manifest data. Human output summarizes the same result. Refusal exits 2 and
`--json` emits `{"kind":"fx-run-export-error-v1","code":"busy","message":"The run is active."}`.
Codes: `unavailable`, `busy`, `incomplete_run`, `invalid_record`,
`destination_exists`, `destination_invalid`, `artifact_unavailable`,
`write_failed`. Messages contain no private absolute paths. CLI parse errors use
the ordinary CLI usage channel. Python and JavaScript saved-record methods invoke
this offline operation and expose native typed results/errors.

Public hosting, content catalogs, publishing adapters and compatibility locks
belong to consumers. Publishing is explicit; exporting never uploads anything.
