# Standalone downloads

FX releases starting with 0.12.0 offer a native `grida-fx` executable for pinned CI images
and other tools that install the CLI directly. Each archive contains the executable and its
license. It includes the FX Viewer.

## Choose your archive

Open the version you want on [FX GitHub Releases](https://github.com/gridaco/fx/releases).
Download its `SHA256SUMS` and the `grida-fx-<version>-<target>.tar.gz` asset for your machine.
Check the machine architecture with `uname -m`.

| Platform | Architecture | Target |
| --- | --- | --- |
| macOS 11 or later | Apple silicon (`arm64`) | `aarch64-apple-darwin` |
| macOS 10.12 or later | Intel (`x86_64`) | `x86_64-apple-darwin` |
| Linux with glibc 2.28 or later | `aarch64` | `aarch64-unknown-linux-gnu` |
| Linux with glibc 2.28 or later | `x86_64` | `x86_64-unknown-linux-gnu` |

On Windows, use the appropriate Linux archive in WSL 2. On Alpine Linux, use a
glibc-based environment for the Linux executable.

## Verify and install

From the download directory, set `archive` to the filename you downloaded. This example
uses 0.12.0 for Apple silicon.

```sh
archive=grida-fx-0.12.0-aarch64-apple-darwin.tar.gz
awk -v name="$archive" '$2 == name { print; count++ } END { exit count != 1 }' \
  SHA256SUMS > selected.sha256 &&
  shasum -a 256 -c selected.sha256
```

On Linux, you can use `sha256sum -c selected.sha256` for the last command.
After the check reports `OK`, unpack into a new directory and install the executable.

```sh
mkdir fx-download
tar -xzf "$archive" -C fx-download
mkdir -p "$HOME/.local/bin"
install -m 755 fx-download/grida-fx "$HOME/.local/bin/grida-fx"
"$HOME/.local/bin/grida-fx" --version
```

Use the full path in CI, or add `$HOME/.local/bin` to your `PATH`. The guide's
`grida fx` commands take the same arguments when run as `grida-fx`.

## Workflow hosts

The executable runs the CLI and serves the bundled viewer directly. Workflows with
Python nodes or built-in local processing need Python 3.11 or later with a matching
`grida` package. Workflows with TypeScript nodes need Node 24 or later and a matching
`@grida/fx` dependency. [Getting started](01-getting-started.md#when-fx-needs-python)
explains how to select the Python or Node interpreter.
