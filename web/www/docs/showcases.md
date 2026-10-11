# Showcase maintenance

This is private tooling for FX's website, not a public workflow packaging API.
The reusable recorded-run contract is [`fx-run-export-v1`](../../../spec/exports.md).
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

Overview presents native website demos and authored usage content. Canvas uses
the shared FX export reader and the installed viewer's canvas. These records are
frozen, without polling or local lifecycle/file actions. Omissions remain
explicit; the inspector distinguishes withheld values from empty ones. The
Overview's demo manifest is a website projection, not the recording's run record.
Custom generation workflows and nodes shown here are not installed by FX.

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
bun --no-env-file run --cwd web/www prepare:showcases \
  /absolute/path/to/reviewed-campaign /absolute/path/to/run-exports
bun --no-env-file run --cwd web/www verify:showcases --media --recordings
bun --no-env-file run build:www
PORT=3101 bun --no-env-file run preview:www
```

Preparation creates `.showcases/delivery/`, updates the small frozen metadata and
content lock, and copies local preview bytes into ignored `public/showcases/`
and `public/runs/`. It refuses different existing immutable content. Large media,
raw runs, approvals, staging directories and verification receipts stay outside Git.
Small frozen documents let source checks/builds run without media or a CDN.

## Validate clients and content

Every website build checks the catalog against current readers. This is offline
and does not regenerate content. A successful build binds the exact static output,
content lock and configured delivery base; a lock change during build is refused.
Explicit `verify:showcases --media` checks all selected disk bytes. Runtime JSON
loaders verify pinned hashes, supported kinds, references and geometry before
rendering. Failure is visible instead of becoming an empty or changed canvas.

```sh
# Use the same base that was supplied to the successful website build.
bun --no-env-file run --cwd web/www candidate:showcases \
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
bun --no-env-file run --cwd web/www publish:tigris \
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
