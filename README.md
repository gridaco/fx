# Grida FX

A workflow engine for generative asset pipelines. Write a workflow file, and FX plans it, prices it before anything is spent, caches every paid call by its request, and records the run. FX works with your own provider keys, and your tests run a workflow offline with [stand-ins](docs/guide/05-running.md#stand-ins-testing-without-a-provider): functions that answer its paid calls in place of a provider, held to the same checks and billed at nothing.

FX grew out of gnode, the Python engine [softmarshmallow/stage-gen](https://github.com/softmarshmallow/stage-gen) ran on until it moved onto FX. See [the overview](docs/wg/overview.md) for the direction and the plan.

## Install (preview)

The preview is published on npm under the `next` tag and on PyPI as a pre-release, for all four platforms below. To develop FX from a clone, you need cargo, [Bun](https://bun.sh) and [uv](https://docs.astral.sh/uv/) ([examples/README.md](examples/README.md#from-a-clone) has the rest):

```sh
git clone https://github.com/gridaco/fx && cd fx
uv sync --project python          # the Python SDK and Pillow, in python/.venv
bun --no-env-file install --frozen-lockfile
python3 tools/build_engine.py     # the engine, built into that SDK; again after every pull
uv run --project python python -m grida.fx --version
```

Both package formats carry the same engine, the `grida-fx` binary, with its viewer embedded. Installed packages need no Bun, Docker or frontend build.

```sh
npm install -g @grida/fx@next    # the grida-fx command, and the JavaScript SDK
grida-fx --version
npx @grida/fx@next --version     # or run it without installing
```

Install the Python SDK:

```sh
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

For viewer development, the [canonical fixture suite](fixtures/viewer/README.md)
generates real provider-free plans and runs covering branches, joins, matrices,
keyed repeats, conditions, media, cache reuse, takes and failures.
