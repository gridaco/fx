"""Structured geometry tools available to the contained character agent."""

from __future__ import annotations

import base64
import json
import math
from collections.abc import Awaitable, Callable, Iterable, Mapping, Sequence
from pathlib import Path
from typing import Any

import jsonschema

from _character_tools.io import confined, digest, write_json
from _character_tools.tool_views import render_view
from _character_tools.worker_client import WorkerClient
from grida.fx import Tool, ToolInvocationError, ToolResult

Record = dict[str, Any]
ToolMethod = Callable[[Record], Awaitable[ToolResult]]
NUMBER = {"type": "number"}
STRING = {"type": "string"}
VECTOR = {"type": "array", "items": NUMBER, "minItems": 3, "maxItems": 3}


def object_schema(properties: Record) -> Record:
    return {
        "type": "object",
        "properties": properties,
        "required": list(properties),
        "additionalProperties": False,
    }


def nullable(schema: Record) -> Record:
    return {"anyOf": [schema, {"type": "null"}]}


POSE = nullable(object_schema({"clip": STRING, "time_seconds": NUMBER, "fps": {"type": "integer"}}))
BOUNDS = nullable(object_schema({"min": VECTOR, "max": VECTOR}))
VIEWS: dict[str, str | Record] = {
    "front": "positive_z",
    "back": "negative_z",
    "right": "positive_x",
    "left": "negative_x",
    "top": "positive_y",
    "bottom": "negative_y",
    "three_quarter": {"name": "three_quarter", "direction": [1, 0.35, 1.5], "up": [0, 1, 0]},
}


def data_url(path: Path) -> str:
    value = path.read_bytes()
    if not value.startswith(b"\x89PNG\r\n\x1a\n"):
        raise ValueError("Only measured PNG renders can be sent as tool images")
    return "data:image/png;base64," + base64.b64encode(value).decode("ascii")


def rigid_matrix(
    scale: float, rotation: Sequence[float], translation: Sequence[float]
) -> list[list[float]]:
    values = [scale, *rotation, *translation]
    if (
        any(type(value) not in {int, float} or not math.isfinite(value) for value in values)
        or not 1e-06 <= scale <= 1000000.0
    ):
        raise ValueError("Transform values must be finite with positive bounded uniform scale")
    x, y, z = [math.radians(angle) for angle in rotation]
    cx, sx, cy, sy, cz, sz = (
        math.cos(x),
        math.sin(x),
        math.cos(y),
        math.sin(y),
        math.cos(z),
        math.sin(z),
    )
    rows = [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
    return [
        [value * scale for value in row] + [translation[index]] for index, row in enumerate(rows)
    ] + [[0, 0, 0, 1]]


class Studio:
    def __init__(
        self,
        worker: WorkerClient,
        *,
        parts: dict[str, Record],
        profile: Record,
        max_revisions: int = 6,
    ) -> None:
        self.worker = worker
        self.parts = parts
        self.profile = profile
        if type(max_revisions) is not int or not 1 <= max_revisions <= 32:
            raise ValueError("Revision limit must be an integer from 1 through 32")
        self.assets = dict(parts)
        self.max_revisions = max_revisions
        self.revisions: list[str] = []
        self.observations: list[Record] = []
        self.counter = 0
        self.frozen = False

    def next_dir(self, operation: str) -> str:
        self.counter += 1
        return f"observations/{operation}-{self.counter:04d}"

    def asset(self, asset_id: str | None) -> Record:
        if asset_id not in self.assets:
            raise ValueError("Unknown asset_id; available: " + ", ".join(self.assets))
        source = self.assets[asset_id]["source"]
        path = confined(self.worker.input_root, source["path"])
        if digest(path) != source["sha256"]:
            raise ValueError("Registered artifact changed since it was frozen")
        return self.assets[asset_id]

    async def inspect_asset(self, args: Record) -> ToolResult:
        asset = self.asset(args["asset_id"])
        options: Record = {"component_limit": 12, "save_geometry": True}
        for incoming, outgoing in (
            ("nearest_points", "nearest_points"),
            ("slice_planes", "slice_planes"),
        ):
            options[outgoing] = [
                {key: value for key, value in query.items() if value is not None}
                for query in args[incoming]
            ]
        if args["pose"] is not None:
            options["pose"] = args["pose"]
        report, directory = await self.worker.execute(
            {
                "schema_version": 1,
                "operation": "inspect",
                "source": asset["source"],
                "output_dir": self.next_dir("inspect"),
                "options": options,
            }
        )
        result = report["result"]
        result["images"] = [
            {key: image[key] for key in ("name", "width", "height", "packed", "packed_sha256")}
            for image in result["images"]
        ]
        return ToolResult(
            json.dumps(
                {
                    "asset_id": args["asset_id"],
                    "source_sha256": asset["source"]["sha256"],
                    "report": directory + "/report.json",
                    "measurement": result,
                }
            )
        )

    async def render(
        self,
        asset_id: str,
        *,
        views: Iterable[str],
        focus: Record | None = None,
        hide: Iterable[str] = (),
        pose: Record | None = None,
        material_mode: str = "native",
        character_height_pixels: int | None = None,
    ) -> tuple[Record, tuple[str, ...]]:
        asset = self.asset(asset_id)
        options: Record = {
            "views": [VIEWS[view] for view in views],
            "resolution": [512, 512],
            "samples": 12,
            "hide_meshes": list(hide),
            "material_mode": material_mode,
        }
        if character_height_pixels is not None:
            if (
                type(character_height_pixels) is not int
                or not 64 <= character_height_pixels <= 1024
            ):
                raise ValueError("Character review height must be from 64 through 1024 pixels")
            resolution = max(512, math.ceil(character_height_pixels * 1.25))
            options["resolution"] = [resolution, resolution]
            options["character_height_pixels"] = character_height_pixels
        if focus is not None:
            options["focus_bounds"] = focus
        if pose is not None:
            options["pose"] = pose
        report, directory = await self.worker.execute(
            {
                "schema_version": 1,
                "operation": "render",
                "source": asset["source"],
                "output_dir": self.next_dir("render"),
                "options": options,
            }
        )
        rendered = report["result"]["images"]
        expected = [view if isinstance(view, str) else view["name"] for view in options["views"]]
        if len(rendered) != len(expected) or {item["name"] for item in rendered} != set(expected):
            raise ValueError("Worker did not produce every required view exactly once")
        if character_height_pixels is not None:
            for item in rendered:
                projection = item.get("projected_geometry", {})
                measured = projection.get("height_pixels")
                if (
                    not isinstance(measured, int | float)
                    or not math.isfinite(measured)
                    or abs(measured - character_height_pixels) > 1
                    or (projection.get("fully_in_frame") is not True)
                    or (item.get("requested_character_height_pixels") != character_height_pixels)
                ):
                    raise ValueError("Worker did not prove the required projected character size")
        images = []
        evidence = []
        for item in report["result"]["images"]:
            ref = directory + "/" + item["image"]
            path = confined(self.worker.run_root, ref)
            images.append(data_url(path))
            evidence.append(
                {"path": ref, "sha256": digest(path), "view": item["name"], "camera": item}
            )
        record = {
            "asset_id": asset_id,
            "source_sha256": asset["source"]["sha256"],
            "report": directory + "/report.json",
            "pose": report["pose"],
            "bounds": report["result"]["framing_bounds"],
            "images": evidence,
            "character_height_pixels": character_height_pixels,
        }
        self.observations.append(record)
        return (record, tuple(images))

    async def render_asset(self, args: Record) -> ToolResult:
        record, images = await self.render(
            args["asset_id"],
            views=args["views"],
            focus=args["focus_bounds"],
            hide=args["hide_meshes"],
            pose=args["pose"],
            material_mode=args["material_mode"],
        )
        report = json.loads(confined(self.worker.run_root, record["report"]).read_text())
        return ToolResult(
            json.dumps(render_view(record, report=report, request=args), separators=(",", ":")),
            images,
        )

    async def build_assembly(self, args: Record) -> ToolResult:
        if self.frozen:
            raise ValueError("Candidate is frozen; this episode cannot mutate it after submission")
        if len(self.revisions) >= self.max_revisions:
            raise ValueError("Assembly revision budget exhausted")
        entries = args["parts"]
        if len(entries) != len(self.parts) or {part["part_id"] for part in entries} != set(
            self.parts
        ):
            raise ValueError("Provide exactly one transform for every declared part")
        part_specs = []
        for part in entries:
            registered = self.asset(part["part_id"])
            part_specs.append(
                {
                    "part_id": part["part_id"],
                    "role": registered["role"],
                    "source": registered["source"],
                    "transform": rigid_matrix(
                        part["uniform_scale"], part["rotation_degrees_xyz"], part["translation"]
                    ),
                    "material_mode": "preserve",
                }
            )
        asset_id = f"assembly_{len(self.revisions) + 1:02d}"
        output_dir = "candidates/" + asset_id
        report, _ = await self.worker.execute(
            {
                "schema_version": 1,
                "operation": "assemble",
                "parts": part_specs,
                "output_dir": output_dir,
            },
            script="assemble.py",
        )
        path = self.worker.run_root / output_dir / "model.glb"
        entry = {
            "source": {
                "path": path.relative_to(self.worker.input_root).as_posix(),
                "sha256": digest(path),
            },
            "role": "assembly",
            "inventory": report["export"]["after_reimport"],
            "part_roles": report["part_roles"],
            "assembly_parameters": entries,
        }
        self.assets[asset_id] = entry
        self.revisions.append(asset_id)
        record, images = await self.render(
            asset_id, views=["front", "right", "back", "three_quarter"]
        )
        return ToolResult(
            json.dumps(
                {
                    "asset_id": asset_id,
                    "inventory": entry["inventory"],
                    "render": record,
                    "seam_policy": report["seams"],
                }
            ),
            images,
        )

    def tools(self, *, read_only: bool = False) -> tuple[Tool, ...]:
        point = object_schema({"name": STRING, "point": VECTOR, "mesh": nullable(STRING)})
        plane = object_schema(
            {"name": STRING, "point": VECTOR, "normal": VECTOR, "mesh": nullable(STRING)}
        )
        declarations: list[tuple[str, str, Record, ToolMethod]] = [
            (
                "inspect_asset",
                (
                    "Measure bounds, topology, joints, nearest surface points and p"
                    "lane slices in glTF coordinates. Mesh names are exact; null me"
                    "ans all meshes."
                ),
                object_schema(
                    {
                        "asset_id": STRING,
                        "nearest_points": {"type": "array", "items": point, "maxItems": 32},
                        "slice_planes": {"type": "array", "items": plane, "maxItems": 12},
                        "pose": POSE,
                    }
                ),
                self.inspect_asset,
            ),
            (
                "render_asset",
                (
                    "Render registered artifact with numeric cameras. front means c"
                    "amera on +Z; these names become anatomical only after orientat"
                    "ion. null focus frames visible model. Hide exact mesh names to"
                    " inspect concealed seams. matte_policy previews the declared m"
                    "atte export finish; native shows raw provider materials that a"
                    "re not shipped."
                ),
                object_schema(
                    {
                        "asset_id": STRING,
                        "views": {
                            "type": "array",
                            "items": {"enum": list(VIEWS), "type": "string"},
                            "minItems": 1,
                            "maxItems": 6,
                        },
                        "focus_bounds": BOUNDS,
                        "hide_meshes": {"type": "array", "items": STRING},
                        "pose": POSE,
                        "material_mode": {
                            "type": "string",
                            "enum": ["native", "clay_diagnostic", "matte_policy"],
                        },
                    }
                ),
                self.render_asset,
            ),
        ]
        if not read_only:
            declarations.append(
                (
                    "build_assembly",
                    (
                        "Build a fresh candidate from ALL original normalized parts, ne"
                        "ver cumulative transforms. Public coordinates are glTF X right"
                        "/Y up/Z front. Transform point = translation + scale*(Rz@Ry@Rx"
                        ")*point, angles degrees. Body feet should sit at Y=0, whole ch"
                        "aracter near target height. Returns exact exported renders for"
                        " review."
                    ),
                    object_schema(
                        {
                            "parts": {
                                "type": "array",
                                "items": object_schema(
                                    {
                                        "part_id": STRING,
                                        "uniform_scale": NUMBER,
                                        "rotation_degrees_xyz": VECTOR,
                                        "translation": VECTOR,
                                    }
                                ),
                                "minItems": 1,
                                "maxItems": 16,
                            }
                        }
                    ),
                    self.build_assembly,
                )
            )
        tools = []
        for name, description, schema, handler in declarations:

            async def invoke(
                args: Mapping[str, object],
                *,
                schema: Record = schema,
                handler: ToolMethod = handler,
            ) -> ToolResult:
                try:
                    jsonschema.validate(dict(args), schema)
                    return await handler(dict(args))
                except (ValueError, jsonschema.ValidationError) as error:
                    raise ToolInvocationError(str(error)[:3000]) from None

            tools.append(Tool(name, description, schema, handler=invoke))
        return tuple(tools)

    def persist(self, *, output_root: Path | None = None) -> None:
        write_json(
            (output_root or self.worker.run_root) / "studio.json",
            {"assets": self.assets, "revisions": self.revisions, "observations": self.observations},
        )
