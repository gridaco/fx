"""Build and verify a native macOS preview bundle; never upload it.

    uv run --locked --project python python tools/prepare_local.py

Use --out to choose a new output directory. Publish the resulting manifest with
tools/publish_local.py after reviewing it. Linux targets use the release workflow's
glibc 2.28 builds, rather than the build machine's system glibc.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

from package_release import ROOT, clean_environment, package, run, source_snapshot


def native_target() -> tuple[str, str]:
    targets = {
        ("Darwin", "arm64"): ("aarch64-apple-darwin", "11.0"),
        ("Darwin", "x86_64"): ("x86_64-apple-darwin", "10.12"),
    }
    try:
        return targets[platform.system(), platform.machine()]
    except KeyError as error:
        raise ValueError(
            "local preparation supports native macOS; use release CI for Linux"
        ) from error


def prepare(out: Path | None) -> Path:
    target, minimum = native_target()
    for program in ("bun", "cargo", "node", "npm", "uv", "git"):
        if shutil.which(program) is None:
            raise ValueError(f"{program} is required on PATH")
    expected_bun = json.loads((ROOT / "package.json").read_text())[
        "packageManager"
    ].split("@")[1]
    actual_bun = subprocess.check_output(["bun", "--version"], text=True).strip()
    if actual_bun != expected_bun:
        raise ValueError(
            f"use Bun {expected_bun} (found {actual_bun}); the lockfile is frozen"
        )
    version = subprocess.check_output(
        [
            sys.executable,
            str(ROOT / "tools/check_versions.py"),
            "--prerelease",
            "--print",
            "semver",
        ],
        cwd=ROOT,
        text=True,
    ).strip()
    out = (out or ROOT / "target/packages" / f"{version}-{target}").absolute()
    if out.exists():
        raise ValueError(
            "output already exists; review it or select a new directory with --out"
        )
    source = source_snapshot()
    environment = clean_environment()
    environment.update(MACOSX_DEPLOYMENT_TARGET=minimum, CARGO_INCREMENTAL="0")
    # Release paths must not reveal the builder's private directories.
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")).resolve()
    rustup_home = Path(os.environ.get("RUSTUP_HOME", Path.home() / ".rustup")).resolve()
    environment.pop("RUSTFLAGS", None)
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(
        f"--remap-path-prefix={source}={name}"
        for source, name in (
            (ROOT, "fx"),
            (cargo_home, "cargo"),
            (rustup_home, "rustup"),
        )
    )
    run(["bun", "--no-env-file", "install", "--frozen-lockfile"], env=environment)
    for check in ("check:viewer", "test:viewer", "build:viewer"):
        run(["bun", "--no-env-file", "run", check], env=environment)
    run(
        ["bun", "--no-env-file", "install", "--frozen-lockfile"],
        cwd=ROOT / "js/fx",
        env=environment,
    )
    run(
        ["bun", "--no-env-file", "run", "typecheck"],
        cwd=ROOT / "js/fx",
        env=environment,
    )
    run(
        [
            "cargo",
            "build",
            "--locked",
            "--release",
            "--bin",
            "grida-fx",
            "--target",
            target,
        ],
        env=environment,
    )
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"],
            cwd=ROOT,
            env=environment,
        )
    )
    binary = Path(metadata["target_directory"]) / target / "release/grida-fx"
    if source_snapshot() != source:
        raise ValueError(
            "source changed during the build; prepare again before packaging"
        )
    return package(binary, target, out, installed_checks=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--out",
        type=Path,
        help="new bundle directory (default: target/packages/<version>-<target>)",
    )
    arguments = parser.parse_args(argv)
    try:
        manifest = prepare(arguments.out)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"prepare_local: {error}", file=sys.stderr)
        return 1
    print(f"Verified bundle: {manifest}")
    print("Nothing uploaded. Review the offline publication plan:")
    print(
        shlex.join(
            [
                "uv",
                "run",
                "--locked",
                "--project",
                "python",
                "python",
                "tools/publish_local.py",
                "--manifest",
                str(manifest),
            ]
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
