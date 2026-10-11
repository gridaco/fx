"""Render a bounded forward-scrolling preview without changing the layer assets.

Each layer completes an explicitly requested integer number of horizontal wraps.
This preview retimes the layers to close exactly at constant forward velocity;
the source manifest's runtime parallax factors remain unchanged and are reported.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass

from PIL import Image, ImageChops, ImageStat

from grida.fx import Ctx, node

BACKGROUND_RGB = (245, 245, 245)
MAX_FRAME_PIXELS = 240_000_000
MAX_SOURCE_PIXELS = 40_000_000
MAX_PREPARED_PIXELS = 24_000_000


@dataclass(frozen=True)
class Settings:
    width: int
    height: int
    fps: int
    duration_seconds: int
    layer_wraps: dict[str, int]
    native_width: int
    native_height: int

    @property
    def frame_count(self) -> int:
        return self.fps * self.duration_seconds


@dataclass(frozen=True)
class Layer:
    id: str
    strip: Image.Image
    period: int
    height: int
    offset_y: int
    parallax: float
    wraps: int
    native_size: tuple[int, int]


def integer(value: object, label: str, minimum: int, maximum: int) -> int:
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError(f"{label} must be an integer from {minimum} to {maximum}")
    return value


def settings_for(manifest: dict, params: dict) -> Settings:
    canvas = manifest.get("canvas")
    if not isinstance(canvas, dict):
        raise ValueError("manifest.canvas must be an object")
    native_width = integer(canvas.get("width"), "canvas width", 1, 8192)
    native_height = integer(canvas.get("height"), "canvas height", 1, 8192)
    width = integer(params.get("width"), "width", 256, 1536)
    height = round(width * native_height / native_width)
    if width % 2 or height % 2 or not 2 <= height <= 1536:
        raise ValueError("preview dimensions must be even and at most 1536 pixels per side")
    fps = integer(params.get("fps"), "fps", 12, 60)
    duration = integer(params.get("duration_seconds"), "duration_seconds", 2, 20)
    wraps = params.get("layer_wraps")
    if not isinstance(wraps, dict) or not 1 <= len(wraps) <= 16:
        raise ValueError("layer_wraps must be an object with one to sixteen entries")
    if any(not isinstance(key, str) or not key for key in wraps):
        raise ValueError("layer_wraps keys must be nonempty strings")
    for value in wraps.values():
        integer(value, "layer wrap count", 1, 16)
    if width * height * fps * duration > MAX_FRAME_PIXELS:
        raise ValueError("preview exceeds the 240 million pixel-frame rendering limit")
    return Settings(width, height, fps, duration, dict(wraps), native_width, native_height)


def prepare_layers(
    manifest: dict, pictures: dict[str, Image.Image], settings: Settings
) -> list[Layer]:
    specs = manifest.get("layers")
    if not isinstance(specs, list) or not 1 <= len(specs) <= 16:
        raise ValueError("manifest.layers must contain between one and sixteen layers")
    if any(not isinstance(spec, dict) for spec in specs):
        raise ValueError("each layer must be an object")
    ids = [spec.get("id") for spec in specs]
    if any(not isinstance(key, str) or not key for key in ids) or len(set(ids)) != len(ids):
        raise ValueError("layer ids must be unique nonempty strings")
    if set(ids) != set(pictures):
        raise ValueError("manifest layer ids must match the input image collection")
    if set(ids) != set(settings.layer_wraps):
        raise ValueError("layer_wraps keys must exactly match the manifest layer ids")
    orders = [integer(spec.get("order"), "layer order", -1000, 1000) for spec in specs]
    if len(set(orders)) != len(orders):
        raise ValueError("layer orders must be unique")
    if sum(image.width * image.height for image in pictures.values()) > MAX_SOURCE_PIXELS:
        raise ValueError("source layers exceed the 40 million pixel limit")
    scale = settings.width / settings.native_width
    layers = []
    prepared_pixels = 0
    for spec in sorted(specs, key=lambda item: item["order"]):
        key = spec["id"]
        image = pictures[key]
        integer(image.width, "layer image width", 1, 8192)
        integer(image.height, "layer image height", 1, 8192)
        factor = spec.get("parallax")
        if type(factor) not in (int, float) or not math.isfinite(factor) or not 0 <= factor <= 8:
            raise ValueError("layer parallax must be finite and between zero and eight")
        offset_y = integer(spec.get("offset_y"), "layer offset_y", -8192, 8192)
        width = max(1, round(image.width * scale))
        height = max(1, round(image.height * scale))
        if width > 8192 or height > 1536:
            raise ValueError("scaled layer exceeds the preview raster limits")
        prepared_pixels += (width + settings.width + 2) * height
        if prepared_pixels > MAX_PREPARED_PIXELS:
            raise ValueError("prepared layers exceed the 24 million pixel memory limit")
        # Interpolate premultiplied colors, preventing transparent pixels from
        # introducing dark fringes when resizing and sampling fractional phases.
        tile = image.convert("RGBa").resize((width, height), Image.Resampling.LANCZOS)
        strip = Image.new("RGBa", (width + settings.width + 2, height))
        for x in range(0, strip.width, width):
            strip.paste(tile, (x, 0))
        layers.append(
            Layer(
                key,
                strip,
                width,
                height,
                round(offset_y * scale),
                float(factor),
                settings.layer_wraps[key],
                image.size,
            )
        )
    return layers


def layer_phase(frame_index: int, period: int, wraps: int, frame_count: int) -> float:
    """Forward, constant-speed periodic sampling; the equal endpoint is excluded."""
    return period * ((frame_index * wraps) % frame_count) / frame_count


def render_frame(layers: list[Layer], settings: Settings, frame_index: int) -> Image.Image:
    frame = Image.new("RGBA", (settings.width, settings.height), (*BACKGROUND_RGB, 255))
    for layer in layers:
        phase = layer_phase(frame_index, layer.period, layer.wraps, settings.frame_count)
        # One periodic sampling surface avoids the fractional-coverage gaps of
        # drawing separately rounded/antialiased adjacent tiles.
        row = layer.strip.transform(
            (settings.width, layer.height),
            Image.Transform.AFFINE,
            (1, 0, phase, 0, 1, 0),
            Image.Resampling.BILINEAR,
        ).convert("RGBA")
        frame.alpha_composite(row, (0, layer.offset_y))
    return frame.convert("RGB")


def pixel_difference(first: Image.Image, second: Image.Image) -> float:
    means = ImageStat.Stat(ImageChops.difference(first, second)).mean
    return round(sum(means) / len(means), 6)


def validate_video(probe: dict, settings: Settings) -> dict:
    streams = probe.get("streams", [])
    if len(streams) != 1:
        raise ValueError("preview must have exactly one video stream and no audio")
    stream = streams[0]
    if (
        stream.get("codec_type") != "video"
        or stream.get("codec_name") != "h264"
        or stream.get("pix_fmt") != "yuv420p"
        or stream.get("width") != settings.width
        or stream.get("height") != settings.height
        or int(stream.get("nb_read_frames", -1)) != settings.frame_count
        or stream.get("avg_frame_rate") != f"{settings.fps}/1"
    ):
        raise ValueError("encoded preview does not match its declared format, size or frame count")
    duration = float(stream.get("duration", 0))
    if abs(duration - settings.duration_seconds) > 1 / settings.fps:
        raise ValueError("encoded preview duration differs from the requested duration")
    return {"codec": "h264", "pixel_format": "yuv420p", "duration_seconds": duration}


@node(
    "render_preview",
    inputs={"layers": "image{}", "manifest": "json"},
    params={
        "width": {"type": "integer", "minimum": 256, "maximum": 1536},
        "fps": {"type": "integer", "minimum": 12, "maximum": 60},
        "duration_seconds": {"type": "integer", "minimum": 2, "maximum": 20},
        "layer_wraps": {
            "type": "object",
            "minProperties": 1,
            "maxProperties": 16,
            "additionalProperties": {"type": "integer", "minimum": 1, "maximum": 16},
        },
    },
    outputs={"animation": "video/mp4", "poster": "image/png", "report": "json"},
    tools=["ffmpeg", "ffprobe"],
)
def render_preview(ctx: Ctx) -> dict:
    """Render a retimed MP4 forward loop from the chosen repeating layers."""
    ffmpeg = ctx.tool("ffmpeg")
    ffprobe = ctx.tool("ffprobe")
    manifest = ctx.read.json("manifest")
    if not isinstance(manifest, dict):
        raise ctx.fail("preview manifest must be an object")
    settings = settings_for(manifest, ctx.params)
    pictures = {}
    source_pixels = 0
    for key, source in ctx.inputs["layers"].items():
        with Image.open(source.path) as opened:
            integer(opened.width, "layer image width", 1, 8192)
            integer(opened.height, "layer image height", 1, 8192)
            source_pixels += opened.width * opened.height
            if source_pixels > MAX_SOURCE_PIXELS:
                raise ctx.fail("source layers exceed the 40 million pixel limit")
            pictures[key] = opened.convert("RGBA")
    layers = prepare_layers(manifest, pictures, settings)
    first = render_frame(layers, settings, 0)
    last = first
    raw_path = ctx.work_path("frames.rgb")
    video_path = ctx.out.path("preview.mp4")
    try:
        with raw_path.open("wb") as raw:
            for index in range(settings.frame_count):
                if ctx.cancelled:
                    raise ctx.fail("preview rendering was cancelled")
                last = first if index == 0 else render_frame(layers, settings, index)
                raw.write(last.tobytes())
                if index % settings.fps == 0:
                    ctx.progress(
                        "Rendering the forward-loop preview", 0.8 * index / settings.frame_count
                    )
        ctx.progress("Encoding the MP4 preview", 0.85)
        ffmpeg.run(
            [
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-y",
                "-f",
                "rawvideo",
                "-pixel_format",
                "rgb24",
                "-video_size",
                f"{settings.width}x{settings.height}",
                "-framerate",
                str(settings.fps),
                "-i",
                "frames.rgb",
                "-an",
                "-c:v",
                "libx264",
                "-preset",
                "medium",
                "-crf",
                "20",
                "-threads",
                "2",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
                "-frames:v",
                str(settings.frame_count),
                "out/preview.mp4",
            ],
            timeout_s=180,
        )
        probe = ffprobe.run(
            ["-v", "error", "-count_frames", "-show_streams", "-of", "json", "out/preview.mp4"],
            timeout_s=60,
        )
        encoding = validate_video(json.loads(probe.stdout), settings)
    finally:
        raw_path.unlink(missing_ok=True)
    endpoint = render_frame(layers, settings, settings.frame_count)
    report = {
        "schema": "parallax_preview_v1",
        "motion_model": "forward_integer_wraps",
        "retimed_preview": True,
        "motion_description": (
            "Layers scroll forward at constant speed for integer wraps; the endpoint is excluded."
        ),
        "retiming_description": (
            "Preview speeds close the loop; source runtime parallax factors remain unchanged."
        ),
        "width": settings.width,
        "height": settings.height,
        "fps": settings.fps,
        "frame_count": settings.frame_count,
        "layer_wraps": settings.layer_wraps,
        "native_canvas": {"width": settings.native_width, "height": settings.native_height},
        "background_rgb": list(BACKGROUND_RGB),
        "endpoint_mean_absolute_pixel_difference": pixel_difference(first, endpoint),
        "last_to_first_mean_absolute_pixel_difference": pixel_difference(last, first),
        "layers": [
            {
                "id": layer.id,
                "input_digest": ctx.inputs["layers"][layer.id].digest,
                "native_width": layer.native_size[0],
                "native_height": layer.native_size[1],
                "source_parallax": layer.parallax,
                "wrap_count": layer.wraps,
                "native_pixels_per_second": (
                    layer.native_size[0] * layer.wraps / settings.duration_seconds
                ),
                "export_pixels_per_second": (
                    layer.period * layer.wraps / settings.duration_seconds
                ),
                "rendered_offset_y": layer.offset_y,
            }
            for layer in layers
        ],
        **encoding,
    }
    ctx.progress("Preview complete", 1)
    return {
        "animation": ctx.out.file(video_path, "video/mp4"),
        "poster": ctx.out.png(first),
        "report": ctx.out.json(report),
    }
