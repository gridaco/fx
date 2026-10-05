"""Compose the chosen layers into a manifest and one preview frame. Pseudo-code: drawing elided."""

from grida.fx import Ctx, node


@node(
    "compose",
    inputs={"layers": "image{}"},
    params={"specs": list, "canvas": dict},
    outputs={"manifest": "json", "preview": "image/png"},
)
def compose(ctx: Ctx) -> dict:
    specs = {spec["id"]: spec for spec in ctx.params["specs"]}
    manifest = {
        "canvas": ctx.params["canvas"],
        "layers": [
            {
                "id": key,
                "file": f"{key}.png",
                "order": specs[key]["order"],
                "parallax": specs[key]["parallax"],
                "offset_y": specs[key]["offset_y"],
            }
            for key in ctx.inputs["layers"]
        ],
    }
    preview = draw_preview(ctx.inputs["layers"], manifest)  # stack the layers at scroll 0
    return {"manifest": ctx.out.json(manifest), "preview": ctx.out.png(preview)}
