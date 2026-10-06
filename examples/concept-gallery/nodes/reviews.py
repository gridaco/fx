"""The concept gallery's reviewers: judges you write, each one ``structured.generate`` call.

Each review step names its own route in the workflow (``route:``), a model other than the one
that wrote what it reviews, and ``independent_of:`` makes the plan refuse if the two ever resolve
to the same model.
"""

from grida.fx import Ctx, node

#: An answer to one question, with a reason either way.
MARK = {
    "type": "object",
    "required": ["pass", "reason"],
    "properties": {"pass": {"type": "boolean"}, "reason": {"type": "string"}},
}


def marks_schema(count: int) -> dict:
    """One mark per criterion, in order."""
    return {
        "title": "review",
        "type": "object",
        "required": ["marks"],
        "properties": {
            "marks": {"type": "array", "items": MARK, "minItems": count, "maxItems": count}
        },
    }


def numbered(criteria: list[str]) -> str:
    return "\n".join(f"{number}. {text}" for number, text in enumerate(criteria, start=1))


def decide(ctx: Ctx, criteria: list[str], marks: list[dict]) -> None:
    """Facts ``criteria`` (each one passed or failed, and why) and ``verdict``: accepted only when
    every criterion passed. The model never gives the verdict itself."""
    if len(marks) != len(criteria):
        raise ctx.fail(f"the reviewer marked {len(marks)} criteria, not {len(criteria)}")
    answered = [
        {"criterion": criterion, "pass": mark["pass"], "reason": mark["reason"]}
        for criterion, mark in zip(criteria, marks, strict=True)
    ]
    ctx.fact("criteria", answered)
    ctx.fact("verdict", "accept" if all(mark["pass"] for mark in marks) else "reject")


@node(
    "admit",
    inputs={"world": "json", "synopsis": "text", "direction": "text"},
    params={"criteria": list},
    outputs={},
    judge=True,
    calls={"structured.generate": 1},
    resources=["prompts/admit.md"],
)
async def admit(ctx: Ctx) -> dict:
    criteria = ctx.params["criteria"]
    answer = await ctx.structured_generate(
        prompt=ctx.prompt("prompts/admit.md", criteria=numbered(criteria)),
        schema=marks_schema(len(criteria)),
        context=[ctx.inputs["synopsis"], ctx.inputs["direction"], ctx.inputs["world"]],
    )
    decide(ctx, criteria, answer.json["marks"])
    return {}


@node(
    "review",
    inputs={"image": "image"},
    params={"criteria": list},
    outputs={},
    judge=True,
    calls={"structured.generate": 1},
    resources=["prompts/review-concept.md"],
)
async def review(ctx: Ctx) -> dict:
    criteria = ctx.params["criteria"]
    answer = await ctx.structured_generate(
        prompt=ctx.prompt("prompts/review-concept.md", criteria=numbered(criteria)),
        schema=marks_schema(len(criteria)),
        context=[ctx.inputs["image"]],
    )
    decide(ctx, criteria, answer.json["marks"])
    return {}
