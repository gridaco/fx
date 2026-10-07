# Releasing

Milestone 2 and the installed-preview checks have passed. Python uses stable releases
starting at `0.1.0`, so users can install with `pip install grida`. npm uses `latest` for stable versions and `next` for pre-releases. [`.github/workflows/release.yml`](.github/workflows/release.yml) builds,
checks and publishes both package formats from the same version. The owner pushes the tag
and approves the publish. CI uses OIDC for both registries and holds no publishing token.
Local uploads have no CI build provenance.

The first CI preview is published: `0.1.0-alpha.2` on npm and `0.1.0a2` on PyPI.
The stable release `0.1.0` is published on npm and PyPI.
All five npm trusted publishers and the PyPI publisher have completed a tokenless
upload. Future releases start with a version bump; the initial bootstrap is complete.

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
  which runs the engine package npm installed for the machine). All five under `latest` for stable versions or `next` for pre-releases, with
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

The release tag is `v` followed by the workspace version, for example `v0.1.0` or `v0.2.0-alpha.1`. The
workflow refuses a tag that names any other version. It accepts stable versions and
pre-releases (`-alpha.N`, `-beta.N` or `-rc.N`).

## One-time setup

1. **Push `main`** and wait for CI to pass.
2. **Create the GitHub environment `release`** (Settings → Environments → New environment):
   - **Required reviewers:** yourself, plus anyone else who may approve a publish. If you are
     the only reviewer, leave "Prevent self-review" off, or you can never approve your own tag.
   - **Deployment branches and tags:** choose "Selected branches and tags" and add a tag rule
     `v*`, so only a release tag can reach the publish job.
3. **npm:**
   - Make sure the npm organization `grida` exists and your account can publish under `@grida`
     (as an owner, or as a member of a team with write access to the scope).
   - **Bootstrap locally with `npm login` and account 2FA.** npm configures a trusted publisher
     on an existing package's settings page. The Apple Silicon local preview creates
     `@grida/fx-darwin-arm64` and `@grida/fx`; the dry run below supplies the remaining three
     engine tarballs for a manual first upload. Do not add an npm token to GitHub.
   - **Then configure each package's trusted publisher.** Open each of
     the five packages' Settings → Trusted Publisher → GitHub Actions and enter: organization
     `gridaco`, repository `fx`, workflow `release.yml`, environment `release`.
     Under allowed actions, enable direct publishing with **`npm publish`**; the default
     staged-only permission does not authorize this workflow. Register the publishers shortly
     before the next release: a new configuration expires if it has not published within two
     days. You can also set each package's publishing access to
     "Require two-factor authentication and disallow tokens".
4. **PyPI:** `grida` already exists; use an account that owns it. Open
   [Manage project → Publishing](https://pypi.org/manage/project/grida/settings/publishing/)
   → Add a new
   publisher → GitHub, and enter: owner `gridaco`, repository `fx`, workflow `release.yml`,
   environment `release`. This needs no token: `pypa/gh-action-pypi-publish` uses trusted
   publishing, and it attaches attestations.
   - **Optional, recommended:** yank `grida` 0.0.1, the placeholder that reserved the name. Until
     you do, on a machine with no wheel (Windows, glibc older than 2.28, musl),
     `pip install grida` can fall back to 0.0.1 instead of failing. A yanked release is
     installed only when pinned exactly.

Both registries trust the same identity:

| Field | Value |
|---|---|
| GitHub owner | `gridaco` |
| Repository | `fx` |
| Workflow filename | `release.yml` (filename only) |
| Environment | `release` |

The publisher uses GitHub-hosted runners and `id-token: write`. No `NPM_TOKEN`,
`NODE_AUTH_TOKEN` or PyPI API token is configured in CI. See the current
[npm trusted publishing instructions](https://docs.npmjs.com/trusted-publishers/)
and [PyPI existing-project setup](https://docs.pypi.org/trusted-publishers/adding-a-publisher/).

## A dry run

Go to Actions → release → Run workflow. Pick `main` (or the tag), and leave **dry_run** checked.
The run builds all four targets and checks everything, but publishes nothing. Its artifacts
(`wheel-<target>`, `npm-<target>` and `npm-sdk`) are the exact files a release would publish, so
you can download and inspect them. Do this before the first tag. (A run started on a `v*` tag
with dry_run unchecked publishes, just as pushing the tag does.)

After the Apple Silicon preview, bootstrap the other three npm packages from a **successful**
dry run of the same source and version. Download its `npm-x86_64-apple-darwin`,
`npm-x86_64-unknown-linux-gnu` and `npm-aarch64-unknown-linux-gnu` artifacts into separate folders
(Actions → run → Artifacts, or `gh run download <run-id> --repo gridaco/fx --dir target/bootstrap`).
Each contains exactly one tested engine tarball. Inspect them and publish from your
npm-authenticated machine:

```sh
npm publish target/bootstrap/npm-x86_64-apple-darwin/*.tgz --tag next --access public --ignore-scripts --registry https://registry.npmjs.org
npm publish target/bootstrap/npm-x86_64-unknown-linux-gnu/*.tgz --tag next --access public --ignore-scripts --registry https://registry.npmjs.org
npm publish target/bootstrap/npm-aarch64-unknown-linux-gnu/*.tgz --tag next --access public --ignore-scripts --registry https://registry.npmjs.org
```

All five npm package pages now exist. Register their trusted publishers, configure PyPI,
bump to the next preview and push its tag. That next release publishes all four platforms
through OIDC, including the three remaining Python wheels. The initial local preview
supports Apple Silicon only until the remaining artifacts are uploaded.

## Local packaging and setup preview

`tools/package_release.py` is the shared packaging entry point for a compiled target.
It calls the existing wheel and npm packagers, checks version agreement, records
the Git-visible source snapshot (including uncommitted source), and writes a manifest
with SHA-256 hashes. It never publishes. CI uses it with each runner's release binary
and then performs its installed-package checks on the oldest supported runtimes.
Locally, `--verify` performs clean Python/npm installs, strict conformance and
embedded-viewer checks, with provider keys removed and dotenv disabled.

For a native macOS setup preview, use Bun **1.4.0**, Node 18 or later, Rust 1.89 or
later, and a current uv (`uv self update` for a standalone uv installation). The
publisher requires uv's `--no-attestations` option; uv 0.6 does not support it.
Prepare the entire verified bundle with one command:

```sh
uv run --locked --project python python tools/prepare_local.py
# Optional: choose a different, new output directory.
uv run --locked --project python python tools/prepare_local.py --out target/packages/review-2
```

`prepare_local.py` installs frozen Bun dependencies, checks/tests/builds the viewer,
typechecks the SDK, builds a release engine with the deployment baseline and remapped
paths, and calls the shared packager with installed verification enabled. It never
uploads. On Apple Silicon, the default output is
`target/packages/<workspace-version>-aarch64-apple-darwin/`
(for example `target/packages/0.1.0-aarch64-apple-darwin/`).

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
the native npm engine before the SDK under `latest` (or `next` for pre-releases), then the PyPI wheel.
Use `--only pypi` or `--only npm` to publish them independently, including from
different machines. Copy the manifest and all three artifacts together: the full
bundle is still validated, but only the selected registry is contacted and only
its uploader is required. A prior PyPI publication does not block npm-only publishing.
The account owner performs npm login/2FA. For local PyPI publishing, open
[PyPI account settings → API tokens](https://pypi.org/manage/account/token/), create a token
scoped to **Project: grida**, and paste it into uv's hidden password prompt
(username is `__token__`). A local machine cannot use GitHub Actions OIDC. Revoke this
bootstrap token after the local upload; CI uses the trusted publisher instead. Never put a
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

First bump every manifest and lockfile to a new version as described above.
Then tag that version; never reuse an already published version or tag.

```sh
git switch main && git pull
version=$(uv run --project python python tools/check_versions.py --print semver)
uv run --project python python tools/check_versions.py --tag "v$version"
git tag -a "v$version" -m "Grida FX $version"
git push origin "v$version"
```

The tag starts `release.yml`, which runs four jobs:

1. **versions:** `tools/check_versions.py --tag <tag>`. Every manifest names the
   workspace version, and the tag names it too.
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
   `@grida/fx` to npm, under `latest` for stable versions or `next` for pre-releases, with `--access public --provenance`, and then the four wheels
   to PyPI. It checks out no code. A package version that is already published is skipped,
   so you can re-run a publish that failed partway.

A published version can never be replaced. If something is wrong after publishing, fix it,
bump to a new version (for example `0.1.1`) and tag again.

## Checking the published release

On a supported machine, after the run is green:

```sh
npm view @grida/fx dist-tags                # latest: 0.1.0
npm install -g @grida/fx
grida-fx --version                          # grida-fx 0.1.0
npx --yes @grida/fx --version               # the same, without installing

python3.12 -m venv fx-check                 # any Python 3.11 or later
fx-check/bin/pip install grida
fx-check/bin/python -m grida.fx --version   # grida-fx 0.1.0
```

- **Provenance:** each package's npm page shows its provenance, linked to the workflow run.
  PyPI shows each wheel's attestations on its file page.
- **Conformance:** to run the whole suite against the installed release, from a checkout:

  ```sh
  GRIDA_FX_PYTHON=fx-check/bin/python uv run --project python python conformance/run.py \
    --strict --command "fx-check/bin/python -m grida.fx"
  ```
