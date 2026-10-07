# Releasing

FX ships as previews until milestone 2 passes and a published preview has passed its checks as
installed ([below](#checking-the-published-preview); [overview](docs/wg/overview.md)): npm versions
go under the dist-tag `next`, and PyPI gets pre-releases. [`.github/workflows/release.yml`](.github/workflows/release.yml)
builds, checks and publishes. The owner does the account setup, pushes the tag and approves the
publish. An explicitly authorized local setup preview can use the shared packaging
tools and the separate manual publisher below; it has no CI build provenance.

## What a release publishes

One engine binary per target, packaged twice. Each binary embeds the same production viewer
bundle built once for that release; installed users need no Bun, Node server, Docker or frontend
build to open it. Python users still need no Node at all.

| Target | Built on | npm package | PyPI wheel tag |
|---|---|---|---|
| `aarch64-apple-darwin` | `macos-15` | `@grida/fx-darwin-arm64` | `macosx_11_0_arm64` |
| `x86_64-apple-darwin` | `macos-15-intel` | `@grida/fx-darwin-x64` | `macosx_10_12_x86_64` |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04`, cargo-zigbuild, glibc 2.28 | `@grida/fx-linux-x64-gnu` | `manylinux_2_28_x86_64` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm`, cargo-zigbuild, glibc 2.28 | `@grida/fx-linux-arm64-gnu` | `manylinux_2_28_aarch64` |

- **npm:** the four engine packages above, then `@grida/fx` (the SDK and the `grida-fx` command,
  which runs the engine package npm installed for the machine). All five under `next`, with
  provenance when published by the release workflow.
- **PyPI:** the distribution `grida`, one wheel per target, each carrying the engine at
  `grida/fx/_bin/grida-fx`. There is no sdist, since it could not run, and no console command:
  Python users run `python -m grida.fx <verb>`.
- **Windows is not supported yet.** The engine's runner uses Unix process groups and signals.
  Windows users can run the Linux packages under WSL 2. Linux with musl (Alpine) isn't supported
  either.

## The version

The workspace version in `Cargo.toml` (`[workspace.package] version`) is the only source of
truth. Every other manifest repeats it in its own form:

- the crates inherit it, and `Cargo.lock` records it;
- `python/pyproject.toml` and `python/uv.lock` use the PEP 440 form (`0.1.0-alpha.1` is `0.1.0a1`);
- `js/fx/package.json` uses it unchanged and pins each engine package to it exactly, and
  `js/fx/src/index.ts` exports it as `version`.

To change it, edit `Cargo.toml` and each of those files. Then run `cargo update --workspace`
and `(cd python && uv lock)`, and check the result:

```sh
uv run --project python python tools/check_versions.py   # every place agrees (CI runs it too)
```

The release tag is `v` followed by the workspace version, for example `v0.1.0-alpha.1`. The
workflow refuses a tag that names any other version, and it refuses a version that is not a
pre-release (`-alpha.N`, `-beta.N` or `-rc.N`).

## One-time setup

1. **Push `main`** and wait for CI to pass.
2. **Create the GitHub environment `release`** (Settings → Environments → New environment):
   - **Required reviewers:** yourself, plus anyone else who may approve a publish. If you are
     the only reviewer, leave "Prevent self-review" off, or you can never approve your own tag.
   - **Deployment branches and tags:** choose "Selected branches and tags" and add a tag rule
     `v*`, so only a release tag can reach the publish job.
   - **Environment secret `NPM_TOKEN`:** needed only for the first publish (step 3).
3. **npm:**
   - Make sure the npm organization `grida` exists and your account can publish under `@grida`
     (as an owner, or as a member of a team with write access to the scope).
   - **The first publish needs a token.** npm configures a trusted publisher on a package's
     settings page, which exists only once the package does. All five packages are new, so the
     first release publishes with a token. On npmjs.com, go to Access Tokens → Generate New
     Token → Granular Access Token. Give it read and write access to packages and scopes,
     limited to the `@grida` scope, and a short expiry. If your account requires two-factor
     authentication for writes, allow the token to bypass it. Store the token as the
     `release` environment's `NPM_TOKEN` secret. The workflow passes it as `NODE_AUTH_TOKEN`
     and adds `--provenance`.
   - **Then switch each package to trusted publishing.** After the first release, open each of
     the five packages' Settings → Trusted Publisher → GitHub Actions and enter: organization
     `gridaco`, repository `fx`, workflow `release.yml`, environment `release`. Delete the
     `NPM_TOKEN` secret and revoke the token. npm tries OIDC before any token, and it then
     attests provenance by itself. You can also set each package's publishing access to
     "Require two-factor authentication and disallow tokens".
4. **PyPI:** you already hold the project `grida`. Open Manage project → Publishing → Add a new
   publisher → GitHub, and enter: owner `gridaco`, repository `fx`, workflow `release.yml`,
   environment `release`. This needs no token: `pypa/gh-action-pypi-publish` uses trusted
   publishing, and it attaches attestations.
   - **Optional, recommended:** yank `grida` 0.0.1, the placeholder that reserved the name. Until
     you do, on a machine with no wheel (Windows, glibc older than 2.28, musl),
     `pip install --pre grida` quietly installs 0.0.1 instead of failing. A yanked release is
     installed only when pinned exactly.

## A dry run

Go to Actions → release → Run workflow. Pick `main` (or the tag), and leave **dry_run** checked.
The run builds all four targets and checks everything, but publishes nothing. Its artifacts
(`wheel-<target>`, `npm-<target>` and `npm-sdk`) are the exact files a release would publish, so
you can download and inspect them. Do this before the first tag. (A run started on a `v*` tag
with dry_run unchecked publishes, just as pushing the tag does.)

## Local packaging and setup preview

`tools/package_release.py` is the shared packaging entry point for a compiled target.
It calls the existing wheel and npm packagers, checks version agreement, records
the Git-visible source snapshot (including uncommitted source), and writes a manifest
with SHA-256 hashes. It never publishes. CI uses it with each runner's release binary
and then performs its installed-package checks on the oldest supported runtimes.
Locally, `--verify` performs clean Python/npm installs, strict conformance and
embedded-viewer checks, with provider keys removed and dotenv disabled.

For an Apple Silicon setup preview, build the viewer and a production engine first:

```sh
bun --no-env-file install --frozen-lockfile
bun --no-env-file run check:viewer
bun --no-env-file run test:viewer
bun --no-env-file run build:viewer
(cd js/fx && bun --no-env-file install --frozen-lockfile)
MACOSX_DEPLOYMENT_TARGET=11.0 CARGO_INCREMENTAL=0 \
  RUSTFLAGS="--remap-path-prefix=$PWD=fx --remap-path-prefix=$HOME/.cargo=cargo --remap-path-prefix=$HOME/.rustup=rustup" \
  cargo build --locked --release --bin grida-fx --target aarch64-apple-darwin
uv run --locked --project python python tools/package_release.py \
  --binary target/aarch64-apple-darwin/release/grida-fx \
  --target aarch64-apple-darwin \
  --out target/packages/0.1.0-alpha.1-macos-arm64 --verify
```

The output directory must be new; an output inside the checkout must be gitignored.
The three artifacts are one `grida` wheel, the native npm engine tarball, and the
`@grida/fx` SDK tarball. Source or artifact changes during packaging refuse the manifest;
the recorded hashes belong to the exact bytes checked by the clean installations.
Inspect them before publication. Do not use a debug binary or plain `uv build` for
a release. Other platforms need their own correctly targeted release builds and
installed checks; an Apple Silicon-only setup preview must be described as such.

`tools/publish_local.py --manifest <output>/release-manifest.json` validates the
verified artifact identities and hashes and prints the publication plan, offline.
Only an explicit `--publish` uploads. It uses the production registries, publishing
the native npm engine before the SDK under `next`, then the PyPI pre-release wheel.
Use `--only pypi` or `--only npm` to publish them independently, including from
different machines. Copy the manifest and all three artifacts together: the full
bundle is still validated, but only the selected registry is contacted and only
its uploader is required. A prior PyPI publication does not block npm-only publishing.
The account owner performs npm login/2FA and provides a project-scoped PyPI token
through the uploader's secure prompt or their own credential setup. Never put a
token in source, command arguments, logs or `.env`. Local publishing does not request
CI provenance or trusted-publisher attestations. Already-published versions are
refused so a partial publication must be inspected before resuming.

```sh
uv run --locked --project python python tools/publish_local.py --manifest <output>/release-manifest.json
# After artifact review and explicit authorization:
npm login --registry https://registry.npmjs.org
uv run --locked --project python python tools/publish_local.py --manifest <output>/release-manifest.json --publish

# Publish Python first; npm authentication and publication can wait:
uv run --locked --project python python tools/publish_local.py --manifest <output>/release-manifest.json --only pypi --publish
# Later, on the npm-authenticated machine with the same verified bundle:
uv run --locked --project python python tools/publish_local.py --manifest <output>/release-manifest.json --only npm --publish
```

## Releasing

```sh
git switch main && git pull
uv run --project python python tools/check_versions.py --tag v0.1.0-alpha.1 --prerelease
git tag -a v0.1.0-alpha.1 -m "Grida FX 0.1.0-alpha.1 (preview)"
git push origin v0.1.0-alpha.1
```

The tag starts `release.yml`, which runs four jobs:

1. **versions:** `tools/check_versions.py --prerelease --tag <tag>`. Every manifest names the
   workspace version, the tag names it too, and it is a pre-release.
2. **viewer:** installs the frozen root Bun workspace, checks, tests and builds the viewer, and uploads
   `web/viewer/dist` as one build artifact. Generated assets stay out of Git. Missing assets fail
   the native build rather than producing a package without its UI.
3. **build**, once per target, on the runner in the table above:
   - downloads that viewer bundle before compiling, so all platforms embed identical assets;
   - builds `grida-fx` with `--locked --release` (cargo-zigbuild for glibc 2.28 on Linux;
     `MACOSX_DEPLOYMENT_TARGET` 11.0 or 10.12 on macOS), and checks that it prints
     `grida-fx <version>`;
   - packages both distributions through `tools/package_release.py`. Its wheel packager,
     `tools/build_wheel.py`, reads the platform tag back from the binary and refuses a
     dishonest one, and checks that the tag is the one in the table;
   - installs that wheel into a fresh Python 3.11 environment, and runs the whole conformance
     suite (`--strict`) through `python -m grida.fx`, with that environment hosting the node
     bodies;
   - its npm packager, `tools/build_npm.mjs`, runs under Node 18 (the oldest Node the
     packages support). It installs the tarballs into a fresh prefix, and runs the suite
     again through the installed `grida-fx` command;
   - runs `tools/check_viewer.py` through both installed commands from an isolated temporary
     directory. A synthetic graph needs no provider; loopback requests must retrieve every
     embedded frontend file byte-for-byte, without a browser or frontend runtime;
   - uploads the wheel and the engine package it just tested. Every job packs `@grida/fx` the
     same way, and the `x86_64-unknown-linux-gnu` job uploads it.
4. **publish** waits for a reviewer to approve the `release` environment. It runs only for a
   `v*` tag, and never for a dry run. It publishes the four engine packages and then
   `@grida/fx` to npm, under `next` with `--access public --provenance`, and then the four wheels
   to PyPI. It checks out no code. A package version that is already published is skipped,
   so you can re-run a publish that failed partway.

A published version can never be replaced. If something is wrong after publishing, fix it,
bump to the next pre-release (`0.1.0-alpha.2`, `0.1.0a2`) and tag again.

## Checking the published preview

On a supported machine, after the run is green:

```sh
npm view @grida/fx dist-tags                # next: 0.1.0-alpha.1
npm install -g @grida/fx@next
grida-fx --version                          # grida-fx 0.1.0-alpha.1
npx --yes @grida/fx@next --version          # the same, without installing

python3.12 -m venv fx-check                 # any Python 3.11 or later
fx-check/bin/pip install --pre grida
fx-check/bin/python -m grida.fx --version   # grida-fx 0.1.0-alpha.1
```

- **Provenance:** each package's npm page shows its provenance, linked to the workflow run.
  PyPI shows each wheel's attestations on its file page.
- **Conformance:** to run the whole suite against the installed preview, from a checkout:

  ```sh
  GRIDA_FX_PYTHON=fx-check/bin/python uv run --project python python conformance/run.py \
    --strict --command "fx-check/bin/python -m grida.fx"
  ```
