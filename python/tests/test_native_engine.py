"""Native requests and diagnostics through real SDK hosts and the offline engine."""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import struct
import subprocess
import zlib
from pathlib import Path

import pytest

import grida.fx as fx
from grida.fx import _api

REPO = Path(__file__).resolve().parents[2]
ENGINE = _api._packaged_binary()
pytestmark = pytest.mark.skipif(
    os.name != "posix" or not ENGINE.is_file(), reason="needs a built engine on POSIX"
)
KEY = "made-up-native-key-28f3"


@pytest.fixture
def native(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    root = tmp_path / "native"
    shutil.copytree(REPO / "fixtures/native/python", root)
    monkeypatch.setenv(_api.BINARY_VARIABLE, str(ENGINE))
    monkeypatch.setenv("GRIDA_FX_NETWORK", "off")
    monkeypatch.setenv("ACME_NATIVE_KEY", KEY)
    for variable in (
        "GG_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "FAL_KEY",
        "TRIPO_API_KEY",
        "ELEVENLABS_API_KEY",
    ):
        monkeypatch.delenv(variable, raising=False)
    return root


def submitted(root: Path) -> list[dict]:
    return [json.loads(line) for line in (root / ".native-submits").read_text().splitlines()]


def run(root: Path, folder: str, **inputs) -> fx.RunResult:
    return fx.run(
        "native.yaml",
        cwd=root,
        run_dir="runs/" + folder,
        live=True,
        max_usd=1,
        inputs={"source": "source.txt", **inputs},
    )


def test_native_files_arbitrary_fields_cancel_resume_and_both_caches(
    native: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    async def interrupted():
        pending = asyncio.create_task(
            fx.run_async(
                "native.yaml",
                cwd=native,
                run_dir="runs/one",
                live=True,
                max_usd=1,
                inputs={"source": "source.txt"},
            )
        )
        try:
            async with asyncio.timeout(20):
                while not (native / ".native-collecting").exists():
                    if pending.done():
                        raise AssertionError((await pending).failures)
                    await asyncio.sleep(0.02)
            cancelled = await fx.cancel_async(
                native / "runs/one", cwd=native, wait=True, timeout="30s"
            )
            assert cancelled.outcome == "completed"
            with pytest.raises(fx.FxError, match="cancelled"):
                await pending
        finally:
            if not pending.done():
                pending.cancel()
                await asyncio.gather(pending, return_exceptions=True)

    assert fx.plan("native.yaml", cwd=native, inputs={"source": "source.txt"}).ok
    asyncio.run(interrupted())
    snapshot = fx.load_run(native / "runs/one").snapshot()
    stopped = fx.RunResult(native / "runs/one", snapshot["events"])
    assert not stopped.ok
    first = submitted(native)
    assert len(first) == 1
    assert first[0]["future"] == 70000
    assert first[0]["source"] == "Synthetic source bytes.\n"
    assert first[0]["vendor_extension"] == {"new_feature": True}
    jobs = list((native / ".fx/cache/jobs").rglob("*.json"))
    assert len(jobs) == 1 and json.loads(jobs[0].read_text())["state"] == "submitted"
    (native / ".native-release").touch()
    resumed = run(native, "one")
    assert resumed.ok, resumed.failures
    assert len(submitted(native)) == 1
    assert resumed.cost == pytest.approx(stopped.cost)
    assert not list((native / ".fx/cache/jobs").rglob("*.json"))

    # A fresh record reuses the step; a changed type forces the body to execute and proves
    # call-cache replay independently of the result cache, still with no live admission.
    monkeypatch.delenv("ACME_NATIVE_KEY")
    cached = fx.run(
        "native.yaml", cwd=native, run_dir="runs/cached", inputs={"source": "source.txt"}
    )
    assert cached.ok
    nodes = native / "nodes.py"
    nodes.write_text(nodes.read_text().replace("version=1", "version=2"))
    replayed = fx.run(
        "native.yaml", cwd=native, run_dir="runs/replay", inputs={"source": "source.txt"}
    )
    assert replayed.ok
    assert any(e.get("event") == "call" and e.get("cached") for e in replayed.events)
    assert len(submitted(native)) == 1

    monkeypatch.setenv("ACME_NATIVE_KEY", KEY)
    assert run(native, "new-field", future=90001).ok
    (native / "source.txt").write_text("Different synthetic source.\n")
    assert run(native, "new-file", future=90001).ok
    assert run(native, "new-operation", future=90001, operation="future_mesh_operation").ok
    records = submitted(native)
    assert len(records) == 4
    assert len({entry["key"] for entry in records}) == 4


def test_provider_diagnostics_reach_sdk_cli_observation_and_uncertain_jobs(native: Path) -> None:
    rejected = run(native, "reject", mode="reject")
    assert not rejected.ok
    failure = rejected.failures["native#1"]
    diagnostic = failure.diagnostic
    assert diagnostic is not None
    assert diagnostic["source"] == "provider" and diagnostic["phase"] == "submit"
    assert diagnostic["http_status"] == 400 and diagnostic["code"] == "bad_option"
    assert diagnostic["field"] == "future_face_budget"
    assert diagnostic["request_id"] == "req-native-1"
    assert diagnostic["redacted"]
    assert "Unsupported future_face_budget" in failure.message
    assert len(submitted(native)) == 1  # a deterministic refusal is not retried
    snapshot = fx.load_run(rejected.run_dir).snapshot()
    event = next(e for e in snapshot["events"] if e["event"] == "node_failed")
    assert event["diagnostic"] == diagnostic
    for command in ("project", "inspect"):
        args = [str(ENGINE), command, str(rejected.run_dir)]
        if command == "inspect":
            args.append("--json")
        result = subprocess.run(args, capture_output=True, text=True, check=True)
        document = json.loads(result.stdout)
        actual = (
            document["instances"]["native#1"]
            if command == "project"
            else document["run"]["steps"][0]
        )
        assert actual["diagnostic"] == diagnostic
    portable = json.dumps(snapshot)
    for forbidden in (
        KEY,
        "made-up-signature",
        "/private/native/input.txt",
        "Synthetic source bytes.",
    ):
        assert forbidden not in portable

    uncertain = run(native, "uncertain", mode="uncertain")
    original = uncertain.failures["native#1"].diagnostic
    assert original and original["http_status"] == 502
    assert uncertain.failures["native#1"].code == "job_unsettled"
    again = run(native, "uncertain", mode="uncertain")
    assert again.failures["native#1"].diagnostic == original
    assert len(submitted(native)) == 2  # one rejected, one uncertain; no second uncertain POST
    job = next(
        job
        for path in (native / ".fx/cache/jobs").rglob("*.json")
        if (job := json.loads(path.read_text()))["state"] == "submitting"
    )
    assert job["state"] == "submitting" and job["diagnostic"] == original


def test_single_image_workflow_plans_before_the_image_exists(native: Path) -> None:
    (native / "fx.yaml").write_text("fx: project/v1\n")
    workflow = native / "mesh.yaml"
    source = """fx: workflow/v1
id: mesh
title: Image to mesh
steps:
  concept:
    uses: fx/image.generate@1
    route: gpt-image-2.5-sunburst@openai
    with: { prompt: An isolated woodland rock, size: 1024x1024 }
  mesh:
    uses: fx/mesh.generate@1
    route: ROUTE
    with:
SOURCE
      face_limit: 1200
      texture: true
      pbr: false
"""
    image = "      image: ${{ steps.concept.outputs.image }}"
    views = """      views:
        front: ${{ steps.concept.outputs.image }}
        back: ${{ steps.concept.outputs.image }}"""
    for fields, route, expected in (
        (image, "P2-20260801@tripo", None),
        (views, "P2-20260801@tripo", None),
        (image, "tripo/p2@gg", "does not support image_input"),
        (image + "\n" + views, "P2-20260801@tripo", "exactly one"),
        ("", "P2-20260801@tripo", "exactly one"),
    ):
        workflow.write_text(source.replace("ROUTE", route).replace("SOURCE", fields))
        planned = fx.plan("mesh.yaml", cwd=native)
        if expected is None:
            assert planned.ok, planned.problems
        else:
            assert not planned.ok
            assert expected in str(planned.problems)


@pytest.mark.parametrize("field", ["code", "field", "request_id", "task_id", "operation"])
def test_metadata_input_echoes_are_withheld_from_errors_records_and_exports(
    native: Path, field: str
) -> None:
    payload = "private_native_file_bytes"
    (native / "source.txt").write_text(payload)
    adapter = native / "adapter.py"
    text = adapter.read_text()
    original = {
        "code": 'code="bad_option"',
        "field": 'field="future_face_budget"',
        "request_id": 'request_id="req-native-1"',
        "operation": 'operation=call.request["operation"]',
    }
    if field == "task_id":
        text = text.replace(
            'request_id="req-native-1",',
            'request_id="req-native-1", task_id=source.read_bytes().decode(),',
        )
    else:
        text = text.replace(original[field], f"{field}=source.read_bytes().decode()")
    adapter.write_text(text)
    for mode in ("reject", "uncertain"):
        result = run(native, mode, mode=mode)
        diagnostic = result.failures["native#1"].diagnostic
        assert diagnostic is not None and field not in diagnostic
        assert diagnostic["redacted"]
        record = fx.load_run(result.run_dir)
        assert payload not in json.dumps(record.snapshot())
        assert payload not in result.failures["native#1"].message
        destination = native / ("export-" + mode)
        record.export(destination, include_diagnostics=True)
        for path in destination.rglob("*.json"):
            assert payload not in path.read_text()
        for path in (native / ".fx/cache/jobs").rglob("*.json"):
            assert payload not in path.read_text()


def test_local_mesh_refusals_keep_adapter_provenance_end_to_end(
    native: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (native / "fx.yaml").write_text("fx: project/v1\n")
    monkeypatch.setenv("TRIPO_API_KEY", "made-up-tripo-validation-key")

    # A synthesized one-pixel RGB PNG, with no processing-library dependency.
    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack("!I", len(data)) + kind + data + struct.pack("!I", zlib.crc32(kind + data))
        )

    image = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack("!2I5B", 1, 1, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(b"\0\x7f\x7f\x7f"))
        + chunk(b"IEND", b"")
    )
    (native / "image.png").write_bytes(image)
    (native / "mesh.yaml").write_text("""fx: workflow/v1
id: mesh
title: Local mesh refusal
inputs:
  source: { type: file, kind: image/png }
steps:
  mesh:
    uses: fx/mesh.generate@1
    route: P2-20260801@tripo
    with:
      image: ${{ inputs.source }}
      provider_options: { tripo: { model: override } }
""")
    result = fx.run(
        "mesh.yaml",
        cwd=native,
        run_dir="runs/local",
        live=True,
        max_usd=10,
        inputs={"source": "image.png"},
    )
    failure = result.failures["mesh#1"]
    assert failure.code == "capability_refused"
    diagnostic = failure.diagnostic
    assert diagnostic and diagnostic["source"] == "adapter" and diagnostic["phase"] == "validate"
    assert "http_status" not in diagnostic
    assert result.cost == 0
    record = fx.load_run(result.run_dir)
    event = next(e for e in record.snapshot()["events"] if e["event"] == "node_failed")
    assert event["diagnostic"] == diagnostic
    assert record.inspection["run"]["steps"][0]["diagnostic"] == diagnostic
