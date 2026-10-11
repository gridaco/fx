"""The spec, its pictures, and the eyes-and-mouth combinations every step lists in one order.

Combinations run eyes outer, mouth inner, rest first in each group, then the spec's states
in their order; the still review is shown every one but rest--rest, after the source.
"""

from __future__ import annotations

import hashlib
import io
from itertools import product
from typing import Any

from PIL import Image

from nodes._portrait_motion import PortraitMotionSpec


def spec_of(value: Any) -> PortraitMotionSpec:
    return PortraitMotionSpec.model_validate(value)


def picture(data: bytes) -> Image.Image:
    with Image.open(io.BytesIO(data)) as opened:
        opened.load()
        return opened.convert("RGB")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def groups(spec: PortraitMotionSpec) -> dict[str, list[str]]:
    """Each feature group's states, rest first, in the spec's order."""

    found: dict[str, list[str]] = {"eyes": ["rest"], "mouth": ["rest"]}
    for state in spec.states:
        found[state.feature_group].append(state.state_id)
    return found


def combinations_of(spec: PortraitMotionSpec) -> list[tuple[str, str]]:
    """Every eyes-and-mouth pair, eyes outer: the order every record lists them in."""

    found = groups(spec)
    return list(product(found["eyes"], found["mouth"]))


def combination_key(eyes: str, mouth: str) -> str:
    return f"{eyes}--{mouth}"


def reviewed(spec: PortraitMotionSpec) -> list[tuple[str, str]]:
    """The combinations the still review is shown, after the source: all but rest--rest."""

    return [pair for pair in combinations_of(spec) if pair != ("rest", "rest")]
