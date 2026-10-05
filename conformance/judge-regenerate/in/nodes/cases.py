"""Node types the conformance cases use: small, local and deterministic."""

from grida.fx import Ctx, node


@node("shout", params={"text": str}, outputs={"text": "text"}, version=1)
def shout(ctx: Ctx) -> dict:
    return {"text": ctx.out.text(ctx.params["text"].upper())}


@node("join", inputs={"parts": "text{}"}, outputs={"text": "text"}, version=1)
def join(ctx: Ctx) -> dict:
    parts = ctx.inputs["parts"].items()
    return {"text": ctx.out.text("|".join(f"{k}={v.read_bytes().decode()}" for k, v in parts))}


@node("lines", inputs={"text": "text"}, outputs={"items": "json"}, version=1)
def lines(ctx: Ctx) -> dict:
    found = [line for line in ctx.read.text("text").splitlines() if line.strip()]
    items = [{"id": f"l{i}", "text": text} for i, text in enumerate(found)]
    return {"items": ctx.out.json({"lines": items})}


@node(
    "verdict",
    inputs={"subject": "file"},
    params={"accept_take": int},
    outputs={},
    judge=True,
    version=1,
)
def verdict(ctx: Ctx) -> dict:
    accepted = ctx.instance.take >= ctx.params["accept_take"]
    ctx.fact("verdict", "accept" if accepted else "reject")
    return {}
