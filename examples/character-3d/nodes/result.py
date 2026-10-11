"""The result: what is delivered, and what it may claim.

With reviews, a rig is accepted only when its review accepted it and its numbers hold: no
missing required weights, no blocking findings. Without, a rig is delivered unreviewed only
when its export is intact and its appearance survived; its findings travel with it. A run
that stops after assembly delivers the assembly the same way.
"""

from __future__ import annotations

from typing import Any

from _character_harness import APPEARANCE_LOST, INTEGRITY_FINDINGS

from grida.fx import Ctx, node


def _codes(findings: list[Any]) -> list[str]:
    return [f["code"] if isinstance(f, dict) else str(f) for f in findings]


@node(
    "result",
    inputs={
        "profile": "json",
        "rig": "model/gltf-binary?",
        "rig_record": "json?",
        "rig_review": "json?",
        "assembly": "model/gltf-binary?",
        "assembly_record": "json?",
        "assembly_review": "json?",
    },
    params={"review_mode": ("required", "none"), "through": ("rig", "assembly")},
    outputs={"character": "model/gltf-binary", "result": "json"},
    version=1,
)
def result(ctx: Ctx) -> dict[str, Any]:
    """Admit the character, or refuse it with the reason."""

    profile = ctx.read.json("profile")
    rig = ctx.params["through"] == "rig"
    stage = "rig" if rig else "assembly"
    if f"{stage}_record" not in ctx.inputs:
        raise ctx.fail(f"the run made no {stage}")
    record = ctx.read.json(f"{stage}_record")
    review = ctx.read.json(f"{stage}_review") if f"{stage}_review" in ctx.inputs else None
    findings = record.get("metrics", {}).get("blocking_findings", []) if rig else []
    missing = record.get("required_but_missing_weights", []) if rig else []
    if record.get("structural_failure") or stage not in ctx.inputs:
        raise ctx.fail("the rig failed the geometry preservation audit on every build")
    if ctx.params["review_mode"] == "required":
        if review is None or not review["accepted"]:
            raise ctx.fail("the character failed its independent review after bounded rebuilds")
        if missing or findings:
            raise ctx.fail("the rig's required checks failed after bounded rebuilds")
        status, review_status = "accepted", "accepted"
    else:
        if APPEARANCE_LOST in _codes(findings):
            raise ctx.fail("the provider rig lost the source's appearance")
        integrity = sorted(set(_codes(findings)) & INTEGRITY_FINDINGS)
        if integrity:
            raise ctx.fail("the rig export failed its integrity checks: " + ", ".join(integrity))
        status, review_status = "completed_unreviewed", "skipped"
    document = {
        "schema_version": 1,
        "status": status,
        "review_status": review_status,
        "delivered": ctx.params["through"],
        "profile_id": profile["profile_id"],
        "partition": (profile.get("partition") or {}).get("preset"),
        "source_sha256": record["source_sha256"],
        "quality_findings": {
            "required_but_missing_weights": missing,
            "numeric_findings": findings,
            "producer_open_issues": record.get("open_issues", []),
        },
        "qualification_eligible": review_status == "accepted",
    }
    ctx.fact("status", status)
    return {
        "character": ctx.out.bytes(ctx.read.bytes(stage), "model/gltf-binary"),
        "result": ctx.out.json(document),
    }
