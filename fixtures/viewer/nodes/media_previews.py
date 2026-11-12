"""Original media for the focused preview playground, synthesized without providers.

Pillow draws the images and video frames, FFmpeg encodes the two video formats,
and the GLB writer below builds textured geometry and two optional animation clips.
No artwork, model files or network resources are read.
"""

import io
import json
import math
import struct

from PIL import Image, ImageDraw

from grida.fx import Ctx, node


@node("viewer_media_image", outputs={"image": "image/png"}, version=1)
def image(ctx: Ctx) -> dict:
    picture = Image.new("RGB", (320, 180), "#f4e9cd")
    pen = ImageDraw.Draw(picture)
    pen.rounded_rectangle((22, 20, 297, 159), radius=24, fill="#354b70")
    pen.ellipse((48, 42, 139, 133), fill="#f2a65a")
    pen.polygon([(205, 36), (273, 137), (137, 137)], fill="#74c4b4")
    return {"image": ctx.out.png(picture)}


def _video(ctx: Ctx, codec: str):
    """Four seconds of visibly moving geometry; both containers hold real video."""
    raw_path = ctx.work_path("frames.rgb")
    with raw_path.open("wb") as raw:
        for index in range(48):
            frame = Image.new("RGB", (320, 180), "#354b70")
            pen = ImageDraw.Draw(frame)
            left = round(20 + (240 * index / 48))
            pen.rounded_rectangle((left, 38, left + 54, 107), radius=10, fill="#74c4b4")
            pen.ellipse((260 - left // 2, 110, 296 - left // 2, 146), fill="#f2a65a")
            pen.line((20, 160, 300, 160), fill="#f4e9cd", width=3)
            pen.ellipse((14 + index * 5, 154, 26 + index * 5, 166), fill="#f2a65a")
            raw.write(frame.tobytes())
    encoding = (
        ["-c:v", "ffv1", "-level", "3", "-pix_fmt", "bgr0"]
        if codec == "ffv1"
        else [
            "-c:v",
            "libvpx-vp9",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            "-pix_fmt",
            "yuv420p",
        ]
    )
    output = ctx.out.path("motion.mkv" if codec == "ffv1" else "motion.webm")
    ctx.tool("ffmpeg").run(
        [
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgb24",
            "-video_size",
            "320x180",
            "-framerate",
            "12",
            "-i",
            str(raw_path),
            "-frames:v",
            "48",
            "-an",
            *encoding,
            "-threads",
            "1",
            "-map_metadata",
            "-1",
            "-fflags",
            "+bitexact",
            "-flags:v",
            "+bitexact",
            str(output),
        ],
        timeout_s=15,
    )
    kind = "video/x-matroska" if codec == "ffv1" else "video/webm"
    return ctx.out.file(output, kind)


CODEC = {"type": "string", "enum": ["vp9", "ffv1"]}


@node(
    "viewer_media_video",
    params={"codec": CODEC},
    outputs={"video": "video"},
    tools=["ffmpeg"],
    version=1,
)
def video(ctx: Ctx) -> dict:
    return {"video": _video(ctx, ctx.params["codec"])}


@node(
    "viewer_media_video_poster",
    params={"codec": CODEC},
    outputs={"video": "video", "poster": "image/png"},
    tools=["ffmpeg"],
    version=1,
)
def video_poster(ctx: Ctx) -> dict:
    codec = ctx.params["codec"]
    poster = Image.new("RGB", (320, 180), "#f4e9cd")
    pen = ImageDraw.Draw(poster)
    pen.rounded_rectangle((26, 20, 293, 159), radius=22, fill="#db746c")
    pen.polygon([(137, 49), (137, 131), (204, 90)], fill="#f4e9cd")
    pen.text((38, 30), f"CUSTOM POSTER / {codec.upper()}", fill="#354b70")
    return {"video": _video(ctx, codec), "poster": ctx.out.png(poster)}


def _model(animated: bool) -> bytes:
    """Build an original checker-textured beacon, with Turn and Bob clips if requested."""
    positions, normals, texcoords, indices = [], [], [], []

    def face(vertices, normal):
        base = len(positions) // 3
        for vertex, uv in zip(vertices, [(0, 0), (1, 0), (1, 1), (0, 1)], strict=True):
            positions.extend(vertex)
            normals.extend(normal)
            texcoords.extend(uv)
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3])

    for x, low, high, z in [(0.9, -0.2, 0.1, 0.55), (0.5, 0.1, 1.9, 0.35), (0.7, 1.9, 2.2, 0.5)]:
        face([[-x, low, z], [x, low, z], [x, high, z], [-x, high, z]], [0, 0, 1])
        face([[x, low, -z], [-x, low, -z], [-x, high, -z], [x, high, -z]], [0, 0, -1])
        face([[x, low, z], [x, low, -z], [x, high, -z], [x, high, z]], [1, 0, 0])
        face([[-x, low, -z], [-x, low, z], [-x, high, z], [-x, high, -z]], [-1, 0, 0])
        face([[-x, high, z], [x, high, z], [x, high, -z], [-x, high, -z]], [0, 1, 0])
        face([[-x, low, -z], [x, low, -z], [x, low, z], [-x, low, z]], [0, -1, 0])

    binary = bytearray()
    views, accessors = [], []

    def buffer_view(data):
        binary.extend(bytes((-len(binary)) % 4))
        index = len(views)
        views.append({"buffer": 0, "byteOffset": len(binary), "byteLength": len(data)})
        binary.extend(data)
        return index

    def accessor(values, kind, width, component=5126, **extra):
        code = "f" if component == 5126 else "H"
        view = buffer_view(struct.pack(f"<{len(values)}{code}", *values))
        index = len(accessors)
        accessors.append(
            {
                "bufferView": view,
                "componentType": component,
                "count": len(values) // width,
                "type": kind,
                **extra,
            }
        )
        return index

    attributes = {
        "POSITION": accessor(positions, "VEC3", 3, min=[-0.9, -0.2, -0.55], max=[0.9, 2.2, 0.55]),
        "NORMAL": accessor(normals, "VEC3", 3),
        "TEXCOORD_0": accessor(texcoords, "VEC2", 2),
    }
    triangles = accessor(indices, "SCALAR", 1, component=5123)
    checker = Image.new("RGB", (4, 4))
    colors = ["#e1282d", "#f5c31e", "#2350e1", "#1eb450"]
    pen = ImageDraw.Draw(checker)
    for y in range(4):
        for x in range(4):
            pen.point((x, y), fill=colors[(x // 2) + 2 * (y // 2)])
    png = io.BytesIO()
    checker.save(png, format="PNG")
    texture_view = buffer_view(png.getvalue())
    document = {
        "asset": {"version": "2.0", "generator": "FX original media preview fixture"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": "Geometric beacon", "mesh": 0}],
        "meshes": [
            {"primitives": [{"attributes": attributes, "indices": triangles, "material": 0}]}
        ],
        "materials": [
            {
                "name": "Original checker",
                "doubleSided": True,
                "pbrMetallicRoughness": {
                    "baseColorTexture": {"index": 0},
                    "metallicFactor": 0.12,
                    "roughnessFactor": 0.65,
                },
            }
        ],
        "textures": [{"source": 0, "sampler": 0}],
        "samplers": [{"magFilter": 9728, "minFilter": 9728}],
        "images": [{"bufferView": texture_view, "mimeType": "image/png"}],
        "bufferViews": views,
        "accessors": accessors,
    }
    if animated:
        turn_times = accessor([0, 1, 2, 3, 4], "SCALAR", 1, min=[0], max=[4])
        rotations = []
        for angle in [0, math.pi / 2, math.pi, math.pi * 1.5, math.pi * 2]:
            rotations.extend([0, math.sin(angle / 2), 0, math.cos(angle / 2)])
        turns = accessor(rotations, "VEC4", 4)
        bob_times = accessor([0, 1, 2], "SCALAR", 1, min=[0], max=[2])
        bobs = accessor([0, 0, 0, 0, 0.35, 0, 0, 0, 0], "VEC3", 3)
        document["animations"] = [
            {
                "name": name,
                "samplers": [{"input": times, "output": values, "interpolation": "LINEAR"}],
                "channels": [{"sampler": 0, "target": {"node": 0, "path": path}}],
            }
            for name, times, values, path in [
                ("Turn", turn_times, turns, "rotation"),
                ("Bob", bob_times, bobs, "translation"),
            ]
        ]
    document["buffers"] = [{"byteLength": len(binary)}]
    data = json.dumps(document, separators=(",", ":")).encode()
    data += b" " * ((-len(data)) % 4)
    binary.extend(bytes((-len(binary)) % 4))
    total = 12 + 8 + len(data) + 8 + len(binary)
    return (
        struct.pack("<5I", 0x46546C67, 2, total, len(data), 0x4E4F534A)
        + data
        + struct.pack("<2I", len(binary), 0x004E4942)
        + binary
    )


@node(
    "viewer_media_model",
    params={"animated": bool},
    outputs={"model": "model/gltf-binary"},
    version=1,
)
def model(ctx: Ctx) -> dict:
    output = ctx.out.path("beacon.glb")
    output.write_bytes(_model(ctx.params["animated"]))
    return {"model": ctx.out.file(output, "model/gltf-binary")}
