# refusals-ts

What a TypeScript project can get refused, while planning and while running.

## While planning (`case`, `expand`, exit 1)

The engine picks a module's host by its suffix ([protocol.md](../../spec/protocol.md) §1), and no
host runs `.tsx` (JSX needs a compiler; Node's type stripping does not read it), `.cts` or
`.cjs` (CommonJS): `card`, `common` and `script` are refused with `<uses>: no node host for
.<suffix> modules`. Without "yet": these are refused for good, unlike suffixes no host runs so
far.

`legacy/old.js` has an accepted suffix, but `legacy/package.json` says `"type": "commonjs"`, so
Node would load it as CommonJS. The TypeScript node host refuses it before its code runs
(§5.1): `legacy/old.js failed to import: legacy/old.js is not an ES module: set "type": "module"
in package.json`. Its code would have written `ran.txt`, which the step checks is absent.

`expand.json` holds the four problems and no instance.

## While running (`values`, exit 1)

The host checks every value it sends, where `JSON.stringify` would silently write something
else (§8.2, "Values"):

- `nan` reports a fact whose value is `NaN`: `TypeError: fact score: NaN is not a JSON number`,
  thrown by `ctx.fact` before anything is sent, and a `node_error` since the body lets it
  through;
- `keyed` puts a `Map` in `ctx.out.json`: `TypeError: ctx.out.json takes a JSON value: at /sizes:
  a Map is not a JSON value`;
- `plain` returns a keyed output as a plain object, which would lose the order of integer-like
  keys: `output entries is an object; a keyed output is a Map`, a `node_failure`.

`project.json` shows the three failures. The two bodies that hand over a wrong value cast it
(`as never`): the SDK's types refuse both at compile time.
