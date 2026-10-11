"""The 47-mask terrain atlas: the locked template's cells, painted in one material.

The template's 48 cells are packed edge to edge at the provider canvas and painted over (a
paid edit); a judge refuses a sheet that cannot be sliced on its fixed cell boundaries and
draws it again; the assembly slices it, applies the 47-mask lookup and harmonizes only the
legal connector edges. The template and the lookup are package resources, so each step holds
them to the digest the plan was keyed on.
"""

from __future__ import annotations

import hashlib
from collections.abc import Sequence
from typing import Any

from grida.fx import Ctx, Group, StepRef, node
from terrain_example.atlas import (
    PAINT_CANVAS_SIZE,
    assemble_terrain_atlas,
    require_terrain_atlas_source,
    terrain_atlas_paint_target,
)
from terrain_example.resources import terrain_atlas_lookup_path, terrain_atlas_template_path

#: Paintings an atlas sheet gets before the run stops on it.
ATLAS_TAKES = 6


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def template_sha256() -> str:
    """The locked template's digest, as a plan keys its steps on it."""

    return _sha256(terrain_atlas_template_path().read_bytes())


def lookup_sha256() -> str:
    """The 47-mask lookup's digest, as a plan keys the assembly on it."""

    return _sha256(terrain_atlas_lookup_path().read_bytes())


def _template(ctx: Ctx) -> bytes:
    template = terrain_atlas_template_path().read_bytes()
    if _sha256(template) != ctx.params["template_sha256"]:
        raise ctx.fail("the terrain atlas template changed since this plan was made")
    return template


@node(
    "atlas_paint_target",
    params={"template_sha256": str},
    outputs={"image": "image/png"},
    version=1,
)
def atlas_paint_target(ctx: Ctx) -> dict[str, Any]:
    """The locked template's 48 cells, packed edge to edge at the provider canvas."""

    return {"image": ctx.out.bytes(terrain_atlas_paint_target(_template(ctx)), "image/png")}


@node(
    "admit_atlas",
    inputs={"image": "image"},
    params={"template_sha256": str},
    judge=True,
    version=1,
)
def admit_atlas(ctx: Ctx) -> dict[str, Any]:
    """The painted sheet can be sliced on its fixed cell boundaries, or it is redrawn."""

    try:
        facts = require_terrain_atlas_source(ctx.read.bytes("image"), template=_template(ctx))
    except ValueError as error:
        ctx.fact("errors", [str(error)])
        ctx.fact("verdict", "reject")
        return {}
    ctx.fact("admission", facts)
    ctx.fact("verdict", "accept")
    return {}


@node(
    "assemble_atlas",
    inputs={"raw": "image"},
    params={"template_sha256": str, "lookup_sha256": str},
    outputs={"image": "image/png", "validation": "json"},
    version=1,
)
def assemble_atlas(ctx: Ctx) -> dict[str, Any]:
    """Slice the painted sheet, apply the 47-mask lookup and harmonize the legal connectors."""

    template = _template(ctx)
    lookup = terrain_atlas_lookup_path().read_bytes()
    if _sha256(lookup) != ctx.params["lookup_sha256"]:
        raise ctx.fail("the terrain atlas lookup changed since this plan was made")
    canonical, validation = assemble_terrain_atlas(
        ctx.read.bytes("raw"), template=template, lookup_data=lookup
    )
    if validation["classification"] != "direct_pass":
        raise ctx.fail("the painted atlas needs repair, and only a direct pass is published")
    return {
        "image": ctx.out.bytes(canonical, "image/png"),
        "validation": ctx.out.json(validation),
    }


def add_atlas_steps(
    group: Group, *, nodes: str, prompt: str, materials: Sequence[Any]
) -> dict[str, StepRef]:
    """Write the paint target, the painting, its judge and the assembly into ``group``.

    ``materials`` are the authored material references, shown after the paint target.
    """

    template = template_sha256()
    target = group.step(
        "paint_target",
        title="Pack the locked template",
        uses=f"{nodes}#atlas_paint_target",
        with_={"template_sha256": template},
    )
    painting = group.step(
        "generate",
        title="Paint the 47-mask atlas",
        uses="fx/image.edit@1",
        with_={
            "image": target.outputs.image,
            "references": list(materials),
            "prompt": prompt,
            "size": PAINT_CANVAS_SIZE,
            "background": "opaque",
        },
        view=True,
    )
    group.step(
        "admit",
        title="Hold the sheet to its cells",
        uses=f"{nodes}#admit_atlas",
        judges="generate",
        with_={"image": painting.outputs.image, "template_sha256": template},
        on_reject={"regenerate": {"max": ATLAS_TAKES, "then": "fail"}},
    )
    assembled = group.step(
        "publish",
        title="Assemble the atlas",
        uses=f"{nodes}#assemble_atlas",
        with_={
            "raw": painting.outputs.image,
            "template_sha256": template,
            "lookup_sha256": lookup_sha256(),
        },
        view=True,
    )
    return {"generate": painting, "publish": assembled}


__all__ = [
    "ATLAS_TAKES",
    "add_atlas_steps",
    "admit_atlas",
    "assemble_atlas",
    "atlas_paint_target",
    "lookup_sha256",
    "template_sha256",
]
