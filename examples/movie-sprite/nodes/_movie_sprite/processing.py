"""Provider-free native sprite finishing with disk-backed frame sequences.

The caller owns provenance and atomic publication. Only compressed artifacts are
returned as bytes; decoded sequences stay on temporary disk and every native
output frame is independently decoded and verified before return.
"""

from __future__ import annotations

import hashlib
import json
import math
import shutil
import subprocess
import tempfile
import zipfile
from fractions import Fraction
from pathlib import Path
from typing import Any

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

from .codec import encode_png
from .frame_store import FrameStore
from .geometry import AttachmentRepair, Pixels, anchor_rgba, fixed_masks, morph
from .models import (
    MAX_FRAME_PIXELS,
    MAX_FRAMES,
    MAX_INPUT_BYTES,
    MAX_OUTPUT_BYTES,
    ChromaSettings,
    FinishSettings,
)


def validate_finish_config(config: dict[str, Any]) -> None:
    """Validate raw-independent configuration before any provider operation."""
    FinishSettings.model_validate(config)


def _run(command: list[str], *, output: Path | None = None) -> bytes:
    try:
        if output is None:
            result = subprocess.run(command, capture_output=True, timeout=1200, check=False)
        else:
            with output.open("wb") as stream:
                result = subprocess.run(
                    command, stdout=stream, stderr=subprocess.PIPE, timeout=1200, check=False
                )
    except subprocess.TimeoutExpired as error:
        raise ValueError("Movie sprite media processing exceeded its time bound") from error
    if result.returncode:
        # The external error may contain a private temporary filename or embedded URL.
        raise ValueError("Movie sprite media command failed")
    return result.stdout or b""


def _probe(path: Path, ffprobe: str) -> dict[str, Any]:
    payload = json.loads(
        _run(
            [
                ffprobe,
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-show_entries",
                "stream=codec_type,codec_name,pix_fmt,width,height,avg_frame_rate,duration:format=duration",
                "-of",
                "json",
                str(path),
            ]
        )
    )
    videos = [item for item in payload.get("streams", []) if item.get("codec_type") == "video"]
    if len(videos) != 1:
        raise ValueError("Source must contain exactly one video stream")
    stream = videos[0]
    width, height = int(stream["width"]), int(stream["height"])
    fps = float(Fraction(stream["avg_frame_rate"]))
    duration = float(stream.get("duration", payload.get("format", {}).get("duration", "nan")))
    if (
        min(width, height) < 2
        or max(width, height) > 3840
        or width % 2
        or height % 2
        or width * height > MAX_FRAME_PIXELS
    ):
        raise ValueError("Even native dimensions up to 2160x3840 or 3840x2160 are required")
    if not math.isfinite(fps) or not 0 < fps <= 60:
        raise ValueError("Source FPS must be positive and at most 60")
    if not math.isfinite(duration) or not 0 < duration <= 60.01:
        raise ValueError("Source duration must be positive and at most 60 seconds")
    return {
        "width": width,
        "height": height,
        "fps": fps,
        "duration_seconds": duration,
        "codec_name": stream["codec_name"],
        "pixel_format": stream["pix_fmt"],
        "audio_stream_count": sum(item.get("codec_type") == "audio" for item in payload["streams"]),
    }


def _validate_video_bytes(raw_video: bytes) -> None:
    if not isinstance(raw_video, bytes) or not 0 < len(raw_video) <= MAX_INPUT_BYTES:
        raise ValueError("raw_video must contain between 1 byte and 1 GiB")
    if not (raw_video[:4] == b"\x1aE\xdf\xa3" or raw_video[4:8] == b"ftyp"):
        raise ValueError("Source must be an MP4 or Matroska/WebM container")


def inspect_source_video(raw_video: bytes) -> dict[str, Any]:
    """Admit one bounded source through complete strict decoding without an RGBA cache.

    Suitable for a provider validator inside its existing retry owner. This function
    makes no provider calls, persists nothing, and returns only portable media facts.
    """
    _validate_video_bytes(raw_video)
    ffmpeg, ffprobe = shutil.which("ffmpeg"), shutil.which("ffprobe")
    if not ffmpeg or not ffprobe:
        raise RuntimeError("ffmpeg and ffprobe are required for movie sprite inspection")
    with tempfile.TemporaryDirectory(prefix="movie-sprite-inspect-") as temporary:
        source = Path(temporary) / "source.video"
        source.write_bytes(raw_video)
        facts = _probe(source, ffprobe)
        if facts["duration_seconds"] * facts["fps"] > MAX_FRAMES + 0.1:
            raise ValueError("Source exceeds the 360-frame bound")
        _run(
            [
                ffmpeg,
                "-v",
                "error",
                "-xerror",
                "-protocol_whitelist",
                "file,pipe",
                "-noautorotate",
                "-i",
                str(source),
                "-map",
                "0:v:0",
                "-frames:v",
                str(MAX_FRAMES + 1),
                "-fps_mode",
                "passthrough",
                "-f",
                "null",
                "-",
            ]
        )
        timeline = json.loads(
            _run(
                [
                    ffprobe,
                    "-v",
                    "error",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-select_streams",
                    "v:0",
                    "-read_intervals",
                    f"%+#{MAX_FRAMES + 1}",
                    "-show_entries",
                    "frame=best_effort_timestamp_time",
                    "-of",
                    "json",
                    str(source),
                ]
            )
        )
        timestamps = np.asarray(
            [float(frame["best_effort_timestamp_time"]) for frame in timeline.get("frames", [])]
        )
        count = len(timestamps)
        if not 2 <= count <= MAX_FRAMES or not np.isfinite(timestamps).all():
            raise ValueError("Source decoded frame count or timestamps exceed the admitted bounds")
        ideal = timestamps[0] + np.arange(count) / facts["fps"]
        if not np.allclose(timestamps, ideal, atol=0.0011, rtol=0):
            raise ValueError("Variable frame rate is unsupported; supply a constant-rate video")
        if abs(count / facts["fps"] - facts["duration_seconds"]) > max(0.002, 1 / facts["fps"]):
            raise ValueError("Source duration disagrees with its decoded frame count")
        facts.update(
            {
                "frame_count": count,
                "timestamps_constant_rate": True,
                "source_sha256": hashlib.sha256(raw_video).hexdigest(),
            }
        )
        return facts


def _decode(
    path: Path,
    directory: Path,
    facts: dict[str, Any],
    ffmpeg: str,
    ffprobe: str,
    *,
    preview_size: tuple[int, int],
) -> FrameStore:
    width, height = facts["width"], facts["height"]
    estimated = facts["duration_seconds"] * facts["fps"]
    if estimated > MAX_FRAMES + 0.1:
        raise ValueError("Source exceeds the 360-frame bound")
    # Reserve both decoded input and output sequences plus bounded compressed exports.
    reserved_frames = math.ceil(estimated) + 2
    decode_limit = min(MAX_FRAMES + 1, reserved_frames)
    required = (
        width * height * 4 * reserved_frames * 2
        + preview_size[0] * preview_size[1] * 3 * reserved_frames
        + MAX_OUTPUT_BYTES
    )
    if shutil.disk_usage(directory).free < required:
        raise ValueError("Insufficient temporary disk space for bounded native finishing")
    output = directory / "source.rgba"
    _run(
        [
            ffmpeg,
            "-v",
            "error",
            "-xerror",
            "-protocol_whitelist",
            "file,pipe",
            "-noautorotate",
            "-i",
            str(path),
            "-map",
            "0:v:0",
            "-frames:v",
            str(decode_limit),
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            "rgba",
            "-f",
            "rawvideo",
            "pipe:1",
        ],
        output=output,
    )
    count, remainder = divmod(output.stat().st_size, width * height * 4)
    if remainder or not 2 <= count <= MAX_FRAMES:
        raise ValueError("Decoded source exceeds the frame bound or contains incomplete pixels")
    timeline = json.loads(
        _run(
            [
                ffprobe,
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-select_streams",
                "v:0",
                "-read_intervals",
                f"%+#{MAX_FRAMES + 1}",
                "-show_entries",
                "frame=best_effort_timestamp_time",
                "-of",
                "json",
                str(path),
            ]
        )
    )
    timestamps = np.asarray(
        [float(frame["best_effort_timestamp_time"]) for frame in timeline.get("frames", [])]
    )
    if len(timestamps) != count or not np.isfinite(timestamps).all():
        raise ValueError("Source timestamps do not describe all decoded frames")
    # Matroska timestamps have millisecond precision; compare against the absolute
    # ideal CFR timeline rather than allowing per-step rounding error to accumulate.
    ideal = timestamps[0] + np.arange(count) / facts["fps"]
    if not np.allclose(timestamps, ideal, atol=0.0011, rtol=0):
        raise ValueError("Variable frame rate is unsupported; supply a constant-rate video")
    if abs(count / facts["fps"] - facts["duration_seconds"]) > max(0.002, 1 / facts["fps"]):
        raise ValueError("Source duration disagrees with its decoded frame count")
    facts["frame_count"] = count
    facts["timestamps_constant_rate"] = True
    return FrameStore(output, (count, height, width, 4))


def key_frame(source: Pixels, config: ChromaSettings) -> Pixels:
    """Extract straight RGBA from a green-dominant source using explicit settings."""
    rgb = source[..., :3].astype(np.float32)
    excess = rgb[..., 1] - np.maximum(rgb[..., 0], rgb[..., 2])
    coverage = np.clip(
        (config.key_excess - excess) / (config.key_excess - config.foreground_excess), 0, 1
    )
    alpha = np.rint(coverage * 255).astype(np.uint8)
    alpha[alpha < config.minimum_alpha] = 0
    coverage = alpha.astype(np.float32) / 255
    key = np.asarray(config.key_rgb, dtype=np.float32)
    recovered = np.clip(
        (rgb - (1 - coverage[..., None]) * key) / np.maximum(coverage[..., None], 1 / 255),
        0,
        255,
    )
    band = Image.fromarray(np.where(alpha < 255, 255, 0).astype(np.uint8))
    if config.despill_radius_pixels:
        band = band.filter(ImageFilter.MaxFilter(2 * config.despill_radius_pixels + 1))
    recovered[..., 1] = np.where(
        np.asarray(band) > 0,
        np.minimum(recovered[..., 1], (recovered[..., 0] + recovered[..., 2]) / 2),
        recovered[..., 1],
    )
    rgba = np.concatenate((np.rint(recovered).astype(np.uint8), alpha[..., None]), axis=2)
    rgba[alpha == 0, :3] = 0
    return rgba


def _size_within(width: int, height: int, maximum: list[int]) -> tuple[int, int]:
    scale = min(1, maximum[0] / width, maximum[1] / height)
    return max(2, int(width * scale) // 2 * 2), max(2, int(height * scale) // 2 * 2)


def _preview_frame(frame: Pixels, size: tuple[int, int]) -> Pixels:
    image = Image.fromarray(frame)
    if image.size != size:
        image = image.resize(size, Image.Resampling.LANCZOS)
    background = Image.new("RGBA", size, (22, 27, 34, 255))
    background.alpha_composite(image)
    return np.asarray(background.convert("RGB"))


def _encode(
    pixels: Path,
    output: Path,
    size: tuple[int, int],
    rate: Fraction,
    ffmpeg: str,
    *,
    alpha: bool,
    maximum_bytes: int,
) -> None:
    codec = (
        [
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-pix_fmt",
            "bgra",
            "-cluster_size_limit",
            "1",
            "-cluster_time_limit",
            "1",
        ]
        if alpha
        else [
            "-c:v",
            "libx264",
            "-crf",
            "16",
            "-preset",
            "medium",
            "-tune",
            "zerolatency",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
        ]
    )
    _run(
        [
            ffmpeg,
            "-v",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba" if alpha else "rgb24",
            "-s",
            f"{size[0]}x{size[1]}",
            "-framerate",
            str(rate),
            "-i",
            str(pixels),
            "-an",
            "-map_metadata",
            "-1",
            # No random container identity or encoder stamp: the same frames are the same file.
            "-fflags",
            "+bitexact",
            "-flags:v",
            "+bitexact",
            *codec,
            "-fs",
            str(maximum_bytes),
            "-flush_packets",
            "1",
            str(output),
        ]
    )
    if output.stat().st_size >= maximum_bytes:
        raise ValueError("Encoded movie sprite exceeds its remaining compressed output byte bound")


def _verify_video(
    path: Path, frames: FrameStore, rate: Fraction, ffmpeg: str, ffprobe: str, *, lossless: bool
) -> None:
    facts = _probe(path, ffprobe)
    count, height, width = frames.shape[:3]
    if (
        facts["audio_stream_count"]
        or abs(facts["fps"] - float(rate)) > 1e-5
        or (facts["width"], facts["height"]) != (width, height)
        or abs(facts["duration_seconds"] - count / float(rate)) > 0.002
    ):
        raise ValueError("Export changed native geometry, playback timing, or silent audio policy")
    if not lossless:
        payload = json.loads(
            _run(
                [
                    ffprobe,
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-count_frames",
                    "-show_entries",
                    "stream=nb_read_frames",
                    "-of",
                    "json",
                    str(path),
                ]
            )
        )
        if int(payload["streams"][0]["nb_read_frames"]) != count:
            raise ValueError("Preview changed the native frame count")
        return
    # Decode a second time without another full sequence on disk or in RAM.
    with tempfile.TemporaryFile() as error_stream:
        process = subprocess.Popen(
            [
                ffmpeg,
                "-v",
                "error",
                "-i",
                str(path),
                "-map",
                "0:v:0",
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "pipe:1",
            ],
            stdout=subprocess.PIPE,
            stderr=error_stream,
        )
        try:
            assert process.stdout is not None
            frame_bytes = width * height * 4
            for expected in frames:
                decoded = process.stdout.read(frame_bytes)
                if len(decoded) != frame_bytes or decoded != expected.tobytes():
                    raise ValueError("Lossless body export changed frame pixels or order")
            if process.stdout.read(1):
                raise ValueError("Lossless body export added frames")
            if process.wait(timeout=1200):
                raise ValueError("Lossless body verification failed")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            if process.stdout is not None:
                process.stdout.close()


def _contact_sheet(frames: FrameStore, rate: Fraction) -> bytes:
    count, height, width = frames.shape[:3]
    size = _size_within(width, height, [160, 320])
    indices = np.linspace(0, count - 1, min(6, count)).round().astype(int)
    sheet = Image.new("RGB", (size[0] * len(indices), (size[1] + 24) * 3), (35, 35, 35))
    draw = ImageDraw.Draw(sheet)
    yy, xx = np.indices((size[1], size[0]))
    checker = np.repeat(np.where((xx // 12 + yy // 12) % 2, 180, 120)[..., None], 3, axis=2)
    for column, index in enumerate(indices):
        small = Image.fromarray(frames[index]).resize(size, Image.Resampling.LANCZOS)
        for row, color in enumerate(((245, 245, 245), (22, 27, 34), None)):
            background = (
                Image.fromarray(checker.astype(np.uint8)).convert("RGBA")
                if color is None
                else Image.new("RGBA", size, (*color, 255))
            )
            background.alpha_composite(small)
            top = row * (size[1] + 24)
            sheet.paste(background.convert("RGB"), (column * size[0], top + 24))
            draw.text(
                (column * size[0] + 3, top + 4),
                f"{index / float(rate):.2f}s / {index}",
                fill="white",
            )
    return encode_png(sheet)


def finish_video(raw_video: bytes, config: dict[str, Any]) -> dict[str, bytes]:
    """Finish a native body clip and return validated standard media artifacts.

    ``video`` is lossless RGBA FFV1 Matroska, ``canonical`` is its exact first-frame
    PNG, ``preview`` is a silent MP4 over a dark background, and ``manifest``,
    ``report`` and ``contact_sheet`` describe the output. ``export_frames`` adds a
    deterministic PNG ``frames_zip``. No facial patches or runtime controls occur.
    """
    settings = FinishSettings.model_validate(config)
    _validate_video_bytes(raw_video)
    source_hash = hashlib.sha256(raw_video).hexdigest()
    if settings.source_sha256 is not None and settings.source_sha256 != source_hash:
        raise ValueError("source_sha256 must match the supplied video bytes")
    ffmpeg, ffprobe = shutil.which("ffmpeg"), shutil.which("ffprobe")
    if not ffmpeg or not ffprobe:
        raise RuntimeError("ffmpeg and ffprobe are required for movie sprite finishing")
    with tempfile.TemporaryDirectory(prefix="movie-sprite-") as temporary:
        directory = Path(temporary)
        source_path = directory / "source.video"
        source_path.write_bytes(raw_video)
        facts = _probe(source_path, ffprobe)
        if settings.coordinate_size is not None and settings.coordinate_size != [
            facts["width"],
            facts["height"],
        ]:
            raise ValueError("coordinate_size must match the decoded native video")
        if settings.source_mode != "chroma" and facts["pixel_format"] not in {
            "rgba",
            "bgra",
            "argb",
            "abgr",
            "yuva420p",
            "yuva422p",
            "yuva444p",
            "gbrap",
        }:
            raise ValueError(
                "RGBA and finished inputs require a decoded format with an alpha channel"
            )
        preview_size = _size_within(facts["width"], facts["height"], settings.preview_max_size)
        source = _decode(source_path, directory, facts, ffmpeg, ffprobe, preview_size=preview_size)
        count, height, width, _ = source.shape
        if settings.loop_closure == "flow" and 2 * settings.seam_frames >= count:
            raise ValueError("Endpoint windows must leave at least one untouched middle frame")
        playback = settings.playback_seconds or count / facts["fps"]
        rate = (Fraction(count, 1) / Fraction(str(playback))).limit_denominator(1_000_000)
        if not 0 < float(rate) <= 60:
            raise ValueError("Playback FPS must be positive and at most 60")
        canonical = (
            key_frame(source[0], settings.chroma)
            if settings.source_mode == "chroma"
            else source[0].copy()
        )
        hard, weight, guards = fixed_masks(settings, (width, height))
        repair_field = np.zeros((height, width), dtype=bool)
        repairs = []
        for repair_config in settings.local_repairs:
            repair = AttachmentRepair(repair_config, canonical, guards, repair_field)
            repairs.append(repair)
            repair_field |= repair.field
        finished_path = directory / "finished.rgba"
        finished = FrameStore(finished_path, source.shape, create=True)
        preview_path = directory / "preview.rgb"
        preview_frames = FrameStore(
            preview_path, (count, preview_size[1], preview_size[0], 3), create=True
        )
        rows = []
        for index in range(count):
            keyed = (
                key_frame(source[index], settings.chroma)
                if settings.source_mode == "chroma"
                else source[index].copy()
            )
            frame = (
                anchor_rgba(keyed, canonical, weight) if settings.fixed_regions else keyed.copy()
            )
            distance = min(index, count - 1 - index)
            amount = (
                0.5 * (1 + math.cos(math.pi * distance / settings.seam_frames))
                if settings.loop_closure == "flow" and distance < settings.seam_frames
                else 0
            )
            flow_maximum = 0.0
            if amount:
                frame, flow_maximum = morph(
                    frame,
                    canonical,
                    amount,
                    proxy_width=settings.flow_proxy_width,
                    maximum_displacement=settings.max_flow_pixels,
                )
                frame[hard] = canonical[hard]
            repair_maxima = []
            for repair in repairs:
                frame, maximum = repair.apply(
                    frame, canonical, hard, weight, keyed if amount == 0 else None
                )
                repair_maxima.append(maximum)
            if not np.array_equal(frame[hard], canonical[hard]):
                raise ValueError("Finishing changed exact fixed-region pixels")
            if amount == 0:
                exterior = (weight == 0) & ~repair_field
                if not np.array_equal(frame[exterior], keyed[exterior]):
                    raise ValueError("Finishing changed moving pixels outside authored corrections")
                if not np.array_equal(frame[guards], keyed[guards]):
                    raise ValueError("Finishing changed a moving guard outside endpoint closure")
            alpha = frame[..., 3]
            if not np.any(alpha == 0) or not np.any(alpha == 255):
                raise ValueError(
                    "Each body frame needs transparent background and opaque foreground"
                )
            finished[index] = frame
            preview_frames[index] = _preview_frame(frame, preview_size)
            rows.append(
                {
                    "frame_index": index,
                    "rgba_sha256": hashlib.sha256(frame.tobytes()).hexdigest(),
                    "endpoint_morph_amount": amount,
                    "maximum_proxy_displacement_pixels": flow_maximum,
                    "local_repair_maximum_native_displacements": repair_maxima,
                    "transparent_pixels": int((alpha == 0).sum()),
                    "partial_alpha_pixels": int(((alpha > 0) & (alpha < 255)).sum()),
                    "opaque_pixels": int((alpha == 255).sum()),
                }
            )
        if not np.array_equal(finished[0], canonical):
            raise ValueError("Finishing changed the canonical first frame")
        if not np.array_equal(finished[-1], canonical):
            raise ValueError(
                "Loop endpoints differ; enable flow closure or supply a closed sequence"
            )
        native_path, mp4_path = directory / "loop.mkv", directory / "preview.mp4"
        _encode(
            finished_path,
            native_path,
            (width, height),
            rate,
            ffmpeg,
            alpha=True,
            maximum_bytes=MAX_OUTPUT_BYTES,
        )
        _encode(
            preview_path,
            mp4_path,
            preview_size,
            rate,
            ffmpeg,
            alpha=False,
            maximum_bytes=MAX_OUTPUT_BYTES - native_path.stat().st_size,
        )
        _verify_video(native_path, finished, rate, ffmpeg, ffprobe, lossless=True)
        _verify_video(mp4_path, preview_frames, rate, ffmpeg, ffprobe, lossless=False)
        canonical_png = encode_png(Image.fromarray(canonical))
        paths = {"video": native_path, "preview": mp4_path}
        if settings.export_frames:
            archive_path = directory / "frames.zip"
            with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_STORED) as archive:
                for index, frame in enumerate(finished):
                    info = zipfile.ZipInfo(
                        f"frame-{index:06d}.png", date_time=(1980, 1, 1, 0, 0, 0)
                    )
                    info.external_attr = 0o600 << 16
                    archive.writestr(info, encode_png(Image.fromarray(frame)))
                    if (
                        archive_path.stat().st_size
                        + sum(path.stat().st_size for path in paths.values())
                        > MAX_OUTPUT_BYTES
                    ):
                        raise ValueError(
                            "PNG frame archive exceeds the compressed output byte bound"
                        )
            paths["frames_zip"] = archive_path
        if sum(path.stat().st_size for path in paths.values()) > MAX_OUTPUT_BYTES:
            raise ValueError("Compressed artifacts exceed the 2 GiB output bound")
        manifest = {
            "schema_version": 1,
            "kind": "movie_sprite_body",
            "width": width,
            "height": height,
            "frame_count": count,
            "playback_fps": float(rate),
            "playback_rate": str(rate),
            "playback_seconds": playback,
            "alpha_mode": "straight",
            "loop": True,
            "audio": False,
            "canonical_frame_index": 0,
            "canonical_sha256": hashlib.sha256(canonical_png).hexdigest(),
            "source_sha256": source_hash,
            "source_frame_indices": list(range(count)),
            "frames_pattern": "frame-{index:06d}.png" if settings.export_frames else None,
        }
        report = {
            "schema_version": 1,
            "kind": "movie_sprite_body_processing",
            "source_sha256": source_hash,
            "source": facts,
            "settings": settings.model_dump(mode="json"),
            "frame_count": count,
            "width": width,
            "height": height,
            "playback_seconds": playback,
            "playback_fps": float(rate),
            "fixed_region_pixels": int(hard.sum()),
            "moving_guard_pixels": int(guards.sum()),
            "local_repair_field_pixels": int(repair_field.sum()),
            "all_native_frames_lossless_verified": True,
            "canonical_first_frame_exact": True,
            "loop_endpoints_exact": True,
            "middle_motion_preserved_outside_authored_fields": True,
            "decoded_sequence_storage": "temporary_disk_frames",
            "semantic_review": "required_separately",
            "frames": rows,
        }
        result = {key: path.read_bytes() for key, path in paths.items()}
        result.update(
            {
                "canonical": canonical_png,
                "manifest": (json.dumps(manifest, indent=2) + "\n").encode(),
                "report": (json.dumps(report, indent=2) + "\n").encode(),
                "contact_sheet": _contact_sheet(finished, rate),
            }
        )
        if sum(len(data) for data in result.values()) > MAX_OUTPUT_BYTES:
            raise ValueError("Complete movie sprite artifacts exceed the 2 GiB output bound")
        return result


__all__ = ["finish_video", "inspect_source_video", "validate_finish_config"]
