"""The optional public showcase input is fetched only through injected test bytes."""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
from pathlib import Path
from urllib.request import Request

import pytest

REPO = Path(__file__).resolve().parents[2]
SOURCE = REPO / "examples/looping-parallax/fetch_showcase_input.py"
SPEC = importlib.util.spec_from_file_location("parallax_showcase_input_under_test", SOURCE)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
DATA = b"test-only public input bytes"
DIGEST = hashlib.sha256(DATA).hexdigest()
PUBLIC_BASE = "https://cdn.example.test"


class Response(io.BytesIO):
    def __init__(self, data, url, length=None):
        super().__init__(data)
        self.url = url
        self.headers = {} if length is None else {"Content-Length": str(length)}

    def geturl(self):
        return self.url


def fixture(project, **changes):
    document = {
        "schema": "fx_showcase_input_v1",
        "delivery_base": PUBLIC_BASE,
        "path": f"runs/example/artifacts/{DIGEST}.png",
        "sha256": DIGEST,
        "bytes": len(DATA),
        "destination": MODULE.DESTINATION,
    }
    document.update(changes)
    (project / "showcase-input.json").write_text(json.dumps(document))
    return document


def fetcher(data=DATA, requests=None, *, length=None, final_url=None):
    def open_fake(request, *, timeout):
        assert timeout == MODULE.TIMEOUT_SECONDS
        if requests is not None:
            requests.append(request.full_url)
        return Response(data, final_url or request.full_url, length=length)

    return open_fake


def test_download_verified_before_atomic_persistence_and_repeat_is_offline(tmp_path):
    fixture(tmp_path)
    requests = []
    path = MODULE.fetch_showcase_input(tmp_path, opener=fetcher(requests=requests))
    assert path.read_bytes() == DATA
    inode = path.stat().st_ino
    assert MODULE.fetch_showcase_input(tmp_path, opener=lambda *_a, **_kw: pytest.fail("network"))
    assert path.stat().st_ino == inode
    assert len(requests) == 1
    assert sorted(item.name for item in path.parent.iterdir()) == [path.name]


def test_base_override_preserves_immutable_path_and_digest(tmp_path):
    document = fixture(tmp_path)
    requests = []
    path = MODULE.fetch_showcase_input(
        tmp_path,
        base_url="https://different.example.test/media/",
        opener=fetcher(requests=requests),
    )
    assert requests == [f"https://different.example.test/media/{document['path']}"]
    assert path.read_bytes() == DATA
    assert json.loads((tmp_path / "showcase-input.json").read_text()) == document


@pytest.mark.parametrize("data", [b"x" * len(DATA), DATA[:-1], DATA + b"extra"])
def test_wrong_bytes_leave_no_input_or_partial_file(tmp_path, data):
    fixture(tmp_path)
    with pytest.raises(ValueError, match="pinned"):
        MODULE.fetch_showcase_input(tmp_path, opener=fetcher(data))
    assert list((tmp_path / ".showcase-inputs").iterdir()) == []


def test_conflicting_existing_art_is_never_replaced_or_downloaded(tmp_path):
    fixture(tmp_path)
    path = tmp_path / MODULE.DESTINATION
    path.parent.mkdir()
    path.write_bytes(b"user artwork")
    with pytest.raises(ValueError, match="conflicts"):
        MODULE.fetch_showcase_input(tmp_path, opener=lambda *_a, **_kw: pytest.fail("network"))
    assert path.read_bytes() == b"user artwork"


def test_corrupt_existing_art_with_equal_length_is_not_replaced(tmp_path):
    fixture(tmp_path)
    path = tmp_path / MODULE.DESTINATION
    path.parent.mkdir()
    path.write_bytes(b"x" * len(DATA))
    with pytest.raises(ValueError, match="conflicts"):
        MODULE.fetch_showcase_input(tmp_path, opener=lambda *_a, **_kw: pytest.fail("network"))
    assert path.read_bytes() == b"x" * len(DATA)


@pytest.mark.parametrize("kind", ["parent", "file"])
def test_symlink_destinations_are_refused(tmp_path, kind):
    project = tmp_path / "project"
    project.mkdir()
    fixture(project)
    outside = tmp_path / "outside"
    outside.mkdir()
    if kind == "parent":
        (project / ".showcase-inputs").symlink_to(outside, target_is_directory=True)
    else:
        directory = project / ".showcase-inputs"
        directory.mkdir()
        target = outside / "original.png"
        target.write_bytes(DATA)
        (directory / "meadow-concept.png").symlink_to(target)
    with pytest.raises(ValueError, match="symlink"):
        MODULE.fetch_showcase_input(project, opener=lambda *_a, **_kw: pytest.fail("network"))
    assert not (outside / "meadow-concept.png").exists()


@pytest.mark.parametrize(
    "change",
    [
        {"destination": "../escape.png"},
        {"path": f"../{DIGEST}.png"},
        {"path": f"/absolute/{DIGEST}.png"},
        {"path": f"runs/%2e%2e/{DIGEST}.png"},
        {"path": f"runs//{DIGEST}.png"},
        {"bytes": MODULE.MAX_BYTES + 1},
        {"bytes": True},
        {"sha256": "wrong"},
    ],
)
def test_invalid_manifest_is_rejected_before_writing_or_network(tmp_path, change):
    fixture(tmp_path, **change)
    with pytest.raises(ValueError):
        MODULE.fetch_showcase_input(tmp_path, opener=lambda *_a, **_kw: pytest.fail("network"))
    assert not (tmp_path / ".showcase-inputs").exists()


@pytest.mark.parametrize(
    "url",
    [
        "http://cdn.example.test",
        "https://user:secret@cdn.example.test",
        "https://cdn.example.test/?signature=secret",
        "https://cdn.example.test/#fragment",
        "https://localhost",
        "https://127.0.0.1",
        "https://10.0.0.1",
        "https://cdn.example.test:8080",
        "https://cdn.example.test/../escape",
        "https://cdn.example.test/%2e%2e/escape",
        "https://cdn.example.test/white space",
    ],
)
def test_unsafe_cdn_override_is_refused_before_writing_or_network(tmp_path, url):
    fixture(tmp_path)
    with pytest.raises(ValueError):
        MODULE.fetch_showcase_input(
            tmp_path, base_url=url, opener=lambda *_a, **_kw: pytest.fail("network")
        )
    assert not (tmp_path / ".showcase-inputs").exists()


def test_unexpected_content_length_is_refused_and_cleaned_up(tmp_path):
    fixture(tmp_path)
    with pytest.raises(ValueError, match="content length"):
        MODULE.fetch_showcase_input(tmp_path, opener=fetcher(length=len(DATA) + 1))
    assert list((tmp_path / ".showcase-inputs").iterdir()) == []


def test_insecure_redirect_is_refused_before_following():
    with pytest.raises(ValueError, match="HTTPS"):
        MODULE.SafeRedirects().redirect_request(
            Request(PUBLIC_BASE), None, 302, "Found", {}, "http://cdn.example.test/input.png"
        )


def test_invalid_final_url_leaves_no_partial_input(tmp_path):
    fixture(tmp_path)
    with pytest.raises(ValueError, match="HTTPS"):
        MODULE.fetch_showcase_input(tmp_path, opener=fetcher(final_url="http://cdn.example.test"))
    assert list((tmp_path / ".showcase-inputs").iterdir()) == []


def test_transport_failure_leaves_no_partial_input(tmp_path):
    fixture(tmp_path)

    def failed(*_args, **_kwargs):
        raise TimeoutError("test transport timed out")

    with pytest.raises(TimeoutError):
        MODULE.fetch_showcase_input(tmp_path, opener=failed)
    assert list((tmp_path / ".showcase-inputs").iterdir()) == []


def test_committed_manifest_is_valid_and_pins_the_public_concept():
    document = MODULE.load_manifest(SOURCE.with_name("showcase-input.json"))
    assert document["bytes"] == 2016648
    assert document["sha256"] == "628b89a8167bf54782e98b8578cfe601d3cc181bb34dfc59912d708f3221d138"
