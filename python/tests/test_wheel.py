"""Tests for tools/build_wheel.py, which builds the ``grida`` wheels that carry the engine.

The binaries here are synthesized: a Mach-O header with its load commands, or an ELF with its
section headers, ``.dynamic`` and ``.gnu.version_r``; just what the tag is read from.
"""

from __future__ import annotations

import base64
import csv
import email.parser
import hashlib
import importlib.util
import io
import os
import shutil
import stat
import struct
import sys
import tomllib
import zipfile
from pathlib import Path
from typing import Any

import pytest

from grida.fx import _api

REPO = Path(__file__).resolve().parents[2]
PYTHON = REPO / "python"
ARM64 = 0x0100000C
X86_64 = 0x01000007
EM_X86_64 = 62
EM_AARCH64 = 183


def _load_tool() -> Any:
    spec = importlib.util.spec_from_file_location(
        "fx_tools_build_wheel", REPO / "tools" / "build_wheel.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


tool = _load_tool()


def macho(
    cpu: int = ARM64,
    minimum: tuple[int, int, int] | None = (11, 0, 0),
    *,
    legacy: bool = False,
    platform: int = 1,
    libraries: tuple[str, ...] = ("/usr/lib/libSystem.B.dylib",),
    magic: int = 0xFEEDFACF,
) -> bytes:
    """A 64-bit Mach-O header and its load commands."""
    commands = []
    if minimum is not None:
        packed = minimum[0] << 16 | minimum[1] << 8 | minimum[2]
        if legacy:  # LC_VERSION_MIN_MACOSX: version, sdk
            commands.append(struct.pack("<IIII", 0x24, 16, packed, packed))
        else:  # LC_BUILD_VERSION: platform, minos, sdk, ntools
            commands.append(struct.pack("<IIIIII", 0x32, 24, platform, packed, packed, 0))
    for name in libraries:  # LC_LOAD_DYLIB: name offset, timestamp, versions
        raw = name.encode() + b"\0"
        raw += b"\0" * (-len(raw) % 8)
        commands.append(struct.pack("<IIIIII", 0x0C, 24 + len(raw), 24, 2, 0, 0) + raw)
    body = b"".join(commands)
    header = struct.pack("<IiiIIIII", magic, cpu, 0, 2, len(commands), len(body), 0, 0)
    return header + body + b"\0" * 64


def elf(
    machine: int = EM_X86_64,
    needed: tuple[str, ...] = ("libgcc_s.so.1", "libc.so.6"),
    glibc: tuple[str, ...] = ("2.2.5", "2.28"),
    *,
    elf_class: int = 2,
) -> bytes:
    """A 64-bit little-endian ELF with ``.dynstr``, ``.dynamic`` and ``.gnu.version_r``."""
    dynstr = bytearray(b"\0")

    def add(text: str) -> int:
        at = len(dynstr)
        dynstr.extend(text.encode() + b"\0")
        return at

    needed_at = [add(name) for name in needed]
    libc_at = add("libc.so.6")
    versions_at = [add(f"GLIBC_{version}") for version in glibc]
    versions_at.append(add("GCC_3.0"))  # another library's version, not glibc's
    dynamic = b"".join(struct.pack("<qQ", 1, at) for at in needed_at) + struct.pack("<qQ", 0, 0)
    auxiliaries = b"".join(
        struct.pack("<IHHII", 0, 0, index + 2, at, 16 if index < len(versions_at) - 1 else 0)
        for index, at in enumerate(versions_at)
    )
    verneed = struct.pack("<HHIII", 1, len(versions_at), libc_at, 16, 0) + auxiliaries

    body = bytearray(64)
    placed = []
    for blob in (bytes(dynstr), dynamic, verneed):
        body.extend(b"\0" * (-len(body) % 8))
        placed.append((len(body), len(blob)))
        body.extend(blob)
    body.extend(b"\0" * (-len(body) % 8))
    section_table = len(body)
    sections = [
        (0, 0, 0, 0, 0),
        (3, *placed[0], 0, 0),
        (6, *placed[1], 1, 0),
        (0x6FFFFFFE, *placed[2], 1, 1),
    ]
    for kind, offset, size, link, info in sections:
        body.extend(struct.pack("<IIQQQQIIQQ", 0, kind, 0, 0, offset, size, link, info, 8, 0))
    ident = b"\x7fELF" + bytes([elf_class, 1, 1, 0]) + bytes(8)
    header = struct.pack(
        "<HHIQQQIHHHHHH", 3, machine, 1, 0, 0, section_table, 0, 64, 56, 0, 64, len(sections), 0
    )
    body[:64] = ident + header
    return bytes(body)


def _project() -> dict[str, Any]:
    return tomllib.loads((PYTHON / "pyproject.toml").read_text(encoding="utf-8"))["project"]


def _build(tmp_path: Path, binary: bytes, target: str, project: Path = PYTHON) -> Path:
    tmp_path.mkdir(parents=True, exist_ok=True)
    path = tmp_path / "grida-fx"
    path.write_bytes(binary)
    return tool.build_wheel(path, target, tmp_path / "dist", project)


def _package_names() -> list[str]:
    root = PYTHON / "src"
    names = []
    for path in (root / "grida").rglob("*"):
        relative = path.relative_to(root)
        if path.is_file() and "__pycache__" not in relative.parts and path.suffix != ".pyc":
            if not relative.as_posix().startswith("grida/fx/_bin/"):
                names.append(relative.as_posix())
    return names


# ------------------------------------------------------------------------------------------------
# The wheel


def test_a_wheel_carries_the_engine_the_package_and_its_record(tmp_path: Path) -> None:
    binary = macho()
    wheel = _build(tmp_path, binary, "aarch64-apple-darwin")
    version = _project()["version"]
    tag = "py3-none-macosx_11_0_arm64"
    assert wheel.name == f"grida-{version}-{tag}.whl"
    dist_info = f"grida-{version}.dist-info"
    with zipfile.ZipFile(wheel) as archive:
        infos = archive.infolist()
        names = [info.filename for info in infos]
        payload = sorted([*_package_names(), "grida/fx/_bin/grida-fx"])
        assert names == [
            *payload,
            *(f"{dist_info}/{n}" for n in ("METADATA", "WHEEL", "licenses/LICENSE", "RECORD")),
        ]
        assert archive.read(f"{dist_info}/licenses/LICENSE") == (REPO / "LICENSE").read_bytes()
        assert archive.read("grida/fx/_bin/grida-fx") == binary
        assert archive.read(f"{dist_info}/WHEEL").decode() == (
            "Wheel-Version: 1.0\n"
            "Generator: grida-fx tools/build_wheel.py\n"
            "Root-Is-Purelib: false\n"
            f"Tag: {tag}\n"
        )
        for info in infos:
            assert info.date_time == (1980, 1, 1, 0, 0, 0)
            assert info.create_system == 3
            assert info.compress_type == zipfile.ZIP_DEFLATED
            mode = info.external_attr >> 16
            assert stat.S_ISREG(mode)
            expected = 0o755 if info.filename == "grida/fx/_bin/grida-fx" else 0o644
            assert stat.S_IMODE(mode) == expected, info.filename

        rows = list(csv.reader(io.StringIO(archive.read(f"{dist_info}/RECORD").decode())))
        assert [row[0] for row in rows] == names
        assert rows[-1] == [f"{dist_info}/RECORD", "", ""]
        for name, hashed, size in rows[:-1]:
            data = archive.read(name)
            digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=")
            assert hashed == f"sha256={digest.decode()}", name
            assert int(size) == len(data), name

        # No console command and nothing installed outside the package.
        assert not any(name.endswith("entry_points.txt") for name in names)
        assert not any(".data/" in name for name in names)
        assert all(name.startswith(("grida/", f"{dist_info}/")) for name in names)


def test_the_metadata_comes_from_pyproject_and_the_readme(tmp_path: Path) -> None:
    wheel = _build(tmp_path, macho(), "aarch64-apple-darwin")
    project = _project()
    with zipfile.ZipFile(wheel) as archive:
        metadata = archive.read(f"grida-{project['version']}.dist-info/METADATA").decode()
    message = email.parser.Parser().parsestr(metadata)
    assert message["Metadata-Version"] == "2.4"
    assert message["License-Expression"] == project["license"] == "Apache-2.0"
    assert message.get_all("License-File") == project["license-files"] == ["LICENSE"]
    assert message["Name"] == project["name"] == "grida"
    assert message["Version"] == project["version"]
    assert message["Summary"] == project["description"]
    assert message["Requires-Python"] == project["requires-python"]
    assert message.get_all("Requires-Dist") == project["dependencies"]
    assert message.get_all("Project-URL") == [
        f"{label}, {url}" for label, url in project["urls"].items()
    ]
    assert message["Description-Content-Type"] == "text/markdown"
    assert message.get_payload() == (PYTHON / project["readme"]).read_text(encoding="utf-8")


def test_the_wheel_is_the_same_bytes_whenever_it_is_built(tmp_path: Path) -> None:
    copy = tmp_path / "project"
    copy.mkdir()
    for name in ("pyproject.toml", "README.md", "LICENSE"):
        shutil.copy2(PYTHON / name, copy / name)
    # As a fresh checkout has it: no caches, and no engine from tools/build_engine.py.
    clean = shutil.ignore_patterns("__pycache__", "_bin")
    shutil.copytree(PYTHON / "src", copy / "src", ignore=clean)
    first = _build(tmp_path / "first", macho(), "aarch64-apple-darwin", copy)
    for path in copy.rglob("*"):
        os.utime(path, (2_000_000_000, 2_000_000_000))
    (copy / "src" / "grida" / "fx" / "__pycache__").mkdir()
    (copy / "src" / "grida" / "fx" / "__pycache__" / "_api.cpython-311.pyc").write_bytes(b"x")
    (copy / "src" / "grida" / ".DS_Store").write_bytes(b"x")
    (copy / "src" / "grida" / "fx" / "_bin").mkdir()
    (copy / "src" / "grida" / "fx" / "_bin" / "grida-fx").write_bytes(b"a stale engine")
    second = _build(tmp_path / "second", macho(), "aarch64-apple-darwin", copy)
    assert first.name == second.name
    assert first.read_bytes() == second.read_bytes()


def test_the_wheel_puts_the_engine_where_the_sdk_looks_for_it() -> None:
    source = PYTHON / "src"
    assert (source / tool.BINARY_PATH).resolve() == _api._packaged_binary()


def test_a_project_key_the_tool_does_not_write_is_refused(tmp_path: Path) -> None:
    copy = tmp_path / "project"
    copy.mkdir()
    text = (PYTHON / "pyproject.toml").read_text(encoding="utf-8")
    (copy / "pyproject.toml").write_text(
        text.replace('readme = "README.md"\n', 'readme = "README.md"\nkeywords = ["fx"]\n'),
        encoding="utf-8",
    )
    with pytest.raises(tool.BuildError, match="keywords, which build_wheel.py does not write"):
        tool.project_metadata(copy)


def test_a_declared_license_cannot_be_silently_omitted(tmp_path: Path) -> None:
    shutil.copy2(PYTHON / "pyproject.toml", tmp_path / "pyproject.toml")
    with pytest.raises(tool.BuildError, match="license-files pattern 'LICENSE' matches no files"):
        _build(tmp_path, macho(), "aarch64-apple-darwin", tmp_path)


def test_the_command_prints_the_wheel_or_the_reason(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    binary = tmp_path / "grida-fx"
    binary.write_bytes(elf(EM_AARCH64, glibc=("2.17",)))
    out = tmp_path / "dist"
    assert (
        tool.main(
            ["--binary", str(binary), "--target", "aarch64-unknown-linux-gnu", "--out", str(out)]
        )
        == 0
    )
    printed = capsys.readouterr().out.strip()
    assert printed.endswith("-py3-none-manylinux_2_28_aarch64.whl")
    assert Path(printed).is_file()
    assert (
        tool.main(
            ["--binary", str(binary), "--target", "x86_64-unknown-linux-gnu", "--out", str(out)]
        )
        == 1
    )
    assert capsys.readouterr().err == "build_wheel: the binary is built for aarch64, not x86_64\n"
    assert [path.name for path in out.iterdir()] == [Path(printed).name]


# ------------------------------------------------------------------------------------------------
# The platform tag


@pytest.mark.parametrize(
    ("binary", "target", "tag"),
    [
        (macho(ARM64, (11, 0, 0)), "aarch64-apple-darwin", "macosx_11_0_arm64"),
        (macho(X86_64, (10, 12, 0)), "x86_64-apple-darwin", "macosx_10_12_x86_64"),
        (macho(X86_64, (10, 9, 0), legacy=True), "x86_64-apple-darwin", "macosx_10_9_x86_64"),
        (macho(X86_64, (10, 12, 1)), "x86_64-apple-darwin", "macosx_10_13_x86_64"),
        (macho(ARM64, (11, 3, 0)), "aarch64-apple-darwin", "macosx_12_0_arm64"),
        (macho(ARM64, (14, 0, 0)), "aarch64-apple-darwin", "macosx_14_0_arm64"),
        (
            macho(ARM64, libraries=("/usr/lib/libiconv.2.dylib", "/System/Library/Frameworks/x")),
            "aarch64-apple-darwin",
            "macosx_11_0_arm64",
        ),
        (elf(EM_X86_64), "x86_64-unknown-linux-gnu", "manylinux_2_28_x86_64"),
        (elf(EM_AARCH64, glibc=("2.17",)), "aarch64-unknown-linux-gnu", "manylinux_2_28_aarch64"),
        (elf(EM_X86_64, needed=(), glibc=()), "x86_64-unknown-linux-gnu", "manylinux_2_28_x86_64"),
    ],
)
def test_the_tag_is_read_from_the_binary(binary: bytes, target: str, tag: str) -> None:
    assert tool.platform_tag(binary, target) == tag


@pytest.mark.parametrize(
    ("binary", "target", "reason"),
    [
        (macho(ARM64), "x86_64-apple-darwin", "the binary is built for arm64, not x86_64"),
        (macho(X86_64), "aarch64-apple-darwin", "the binary is built for x86_64, not arm64"),
        (elf(), "aarch64-apple-darwin", "the binary is not a 64-bit Mach-O file"),
        (macho(), "x86_64-unknown-linux-gnu", "the binary is not an ELF file"),
        (
            macho(magic=0xBEBAFECA),
            "aarch64-apple-darwin",
            "the binary is a universal Mach-O; build one per architecture",
        ),
        (
            macho(minimum=None),
            "aarch64-apple-darwin",
            "the binary does not say its minimum macOS version",
        ),
        (
            macho(platform=2),
            "aarch64-apple-darwin",
            "the binary is built for platform 2, not macOS",
        ),
        (
            macho(libraries=("/usr/lib/libSystem.B.dylib", "/opt/homebrew/lib/libz.1.dylib")),
            "aarch64-apple-darwin",
            "the binary links libraries outside the system: /opt/homebrew/lib/libz.1.dylib",
        ),
        (
            elf(EM_AARCH64),
            "x86_64-unknown-linux-gnu",
            "the binary is built for aarch64, not x86_64",
        ),
        (
            elf(elf_class=1),
            "x86_64-unknown-linux-gnu",
            "the binary is not a 64-bit little-endian ELF file",
        ),
        (
            elf(glibc=("2.2.5", "2.34")),
            "x86_64-unknown-linux-gnu",
            "the binary needs glibc 2.34, past manylinux_2_28's 2.28: build it with cargo zigbuild"
            " --target <target>.2.28",
        ),
        (
            elf(needed=("libc.so.6", "libssl.so.3")),
            "x86_64-unknown-linux-gnu",
            "the binary needs libraries manylinux does not have: libssl.so.3",
        ),
        (b"\xcf\xfa", "aarch64-apple-darwin", "the binary is truncated or malformed"),
        (
            macho(),
            "x86_64-pc-windows-msvc",
            "x86_64-pc-windows-msvc is not a preview target (aarch64-apple-darwin,"
            " x86_64-apple-darwin, x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu); Windows"
            " is not in the preview: the runner uses Unix process groups and signals",
        ),
    ],
)
def test_a_binary_the_tag_would_lie_about_is_refused(
    binary: bytes, target: str, reason: str
) -> None:
    with pytest.raises(tool.BuildError) as raised:
        tool.platform_tag(binary, target)
    assert str(raised.value).startswith(reason)
