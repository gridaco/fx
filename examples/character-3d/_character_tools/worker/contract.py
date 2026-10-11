"""Provider-free worker request validation and confined artifact persistence."""

from __future__ import annotations

import hashlib
import json
import math
import re
from pathlib import Path
from typing import Any

VERSION = 1
AXES = {
    "positive_x": (1, 0, 0),
    "negative_x": (-1, 0, 0),
    "positive_y": (0, 1, 0),
    "negative_y": (0, -1, 0),
    "positive_z": (0, 0, 1),
    "negative_z": (0, 0, -1),
}
COORDINATES = {
    "public": "gltf_right_handed_x_right_y_up_z_nominal_front",
    "anatomical_front": "unverified_until_explicitly_oriented",
    "gltf_to_blender": [[1, 0, 0, 0], [0, 0, -1, 0], [0, 1, 0, 0], [0, 0, 0, 1]],
    "blender_to_gltf": [[1, 0, 0, 0], [0, 0, 1, 0], [0, -1, 0, 0], [0, 0, 0, 1]],
    "length_unit": (
        "scene_unit_after_format_importer_unit_conversion_no_worker_height_normalization"
    ),
}
PRESERVATION_REFUSALS = {
    "correspondence": (
        "No verified axis-aligned rigid/uniform-scale correspondence; p"
        "rovide a measured transform or reject preservation"
    ),
    "geometry_uv": "Provider geometry/UV differs beyond the correspondence gate",
    "p95": "Provider correspondence p95 exceeds 0.01mm",
    "connectivity": "Provider triangle connectivity or winding differs",
    "ambiguous_normals": ("Ambiguous original normals without provider normals cannot be restored"),
}


def digest(path: str | Path) -> str:
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def identifier(value: object) -> str:
    if not isinstance(value, str) or not re.fullmatch("[A-Za-z0-9][A-Za-z0-9_-]{0,79}", value):
        raise ValueError("Names must be plain identifiers up to 80 characters")
    return value


def finite(value: object) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)) or (not math.isfinite(value)):
        raise ValueError("Expected a finite number")
    return float(value)


def vector(value: object, size: int = 3) -> list[float]:
    if not isinstance(value, list) or len(value) != size:
        raise ValueError(f"Expected a vector with {size} values")
    return [finite(x) for x in value]


def relative(value: object) -> Path:
    if not isinstance(value, str) or not value or "\\" in value or ("\x00" in value):
        raise ValueError("Expected a portable relative path")
    path = Path(value)
    if (
        path.is_absolute()
        or ":" in value.split("/")[0]
        or any(part in {"..", ".", ""} for part in value.split("/"))
    ):
        raise ValueError("Absolute paths and traversal are refused")
    return path


def confined(root: str | Path, value: object, *, exists: bool = False) -> Path:
    root = Path(root).resolve(strict=True)
    path = root / relative(value)
    current = root
    for part in relative(value).parts:
        current /= part
        if current.is_symlink():
            raise ValueError("Symlinks are refused inside declared roots")
    if not path.resolve().is_relative_to(root):
        raise ValueError("Path escapes its declared root")
    if exists and (not path.is_file()):
        raise ValueError("Declared input is missing or not a regular file")
    return path


def write_json(path: str | Path, data: object) -> None:
    path = Path(path)
    if path.exists() or path.is_symlink():
        raise ValueError("Artifact exists; use a fresh operation directory")
    payload = json.dumps(data, indent=2, allow_nan=False).encode() + b"\n"
    with path.open("xb") as stream:
        stream.write(payload)


def validate_request(request: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(request, dict) or set(request) != {
        "schema_version",
        "operation",
        "source",
        "output_dir",
        "options",
    }:
        raise ValueError(
            "Request requires exactly schema_version, operation, source, output_dir and options"
        )
    if request["schema_version"] != VERSION or isinstance(request["schema_version"], bool):
        raise ValueError("Unsupported worker schema version")
    operation = request["operation"]
    if operation not in {"inspect", "normalize", "render"}:
        raise ValueError("Unsupported worker operation")
    source = request["source"]
    if not isinstance(source, dict) or set(source) != {"path", "sha256"}:
        raise ValueError("Source needs a portable path and SHA-256")
    relative(source["path"])
    if not isinstance(source.get("sha256"), str) or not re.fullmatch(
        "[a-f0-9]{64}", source["sha256"]
    ):
        raise ValueError("Expected lowercase source SHA-256")
    relative(request["output_dir"])
    opts = request["options"]
    allowed = {
        "inspect": {
            "pose",
            "weld_tolerance",
            "component_limit",
            "slice_planes",
            "nearest_points",
            "save_geometry",
        },
        "normalize": {"transform", "require_textures"},
        "render": {
            "pose",
            "views",
            "resolution",
            "samples",
            "material_mode",
            "focus_bounds",
            "ground_height",
            "hide_meshes",
            "margin",
            "character_height_pixels",
        },
    }[operation]
    if not isinstance(opts, dict) or set(opts) - allowed:
        raise ValueError("Operation has unknown options")
    if "pose" in opts:
        pose = opts["pose"]
        if not isinstance(pose, dict) or set(pose) != {"clip", "time_seconds", "fps"}:
            raise ValueError("Pose requires exact clip, time_seconds and fps")
        if not isinstance(pose["clip"], str) or not pose["clip"]:
            raise ValueError("Pose clip must be a nonempty name")
        if (
            finite(pose["time_seconds"]) < 0
            or type(pose["fps"]) is not int
            or (not 1 <= pose["fps"] <= 240)
        ):
            raise ValueError("Invalid pose time or FPS")
    if operation == "inspect":
        if "weld_tolerance" in opts and finite(opts["weld_tolerance"]) < 0:
            raise ValueError("Weld tolerance must be nonnegative")
        if (
            type(opts.get("component_limit", 20)) is not int
            or not 1 <= opts.get("component_limit", 20) <= 100
        ):
            raise ValueError("Component summary limit must be 1 through 100")
        if type(opts.get("save_geometry", True)) is not bool:
            raise ValueError("save_geometry must be boolean")
        for key, maximum in (("slice_planes", 32), ("nearest_points", 128)):
            values = opts.get(key, [])
            if not isinstance(values, list) or len(values) > maximum:
                raise ValueError("Geometry query count exceeds its bound")
            names = set()
            for query in values:
                fields = (
                    {"name", "point", "normal", "mesh"}
                    if key == "slice_planes"
                    else {"name", "point", "mesh"}
                )
                required = fields - {"mesh"}
                if (
                    not isinstance(query, dict)
                    or not required <= query.keys()
                    or query.keys() - fields
                ):
                    raise ValueError("Invalid geometry query fields")
                name = identifier(query["name"])
                if name in names:
                    raise ValueError("Query names must be unique")
                names.add(name)
                vector(query["point"])
                if "normal" in query and sum(x * x for x in vector(query["normal"])) <= 1e-20:
                    raise ValueError("Plane normal cannot be zero")
                if "mesh" in query and (not isinstance(query["mesh"], str)):
                    raise ValueError("Mesh selector must be an exact name")
    elif operation == "normalize":
        if type(opts.get("require_textures", False)) is not bool:
            raise ValueError("require_textures must be boolean")
        if "transform" in opts:
            if not isinstance(opts["transform"], list) or len(opts["transform"]) != 4:
                raise ValueError("Transform must be a row-major 4x4 matrix")
            for row in opts["transform"]:
                vector(row, 4)
    else:
        resolution = opts.get("resolution", [512, 512])
        if (
            not isinstance(resolution, list)
            or len(resolution) != 2
            or any(type(x) is not int or not 64 <= x <= 2048 for x in resolution)
        ):
            raise ValueError("Render dimensions must be integers from 64 through 2048")
        if "character_height_pixels" in opts:
            target = opts["character_height_pixels"]
            if type(target) is not int or not 64 <= target <= min(1024, resolution[1]):
                raise ValueError(
                    "Character height must be an integer from 64 through 1024 withi"
                    "n the canvas height"
                )
            if "focus_bounds" in opts:
                raise ValueError("Character pixel height and focus_bounds cannot be combined")
        if type(opts.get("samples", 16)) is not int or not 1 <= opts.get("samples", 16) <= 128:
            raise ValueError("Render samples must be 1 through 128")
        if opts.get("material_mode", "native") not in {"native", "clay_diagnostic", "matte_policy"}:
            raise ValueError("Unsupported render material mode")
        if opts.get("ground_height") is not None:
            finite(opts["ground_height"])
        if not 1.01 <= finite(opts.get("margin", 1.15)) <= 2:
            raise ValueError("Framing margin must be from 1.01 through 2")
        if "focus_bounds" in opts:
            bounds = opts["focus_bounds"]
            if not isinstance(bounds, dict) or set(bounds) != {"min", "max"}:
                raise ValueError("Focus bounds need min and max")
            lo, hi = (vector(bounds["min"]), vector(bounds["max"]))
            if any((a >= b for a, b in zip(lo, hi, strict=True))):
                raise ValueError("Focus bounds must have positive dimensions")
        hidden = opts.get("hide_meshes", [])
        if (
            not isinstance(hidden, list)
            or any(not isinstance(x, str) for x in hidden)
            or len(hidden) != len(set(hidden))
        ):
            raise ValueError("hide_meshes must contain unique exact mesh names")
        views = opts.get("views", list(AXES))
        if not isinstance(views, list) or not 1 <= len(views) <= 16:
            raise ValueError("Supply 1 through 16 views")
        names = set()
        for view in views:
            if isinstance(view, str):
                if view not in AXES:
                    raise ValueError("Unknown numeric axis view")
                name = view
            else:
                if not isinstance(view, dict) or set(view) != {"name", "direction", "up"}:
                    raise ValueError("Custom view needs name, direction and up")
                name = identifier(view["name"])
                vector(view["direction"])
                vector(view["up"])
            if name in names:
                raise ValueError("View names must be unique")
            names.add(name)
    return request
