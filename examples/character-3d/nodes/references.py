"""References: an agent draws the character and every part's views; a reviewer judges them.

The producer has the reference studio's tools: it draws a canonical picture, then part atlases
or views from it (each a paid ``image.generate`` or, with references, ``image.edit`` call),
crops atlases into labelled views, and submits a bundle: the canonical and a front and back of
every part. The reviewer sees the canonical and every selected view.
"""

from __future__ import annotations

import hashlib
from typing import Any

import jsonschema
from _character_harness import (
    REFERENCE_CRITERIA,
    REFERENCE_SUBMIT,
    REVIEW_SCHEMA,
    episode,
    report_verdict,
    review_view,
    verdict_check,
)
from _character_tools.io import confined, digest
from _character_tools.reference_studio import ReferenceStudio

from grida.fx import Ctx, node

_RIGHTS = "Original character drawn for this project from its own brief."


def _view_key(role: str, view: str) -> str:
    return f"{role}-{view}"


@node(
    "draw",
    inputs={"brief": "text", "reference": "image?", "profile": "json"},
    params={
        "review": ("required", "none"),
        "feedback": {"type": ["object", "null"], "default": None},
        "max_steps": {"type": "integer", "minimum": 1, "maximum": 40, "default": 16},
        "generations": {"type": "integer", "minimum": 1, "maximum": 16, "default": 6},
        "crops": {"type": "integer", "minimum": 1, "maximum": 128, "default": 32},
        "recent_images": {"type": "integer", "minimum": 1, "maximum": 64, "default": 24},
    },
    outputs={"canonical": "image/png", "views": "image/png{}", "bundle": "json"},
    calls={"agent.turn": "max_steps", "image.generate": "generations", "image.edit": "generations"},
    resources=["prompts/plan_references.md"],
    version=1,
)
async def draw(ctx: Ctx) -> dict[str, Any]:
    """Draw the canonical picture and each part's views, and submit them as one bundle."""

    profile = ctx.read.json("profile")
    root = ctx.work_path("references")
    (root / "run").mkdir(parents=True, exist_ok=True)

    async def generate(plan: dict[str, Any]) -> dict[str, Any]:
        sources = [confined(root, item["path"]).read_bytes() for item in plan["inputs"]]
        pictures = [ctx.out.bytes(data, "image/png") for data in sources]
        size = plan["params"]["size"]
        if pictures:
            made = await ctx.image_edit(
                image=pictures[0],
                references=pictures[1:],
                prompt=plan["prompt"],
                size=size,
                background="opaque",
            )
        else:
            made = await ctx.image_generate(prompt=plan["prompt"], size=size, background="opaque")
        target = root / "run" / "references" / "generated" / f"{plan['operation_id']}.png"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(made.image.read_bytes())
        return {
            "source": {"path": target.relative_to(root).as_posix(), "sha256": digest(target)},
            "provenance": None,
        }

    studio = ReferenceStudio(
        root,
        root / "run",
        profile=profile,
        rights_basis=_RIGHTS,
        generate_image=generate,
        max_revisions=ctx.params["generations"],
        max_crops=ctx.params["crops"],
    )
    images: list[Any] = []
    supplied = None
    if "reference" in ctx.inputs:
        data = ctx.read.bytes("reference")
        source = {
            "path": "supplied/reference.png",
            "sha256": hashlib.sha256(data).hexdigest(),
        }
        (root / "supplied").mkdir(exist_ok=True)
        (root / "supplied" / "reference.png").write_bytes(data)
        supplied = studio.adopt_reference(
            source, purpose="canonical", roles=[], rights_basis=_RIGHTS
        )
        images.append(ctx.inputs["reference"])
    frozen: dict[str, Any] = {}

    def check(value: Any) -> None:
        jsonschema.validate(value, REFERENCE_SUBMIT)
        frozen.update(
            studio.freeze_bundle(
                value["canonical_asset_id"], value["selections"], REFERENCE_CRITERIA
            )
        )

    await episode(
        ctx,
        system=ctx.prompt("prompts/plan_references.md"),
        instructions={
            "brief": ctx.read.text("brief"),
            "review_mode": ctx.params["review"],
            "profile": profile,
            "previous_review": review_view(ctx.params["feedback"]),
            "required_criteria": REFERENCE_CRITERIA,
            "input_reference": supplied,
            "registered_references": studio.snapshot()["assets"],
        },
        schema=REFERENCE_SUBMIT,
        check=check,
        review_mode=ctx.params["review"],
        max_steps=ctx.params["max_steps"],
        tools=studio.tools(),
        images=images,
        recent_images=ctx.params["recent_images"],
    )
    canonical = confined(root, frozen["canonical"]["source"]["path"]).read_bytes()
    views = {
        _view_key(role, item["view"]): ctx.out.bytes(
            confined(root, item["source"]["path"]).read_bytes(), "image/png"
        )
        for role, part in frozen["parts"].items()
        for item in part["views"]
    }
    ctx.fact("views", sorted(views))
    return {
        "canonical": ctx.out.bytes(canonical, "image/png"),
        "views": views,
        "bundle": ctx.out.json({k: v for k, v in frozen.items() if k != "manifest"}),
    }


@node(
    "review",
    inputs={
        "brief": "text",
        "canonical": "image",
        "views": "image{}",
        "bundle": "json",
        "profile": "json",
        "bar": "json",
    },
    params={"max_steps": {"type": "integer", "minimum": 1, "maximum": 40, "default": 6}},
    outputs={"review": "json"},
    calls={"agent.turn": "max_steps"},
    resources=["prompts/review_references.md"],
    version=1,
)
async def review(ctx: Ctx) -> dict[str, Any]:
    """Judge the bundle against its criteria, shown the canonical and every view."""

    bundle = ctx.read.json("bundle")
    views = ctx.inputs["views"]
    order = sorted(views)
    pictures = [ctx.inputs["canonical"], *(views[key] for key in order)]
    verdict = await episode(
        ctx,
        system=ctx.prompt("prompts/review_references.md"),
        instructions={
            "brief": ctx.read.text("brief"),
            "profile": ctx.read.json("profile"),
            "bundle": bundle,
            "required_criteria": REFERENCE_CRITERIA,
            "quality_bar": ctx.read.json("bar")["upstream"],
            "picture_order": ["canonical", *order],
        },
        schema=REVIEW_SCHEMA,
        check=verdict_check(bundle["bundle_id"], bundle["bundle_sha256"], REFERENCE_CRITERIA),
        review_mode="required",
        max_steps=ctx.params["max_steps"],
        images=pictures,
        reviewer=True,
    )
    report_verdict(ctx, verdict)
    return {"review": ctx.out.json(verdict)}


#: The sides a mesh is generated from (``mesh.generate@1``); other views are for reviewers.
SIDES = ("front", "back", "left", "right")


@node(
    "role_views",
    inputs={"views": "image{}"},
    params={"role": str},
    outputs={"views": "image{}", "sides": "image{}"},
    version=2,
)
def role_views(ctx: Ctx) -> dict[str, Any]:
    """One part's views by view name, and the sides its mesh is generated from."""

    prefix = f"{ctx.params['role']}-"
    picked = {
        key.removeprefix(prefix): file
        for key, file in ctx.inputs["views"].items()
        if key.startswith(prefix)
    }
    if not {"front", "back"} <= set(picked):
        raise ctx.fail(f"the {ctx.params['role']} part has no front and back view")
    return {
        "views": picked,
        "sides": {view: file for view, file in picked.items() if view in SIDES},
    }
