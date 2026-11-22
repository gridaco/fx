"""Check release assets against both tested package formats."""

import io
import stat
import sys
import tarfile
import zipfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
TARGET = "aarch64-apple-darwin"
VERSION = "0.12.0"


@pytest.mark.parametrize(
    ("mode", "wheel_matches", "expected_version", "error"),
    [
        (0o755, True, VERSION, None),
        (0o644, True, VERSION, "unexpected member"),
        (0o755, False, VERSION, "different executables"),
        (0o755, True, "0.11.0", "standalone executable reported"),
    ],
)
def test_standalone_matches_tested_packages(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mode: int,
    wheel_matches: bool,
    expected_version: str,
    error: str | None,
) -> None:
    monkeypatch.syspath_prepend(str(ROOT / "tools"))
    from standalone_release import package

    engine = tmp_path / f"grida-fx-darwin-arm64-{VERSION}.tgz"
    executable = f"#!{sys.executable}\nprint('grida-fx {expected_version}')\n".encode()
    with tarfile.open(engine, "w:gz") as archive:
        for name, contents, permissions in (
            ("package/bin/grida-fx", executable, mode),
            ("package/LICENSE", b"license text\n", 0o644),
        ):
            entry = tarfile.TarInfo(name)
            entry.mode = permissions
            entry.size = len(contents)
            archive.addfile(entry, io.BytesIO(contents))
    wheel = tmp_path / "grida.whl"
    with zipfile.ZipFile(wheel, "w") as archive:
        archive.writestr("grida/fx/_bin/grida-fx", executable if wheel_matches else b"other binary")
    out = tmp_path / "standalone"
    if error:
        with pytest.raises(ValueError, match=error):
            package(engine, wheel, TARGET, VERSION, out)
        assert not list(out.glob("*.tar.gz"))
        return
    path = package(engine, wheel, TARGET, VERSION, out)
    assert path.name == f"grida-fx-{VERSION}-{TARGET}.tar.gz"
    with tarfile.open(path, "r:gz") as archive:
        assert archive.getnames() == ["grida-fx", "LICENSE"]
        assert stat.S_IMODE(archive.getmember("grida-fx").mode) == mode
        binary = archive.extractfile("grida-fx")
        license_file = archive.extractfile("LICENSE")
        assert binary is not None and binary.read() == executable
        assert license_file is not None and license_file.read() == b"license text\n"
    monkeypatch.chdir(tmp_path)
    again = package(engine, wheel, TARGET, VERSION, Path("again"))
    assert again.read_bytes() == path.read_bytes()
