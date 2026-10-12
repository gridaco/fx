"""Local, source-pixel motion composition. No model redraws the supplied interface."""

from __future__ import annotations

import json
import math
import subprocess
from itertools import pairwise
from pathlib import Path

from PIL import Image, ImageColor, ImageDraw, ImageFont

SIZE = (1920, 1080)
FPS = 30


def smooth(value: float) -> float:
    value = max(0.0, min(value, 1.0))
    return value * value * (3 - 2 * value)


def ease(value: float) -> float:
    return 1 - (1 - max(0.0, min(value, 1.0))) ** 4


class Type:
    def __init__(self, path: Path):
        self.path = path
        self.faces: dict[tuple[int, int], ImageFont.FreeTypeFont] = {}

    def font(self, size: int, weight: int = 600) -> ImageFont.FreeTypeFont:
        key = (size, weight)
        if key in self.faces:
            return self.faces[key]
        face = ImageFont.truetype(str(self.path), size)
        axes = face.get_variation_axes()
        if axes:
            face.set_variation_by_axes([weight])
        self.faces[key] = face
        return face

    def fit(self, text: str, size: int, max_width: int, weight: int = 600):
        while self.font(size, weight).getlength(text) > max_width and size > 24:
            size -= 2
        return self.font(size, weight)


def probe(executable: Path, source: Path) -> dict:
    try:
        done = subprocess.run(
            [
                str(executable),
                "-v",
                "error",
                "-show_streams",
                "-show_format",
                "-of",
                "json",
                str(source),
            ],
            capture_output=True,
            timeout=30,
            check=True,
        )
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError):
        raise ValueError("FFprobe could not inspect the supplied video") from None
    data = json.loads(done.stdout)
    streams = [stream for stream in data["streams"] if stream.get("codec_type") == "video"]
    if len(streams) != 1:
        raise ValueError("supply exactly one video stream")
    video = streams[0]
    duration = float(data["format"]["duration"])
    if not 1 <= duration <= 120:
        raise ValueError("the source recording must be 1-120 seconds")
    if not 320 <= int(video["width"]) <= 7680 or not 180 <= int(video["height"]) <= 4320:
        raise ValueError("the source dimensions are outside the supported range")
    return {
        "width": int(video["width"]),
        "height": int(video["height"]),
        "seconds": duration,
        "frame_rate": video["avg_frame_rate"],
        "codec": video["codec_name"],
        "has_audio": any(stream.get("codec_type") == "audio" for stream in data["streams"]),
    }


def validate_crop(crop: list) -> list:
    if not isinstance(crop, list) or len(crop) != 4:
        raise ValueError("crop must be four normalized source bounds")
    if any(
        isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value)
        for value in crop
    ):
        raise ValueError("crop bounds must be finite numbers")
    if not 0 <= crop[0] < crop[2] <= 1 or not 0 <= crop[1] < crop[3] <= 1:
        raise ValueError("crop must be a nonempty rectangle within the source")
    if crop[2] - crop[0] < 0.1 or crop[3] - crop[1] < 0.1:
        raise ValueError("crop dimensions must cover at least one tenth of the source")
    return crop


def validate_brief(brief: dict) -> dict:
    for field in ("product", "eyebrow", "support", "intent"):
        if not isinstance(brief.get(field), str) or not 1 <= len(brief[field]) <= 1200:
            raise ValueError(f"brief.{field} must be a nonempty string of at most 1200 characters")
    for field in ("opening", "closing"):
        lines = brief.get(field)
        if not isinstance(lines, list) or len(lines) != 2:
            raise ValueError(f"brief.{field} needs exactly two lines")
        if any(
            not isinstance(line, str) or not 1 <= len(line) <= 48 or any(ord(c) < 32 for c in line)
            for line in lines
        ):
            raise ValueError(f"brief.{field} lines need 1-48 characters")
    for field in ("accent", "background", "ink"):
        color = brief.get(field)
        if not isinstance(color, str) or len(color) != 7 or not color.startswith("#"):
            raise ValueError(f"brief.{field} needs a #rrggbb color")
        ImageColor.getrgb(color)
    if len(brief["product"]) > 24 or len(brief["eyebrow"]) > 48 or len(brief["support"]) > 80:
        raise ValueError("product, eyebrow or support copy is too long for this composition")
    regions = brief.get("regions", {})
    if not isinstance(regions, dict) or len(regions) > 12:
        raise ValueError("brief.regions must contain at most twelve named source crop regions")
    for name, crop in regions.items():
        if not isinstance(name, str) or not 1 <= len(name) <= 40:
            raise ValueError("source region names must contain 1-40 characters")
        validate_crop(crop)
    hints = brief.get("focus_hints", [])
    if not isinstance(hints, list) or len(hints) > 12:
        raise ValueError("brief.focus_hints must contain at most twelve hints")
    for hint in hints:
        if not isinstance(hint, dict):
            raise ValueError("each focus hint must be an object")
        for key in ("source_second", "x", "y"):
            value = hint.get(key)
            if (
                isinstance(value, bool)
                or not isinstance(value, (int, float))
                or not math.isfinite(value)
            ):
                raise ValueError("focus hint times and coordinates must be finite numbers")
        if hint["source_second"] < 0 or not 0 <= hint["x"] <= 1 or not 0 <= hint["y"] <= 1:
            raise ValueError("focus hint coordinates or times are outside their bounds")
        if hint.get("region") is not None and hint["region"] not in regions:
            raise ValueError("a focus hint names an undefined source crop region")
    return brief


def validate_edit(edit: dict) -> dict:
    bounds = {"opening_seconds": (1, 3.5), "footage_seconds": (6, 16), "closing_seconds": (1.5, 4)}
    for name, (low, high) in bounds.items():
        number = edit.get(name)
        if (
            isinstance(number, bool)
            or not isinstance(number, (int, float))
            or not math.isfinite(number)
        ):
            raise ValueError(f"edit.{name} must be a finite number")
        if not low <= number <= high:
            raise ValueError(f"edit.{name} must be between {low} and {high}")
    total = sum(edit[name] for name in bounds)
    if not 10 <= total <= 22:
        raise ValueError("the edit must be 10-22 seconds")
    focus = edit.get("focus")
    if not isinstance(focus, list) or not 3 <= len(focus) <= 8:
        raise ValueError("the edit needs 3-8 focus beats")
    previous = -1.0
    for point in focus:
        if (
            not isinstance(point, dict)
            or not {"at", "x", "y", "zoom"} <= set(point)
            or set(point) - {"at", "x", "y", "zoom", "crop"}
        ):
            raise ValueError("each focus beat needs at, x, y, zoom and optional crop")
        if any(
            isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v)
            for v in (point[key] for key in ("at", "x", "y", "zoom"))
        ):
            raise ValueError("focus coordinates must be finite numbers")
        if not 0 <= point["at"] <= edit["footage_seconds"] or point["at"] <= previous:
            raise ValueError("focus beats must be strictly sorted within the footage duration")
        if not 0 <= point["x"] <= 1 or not 0 <= point["y"] <= 1 or not 1 <= point["zoom"] <= 2.25:
            raise ValueError("focus coordinates or zoom are outside their bounds")
        validate_crop(point.get("crop", [0, 0, 1, 1]))
        previous = point["at"]
    if abs(focus[0]["at"]) > 0.001 or abs(focus[-1]["at"] - edit["footage_seconds"]) > 0.001:
        raise ValueError("focus beats must begin at zero and end at footage_seconds")
    if any(point.get("crop", [0, 0, 1, 1]) != [0, 0, 1, 1] for point in (focus[0], focus[-1])):
        raise ValueError("the first and last beats must show the complete source bounds")
    if focus[0]["zoom"] > 1.1 or focus[-1]["zoom"] > 1.1:
        raise ValueError("the first and last focus beats must show the complete recording")
    if any(b["at"] - a["at"] < 0.5 for a, b in pairwise(focus)):
        raise ValueError("focus beats need at least half a second between them")
    note = edit.get("editorial_note")
    if not isinstance(note, str) or not note or len(note) > 1200:
        raise ValueError("edit.editorial_note must contain 1-1200 characters")
    if "source_segments" in edit:
        segments = edit["source_segments"]
        if not isinstance(segments, list) or not 1 <= len(segments) <= 8:
            raise ValueError("source_segments needs 1-8 chronological intervals")
        previous_end, duration = 0.0, 0.0
        for segment in segments:
            if not isinstance(segment, dict) or set(segment) != {
                "source_start",
                "source_end",
                "duration_seconds",
            }:
                raise ValueError(
                    "each source segment needs source_start, source_end and duration_seconds"
                )
            if any(
                isinstance(value, bool)
                or not isinstance(value, (int, float))
                or not math.isfinite(value)
                for value in segment.values()
            ):
                raise ValueError("source segment values must be finite numbers")
            start, end, seconds = (
                segment[key] for key in ("source_start", "source_end", "duration_seconds")
            )
            if start < previous_end or not 0 <= start < end <= 120 or end - start < 0.1 - 1e-9:
                raise ValueError(
                    "source segments must be chronological, non-overlapping and nonempty"
                )
            if not 0.5 <= seconds <= 16 or abs(seconds * FPS - round(seconds * FPS)) > 0.000001:
                raise ValueError(
                    "segment durations need at least half a second and whole output frames"
                )
            previous_end, duration = end, duration + seconds
        if not math.isclose(duration, edit["footage_seconds"], abs_tol=0.000001):
            raise ValueError("source segment durations must sum to footage_seconds")
    return edit


def source_timeline(edit: dict, source_seconds: float) -> list[dict] | None:
    """Explicit chronological source intervals; omitted field keeps legacy retiming."""
    if "source_segments" not in edit:
        return None
    validate_edit(edit)
    timeline, output_start = [], 0.0
    for segment in edit["source_segments"]:
        if segment["source_end"] > source_seconds + 0.000001:
            raise ValueError("a source segment falls after the recording ends")
        frames = round(segment["duration_seconds"] * FPS)
        output_end = output_start + frames / FPS
        timeline.append(
            {
                **segment,
                "output_start": output_start,
                "output_end": output_end,
                "frames": frames,
                "playback_rate": (segment["source_end"] - segment["source_start"]) / (frames / FPS),
            }
        )
        output_start = output_end
    return timeline


def map_source_hint(timeline: list[dict], source_second: float) -> float:
    """Map a retained source moment into the body, including exact cut boundaries."""
    for segment in timeline:
        if segment["source_start"] <= source_second < segment["source_end"]:
            return segment["output_start"] + (
                (source_second - segment["source_start"]) / segment["playback_rate"]
            )
    raise ValueError("a requested focus hint falls in an omitted source interval")


def camera(points: list[dict], seconds: float) -> tuple[float, ...]:
    keys = ("x", "y", "zoom")
    for left, right in pairwise(points):
        if seconds <= right["at"]:
            move_start = max(left["at"], right["at"] - 0.75)
            share = smooth((seconds - move_start) / (right["at"] - move_start))
            position = tuple(left[key] + (right[key] - left[key]) * share for key in keys)
            left_crop, right_crop = left.get("crop", [0, 0, 1, 1]), right.get("crop", [0, 0, 1, 1])
            if left_crop != right_crop:
                selected = right if seconds >= move_start else left
                return tuple(selected[key] for key in keys) + tuple(
                    selected.get("crop", [0, 0, 1, 1])
                )
            return position + tuple(left_crop)
    return tuple(points[-1][key] for key in keys) + tuple(points[-1].get("crop", [0, 0, 1, 1]))


class Composition:
    def __init__(self, brief: dict, edit: dict, font: Path):
        self.brief = validate_brief(brief)
        self.edit = validate_edit(edit)
        self.type = Type(font)
        self.duration = sum(
            edit[key] for key in ("opening_seconds", "footage_seconds", "closing_seconds")
        )
        self.frames = round(self.duration * FPS)
        self.bg = ImageColor.getrgb(brief["background"])
        self.ink = ImageColor.getrgb(brief["ink"])
        self.accent = ImageColor.getrgb(brief["accent"])
        self.base = Image.new("RGB", SIZE, self.bg)
        draw = ImageDraw.Draw(self.base)
        # Fine, low-contrast registration lines frame the launch composition.
        line = tuple(round(a * 0.94 + b * 0.06) for a, b in zip(self.bg, self.ink, strict=True))
        draw.line((92, 96, 1828, 96), fill=line, width=1)
        draw.line((92, 984, 1828, 984), fill=line, width=1)
        draw.text((96, 42), brief["product"], font=self.type.font(32, 750), fill=self.ink)
        draw.text(
            (1824, 52), brief["eyebrow"], font=self.type.font(17, 650), anchor="ra", fill=self.ink
        )
        draw.text((96, 1007), brief["support"], font=self.type.font(22, 500), fill=self.ink)
        draw.text(
            (1824, 1009),
            "01 / IDEA TO CANVAS",
            font=self.type.font(17, 650),
            anchor="ra",
            fill=self.ink,
        )

    def title(self, seconds: float, closing: bool = False) -> Image.Image:
        canvas = self.base.copy() if not closing else Image.new("RGB", SIZE, self.ink)
        draw = ImageDraw.Draw(canvas)
        if closing:
            draw.text((96, 58), self.brief["product"], font=self.type.font(34, 750), fill=self.bg)
            draw.text(
                (1824, 66),
                self.brief["eyebrow"],
                font=self.type.font(17),
                anchor="ra",
                fill=self.bg,
            )
        lines = self.brief["closing" if closing else "opening"]
        color = self.bg if closing else self.ink
        for index, line in enumerate(lines):
            progress = ease((seconds - index * 0.27) / 0.8)
            y = 320 + index * 208 + round((1 - progress) * 95)
            face = self.type.fit(line, 168 if not closing else 150, 1680, 600)
            alpha = progress
            tint = tuple(
                round(a * alpha + b * (1 - alpha))
                for a, b in zip(color, self.ink if closing else self.bg, strict=True)
            )
            draw.text((142, y), line, font=face, fill=tint)
        marker = ease((seconds - 0.45) / 0.75)
        y = 325 + 208 + round((1 - ease((seconds - 0.27) / 0.8)) * 95)
        # The accent advances as an underline: text remains exact and unobscured.
        width = min(
            1560,
            int(self.type.fit(lines[1], 168 if not closing else 150, 1680).getlength(lines[1])),
        )
        draw.rounded_rectangle(
            (148, y + 176, 148 + max(2, width * marker), y + 183), radius=4, fill=self.accent
        )
        if closing:
            draw.text((148, 876), self.brief["support"], font=self.type.font(30, 500), fill=self.bg)
            # Original geometric mark, not a third-party logo.
            for offset in (0, 28, 56):
                draw.rounded_rectangle(
                    (1712 + offset, 876 - offset // 2, 1730 + offset, 936 - offset // 2),
                    radius=9,
                    fill=self.accent,
                )
        return canvas

    def body(self, source: Image.Image, seconds: float) -> Image.Image:
        x, y, zoom, left_bound, top_bound, right_bound, bottom_bound = camera(
            self.edit["focus"], seconds
        )
        canvas = self.base.copy()
        full_width, full_height = source.size
        source = source.crop(
            (
                round(left_bound * full_width),
                round(top_bound * full_height),
                round(right_bound * full_width),
                round(bottom_bound * full_height),
            )
        )
        x = (x - left_bound) / (right_bound - left_bound)
        y = (y - top_bound) / (bottom_bound - top_bound)
        scale = min(1712 / source.width, 820 / source.height, 1.7) * zoom
        width, height = round(source.width * scale), round(source.height * scale)
        # At wide framing the complete recording is centered. Close framing follows
        # the caller's source-bound focus; the source itself is never painted over.
        follow = min(1.0, (zoom - 1) / 0.55)
        focus_x, focus_y = 0.5 + (x - 0.5) * follow, 0.5 + (y - 0.5) * follow
        left, top = round(960 - focus_x * width), round(540 - focus_y * height)
        if width >= 1712:
            left = max(1824 - width, min(96, left))
        if height >= 820:
            top = max(950 - height, min(130, top))
        layer = source.resize((width, height), Image.Resampling.LANCZOS)
        mask = Image.new("L", (width, height))
        ImageDraw.Draw(mask).rounded_rectangle(
            (0, 0, width - 1, height - 1), radius=round(18 * zoom), fill=255
        )
        # A shadow and border belong to the frame, never to the source UI.
        shadow = ImageDraw.Draw(canvas)
        shadow.rounded_rectangle(
            (left - 2, top + 7, left + width + 2, top + height + 10),
            radius=round(20 * zoom),
            fill=(215, 220, 207),
        )
        canvas.paste(layer, (left, top), mask)
        # Persistent title and footer are redrawn above any camera overscan.
        canvas.paste(self.base.crop((0, 0, 1920, 105)), (0, 0))
        canvas.paste(self.base.crop((0, 978, 1920, 1080)), (0, 978))
        return canvas

    def picture(self, index: int, source: Image.Image | None) -> Image.Image:
        seconds = index / FPS
        opening = self.edit["opening_seconds"]
        body_end = opening + self.edit["footage_seconds"]
        if seconds < opening:
            return self.title(seconds)
        if seconds >= body_end:
            return self.title(seconds - body_end, closing=True)
        if source is None:
            raise ValueError("a source frame is required during the footage segment")
        canvas = self.body(source, seconds - opening)
        # Short dissolves connect the title scenes to exact recording pixels.
        entrance = min(1.0, (seconds - opening) / 0.22)
        exit_share = min(1.0, (body_end - seconds) / 0.18)
        if entrance < 1:
            canvas = Image.blend(self.title(opening), canvas, entrance)
        if exit_share < 1:
            canvas = Image.blend(self.title(0, closing=True), canvas, exit_share)
        return canvas
