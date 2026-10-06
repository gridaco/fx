# Grida FX

A workflow engine for generative asset pipelines. Write a workflow file, and FX plans it, prices it before anything is spent, caches every paid call by its request, and records the run. FX works with your own provider keys.

FX is moving here from [softmarshmallow/stage-gen](https://github.com/softmarshmallow/stage-gen), where it is called gnode. See [the overview](docs/wg/overview.md) for the direction and the plan.

## Install (preview)

FX is in preview: npm has it under the `next` tag and PyPI as a pre-release. Both packages carry the same engine, the `grida-fx` binary.

```sh
npm install -g @grida/fx@next    # the grida-fx command, and the JavaScript SDK
grida-fx --version
npx @grida/fx@next --version     # or run it without installing

pip install --pre grida          # the Python SDK, imported as grida.fx
python -m grida.fx --version     # the engine from Python, with no Node needed
```

The preview runs on:

| Platform | npm engine package | PyPI wheel |
|---|---|---|
| macOS 11 or later on Apple silicon | `@grida/fx-darwin-arm64` | `macosx_11_0_arm64` |
| macOS 10.12 or later on Intel | `@grida/fx-darwin-x64` | `macosx_10_12_x86_64` |
| Linux on x64, glibc 2.28 or later | `@grida/fx-linux-x64-gnu` | `manylinux_2_28_x86_64` |
| Linux on arm64, glibc 2.28 or later | `@grida/fx-linux-arm64-gnu` | `manylinux_2_28_aarch64` |

The npm package needs Node 18 or later; the Python one needs Python 3.11 or later. **Windows is not supported yet**, since the engine's runner uses Unix process groups and signals; use WSL 2. Linux with musl (Alpine) isn't supported either.

Start with [the guide](docs/guide/01-getting-started.md). [RELEASING.md](RELEASING.md) says how a preview is published.
