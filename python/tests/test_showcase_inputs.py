"""The example input downloader checks injected bytes, never an external service."""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "showcase_inputs_under_test", ROOT / "examples/fetch_showcase_inputs.py"
)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
DATA = b"synthetic input, never generated artwork"
BASE = "https://assets.example.test"


class Response(io.BytesIO):
    def __init__(self, data, url):
        super().__init__(data)
        self.url = url
        self.headers = {"Content-Length": str(len(data))}

    def geturl(self):
        return self.url


def artifact(filename="input.png"):
    return {
        "filename": filename,
        "path": "showcases/pinned/input.png",
        "sha256": hashlib.sha256(DATA).hexdigest(),
        "bytes": len(DATA),
    }


def manifest(project, inputs=None):
    document = {
        "schema": "fx_showcase_inputs_v1",
        "delivery_base": BASE,
        "inputs": inputs if inputs is not None else [artifact()],
    }
    (project / "showcase-inputs.json").write_text(json.dumps(document))
    return document


def fetcher(data=DATA, requests=None):
    def fetch(request, *, timeout):
        assert timeout == MODULE.TIMEOUT_SECONDS
        if requests is not None:
            requests.append(request.full_url)
        return Response(data, request.full_url)

    return fetch


def no_network(*_args, **_kwargs):
    pytest.fail("Already verified inputs must not use the network")


def test_multiple_verified_inputs_and_idempotent_reuse(tmp_path):
    manifest(tmp_path, [artifact(), artifact("take.mp4")])
    requests = []
    paths = MODULE.fetch_showcase_inputs(tmp_path, opener=fetcher(requests=requests))
    assert len(paths) == len(requests) == 2
    assert all(path.read_bytes() == DATA for path in paths)
    assert MODULE.fetch_showcase_inputs(tmp_path, opener=no_network) == paths
    assert sorted(path.name for path in paths[0].parent.iterdir()) == ["input.png", "take.mp4"]


def test_cdn_override_changes_only_base(tmp_path):
    original = manifest(tmp_path)
    requests = []
    MODULE.fetch_showcase_inputs(
        tmp_path, base_url="https://cdn.example.test/root/", opener=fetcher(requests=requests)
    )
    assert requests == ["https://cdn.example.test/root/showcases/pinned/input.png"]
    assert json.loads((tmp_path / "showcase-inputs.json").read_text()) == original


@pytest.mark.parametrize("data", [DATA[:-1], DATA + b"x", b"x" * len(DATA)])
def test_corruption_never_places_an_input(tmp_path, data):
    manifest(tmp_path)
    with pytest.raises(ValueError, match="pinned"):
        MODULE.fetch_showcase_inputs(tmp_path, opener=fetcher(data))
    assert list((tmp_path / ".showcase-inputs").iterdir()) == []


def test_existing_user_file_is_never_overwritten(tmp_path):
    manifest(tmp_path)
    folder = tmp_path / ".showcase-inputs"
    folder.mkdir()
    (folder / "input.png").write_bytes(b"user file")
    with pytest.raises(ValueError, match="conflicts"):
        MODULE.fetch_showcase_inputs(tmp_path, opener=no_network)
    assert (folder / "input.png").read_bytes() == b"user file"


@pytest.mark.parametrize(
    "field,value",
    [
        ("filename", "../outside.png"),
        ("filename", "/outside.png"),
        ("path", "../outside.png"),
        ("path", "https://other.example.test/input.png"),
        ("path", "inputs//file.png"),
        ("sha256", "bad"),
        ("bytes", True),
        ("bytes", 0),
        ("bytes", MODULE.MAX_BYTES + 1),
    ],
)
def test_invalid_manifest_is_rejected_before_any_file_is_downloaded(tmp_path, field, value):
    bad = artifact("second.png")
    bad[field] = value
    manifest(tmp_path, [artifact(), bad])
    with pytest.raises(ValueError):
        MODULE.fetch_showcase_inputs(tmp_path, opener=no_network)
    assert not (tmp_path / ".showcase-inputs").exists()


def test_duplicate_filenames_are_refused(tmp_path):
    manifest(tmp_path, [artifact(), artifact()])
    with pytest.raises(ValueError, match="unique"):
        MODULE.fetch_showcase_inputs(tmp_path, opener=no_network)


@pytest.mark.parametrize("kind", ["directory", "file"])
def test_symlink_destinations_are_refused(tmp_path, kind):
    project, outside = tmp_path / "project", tmp_path / "outside"
    project.mkdir()
    outside.mkdir()
    manifest(project)
    directory = project / ".showcase-inputs"
    if kind == "directory":
        directory.symlink_to(outside, target_is_directory=True)
    else:
        directory.mkdir()
        target = outside / "original.png"
        target.write_bytes(DATA)
        (directory / "input.png").symlink_to(target)
    with pytest.raises(ValueError, match="symlink"):
        MODULE.fetch_showcase_inputs(project, opener=no_network)
    assert not (outside / "input.png").exists()


def test_committed_inputs_are_bound_to_included_public_website_bytes():
    content = ROOT / "www/content/showcases"
    lock = json.loads((content / "lock.json").read_text())
    release = json.loads((content / lock["release"]["fixture"]).read_text())
    resources = {
        f"showcases/{lock['release']['sha256']}/{item['path']}": (item["sha256"], item["bytes"])
        for item in release["files"]
    }
    for entry in lock["workflows"].values():
        for recording in entry["recordings"]:
            document = json.loads((content / recording["fixture"]).read_text())
            for item in document["artifacts"]:
                if item["path"] is not None:
                    resources[f"runs/{recording['sha256']}/{item['path']}"] = (
                        item["digest"],
                        item["size"],
                    )
    for name in ("world", "terrain", "ui-kit", "movie-sprite", "portrait-motion"):
        document = MODULE.load_manifest(ROOT / "examples" / name / "showcase-inputs.json")
        for item in document["inputs"]:
            assert resources[item["path"]] == (item["sha256"], item["bytes"])


def test_slow_trickle_is_checked_between_single_reads(tmp_path, monkeypatch):
    manifest(tmp_path)
    clock = [0.0]
    reads = []
    monkeypatch.setattr(MODULE.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(MODULE, "TOTAL_SECONDS", 5)

    class Trickle(Response):
        def read(self, *_args):
            pytest.fail("A filling read can hide the total download deadline")

        def read1(self, size):
            reads.append(size)
            clock[0] += 3
            return super().read1(min(size, 1))

    def open_trickle(request, *, timeout):
        assert timeout == MODULE.TIMEOUT_SECONDS
        return Trickle(DATA, request.full_url)

    with pytest.raises(ValueError, match="time limit"):
        MODULE.fetch_showcase_inputs(tmp_path, opener=open_trickle)
    assert len(reads) == 2
    assert list((tmp_path / ".showcase-inputs").iterdir()) == []
