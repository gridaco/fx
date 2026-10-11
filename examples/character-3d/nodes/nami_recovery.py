"""Replay the measured finishing of the exact published Nami provider rig."""

from __future__ import annotations

import hashlib
import json
import zipfile
from typing import Any

from _character_harness import Bench, atlas_evidence
from _character_tools.profiles import articulation, diagnostic_samples
from _character_tools.requirements import validate_export_space
from _character_tools.studio import Studio
from _character_tools.worker.provider_rig_cli import run as finish_existing_rig

from grida.fx import Ctx, node

ASSEMBLY_SHA = "013667face278a1dbbbb8be1bb3ef49274e75a413226959d7b0f88a658d511a5"
PROVIDER_SHA = "2a77277b4ec5fcccdac89507354cf1c3400765279d8ff3d54261316185fb94a8"


@node(
    "finish",
    inputs={
        "rigged": "model/gltf-binary",
        "assembly": "model/gltf-binary",
        "assembly_record": "json",
        "measurement": "json",
        "profile": "json",
    },
    outputs={
        "model": "model/gltf-binary",
        "record": "json",
        "preservation": "json",
        "worker_evidence": "file",
    },
    tools=["blender"],
    version=1,
)
async def finish(ctx: Ctx) -> dict[str, Any]:
    profile = ctx.read.json("profile")
    measurement = ctx.read.json("measurement")
    bench = Bench(ctx)
    assembly = bench.adopt(ctx.read.bytes("assembly"), "assembly.glb")
    rigged = bench.adopt(ctx.read.bytes("rigged"), "provider.glb")
    if (
        assembly["sha256"] != ASSEMBLY_SHA
        or rigged["sha256"] != PROVIDER_SHA
        or ctx.read.json("assembly_record")["source_sha256"] != ASSEMBLY_SHA
        or measurement["source_sha256"] != ASSEMBLY_SHA
        or measurement["provider_sha256"] != PROVIDER_SHA
        or measurement["default_max_position_error"] != 1e-5
        or measurement["default_uv_tolerance"] != 1e-6
    ):
        raise ctx.fail("Recovery is pinned to the exact retained source and measured transform")
    request = {
        "schema_version": 1,
        "operation": "provider_rig",
        "source": rigged,
        "output_dir": "candidates/rig",
        "options": {
            "mapping_preset": "mixamo_biped",
            "required_joints": list(articulation(profile).PARENTS),
            "diagnostics": True,
            "fps": 24,
            "target_height": profile["target_height"],
            "material_policy": profile["surface_policy"]["default"],
            "preservation": {
                "source": assembly,
                "mode": "audit",
                "transform": measurement["source_to_provider"],
            },
        },
    }
    # Existing hash-bound worker, pure Python for exact GLB inputs; no provider or retry.
    result = finish_existing_rig(request, bench.root, bench.worker.run_root)
    report = json.loads(bench.run_file(result["report"]).read_text())
    exported = bench.run_file(result["output"]["path"])
    exported_source = {
        "path": exported.relative_to(bench.root).as_posix(),
        "sha256": result["output"]["sha256"],
    }
    inspection, _ = await bench.worker.execute(
        {
            "schema_version": 1,
            "operation": "inspect",
            "source": exported_source,
            "output_dir": "rig_inspection",
            "options": {"component_limit": 12, "save_geometry": True},
        }
    )
    validate_export_space(inspection["result"], float(profile["target_height"]))
    missing = report["external_rig"]["unsupported_control_semantics"]
    blocking = ["unsupported_control:" + name for name in missing]
    if (
        profile["surface_policy"].get("texture_edits") == "disabled"
        and (report.get("preservation") or {}).get("appearance_preserved") is not True
    ):
        blocking.append("source_texture_appearance_or_binding_not_preserved")
    if hashlib.sha256(ctx.read.bytes("assembly")).hexdigest() != ASSEMBLY_SHA:
        raise ctx.fail("Original assembly bytes changed")
    if hashlib.sha256(ctx.read.bytes("rigged")).hexdigest() != PROVIDER_SHA:
        raise ctx.fail("Original provider bytes changed")
    evidence_zip = ctx.work_path("worker-evidence.zip")
    with zipfile.ZipFile(evidence_zip, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(bench.worker.run_root.rglob("*")):
            if path.is_file():
                archive.write(path, path.relative_to(bench.worker.run_root).as_posix())
    ctx.fact("provider_calls", 0)
    ctx.fact("blocking_findings", blocking)
    record = {
        "asset_id": "rig",
        "source_sha256": exported_source["sha256"],
        "inventory": inspection["result"],
        "part_roles": {mesh["name"]: "character" for mesh in inspection["result"]["meshes"]},
        "rig_plan": {
            "clips": report["clips"],
            "rig_author": "provider",
            "source_sha256": ASSEMBLY_SHA,
        },
        "rig_report": report,
        "metrics": {"blocking_findings": blocking},
        "required_but_missing_weights": missing,
        "open_issues": [*missing, "extra_rear_blossom", "user_art_acceptance_pending"],
        "recovery": {
            "provider_calls": 0,
            "source_assembly_sha256": ASSEMBLY_SHA,
            "retained_provider_sha256": PROVIDER_SHA,
            "measured_transform": measurement,
            "default_preservation_gates_unchanged": True,
            "status": "completed_unreviewed",
            "original_failed_run_preserved": True,
        },
    }
    return {
        "model": ctx.out.file(exported, "model/gltf-binary"),
        "record": ctx.out.json(record),
        "preservation": ctx.out.json(report["preservation"]),
        "worker_evidence": ctx.out.file(evidence_zip, "application/zip"),
    }


@node(
    "diagnostics",
    inputs={"model": "model/gltf-binary", "record": "json", "profile": "json", "bar": "json"},
    outputs={"atlas": "image[]", "evidence": "json"},
    tools=["blender"],
    version=1,
)
async def diagnostics(ctx: Ctx) -> dict[str, Any]:
    profile = ctx.read.json("profile")
    record = ctx.read.json("record")
    bench = Bench(ctx)
    source = bench.adopt(ctx.read.bytes("model"), "rig.glb")
    if source["sha256"] != record["source_sha256"]:
        raise ctx.fail("Diagnostic model differs from the finished record")
    studio = Studio(
        bench.worker,
        parts={
            "rig": {
                "source": source,
                "role": "rig",
                "inventory": record["inventory"],
                "part_roles": record["part_roles"],
            }
        },
        profile=profile,
    )
    evidence, files = await atlas_evidence(
        studio,
        profile,
        ctx.read.json("bar")["rig"],
        "rig",
        diagnostic_samples(profile, record["rig_plan"]),
    )
    ctx.fact("provider_calls", 0)
    return {
        "atlas": [ctx.out.file(path, "image/png") for path in files],
        "evidence": ctx.out.json(
            {
                "source_sha256": source["sha256"],
                "provider_calls": 0,
                "status": "rendered_unreviewed",
                "evidence": evidence,
            }
        ),
    }
