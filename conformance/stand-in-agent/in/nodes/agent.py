"""A node type of the stand-in-agent case: an agent with one tool that must submit a height."""

from grida.fx import Ctx, node, tool

HEIGHT = {
    "type": "object",
    "properties": {"height": {"type": "integer"}, "unit": {"type": "string"}},
    "required": ["height", "unit"],
    "additionalProperties": False,
}


@tool
def measure(ctx: Ctx, thing: str) -> dict:
    """Measures a thing and says how tall it is."""
    ctx.state.setdefault("measured", []).append(thing)
    return {"thing": thing, "height": 3, "unit": "hands"}


@node(
    "survey",
    params={"task": str},
    outputs={"answer": "json"},
    calls={"agent.turn": 3},
    version=1,
)
async def survey(ctx: Ctx) -> dict:
    agent = ctx.agent(system="You measure things before you answer.", tools=[measure])
    submitted = await agent.run(ctx.params["task"], max_steps=3, submit=HEIGHT)
    ctx.fact("measured", ctx.state.get("measured", []))
    ctx.fact("transcript_roles", [message["role"] for message in agent.transcript])
    return {"answer": ctx.out.json({"submitted": submitted, "transcript": agent.transcript})}
