"""Native preparation refuses unsupported platforms, wrong tooling and source changes."""

from __future__ import annotations

import importlib
import json
import subprocess
from pathlib import Path

import pytest


@pytest.fixture
def preparer(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parents[2] / "tools"))
    return importlib.import_module("prepare_local")


@pytest.mark.parametrize(
    ("system", "machine", "expected"),
    [
        ("Darwin", "arm64", ("aarch64-apple-darwin", "11.0")),
        ("Darwin", "x86_64", ("x86_64-apple-darwin", "10.12")),
    ],
)
def test_native_baseline(preparer, monkeypatch, system, machine, expected):
    monkeypatch.setattr(preparer.platform, "system", lambda: system)
    monkeypatch.setattr(preparer.platform, "machine", lambda: machine)
    assert preparer.native_target() == expected


def test_linux_cannot_accidentally_publish_system_glibc(preparer, monkeypatch):
    monkeypatch.setattr(preparer.platform, "system", lambda: "Linux")
    monkeypatch.setattr(preparer.platform, "machine", lambda: "aarch64")
    with pytest.raises(ValueError, match="use release CI"):
        preparer.native_target()


@pytest.fixture
def build_inputs(preparer, monkeypatch, tmp_path):
    monkeypatch.setattr(preparer, "ROOT", tmp_path)
    (tmp_path / "package.json").write_text(json.dumps({"packageManager": "bun@1.4.0"}))
    monkeypatch.setattr(preparer, "native_target", lambda: ("aarch64-apple-darwin", "11.0"))
    monkeypatch.setattr(preparer.shutil, "which", lambda name: f"/tools/{name}")

    def output(command, **kwargs):
        if command == ["bun", "--version"]:
            return "1.4.0\n"
        if command[0] == "cargo":
            return json.dumps({"target_directory": str(tmp_path / "target")})
        return "0.1.0-alpha.1\n"

    monkeypatch.setattr(preparer.subprocess, "check_output", output)
    return preparer


def test_wrong_bun_stops_before_build(build_inputs, monkeypatch, tmp_path):
    monkeypatch.setattr(build_inputs.subprocess, "check_output", lambda *a, **kw: "1.3.3\n")
    with pytest.raises(ValueError, match="use Bun 1.4.0"):
        build_inputs.prepare(tmp_path / "bundle")


def test_failed_build_never_packages(build_inputs, monkeypatch, tmp_path):
    monkeypatch.setattr(build_inputs, "source_snapshot", lambda: {"sha256": "original"})
    packages = []
    monkeypatch.setattr(build_inputs, "package", lambda *a, **kw: packages.append(a))

    def fail(command, **kwargs):
        raise subprocess.CalledProcessError(1, command)

    monkeypatch.setattr(build_inputs, "run", fail)
    with pytest.raises(subprocess.CalledProcessError):
        build_inputs.prepare(tmp_path / "bundle")
    assert not packages


def test_source_change_during_build_never_packages(build_inputs, monkeypatch, tmp_path):
    sources = iter([{"sha256": "original"}, {"sha256": "changed"}])
    monkeypatch.setattr(build_inputs, "source_snapshot", lambda: next(sources))
    monkeypatch.setattr(build_inputs, "run", lambda *a, **kw: None)
    packages = []
    monkeypatch.setattr(build_inputs, "package", lambda *a, **kw: packages.append(a))
    with pytest.raises(ValueError, match="source changed during the build"):
        build_inputs.prepare(tmp_path / "bundle")
    assert not packages
