"""Example node exports for the terrain helper and a deterministic preview."""

from terrain_example.atlas import compose_canonical_terrain
from terrain_example.steps import admit_atlas, assemble_atlas, atlas_paint_target

from grida.fx import Ctx, node


@node(
    "terrain_preview",
    inputs={"atlas": "image/png"},
    params={"rows": list},
    outputs={"image": "image/png", "manifest": "json"},
    version=1,
)
def terrain_preview(ctx: Ctx) -> dict:
    image, manifest = compose_canonical_terrain(ctx.read.bytes("atlas"), ctx.params["rows"])
    return {"image": ctx.out.bytes(image, "image/png"), "manifest": ctx.out.json(manifest)}


__all__ = ["atlas_paint_target", "admit_atlas", "assemble_atlas", "terrain_preview"]
