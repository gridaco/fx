"""Builds a ``grida`` wheel that carries the ``grida-fx`` engine (milestone 1, step 5).

    uv run --project python python tools/build_wheel.py \\
        --binary target/release/grida-fx --target aarch64-apple-darwin --out dist/

prints the wheel's path. The published ``grida`` wheels come from this script only, one per
platform, each with the engine at ``grida/fx/_bin/grida-fx``. There is no console command
(``python -m grida.fx <verb>`` runs the engine) and no sdist, which could not run. ``uv build`` in
``python/`` makes a wheel without the engine, for development only.

- **Targets.** ``aarch64-apple-darwin``, ``x86_64-apple-darwin``, ``x86_64-unknown-linux-gnu`` and
  ``aarch64-unknown-linux-gnu``. Windows is not in the preview: the runner uses Unix process
  groups and signals.
- **Contents.** Every file of the packages ``python/pyproject.toml`` names under
  ``[tool.hatch.build.targets.wheel]`` except ``__pycache__``, compiled files, dotfiles and
  ``grida/fx/_bin/``; the binary at ``grida/fx/_bin/grida-fx`` with mode 0o755 (every other file
  0o644); then ``METADATA``, ``WHEEL``, license files and ``RECORD`` (sha256 and size of every
  other entry). License files live in ``.dist-info/licenses/``.
- **Metadata** is Core Metadata 2.4 from ``[project]``: ``Name``, ``Version``, ``Summary``,
  ``Project-URL``, ``Requires-Python``, one ``Requires-Dist`` per dependency,
  ``License-Expression``, ``License-File``, and the readme as the description. A ``[project]``
  key this script does not write is refused, so nothing is dropped silently.
  ``WHEEL`` says ``Root-Is-Purelib: false`` and the one tag
  ``py3-none-<platform>``.
- **The platform tag is read from the binary**, so it is honest for it:
  - macOS: a thin 64-bit Mach-O of the target's architecture that links only system libraries
    (``/usr/lib/``, ``/System/Library/``). Its minimum macOS (``LC_BUILD_VERSION``, or
    ``LC_VERSION_MIN_MACOSX``) is the tag: 11.0 gives ``macosx_11_0_arm64``, 10.12
    ``macosx_10_12_x86_64``. From macOS 11 installers match only ``<major>_0``, so a minimum past
    ``<major>.0`` takes the next major's tag.
  - Linux: a 64-bit little-endian ELF of the target's architecture that needs only libraries
    every manylinux system has, and no glibc symbol version past 2.28: ``manylinux_2_28_<arch>``.
    Build it with ``cargo zigbuild --target <target>.2.28``.
- **Deterministic.** The package files and the binary sorted by name, then the .dist-info with
  ``RECORD`` last; every timestamp 1980-01-01 00:00; Unix modes; DEFLATE at level 9. The same
  inputs give the same bytes with the same zlib.

Exit status 1 with ``build_wheel: <reason>`` on stderr when the wheel cannot be built honestly.
"""

from __future__ import annotations

import argparse
import base64
import csv
import hashlib
import io
import os
import re
import stat
import struct
import sys
import tempfile
import tomllib
import zipfile
from collections.abc import Iterator
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
PYTHON = REPO / "python"

#: Where the wheel carries the engine; ``grida.fx._api`` looks for it there.
BINARY_PATH = "grida/fx/_bin/grida-fx"
_BINARY_FOLDER = BINARY_PATH.rsplit("/", 1)[0] + "/"
#: The preview's targets: (operating system, the architecture's name in a platform tag).
TARGETS = {
    "aarch64-apple-darwin": ("macos", "arm64"),
    "x86_64-apple-darwin": ("macos", "x86_64"),
    "x86_64-unknown-linux-gnu": ("linux", "x86_64"),
    "aarch64-unknown-linux-gnu": ("linux", "aarch64"),
}
#: The newest glibc a Linux binary may need, and the manylinux tag that promises it.
GLIBC_BASELINE = (2, 28)
#: The libraries a manylinux_2_28 system provides that a Rust binary may link.
MANYLINUX_LIBRARIES = frozenset(
    {
        "libc.so.6",
        "libm.so.6",
        "libdl.so.2",
        "librt.so.1",
        "libpthread.so.0",
        "libutil.so.1",
        "libresolv.so.2",
        "libnsl.so.1",
        "libgcc_s.so.1",
        "libstdc++.so.6",
        "ld-linux-x86-64.so.2",
        "ld-linux-aarch64.so.1",
    }
)
#: Where a macOS binary's libraries may live: the system's own.
MACOS_SYSTEM_PREFIXES = ("/usr/lib/", "/System/Library/")
#: Every entry's timestamp: the earliest a zip can hold.
TIMESTAMP = (1980, 1, 1, 0, 0, 0)
GENERATOR = "grida-fx tools/build_wheel.py"
#: The ``[project]`` keys written into METADATA.
PROJECT_KEYS = frozenset(
    {
        "name",
        "version",
        "description",
        "readme",
        "requires-python",
        "dependencies",
        "urls",
        "license",
        "license-files",
    }
)
_CANONICAL_VERSION = re.compile(r"\d+(\.\d+)*((a|b|rc)\d+)?(\.post\d+)?(\.dev\d+)?")
_README_TYPES = {".md": "text/markdown", ".rst": "text/x-rst", ".txt": "text/plain"}

# Mach-O
_MH_MAGIC_64 = 0xFEEDFACF
#: A universal binary's magic (big-endian on disk), read little-endian, 32- and 64-bit.
_FAT_MAGICS = (0xBEBAFECA, 0xBFBAFECA)
_CPU_TYPES = {"arm64": 0x0100000C, "x86_64": 0x01000007}
_LC_VERSION_MIN_MACOSX = 0x24
_LC_BUILD_VERSION = 0x32
_PLATFORM_MACOS = 1
_DYLIB_COMMANDS = frozenset(
    {
        0x0C,  # LC_LOAD_DYLIB
        0x20,  # LC_LAZY_LOAD_DYLIB
        0x80000018,  # LC_LOAD_WEAK_DYLIB
        0x8000001F,  # LC_REEXPORT_DYLIB
        0x80000023,  # LC_LOAD_UPWARD_DYLIB
    }
)

# ELF
_EM_MACHINES = {"x86_64": 62, "aarch64": 183}
_SHT_DYNAMIC = 6
_SHT_GNU_VERNEED = 0x6FFFFFFE
_DT_NULL = 0
_DT_NEEDED = 1
_GLIBC_VERSION = re.compile(r"GLIBC_(\d+(?:\.\d+)*)")


class BuildError(Exception):
    """The wheel cannot be built honestly: the reason."""


# ------------------------------------------------------------------------------------------------
# The platform tag


def platform_tag(binary: bytes, target: str) -> str:
    """The wheel platform tag for ``binary`` built for ``target`` (module docstring)."""
    if target not in TARGETS:
        known = ", ".join(TARGETS)
        reason = f"{target} is not a preview target ({known})"
        if "windows" in target:
            reason += "; Windows is not in the preview: the runner uses Unix process groups"
            reason += " and signals"
        raise BuildError(reason)
    system, arch = TARGETS[target]
    try:
        if system == "macos":
            return _macos_tag(binary, arch)
        return _linux_tag(binary, arch)
    except (struct.error, IndexError, ValueError, UnicodeDecodeError) as error:
        raise BuildError(f"the binary is truncated or malformed ({error})") from None


def _macos_tag(data: bytes, arch: str) -> str:
    (magic,) = struct.unpack_from("<I", data, 0)
    if magic in _FAT_MAGICS:
        raise BuildError("the binary is a universal Mach-O; build one per architecture")
    if magic != _MH_MAGIC_64:
        raise BuildError("the binary is not a 64-bit Mach-O file")
    cpu_type, _, _, commands, _ = struct.unpack_from("<iiIII", data, 4)
    if cpu_type != _CPU_TYPES[arch]:
        names = {code: name for name, code in _CPU_TYPES.items()}
        built = names.get(cpu_type, f"CPU type {cpu_type:#x}")
        raise BuildError(f"the binary is built for {built}, not {arch}")
    minimum: tuple[int, int, int] | None = None
    libraries: list[str] = []
    offset = 32
    for _ in range(commands):
        command, size = struct.unpack_from("<II", data, offset)
        if size < 8 or offset + size > len(data):
            raise BuildError("the binary's load commands are truncated")
        if command == _LC_BUILD_VERSION:
            platform, packed = struct.unpack_from("<II", data, offset + 8)
            if platform != _PLATFORM_MACOS:
                raise BuildError(f"the binary is built for platform {platform}, not macOS")
            minimum = max(minimum or (0, 0, 0), _macos_version(packed))
        elif command == _LC_VERSION_MIN_MACOSX:
            (packed,) = struct.unpack_from("<I", data, offset + 8)
            minimum = max(minimum or (0, 0, 0), _macos_version(packed))
        elif command in _DYLIB_COMMANDS:
            (name_at,) = struct.unpack_from("<I", data, offset + 8)
            name = data[offset + name_at : offset + size].split(b"\0", 1)[0]
            libraries.append(name.decode("utf-8"))
        offset += size
    foreign = [name for name in libraries if not name.startswith(MACOS_SYSTEM_PREFIXES)]
    if foreign:
        raise BuildError(f"the binary links libraries outside the system: {', '.join(foreign)}")
    if minimum is None:
        raise BuildError("the binary does not say its minimum macOS version")
    major, minor, patch = minimum
    if major >= 11:
        if (minor, patch) != (0, 0):
            major += 1
        return f"macosx_{major}_0_{arch}"
    if patch:
        minor += 1
    return f"macosx_{major}_{minor}_{arch}"


def _macos_version(packed: int) -> tuple[int, int, int]:
    """``xxxx.yy.zz`` packed in nibbles, as Mach-O load commands hold a version."""
    return packed >> 16, (packed >> 8) & 0xFF, packed & 0xFF


def _linux_tag(data: bytes, arch: str) -> str:
    if data[:4] != b"\x7fELF":
        raise BuildError("the binary is not an ELF file")
    if data[4] != 2 or data[5] != 1:
        raise BuildError("the binary is not a 64-bit little-endian ELF file")
    (machine,) = struct.unpack_from("<H", data, 18)
    if machine != _EM_MACHINES[arch]:
        names = {code: name for name, code in _EM_MACHINES.items()}
        built = names.get(machine, f"ELF machine {machine}")
        raise BuildError(f"the binary is built for {built}, not {arch}")
    needed, glibc = _elf_requirements(data)
    foreign = [name for name in needed if name not in MANYLINUX_LIBRARIES]
    if foreign:
        missing = ", ".join(foreign)
        raise BuildError(f"the binary needs libraries manylinux does not have: {missing}")
    newest = max(glibc, default=None)
    if newest is not None and newest > GLIBC_BASELINE:
        baseline = ".".join(map(str, GLIBC_BASELINE))
        raise BuildError(
            f"the binary needs glibc {'.'.join(map(str, newest))}, past manylinux_2_28's"
            f" {baseline}: build it with cargo zigbuild --target <target>.{baseline}"
        )
    return f"manylinux_{GLIBC_BASELINE[0]}_{GLIBC_BASELINE[1]}_{arch}"


def _elf_requirements(data: bytes) -> tuple[list[str], list[tuple[int, ...]]]:
    """The libraries a 64-bit little-endian ELF needs (``DT_NEEDED``) and the glibc versions its
    symbols need (``GLIBC_*`` in ``.gnu.version_r``)."""
    (section_table,) = struct.unpack_from("<Q", data, 0x28)
    entry_size, count = struct.unpack_from("<HH", data, 0x3A)
    if section_table == 0 or count == 0:
        raise BuildError("the binary has no section headers to read its requirements from")
    # (type, offset, size, link, info) of each section header.
    sections = []
    for index in range(count):
        fields = struct.unpack_from("<IIQQQQIIQQ", data, section_table + index * entry_size)
        sections.append((fields[1], fields[4], fields[5], fields[6], fields[7]))

    def strings(index: int) -> bytes:
        _, offset, size, _, _ = sections[index]
        return data[offset : offset + size]

    def string(table: bytes, at: int) -> str:
        return table[at : table.index(b"\0", at)].decode("utf-8")

    needed: list[str] = []
    glibc: list[tuple[int, ...]] = []
    for kind, offset, size, link, info in sections:
        if kind == _SHT_DYNAMIC:
            table = strings(link)
            for at in range(offset, offset + size, 16):
                tag, value = struct.unpack_from("<qQ", data, at)
                if tag == _DT_NULL:
                    break
                if tag == _DT_NEEDED:
                    needed.append(string(table, value))
        elif kind == _SHT_GNU_VERNEED:
            table = strings(link)
            at = offset
            for _ in range(info):
                _, auxiliaries, _, first, following = struct.unpack_from("<HHIII", data, at)
                here = at + first
                for _ in range(auxiliaries):
                    _, _, _, name, step = struct.unpack_from("<IHHII", data, here)
                    match = _GLIBC_VERSION.fullmatch(string(table, name))
                    if match:
                        glibc.append(tuple(int(part) for part in match.group(1).split(".")))
                    here += step
                at += following
    return needed, glibc


# ------------------------------------------------------------------------------------------------
# The metadata


def project_metadata(project: Path) -> tuple[str, str, bytes, list[Path], list[Path]]:
    """The distribution's name and version, its METADATA, and the package folders to ship, from
    ``project``'s ``pyproject.toml`` (module docstring)."""
    pyproject = tomllib.loads((project / "pyproject.toml").read_text(encoding="utf-8"))
    table = pyproject.get("project", {})
    unwritten = sorted(set(table) - PROJECT_KEYS)
    if unwritten:
        raise BuildError(
            f"pyproject.toml [project] has {', '.join(unwritten)}, which build_wheel.py does not"
            " write into METADATA: teach it, or the wheel would drop it"
        )
    name = table.get("name", "")
    version = table.get("version", "")
    if not re.fullmatch(r"[A-Za-z0-9]([A-Za-z0-9._-]*[A-Za-z0-9])?", name):
        raise BuildError(f"pyproject.toml [project] name {name!r} is not a distribution name")
    if not _CANONICAL_VERSION.fullmatch(version):
        raise BuildError(f"pyproject.toml [project] version {version!r} is not canonical PEP 440")
    license_files = project_license_files(project, table.get("license-files", []))
    metadata_version = "2.4" if "license" in table or "license-files" in table else "2.1"
    lines = [f"Metadata-Version: {metadata_version}", f"Name: {name}", f"Version: {version}"]
    if "license" in table:
        license_expression = table["license"]
        if (
            not isinstance(license_expression, str)
            or not license_expression.strip()
            or "\n" in license_expression
            or "\r" in license_expression
        ):
            raise BuildError("pyproject.toml [project] license must be a one-line SPDX expression")
        lines.append(f"License-Expression: {license_expression}")
    lines += [f"License-File: {path.relative_to(project).as_posix()}" for path in license_files]
    summary = table.get("description")
    if summary is not None:
        if "\n" in summary:
            raise BuildError("pyproject.toml [project] description must be one line")
        lines.append(f"Summary: {summary}")
    for label, url in table.get("urls", {}).items():
        lines.append(f"Project-URL: {label}, {url}")
    if "requires-python" in table:
        lines.append(f"Requires-Python: {table['requires-python']}")
    lines += [f"Requires-Dist: {requirement}" for requirement in table.get("dependencies", [])]
    text = "\n".join(lines) + "\n"
    readme = table.get("readme")
    if readme is not None:
        if not isinstance(readme, str):
            raise BuildError("pyproject.toml [project] readme must be a file name")
        path = project / readme
        content_type = _README_TYPES.get(path.suffix.lower())
        if content_type is None:
            raise BuildError(f"the readme {readme} is not .md, .rst or .txt")
        body = path.read_text(encoding="utf-8")
        text += f"Description-Content-Type: {content_type}\n\n{body}"
        if not body.endswith("\n"):
            text += "\n"
    wheel = pyproject.get("tool", {}).get("hatch", {}).get("build", {}).get("targets", {})
    packages = wheel.get("wheel", {}).get("packages")
    if not packages:
        raise BuildError("pyproject.toml names no [tool.hatch.build.targets.wheel] packages")
    return (
        name,
        version,
        text.encode("utf-8"),
        [project / package for package in packages],
        license_files,
    )


def project_license_files(project: Path, patterns: list[str]) -> list[Path]:
    """Resolve declared license files within the project; refuse missing or escaping files."""
    if not isinstance(patterns, list) or any(not isinstance(pattern, str) for pattern in patterns):
        raise BuildError("pyproject.toml [project] license-files must be an array of glob patterns")
    files: set[Path] = set()
    for pattern in patterns:
        if (
            not pattern
            or Path(pattern).is_absolute()
            or ".." in Path(pattern).parts
            or "\\" in pattern
        ):
            raise BuildError(f"license-files pattern {pattern!r} must stay within the project")
        matches = [path for path in project.glob(pattern) if path.is_file()]
        if not matches:
            raise BuildError(f"license-files pattern {pattern!r} matches no files")
        for path in matches:
            if not path.resolve().is_relative_to(project.resolve()):
                raise BuildError(f"license file {path} is outside the project")
            path.read_text(encoding="utf-8")
            files.add(path)
    return sorted(files)


def package_files(folder: Path) -> Iterator[tuple[str, Path]]:
    """Each file of the package ``folder`` to ship, as (its name in the wheel, its path)."""
    root = folder.parent
    for path in sorted(folder.rglob("*")):
        name = path.relative_to(root).as_posix()
        parts = path.relative_to(root).parts
        if any(part.startswith(".") or part == "__pycache__" for part in parts):
            continue
        if path.suffix in (".pyc", ".pyo") or name.startswith(_BINARY_FOLDER):
            continue
        if path.is_symlink():
            raise BuildError(f"{name} is a symbolic link; a wheel holds files")
        if path.is_file():
            yield name, path


# ------------------------------------------------------------------------------------------------
# The wheel


def _record_hash(data: bytes) -> str:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=")
    return "sha256=" + digest.decode("ascii")


def build_wheel(binary: Path, target: str, out: Path, project: Path = PYTHON) -> Path:
    """Builds the wheel for ``binary`` (built for ``target``) into ``out``; returns its path."""
    if not binary.is_file():
        raise BuildError(f"{binary} is not a file")
    engine = binary.read_bytes()
    tag = f"py3-none-{platform_tag(engine, target)}"
    name, version, metadata, packages, license_files = project_metadata(project)
    distribution = re.sub(r"[-_.]+", "_", name).lower()
    dist_info = f"{distribution}-{version}.dist-info"

    entries: list[tuple[str, bytes, int]] = [(BINARY_PATH, engine, 0o755)]
    for package in packages:
        entries += [(arcname, path.read_bytes(), 0o644) for arcname, path in package_files(package)]
    names = [arcname for arcname, _, _ in entries]
    if len(set(names)) != len(names):
        raise BuildError("two files would have the same name in the wheel")
    entries.sort(key=lambda entry: entry[0])
    wheel = (
        f"Wheel-Version: 1.0\nGenerator: {GENERATOR}\nRoot-Is-Purelib: false\nTag: {tag}\n"
    ).encode()
    entries += [(f"{dist_info}/METADATA", metadata, 0o644), (f"{dist_info}/WHEEL", wheel, 0o644)]
    entries += [
        (f"{dist_info}/licenses/{path.relative_to(project).as_posix()}", path.read_bytes(), 0o644)
        for path in license_files
    ]

    record = io.StringIO()
    writer = csv.writer(record, lineterminator="\n")
    for arcname, data, _ in entries:
        writer.writerow([arcname, _record_hash(data), len(data)])
    writer.writerow([f"{dist_info}/RECORD", "", ""])
    entries.append((f"{dist_info}/RECORD", record.getvalue().encode("utf-8"), 0o644))

    out.mkdir(parents=True, exist_ok=True)
    destination = out / f"{distribution}-{version}-{tag}.whl"
    handle, temporary = tempfile.mkstemp(prefix=".build_wheel-", suffix=".whl", dir=out)
    os.close(handle)
    try:
        with zipfile.ZipFile(temporary, "w") as archive:
            for arcname, data, mode in entries:
                info = zipfile.ZipInfo(arcname, date_time=TIMESTAMP)
                info.create_system = 3  # Unix, so installers read the mode
                info.external_attr = (stat.S_IFREG | mode) << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, data, compresslevel=9)
        os.chmod(temporary, 0o644)
        os.replace(temporary, destination)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise
    return destination


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Build a grida wheel that carries the grida-fx engine."
    )
    parser.add_argument("--binary", type=Path, required=True, help="the built grida-fx")
    parser.add_argument("--target", required=True, help=f"its target: {', '.join(TARGETS)}")
    parser.add_argument("--out", type=Path, required=True, help="the folder to write into")
    parser.add_argument("--project", type=Path, default=PYTHON, help="the python/ project")
    args = parser.parse_args(argv)
    try:
        wheel = build_wheel(args.binary, args.target, args.out, args.project)
    except BuildError as error:
        print(f"build_wheel: {error}", file=sys.stderr)
        return 1
    print(wheel)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
