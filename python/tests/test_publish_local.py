"""Local publishing refuses changed files and unsafe manifests before any upload."""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import sys
import tarfile
import urllib.error
import zipfile
from pathlib import Path

import pytest


@pytest.fixture
def publisher(monkeypatch: pytest.MonkeyPatch):
    path = Path(__file__).resolve().parents[2] / "tools" / "publish_local.py"
    spec = importlib.util.spec_from_file_location("fx_local_publisher", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, spec.name, module)
    spec.loader.exec_module(module)
    return module


def npm_tarball(
    path: Path,
    manifest: dict,
    executable: str,
    *,
    link: bool = False,
    binary: bytes = b"engine",
) -> None:
    files = {
        "package/package.json": json.dumps(manifest).encode(),
        executable: binary,
    }
    if manifest["name"] == "@grida/fx":
        files["package/dist/index.js"] = b"export const version = '0.1.0-alpha.1';"
        files["package/dist/index.d.ts"] = b"export declare const version: string;"
    with tarfile.open(path, "w:gz") as archive:
        for name, content in files.items():
            member = tarfile.TarInfo(name)
            member.mode = 0o755 if name == executable else 0o644
            member.size = len(content)
            archive.addfile(member, io.BytesIO(content))
        if link:
            member = tarfile.TarInfo("package/alias")
            member.type = tarfile.SYMTYPE
            member.linkname = "/outside"
            archive.addfile(member)


@pytest.fixture
def release_files(publisher, tmp_path: Path):
    version = "0.1.0-alpha.1"
    python_version = "0.1.0a1"
    wheels = tmp_path / "wheels"
    npm = tmp_path / "npm"
    wheels.mkdir()
    npm.mkdir()
    wheel = wheels / "grida-0.1.0a1-py3-none-macosx_11_0_arm64.whl"
    with zipfile.ZipFile(wheel, "w") as archive:
        prefix = "grida-0.1.0a1.dist-info/"
        archive.writestr(
            prefix + "METADATA", "Metadata-Version: 2.1\nName: grida\nVersion: 0.1.0a1\n"
        )
        archive.writestr(
            prefix + "WHEEL",
            "Wheel-Version: 1.0\nRoot-Is-Purelib: false\nTag: py3-none-macosx_11_0_arm64\n",
        )
        executable = zipfile.ZipInfo("grida/fx/_bin/grida-fx")
        executable.external_attr = 0o100755 << 16
        archive.writestr(executable, b"engine")
    engine = npm / f"grida-fx-darwin-arm64-{version}.tgz"
    engine_manifest = {
        "name": "@grida/fx-darwin-arm64",
        "version": version,
        "os": ["darwin"],
        "cpu": ["arm64"],
        "publishConfig": {"access": "public", "tag": "next"},
    }
    npm_tarball(engine, engine_manifest, "package/bin/grida-fx")
    sdk = npm / f"grida-fx-{version}.tgz"
    sdk_manifest = {
        "name": "@grida/fx",
        "version": version,
        "bin": {"grida-fx": "bin/grida-fx.js"},
        "optionalDependencies": {platform[0]: version for platform in publisher.TARGETS.values()},
        "publishConfig": {"access": "public", "tag": "next"},
    }
    npm_tarball(sdk, sdk_manifest, "package/bin/grida-fx.js")
    artifacts = [
        ("wheel", "grida", python_version, wheel),
        ("npm", "@grida/fx", version, sdk),
        ("npm", "@grida/fx-darwin-arm64", version, engine),
    ]
    value = {
        "kind": "fx-local-release-v1",
        "version": version,
        "python_version": python_version,
        "target": "aarch64-apple-darwin",
        "binary_sha256": hashlib.sha256(b"engine").hexdigest(),
        "source": {"head": "a" * 40, "sha256": "b" * 64},
        "verified": True,
        "checks": sorted(publisher.REQUIRED_CHECKS),
        "artifacts": [
            {
                "kind": kind,
                "name": name,
                "version": artifact_version,
                "filename": path.relative_to(tmp_path).as_posix(),
                "sha256": publisher.sha256(path),
            }
            for kind, name, artifact_version, path in artifacts
        ],
    }
    manifest = tmp_path / "release-manifest.json"
    manifest.write_text(json.dumps(value), encoding="utf-8")
    return manifest, value, engine_manifest


def rewrite(manifest: Path, value: dict) -> None:
    manifest.write_text(json.dumps(value), encoding="utf-8")


def test_default_plan_is_offline_and_orders_engine_before_sdk(
    publisher, release_files, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture
):
    manifest, _, _ = release_files

    def forbidden(*args, **kwargs):
        pytest.fail("offline planning must not access a registry or execute a publish command")

    monkeypatch.setattr(publisher.urllib.request, "urlopen", forbidden)
    monkeypatch.setattr(publisher.subprocess, "run", forbidden)
    assert publisher.main(["--manifest", str(manifest)]) == 0
    output = capsys.readouterr().out
    assert "Offline plan only" in output
    commands = publisher.commands(publisher.load_manifest(manifest))
    assert "darwin-arm64" in commands[0][2]
    assert commands[1][2].endswith("grida-fx-0.1.0-alpha.1.tgz")
    assert commands[0][-2:] == ["--registry", "https://registry.npmjs.org"]
    assert commands[2][:5] == [
        "uv",
        "--no-config",
        "publish",
        "--publish-url",
        "https://upload.pypi.org/legacy/",
    ]
    assert "--no-attestations" in commands[2]


@pytest.mark.parametrize("only,program,count", [("pypi", "uv", 1), ("npm", "npm", 2)])
def test_selected_offline_plan_omits_other_registry(
    publisher,
    release_files,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture,
    only: str,
    program: str,
    count: int,
):
    manifest, _, _ = release_files

    def forbidden(*args, **kwargs):
        pytest.fail("offline planning must not request auth, access a registry or execute commands")

    monkeypatch.setattr(publisher.urllib.request, "urlopen", forbidden)
    monkeypatch.setattr(publisher.subprocess, "run", forbidden)
    monkeypatch.setattr(publisher.shutil, "which", forbidden)
    assert publisher.main(["--manifest", str(manifest), "--only", only]) == 0
    output = capsys.readouterr().out
    assert "Offline plan only" in output
    assert ("@grida/fx" in output) == (only == "npm")
    assert (publisher.PYPI_UPLOAD in output) == (only == "pypi")
    commands = publisher.commands(publisher.load_manifest(manifest), only)
    assert len(commands) == count
    assert all(command[0] == program for command in commands)


@pytest.mark.parametrize(
    "only,program,registry,count",
    [("pypi", "uv", "https://pypi.org/", 1), ("npm", "npm", "https://registry.npmjs.org/", 2)],
)
def test_selected_publish_never_uses_other_uploader_or_registry(
    publisher,
    release_files,
    monkeypatch: pytest.MonkeyPatch,
    only: str,
    program: str,
    registry: str,
    count: int,
):
    manifest, _, _ = release_files
    uploads = []
    checks = []

    def which(name):
        assert name == program, "unselected uploader must not be required"
        return f"/fake/{name}"

    def urlopen(request, timeout):
        # The excluded registry could already have this version; never query it.
        assert request.full_url.startswith(registry), "unselected registry must not be queried"
        checks.append(request.full_url)
        raise urllib.error.HTTPError(request.full_url, 404, "Not Found", {}, None)

    def run(command, check):
        assert command[0] == program, "unselected uploader must not execute"
        assert check is False
        uploads.append(command)
        return publisher.subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(publisher.shutil, "which", which)
    monkeypatch.setattr(publisher.urllib.request, "urlopen", urlopen)
    monkeypatch.setattr(publisher.subprocess, "run", run)
    publisher.publish(publisher.load_manifest(manifest), only)
    assert len(checks) == len(uploads) == count
    if only == "npm":
        assert "darwin-arm64" in uploads[0][2]
        assert uploads[1][2].endswith("grida-fx-0.1.0-alpha.1.tgz")


def test_pypi_only_cli_passes_selection_and_reports_one_artifact(
    publisher, release_files, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture
):
    manifest, _, _ = release_files
    selections = []
    monkeypatch.setattr(publisher, "publish", lambda release, only: selections.append(only))
    assert publisher.main(["--manifest", str(manifest), "--only", "pypi", "--publish"]) == 0
    assert selections == ["pypi"]
    assert "Published 1 selected artifact." in capsys.readouterr().out


@pytest.mark.parametrize("filename", ["../outside.tgz", "/outside.tgz", "npm/../outside.tgz"])
def test_manifest_paths_cannot_escape_output(publisher, release_files, filename: str):
    manifest, value, _ = release_files
    value["artifacts"][0]["filename"] = filename
    rewrite(manifest, value)
    with pytest.raises(publisher.PublishError, match="relative path"):
        publisher.load_manifest(manifest)


def test_symlink_artifact_is_refused_even_when_target_is_inside_output(publisher, release_files):
    manifest, value, _ = release_files
    original = manifest.parent / value["artifacts"][0]["filename"]
    alias = original.parent / "alias.whl"
    alias.symlink_to(original)
    value["artifacts"][0]["filename"] = alias.relative_to(manifest.parent).as_posix()
    rewrite(manifest, value)
    with pytest.raises(publisher.PublishError, match="symbolic link"):
        publisher.load_manifest(manifest)


def test_changed_artifact_digest_is_refused(publisher, release_files):
    manifest, value, _ = release_files
    artifact = manifest.parent / value["artifacts"][0]["filename"]
    with artifact.open("ab") as stream:
        stream.write(b"changed after verification")
    with pytest.raises(publisher.PublishError, match="digest disagrees"):
        publisher.load_manifest(manifest)


@pytest.mark.parametrize("value", [None, "not-a-digest", "A" * 64])
def test_binary_digest_must_be_canonical(publisher, release_files, value):
    manifest, document, _ = release_files
    document["binary_sha256"] = value
    rewrite(manifest, document)
    with pytest.raises(publisher.PublishError, match="binary_sha256 must"):
        publisher.load_manifest(manifest)


def test_npm_embedded_engine_must_match_verified_binary(publisher, release_files):
    manifest, value, engine_manifest = release_files
    record = value["artifacts"][2]
    path = manifest.parent / record["filename"]
    npm_tarball(path, engine_manifest, "package/bin/grida-fx", binary=b"different engine")
    record["sha256"] = publisher.sha256(path)
    rewrite(manifest, value)
    with pytest.raises(publisher.PublishError, match="npm package's embedded engine disagrees"):
        publisher.load_manifest(manifest)


def test_wheel_embedded_engine_must_match_verified_binary(publisher, release_files):
    manifest, value, _ = release_files
    record = value["artifacts"][0]
    path = manifest.parent / record["filename"]
    with zipfile.ZipFile(path) as archive:
        files = [(entry, archive.read(entry)) for entry in archive.infolist()]
    with zipfile.ZipFile(path, "w") as archive:
        for entry, content in files:
            if entry.filename == "grida/fx/_bin/grida-fx":
                content = b"different engine"
            archive.writestr(entry, content)
    record["sha256"] = publisher.sha256(path)
    rewrite(manifest, value)
    with pytest.raises(publisher.PublishError, match="wheel's embedded engine disagrees"):
        publisher.load_manifest(manifest)


def test_tarball_identity_is_checked_after_digest(publisher, release_files):
    manifest, value, engine_manifest = release_files
    record = value["artifacts"][2]
    path = manifest.parent / record["filename"]
    engine_manifest["name"] = "@acme/different"
    npm_tarball(path, engine_manifest, "package/bin/grida-fx")
    record["sha256"] = publisher.sha256(path)
    rewrite(manifest, value)
    with pytest.raises(publisher.PublishError, match="identity disagrees"):
        publisher.load_manifest(manifest)


def test_tarball_cannot_carry_symbolic_links(publisher, release_files):
    manifest, value, engine_manifest = release_files
    record = value["artifacts"][2]
    path = manifest.parent / record["filename"]
    npm_tarball(path, engine_manifest, "package/bin/grida-fx", link=True)
    record["sha256"] = publisher.sha256(path)
    rewrite(manifest, value)
    with pytest.raises(publisher.PublishError, match="link or special file"):
        publisher.load_manifest(manifest)


@pytest.mark.parametrize("verified,checks", [(False, True), (True, False)])
def test_unverified_release_cannot_reach_registry(
    publisher, release_files, monkeypatch: pytest.MonkeyPatch, verified: bool, checks: bool
):
    manifest, value, _ = release_files
    value["verified"] = verified
    if not checks:
        value["checks"].remove("npm_viewer")
    rewrite(manifest, value)

    def forbidden(*args, **kwargs):
        pytest.fail("unverified releases must stop before network checks")

    monkeypatch.setattr(publisher, "refuse_existing", forbidden)
    with pytest.raises(publisher.PublishError, match="installed-package checks"):
        publisher.publish(publisher.load_manifest(manifest))


@pytest.mark.parametrize("only", ["all", "pypi", "npm"])
def test_existing_registry_version_prevents_every_upload(
    publisher, release_files, monkeypatch: pytest.MonkeyPatch, only: str
):
    manifest, _, _ = release_files
    calls = []
    monkeypatch.setattr(publisher.shutil, "which", lambda name: f"/fake/{name}")

    class Existing:
        status = 200

        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

    monkeypatch.setattr(publisher.urllib.request, "urlopen", lambda *a, **kw: Existing())
    monkeypatch.setattr(publisher.subprocess, "run", lambda *a, **kw: calls.append(a))
    with pytest.raises(publisher.PublishError, match="already exists"):
        publisher.publish(publisher.load_manifest(manifest), only)
    assert calls == []


def test_network_error_does_not_masquerade_as_unpublished(
    publisher, release_files, monkeypatch: pytest.MonkeyPatch
):
    manifest, _, _ = release_files

    def unavailable(*args, **kwargs):
        raise urllib.error.URLError("unavailable")

    monkeypatch.setattr(publisher.urllib.request, "urlopen", unavailable)
    with pytest.raises(publisher.PublishError, match="registry check failed"):
        publisher.refuse_existing(publisher.load_manifest(manifest).artifacts[0])


@pytest.mark.parametrize("only", ["all", "pypi", "npm"])
def test_file_changed_during_registry_preflight_is_not_uploaded(
    publisher, release_files, monkeypatch: pytest.MonkeyPatch, only: str
):
    manifest, value, _ = release_files
    release = publisher.load_manifest(manifest)
    calls = []
    monkeypatch.setattr(publisher.shutil, "which", lambda name: f"/fake/{name}")

    def modify(*args):
        path = manifest.parent / value["artifacts"][0]["filename"]
        with path.open("ab") as stream:
            stream.write(b"changed")

    monkeypatch.setattr(publisher, "refuse_existing", modify)
    monkeypatch.setattr(publisher.subprocess, "run", lambda *a, **kw: calls.append(a))
    with pytest.raises(publisher.PublishError, match="digest disagrees"):
        publisher.publish(release, only)
    assert calls == []
