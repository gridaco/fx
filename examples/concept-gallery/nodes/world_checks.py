"""Deterministic checks of a proposed world: free, exact, and run before the paid reviewer."""

from grida.fx import Ctx, node


@node(
    "well_formed",
    inputs={"world": "json"},
    params={"max_entities": int},
    outputs={},
    judge=True,
    version=1,
)
def well_formed(ctx: Ctx) -> dict:
    world = ctx.read.json("world")
    ids = [entity["id"] for entity in world["entities"]]
    problems = []
    if len(ids) != len(set(ids)):
        problems.append("two entities share an id")
    if not 1 <= len(ids) <= ctx.params["max_entities"]:
        problems.append(f"{len(ids)} entities, outside 1..{ctx.params['max_entities']}")
    known = set(ids)
    for relation in world["relationships"]:
        for end in (relation.get("from"), relation.get("to")):
            if end not in known:
                problems.append(f"a relationship names {end}, which is not an entity")
    ctx.fact("problems", problems)
    ctx.fact("verdict", "reject" if problems else "accept")
    return {}
