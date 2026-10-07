#!/usr/bin/env python3
"""Validate an FX release manifest offline, or explicitly publish its verified artifacts.

    python tools/publish_local.py --manifest /path/to/release-manifest.json
    python tools/publish_local.py --manifest /path/to/release-manifest.json --publish
    python tools/publish_local.py --manifest /path/to/release-manifest.json --only pypi --publish

The default only validates files and prints a plan. Publishing uses existing npm authentication
and uv's PyPI token prompt or user-managed token environment. This tool never opens .env,
stores credentials, invokes a shell, or publishes an unverified packaging result.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tarfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from dataclasses import dataclass
from email.parser import BytesParser
from pathlib import Path, PurePosixPath
from typing import Any, BinaryIO

MANIFEST_KIND = "fx-local-release-v1"
REQUIRED_CHECKS = frozenset({"wheel_conformance", "npm_conformance", "wheel_viewer", "npm_viewer"})
NPM_REGISTRY = "https://registry.npmjs.org"
PYPI_REGISTRY = "https://pypi.org"
PYPI_UPLOAD = "https://upload.pypi.org/legacy/"
MAX_METADATA = 2 * 1024 * 1024
MAX_ARCHIVE_CONTENT = 1024 * 1024 * 1024
VERSION = re.compile(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)-(alpha|beta|rc)\.(0|[1-9]\d*)")
SHA256 = re.compile(r"[0-9a-f]{64}")
TARGETS = {
    "aarch64-apple-darwin": (
        "@grida/fx-darwin-arm64",
        "darwin",
        "arm64",
        None,
        "macosx_11_0_arm64",
    ),
    "x86_64-apple-darwin": ("@grida/fx-darwin-x64", "darwin", "x64", None, "macosx_10_12_x86_64"),
    "x86_64-unknown-linux-gnu": (
        "@grida/fx-linux-x64-gnu",
        "linux",
        "x64",
        "glibc",
        "manylinux_2_28_x86_64",
    ),
    "aarch64-unknown-linux-gnu": (
        "@grida/fx-linux-arm64-gnu",
        "linux",
        "arm64",
        "glibc",
        "manylinux_2_28_aarch64",
    ),
}


class PublishError(ValueError):
    """The requested release cannot be published safely."""


@dataclass(frozen=True)
class Artifact:
    kind: str
    name: str
    version: str
    path: Path
    sha256: str


@dataclass(frozen=True)
class Release:
    manifest: Path
    version: str
    python_version: str
    target: str
    binary_sha256: str
    verified: bool
    checks: frozenset[str]
    artifacts: tuple[Artifact, ...]


def fail(reason: str) -> None:
    raise PublishError(reason)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def embedded_digest(stream: BinaryIO) -> str:
    digest = hashlib.sha256()
    total = 0
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        total += len(block)
        if total > MAX_ARCHIVE_CONTENT:
            fail("embedded engine exceeds the inspection limit")
        digest.update(block)
    return digest.hexdigest()


def refuse_symlinks(path: Path) -> None:
    for candidate in (path, *path.parents):
        if candidate.is_symlink():
            fail("manifest and artifact paths must not contain symbolic links")


def relative_path(value: Any) -> PurePosixPath:
    if not isinstance(value, str) or not value or "\\" in value:
        fail("artifact filename must be a portable relative path")
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or path.as_posix() != value
        or any(part in {".", ".."} for part in path.parts)
        or any(ord(char) < 32 for char in value)
        or ":" in value
    ):
        fail("artifact filename must be a normalized relative path without traversal")
    return path


def checked_file(root: Path, value: Any) -> Path:
    relative = relative_path(value)
    path = root.joinpath(*relative.parts)
    refuse_symlinks(path)
    if not path.is_file() or not path.resolve().is_relative_to(root):
        fail("artifact must be an existing regular file inside the manifest directory")
    return path


def json_object(data: bytes, what: str) -> dict[str, Any]:
    try:
        value = json.loads(data)
    except (ValueError, UnicodeDecodeError):
        fail(f"{what} is not valid JSON")
    if not isinstance(value, dict):
        fail(f"{what} must be a JSON object")
    return value


def archive_name(name: str) -> None:
    # Archives are inspected in place; nothing is extracted to the filesystem.
    relative_path(name.rstrip("/"))


def validate_wheel(artifact: Artifact, target: str, binary_sha256: str) -> None:
    tag = f"py3-none-{TARGETS[target][4]}"
    expected = f"grida-{artifact.version}-{tag}.whl"
    if artifact.path.name != expected:
        fail("wheel filename does not match the declared version and target")
    prefix = f"grida-{artifact.version}.dist-info/"
    with zipfile.ZipFile(artifact.path) as archive:
        entries = archive.infolist()
        if len({entry.filename for entry in entries}) != len(entries):
            fail("wheel contains duplicate entries")
        if sum(entry.file_size for entry in entries) > MAX_ARCHIVE_CONTENT:
            fail("wheel's uncompressed contents exceed the inspection limit")
        for entry in entries:
            archive_name(entry.filename)
            if stat.S_ISLNK(entry.external_attr >> 16):
                fail("wheel contains a symbolic link")
        metadata_entries = [entry for entry in entries if entry.filename.endswith("/METADATA")]
        if len(metadata_entries) != 1 or metadata_entries[0].filename != prefix + "METADATA":
            fail("wheel must contain exactly its declared distribution metadata")
        for name in (prefix + "METADATA", prefix + "WHEEL"):
            if archive.getinfo(name).file_size > MAX_METADATA:
                fail("wheel metadata exceeds the inspection limit")
        metadata = BytesParser().parsebytes(archive.read(prefix + "METADATA"))
        wheel = BytesParser().parsebytes(archive.read(prefix + "WHEEL"))
        if metadata.get_all("Name") != ["grida"] or metadata.get_all("Version") != [
            artifact.version
        ]:
            fail("wheel distribution identity disagrees with the manifest")
        if wheel.get_all("Tag") != [tag] or wheel.get("Root-Is-Purelib", "").lower() != "false":
            fail("wheel platform metadata disagrees with the manifest")
        engine = archive.getinfo("grida/fx/_bin/grida-fx")
        if engine.file_size == 0 or not (engine.external_attr >> 16) & 0o111:
            fail("wheel does not carry an executable engine")
        with archive.open(engine) as stream:
            if embedded_digest(stream) != binary_sha256:
                fail("wheel's embedded engine disagrees with binary_sha256")


def validate_npm(artifact: Artifact, target: str, binary_sha256: str) -> None:
    expected = artifact.name.removeprefix("@").replace("/", "-")
    if artifact.path.name != f"{expected}-{artifact.version}.tgz":
        fail("npm tarball filename does not match the declared package identity")
    entries: dict[str, tarfile.TarInfo] = {}
    total = 0
    engine_digest = None
    with tarfile.open(artifact.path, "r:gz") as archive:
        for entry in archive:
            archive_name(entry.name)
            if entry.name in entries:
                fail("npm tarball contains duplicate entries")
            if not entry.name.startswith("package/"):
                fail("npm tarball contains entries outside package/")
            if not entry.isfile() and not entry.isdir():
                fail("npm tarball contains a link or special file")
            entries[entry.name] = entry
            total += entry.size
            if total > MAX_ARCHIVE_CONTENT:
                fail("npm tarball's contents exceed the inspection limit")
        package = entries.get("package/package.json")
        if package is None or not package.isfile() or package.size > MAX_METADATA:
            fail("npm tarball must contain bounded package metadata")
        stream = archive.extractfile(package)
        if stream is None:
            fail("npm package metadata cannot be read")
        manifest = json_object(stream.read(MAX_METADATA + 1), "npm package metadata")
        if artifact.name != "@grida/fx":
            engine = entries.get("package/bin/grida-fx")
            if engine is not None and engine.isfile():
                stream = archive.extractfile(engine)
                if stream is None:
                    fail("npm engine cannot be read")
                with stream:
                    engine_digest = embedded_digest(stream)
    if manifest.get("name") != artifact.name or manifest.get("version") != artifact.version:
        fail("npm package identity disagrees with the manifest")
    if manifest.get("private") is True or manifest.get("scripts"):
        fail("release npm packages must be public and have no lifecycle scripts")
    if manifest.get("publishConfig") != {"access": "public", "tag": "next"}:
        fail("npm package must declare public preview publishing under next")
    engine_name, os_name, cpu, libc, _ = TARGETS[target]
    if artifact.name == "@grida/fx":
        pins = manifest.get("optionalDependencies", {})
        if not isinstance(pins, dict) or any(
            pins.get(platform[0]) != artifact.version for platform in TARGETS.values()
        ):
            fail("SDK must pin each platform engine to the declared version")
        if manifest.get("bin") != {"grida-fx": "bin/grida-fx.js"}:
            fail("SDK command declaration is missing or unexpected")
        executable = "package/bin/grida-fx.js"
        if "package/dist/index.js" not in entries or "package/dist/index.d.ts" not in entries:
            fail("SDK compiled entry point and declarations are missing")
    else:
        if artifact.name != engine_name:
            fail("npm engine package is not the manifest target's engine")
        if manifest.get("os") != [os_name] or manifest.get("cpu") != [cpu]:
            fail("npm engine platform metadata disagrees with the target")
        if (libc is not None and manifest.get("libc") != [libc]) or (
            libc is None and "libc" in manifest
        ):
            fail("npm engine C library metadata disagrees with the target")
        executable = "package/bin/grida-fx"
    entry = entries.get(executable)
    if entry is None or not entry.isfile() or entry.size == 0 or not entry.mode & 0o111:
        fail("npm package's executable command is missing")
    if artifact.name != "@grida/fx" and engine_digest != binary_sha256:
        fail("npm package's embedded engine disagrees with binary_sha256")


def load_manifest(path: Path) -> Release:
    path = path.absolute()
    refuse_symlinks(path)
    if not path.is_file() or path.stat().st_size > MAX_METADATA:
        fail("manifest must be an existing bounded regular file")
    path = path.resolve()
    value = json_object(path.read_bytes(), "release manifest")
    if value.get("kind") != MANIFEST_KIND:
        fail(f"manifest kind must be {MANIFEST_KIND}")
    version = value.get("version")
    match = VERSION.fullmatch(version) if isinstance(version, str) else None
    if match is None:
        fail("manifest version must be a canonical alpha, beta or rc preview")
    major, minor, patch, phase, number = match.groups()
    python_version = (
        f"{major}.{minor}.{patch}{ {'alpha': 'a', 'beta': 'b', 'rc': 'rc'}[phase] }{number}"
    )
    if value.get("python_version") != python_version:
        fail("Python and npm preview versions disagree")
    target = value.get("target")
    if not isinstance(target, str) or target not in TARGETS:
        fail("manifest target is not a supported preview target")
    binary_sha256 = value.get("binary_sha256")
    if not isinstance(binary_sha256, str) or SHA256.fullmatch(binary_sha256) is None:
        fail("manifest binary_sha256 must be a lowercase SHA-256 digest")
    if not isinstance(value.get("verified"), bool):
        fail("manifest verified must be a boolean")
    checks = value.get("checks")
    if not isinstance(checks, list) or not all(isinstance(check, str) for check in checks):
        fail("manifest checks must be an array of check names")
    source = value.get("source")
    if (
        not isinstance(source, dict)
        or not isinstance(source.get("head"), str)
        or re.fullmatch(r"[0-9a-f]{40}", source["head"]) is None
        or not isinstance(source.get("sha256"), str)
        or SHA256.fullmatch(source["sha256"]) is None
    ):
        fail("manifest must identify the source commit and source snapshot digest")
    records = value.get("artifacts")
    if not isinstance(records, list) or len(records) != 3:
        fail("manifest must contain exactly one wheel, one engine and one SDK tarball")
    artifacts = []
    identities = set()
    paths = set()
    expected_identities = {("wheel", "grida"), ("npm", "@grida/fx"), ("npm", TARGETS[target][0])}
    for record in records:
        if not isinstance(record, dict):
            fail("artifact record must be an object")
        kind, name = record.get("kind"), record.get("name")
        if not isinstance(kind, str) or not isinstance(name, str):
            fail("artifact identity must contain string kind and name")
        identity = (kind, name)
        if identity not in expected_identities or identity in identities:
            fail("manifest has an unexpected or repeated artifact identity")
        identities.add(identity)
        expected_version = python_version if kind == "wheel" else version
        if record.get("version") != expected_version:
            fail("artifact version disagrees with the release")
        digest = record.get("sha256")
        if not isinstance(digest, str) or SHA256.fullmatch(digest) is None:
            fail("artifact sha256 must be a lowercase SHA-256 digest")
        artifact_path = checked_file(path.parent, record.get("filename"))
        if artifact_path in paths:
            fail("manifest repeats an artifact file")
        paths.add(artifact_path)
        if sha256(artifact_path) != digest:
            fail(f"artifact digest disagrees with the manifest: {artifact_path.name}")
        artifact = Artifact(kind, name, expected_version, artifact_path, digest)
        if kind == "wheel":
            validate_wheel(artifact, target, binary_sha256)
        else:
            validate_npm(artifact, target, binary_sha256)
        artifacts.append(artifact)
    order = {TARGETS[target][0]: 0, "@grida/fx": 1, "grida": 2}
    artifacts.sort(key=lambda artifact: order[artifact.name])
    return Release(
        path,
        version,
        python_version,
        target,
        binary_sha256,
        value["verified"],
        frozenset(checks),
        tuple(artifacts),
    )


def selected_artifacts(release: Release, only: str = "all") -> tuple[Artifact, ...]:
    if only not in {"all", "pypi", "npm"}:
        fail("publication selection must be all, pypi or npm")
    return tuple(
        artifact
        for artifact in release.artifacts
        if only == "all" or artifact.kind == ("wheel" if only == "pypi" else "npm")
    )


def commands(release: Release, only: str = "all") -> list[list[str]]:
    result = []
    for artifact in selected_artifacts(release, only):
        if artifact.kind == "npm":
            result.append(
                [
                    "npm",
                    "publish",
                    str(artifact.path),
                    "--tag",
                    "next",
                    "--access",
                    "public",
                    "--ignore-scripts",
                    "--registry",
                    NPM_REGISTRY,
                ]
            )
        else:
            result.append(
                [
                    "uv",
                    "--no-config",
                    "publish",
                    "--publish-url",
                    PYPI_UPLOAD,
                    "--trusted-publishing",
                    "never",
                    "--username",
                    "__token__",
                    "--no-attestations",
                    str(artifact.path),
                ]
            )
    return result


def require_verified(release: Release) -> None:
    missing = REQUIRED_CHECKS - release.checks
    if not release.verified or missing:
        fail("publication requires verified=true and all four installed-package checks")


def refuse_existing(artifact: Artifact) -> None:
    if artifact.kind == "npm":
        encoded = urllib.parse.quote(artifact.name, safe="")
        url = f"{NPM_REGISTRY}/{encoded}/{artifact.version}"
    else:
        url = f"{PYPI_REGISTRY}/pypi/{artifact.name}/{artifact.version}/json"
    request = urllib.request.Request(url, headers={"User-Agent": "grida-fx-local-release/1"})
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            if response.status == 200:
                fail(f"{artifact.name}@{artifact.version} already exists; no files were uploaded")
            fail(f"unexpected registry response for {artifact.name}; publication refused")
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return
        fail(f"registry check failed for {artifact.name} (HTTP {error.code}); publication refused")
    except (urllib.error.URLError, TimeoutError, OSError):
        fail(f"registry check failed for {artifact.name}; publication refused")


def publish(release: Release, only: str = "all") -> None:
    require_verified(release)
    artifacts = selected_artifacts(release, only)
    programs = {"npm" if artifact.kind == "npm" else "uv" for artifact in artifacts}
    for program in sorted(programs):
        if shutil.which(program) is None:
            fail(f"{program} is required for local publication")
    for artifact in artifacts:
        refuse_existing(artifact)
    # Revalidate after the network preflight and before each irreversible upload.
    current = load_manifest(release.manifest)
    if current != release:
        fail("release files changed during preflight; publication refused")
    for artifact, command in zip(artifacts, commands(release, only), strict=True):
        current = load_manifest(release.manifest)
        if current != release:
            fail("release files changed before upload; publication stopped")
        print(f"Publishing {artifact.name}@{artifact.version}", flush=True)
        result = subprocess.run(command, check=False)
        if result.returncode != 0:
            fail(
                f"publication failed for {artifact.name}; earlier packages may already exist. "
                "Inspect the registries before attempting another release."
            )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--manifest", type=Path, required=True, help="packaging output manifest")
    parser.add_argument(
        "--only",
        choices=("all", "pypi", "npm"),
        default="all",
        help="select registries to plan or publish (default: all)",
    )
    parser.add_argument("--publish", action="store_true", help="explicitly upload verified files")
    args = parser.parse_args(argv)
    try:
        release = load_manifest(args.manifest)
        print(f"Release {release.version} (PyPI {release.python_version}), {release.target}")
        for artifact in selected_artifacts(release, args.only):
            print(f"  {artifact.name}@{artifact.version}  sha256 {artifact.sha256}")
        if not args.publish:
            print("Offline plan only: nothing uploaded and no authentication requested.")
            for command in commands(release, args.only):
                print(f"  {shlex.join(command)}")
            if not release.verified or not REQUIRED_CHECKS <= release.checks:
                print("Publication is disabled until all installed-package checks are verified.")
            return 0
        publish(release, args.only)
    except (
        PublishError,
        OSError,
        tarfile.TarError,
        zipfile.BadZipFile,
        KeyError,
        UnicodeDecodeError,
    ) as error:
        print(f"publish_local: {error}", file=sys.stderr)
        return 1
    count = len(selected_artifacts(release, args.only))
    noun = "artifact" if count == 1 else "artifacts"
    print(f"Published {count} selected {noun}. Verify fresh registry installations next.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
