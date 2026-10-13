"""The Python half of the mixed-hosts case: node types that read what the TypeScript half made,
and make what it reads."""

from grida.fx import Ctx, node


@node("keys", inputs={"entries": "text{}"}, outputs={"text": "text"}, version=1)
def keys(ctx: Ctx) -> dict:
    entries = ctx.inputs["entries"].items()
    lines = [f"{key} {file.read_bytes().decode()}\n" for key, file in entries]
    return {"text": ctx.out.text("".join(lines))}


@node("measure", inputs={"report": "json"}, outputs={"sizes": "text[]"}, version=1)
def measure(ctx: Ctx) -> dict:
    report = ctx.read.json("report")
    ctx.fact("letters", report["letters"])
    return {"sizes": [ctx.out.text(f"{word}: {len(word)}\n") for word in report["words"]]}
