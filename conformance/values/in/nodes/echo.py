from grida.fx import node, param


@node(
    "acme_echo",
    params={"p1": str, "p2": param(str, default="schema default")},
    outputs={"text": "text/plain"},
    version=1,
)
def echo(ctx):
    return {"text": ctx.out.text(f"{ctx.params['p1']}; {ctx.params['p2']}")}
