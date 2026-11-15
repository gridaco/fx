"""A fixed-topology terrain atlas and deterministic preview."""

from pathlib import Path

from terrain_example.atlas import terrain_atlas_generation_prompt
from terrain_example.steps import add_atlas_steps

from grida.fx import Workflow

ROOT = Path(__file__).resolve().parent
ROWS = [
    "0000000000000000",
    "0000000111000000",
    "0000000111000000",
    "0011100111000000",
    "0011100111001100",
    "1111111111111111",
    "1111111111111111",
    "1111111111111111",
]


def _build(reference: str) -> Workflow:
    direction = (ROOT / "direction.txt").read_text().strip()
    wf = Workflow(
        "showcase-meadow-terrain",
        title="Golden meadow terrain",
        description="A locked 47-mask atlas and one deterministic occupancy preview.",
        budget={"max_usd": 15},
    )
    wf.note(
        "locked_topology",
        about="terrain",
        text=(
            "## Paint the material, keep the masks\n\n"
            "Change `direction.txt` and the reference to choose a different material. "
            "The 47-mask template and neighborhood lookup define which tile belongs at "
            "each join; changing their geometry is a different atlas contract. "
            "The pixel gate can request another painting, but does not certify the art."
        ),
    )
    wf.note(
        "map_preview",
        about="preview",
        text=(
            "## Inspect the joins in a real map\n\n"
            "`ROWS` is the occupancy map: each 1 selects a tile from its neighbors. "
            "The preview reuses the assembled atlas without repainting it. "
            "Check inside corners and transitions here before using the atlas in a game; "
            "a valid sheet can still have visually distracting joins."
        ),
    )
    steps = add_atlas_steps(
        wf.group("terrain", title="Golden meadow material"),
        nodes="./nodes.py",
        prompt=terrain_atlas_generation_prompt(direction),
        materials=[reference],
    )
    preview = wf.step(
        "preview",
        title="Inspect legal tile joins",
        uses="./nodes.py#terrain_preview",
        with_={"atlas": steps["publish"].outputs.image, "rows": ROWS},
        view=True,
    )
    cover = wf.step(
        "cover",
        title="Build 47 on shared terrain ground",
        uses="./cover.py#terrain_cover",
        with_={"atlas": steps["publish"].outputs.image},
        view=True,
    )
    package = wf.step(
        "package",
        title="Terrain handoff files",
        uses="fx/package@1",
        with_={
            "files": {
                "terrain/atlas.png": steps["publish"].outputs.image,
                "terrain/validation.json": steps["publish"].outputs.validation,
                "terrain/preview.png": preview.outputs.image,
                "terrain/preview.json": preview.outputs.manifest,
            },
            "manifest": {
                "kind": "showcase-terrain-package-v1",
                "material": direction,
                "topology": "47-mask",
                "publication_status": "unreviewed",
            },
        },
    )
    wf.outputs(
        atlas=steps["publish"].outputs.image,
        validation=steps["publish"].outputs.validation,
        preview=preview.outputs.image,
        preview_manifest=preview.outputs.manifest,
        cover=cover.outputs.cover,
        cover_manifest=cover.outputs.layout,
        package=package.outputs.files,
    )
    return wf


def build() -> Workflow:
    return _build("./.showcase-inputs/meadow.png")


def build_fixture() -> Workflow:
    """The same graph with a synthesized offline reference; run make_fixture.py first."""
    return _build("./.fixture/meadow.png")
