"""Caller-owned, source-bound settings for sprite video finishing."""

from __future__ import annotations

import math
from typing import Annotated, Literal

from pydantic import Field, model_validator

from .contracts import PersistedContractModel

MAX_FRAMES = 360
MAX_FRAME_PIXELS = 2160 * 3840
MAX_INPUT_BYTES = 1024 * 1024 * 1024
MAX_OUTPUT_BYTES = 2 * 1024 * 1024 * 1024

Coordinate = Annotated[float, Field(ge=0, le=3840, allow_inf_nan=False)]


class Region(PersistedContractModel):
    """A polygon in the exact source video's native pixel coordinates."""

    polygon_xy: list[list[Coordinate]] = Field(min_length=3, max_length=128)

    @model_validator(mode="after")
    def validate_polygon(self) -> Region:
        if any(len(point) != 2 for point in self.polygon_xy):
            raise ValueError("polygon_xy requires pairs of coordinates")
        points = self.polygon_xy
        area = (
            abs(
                sum(
                    left[0] * right[1] - left[1] * right[0]
                    for left, right in zip(points, [*points[1:], points[0]], strict=True)
                )
            )
            / 2
        )
        if area < 0.5:
            raise ValueError("polygon_xy must enclose a nonzero area")
        return self


class LocalRepair(Region):
    """Bounded opaque attachment alignment, followed by an exact canonical pin."""

    roi_xyxy: list[int] = Field(min_length=4, max_length=4)
    falloff_pixels: float = Field(default=60, ge=1, le=128, allow_inf_nan=False)
    pin_feather_pixels: float = Field(default=3, ge=0, le=32, allow_inf_nan=False)
    max_displacement_pixels: float = Field(default=36, gt=0, le=128, allow_inf_nan=False)

    @model_validator(mode="after")
    def validate_roi(self) -> LocalRepair:
        x0, y0, x1, y1 = self.roi_xyxy
        if not (0 <= x0 < x1 <= 3840 and 0 <= y0 < y1 <= 3840):
            raise ValueError("roi_xyxy requires an ordered, positive pixel rectangle")
        if min(x1 - x0, y1 - y0) < 24:
            raise ValueError("local alignment ROI dimensions must be at least 24 pixels")
        if self.pin_feather_pixels > self.falloff_pixels:
            raise ValueError("pin feather must fit within the alignment falloff")
        return self


class ChromaSettings(PersistedContractModel):
    """Green-excess alpha extraction with bounded edge despill."""

    key_rgb: list[int] = Field(default_factory=lambda: [11, 242, 18], min_length=3, max_length=3)
    key_excess: float = Field(default=224, ge=-255, le=255, allow_inf_nan=False)
    foreground_excess: float = Field(default=8, ge=-255, le=255, allow_inf_nan=False)
    minimum_alpha: int = Field(default=24, ge=0, le=255)
    despill_radius_pixels: int = Field(default=3, ge=0, le=20)

    @model_validator(mode="after")
    def validate_key(self) -> ChromaSettings:
        if any(not 0 <= value <= 255 for value in self.key_rgb):
            raise ValueError("key_rgb channels must be between 0 and 255")
        if self.key_rgb[1] <= max(self.key_rgb[0], self.key_rgb[2]):
            raise ValueError("chroma extraction requires a green-dominant key")
        if self.key_excess <= self.foreground_excess:
            raise ValueError("key_excess must exceed foreground_excess")
        return self


class FinishSettings(PersistedContractModel):
    """Deterministic body processing; facial repaint is a separate pipeline."""

    source_mode: Literal["chroma", "rgba", "finished"] = "chroma"
    playback_seconds: float | None = Field(default=12, ge=1, le=60, allow_inf_nan=False)
    chroma: ChromaSettings = Field(default_factory=ChromaSettings)
    source_sha256: str | None = Field(default=None, pattern=r"^[a-f0-9]{64}$")
    coordinate_size: list[int] | None = Field(default=None, min_length=2, max_length=2)
    fixed_regions: list[Region] = Field(default_factory=list, max_length=32)
    moving_guards: list[Region] = Field(default_factory=list, max_length=32)
    outside_feather_pixels: float = Field(default=12, ge=0, le=128, allow_inf_nan=False)
    loop_closure: Literal["flow", "none"] = "flow"
    seam_frames: int = Field(default=12, ge=1, le=90)
    flow_proxy_width: int = Field(default=720, ge=24, le=1280)
    max_flow_pixels: float = Field(default=36, gt=0, le=128, allow_inf_nan=False)
    local_repairs: list[LocalRepair] = Field(default_factory=list, max_length=8)
    export_frames: bool = False
    preview_max_size: list[int] = Field(default_factory=lambda: [720, 1280])

    @model_validator(mode="after")
    def validate_geometry(self) -> FinishSettings:
        if len(self.preview_max_size) != 2 or any(
            value < 2 or value > 3840 or value % 2 for value in self.preview_max_size
        ):
            raise ValueError("preview_max_size requires two even dimensions between 2 and 3840")
        if math.prod(self.preview_max_size) > MAX_FRAME_PIXELS:
            raise ValueError("preview_max_size exceeds the native pixel bound")
        regions = [*self.fixed_regions, *self.moving_guards, *self.local_repairs]
        if regions and (self.source_sha256 is None or self.coordinate_size is None):
            raise ValueError("spatial settings require source_sha256 and coordinate_size")
        if self.source_mode == "finished" and (regions or self.loop_closure != "none"):
            raise ValueError("finished input requires loop_closure=none and no spatial corrections")
        if self.coordinate_size is not None:
            width, height = self.coordinate_size
            if (
                min(width, height) < 2
                or max(width, height) > 3840
                or width % 2
                or height % 2
                or width * height > MAX_FRAME_PIXELS
            ):
                raise ValueError("coordinate_size exceeds the supported native dimensions")
            for region in regions:
                if any(x >= width or y >= height for x, y in region.polygon_xy):
                    raise ValueError("polygon leaves the native frame")
            for repair in self.local_repairs:
                if repair.roi_xyxy[2] > width or repair.roi_xyxy[3] > height:
                    raise ValueError("local repair ROI leaves the native frame")
        return self


__all__ = ["ChromaSettings", "FinishSettings", "LocalRepair", "Region"]
