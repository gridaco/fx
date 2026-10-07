"""The contributor harness must never copy its source into itself or overwrite evidence."""

import importlib.util
import json
from pathlib import Path

import pytest


@pytest.fixture
def fixture_tool(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    path = Path(__file__).resolve().parents[2] / "tools" / "check_viewer_fixtures.py"
    spec = importlib.util.spec_from_file_location("viewer_fixture_tool", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    source = tmp_path / "fixtures" / "viewer"
    source.mkdir(parents=True)
    monkeypatch.setattr(module, "SOURCE", source)
    return module


def test_output_cannot_enter_source_through_dotdot(fixture_tool, tmp_path: Path):
    destination = tmp_path / "fixtures" / ".." / "fixtures" / "viewer" / "generated"
    with pytest.raises(ValueError, match="overlap the fixture source"):
        fixture_tool.prepare_output(destination)
    assert not (fixture_tool.SOURCE / "generated").exists()


def test_output_cannot_contain_source(fixture_tool, tmp_path: Path):
    with pytest.raises(ValueError, match="overlap the fixture source"):
        fixture_tool.prepare_output(tmp_path)


def test_output_preserves_existing_evidence(fixture_tool, tmp_path: Path):
    destination = tmp_path / "evidence"
    destination.mkdir()
    marker = destination / "previous.json"
    marker.write_text("previous", encoding="utf-8")
    with pytest.raises(ValueError, match="not empty"):
        fixture_tool.prepare_output(destination)
    assert marker.read_text(encoding="utf-8") == "previous"


def test_output_refuses_symbolic_link_parents(fixture_tool, tmp_path: Path):
    target = tmp_path / "destination"
    target.mkdir()
    link = tmp_path / "alias"
    link.symlink_to(target, target_is_directory=True)
    with pytest.raises(ValueError, match="symbolic link"):
        fixture_tool.prepare_output(link / "new")
    assert not (target / "new").exists()


def test_empty_output_directory_is_created(fixture_tool, tmp_path: Path):
    destination = tmp_path / "output" / "new"
    assert fixture_tool.prepare_output(destination) == destination.resolve()
    assert destination.is_dir()
    assert not list(destination.iterdir())


def test_legacy_copy_preserves_data_and_does_not_modify_the_original(fixture_tool, tmp_path: Path):
    source = tmp_path / "current"
    source.mkdir()
    graph = {
        "scopes": [{"id": "scope:imported#1", "nodes": ["render#1"]}],
        "types": {"example": {"identity": "example@1", "ports": {"outputs": {"image": "image"}}}},
        "instances": [
            {
                "id": "render#1",
                "bindings": [{"source": "input#1", "source_port": "image"}],
                "interface_bindings": [{"source": "scope:imported#1", "source_port": "image"}],
                "needs": ["input#1"],
                "with": {"ports": "authored value", "bindings": [1, 2], "scopes": "authored data"},
            }
        ],
    }
    started = {
        "event": "node_started",
        "ports": {},
        "bindings": [],
        "interface_bindings": [],
        "needs": [],
        "judges": None,
    }
    snapshot = {"event": "scopes_updated", "scopes": graph["scopes"]}
    finished = {"event": "node_finished", "outputs": {"report": {"value": {"ports": "data"}}}}
    fixture_tool.write_json(source / "plan.json", graph)
    (source / "events.jsonl").write_text(
        "".join(json.dumps(event) + "\n" for event in [started, snapshot, finished])
    )
    (source / "artifact.bin").write_bytes(b"original artifact bytes")
    original = {file.name: file.read_bytes() for file in source.iterdir()}

    target = tmp_path / "legacy"
    fixture_tool.legacy_run(source, target)

    assert {file.name: file.read_bytes() for file in source.iterdir()} == original
    old = json.loads((target / "plan.json").read_text())
    assert "ports" not in old["types"]["example"]
    assert "scopes" not in old
    assert "bindings" not in old["instances"][0]
    assert "interface_bindings" not in old["instances"][0]
    assert old["instances"][0]["needs"] == ["input#1"]
    assert old["instances"][0]["with"] == graph["instances"][0]["with"]
    assert fixture_tool.events(target / "events.jsonl") == [{"event": "node_started"}, finished]
    assert (target / "artifact.bin").read_bytes() == b"original artifact bytes"
