"""Stand-in for the game's own level reader (in the real repo: the game's installed package)."""

from __future__ import annotations

import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class Level:
    name: str
    light: str
    ground: dict[str, Any]
    pickups: list[dict[str, Any]]
    sky: dict[str, Any]


def read_level(path: Path) -> Level:
    raw = tomllib.loads(path.read_text(encoding="utf-8"))
    sky = dict(raw["sky"])
    # Sky layer files are written relative to the level file: join them to its folder.
    sky["layers"] = [{**layer, "file": path.parent / layer["file"]} for layer in sky["layers"]]
    return Level(raw["name"], raw["light"], raw["ground"], raw["pickups"], sky)
