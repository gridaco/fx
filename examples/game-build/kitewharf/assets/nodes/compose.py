"""Compose the chosen layers: a manifest a game reads, and one preview frame at scroll 0."""

from PIL import Image

from grida.fx import Ctx, node


def preview(layers: dict, specs: dict, canvas: dict) -> Image.Image:
    """The layers back to front, each repeated across the canvas from x = 0 at its ``offset_y``."""
    frame = Image.new("RGBA", (canvas["width"], canvas["height"]), (0, 0, 0, 0))
    for key in sorted(layers, key=lambda key: specs[key]["order"]):
        with Image.open(layers[key].path) as opened:
            layer = opened.convert("RGBA")
        row = Image.new("RGBA", frame.size, (0, 0, 0, 0))
        for x in range(0, frame.width, layer.width):
            row.paste(layer, (x, specs[key]["offset_y"]))  # clipped at the canvas
        frame = Image.alpha_composite(frame, row)
    return frame


@node(
    "compose",
    inputs={"layers": "image{}"},
    params={"specs": list, "canvas": dict},
    outputs={"manifest": "json", "preview": "image/png"},
)
def compose(ctx: Ctx) -> dict:
    specs = {spec["id"]: spec for spec in ctx.params["specs"]}
    layers = ctx.inputs["layers"]
    manifest = {
        "canvas": ctx.params["canvas"],
        "layers": [
            {
                "id": key,
                "file": f"layers/{key}.png",  # as the run lays out its `layers` output
                "order": specs[key]["order"],
                "parallax": specs[key]["parallax"],
                "offset_y": specs[key]["offset_y"],
            }
            for key in layers
        ],
    }
    frame = preview(layers, specs, ctx.params["canvas"])
    return {"manifest": ctx.out.json(manifest), "preview": ctx.out.png(frame)}
