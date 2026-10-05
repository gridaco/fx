"""Node types of the run-local case: every output shape a local node returns."""

from grida.fx import Ctx, node


@node("say", params={"word": str}, outputs={"text": "text"}, version=1)
def say(ctx: Ctx) -> dict:
    ctx.fact("letters", len(ctx.params["word"]))
    return {"text": ctx.out.text(ctx.params["word"] + "\n")}


@node("describe", inputs={"text": "text"}, outputs={"report": "json"}, version=1)
def describe(ctx: Ctx) -> dict:
    word = ctx.read.text("text").strip()
    report = {
        "word": word,
        "letters": len(word),
        "half": len(word) / 2,
        "ratio": len(word) / 8,
        "vowels": [letter for letter in word if letter in "aeiou"],
        "upper": word.upper(),
        "empty": None,
    }
    return {"report": ctx.out.json(report)}


@node("letters", params={"word": str}, outputs={"parts": "text[]"}, version=1)
def letters(ctx: Ctx) -> dict:
    return {"parts": [ctx.out.text(letter) for letter in ctx.params["word"][:2]]}


@node("index", params={"word": str}, outputs={"entries": "text{}"}, version=1)
def index(ctx: Ctx) -> dict:
    word = ctx.params["word"]
    return {"entries": {"first": ctx.out.text(word[0]), "last": ctx.out.text(word[-1])}}


@node("mark", inputs={"subject": "text"}, outputs={"annotations": "annotations"}, version=1)
def mark(ctx: Ctx) -> dict:
    ctx.annotate(shape="point", at=[0.25, 0.5], label="the dot of the i", tag="spot")
    ctx.annotate(shape="box", box=[0.1, 0.2, 0.4, 0.6], label="the first letter", color="#e4572e")
    ctx.annotate(shape="points", points=[[0.1, 0.9], [0.9, 0.9]], closed=False, label="a line")
    ctx.annotate(label="a note about the whole word")
    return {}


@node("check", inputs={"subject": "text"}, outputs={}, judge=True, version=1)
def check(ctx: Ctx) -> dict:
    words = ctx.read.text("subject").split()
    ctx.fact("words", len(words))
    ctx.fact("verdict", "accept" if words else "reject")
    return {}
