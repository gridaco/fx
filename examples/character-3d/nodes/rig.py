"""Rigs: the provider's skeleton finished locally, or an agent's own rig, then one review.

Provider lane: Tripo rigs the admitted assembly (``mesh.rig``), and ``finish`` maps its
skeleton, audits that every vertex, UV and triangle of the assembly survived, restores normals
when asked, applies the material policy, appends the diagnostic clips without touching the
provider's nodes, skins or meshes, and sets the export at the profile's height on the ground.
An export the preservation audit refuses is a rejected candidate, not an error: the build is
drawn again. Agent lane: an agent places landmarks and binding regions with the rig studio's
tools. Either way the reviewer judges one labeled atlas at the bar's height, in one turn.
"""

from __future__ import annotations

from typing import Any

import jsonschema
from _character_harness import (
    APPEARANCE_LOST,
    PRODUCE_SCHEMA,
    REVIEW_SCHEMA,
    Bench,
    atlas_evidence,
    episode,
    report_verdict,
    review_view,
    structural_rejection,
    verdict_check,
)
from _character_tools.profiles import articulation, diagnostic_samples
from _character_tools.quality_bar import numeric_tools
from _character_tools.requirements import validate_export_space
from _character_tools.rig_studio import RigStudio
from _character_tools.studio import Studio
from _character_tools.worker import contract as worker_contract
from _character_tools.worker_client import WorkerRefusal

from grida.fx import Ctx, node

BLENDER = ["blender"]


def preservation_refusal(error: WorkerRefusal) -> dict[str, str] | None:
    """A worker refusal that rejects the provider's output, or None for any other error."""

    for name, message in worker_contract.PRESERVATION_REFUSALS.items():
        if error.message == message:
            return {
                "stage": "preservation_audit",
                "check": name,
                "error_type": error.error_type,
                "message": message,
            }
    return None


def _assembly(ctx: Ctx, bench: Bench, record_name: str = "assembly_record") -> dict[str, Any]:
    record = ctx.read.json(record_name)
    source = bench.adopt(ctx.read.bytes("assembly"), "assembly.glb")
    if source["sha256"] != record["source_sha256"]:
        raise ctx.fail("the assembly differs from the one its record names")
    return {
        "source": source,
        "role": "assembly",
        "inventory": record["inventory"],
        "part_roles": record["part_roles"],
    }


@node(
    "finish",
    inputs={
        "rigged": "model/gltf-binary",
        "assembly": "model/gltf-binary",
        "assembly_record": "json",
        "profile": "json",
    },
    params={"preservation": ("audit", "restore_normals")},
    outputs={"model": "model/gltf-binary?", "record": "json"},
    tools=BLENDER,
    version=1,
)
async def finish(ctx: Ctx) -> dict[str, Any]:
    """Finish the provider's rig locally: mapping, preservation, diagnostics, export space."""

    profile = ctx.read.json("profile")
    bench = Bench(ctx)
    assembly = _assembly(ctx, bench)
    rigged = bench.adopt(ctx.read.bytes("rigged"), "rigged.glb")
    try:
        report, directory = await bench.worker.execute(
            {
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
                        "source": assembly["source"],
                        "mode": ctx.params["preservation"],
                    },
                },
            },
            script="provider_rig_cli.py",
        )
    except WorkerRefusal as error:
        failure = preservation_refusal(error)
        if failure is None:
            raise
        ctx.fact("structural_failure", failure)
        return {
            "record": ctx.out.json(
                {
                    "asset_id": "rig",
                    "source_sha256": rigged["sha256"],
                    "structural_failure": failure,
                    "open_issues": [failure["message"]],
                }
            )
        }
    exported = bench.run_file(f"{directory}/{report['output']['path']}")
    exported_source = {
        "path": exported.relative_to(bench.root).as_posix(),
        "sha256": report["output"]["sha256"],
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
    try:
        validate_export_space(inspection["result"], float(profile["target_height"]))
    except ValueError as error:
        raise ctx.fail(str(error)) from error
    missing = report["external_rig"]["unsupported_control_semantics"]
    blocking = ["unsupported_control:" + name for name in missing]
    if (
        profile["surface_policy"].get("texture_edits") == "disabled"
        and (report.get("preservation") or {}).get("appearance_preserved") is not True
    ):
        blocking.append(APPEARANCE_LOST)
    ctx.fact("structural_failure", None)
    ctx.fact("blocking_findings", blocking)
    return {
        "model": ctx.out.file(exported, "model/gltf-binary"),
        "record": ctx.out.json(
            {
                "asset_id": "rig",
                "source_sha256": exported_source["sha256"],
                "inventory": inspection["result"],
                "part_roles": {
                    mesh["name"]: "character" for mesh in inspection["result"]["meshes"]
                },
                "rig_plan": {
                    "clips": report["clips"],
                    "rig_author": "provider",
                    "source_sha256": assembly["source"]["sha256"],
                },
                "rig_report": report,
                "metrics": {"blocking_findings": blocking},
                "required_but_missing_weights": missing,
                "open_issues": missing,
            }
        ),
    }


@node(
    "agent",
    inputs={"assembly": "model/gltf-binary", "assembly_record": "json", "profile": "json"},
    params={
        "review": ("required", "none"),
        "feedback": {"type": ["object", "null"], "default": None},
        "max_steps": {"type": "integer", "minimum": 1, "maximum": 40, "default": 16},
        "revisions": {"type": "integer", "minimum": 1, "maximum": 32, "default": 6},
        "recent_images": {"type": "integer", "minimum": 1, "maximum": 64, "default": 24},
    },
    outputs={"model": "model/gltf-binary", "record": "json"},
    calls={"agent.turn": "max_steps"},
    tools=BLENDER,
    resources=["prompts/rig.md"],
    version=1,
)
async def agent(ctx: Ctx) -> dict[str, Any]:
    """Place the profile's joints and binding regions, build rigs, and submit one."""

    profile = ctx.read.json("profile")
    bench = Bench(ctx)
    assembly = _assembly(ctx, bench)
    studio = RigStudio(
        bench.worker,
        parts={"assembly": assembly},
        profile=profile,
        max_rig_revisions=ctx.params["revisions"],
    )
    studio.admitted_assembly = "assembly"
    record, images = await studio.render("assembly", views=["front", "right", "back"])

    def check(value: Any) -> None:
        jsonschema.validate(value, PRODUCE_SCHEMA)
        asset = studio.assets.get(value["asset_id"])
        if value["asset_id"] not in studio.rig_revisions or not (asset or {}).get("rig_ready"):
            raise ValueError("Submit a real successful rig revision")
        studio.asset(value["asset_id"])
        studio.rig_frozen = True

    chosen = await episode(
        ctx,
        system=ctx.prompt("prompts/rig.md"),
        instructions={
            "stage": "rigging",
            "review_mode": ctx.params["review"],
            "profile": profile,
            "assembly_asset_id": "assembly",
            "assembly_inventory": assembly["inventory"],
            "required_joints_and_parents": articulation(profile).PARENTS,
            "previous_review": review_view(ctx.params["feedback"]),
            "remaining_rig_revisions": studio.max_rig_revisions,
            "initial_views": record,
        },
        schema=PRODUCE_SCHEMA,
        check=check,
        review_mode=ctx.params["review"],
        max_steps=ctx.params["max_steps"],
        tools=studio.rig_tools(),
        images=images,
        recent_images=ctx.params["recent_images"],
    )
    asset = studio.asset(chosen["asset_id"])
    ctx.fact("blocking_findings", asset["metrics"]["blocking_findings"])
    return {
        "model": ctx.out.bytes(bench.read(asset["source"]), "model/gltf-binary"),
        "record": ctx.out.json(
            {
                "asset_id": chosen["asset_id"],
                "source_sha256": asset["source"]["sha256"],
                "inventory": asset["inventory"],
                "part_roles": asset["part_roles"],
                "rig_plan": asset["rig_plan"],
                "rig_report": asset["rig_report"],
                "metrics": asset["metrics"],
                "required_but_missing_weights": asset["required_but_missing_weights"],
                "open_issues": chosen["open_issues"],
            }
        ),
    }


@node(
    "review",
    inputs={"model": "model/gltf-binary?", "record": "json", "profile": "json", "bar": "json"},
    outputs={"review": "json", "atlas": "image/png[]"},
    calls={"agent.turn": 2},
    tools=BLENDER,
    resources=["prompts/review_rig.md"],
    version=3,
)
async def review(ctx: Ctx) -> dict[str, Any]:
    """Judge the rig in one turn from a labeled atlas of every diagnostic pose."""

    profile = ctx.read.json("profile")
    record = ctx.read.json("record")
    bar = ctx.read.json("bar")["rig"]
    criteria = profile["review"]["rig_criteria"]
    if record.get("structural_failure") or "model" not in ctx.inputs:
        failure = record.get("structural_failure") or {"message": "No export was made."}
        verdict = structural_rejection(
            record["asset_id"],
            record["source_sha256"],
            criteria,
            region="mesh",
            message=failure["message"],
            repair="Regenerate the mesh from the admitted references and rig it again.",
            height=bar["verdict_character_height_pixels"],
        )
        report_verdict(ctx, verdict)
        return {"review": ctx.out.json({**verdict, "quality_bar": bar}), "atlas": []}
    bench = Bench(ctx)
    source = bench.adopt(ctx.read.bytes("model"), "rig.glb")
    if source["sha256"] != record["source_sha256"]:
        raise ctx.fail("the rig differs from the one its record names")
    entry = {
        "source": source,
        "role": "rig",
        "inventory": record["inventory"],
        "part_roles": record["part_roles"],
    }
    studio = Studio(bench.worker, parts={"rig": entry}, profile=profile)
    evidence, files = await atlas_evidence(
        studio, profile, bar, "rig", diagnostic_samples(profile, record["rig_plan"])
    )
    failing = record["required_but_missing_weights"] or record["metrics"]["blocking_findings"]

    def numbers_hold(value: dict[str, Any]) -> None:
        if value["accepted"] and failing:
            raise ValueError(
                "Required rig metrics fail; acceptance must be false with actionable issues"
            )

    verdict = await episode(
        ctx,
        system=ctx.prompt("prompts/review_rig.md"),
        instructions={
            "stage": "rigging",
            "profile": profile,
            "asset_id": "rig",
            "source_sha256": source["sha256"],
            "required_criteria": criteria,
            "quality_bar": bar,
            "required_but_missing_weights": record["required_but_missing_weights"],
            "exported_rig_report": record["rig_report"],
            "numeric_findings": record["metrics"]["blocking_findings"],
            "initial_evidence": evidence,
        },
        schema=REVIEW_SCHEMA,
        check=verdict_check("rig", source["sha256"], criteria, bar=bar, extra=numbers_hold),
        review_mode="required",
        max_steps=2,
        tools=numeric_tools(studio.tools(read_only=True)),
        images=list(files),
        reviewer=True,
    )
    report_verdict(ctx, verdict)
    return {
        "review": ctx.out.json({**verdict, "initial_evidence": evidence, "quality_bar": bar}),
        "atlas": [ctx.out.file(path, "image/png") for path in files],
    }
