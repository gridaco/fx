"""A tiny runtime-expanded failure branch for viewer terminal-event metadata."""

from grida.fx import Ctx, node


@node("viewer_failure_items", outputs={"items": "json"}, version=1)
def propose(ctx: Ctx) -> dict:
    return {
        "items": ctx.out.json(
            {"entries": [{"id": "broken", "message": "Intentional dynamic viewer fixture failure"}]}
        )
    }


@node(
    "viewer_dynamic_fail",
    params={"message": str},
    outputs={"report": "json"},
    version=1,
)
def fail(ctx: Ctx) -> dict:
    raise ctx.fail(ctx.params["message"])


@node(
    "viewer_dynamic_blocked",
    inputs={"report": "json"},
    outputs={"report": "json"},
    version=1,
)
def blocked(ctx: Ctx) -> dict:
    # The harness requires that failed input resolution prevents this body from starting.
    return {"report": ctx.inputs["report"]}
