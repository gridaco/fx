"""Render Nami's corrected rig with flat textures and fixed framing, without a browser."""

from __future__ import annotations

import json

from _character_harness import Bench
from PIL import Image

from grida.fx import Ctx, node


@node(
    "render_cover",
    inputs={"model": "model/gltf-binary"},
    outputs={"video": "video/mp4", "poster": "image/webp", "capture": "json"},
    tools=["blender", "ffmpeg", "ffprobe"],
    resources=["_character_tools/worker/cover.py"],
    version=3,
)
def render_cover(ctx: Ctx) -> dict:
    bench = Bench(ctx)
    adopted = bench.adopt(ctx.read.bytes("model"), "cover-source.glb")
    source = bench.root / adopted["path"]
    frames = ctx.work_path("capture")
    bootstrap = ctx.work_path("render-cover.py")
    bootstrap.write_text(
        "import runpy, sys\n"
        f"sys.path.insert(0, {str(bench.root / 'code')!r})\n"
        "runpy.run_module('_character_tools.worker.cover', run_name='__main__')\n"
    )
    ctx.tool("blender").run(
        [
            "--background",
            "--factory-startup",
            "--disable-autoexec",
            "--python",
            str(bootstrap),
            "--",
            "--model",
            str(source),
            "--output",
            str(frames),
        ],
        timeout_s=1800,
    )
    capture = json.loads((frames / "capture.json").read_text())
    video = ctx.out.path("cover.mp4")
    poster = ctx.out.path("cover.webp")
    ctx.tool("ffmpeg").run(
        [
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-framerate",
            "24",
            "-i",
            str(frames / "%04d.png"),
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            "20",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            str(video),
        ],
        timeout_s=180,
    )
    with Image.open(frames / "0024.png") as frame:
        frame.convert("RGB").save(poster, "WEBP", quality=90, method=6)
    probe = json.loads(
        ctx.tool("ffprobe")
        .run(
            ["-v", "error", "-count_frames", "-show_streams", "-of", "json", str(video)],
            timeout_s=60,
        )
        .stdout
    )
    streams = probe.get("streams", [])
    if len(streams) != 1 or any(
        streams[0].get(key) != value
        for key, value in {
            "codec_type": "video",
            "codec_name": "h264",
            "width": 960,
            "height": 600,
            "pix_fmt": "yuv420p",
            "avg_frame_rate": "24/1",
            "nb_read_frames": "144",
        }.items()
    ):
        raise ctx.fail("Encoded rig cover differs from its declared capture")
    if capture["model_sha256"] != adopted["sha256"]:
        raise ctx.fail("Rendered cover names a different model")
    if (
        capture["material_policy"] != "base_color_texture_emission_only"
        or capture["scene_light_count"] != 0
    ):
        raise ctx.fail("Rendered rig cover differs from its flat texture policy")
    ctx.fact("provider_calls", 0)
    return {
        "video": ctx.out.file(video, "video/mp4"),
        "poster": ctx.out.file(poster, "image/webp"),
        "capture": ctx.out.json(capture),
    }
