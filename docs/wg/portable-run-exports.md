# Portable run exports

Status: implemented in source, 2026-10-11. [spec/exports.md](../../spec/exports.md)
owns the normative `fx-run-export-v1` format. The CLI, Python and JavaScript
saved-record APIs, and shared static browser reader follow it. Check installed
help before using this capability with an older published engine.

## Purpose and boundaries

A run export packages recorded workflow topology, execution evidence, engine
layout and selected artifact bytes for inspection without the author's machine
or FX service. Useful jobs include sharing a result with a collaborator,
attaching a trace to an issue, publishing workflow examples and retaining a
read-only record after retiring a local service.

An export is neither an executable workflow nor a resumable backup. Exporting
does not import author code, open a cache, load credentials, rerun steps or upload
files. Execution, cache and observation identities stay unchanged. Public or
private hosting, catalogs, editorial pages and publication adapters belong to
consumers; they are not part of the export contract.

The same canvas can inspect local runs and frozen exports. Live observation and
local file actions remain capabilities of the local host, not of the recording.

## Decisions

- A directory is the canonical transport: `manifest.json` plus selected files
  under `artifacts/`. A future ZIP can wrap it without changing the format.
- Capture only a terminal, inactive record while holding its existing ownership
  lock. Refuse busy, unfinished or malformed records without repairing them.
- Use a positive disclosure projection. Values and diagnostics are withheld by
  default; identifiers, labels and selected media still require the author's
  review before sharing. Withheld values differ from originally empty values.
- Reuse the engine's recorded projection, layout and confined artifact reader.
  Verify selected bytes and reference closure before atomically placing a new
  directory. Refuse existing destinations and broken selected artifacts.
- Fingerprint exact saved plan bytes and the canonical captured event array.
  Hash the exact manifest bytes externally; do not put its own digest inside it.
  Fingerprints identify evidence, not signatures or proof of execution.
- Use an explicit browser parser and trusted-base resource resolver. Keep local
  API URL validation unchanged; never auto-discover external references inside
  arbitrary attachments or automatically execute HTML.
- The wire `kind` determines compatibility. Ignore optional additions, reject
  missing required fields and invalid references, and use a new major kind for
  changed required meanings. Producer versions are diagnostic only.

## User operations

```sh
grida-fx export runs/example --out ./review --json
# Deliberately disclose recorded values and diagnostics:
grida-fx export runs/example --out ./detailed-review --include-values --include-diagnostics --json
```

Python `RunRecord.export` / `export_async` and JavaScript `RunRecord.export`
invoke the same offline engine operation. They expose native receipts and
structured refusal codes rather than reimplementing capture or identity.
Relative output destinations resolve against the caller's current directory.

See [the sharing guide](../guide/11-exports.md) for disclosure, omissions,
hosting requirements and examples.

## Verification and limitations

Rust exporter regression tests cover terminal admission, locking, disclosure,
literal JSON normalization, selected-file verification, unsafe paths and
non-overwriting placement. [CLI conformance](../../conformance/portable-export/)
exercises a real provider-free saved run and structured refusals. SDK tests
cover options, receipts and errors; CLI, Python and JavaScript produced identical
manifest bytes from the same saved recording.

The [shared browser tests](../../web/fx-web/tests/export.test.ts) cover pinned
manifest verification, required shapes, reference and layout consistency,
resource confinement and static controller behavior. A consumer must also check
actual rendered behavior against its selected content; parser acceptance alone
does not prove a page displays that evidence correctly.

By design, an export discloses embedded media metadata and the shape of withheld
values (digests, sizes, original filenames); [the export contract](../../spec/exports.md)
lists them.

Older records lacking imported-scope membership retain unused planned instances
at root. The export client does not infer missing nesting from display paths.
Unfinished/crashed captures, arbitrary attachment packaging, automatic
publication and executable workflow bundles remain outside v1.
