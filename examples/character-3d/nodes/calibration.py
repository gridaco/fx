"""A frozen rig as the rig reviewer's subject: what calibrates the reviewer.

A calibration subject is a finished provider-rig export frozen with the facts it was
labelled under (``rig_review_subject_v2``): its clips, its numeric findings and the weights
it misses. ``subject`` measures the export in Blender and writes the rig record the
workflow's own ``finish`` would have written, so the workflow's own rig review judges it
unchanged. The labels never reach a run; a calibration compares verdicts with them later.
"""

from __future__ import annotations

from typing import Any

from _character_harness import Bench

from grida.fx import Ctx, node

BLENDER = ["blender"]
SUBJECT_KIND = "rig_review_subject_v2"


@node(
    "subject",
    inputs={"subject": "json", "candidate": "model/gltf-binary", "profile": "json"},
    outputs={"record": "json"},
    tools=BLENDER,
    version=1,
)
async def subject(ctx: Ctx) -> dict[str, Any]:
    """The rig record of a frozen subject, held to the criteria it was labelled under."""

    frozen = ctx.read.json("subject")
    profile = ctx.read.json("profile")
    if frozen.get("kind") != SUBJECT_KIND:
        raise ctx.fail(f"a calibration subject is a {SUBJECT_KIND} document")
    if frozen["required_criteria"] != profile["review"]["rig_criteria"]:
        raise ctx.fail("the subject was labelled under other rig criteria than this profile's")
    bench = Bench(ctx)
    source = bench.adopt(ctx.read.bytes("candidate"), "candidate.glb")
    if source["sha256"] != frozen["candidate"]["sha256"]:
        raise ctx.fail("the candidate differs from the one its subject names")
    inspection, _ = await bench.worker.execute(
        {
            "schema_version": 1,
            "operation": "inspect",
            "source": source,
            "output_dir": "rig_inspection",
            "options": {"component_limit": 12, "save_geometry": True},
        }
    )
    facts = frozen["rig_facts"]
    findings = list(frozen["numeric_findings"])
    missing = list(frozen["required_but_missing_weights"])
    return {
        "record": ctx.out.json(
            {
                "asset_id": "rig",
                "source_sha256": source["sha256"],
                "inventory": inspection["result"],
                "part_roles": {
                    mesh["name"]: "character" for mesh in inspection["result"]["meshes"]
                },
                "rig_plan": {
                    "clips": facts["clips"],
                    "rig_author": "provider",
                    "source_sha256": source["sha256"],
                },
                "rig_report": facts,
                "metrics": {"blocking_findings": findings},
                "required_but_missing_weights": missing,
                "open_issues": missing,
            }
        )
    }
