"""The example's immutable, attributed topology inputs."""

from pathlib import Path


def terrain_atlas_template_path() -> Path:
    return Path(__file__).parent / "resources" / "template.png"


def terrain_atlas_lookup_path() -> Path:
    return Path(__file__).parent / "resources" / "lookup.json"
