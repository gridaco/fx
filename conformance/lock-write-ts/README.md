# lock-write-ts

[`lock-write`](../lock-write/) for TypeScript. `fx.lock` records the source digest of every
versioned node type in the project ([identity.md](../../spec/identity.md) §6); `lock` finds them
by scanning the project for node modules of every host's suffixes and describing each.

| File | Scanned? |
|---|---|
| `nodes/n.ts` | yes: `echo` (unversioned, not locked) and `pinned` (version 2) |
| `nodes/plain.mjs` | yes: `shout` (version 1), plain JavaScript, used by no workflow |
| `nodes/helper.ts` | yes, and declares no node type; it is in `n.ts`'s closure |
| `nodes/shapes.d.ts` | no: a declaration file holds types only |
| `node_modules/@grida/fx/…` | no: nothing under `node_modules`, where the runner installs `@grida/fx` |

## What the case pins

- `lock --check` names both unlocked types and fails.
- `lock` writes `fx.lock` with two entries: `nodes/n.ts#pinned@2` (the digest of `nodes/n.ts`
  and `nodes/helper.ts`) and `nodes/plain.mjs#shout@1` (the digest of `nodes/plain.mjs`). Both
  digests agree with `tools/digest.py`'s `node_source_digest` over those files.
- `lock --check` then passes, and the workflow plans.
