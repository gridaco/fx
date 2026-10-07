"""Release preparation protects source/output boundaries and excludes dotenv."""

from __future__ import annotations

import importlib.util
import io
import json
import subprocess
import tarfile
import zipfile
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "fx_package_release", REPO / "tools" / "package_release.py"
)
assert spec is not None and spec.loader is not None
tool = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tool)


def repository(path: Path) -> Path:
    path.mkdir()
    subprocess.run(["git", "init", "--quiet", str(path)], check=True)
    (path / ".gitignore").write_text("/target/\n", encoding="utf-8")
    subprocess.run(["git", "add", ".gitignore"], cwd=path, check=True)
    subprocess.run(
        [
            "git",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
        cwd=path,
        check=True,
    )
    return path


def test_output_refuses_source_parent_and_existing_directory(tmp_path: Path) -> None:
    root = repository(tmp_path / "source")
    for destination in (root, tmp_path, root / "python"):
        with pytest.raises(ValueError):
            tool.prepare_output(destination, root)
    destination = tmp_path / "release"
    tool.prepare_output(destination, root)
    with pytest.raises(FileExistsError):
        tool.prepare_output(destination, root)


def test_output_allows_only_ignored_paths_inside_source(tmp_path: Path) -> None:
    root = repository(tmp_path / "source")
    destination = root / "target" / "release-artifacts"
    assert tool.prepare_output(destination, root) == destination


def test_output_refuses_symlink_ancestor(tmp_path: Path) -> None:
    root = repository(tmp_path / "source")
    destination = tmp_path / "destination"
    destination.mkdir()
    link = tmp_path / "link"
    link.symlink_to(destination, target_is_directory=True)
    with pytest.raises(ValueError, match="symlinks"):
        tool.prepare_output(link / "release", root)


def test_source_snapshot_includes_dirty_files_but_never_reads_dotenv(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root = repository(tmp_path / "source")
    (root / "workflow.py").write_text("original\n", encoding="utf-8")
    (root / ".env").write_text("must not be read", encoding="utf-8")
    original = tool.digest

    def digest(path: Path) -> str:
        assert path.name != ".env"
        return original(path)

    monkeypatch.setattr(tool, "digest", digest)
    first = tool.source_snapshot(root)
    assert "workflow.py" in first["files"]
    assert ".env" not in first["files"]
    (root / "workflow.py").write_text("changed\n", encoding="utf-8")
    assert tool.source_snapshot(root)["sha256"] != first["sha256"]


def test_verification_environment_never_uses_checkout_engine_or_provider_keys(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("OPENAI_API_KEY", "stand-in")
    monkeypatch.setenv("GRIDA_FX_BIN", "/checkout/engine")
    monkeypatch.setenv("PYTHONPATH", "/checkout/python")
    environment = tool.clean_environment()
    assert "OPENAI_API_KEY" not in environment
    assert "GRIDA_FX_BIN" not in environment
    assert "PYTHONPATH" not in environment
    assert environment["GRIDA_FX_DISABLE_DOTENV"] == "1"
    assert environment["GRIDA_FX_NETWORK"] == "off"


def test_both_packages_must_carry_the_exact_input_binary(tmp_path: Path) -> None:
    binary = tmp_path / "grida-fx"
    binary.write_bytes(b"original engine")
    wheel = tmp_path / "package.whl"
    npm = tmp_path / "package.tgz"

    def archives(wheel_bytes: bytes, npm_bytes: bytes) -> None:
        with zipfile.ZipFile(wheel, "w") as archive:
            archive.writestr("grida/fx/_bin/grida-fx", wheel_bytes)
        with tarfile.open(npm, "w:gz") as archive:
            entry = tarfile.TarInfo("package/bin/grida-fx")
            entry.size = len(npm_bytes)
            archive.addfile(entry, io.BytesIO(npm_bytes))

    expected = tool.digest(binary)
    archives(binary.read_bytes(), binary.read_bytes())
    tool.check_embedded_engines(wheel, npm, expected)
    archives(b"replaced engine", binary.read_bytes())
    with pytest.raises(ValueError, match="wheel carries a different engine"):
        tool.check_embedded_engines(wheel, npm, expected)
    archives(binary.read_bytes(), b"replaced engine")
    with pytest.raises(ValueError, match="npm tarball carries a different engine"):
        tool.check_embedded_engines(wheel, npm, expected)


@pytest.fixture
def packaged_inputs(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    binary = tmp_path / "grida-fx"
    binary.write_bytes(b"original engine")
    out = tmp_path / "release"
    monkeypatch.setattr(tool, "source_snapshot", lambda: {"head": "a" * 40, "sha256": "b" * 64})

    def version(command, **kwargs):
        return b"0.1.0a1" if command[-1] == "pep440" else b"0.1.0-alpha.1"

    def run(command, **kwargs):
        if command[1].endswith("build_wheel.py"):
            wheels = out / "wheels"
            wheels.mkdir()
            with zipfile.ZipFile(
                wheels / "grida-0.1.0a1-py3-none-macosx_11_0_arm64.whl", "w"
            ) as archive:
                archive.writestr("grida/fx/_bin/grida-fx", binary.read_bytes())
        elif command[1].endswith("build_npm.mjs"):
            npm = out / "npm"
            npm.mkdir()
            with tarfile.open(npm / "grida-fx-darwin-arm64-0.1.0-alpha.1.tgz", "w:gz") as archive:
                entry = tarfile.TarInfo("package/bin/grida-fx")
                entry.size = binary.stat().st_size
                archive.addfile(entry, io.BytesIO(binary.read_bytes()))
            (npm / "grida-fx-0.1.0-alpha.1.tgz").write_bytes(b"SDK artifact")

    monkeypatch.setattr(tool.subprocess, "check_output", version)
    monkeypatch.setattr(tool, "run", run)
    return binary, out


@pytest.mark.parametrize("changed", ["wheel", "engine", "sdk"])
def test_changed_artifact_during_installed_checks_cannot_be_marked_verified(
    packaged_inputs, monkeypatch: pytest.MonkeyPatch, changed: str
) -> None:
    binary, out = packaged_inputs

    def verify(wheel, tarballs, output):
        artifact = (
            wheel
            if changed == "wheel"
            else next(
                path for path in tarballs if ("darwin-arm64" in path.name) == (changed == "engine")
            )
        )
        with artifact.open("ab") as stream:
            stream.write(b"changed during installation checks")

    monkeypatch.setattr(tool, "verify", verify)
    with pytest.raises(ValueError, match="artifact changed during verification"):
        tool.package(binary, "aarch64-apple-darwin", out, installed_checks=True)
    assert not (out / "release-manifest.json").exists()


def test_verified_manifest_binds_the_exact_checked_artifacts(
    packaged_inputs, monkeypatch: pytest.MonkeyPatch
) -> None:
    binary, out = packaged_inputs
    checked = {}

    def verify(wheel, tarballs, output):
        checked.update(
            {path.relative_to(output).as_posix(): tool.digest(path) for path in [wheel, *tarballs]}
        )

    monkeypatch.setattr(tool, "verify", verify)
    manifest = tool.package(binary, "aarch64-apple-darwin", out, installed_checks=True)
    value = json.loads(manifest.read_text())
    assert value["verified"] is True
    assert {artifact["filename"]: artifact["sha256"] for artifact in value["artifacts"]} == checked
