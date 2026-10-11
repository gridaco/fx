"""Agent-authored rig plans compiled through a replaceable articulation profile."""

from __future__ import annotations

import json
from collections.abc import Mapping
from copy import deepcopy
from pathlib import Path
from typing import Any

import jsonschema

from _character_tools.io import (
    canonical_digest,
    confined,
    digest,
    read_json,
    write_json,
)
from _character_tools.profiles import articulation
from _character_tools.rig_recipes import (
    PRESERVATION,
    recipe_inputs,
    revise_plan,
)
from _character_tools.studio import (
    NUMBER,
    STRING,
    VECTOR,
    Studio,
    ToolMethod,
    nullable,
    object_schema,
)
from _character_tools.tool_views import rig_build_view
from _character_tools.worker_client import WorkerClient
from grida.fx import Tool, ToolInvocationError, ToolResult

Record = dict[str, Any]


class RigStudio(Studio):
    def __init__(
        self,
        worker: WorkerClient,
        *,
        parts: dict[str, Record],
        profile: Record,
        max_revisions: int = 6,
        max_rig_revisions: int = 6,
    ) -> None:
        super().__init__(worker, parts=parts, profile=profile, max_revisions=max_revisions)
        if type(max_rig_revisions) is not int or not 1 <= max_rig_revisions <= 32:
            raise ValueError("Rig revision limit must be from 1 through 32")
        self.max_rig_revisions = max_rig_revisions
        self.rig_revisions: list[str] = []
        self.rig_frozen = False
        self.admitted_assembly: str | None = None

    def persist(self, *, output_root: Path | None = None) -> None:
        super().persist(output_root=output_root)
        write_json(
            (output_root or self.worker.run_root) / "rig_studio.json",
            {
                "rig_revisions": self.rig_revisions,
                "admitted_assembly": self.admitted_assembly,
                "rig_frozen": self.rig_frozen,
            },
        )

    async def build_rig(self, args: Record) -> ToolResult:
        source = self._assembly_source(args["source_asset_id"])
        plan = articulation(self.profile).compile_plan(
            source, args["landmarks"], args["binding_regions"], self.profile
        )
        return await self._build_plan(args["source_asset_id"], source, plan)

    def _assembly_source(self, asset_id: str | None) -> Record:
        if self.rig_frozen:
            raise ValueError("Rig candidate is frozen after submission")
        if len(self.rig_revisions) >= self.max_rig_revisions:
            raise ValueError("Rig revision budget exhausted")
        if asset_id != self.admitted_assembly:
            raise ValueError("Rigging requires the admitted assembly, without replacing it")
        return self.asset(asset_id)

    def _registered_recipe(self, asset_id: str) -> tuple[Record, Record]:
        if asset_id not in self.rig_revisions:
            raise ValueError("Recipe inspection requires a registered rig candidate")
        asset = self.asset(asset_id)
        if asset.get("assembly_asset_id") != self.admitted_assembly or not asset.get("rig_ready"):
            raise ValueError("Recipe must belong to a completed rig of the admitted assembly")
        plan = asset["rig_plan"]
        path = confined(self.worker.run_root, "candidates/" + asset_id + "/plan.json")
        if read_json(path) != plan or asset["rig_report"].get("plan_sha256") != canonical_digest(
            plan
        ):
            raise ValueError("Registered recipe differs from its persisted worker evidence")
        assembly = self.asset(self.admitted_assembly)
        if plan["source_sha256"] != assembly["source"]["sha256"]:
            raise ValueError("Recipe source differs from the admitted assembly")
        return (asset, deepcopy(plan))

    async def inspect_rig_plan(self, args: Record) -> ToolResult:
        asset, plan = self._registered_recipe(args["asset_id"])
        landmarks, regions = recipe_inputs(plan)
        regions = [
            {"fade_width": 0, "whole_components": False, "blend_axis": None, **item}
            for item in regions
        ]
        return ToolResult(
            json.dumps(
                {
                    "asset_id": args["asset_id"],
                    "source_sha256": asset["source"]["sha256"],
                    "plan_sha256": canonical_digest(plan),
                    "plan": plan,
                    "editable_landmarks": landmarks,
                    "editable_regions": regions,
                    "preservation_scope": PRESERVATION,
                },
                separators=(",", ":"),
            )
        )

    async def revise_rig_plan(self, args: Record) -> ToolResult:
        source = self._assembly_source(self.admitted_assembly)
        base, plan = self._registered_recipe(args["base_asset_id"])
        merged = revise_plan(
            plan, args, compiler=articulation(self.profile), source=source, profile=self.profile
        )
        lineage = {
            "schema_version": 1,
            "operation": "named_rig_recipe_revision",
            "base_asset_id": args["base_asset_id"],
            "base_source_sha256": base["source"]["sha256"],
            "base_plan_sha256": canonical_digest(plan),
            "patch": deepcopy(args),
            "result_plan_sha256": canonical_digest(merged),
            "result_plan": merged,
            "preservation_scope": PRESERVATION,
        }
        return await self._build_plan(self.admitted_assembly, source, merged, lineage=lineage)

    async def _build_plan(
        self,
        source_asset_id: str | None,
        source: Record,
        plan: Record,
        *,
        lineage: Record | None = None,
    ) -> ToolResult:
        asset_id = f"rig_{len(self.rig_revisions) + 1:02d}"
        self.rig_revisions.append(asset_id)
        directory = "candidates/" + asset_id
        if lineage is not None:
            write_json(
                confined(
                    self.worker.run_root, "rig_recipes/" + asset_id + ".json", must_exist=False
                ),
                lineage,
            )
        report, _ = await self.worker.execute(
            {
                "schema_version": 1,
                "operation": "rig",
                "source": source["source"],
                "plan": plan,
                "output_dir": directory,
            },
            script="rig_cli.py",
        )
        path = self.worker.run_root / directory / "animated.glb"
        entry: Record = {
            "source": {
                "path": path.relative_to(self.worker.input_root).as_posix(),
                "sha256": digest(path),
            },
            "role": "rig",
            "part_roles": source["part_roles"],
            "rig_plan": plan,
            "rig_report": report,
            "assembly_asset_id": source_asset_id,
        }
        if lineage is not None:
            entry["recipe_revision"] = lineage
        self.assets[asset_id] = entry
        inspection, _ = await self.worker.execute(
            {
                "schema_version": 1,
                "operation": "inspect",
                "source": entry["source"],
                "output_dir": self.next_dir("rig_inspect"),
                "options": {"component_limit": 8, "save_geometry": False},
            }
        )
        entry["inventory"] = inspection["result"]
        metrics, metrics_dir = await self.worker.execute(
            {
                "schema_version": 1,
                "operation": "rig_metrics",
                "source": entry["source"],
                "plan": plan,
                "output_dir": self.next_dir("rig_metrics"),
            },
            script="rig_metrics.py",
        )
        weighted = {
            name
            for name, counts in metrics.get("joints", {}).get("weighted_vertices", {}).items()
            if name in plan["bones"] and counts["above_epsilon"]
        }
        effective = set(weighted)
        for name in weighted:
            parent = plan["bones"][name]["parent"]
            while parent:
                effective.add(parent)
                parent = plan["bones"][parent]["parent"]
        entry["required_but_missing_weights"] = sorted(set(plan["bones"]) - effective)
        entry["metrics"] = metrics
        entry["metrics_report"] = metrics_dir + "/report.json"
        motion = self.profile["review"]["required_motion"]
        motion_clip = next(clip for clip in plan["clips"] if clip["name"] == motion)
        evidence, images = await self.render(
            asset_id,
            views=["front", "right", "three_quarter"],
            pose={
                "clip": motion,
                "time_seconds": motion_clip["duration_seconds"] * 0.375,
                "fps": motion_clip["fps"],
            },
        )
        entry["rig_ready"] = True
        render_report = json.loads(confined(self.worker.run_root, evidence["report"]).read_text())
        return ToolResult(
            json.dumps(
                rig_build_view(
                    {
                        "asset_id": asset_id,
                        "source_sha256": entry["source"]["sha256"],
                        "rig_report": report,
                        "rig_report_path": directory + "/report.json",
                        "required_but_missing_weights": entry["required_but_missing_weights"],
                        "numeric_findings": metrics["blocking_findings"],
                        "metrics_report": entry["metrics_report"],
                        **(
                            {
                                "recipe_revision": {
                                    key: value
                                    for key, value in lineage.items()
                                    if key != "result_plan"
                                }
                            }
                            if lineage is not None
                            else {}
                        ),
                        "available_clips": entry["inventory"]["clips"],
                        "render": evidence,
                        "next_check": (
                            "Inspect the profile's required diagnostics and palmward curl, "
                            "accessory attachment and rest; a successful export alone is in"
                            "sufficient."
                        ),
                    },
                    metrics=metrics,
                    render_report=render_report,
                ),
                separators=(",", ":"),
            ),
            images,
        )

    def rig_tools(self, *, read_only: bool = False) -> tuple[Tool, ...]:
        tools = list(super().tools(read_only=True))
        if read_only:
            return tuple(tools)
        compiler = articulation(self.profile)
        bone = object_schema(
            {
                "joint": {"type": "string", "enum": list(compiler.PARENTS)},
                "head": VECTOR,
                "tail": VECTOR,
                "palm_direction": nullable(VECTOR),
            }
        )
        region = object_schema(
            {
                "role": STRING,
                "name": STRING,
                "bounds": {"type": "array", "items": VECTOR, "minItems": 2, "maxItems": 2},
                "bones": {"type": "array", "items": STRING, "minItems": 1},
                "fade_width": NUMBER,
                "whole_components": {"type": "boolean"},
                "blend_axis": nullable(
                    object_schema(
                        {
                            "axis": {"type": "integer", "enum": [0, 1, 2]},
                            "start": NUMBER,
                            "end": NUMBER,
                            "reverse": {"type": "boolean"},
                        }
                    )
                ),
            }
        )
        schema = object_schema(
            {
                "source_asset_id": STRING,
                "landmarks": {
                    "type": "array",
                    "items": bone,
                    "minItems": len(compiler.PARENTS),
                    "maxItems": len(compiler.PARENTS),
                },
                "binding_regions": {"type": "array", "items": region, "maxItems": 64},
            }
        )

        async def invoke(args: Mapping[str, object], *, schema: Record = schema) -> ToolResult:
            try:
                jsonschema.validate(dict(args), schema)
                return await self.build_rig(dict(args))
            except (ValueError, jsonschema.ValidationError) as error:
                raise ToolInvocationError(str(error)[:4000]) from None

        tools.append(
            Tool(
                "build_rig",
                (
                    "Build a fresh rig from the admitted assembly and measured join"
                    "t positions. All coordinates glTF Y-up/Z-front. The profile su"
                    "pplies hierarchy and diagnostic motion; you supply every bone "
                    "head/tail and anatomically observed palm directions. Use heat "
                    "weights plus explicit regions to keep hands/limbs/accessories "
                    "from pulling unrelated vertices. Regions name entire disconnec"
                    "ted components only when whole_components=true; bounding-box s"
                    "elections with that option can affect whole garments. The work"
                    "er preserves source surfaces and textures, and applies the pro"
                    "file's matte/unlit finish. Every attempt consumes a revision."
                ),
                schema,
                handler=invoke,
            )
        )
        declarations: list[tuple[str, str, Record, ToolMethod]] = [
            (
                "inspect_rig_plan",
                (
                    "Read the exact registered rig recipe and its digest, including"
                    " landmarks and ordered named binding regions. This is not a fi"
                    "le-reading tool."
                ),
                object_schema({"asset_id": STRING}),
                self.inspect_rig_plan,
            ),
            (
                "revise_rig_plan",
                (
                    "Revise a registered recipe by exact base-plan digest. Replace "
                    "only listed complete landmark/region entries; update/removal t"
                    "argets must exist, additions must be new. Region identity is ("
                    "role, name); updates retain order and additions append. Empty "
                    "lists leave those entries untouched. "
                )
                + PRESERVATION,
                object_schema(
                    {
                        "base_asset_id": STRING,
                        "expected_plan_sha256": {"type": "string", "pattern": "^[a-f0-9]{64}$"},
                        "landmark_updates": {
                            "type": "array",
                            "items": bone,
                            "maxItems": len(compiler.PARENTS),
                        },
                        "region_updates": {"type": "array", "items": region, "maxItems": 64},
                        "region_additions": {"type": "array", "items": region, "maxItems": 64},
                        "region_removals": {
                            "type": "array",
                            "items": object_schema({"role": STRING, "name": STRING}),
                            "maxItems": 64,
                        },
                    }
                ),
                self.revise_rig_plan,
            ),
        ]
        for name, description, schema, method in declarations:

            async def invoke_recipe(
                args: Mapping[str, object], *, schema: Record = schema, method: ToolMethod = method
            ) -> ToolResult:
                try:
                    jsonschema.validate(dict(args), schema)
                    return await method(dict(args))
                except (ValueError, jsonschema.ValidationError) as error:
                    raise ToolInvocationError(str(error)[:4000]) from None

            tools.append(Tool(name, description, schema, handler=invoke_recipe))
        return tuple(tools)
