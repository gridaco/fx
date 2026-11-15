"""Native file access carries portable references to the engine, never reconstructing paths."""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from typing import Any

import pytest
from test_api import Fake

import grida.fx as fx
from grida.fx import _api

pytestmark = pytest.mark.skipif(os.name != "posix", reason="the fake binary is a script")

ARTIFACT = {
    "kind": "artifact",
    "artifact": {
        "digest": "a" * 64,
        "kind": "image/png",
        "name": "picture.png",
        "size": 12,
        "url": None,
    },
}
SOURCE = {
    "kind": "source",
    "source": "nodes/draw.py",
    "symbol": "draw",
    "recorded_digest": "b" * 64,
}
LIST = {
    "kind": "fx-files-v1",
    "files": [
        {
            "target": ARTIFACT,
            "role": "artifact",
            "instance_id": "draw#1",
            "scope_id": "scope:child#1",
        },
        {"target": SOURCE, "role": "node", "instance_id": "draw#1"},
    ],
}


def reference(target: dict[str, Any]) -> dict[str, Any]:
    return {"subject": {"kind": "run", "workflow": {"id": "case"}}, "target": target}


@pytest.fixture
def fake(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Fake:
    folder = tmp_path / "bin"
    folder.mkdir()
    engine = Fake(folder, monkeypatch)
    engine.reply(files={"stdout": json.dumps(LIST)})
    return engine


def test_list_plans_and_pin_record_with_explicit_bindings(fake: Fake, tmp_path: Path) -> None:
    listed = fx.list_files(
        "plan.json", project="project folder", instance_id="draw#1", cwd=tmp_path
    )
    assert listed == [
        fx.FileDescriptor(ARTIFACT, "artifact", "draw#1", "scope:child#1"),
        fx.FileDescriptor(SOURCE, "node", "draw#1"),
    ]
    assert fake.invocations == [
        [
            "files",
            "list",
            "plan.json",
            "--json",
            f"--project={tmp_path}/project folder",
            "--instance=draw#1",
        ]
    ]
    record = fx.RunRecord(tmp_path / "runs/pinned", {"run": {"folder": "runs/pinned"}})
    record.inspection["run"]["folder"] = "runs/changed"
    assert record.list_files() == listed
    assert fake.invocations[-1] == ["files", "list", str(record.run_dir), "--json"]


def test_resolve_changed_source_and_unavailable_copy(fake: Fake, tmp_path: Path) -> None:
    local = tmp_path / "nodes/draw.py"
    fake.reply(
        files={
            "stdout": json.dumps(
                {
                    "kind": "fx-file-resolution-v1",
                    "target": SOURCE,
                    "local_path": str(local),
                    "source_match": "changed",
                    "reason": None,
                }
            )
        }
    )
    record = fx.RunRecord(tmp_path / "run", {})
    result = record.resolve_file(
        reference(SOURCE), project=tmp_path, source_root=tmp_path / "owner"
    )
    assert result == fx.FileResolution(SOURCE, local, "changed", None)
    args = fake.invocations[-1]
    assert args[:4] == ["files", "resolve", str(record.run_dir), "--reference"]
    assert json.loads(args[4]) == reference(SOURCE)
    assert args[5:] == ["--json", f"--project={tmp_path}", f"--source-root={tmp_path}/owner"]
    fake.reply(
        files={
            "stdout": json.dumps(
                {
                    "kind": "fx-file-resolution-v1",
                    "target": ARTIFACT,
                    "local_path": None,
                    "source_match": None,
                    "reason": "Recorded bytes are unavailable.",
                }
            )
        }
    )
    assert fx.resolve_file("plan.json", reference(ARTIFACT), cwd=tmp_path).local_path is None


def test_structured_refusal_preserves_unknown_code(fake: Fake) -> None:
    refusal = {
        "kind": "fx-file-error-v1",
        "code": "future_refusal",
        "message": "Unavailable.",
        "future": True,
    }
    fake.reply(files={"stdout": json.dumps(refusal), "status": 2})
    with pytest.raises(fx.FileAccessError) as caught:
        fx.resolve_file("run", reference(ARTIFACT))
    assert caught.value.code == "future_refusal"
    assert caught.value.document == refusal
    assert caught.value.status == 2


@pytest.mark.parametrize(
    "body",
    [
        "not JSON",
        "[]",
        "{}",
        json.dumps(
            {"kind": "fx-files-v1", "files": [{"target": SOURCE, "role": "node", "instance_id": 3}]}
        ),
        json.dumps({"kind": "fx-files-v1", "files": [{"target": SOURCE, "role": {}}]}),
        json.dumps(
            {
                "kind": "fx-files-v1",
                "files": [
                    {"target": {"kind": "source", "source": "/private/file.py"}, "role": "source"}
                ],
            }
        ),
        json.dumps(
            {
                "kind": "fx-files-v1",
                "files": [
                    {
                        "target": {**ARTIFACT, "artifact": {**ARTIFACT["artifact"], "size": True}},
                        "role": "artifact",
                    }
                ],
            }
        ),
    ],
)
def test_reject_malformed_file_inventory(fake: Fake, body: str) -> None:
    fake.reply(files={"stdout": body})
    with pytest.raises(fx.FxError):
        fx.list_files("run")


@pytest.mark.parametrize(
    "change",
    [
        {"local_path": "relative.py"},
        {"source_match": False},
        {"reason": 1},
        {"target": {}},
        {"source_match": []},
        {"local_path": "/bad\0path"},
    ],
)
def test_reject_malformed_resolution(fake: Fake, tmp_path: Path, change: dict[str, Any]) -> None:
    document = {
        "kind": "fx-file-resolution-v1",
        "target": SOURCE,
        "local_path": str(tmp_path / "file.py"),
        "source_match": "unknown",
        "reason": None,
        **change,
    }
    fake.reply(files={"stdout": json.dumps(document)})
    with pytest.raises(fx.FxError):
        fx.resolve_file("run", reference(SOURCE))


def test_reject_invalid_arguments_before_spawning(fake: Fake) -> None:
    for invalid in ("", "bad\0target", "--option"):
        with pytest.raises(ValueError):
            fx.list_files(invalid)
    with pytest.raises(ValueError):
        fx.list_files("run", project="")
    with pytest.raises(ValueError):
        fx.resolve_file("run", reference(SOURCE), source_root="")
    with pytest.raises(TypeError):
        fx.resolve_file("run", reference({"kind": "source", "source": "../outside.py"}))
    assert fake.invocations == []


@pytest.mark.parametrize(
    "target",
    [
        *(
            {**ARTIFACT, "artifact": {**ARTIFACT["artifact"], member: value}}
            for member, value in [
                ("kind", ""),
                ("kind", "image/png\x85"),
                ("name", "/private/file.png"),
                ("name", "C:\\workspace\\file.png"),
                ("name", "FILE:///private/file.png"),
                ("name", "bad\nname"),
                ("name", "bad\x85name"),
                ("url", "/api/file?"),
                ("url", "/api/file#"),
                ("url", "/api/file\x85"),
                ("url", "https://acme.invalid/file?"),
                ("url", "https://acme.invalid/file#"),
                ("url", "https://user:secret@acme.invalid/file"),
                ("url", "https://@acme.invalid/file"),
                ("url", "https://acme.invalid\\file"),
                ("url", "https://acme.invalid/file with spaces"),
                ("url", "https://acm\u00e9.invalid/file"),
                ("url", "//acme.invalid/file"),
                ("url", "http:relative"),
                ("size", True),
                ("extra", "not a target field"),
            ]
        ),
        {**ARTIFACT, "local_path": "/private/file.png"},
        {**SOURCE, "source": "nodes/bad\x85.py"},
        {**SOURCE, "local_path": "/private/file.py"},
        {"kind": "source", "source": "nodes/source.py", "symbol": "bad\nname"},
        {"kind": "source", "source": "nodes/source.py", "recorded_digest": "bad"},
    ],
)
def test_portable_target_validation_matches_browser_contract(fake: Fake, target: dict) -> None:
    with pytest.raises(TypeError, match="portable viewer file reference"):
        fx.resolve_file("run", reference(target))
    assert fake.invocations == []
    fake.reply(
        files={
            "stdout": json.dumps(
                {"kind": "fx-files-v1", "files": [{"target": target, "role": "source"}]}
            )
        }
    )
    with pytest.raises(fx.FxError, match="malformed file descriptors"):
        fx.list_files("run")


def test_portable_names_and_source_labels_allow_unicode(fake: Fake) -> None:
    targets = [
        {
            **ARTIFACT,
            "artifact": {**ARTIFACT["artifact"], "name": "\u753b.png", "url": "/api/file"},
        },
        {"kind": "source", "source": "nodes/\u753b.py"},
    ]
    fake.reply(
        files={
            "stdout": json.dumps(
                {
                    "kind": "fx-files-v1",
                    "files": [{"target": target, "role": "source"} for target in targets],
                }
            )
        }
    )
    assert [file.target for file in fx.list_files("run")] == targets


def test_async_native_jobs_and_record_methods(fake: Fake, tmp_path: Path) -> None:
    async def use() -> None:
        record = fx.RunRecord(tmp_path / "run", {})
        assert (await record.list_files_async())[1].target == SOURCE
        assert (await fx.list_files_async("plan.json"))[0].target == ARTIFACT
        fake.reply(
            files={
                "stdout": json.dumps(
                    {
                        "kind": "fx-file-resolution-v1",
                        "target": ARTIFACT,
                        "local_path": None,
                        "source_match": None,
                        "reason": "Unavailable.",
                    }
                )
            }
        )
        assert (await record.resolve_file_async(reference(ARTIFACT))).local_path is None
        assert (await fx.resolve_file_async("plan.json", reference(ARTIFACT))).local_path is None
        for call in (record.list_files, lambda: record.resolve_file(reference(ARTIFACT))):
            with pytest.raises(RuntimeError, match="async"):
                call()

    asyncio.run(use())


def test_real_engine_resolves_bytes_and_changed_source_without_import(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    engine = _api._packaged_binary()
    if not engine.is_file():
        pytest.skip("needs the engine beside the SDK")
    monkeypatch.setenv("GRIDA_FX_BIN", str(engine))
    monkeypatch.setenv("GRIDA_FX_NETWORK", "off")
    monkeypatch.setenv("GRIDA_FX_DISABLE_DOTENV", "1")
    for key in (
        "GG_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
    ):
        monkeypatch.delenv(key, raising=False)
    (tmp_path / "nodes").mkdir()
    (tmp_path / "fx.yaml").write_text("fx: project/v1\n")
    node = tmp_path / "nodes/source.py"
    node.write_text(
        "from grida.fx import node\n"
        '@node("make", outputs={"text":"text"}, version=1)\n'
        "def make(ctx):\n"
        '    return {"text":ctx.out.text("original bytes")}\n'
    )
    (tmp_path / "workflow.yaml").write_text(
        "fx: workflow/v1\nid: file-access\ntitle: File access\nsteps:\n"
        "  make:\n    uses: ./nodes/source.py#make\noutputs:\n"
        "  result: ${{ steps.make.outputs.text }}\n"
    )
    result = fx.run("workflow.yaml", max_usd=0, cwd=tmp_path)
    assert result.ok
    record = fx.load_run(result.run_dir)
    files = record.list_files()
    artifact = next(file for file in files if file.target["kind"] == "artifact")
    subject = {"kind": "run", "workflow": {"id": "file-access"}}
    resolved = record.resolve_file({"subject": subject, "target": artifact.target})
    assert resolved.local_path is not None
    assert resolved.local_path.read_text() == "original bytes"
    source = next(file for file in files if file.target.get("source") == "nodes/source.py")
    node.write_text('raise RuntimeError("source lookup must not import this")\n')
    changed = record.resolve_file({"subject": subject, "target": source.target}, project=tmp_path)
    assert changed.local_path == node.resolve()
    assert changed.source_match == "changed"
