"""Check `grida fx`: FX as the npm `grida` command carries it (spec/distribution.md).

    python tools/check_host.py --engine target/debug/grida-fx
    python tools/check_host.py --engine target/release/grida-fx --target aarch64-apple-darwin
    python tools/check_host.py --tarballs "$DIST/npm"   # a release's own @grida/fx and engine

Packs `@grida/fx` and the engine package for `--target` (default: this machine's) with
`tools/build_npm.mjs`, packs the host fixture (`fixtures/host/grida/`, a stand-in for `grida`
reduced to its `fx` verb), installs the three into a fresh npm prefix, and runs through
`<prefix>/node_modules/.bin/grida fx`:

- the launcher checks: `--version` names the engine, usage and usage errors name `grida fx`, and
  any other verb of the stand-in is refused;
- the whole conformance suite, strict, as the `grida` invoker (conformance/run.py --invoker);
- the embedded-viewer check (tools/check_viewer.py).

With `--tarballs`, the two tarballs in that folder (`@grida/fx` and one engine package) are used
as they are: the release workflow checks the exact tarballs it uploads, and
`tools/package_release.py --verify` runs the same checks ([`install`] and [`check`] here).
Providers stay off and no `.env` is read. Node bodies need a Python with `grida` installed:
`GRIDA_FX_PYTHON`, as for conformance/run.py.
"""

from __future__ import annotations

import argparse
import os
import shlex
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "fixtures" / "host" / "grida"
NPM_FLAGS = ("--no-save", "--no-audit", "--no-fund", "--ignore-scripts")
PROVIDER_KEYS = (
    "GG_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "FAL_KEY",
    "TRIPO_API_KEY",
    "ELEVENLABS_API_KEY",
)


class HostError(Exception):
    """`grida fx` did not behave as spec/distribution.md says."""


def environment() -> dict[str, str]:
    """This process's environment without provider keys, engine overrides or an invoker."""
    env = dict(os.environ)
    for name in (*PROVIDER_KEYS, "GRIDA_FX_BIN", "GRIDA_FX_INVOKER", "PYTHONPATH", "PYTHONHOME"):
        env.pop(name, None)
    env.update(GRIDA_FX_DISABLE_DOTENV="1", GRIDA_FX_NETWORK="off")
    return env


def workspace_version() -> str:
    with (ROOT / "Cargo.toml").open("rb") as manifest:
        return tomllib.load(manifest)["workspace"]["package"]["version"]


def pack_fixture(out: Path) -> Path:
    """The host fixture packed into `out`, as npm would publish it."""
    out.mkdir(parents=True, exist_ok=True)
    before = set(out.glob("*.tgz"))
    subprocess.run(
        ["npm", "pack", str(FIXTURE), "--pack-destination", str(out), "--ignore-scripts"],
        cwd=out,
        env=environment(),
        check=True,
        stdout=subprocess.DEVNULL,
    )
    made = sorted(set(out.glob("*.tgz")) - before)
    if len(made) != 1:
        raise HostError(f"npm pack made {len(made)} tarballs of the host fixture, not one")
    return made[0]


def install(tarballs: list[Path], prefix: Path) -> Path:
    """Installs `tarballs` (the host fixture, `@grida/fx` and an engine package) into a fresh npm
    prefix; returns its `grida` command."""
    subprocess.run(
        [
            "npm",
            "install",
            "--prefix",
            str(prefix),
            *NPM_FLAGS,
            "--registry",
            "https://registry.npmjs.org",
            *map(str, tarballs),
        ],
        cwd=prefix.parent,
        env=environment(),
        check=True,
    )
    grida = prefix / "node_modules" / ".bin" / "grida"
    if not grida.is_file():
        raise HostError(f"the host fixture installed no grida command at {grida}")
    return grida


def _ran(argv: list[str], cwd: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True, timeout=120)


def check(grida: Path, version: str, cwd: Path, env: dict[str, str]) -> None:
    """The launcher checks, then conformance and the viewer through `grida fx`."""
    command = [str(grida), "fx"]
    shown = shlex.join(command)
    done = _ran([*command, "--version"], cwd, env)
    if (done.returncode, done.stdout) != (0, f"grida-fx {version}\n"):
        raise HostError(f"{shown} --version printed {done.stdout!r} {done.stderr!r}")
    done = _ran([*command, "--help"], cwd, env)
    if done.returncode != 0 or "Usage: grida fx <COMMAND>" not in done.stdout:
        raise HostError(f"{shown} --help does not name grida fx:\n{done.stdout}{done.stderr}")
    done = _ran([*command, "run", "--help"], cwd, env)
    if done.returncode != 0 or "Usage: grida fx run " not in done.stdout:
        raise HostError(f"{shown} run --help does not name grida fx run:\n{done.stdout}")
    done = _ran([*command, "no-such-verb"], cwd, env)
    if done.returncode != 2 or "Usage: grida fx <COMMAND>" not in done.stderr:
        raise HostError(f"{shown} no-such-verb exited {done.returncode}:\n{done.stderr}")
    done = _ran([*command, "inspect", "no-such-run"], cwd, env)
    if done.returncode != 2 or not done.stderr.startswith("grida fx: "):
        raise HostError(f"{shown} errors do not start with 'grida fx: ':\n{done.stderr}")
    done = _ran([str(grida), "no-such-verb"], cwd, env)
    if done.returncode != 2:
        raise HostError(f"the stand-in ran a verb other than fx (exit {done.returncode})")
    print(f"check_host: {shown}: version, usage, errors and dispatch", flush=True)
    subprocess.run(
        [
            sys.executable,
            str(ROOT / "conformance" / "run.py"),
            "--strict",
            "--invoker",
            "grida",
            "--command",
            shown,
        ],
        cwd=cwd,
        env=env,
        check=True,
    )
    subprocess.run(
        [sys.executable, str(ROOT / "tools" / "check_viewer.py"), "--command", shown],
        cwd=cwd,
        env=env,
        check=True,
    )


def host_target() -> str:
    """This machine's Rust target triple."""
    lines = subprocess.check_output(["rustc", "-vV"], text=True).splitlines()
    for line in lines:
        if line.startswith("host: "):
            return line.removeprefix("host: ")
    raise HostError("rustc -vV names no host")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--engine", type=Path, help="a grida-fx binary, packed here")
    source.add_argument("--tarballs", type=Path, help="a folder of packed @grida/fx and an engine")
    parser.add_argument("--target", help="the engine's Rust target (default: this machine's)")
    args = parser.parse_args(argv)
    if args.target and args.tarballs:
        parser.error("--target goes with --engine")
    version = workspace_version()
    with tempfile.TemporaryDirectory(prefix="fx-host-check-") as folder:
        work = Path(folder)
        packed = args.tarballs.resolve(strict=True) if args.tarballs else work / "npm"
        if args.engine:
            subprocess.run(
                [
                    "node",
                    str(ROOT / "tools" / "build_npm.mjs"),
                    "--out",
                    str(packed),
                    "--target",
                    f"{args.target or host_target()}={args.engine.resolve(strict=True)}",
                ],
                cwd=ROOT,
                env=environment(),
                check=True,
            )
        tarballs = sorted(packed.glob("*.tgz"))
        if len(tarballs) != 2:
            raise HostError(f"{packed} holds {len(tarballs)} tarballs, not @grida/fx and an engine")
        tarballs.append(pack_fixture(work / "host"))
        grida = install(tarballs, work / "prefix")
        env = environment()
        python = os.environ.get("GRIDA_FX_PYTHON")
        if python:
            env["GRIDA_FX_PYTHON"] = os.path.abspath(python) if os.sep in python else python
        check(grida, version, work, env)
    print("check_host: grida fx holds to spec/distribution.md")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (HostError, subprocess.CalledProcessError) as error:
        print(f"check_host: {error}", file=sys.stderr)
        sys.exit(1)
