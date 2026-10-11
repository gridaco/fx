# The grida host fixture

A stand-in for the npm `grida` package ([gridaco/grida](https://github.com/gridaco/grida),
`packages/grida-cli`) reduced to the `fx` verb, so this repository can hold `grida fx` to
[spec/distribution.md](../../../spec/distribution.md) without the rest of `grida`:

- `package.json` pins `@grida/fx` exactly at the workspace version, as `grida` does
  (`tools/check_versions.py` holds it there).
- `bin/grida.js` is the `fx` branch of `grida`'s launcher, as `grida` carries it: on `fx` it
  imports `@grida/fx/cli`, refuses a host contract it was not written for, and calls `launch` as
  the `grida` invoker. Any other verb exits 2.

[`tools/check_host.py`](../../../tools/check_host.py) packs this folder beside a packed
`@grida/fx` and the engine package for the machine, installs the three into a fresh npm prefix,
and runs the whole conformance suite and the viewer check through `<prefix>/.bin/grida fx`.
`tools/package_release.py --verify` runs the same checks on every release candidate.

The package is private and never published. A change to the `fx` branch here is a change to the
host contract: change `spec/distribution.md` first, then `grida`'s launcher to match.
