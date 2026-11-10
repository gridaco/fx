"""Original geometric videos and contrasting authored posters for browser inspection.

Pillow draws four RGB frames and the PNG poster. Local FFmpeg encodes either
lossless FFV1/Matroska or VP9/WebM; no media, network or provider input is used.
"""

from PIL import Image, ImageDraw

from grida.fx import Ctx, node


@node(
    "viewer_poster_video",
    params={"codec": {"type": "string", "enum": ["ffv1", "vp9"]}},
    outputs={"video": "video", "poster": "image/png"},
    tools=["ffmpeg"],
    version=1,
)
def video(ctx: Ctx) -> dict:
    """Encode a short blue/green clip and a distinct cream/coral poster."""
    codec = ctx.params["codec"]
    raw_path = ctx.work_path("frames.rgb")
    with raw_path.open("wb") as raw:
        for index in range(4):
            frame = Image.new("RGB", (64, 48), "#354b70")
            pen = ImageDraw.Draw(frame)
            left = 8 + index * 9
            pen.rectangle((left, 12, left + 14, 35), fill="#74c4b4")
            raw.write(frame.tobytes())

    kind = "video/x-matroska" if codec == "ffv1" else "video/webm"
    output = ctx.out.path("clip.mkv" if codec == "ffv1" else "clip.webm")
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
            "64x48",
            "-framerate",
            "4",
            "-i",
            str(raw_path),
            "-frames:v",
            "4",
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
    poster = Image.new("RGB", (96, 64), "#f4e9cd")
    pen = ImageDraw.Draw(poster)
    pen.polygon([(18, 48), (48, 12), (78, 48)], fill="#db746c")
    pen.text((4, 2), codec.upper(), fill="#354b70")
    return {"video": ctx.out.file(output, kind), "poster": ctx.out.png(poster)}
