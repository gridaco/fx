"""Bind a replay to the already independently reviewed corrected Nami model."""

from __future__ import annotations

import copy
import hashlib

from grida.fx import Ctx, node

CORRECTED_SHA = "5c0d91039150a4db1eec6828d88a277cf6599074c0d21a72a504401538f4ed45"
RECOVERED_SHA = "73cfe88743514eb1f3eb939314107d5b69d609a9ddd422058c1d98d7ec903dcc"


@node(
    "qualify",
    inputs={
        "model": "model/gltf-binary",
        "record": "json",
        "audit": "json",
        "evidence": "json",
        "review": "json",
    },
    outputs={"record": "json", "result": "json"},
    version=1,
)
def qualify(ctx: Ctx) -> dict:
    record = copy.deepcopy(ctx.read.json("record"))
    audit, evidence, review = (ctx.read.json(name) for name in ("audit", "evidence", "review"))
    if (
        hashlib.sha256(ctx.read.bytes("model")).hexdigest() != CORRECTED_SHA
        or record["source_sha256"] != CORRECTED_SHA
        or audit["source_sha256"] != RECOVERED_SHA
        or audit["output_sha256"] != CORRECTED_SHA
        or audit["changed_vertex_count"] != 514
        or not audit["animation_tracks_and_amplitudes_unchanged"]
        or evidence["source_sha256"] != CORRECTED_SHA
        or review["model_sha256"] != CORRECTED_SHA
        or review["accepted"] is not True
        or not all(item["passed"] for item in review["criteria"])
        or record["local_weight_correction"] != audit
    ):
        raise ctx.fail(
            "The replay does not match the independently reviewed exact corrected Nami rig"
        )
    historical = record.pop("rig_report")
    record["original_rig_report"] = {
        "scope": (
            "Historical provider rig finishing before the explicit local lower-body "
            "weight correction"
        ),
        "source_sha256": RECOVERED_SHA,
        "report": historical,
    }
    record["recovery"].update(
        scope="Historical recovery before the explicit local correction",
        output_sha256=RECOVERED_SHA,
    )
    record["rig_report"] = {
        "schema_version": 1,
        "author": "provider_with_explicit_local_lower_body_weight_correction",
        "output": {"sha256": CORRECTED_SHA},
        "source_sha256": RECOVERED_SHA,
        "clips": copy.deepcopy(historical["clips"]),
        "animation_tracks_and_amplitudes_unchanged": True,
        "rest_surface_preservation": copy.deepcopy(audit["rest_surface_default_audit"]),
        "semantic_review": review,
        "semantic_review_scope": (
            "Unchanged low-bar diagnostic samples; "
            "not continuous-motion or game-engine qualification"
        ),
    }
    record["open_issues"] = ["extra_rear_blossom", "game_and_runtime_qualification_pending"]
    result = {
        "character_name": "Nami",
        "status": "sampled motion checks passed",
        "source_sha256": CORRECTED_SHA,
        "runtime_qualified": False,
        "rig_author": record["rig_plan"]["rig_author"],
        "local_weight_correction_vertex_count": 514,
        "remaining_limits": review["remaining_limits"],
        "provider_calls": 0,
    }
    ctx.fact("provider_calls", 0)
    ctx.fact("sampled_review_passed", True)
    return {"record": ctx.out.json(record), "result": ctx.out.json(result)}
