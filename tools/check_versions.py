#!/usr/bin/env python3
"""The version gate: every manifest agrees with the Cargo workspace's version.

The workspace version in Cargo.toml (``[workspace.package] version``) is the one source of truth.
Every other place that states a version repeats it, in its own ecosystem's form:

- each crate under crates/ inherits it (``version.workspace = true``) or states it, and
  Cargo.lock records it for every workspace member;
- python/pyproject.toml states its PEP 440 form (0.1.0-alpha.1 is 0.1.0a1), and python/uv.lock
  records that form for the editable ``grida``;
- js/fx/package.json states it as it is, and pins every ``@grida/fx-*`` package it depends on to
  exactly it, among them every engine package js/fx/src/platforms.ts names; a ``version``
  constant exported from js/fx/src states it too;
- every other package.json under js/ for an ``@grida/fx-*`` package (the per-platform engine
  packages) states it.

    uv run --project python python tools/check_versions.py                       # the tree
    uv run --project python python tools/check_versions.py --tag v0.1.0-alpha.1  # and a tag
    uv run --project python python tools/check_versions.py --prerelease  # it is a pre-release
    uv run --project python python tools/check_versions.py --print pep440  # one form, for scripts
    uv run --project python python tools/check_versions.py --self-check  # this file's doctests

Standard library only: any Python 3.11 or later (for tomllib) runs it. It prints one line per
place it checked and exits 1 when any of them disagrees, naming each.
"""

from __future__ import annotations

import argparse
import doctest
import json
import re
import sys
import tomllib
from collections.abc import Iterator
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PLATFORM_PREFIX = "@grida/fx-"
# What a version may be: SemVer 2.0 with at most one pre-release part that PEP 440 can say.
SEMVER = re.compile(
    r"(?P<release>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))"
    r"(?:-(?P<phase>alpha|beta|rc)\.(?P<number>0|[1-9]\d*))?"
)
PEP440_PHASE = {"alpha": "a", "beta": "b", "rc": "rc"}
EXPORTED_VERSION = re.compile(r"""export\s+const\s+(?:version|VERSION)\s*=\s*["']([^"']*)["']""")
# An engine package named in js/fx/src/platforms.ts, the one list of them.
PLATFORM_PACKAGE = re.compile(r"""\bpackage:\s*["'](@grida/fx-[^"']+)["']""")


class VersionError(ValueError):
    """A version this repository cannot release, or a place that disagrees."""


def parse(version: str) -> re.Match[str]:
    """The parts of a release version.

    >>> parse("0.1.0-alpha.1").group("release", "phase", "number")
    ('0.1.0', 'alpha', '1')
    >>> parse("1.2.3").group("phase") is None
    True
    >>> for bad in ("0.1.0-dev.1", "0.1.0-alpha.01", "0.1.0a1", "v0.1.0"):
    ...     try:
    ...         parse(bad)
    ...     except VersionError as error:
    ...         print(error)
    0.1.0-dev.1 is not MAJOR.MINOR.PATCH with an optional -alpha.N, -beta.N or -rc.N
    0.1.0-alpha.01 is not MAJOR.MINOR.PATCH with an optional -alpha.N, -beta.N or -rc.N
    0.1.0a1 is not MAJOR.MINOR.PATCH with an optional -alpha.N, -beta.N or -rc.N
    v0.1.0 is not MAJOR.MINOR.PATCH with an optional -alpha.N, -beta.N or -rc.N
    """

    match = SEMVER.fullmatch(version)
    if match is None:
        raise VersionError(
            f"{version} is not MAJOR.MINOR.PATCH with an optional -alpha.N, -beta.N or -rc.N"
        )
    return match


def pep440(version: str) -> str:
    """The PEP 440 form of a workspace version, as PyPI and pip write it.

    >>> pep440("0.1.0-alpha.1")
    '0.1.0a1'
    >>> pep440("0.2.0-beta.3")
    '0.2.0b3'
    >>> pep440("1.0.0-rc.2")
    '1.0.0rc2'
    >>> pep440("1.0.0")
    '1.0.0'
    """

    match = parse(version)
    phase = match.group("phase")
    if phase is None:
        return match.group("release")
    return f"{match.group('release')}{PEP440_PHASE[phase]}{match.group('number')}"


def is_prerelease(version: str) -> bool:
    """
    >>> is_prerelease("0.1.0-alpha.1"), is_prerelease("0.1.0")
    (True, False)
    """

    return parse(version).group("phase") is not None


def tag_problem(tag: str, version: str) -> str | None:
    """Why a release tag does not name this version, if it does not.

    >>> tag_problem("v0.1.0-alpha.1", "0.1.0-alpha.1") is None
    True
    >>> tag_problem("refs/tags/v0.1.0-alpha.1", "0.1.0-alpha.1") is None
    True
    >>> tag_problem("v0.1.0a1", "0.1.0-alpha.1")
    'the tag v0.1.0a1 is not v0.1.0-alpha.1'
    """

    name = tag.removeprefix("refs/tags/")
    want = f"v{version}"
    return None if name == want else f"the tag {name} is not {want}"


# --------------------------------------------------------------------------- reading the tree


def workspace_version(root: Path) -> str:
    manifest = _toml(root / "Cargo.toml")
    try:
        version = manifest["workspace"]["package"]["version"]
    except KeyError as error:
        raise VersionError("Cargo.toml has no [workspace.package] version") from error
    parse(version)
    return version


def statements(root: Path, version: str) -> Iterator[tuple[str, str | None, str]]:
    """Every place that states a version: (where, what it says, what it must say). What it says
    is None when the place is missing or says nothing."""

    python_version = pep440(version)
    manifest = _toml(root / "Cargo.toml")
    members: list[str] = []
    for path in sorted((root / "crates").glob("*/Cargo.toml")):
        crate = _toml(path)
        package = crate.get("package", {})
        name = package.get("name", path.parent.name)
        members.append(name)
        stated = package.get("version")
        if stated == {"workspace": True}:
            stated = version
        yield _where(root, path, f"[package] {name}"), _text(stated), version
    for name, dependency in manifest.get("workspace", {}).get("dependencies", {}).items():
        if isinstance(dependency, dict) and "path" in dependency and "version" in dependency:
            where = f"Cargo.toml [workspace.dependencies] {name}"
            yield where, _text(dependency["version"]), f"={version}"

    lock = _toml(root / "Cargo.lock")
    locked = {
        entry["name"]: entry.get("version")
        for entry in lock.get("package", [])
        if "source" not in entry
    }
    for name in members:
        yield f"Cargo.lock {name}", locked.get(name), version

    project = _toml(root / "python" / "pyproject.toml").get("project", {})
    yield "python/pyproject.toml [project] version", _text(project.get("version")), python_version
    uv_lock = _toml(root / "python" / "uv.lock")
    name = project.get("name", "grida")
    editable = [
        entry.get("version")
        for entry in uv_lock.get("package", [])
        if entry.get("name") == name and "editable" in entry.get("source", {})
    ]
    yield f"python/uv.lock {name}", editable[0] if editable else None, python_version

    sdk = root / "js" / "fx" / "package.json"
    package = _json(sdk)
    yield _where(root, sdk, f"{package.get('name')} version"), package.get("version"), version
    for field in ("dependencies", "optionalDependencies", "peerDependencies"):
        for name, wanted in package.get(field, {}).items():
            if name.startswith(PLATFORM_PREFIX):
                yield _where(root, sdk, f"{field} {name}"), wanted, version
    # Each engine package the platform table names must be an optional dependency, or npm never
    # installs it.
    table = root / "js" / "fx" / "src" / "platforms.ts"
    optional = package.get("optionalDependencies", {})
    if table.is_file():
        for name in PLATFORM_PACKAGE.findall(table.read_text(encoding="utf-8")):
            if name not in optional:
                yield _where(root, sdk, f"optionalDependencies {name}"), None, version
    for source in sorted((root / "js" / "fx" / "src").glob("*.ts")):
        if source.name.endswith((".test.ts", ".d.ts")):
            continue
        for match in EXPORTED_VERSION.finditer(source.read_text(encoding="utf-8")):
            yield _where(root, source, "export const version"), match.group(1), version

    for path in platform_manifests(root):
        platform = _json(path)
        where = _where(root, path, f"{platform.get('name')} version")
        yield where, platform.get("version"), version


def platform_manifests(root: Path) -> list[Path]:
    """Every package.json under js/ for an @grida/fx-* package, outside installed and built
    folders."""

    found = []
    for path in sorted((root / "js").rglob("package.json")):
        parts = set(path.relative_to(root).parts)
        if parts & {"node_modules", "dist"}:
            continue
        if str(_json(path).get("name", "")).startswith(PLATFORM_PREFIX):
            found.append(path)
    return found


def _toml(path: Path) -> dict:
    try:
        with path.open("rb") as file:
            return tomllib.load(file)
    except FileNotFoundError as error:
        raise VersionError(f"{path.relative_to(ROOT)} is missing") from error
    except tomllib.TOMLDecodeError as error:
        raise VersionError(f"{path.relative_to(ROOT)} is not TOML: {error}") from error


def _json(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise VersionError(f"{path.relative_to(ROOT)} is missing") from error
    except json.JSONDecodeError as error:
        raise VersionError(f"{path.relative_to(ROOT)} is not JSON: {error}") from error
    if not isinstance(value, dict):
        raise VersionError(f"{path.relative_to(ROOT)} is not a JSON object")
    return value


def _where(root: Path, path: Path, what: str) -> str:
    return f"{path.relative_to(root).as_posix()} {what}"


def _text(value: object) -> str | None:
    return value if isinstance(value, str) else None if value is None else repr(value)


# ----------------------------------------------------------------------------------- main


def self_check() -> int:
    failed, tried = doctest.testmod(sys.modules[__name__], verbose=False)
    print(f"check_versions: self-check, {tried - failed} of {tried} examples pass")
    return 1 if failed else 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--tag", help="a release tag (v<version>) that must name the version")
    parser.add_argument(
        "--prerelease", action="store_true", help="the version must be a pre-release (a preview)"
    )
    parser.add_argument(
        "--print",
        choices=("semver", "pep440"),
        dest="form",
        help="print the workspace version in this form and exit (after the checks pass)",
    )
    parser.add_argument("--self-check", action="store_true", help="run this file's doctests")
    args = parser.parse_args(argv)
    if args.self_check:
        return self_check()

    try:
        version = workspace_version(ROOT)
        places = list(statements(ROOT, version))
    except VersionError as error:
        print(f"FAIL {error}", file=sys.stderr)
        return 1
    problems = []
    for where, says, wants in places:
        if says == wants:
            if args.form is None:
                print(f"ok   {where}: {says}")
        else:
            problems.append(f"{where} says {says or 'nothing'}, not {wants}")
    if args.tag is not None and (problem := tag_problem(args.tag, version)):
        problems.append(problem)
    if args.prerelease and not is_prerelease(version):
        problems.append(f"{version} is not a pre-release, and releases are previews for now")
    for problem in problems:
        print(f"FAIL {problem}", file=sys.stderr)
    if problems:
        print(
            f"check_versions: {len(problems)} disagreement(s) with the Cargo workspace version "
            f"{version}",
            file=sys.stderr,
        )
        return 1
    if args.form is not None:
        print(version if args.form == "semver" else pep440(version))
        return 0
    print(
        f"check_versions: {len(places)} places agree on {version} (PyPI {pep440(version)})"
        + (f", tag {args.tag.removeprefix('refs/tags/')}" if args.tag else "")
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
