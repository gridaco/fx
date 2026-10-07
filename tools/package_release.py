"""Package one compiled release target for both Python and npm, without publishing.

    python tools/package_release.py --binary <grida-fx> --target <triple> --out <new-folder>

Add --verify to install the artifacts in clean environments and run strict
conformance and embedded-viewer checks. CI can supply its platform-built binary
and use this same command. The output manifest pins every artifact by SHA-256.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGETS = {
    "aarch64-apple-darwin": "@grida/fx-darwin-arm64",
    "x86_64-apple-darwin": "@grida/fx-darwin-x64",
    "x86_64-unknown-linux-gnu": "@grida/fx-linux-x64-gnu",
    "aarch64-unknown-linux-gnu": "@grida/fx-linux-arm64-gnu",
}
CHECKS = ("wheel_conformance", "npm_conformance", "wheel_viewer", "npm_viewer")


def clean_environment() -> dict[str, str]:
    environment = dict(os.environ)
    for name in (
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
        "GRIDA_FX_BIN",
        "PYTHONPATH",
        "PYTHONHOME",
    ):
        environment.pop(name, None)
    environment.update(GRIDA_FX_DISABLE_DOTENV="1", GRIDA_FX_NETWORK="off")
    return environment


def run(arguments: list[str], *, cwd: Path = ROOT, env: dict[str, str] | None = None) -> None:
    subprocess.run(arguments, cwd=cwd, env=env or clean_environment(), check=True)


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def check_embedded_engines(wheel: Path, engine: Path, expected: str) -> None:
    with zipfile.ZipFile(wheel) as archive, archive.open("grida/fx/_bin/grida-fx") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != expected:
            raise ValueError("wheel carries a different engine from the input binary")
    with tarfile.open(engine, "r:gz") as archive:
        stream = archive.extractfile("package/bin/grida-fx")
        if stream is None:
            raise ValueError("npm tarball has no engine")
        with stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != expected:
                raise ValueError("npm tarball carries a different engine from the input binary")


def prepare_output(out: Path, root: Path = ROOT) -> Path:
    out = out.absolute()
    if out != out.resolve():
        raise ValueError("output must not traverse symlinks")
    root = root.resolve()
    if out == root or out in root.parents:
        raise ValueError("output must not contain the source checkout")
    if root in out.parents:
        ignored = subprocess.run(
            ["git", "check-ignore", "--quiet", str(out)], cwd=root, check=False
        )
        if ignored.returncode != 0:
            raise ValueError("output inside the checkout must be gitignored")
    out.mkdir(parents=True, exist_ok=False)
    return out


def source_snapshot(root: Path = ROOT) -> dict[str, object]:
    """Fingerprint Git-visible source, including uncommitted files, without dotenv."""
    names = (
        subprocess.check_output(
            ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=root
        )
        .decode()
        .split("\0")
    )
    files: dict[str, str] = {}
    for name in sorted(set(filter(None, names))):
        path = root / name
        if path.name == ".env" or (path.name.startswith(".env.") and path.name != ".env.example"):
            continue
        if path.is_symlink():
            files[name] = hashlib.sha256(os.readlink(path).encode()).hexdigest()
        elif path.is_file():
            files[name] = digest(path)
        else:
            files[name] = "missing"
    encoded = json.dumps(files, sort_keys=True, separators=(",", ":")).encode()
    return {
        "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root).decode().strip(),
        "sha256": hashlib.sha256(encoded).hexdigest(),
        "files": files,
    }


def verify(wheel: Path, npm: list[Path], out: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="fx-release-check-", dir=out.parent) as folder:
        temporary = Path(folder)
        venv = temporary / "python"
        run(["uv", "--no-config", "venv", "--python", sys.executable, str(venv)], cwd=temporary)
        python = venv / "bin" / "python"
        run(
            [
                "uv",
                "--no-config",
                "pip",
                "install",
                "--python",
                str(python),
                "--index-url",
                "https://pypi.org/simple",
                str(wheel),
            ],
            cwd=temporary,
        )
        prefix = temporary / "npm"
        run(
            [
                "npm",
                "install",
                "--prefix",
                str(prefix),
                "--no-save",
                "--no-audit",
                "--no-fund",
                "--ignore-scripts",
                "--registry",
                "https://registry.npmjs.org",
                *map(str, npm),
            ],
            cwd=temporary,
        )
        npm_bin = prefix / "node_modules" / ".bin" / "grida-fx"
        environment = clean_environment()
        environment["GRIDA_FX_PYTHON"] = str(python)
        for command in ([str(python), "-m", "grida.fx"], [str(npm_bin)]):
            run([*command, "--version"], cwd=temporary, env=environment)
            run(
                [
                    sys.executable,
                    str(ROOT / "conformance" / "run.py"),
                    "--strict",
                    "--command",
                    shlex.join(command),
                ],
                cwd=temporary,
                env=environment,
            )
            run(
                [
                    sys.executable,
                    str(ROOT / "tools" / "check_viewer.py"),
                    "--command",
                    shlex.join(command),
                ],
                cwd=temporary,
                env=environment,
            )


def package(binary: Path, target: str, out: Path, *, installed_checks: bool) -> Path:
    binary = binary.resolve(strict=True)
    run([sys.executable, str(ROOT / "tools" / "check_versions.py"), "--prerelease"])
    version = (
        subprocess.check_output(
            [
                sys.executable,
                str(ROOT / "tools" / "check_versions.py"),
                "--print",
                "semver",
            ],
            cwd=ROOT,
        )
        .decode()
        .strip()
    )
    python_version = (
        subprocess.check_output(
            [
                sys.executable,
                str(ROOT / "tools" / "check_versions.py"),
                "--print",
                "pep440",
            ],
            cwd=ROOT,
        )
        .decode()
        .strip()
    )
    source = source_snapshot()
    binary_hash = digest(binary)
    out = prepare_output(out)
    run(
        [
            sys.executable,
            str(ROOT / "tools" / "build_wheel.py"),
            "--binary",
            str(binary),
            "--target",
            target,
            "--out",
            str(out / "wheels"),
        ]
    )
    run(
        [
            "node",
            str(ROOT / "tools" / "build_npm.mjs"),
            "--out",
            str(out / "npm"),
            "--target",
            f"{target}={binary}",
        ]
    )
    wheels = list((out / "wheels").glob("*.whl"))
    tarballs = sorted((out / "npm").glob("*.tgz"))
    if len(wheels) != 1 or len(tarballs) != 2:
        raise ValueError("one wheel and two npm tarballs are required for a single target")
    artifact_hashes = {path: digest(path) for path in [*wheels, *tarballs]}
    engine_name = f"{TARGETS[target].removeprefix('@').replace('/', '-')}-{version}.tgz"
    check_embedded_engines(wheels[0], out / "npm" / engine_name, binary_hash)
    if installed_checks:
        verify(wheels[0], tarballs, out)
    if source_snapshot() != source:
        raise ValueError("source changed during packaging; prepare a new artifact set")
    if digest(binary) != binary_hash:
        raise ValueError("input binary changed during packaging; prepare a new artifact set")
    if any(digest(path) != expected for path, expected in artifact_hashes.items()):
        raise ValueError("artifact changed during verification; prepare a new artifact set")
    artifacts = [
        {
            "kind": "wheel",
            "name": "grida",
            "version": python_version,
            "filename": wheels[0].relative_to(out).as_posix(),
            "sha256": artifact_hashes[wheels[0]],
        }
    ]
    for name in (TARGETS[target], "@grida/fx"):
        filename = f"{name.removeprefix('@').replace('/', '-')}-{version}.tgz"
        path = out / "npm" / filename
        artifacts.append(
            {
                "kind": "npm",
                "name": name,
                "version": version,
                "filename": path.relative_to(out).as_posix(),
                "sha256": artifact_hashes[path],
            }
        )
    manifest = out / "release-manifest.json"
    manifest.write_text(
        json.dumps(
            {
                "kind": "fx-local-release-v1",
                "version": version,
                "python_version": python_version,
                "target": target,
                "source": source,
                "binary_sha256": binary_hash,
                "verified": installed_checks,
                "checks": list(CHECKS) if installed_checks else [],
                "artifacts": artifacts,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--out", type=Path, required=True, help="new output directory")
    parser.add_argument("--verify", action="store_true", help="check clean installs before handoff")
    args = parser.parse_args()
    try:
        print(package(args.binary, args.target, args.out, installed_checks=args.verify))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"package_release: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
