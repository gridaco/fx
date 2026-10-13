# local-identity-ts

[`local-identity`](../local-identity/) for TypeScript. An unversioned project type is named by
its export and its source ([identity.md](../../spec/identity.md) §6): the digest of its
module's source closure and its declared resources. The TypeScript node host reports the closure
from the module's literal imports, resolved as Node resolves them, and from Node's own module
resolution ([protocol.md](../../spec/protocol.md) §5.1, "For TypeScript").

## The project

| File | In `echo`'s closure? |
|---|---|
| `nodes/n.ts` | yes: the module |
| `nodes/helper.ts` | yes: `n.ts` imports it |
| `lib/words.ts` | yes: `helper.ts` imports it (a transitive import) |
| `lib/pad.ts` | yes: imported as `#lib/pad.ts`, through `package.json`'s `imports` |
| `packages/shapes/index.ts` | yes, as `@acme/shapes/index.ts`: see below |
| `nodes/types.ts` | no: `n.ts` imports it with `import type`, which is erased before the module runs |
| `node_modules/acme-colors/index.js` | no: an installed dependency, under `node_modules` |
| `prompts/r.md` | a declared resource of `echo`, not of `pinned` |

`@acme/shapes` is a workspace package: `in/node_modules/@acme/shapes` is a relative symbolic
link to `../../packages/shapes`, as a package manager's workspace makes it, and `fx.yaml` declares
it in `sources`. Node loads its file by its real path, and the host labels files under a declared
source package by the package's name and their path inside it, so the label is
`@acme/shapes/index.ts`, not `packages/shapes/index.ts`. The runner copies the link as a link.
`tools/check_spec.py` rehashes the labels it finds in the project (`expand.json`); a source
package's label is not one of them, and its digest is `packages/shapes/index.ts`'s.

`node_modules/acme-colors` (`package.json`, `index.js` and `index.d.ts`) is committed as a
fixture: `.gitignore` keeps only `node_modules/@grida/` out of a case.

## What the case pins

- `expand.json`: the closure above, with each file's digest; `echo` is
  `nodes/n.ts#echo@source:<digest>`, and `pinned` is `nodes/n.ts#pinned@2`.
- Editing `nodes/types.ts` or the dependency leaves every identity as it was (`identity.json`,
  saved three times, must be the same bytes).
- Editing `lib/words.ts`, `lib/pad.ts`, the package's `index.ts` and the resource each moves
  `a#1`, to a new identity each time.
- `b#1` never moves: every step that saves `identity-pinned.json` prints the same bytes.
