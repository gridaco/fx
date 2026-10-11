"""Spatially bounded stabilization and motion-aligned loop endpoint closure."""

from __future__ import annotations

import importlib
from typing import Any

import numpy as np
from numpy.typing import NDArray
from PIL import Image, ImageDraw
from scipy.ndimage import distance_transform_edt

from .models import FinishSettings, LocalRepair, Region

Pixels = NDArray[np.uint8]
Mask = NDArray[np.bool_]
Weights = NDArray[np.float32]


def polygon_mask(regions: list[Region], size: tuple[int, int]) -> Mask:
    plate = Image.new("L", size)
    draw = ImageDraw.Draw(plate)
    for region in regions:
        draw.polygon([tuple(point) for point in region.polygon_xy], fill=255)
    return np.asarray(np.asarray(plate) == 255, dtype=np.bool_)


def feather_mask(hard: Mask, radius: float) -> Weights:
    if not hard.any() or radius == 0:
        return hard.astype(np.float32)
    distance = distance_transform_edt(~hard)
    result = (0.5 * (1 + np.cos(np.pi * np.minimum(distance / radius, 1)))).astype(np.float32)
    result[hard] = 1
    return np.asarray(result, dtype=np.float32)


def fixed_masks(settings: FinishSettings, size: tuple[int, int]) -> tuple[Mask, Weights, Mask]:
    guards = polygon_mask(settings.moving_guards, size)
    hard = polygon_mask(settings.fixed_regions, size) & ~guards
    weights = feather_mask(hard, settings.outside_feather_pixels)
    weights[guards] = 0
    return hard, weights, guards


def anchor_rgba(current: Pixels, canonical: Pixels, weights: Weights) -> Pixels:
    """Replace hard pixels exactly; blend the exterior feather in premultiplied RGBA."""
    if np.array_equal(current, canonical):
        return current.copy()
    result = current.copy()
    hard = weights == 1
    result[hard] = canonical[hard]
    soft = (weights > 0) & (weights < 1)
    if soft.any():
        left, right = current[soft].astype(np.float32), canonical[soft].astype(np.float32)
        alpha_left, alpha_right = left[:, 3:] / 255, right[:, 3:] / 255
        weight = weights[soft, None]
        alpha = alpha_left * (1 - weight) + alpha_right * weight
        premultiplied = (
            left[:, :3] * alpha_left * (1 - weight) + right[:, :3] * alpha_right * weight
        )
        rgb = np.divide(premultiplied, alpha, out=np.zeros_like(premultiplied), where=alpha > 0)
        result[soft] = (
            np.rint(np.concatenate((rgb, alpha * 255), axis=1)).clip(0, 255).astype(np.uint8)
        )
    return result


def _cv() -> Any:
    try:
        return importlib.import_module("cv2")
    except ImportError as error:
        raise RuntimeError("Optical-flow finishing requires opencv-python-headless") from error


def _premultiply(frame: Pixels) -> Weights:
    alpha = frame[..., 3:4].astype(np.float32) / 255
    return np.concatenate((frame[..., :3] * alpha, alpha * 255), axis=2)


def _unpremultiply(frame: Weights) -> Pixels:
    alpha = frame[..., 3:4] / 255
    rgb = np.divide(frame[..., :3], alpha, out=np.zeros_like(frame[..., :3]), where=alpha > 0)
    result = np.rint(np.concatenate((rgb, frame[..., 3:4]), axis=2)).clip(0, 255).astype(np.uint8)
    result[result[..., 3] == 0, :3] = 0
    return result


def morph(
    current: Pixels,
    canonical: Pixels,
    amount: float,
    *,
    proxy_width: int = 720,
    maximum_displacement: float = 36,
) -> tuple[Pixels, float]:
    """Bidirectional DIS morph; refuse motion beyond the caller's admitted bound."""
    if (
        current.shape != canonical.shape
        or current.ndim != 3
        or current.shape[2] != 4
        or current.dtype != np.uint8
        or canonical.dtype != np.uint8
        or not 0 <= amount <= 1
    ):
        raise ValueError("Matching uint8 RGBA frames and a finite morph amount in 0..1 required")
    if amount == 0 or np.array_equal(current, canonical):
        return current.copy(), 0
    if amount == 1:
        return canonical.copy(), 0
    cv = _cv()
    height, width = current.shape[:2]
    proxy_width = min(width, proxy_width)
    proxy_height = round(height * proxy_width / width)
    if min(proxy_width, proxy_height) < 24:
        raise ValueError("Optical-flow proxy dimensions must be at least 24 pixels")
    size = (proxy_width, proxy_height)

    def prepare(frame: Pixels) -> tuple[Weights, Pixels]:
        premultiplied = _premultiply(frame)
        alpha = frame[..., 3:4].astype(np.float32) / 255
        over = premultiplied[..., :3] + np.array([22, 27, 34]) * (1 - alpha)
        small = cv.resize(np.rint(over).astype(np.uint8), size, interpolation=cv.INTER_AREA)
        return premultiplied, cv.cvtColor(small, cv.COLOR_RGB2GRAY)

    left, gray_left = prepare(current)
    right, gray_right = prepare(canonical)
    forward = cv.DISOpticalFlow_create(cv.DISOPTICAL_FLOW_PRESET_MEDIUM).calc(
        gray_left, gray_right, None
    )
    backward = cv.DISOpticalFlow_create(cv.DISOPTICAL_FLOW_PRESET_MEDIUM).calc(
        gray_right, gray_left, None
    )
    maximum = max(float(np.linalg.norm(flow, axis=2).max()) for flow in (forward, backward))
    if not np.isfinite(maximum) or maximum > maximum_displacement:
        raise ValueError("Endpoint flow exceeds the admitted small-motion displacement")
    y, x = np.mgrid[:height, :width].astype(np.float32)

    def warp(frame: Weights, flow: Weights, fraction: float) -> Weights:
        flow = cv.resize(flow, (width, height), interpolation=cv.INTER_LINEAR)
        flow[..., 0] *= width / proxy_width
        flow[..., 1] *= height / proxy_height
        return np.asarray(
            cv.remap(
                frame,
                x - fraction * flow[..., 0],
                y - fraction * flow[..., 1],
                interpolation=cv.INTER_LINEAR,
                borderMode=cv.BORDER_CONSTANT,
                borderValue=0,
            ),
            dtype=np.float32,
        )

    moved_left = warp(left, forward, amount)
    moved_right = warp(right, backward, 1 - amount)
    return _unpremultiply(moved_left * (1 - amount) + moved_right * amount), maximum


class AttachmentRepair:
    """Prevalidated native masks for one local opaque attachment correction."""

    def __init__(
        self, config: LocalRepair, canonical: Pixels, guards: Mask, earlier_fields: Mask
    ) -> None:
        self.config = config
        height, width = canonical.shape[:2]
        self.hard = polygon_mask([config], (width, height))
        self.taper = feather_mask(self.hard, config.falloff_pixels)
        self.pin = feather_mask(self.hard, config.pin_feather_pixels)
        self.field = self.taper > 0
        x0, y0, x1, y1 = config.roi_xyxy
        bounded = np.zeros((height, width), dtype=bool)
        bounded[y0:y1, x0:x1] = True
        if np.any(self.field & ~bounded):
            raise ValueError("Local repair ROI must enclose the entire polygon and falloff")
        if np.any(self.field & (guards | earlier_fields)):
            raise ValueError("Local repair must exclude moving guards and other repair fields")
        if not np.all(canonical[self.field, 3] == 255):
            raise ValueError("Local repair requires an opaque canonical alignment field")

    def apply(
        self,
        before: Pixels,
        canonical: Pixels,
        fixed_hard: Mask,
        fixed_weight: Weights,
        raw_frame: Pixels | None,
    ) -> tuple[Pixels, float]:
        if np.array_equal(before, canonical):
            return before.copy(), 0
        cv = _cv()
        x0, y0, x1, y1 = self.config.roi_xyxy
        current = (before if raw_frame is None else raw_frame)[y0:y1, x0:x1].copy()
        target = canonical[y0:y1, x0:x1]
        weights = self.taper[y0:y1, x0:x1]
        local_field = weights > 0
        nonopaque = local_field & (current[..., 3] < 255)
        protected = fixed_hard[y0:y1, x0:x1]
        if np.any(nonopaque & ~protected) or (nonopaque.any() and raw_frame is None):
            raise ValueError("Local alignment encountered nonopaque unprotected source pixels")
        current[nonopaque] = before[y0:y1, x0:x1][nonopaque]
        if not np.all(current[local_field, 3] == 255):
            raise ValueError("Protected alignment input restoration did not preserve opacity")
        left, right = _premultiply(current), _premultiply(target)
        gray_target = cv.cvtColor(np.rint(right[..., :3]).astype(np.uint8), cv.COLOR_RGB2GRAY)
        gray_current = cv.cvtColor(np.rint(left[..., :3]).astype(np.uint8), cv.COLOR_RGB2GRAY)
        backward = cv.DISOpticalFlow_create(cv.DISOPTICAL_FLOW_PRESET_MEDIUM).calc(
            gray_target, gray_current, None
        )
        displacement = backward * weights[..., None]
        maximum = float(np.linalg.norm(displacement, axis=2).max())
        if not np.isfinite(maximum) or maximum > self.config.max_displacement_pixels:
            raise ValueError("Local alignment exceeds its admitted native displacement")
        yy, xx = np.mgrid[: current.shape[0], : current.shape[1]].astype(np.float32)
        warped = cv.remap(
            left,
            xx + displacement[..., 0],
            yy + displacement[..., 1],
            interpolation=cv.INTER_LINEAR,
            borderMode=cv.BORDER_CONSTANT,
            borderValue=0,
        )
        aligned = before.copy()
        aligned[y0:y1, x0:x1][local_field] = _unpremultiply(warped)[local_field]
        result = anchor_rgba(aligned, canonical, self.pin)
        if raw_frame is not None:
            restored = anchor_rgba(result, canonical, fixed_weight)
            result[self.field] = restored[self.field]
        result[fixed_hard] = before[fixed_hard]
        if not np.array_equal(result[..., 3], before[..., 3]):
            raise ValueError("Local attachment alignment changed alpha")
        if not np.array_equal(result[~self.field], before[~self.field]):
            raise ValueError("Local attachment alignment changed its exterior")
        return result, maximum
