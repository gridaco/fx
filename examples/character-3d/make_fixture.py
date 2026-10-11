"""Synthesize a provider-free planning/refusal input, not character artwork."""

import json
import struct
from io import BytesIO
from pathlib import Path

from PIL import Image, ImageDraw


def textured_box() -> bytes:
    """A tiny, original GLB with an embedded checker texture and UVs."""
    positions = [
        (-0.5, 0, -0.3),
        (0.5, 0, -0.3),
        (0.5, 2, -0.3),
        (-0.5, 2, -0.3),
        (-0.5, 0, 0.3),
        (0.5, 0, 0.3),
        (0.5, 2, 0.3),
        (-0.5, 2, 0.3),
    ]
    triangles = [
        0,
        2,
        1,
        0,
        3,
        2,
        4,
        5,
        6,
        4,
        6,
        7,
        0,
        1,
        5,
        0,
        5,
        4,
        3,
        7,
        6,
        3,
        6,
        2,
        0,
        4,
        7,
        0,
        7,
        3,
        1,
        2,
        6,
        1,
        6,
        5,
    ]
    texture = Image.new("RGB", (4, 4), (40, 112, 126))
    texture.putpixel((0, 0), (224, 188, 110))
    png = BytesIO()
    texture.save(png, format="PNG")
    chunks = [
        struct.pack("<24f", *(value for point in positions for value in point)),
        struct.pack("<16f", *([0, 0, 1, 0, 1, 1, 0, 1] * 2)),
        struct.pack("<36H", *triangles),
        png.getvalue(),
    ]
    binary, views = bytearray(), []
    for chunk in chunks:
        binary.extend(b"\x00" * (-len(binary) % 4))
        views.append({"buffer": 0, "byteOffset": len(binary), "byteLength": len(chunk)})
        binary.extend(chunk)
    doc = {
        "asset": {"version": "2.0", "generator": "FX geometric fixture"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": "character", "mesh": 0}],
        "meshes": [
            {
                "name": "character",
                "primitives": [
                    {
                        "attributes": {"POSITION": 0, "TEXCOORD_0": 1},
                        "indices": 2,
                        "material": 0,
                    }
                ],
            }
        ],
        "accessors": [
            {
                "bufferView": 0,
                "componentType": 5126,
                "count": 8,
                "type": "VEC3",
                "min": [-0.5, 0, -0.3],
                "max": [0.5, 2, 0.3],
            },
            {"bufferView": 1, "componentType": 5126, "count": 8, "type": "VEC2"},
            {"bufferView": 2, "componentType": 5123, "count": 36, "type": "SCALAR"},
        ],
        "materials": [
            {
                "name": "checker",
                "doubleSided": True,
                "pbrMetallicRoughness": {
                    "baseColorTexture": {"index": 0},
                    "metallicFactor": 0,
                    "roughnessFactor": 1,
                },
            }
        ],
        "textures": [{"source": 0}],
        "images": [{"bufferView": 3, "mimeType": "image/png"}],
        "bufferViews": views,
        "buffers": [{"byteLength": len(binary)}],
    }
    encoded = json.dumps(doc, separators=(",", ":")).encode()
    encoded += b" " * (-len(encoded) % 4)
    binary.extend(b"\x00" * (-len(binary) % 4))
    length = 12 + 8 + len(encoded) + 8 + len(binary)
    return (
        struct.pack("<4sII", b"glTF", 2, length)
        + struct.pack("<I4s", len(encoded), b"JSON")
        + encoded
        + struct.pack("<I4s", len(binary), b"BIN\x00")
        + binary
    )


def make_fixture(home: Path | None = None) -> Path:
    root = (home or Path(__file__).resolve().parent) / "inputs" / "fixture"
    root.mkdir(parents=True, exist_ok=True)
    image = Image.new("RGBA", (96, 144), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.ellipse((30, 8, 66, 44), fill=(195, 159, 122, 255))
    draw.rectangle((28, 44, 68, 96), fill=(40, 112, 126, 255))
    draw.rectangle((29, 96, 44, 135), fill=(62, 73, 91, 255))
    draw.rectangle((52, 96, 67, 135), fill=(62, 73, 91, 255))
    image.save(root / "reference.png")
    (root / "brief.md").write_text(
        "# Geometry fixture\n\n"
        "One original adult caretaker with short hair, a fitted jacket, trousers, boots "
        "and fixed mitten hands. This flat geometric fixture exercises input loading "
        "and planning only; it is not generated art or an approved character.\n",
        encoding="utf-8",
    )
    (root / "inputs.yaml").write_text(
        "brief: brief.md\nreference: reference.png\n"
        "profile: sd_human_fixed_hands_v1\npartition: whole\n"
        "review: required\nquality_bar: low\nrounds: 1\nrebuilds: 1\n"
        "through: rig\npreservation: audit\n"
        "mesh:\n  face_limit: 5000\n  quad: true\n  pbr: true\n  texture: true\n"
        "limits:\n  producer_turns: 2\n  reviewer_turns: 1\n  reference_images: 1\n"
        "  crops: 2\n  revisions: 1\n  recent_images: 4\n",
        encoding="utf-8",
    )
    (root / "part.glb").write_bytes(textured_box())
    (root / "normalize.yaml").write_text(
        "fx: workflow/v1\nid: normalize-fixture\ntitle: Normalize the geometric fixture\n"
        "inputs:\n  mesh: {type: file, kind: model}\n"
        "steps:\n  normalize:\n    uses: ./nodes/parts.py#normalize\n"
        "    with:\n      mesh: ${{ inputs.mesh }}\n      role: character\n"
        "outputs:\n  model: ${{ steps.normalize.outputs.model }}\n"
        "  report: ${{ steps.normalize.outputs.report }}\n",
        encoding="utf-8",
    )
    (root / "normalize-inputs.yaml").write_text("mesh: part.glb\n", encoding="utf-8")
    return root / "inputs.yaml"


if __name__ == "__main__":
    print(make_fixture())
