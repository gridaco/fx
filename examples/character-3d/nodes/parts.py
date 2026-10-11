"""Parts: each generated mesh normalized in Blender, then judged against its references.

A mesh that cannot be normalized (missing textures, a rig or animation it should not have,
geometry that changes on re-import) is rejected without a model call: the group draws it
again from the same references.
"""

from __future__ import annotations

from typing import Any

from _character_harness import (
    PART_CRITERIA,
    REVIEW_SCHEMA,
    Bench,
    episode,
    report_verdict,
    review_material_mode,
    structural_rejection,
    verdict_check,
)
from _character_tools.studio import Studio
from _character_tools.worker_client import WorkerRefusal

from grida.fx import Ctx, node

BLENDER = ["blender"]
_SUFFIX = {"model/fbx": "fbx", "model/gltf-binary": "glb"}


@node(
    "normalize",
    inputs={"mesh": "model"},
    params={"role": str},
    outputs={"model": "model/gltf-binary?", "report": "json"},
    tools=BLENDER,
    version=1,
)
async def normalize(ctx: Ctx) -> dict[str, Any]:
    """Import the provider's mesh once and export it as a clean, textured GLB."""

    bench = Bench(ctx)
    mesh = ctx.inputs["mesh"]
    suffix = _SUFFIX.get(mesh.kind)
    if suffix is None:
        raise ctx.fail(f"a part mesh is FBX or GLB, not {mesh.kind}")
    source = bench.adopt(mesh.read_bytes(), f"raw.{suffix}")
    try:
        report, directory = await bench.worker.execute(
            {
                "schema_version": 1,
                "operation": "normalize",
                "source": source,
                "output_dir": "normalized",
                "options": {"require_textures": True},
            }
        )
    except (WorkerRefusal, ValueError) as error:
        failure = {"error_type": type(error).__name__, "message": str(error)[:2000]}
        ctx.fact("structural_failure", failure)
        return {"report": ctx.out.json({"source_sha256": source["sha256"], "failure": failure})}
    ctx.fact("structural_failure", None)
    model = bench.run_file(f"{directory}/model.glb")
    return {
        "model": ctx.out.file(model, "model/gltf-binary"),
        "report": ctx.out.json(
            {"source_sha256": source["sha256"], "inventory": report["result"]["after_reimport"]}
        ),
    }


@node(
    "review",
    inputs={
        "model": "model/gltf-binary?",
        "report": "json",
        "views": "image{}",
        "profile": "json",
        "bar": "json",
    },
    params={
        "role": str,
        "max_steps": {"type": "integer", "minimum": 1, "maximum": 40, "default": 6},
    },
    outputs={"review": "json"},
    calls={"agent.turn": "max_steps"},
    tools=BLENDER,
    resources=["prompts/review_part.md"],
    version=1,
)
async def review(ctx: Ctx) -> dict[str, Any]:
    """Judge one raw part against its reference views, from every side."""

    role = ctx.params["role"]
    report = ctx.read.json("report")
    asset_id = f"part_{role}"
    if "model" not in ctx.inputs:
        verdict = structural_rejection(
            asset_id,
            report["source_sha256"],
            PART_CRITERIA,
            region=role,
            message="Import or texture validation failed before review.",
            repair="Try the next bounded generation from approved references.",
            height=ctx.read.json("bar")["rig"]["verdict_character_height_pixels"],
        )
        report_verdict(ctx, verdict)
        return {"review": ctx.out.json(verdict)}
    profile = ctx.read.json("profile")
    bench = Bench(ctx)
    source = bench.adopt(ctx.read.bytes("model"), f"{asset_id}.glb")
    entry = {"source": source, "role": role, "inventory": report["inventory"]}
    studio = Studio(bench.worker, parts={asset_id: entry}, profile=profile)
    mode = review_material_mode(profile)
    record, renders = await studio.render(
        asset_id, views=["front", "back", "left", "right", "three_quarter"], material_mode=mode
    )
    views = ctx.inputs["views"]
    pictures = [views[key] for key in sorted(views)]
    verdict = await episode(
        ctx,
        system=ctx.prompt("prompts/review_part.md"),
        instructions={
            "role": role,
            "profile": profile,
            "asset_id": asset_id,
            "source_sha256": source["sha256"],
            "inventory": report["inventory"],
            "reference_views": sorted(views),
            "required_criteria": PART_CRITERIA,
            "quality_bar": ctx.read.json("bar")["upstream"],
            "render_material_mode": mode,
            "surface_policy": profile.get("surface_policy"),
            "initial_evidence": record,
        },
        schema=REVIEW_SCHEMA,
        check=verdict_check(asset_id, source["sha256"], PART_CRITERIA),
        review_mode="required",
        max_steps=ctx.params["max_steps"],
        tools=studio.tools(read_only=True),
        images=[*pictures, *renders],
        reviewer=True,
    )
    report_verdict(ctx, verdict)
    return {"review": ctx.out.json({**verdict, "initial_evidence": record})}
