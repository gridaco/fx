"""Assembly: an agent stands the parts up as one character; a reviewer judges the result.

The producer has the studio's tools: it measures and renders the parts, and builds candidate
assemblies, each from all the original parts with one transform per part. It submits the
revision it stands behind. The reviewer sees the rest pose at inspection height and at every
gameplay height, from every required view, in the export's finish.
"""

from __future__ import annotations

import json
from typing import Any

import jsonschema
from _character_harness import (
    PRODUCE_SCHEMA,
    REVIEW_SCHEMA,
    Bench,
    episode,
    report_verdict,
    review_evidence,
    review_view,
    verdict_check,
)
from _character_tools.studio import Studio

from grida.fx import Ctx, node

BLENDER = ["blender"]


def _parts(ctx: Ctx, bench: Bench) -> dict[str, dict[str, Any]]:
    """Every part on the bench under its role, with the inventory its normalize measured."""

    models, reports = ctx.inputs.get("parts", {}), ctx.inputs["reports"]
    parts = {}
    for role in sorted(reports):
        report = json.loads(reports[role].read_bytes())
        if role not in models:
            failure = report.get("failure", {}).get("message", "it was not normalized")
            raise ctx.fail(f"the {role} part could not be normalized: {failure}")
        inventory = report["inventory"]
        source = bench.adopt(models[role].read_bytes(), f"parts/{role}.glb")
        parts[role] = {"source": source, "role": role, "inventory": inventory}
    return parts


@node(
    "assemble",
    inputs={"parts": "model{}?", "reports": "json{}", "canonical": "image?", "profile": "json"},
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
    resources=["prompts/assemble.md"],
    version=1,
)
async def assemble(ctx: Ctx) -> dict[str, Any]:
    """Build candidate assemblies until one stands, and submit it."""

    profile = ctx.read.json("profile")
    bench = Bench(ctx)
    parts = _parts(ctx, bench)
    studio = Studio(
        bench.worker, parts=parts, profile=profile, max_revisions=ctx.params["revisions"]
    )
    images = [ctx.inputs["canonical"]] if "canonical" in ctx.inputs else []

    def check(value: Any) -> None:
        jsonschema.validate(value, PRODUCE_SCHEMA)
        if value["asset_id"] not in studio.revisions:
            raise ValueError("Submit a real built assembly revision")
        studio.asset(value["asset_id"])
        studio.frozen = True

    chosen = await episode(
        ctx,
        system=ctx.prompt("prompts/assemble.md"),
        instructions={
            "stage": "assembly",
            "review_mode": ctx.params["review"],
            "profile": profile,
            "available_parts": parts,
            "previous_review": review_view(ctx.params["feedback"]),
            "remaining_revisions": studio.max_revisions,
        },
        schema=PRODUCE_SCHEMA,
        check=check,
        review_mode=ctx.params["review"],
        max_steps=ctx.params["max_steps"],
        tools=studio.tools(),
        images=images,
        recent_images=ctx.params["recent_images"],
    )
    asset = studio.asset(chosen["asset_id"])
    ctx.fact("revisions", len(studio.revisions))
    return {
        "model": ctx.out.bytes(bench.read(asset["source"]), "model/gltf-binary"),
        "record": ctx.out.json(
            {
                "asset_id": chosen["asset_id"],
                "source_sha256": asset["source"]["sha256"],
                "inventory": asset["inventory"],
                "part_roles": asset["part_roles"],
                "assembly_parameters": asset["assembly_parameters"],
                "rationale": chosen["rationale"],
                "open_issues": chosen["open_issues"],
            }
        ),
    }


@node(
    "review",
    inputs={
        "model": "model/gltf-binary",
        "record": "json",
        "canonical": "image?",
        "profile": "json",
        "bar": "json",
    },
    params={"max_steps": {"type": "integer", "minimum": 1, "maximum": 40, "default": 6}},
    outputs={"review": "json"},
    calls={"agent.turn": "max_steps"},
    tools=BLENDER,
    resources=["prompts/review_assembly.md"],
    version=1,
)
async def review(ctx: Ctx) -> dict[str, Any]:
    """Judge the assembly against the profile's criteria at the bar's heights."""

    profile = ctx.read.json("profile")
    record = ctx.read.json("record")
    bar = ctx.read.json("bar")["rig"]
    criteria = profile["review"]["assembly_criteria"]
    bench = Bench(ctx)
    asset_id = record["asset_id"]
    source = bench.adopt(ctx.read.bytes("model"), f"{asset_id}.glb")
    if source["sha256"] != record["source_sha256"]:
        raise ctx.fail("the assembly differs from the one its record names")
    entry = {
        "source": source,
        "role": "assembly",
        "inventory": record["inventory"],
        "part_roles": record["part_roles"],
    }
    studio = Studio(bench.worker, parts={asset_id: entry}, profile=profile)
    evidence, renders = await review_evidence(studio, profile, asset_id)
    pictures = [ctx.inputs["canonical"]] if "canonical" in ctx.inputs else []
    pictures += renders
    verdict = await episode(
        ctx,
        system=ctx.prompt("prompts/review_assembly.md"),
        instructions={
            "stage": "assembly",
            "profile": profile,
            "asset_id": asset_id,
            "source_sha256": source["sha256"],
            "required_criteria": criteria,
            "quality_bar": bar,
            "final_render_evidence": evidence,
        },
        schema=REVIEW_SCHEMA,
        check=verdict_check(asset_id, source["sha256"], criteria, bar=bar),
        review_mode="required",
        max_steps=ctx.params["max_steps"],
        tools=studio.tools(read_only=True),
        images=pictures,
        reviewer=True,
    )
    report_verdict(ctx, verdict)
    return {"review": ctx.out.json({**verdict, "initial_evidence": evidence, "quality_bar": bar})}
