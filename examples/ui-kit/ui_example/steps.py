"""Interface sheets: nine-slice panels and buttons, icon grids and cursor sets.

Each role's layout template is rendered from its geometry record, painted over in the
game's style (a paid edit), held to its family's pixel gate by a judge that draws it again
when it fails, canonicalized (exterior cleared, content clamped) with evidence a reviewer
reads, and reviewed in one structured answer. The review is evidence, never a gate.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from typing import Any, cast

from grida.fx import Ctx, Group, StepRef, node
from ui_example.models import UiArtwork
from ui_example.nodes import (
    NINE_SLICE_FAMILY,
    UI_ATLAS_VALIDATION_VERSION,
    UI_SHEET_ROLES,
    UiSheetRole,
    sheet_family,
    ui_atlas_review_schema,
)

#: Paintings a sheet gets before the run stops on it.
SHEET_TAKES = 6
REVIEW_SYSTEM = (
    "You are a strict independent 2D game-art technical director. Return only the "
    "requested structured review."
)
REVIEW_MAX_TOKENS = 1800
_ROLE = {"role": str}


def _role(ctx: Ctx) -> UiSheetRole:
    role = UI_SHEET_ROLES.get(ctx.params["role"])
    if role is None:
        raise ctx.fail(f"no interface sheet role {ctx.params['role']}")
    return role


@node("ui_template", params=_ROLE, outputs={"image": "image/png"}, version=1)
def ui_template(ctx: Ctx) -> dict[str, Any]:
    """The role's layout template, rendered from its geometry record."""

    role = _role(ctx)
    return {"image": ctx.out.bytes(sheet_family(role).template(role), "image/png")}


@node("admit_ui_sheet", inputs={"image": "image"}, params=_ROLE, judge=True, version=1)
def admit_ui_sheet(ctx: Ctx) -> dict[str, Any]:
    """The sheet passes its family's pixel gate, or it is drawn again."""

    role = _role(ctx)
    try:
        facts = sheet_family(role).validate(ctx.read.bytes("image"), role)
    except ValueError as error:
        ctx.fact("errors", [str(error)])
        ctx.fact("verdict", "reject")
        return {}
    ctx.fact("admission", facts)
    ctx.fact("verdict", "accept")
    return {}


@node(
    "publish_ui_sheet",
    inputs={"raw": "image"},
    params=_ROLE,
    outputs={"image": "image/png", "validation": "json", "evidence": "image/png"},
    version=1,
)
def publish_ui_sheet(ctx: Ctx) -> dict[str, Any]:
    """Normalize only the admitted alpha boundary, record the geometry, draw the evidence."""

    role = _role(ctx)
    family = sheet_family(role)
    canonical, facts = family.canonicalize(ctx.read.bytes("raw"), role)
    canonical_facts = cast(dict[str, object], facts["canonical"])
    record = {
        "schema_version": 1,
        "kind": UI_ATLAS_VALIDATION_VERSION,
        **family.contract(canonical_facts),
        "facts": facts,
    }
    return {
        "image": ctx.out.bytes(canonical, "image/png"),
        "validation": ctx.out.json(record),
        "evidence": ctx.out.bytes(family.evidence(canonical, canonical_facts), "image/png"),
    }


@node("ui_review_schema", params=_ROLE, outputs={"schema": "json"}, version=1)
def ui_review_schema(ctx: Ctx) -> dict[str, Any]:
    """The review's answer shape: the questions the role's pixel gate cannot decide."""

    return {"schema": ctx.out.json(ui_atlas_review_schema(sheet_family(_role(ctx)).review_checks))}


def add_ui_sheet_steps(
    group: Group,
    *,
    nodes: str,
    ui: UiArtwork,
    role: UiSheetRole,
    style_prompt: Callable[[str], str],
    reference_path: Callable[[str], str],
    review: bool = True,
) -> dict[str, StepRef]:
    """Write one role's template, painting, judge, publish and (unless left out) review."""

    family = sheet_family(role)
    direction = getattr(ui, role.role, None)
    if not isinstance(direction, family.direction_type):
        raise ValueError(f"the UI document names no {role.role} direction of its family")
    sources: Mapping[str, str] = {entry.reference_id: entry.source for entry in ui.references}
    authored = [reference_path(sources[reference_id]) for reference_id in direction.reference_ids]
    template = group.step(
        "template",
        title="Render the layout template",
        uses=f"{nodes}#ui_template",
        with_={"role": role.role},
    )
    # The authored references come first and the template last, as the painter is told.
    pictures: list[Any] = [*authored, template.outputs.image]
    painting = group.step(
        "generate",
        title=f"Paint the {role.role} sheet",
        uses="fx/image.edit@1",
        with_={
            "image": pictures[0],
            "references": pictures[1:],
            "prompt": style_prompt(family.content_task(role, direction.prompt)),
            "size": f"{role.canvas[0]}x{role.canvas[1]}",
            "background": "transparent",
        },
        requires=["alpha"],
        view=True,
    )
    group.step(
        "admit",
        title="Hold the sheet to its gate",
        uses=f"{nodes}#admit_ui_sheet",
        judges="generate",
        with_={"image": painting.outputs.image, "role": role.role},
        on_reject={"regenerate": {"max": SHEET_TAKES, "then": "fail"}},
    )
    published = group.step(
        "publish",
        title="Normalize the sheet",
        uses=f"{nodes}#publish_ui_sheet",
        with_={"raw": painting.outputs.image, "role": role.role},
        view=True,
    )
    steps = {"generate": painting, "publish": published}
    if review:
        steps["review"] = _review(
            group,
            nodes=nodes,
            role=role,
            prompt=family.review_prompt(
                role,
                direction.prompt,
                # A nine-slice review names the band fill the gate admitted, which only the
                # validation record knows.
                {"band_fill": str(published.outputs.validation.band_fill)}
                if family is NINE_SLICE_FAMILY
                else {},
            ),
            pictures=[
                published.outputs.evidence,
                *(
                    reference_path(entry.source)
                    for entry in ui.references
                    if entry.reference_id in set(direction.reference_ids)
                ),
            ],
        )
    return steps


def _review(
    group: Group, *, nodes: str, role: UiSheetRole, prompt: str, pictures: Sequence[Any]
) -> StepRef:
    answer = group.step(
        "review_schema",
        title="The review's shape",
        uses=f"{nodes}#ui_review_schema",
        with_={"role": role.role},
    )
    return group.step(
        "review",
        title=f"Review the {role.role} sheet",
        uses="fx/structured.generate@1",
        with_={
            "prompt": prompt,
            "system": REVIEW_SYSTEM,
            "schema": answer.outputs.schema,
            "context": list(pictures),
            "max_tokens": REVIEW_MAX_TOKENS,
        },
    )


__all__ = [
    "REVIEW_MAX_TOKENS",
    "REVIEW_SYSTEM",
    "SHEET_TAKES",
    "add_ui_sheet_steps",
    "admit_ui_sheet",
    "publish_ui_sheet",
    "ui_review_schema",
    "ui_template",
]
