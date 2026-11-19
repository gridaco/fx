# File references and native resolution

File discovery and resolution are read-only jobs. They do not execute workflow
source, import modules, start node hosts, load provider credentials, perform
network requests, or open/reveal anything. The caller owns presentation.

## Portable targets

An external file reference is `{subject, target}`. `subject` follows the
[viewer contract](viewer.md). `target` is one of:

- `{kind: artifact, artifact: {digest, kind, name, size, url}}`, where `digest` is
  a lowercase SHA-256, `size` is a nonnegative safe integer (at most
  9007199254740991), and `url` is a root-relative served URL, HTTP(S) URL, or
  `null`. URLs have no credentials, query, fragment, backslash or control
  character; names are display labels without absolute paths or controls and
  kinds are nonempty control-free identifiers;
- `{kind: source, source, symbol?, recorded_digest?}`, where `source` is a
  portable relative file label, `symbol` is an optional declared identifier,
  and `recorded_digest` is present only when the recorded source inventory
  knows that file's digest.

Targets are resolved against existing recorded evidence. Neither a URL nor a
caller-provided source path grants access to arbitrary bytes. Absolute source
paths, parent-directory escapes, URL-shaped labels and symbolic links are not
returned as portable source references. Builders use a file label plus their
function as `symbol`; node modules use their recorded module selector.

## Discovery

`grida-fx files list TARGET --json [--instance ID] [--project DIRECTORY]` reads
one existing run folder or a saved graph JSON file. It returns
`{kind: fx-files-v1, files: [...]}`. Each descriptor has `target`, `role`
(`artifact`, `workflow`, `node`, `source`, or `resource`), and optional
`instance_id` / `scope_id` attribution. A missing instance is a `not_found`
error, not a request to select a newer run.

The viewer's `GET /api/files` uses the same portable inventory for its selected
run or materialized plan. HTTP replies never include absolute paths and use
the selected entry's artifact URLs. It does not serve source code. Portable
exports retain their existing source-disclosure policy.

Source discovery uses recorded workflow, scope and type metadata. Older records
may omit it. It is not a workspace-wide file browser. Runtime source imports are
not performed to fill absent facts.

Legacy saved graph value positions use the [plain file projection](identity.md#3-the-plain-projection),
`{file: digest}`, which omits the name, kind and size required for an artifact
target. Discovery does not invent those facts or interpret file-shaped user JSON
as recorded artifact evidence.
New [value views](values.md) preserve explicit typed file metadata. Run-local
`values/<digest>` snapshots may supply original input bytes; they use the same confined
verification and native resolution as output artifacts. A bare saved plan with metadata
but no captured bytes remains unavailable. The viewer never reads current input source
paths merely to display a thumbnail.

## Native resolution

`grida-fx files resolve TARGET --reference JSON --json [--project DIRECTORY]
[--source-root DIRECTORY]`
accepts a captured external-file reference and returns:

```json
{
  "kind": "fx-file-resolution-v1",
  "target": {"kind": "source", "source": "nodes/render.py", "symbol": "render"},
  "local_path": null,
  "source_match": "unknown",
  "reason": "An explicit local source-root binding is required."
}
```

For a captured subject with a `service_entry` reference, omit the positional
`TARGET` and use
`grida-fx files resolve --project DIRECTORY --reference JSON --json`
with optional `--source-root DIRECTORY`. This form requires a valid
`service_entry` reference and the caller's exact project directory. The explicit
`--service-entry` spelling is accepted for compatibility. The engine checks
that the reference's project, entry kind and ID match the registered evidence.
It reads only the selected catalog entry and needs no running service. It
never discovers, creates, or changes catalog entries, or follows an entry to
a replacement run. The reference is an evidence selector. The caller supplies
an authorized project binding and decides whether to open the resulting file.

`target` is the validated recorded target. `local_path` is an absolute native
filesystem path only when an available local placement can be resolved.
`source_match` is `match`, `changed`, or `unknown` for source files, and `null`
for artifacts. `reason` is a safe, nonempty explanation when no path is
available. Missing bytes are a successful unavailable result. Malformed input,
unrecorded targets and verifiable subject mismatches are errors.

Artifact resolution reuses the HTTP inventory and verifies confinement, size
and digest for exactly the selected placement. Identical bytes may have several
placements; this resolves one available recorded copy, not a particular alias
or the user's original input pathname. Lookup is point-in-time, not a file lease.
Returned artifact paths refer to immutable stored/placed bytes: native consumers
must treat them as read-only, as with node-host file paths. Source paths below
refer to editable workspace files.

Artifact verification and source comparison open a confined regular file without
following symbolic links in its parent directories or final entry. Validation and
hashing use the same open handle, and a replacement FIFO cannot block the lookup.
Source lookup also validates the open handle when no recorded digest is available.
Returned source paths are point-in-time results, not file leases for a later host open.

Source resolution requires an explicit local source-root binding. `--project`
binds the local planning project/catalog and is also the default source root;
`--source-root` explicitly supplies the owning source root when different.
Neither binding is inferred from the current directory or run-folder location.
The default applies to directly recorded workflow, node-module and resource
paths. A closure-only helper label may instead be relative to a source package;
existing records do not retain that namespace. Such labels require an explicit
`--source-root` for the complete portable label, even if it is the same directory
as `--project`. This is a caller-owned mapping, not package discovery. A
direct-source label does not establish a closure label's namespace merely by
sharing its path. When the same complete target is recorded as both a closure
file and a direct source, it also requires an explicit `--source-root`. A
reference that omits a digest and matches distinct recorded targets is refused
as `invalid_request` rather than selecting an arbitrary target. Current
`fx.yaml` is never read or imported to infer that historical mapping: direct
source remains openable when the configuration needs repair.
It opens the current editable file, not a historical snapshot. A digest mismatch
reports `changed` and does not prevent opening. Without a comparable recorded
digest, the status is `unknown`. A missing file, withheld source, or a source
package whose local root is not bound is unavailable. Source labels are never
blindly joined to a run directory or inferred from a browser URL. The binding is
the workflow's owning source root, which can differ from the planning project.

An exact `service_entry` reference is checked against the corresponding private
local catalog binding and current run/plan identity. An export reference cannot
be interpreted as a local run. When a standalone subject has no durable
reference, the caller explicitly binds its viewer to the selected native source;
workflow metadata is still checked when supplied. Unknown references are not
silently attached to another run.

Structured errors are `{kind: fx-file-error-v1, code, message}` and exit 2.
Codes include `invalid_request`, `invalid_source`, `not_found`, and
`stale_subject`. Messages contain no private absolute path or authorization
material. Python and JavaScript expose the same jobs without implementing
identity or path resolution themselves; see [sdk.md](sdk.md).

Native results are local caller data. Never copy `local_path` into viewer
messages, browser documents, events, exported records, or caches. The resolver
does not provide filesystem access to the server or page.
