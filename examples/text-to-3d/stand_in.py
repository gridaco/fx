"""Original procedural image and mesh for an offline wiring check, never generated artwork."""

import io
import json
import struct

from PIL import Image, ImageDraw

from grida.fx import Answer, StandInCall


def reference_png() -> bytes:
    """A flat moss-green lantern silhouette on a gray square, drawn here with Pillow."""
    picture = Image.new("RGB", (1024, 1024), (238, 238, 240))
    draw = ImageDraw.Draw(picture)
    draw.rounded_rectangle((350, 130, 674, 490), radius=145, outline=(57, 98, 65), width=48)
    draw.rounded_rectangle((260, 350, 764, 850), radius=150, fill=(57, 98, 65))
    for left in (330, 470, 610):
        draw.rounded_rectangle((left, 470, left + 85, 700), radius=30, fill=(230, 170, 65))
    output = io.BytesIO()
    picture.save(output, "PNG")
    return output.getvalue()


def model_glb() -> bytes:
    """A self-contained, original cuboid with a moss-green material. No sidecar files."""
    positions = [
        -0.5,
        0,
        -0.4,
        0.5,
        0,
        -0.4,
        0.5,
        1.2,
        -0.4,
        -0.5,
        1.2,
        -0.4,
        -0.5,
        0,
        0.4,
        0.5,
        0,
        0.4,
        0.5,
        1.2,
        0.4,
        -0.5,
        1.2,
        0.4,
    ]
    indices = [
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
    vertices = struct.pack("<24f", *positions)
    triangles = struct.pack("<36H", *indices)
    binary = vertices + triangles
    document = {
        "asset": {"version": "2.0", "generator": "Original FX text-to-3d stand-in"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "material": 0}]}],
        "materials": [
            {
                "pbrMetallicRoughness": {
                    "baseColorFactor": [0.22, 0.38, 0.25, 1],
                    "metallicFactor": 0,
                    "roughnessFactor": 0.7,
                }
            }
        ],
        "buffers": [{"byteLength": len(binary)}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": len(vertices)},
            {"buffer": 0, "byteOffset": len(vertices), "byteLength": len(triangles)},
        ],
        "accessors": [
            {
                "bufferView": 0,
                "componentType": 5126,
                "count": 8,
                "type": "VEC3",
                "min": [-0.5, 0, -0.4],
                "max": [0.5, 1.2, 0.4],
            },
            {"bufferView": 1, "componentType": 5123, "count": 36, "type": "SCALAR"},
        ],
    }
    encoded = json.dumps(document, separators=(",", ":")).encode()
    encoded += b" " * (-len(encoded) % 4)
    length = 12 + 8 + len(encoded) + 8 + len(binary)
    return (
        struct.pack("<4sII", b"glTF", 2, length)
        + struct.pack("<I4s", len(encoded), b"JSON")
        + encoded
        + struct.pack("<I4s", len(binary), b"BIN\0")
        + binary
    )


def answer(call: StandInCall) -> Answer:
    if call.capability == "image.generate":
        return Answer(files={"image": reference_png()})
    if call.capability == "mesh.generate":
        assert call.route.id == "P2-20260801@tripo"
        assert call.request["image"].read_bytes() == reference_png()
        assert not call.request.get("views")
        return Answer(files={"model": model_glb()})
    raise AssertionError(f"this example makes no {call.capability} calls")
