"""Explicit exact-asset Nami lower-body correction and selection evidence."""

from __future__ import annotations

import copy
from typing import Any

from _character_harness import Bench
from _character_tools.nami_weights import apply
from _character_tools.requirements import validate_export_space

from grida.fx import Ctx, node


@node(
    "correct",
    inputs={
        "model": "model/gltf-binary",
        "record": "json",
        "parameters": "json",
        "profile": "json",
    },
    outputs={
        "model": "model/gltf-binary",
        "record": "json",
        "audit": "json",
        "selection_mask": "model/gltf-binary",
    },
    tools=["blender"],
    version=1,
)
async def correct(ctx: Ctx) -> dict[str, Any]:
    bench = Bench(ctx)
    adopted = bench.adopt(ctx.read.bytes("model"), "recovered.glb")
    source = bench.root / adopted["path"]
    audit = apply(source, ctx.work_path("correction"), ctx.read.json("parameters"))
    model = ctx.work_path("correction/model.glb")
    current = bench.adopt(model.read_bytes(), "corrected.glb")
    inspection, _ = await bench.worker.execute(
        {
            "schema_version": 1,
            "operation": "inspect",
            "source": current,
            "output_dir": "corrected_inspection",
            "options": {"component_limit": 12, "save_geometry": True},
        }
    )
    profile = ctx.read.json("profile")
    validate_export_space(inspection["result"], float(profile["target_height"]))
    record = copy.deepcopy(ctx.read.json("record"))
    if record["source_sha256"] != adopted["sha256"]:
        raise ctx.fail("Original record and frozen recovered model differ")
    record.update(
        source_sha256=audit["output_sha256"],
        inventory=inspection["result"],
        local_weight_correction=audit,
    )
    record["rig_plan"]["rig_author"] = "provider_with_explicit_local_lower_body_weight_correction"
    record["open_issues"] = [
        "extra_rear_blossom",
        "local_weight_correction_semantic_review_pending",
        "user_art_acceptance_pending",
    ]
    ctx.fact("provider_calls", 0)
    ctx.fact("changed_vertex_count", audit["changed_vertex_count"])
    ctx.fact("preservation", audit["rest_surface_default_audit"]["status"])
    return {
        "model": ctx.out.file(model, "model/gltf-binary"),
        "record": ctx.out.json(record),
        "audit": ctx.out.json(audit),
        "selection_mask": ctx.out.file(
            ctx.work_path("correction/selection-mask.glb"), "model/gltf-binary"
        ),
    }


@node(
    "selection",
    inputs={"model": "model/gltf-binary"},
    outputs={"images": "image[]", "evidence": "json"},
    tools=["blender"],
    version=1,
)
async def selection(ctx: Ctx) -> dict[str, Any]:
    bench = Bench(ctx)
    source = bench.adopt(ctx.read.bytes("model"), "selection.glb")
    report, directory = await bench.worker.execute(
        {
            "schema_version": 1,
            "operation": "render",
            "source": source,
            "output_dir": "selection_render",
            "options": {
                "views": ["positive_z", "negative_z", "positive_x", "negative_x"],
                "resolution": [768, 768],
                "samples": 12,
                "material_mode": "native",
                "character_height_pixels": 600,
            },
        }
    )
    return {
        "images": [
            ctx.out.file(bench.run_file(directory + "/" + image["image"]), "image/png")
            for image in report["result"]["images"]
        ],
        "evidence": ctx.out.json(
            {
                "scope": (
                    "Static diagnostic selection visualization; red is the corrected "
                    "lower-body subset, gray is unchanged. Not delivered artwork."
                ),
                "source_sha256": source["sha256"],
                "provider_calls": 0,
                "report": report,
            }
        ),
    }
