# Share a recorded run

A portable export contains a recorded workflow graph, node states, engine layout
and selected media. Use it for a review, documentation, a static demonstration,
or an inspectable result shared with a collaborator. This describes current
source; check `grida-fx export --help` before using an older installed release.

```sh
grida-fx export ./runs/example --out ./review --json
```

The source must be inactive with a terminal invocation. Failed and cancelled
terminal runs are valid. The destination must be new. Export reads saved evidence
without importing workflow code or using the cache, service or providers.

The result gives the manifest's SHA-256, output folder, copied artifact counts and
omitted artifacts. A successful export includes every selected file with verified
bytes. Recorded inputs that were never placed and unsupported artifact formats
remain visible omissions. A selected file that is missing or damaged fails the
export instead of producing a misleading partial success.

The canvas preserves recorded scopes and unused planned takes, including skipped
steps. If an older record lacks scope membership, the client keeps those steps
visible instead of inventing a nested execution history.

## Choose what to disclose

The default includes structure, status, timing, recorded cost and supported media.
Recorded values and diagnostic messages are withheld. Include them deliberately:

```sh
grida-fx export ./runs/example --out ./detailed-review \
  --include-values --include-diagnostics --json
```

Review the concrete output before sharing it. Labels and images can disclose
private information too. FX excludes known local transport fields and projects
file references; it cannot certify arbitrary authored text or media public-safe.
The exact media allowlist and projection are in [the export contract](../../spec/exports.md).

## Use the SDK

```python
from grida.fx import load_run

record = load_run("./runs/example")
receipt = record.export("./review")
print(receipt["manifest_sha256"], receipt["omitted_artifacts"])
```

Async Python uses `await record.export_async(...)`.

```ts
import { loadRun } from "@grida/fx";

const record = await loadRun("./runs/example");
const receipt = await record.export("./review");
console.log(receipt.manifest_sha256, receipt.omitted_artifacts);
```

`RunExportError` carries a structured refusal code. A busy or unfinished source
is a refusal to export, not an instruction to execute or cancel it.

## Keep the manifest and files together

```text
review/
  manifest.json
  artifacts/<digest>.<extension>
```

Resource paths are relative, so the directory can be copied or served by an
ordinary file host. A compatible browser client resolves its files against that
directory and validates the pinned manifest digest. The format is independent of
any CDN and contains no browser credentials. It carries evidence for inspection;
it does not provide executable workflow code or a resumable run backup.

Publish at an immutable location and keep the manifest digest with the reference.
Changing presentation packaging creates a new export; it does not require
regenerating paid results. A client must support the export's wire version and
report unsupported formats explicitly. Uploading, site authoring and storage
account management belong to the publishing application.
