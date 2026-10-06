"""Agent nodes for rigged-character. Pseudo-code."""

from grida.fx import Ctx, node, tool

# ---- references: an agent that uses image generation as a tool ---------------------------


@tool
async def generate_image(ctx: Ctx, prompt: str, view: str) -> str:
    """Draw one view of the character (sheet, front or back). Returns its handle."""
    if view == "sheet":
        result = await ctx.image_generate(prompt=prompt, size="1024x1536")
    elif "sheet" not in ctx.state:
        return "draw the sheet first"  # front and back are drawn from it
    else:  # an edit takes input pictures: the view is drawn from the sheet
        result = await ctx.image_edit(prompt=prompt, image=ctx.state["sheet"], size="1024x1536")
    ctx.state[view] = result.image
    return f"drew {view}"


@node(
    "draw_references",
    inputs={"brief": "text"},
    # the most it may spend, priced by the plan: the sheet, then front and back from it
    calls={"agent.turn": 12, "image.generate": 1, "image.edit": 2},
    resources=["prompts/reference-agent.md"],
    outputs={"sheet": "image/png", "front": "image/png", "back": "image/png"},
)
async def draw_references(ctx: Ctx) -> dict:
    agent = ctx.agent(system=ctx.prompt("prompts/reference-agent.md"), tools=[generate_image])
    await agent.run(ctx.read.text("brief"), max_steps=12)
    missing = {"sheet", "front", "back"} - set(ctx.state)
    if missing:
        raise ctx.fail(f"agent finished without {sorted(missing)}")  # a broken result, not a take
    return {v: ctx.state[v] for v in ("sheet", "front", "back")}


# ---- assembly: an agent with Blender tools --------------------------------------------------


@tool
def rotate(ctx: Ctx, axis: str, degrees: float) -> str:
    """Rotate the assembly around an axis (x, y or z)."""
    ctx.tool("blender").script(
        "blender/rotate.py", scene=ctx.state["scene"], axis=axis, degrees=degrees
    )
    return "ok"


@tool
def ground(ctx: Ctx) -> str:
    """Move the assembly so its lowest point rests on the ground plane."""
    ctx.tool("blender").script("blender/ground.py", scene=ctx.state["scene"])
    return "ok"


@tool
def look(ctx: Ctx) -> list:
    """Render front, side and top views of the current assembly."""
    return ctx.tool("blender").script("blender/render_views.py", scene=ctx.state["scene"]).images


@node(
    "orient_and_ground",
    inputs={"parts": "model[]"},
    outputs={"mesh": "model/gltf-binary"},
    calls={"agent.turn": 30},
    tools=["blender>=5.2"],
    resources=[
        "prompts/orient-agent.md",
        "blender/assemble.py",
        "blender/rotate.py",
        "blender/ground.py",
        "blender/render_views.py",
        "blender/export.py",
    ],
)
async def orient_and_ground(ctx: Ctx) -> dict:
    ctx.state["scene"] = (
        ctx.tool("blender")
        .script(
            "blender/assemble.py",
            parts=[p.path for p in ctx.inputs["parts"]],
            out=ctx.work_path("scene.blend"),
        )
        .scene
    )
    agent = ctx.agent(system=ctx.prompt("prompts/orient-agent.md"), tools=[rotate, ground, look])
    await agent.run("Stand the character upright, facing +Y, feet on the ground.", max_steps=30)
    ctx.tool("blender").script(
        "blender/export.py", scene=ctx.state["scene"], out=ctx.out.path("mesh")
    )
    return {"mesh": ctx.out.file(ctx.out.path("mesh"))}


@node(
    "rig_with_blender",
    inputs={"mesh": "model"},
    outputs={"model": "model/gltf-binary"},
    calls={"agent.turn": 40},
    tools=["blender>=5.2"],
)
async def rig_with_blender(
    ctx: Ctx,
) -> dict: ...  # same pattern: an agent with Blender rigging tools
