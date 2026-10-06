"""Blender steps for rigged-character: free, local, deterministic. Pseudo-code."""

from grida.fx import Ctx, node

BLENDER = ["blender>=5.2"]


@node(
    "normalize",
    inputs={"mesh": "model"},
    params={"height_m": float},
    outputs={"mesh": "model/gltf-binary"},
    tools=BLENDER,
    resources=["blender/normalize.py"],
    version=1,
)
def normalize(ctx: Ctx) -> dict:
    """Scale to height, pivot at the feet, report the bounds as facts."""
    report = ctx.tool("blender").script(
        "blender/normalize.py",
        mesh=ctx.inputs["mesh"].path,
        height_m=ctx.params["height_m"],
        out=ctx.out.path("mesh"),
    )
    for name, value in report.facts.items():
        ctx.fact(name, value)
    return {"mesh": ctx.out.file(ctx.out.path("mesh"))}


@node(
    "turntable",
    inputs={"mesh": "model"},
    outputs={"views": "image[]"},
    tools=BLENDER,
    resources=["blender/render_views.py"],
    version=1,
)
def turntable(ctx: Ctx) -> dict:
    views = (
        ctx.tool("blender").script("blender/render_views.py", scene=ctx.inputs["mesh"].path).images
    )
    return {"views": [ctx.out.file(view) for view in views]}


@node(
    "audit_rig",
    inputs={"model": "model"},
    params={"height_m": float},
    outputs={},
    judge=True,
    tools=BLENDER,
    resources=["blender/audit.py"],
    version=1,
)
def audit_rig(ctx: Ctx) -> dict:
    """Numbers, not pictures: bones, weights, scale, and that the test clips play."""
    audit = (
        ctx.tool("blender")
        .script("blender/audit.py", model=ctx.inputs["model"].path, height_m=ctx.params["height_m"])
        .facts
    )
    for name, value in audit.items():
        ctx.fact(name, value)
    ctx.fact("verdict", "accept" if not audit["problems"] else "reject")
    return {}


@node(
    "export_game_glb",
    inputs={"model": "model"},
    outputs={"model": "model/gltf-binary", "renders": "image[]"},
    tools=BLENDER,
    resources=["blender/export.py", "blender/render_views.py"],
    version=1,
)
def export_game_glb(ctx: Ctx) -> dict:
    ctx.tool("blender").script(
        "blender/export.py",
        scene=ctx.inputs["model"].path,
        out=ctx.out.path("model"),
        clips=["idle", "walk"],
    )
    views = (
        ctx.tool("blender").script("blender/render_views.py", scene=ctx.out.path("model")).images
    )
    return {
        "model": ctx.out.file(ctx.out.path("model")),
        "renders": [ctx.out.file(view) for view in views],
    }
