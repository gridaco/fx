"""A text description becomes a reference image, then a textured 3D model."""

from grida.fx import Workflow

DEFAULT_TEXT = (
    "A small moss-green ceramic lantern with a rounded body, a wide handle, "
    "and three amber window openings."
)


def generate_3d_asset(text: str = DEFAULT_TEXT) -> Workflow:
    """Describe two cached steps. The engine plans and runs their paid calls."""
    if not isinstance(text, str) or not text.strip():
        raise ValueError("text must be a nonblank description")

    workflow = Workflow(
        "text-to-3d",
        title="Text to 3D asset",
        description="Generate one reference image; Tripo P2 builds its textured mesh.",
        inputs={"description": {"type": "string", "default": text}},
    )
    reference = workflow.step(
        "reference",
        title="Reference image",
        uses="fx/image.generate@1",
        with_={
            "prompt": "./prompts/reference.md",
            "size": "1024x1024",
            "background": "opaque",
        },
    )
    model = workflow.step(
        "model",
        title="Textured 3D model",
        uses="fx/mesh.generate@1",
        with_={
            "image": reference.outputs.image,
            "face_limit": 5000,
            "quad": False,
            "texture": True,
            "pbr": True,
        },
    )
    workflow.outputs(reference=reference.outputs.image, model=model.outputs.model)
    return workflow
