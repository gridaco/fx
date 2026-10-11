"""What every character step shares: the profile, a Blender bench, agent episodes and the
review contract.

A step is a pure function of what it reads. It gets a bench of its own (folders under its
work directory and the hardened Blender worker client), registers the models it was given
there, and hands back the files it made. Agents run as FX agents: every turn is a paid,
call-cached call, so a resumed run replays the turns it already paid for.
"""

from __future__ import annotations

import hashlib
import json
import shutil
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import TYPE_CHECKING, Any

import jsonschema
from _character_tools.io import confined, digest
from _character_tools.partitions import apply_partition
from _character_tools.quality_bar import check_issue_heights
from _character_tools.requirements import validate_requirements
from _character_tools.studio import STRING, object_schema
from _character_tools.worker_client import WorkerClient

from grida.fx import Ctx

if TYPE_CHECKING:
    # Blender owns these imports at runtime; FX hashes their source while planning.
    from _character_tools.worker import assemble as assemble
    from _character_tools.worker import main as main
    from _character_tools.worker import provider_rig_cli as provider_rig_cli
    from _character_tools.worker import rig_cli as rig_cli
    from _character_tools.worker import rig_metrics as rig_metrics

Record = dict[str, Any]

#: Each profile's file under ``profiles/``, and the rig lane it is qualified for.
PROFILE_FILES = {
    "sd_human_fixed_hands_v1": "profiles/sd_human_fixed.json",
    "sd_human_grouped_hands_v1": "profiles/sd_human.json",
    "sd_cat_grouped_paws_v1": "profiles/sd_cat.json",
}
LANES = {
    "sd_human_fixed_hands_v1": "provider",
    "sd_human_grouped_hands_v1": "agent",
    "sd_cat_grouped_paws_v1": "agent",
}
REFERENCE_CRITERIA = [
    "original_identity",
    "cross_view_consistency",
    "part_coverage",
    "generation_readiness",
    "style_consistency",
]
PART_CRITERIA = [
    "recognizable_part",
    "reference_match",
    "view_coverage",
    "texture_integrity",
    "usable_topology",
]
PRODUCE_SCHEMA = object_schema(
    {"asset_id": STRING, "rationale": STRING, "open_issues": {"type": "array", "items": STRING}}
)
REVIEW_SCHEMA = object_schema(
    {
        "asset_id": STRING,
        "source_sha256": STRING,
        "accepted": {"type": "boolean"},
        "criteria": {
            "type": "array",
            "items": object_schema(
                {"criterion": STRING, "passed": {"type": "boolean"}, "evidence": STRING}
            ),
        },
        "issues": {
            "type": "array",
            "items": object_schema(
                {
                    "severity": {"type": "string", "enum": ["blocking", "minor"]},
                    "region": STRING,
                    "description": STRING,
                    "repair": STRING,
                    "smallest_visible_character_height_pixels": {"type": "integer"},
                }
            ),
        },
        "notes": STRING,
    }
)
REFERENCE_SUBMIT = object_schema(
    {
        "canonical_asset_id": STRING,
        "selections": {
            "type": "array",
            "minItems": 2,
            "maxItems": 48,
            "items": object_schema({"role": STRING, "view": STRING, "asset_id": STRING}),
        },
        "rationale": STRING,
        "open_issues": {"type": "array", "items": STRING},
    }
)
#: Findings that say an export is unreadable or incomplete, not how it deforms.
INTEGRITY_FINDINGS = frozenset(
    {
        "invalid_or_unsupported_glb",
        "ambiguous_joint_names",
        "required_joint_missing",
        "exported_joint_hierarchy_mismatch",
        "required_mesh_missing",
        "invalid_mesh",
        "required_mesh_unskinned",
        "invalid_skin_weights",
        "invalid_or_unsupported_surface",
        "no_evaluable_surfaces",
        "rest_bind_identity_mismatch",
        "duplicate_clip_name",
        "required_clip_missing",
        "required_animation_channel_missing",
        "animation_does_not_cover_plan_duration",
        "animation_evaluation_incomplete",
    }
)
APPEARANCE_LOST = "source_texture_appearance_or_binding_not_preserved"
#: The most a reply may run to.
TURN_TOKENS = 8192


def experiment(settings: Mapping[str, Any]) -> Record:
    """The settings the component's character rules read, under their own names."""

    return {
        "pipeline_mode": settings["pipeline_mode"],
        "review_mode": settings["review"],
        "review_quality_bar": settings["quality_bar"],
        **({"rigging": {"strategy": "provider"}} if settings.get("lane") == "provider" else {}),
        "parts": [{"role": role} for role in settings.get("roles", [])],
    }


def compile_profile(raw: Record, settings: Mapping[str, Any]) -> Record:
    """A profile with its partition applied, held to the component's requirements."""

    profile = dict(raw)
    if profile.get("profile_id") == "sd_human_fixed_hands_v1":
        profile = apply_partition(profile, settings["partition"])
    validate_requirements(profile, experiment(settings))
    return profile


# ---------------------------------------------------------------------------- the bench


def _bootstrap(module: str) -> str:
    """A Blender entry point beside the copied, project-owned worker package."""

    return (
        '"""Run one project-owned worker module inside Blender."""\n'
        "import runpy\nimport sys\nfrom pathlib import Path\n"
        "sys.path.insert(0, str(Path(__file__).resolve().parents[1]))\n"
        f"runpy.run_module({'_character_tools.worker.' + module!r}, "
        'run_name="__main__")\n'
    )


_ENTRY_POINTS = {
    "main.py": "main",
    "assemble.py": "assemble",
    "rig_cli.py": "rig_cli",
    "rig_metrics.py": "rig_metrics",
    "provider_rig_cli.py": "provider_rig_cli",
}


class Bench:
    """One step's Blender: its folders, the models it registered, the worker client."""

    def __init__(self, ctx: Ctx, *, max_calls: int = 200, timeout_seconds: float = 240) -> None:
        root = ctx.work_path("bench")
        (root / "run").mkdir(parents=True, exist_ok=True)
        package = root / "code" / "_character_tools"
        shutil.copytree(
            Path(__file__).parent / "_character_tools",
            package,
            ignore=shutil.ignore_patterns("__pycache__", "*.pyc"),
        )
        code = root / "code" / "worker"
        code.mkdir(parents=True, exist_ok=True)
        for script, module in _ENTRY_POINTS.items():
            (code / script).write_text(_bootstrap(module), encoding="utf-8")
        blender = ctx.tool("blender").executable
        if blender is None:
            raise ctx.fail("blender is not installed: install it or set GRIDA_FX_TOOL_BLENDER")
        self.root = root
        self.worker = WorkerClient(
            blender=blender,
            package_root=root / "code",
            input_root=root,
            run_root=root / "run",
            max_calls=max_calls,
            timeout_seconds=timeout_seconds,
        )

    def adopt(self, data: bytes, name: str) -> Record:
        """Place a model or picture on the bench: its path from the bench root, and digest."""

        path = confined(self.root, f"inputs/{name}", must_exist=False)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return {"path": path.relative_to(self.root).as_posix(), "sha256": digest(path)}

    def read(self, source: Mapping[str, Any]) -> bytes:
        path = confined(self.root, str(source["path"]))
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != source["sha256"]:
            raise ValueError("A bench file changed after it was registered")
        return data

    def run_file(self, ref: str) -> Path:
        return confined(self.worker.run_root, ref)


# ---------------------------------------------------------------------------- episodes


def host_note(review_mode: str, *, max_steps: int, reviewer: bool) -> str:
    """What the host tells every agent about its limits, and what its answer is for."""

    caps = json.dumps({"max_steps": max_steps, "max_tokens_per_reply": TURN_TOKENS})
    if review_mode == "none" and not reviewer:
        purpose = (
            "The caller selected review_mode none. Any earlier description of a separate "
            "reviewer or review-requested repair round does not apply to this run. Submit your "
            "best real candidate within the generation limits, recording remaining defects and "
            "uncertainties in open_issues. The host will retain and deliver it as unreviewed "
            "after technical validation; do not claim acceptance."
        )
    else:
        purpose = (
            "Plan for an admitted submit before your turns run out. A producer submission is a "
            "candidate for independent review; a reviewer must still apply every required "
            "criterion to the full evidence."
        )
    return f"\nHost episode limits: {caps}. {purpose}"


async def episode(
    ctx: Ctx,
    *,
    system: str,
    instructions: Mapping[str, Any],
    schema: Mapping[str, Any],
    check: Callable[[Any], None],
    review_mode: str,
    max_steps: int,
    tools: Sequence[Any] = (),
    images: Sequence[Any] = (),
    reviewer: bool = False,
    recent_images: int | None = None,
) -> Record:
    """One agent episode: its answer, submitted and checked, or the step fails."""

    agent = ctx.agent(
        system=system + host_note(review_mode, max_steps=max_steps, reviewer=reviewer),
        tools=tools,
        recent_images=recent_images,
        max_tokens=TURN_TOKENS,
    )
    answer = await agent.run(
        json.dumps(instructions, sort_keys=True),
        max_steps=max_steps,
        images=images,
        submit=schema,
        check=check,
    )
    return dict(answer)


# ---------------------------------------------------------------------------- reviews


def verdict_check(
    asset_id: str,
    source_sha256: str,
    criteria: Sequence[str],
    *,
    bar: Mapping[str, Any] | None = None,
    extra: Callable[[Record], None] | None = None,
) -> Callable[[Any], None]:
    """What a reviewer's answer must hold: the exact candidate, every criterion once, and an
    acceptance that agrees with them."""

    def check(value: Any) -> None:
        jsonschema.validate(value, REVIEW_SCHEMA)
        if value["asset_id"] != asset_id or value["source_sha256"] != source_sha256:
            raise ValueError("Review must name the exact frozen candidate")
        named = [item["criterion"] for item in value["criteria"]]
        if len(named) != len(criteria) or set(named) != set(criteria):
            raise ValueError("Review every required criterion exactly once")
        passed = all(item["passed"] for item in value["criteria"]) and not any(
            issue["severity"] == "blocking" for issue in value["issues"]
        )
        if value["accepted"] != passed:
            raise ValueError("Acceptance must match criterion results and blocking issues")
        if bar is not None:
            check_issue_heights(value, dict(bar))
        if extra is not None:
            extra(value)

    return check


def structural_rejection(
    asset_id: str,
    source_sha256: str,
    criteria: Sequence[str],
    *,
    region: str,
    message: str,
    repair: str,
    height: int | None = None,
) -> Record:
    """A deterministic rejection of a candidate that never reached review."""

    issue: Record = {
        "severity": "blocking",
        "region": region,
        "description": message,
        "repair": repair,
    }
    if height is not None:
        issue["smallest_visible_character_height_pixels"] = height
    return {
        "asset_id": asset_id,
        "source_sha256": source_sha256,
        "accepted": False,
        "criteria": [
            {"criterion": name, "passed": False, "evidence": message} for name in criteria
        ],
        "issues": [issue],
        "notes": "Deterministic structural rejection; no model review was claimed.",
    }


def report_verdict(ctx: Ctx, review: Mapping[str, Any]) -> None:
    """A review's verdict as the facts a regenerating group reads, and the next take hears."""

    ctx.fact("verdict", "accept" if review["accepted"] else "reject")
    ctx.fact("failed_criteria", [c["criterion"] for c in review["criteria"] if not c["passed"]])
    ctx.fact(
        "issues",
        [
            {key: issue[key] for key in ("severity", "region", "description", "repair")}
            for issue in review["issues"]
        ],
    )


def review_view(said: Mapping[str, Any] | None, step: str = "review") -> Record | None:
    """The previous take's verdict as a producer is shown it: what failed, how to repair."""

    facts = (said or {}).get(step)
    if not facts:
        return None
    return {
        "accepted": facts.get("verdict") == "accept",
        "failed_criteria": facts.get("failed_criteria", []),
        "issues": facts.get("issues", []),
    }


# ---------------------------------------------------------------------------- evidence


def review_material_mode(profile: Mapping[str, Any]) -> str:
    """Pre-export reviews see the declared finish, never raw provider gloss."""

    policy = profile.get("surface_policy", {}).get("default", "preserve")
    return "matte_policy" if policy == "matte" else "native"


async def review_evidence(
    studio: Any, profile: Mapping[str, Any], asset_id: str
) -> tuple[list[Record], list[str]]:
    """The rest pose at inspection height and at every gameplay height, from every required
    view, in the export's finish: the records and the pictures."""

    review = profile.get("review", {})
    views = review.get("required_views", ["front", "back", "left", "right", "three_quarter"])
    sizes = [
        (review.get("inspection_height_pixels"), "inspection"),
        *((size, "gameplay") for size in review.get("target_character_height_pixels", [])),
    ]
    if len(sizes) * len(views) > 60:
        raise ValueError("Mandatory evidence exceeds the bounded review image allowance")
    evidence: list[Record] = []
    pictures: list[str] = []
    for size, tier in sizes:
        record, images = await studio.render(
            asset_id,
            views=views,
            material_mode=review_material_mode(profile),
            character_height_pixels=size,
        )
        evidence.append({**record, "review_tier": tier})
        pictures.extend(images)
    return evidence, pictures


async def atlas_evidence(
    studio: Any,
    profile: Mapping[str, Any],
    bar: Mapping[str, Any],
    asset_id: str,
    poses: Sequence[tuple[str | None, float]],
) -> tuple[Record, list[Path]]:
    """Every pose at the bar's presentation height cut into one labeled row per pose, a face
    strip at inspection height, and chest to mid-thigh of every pose at the torso height, one
    image per pose: the evidence record and the picture files."""

    import asyncio

    from _character_tools.atlas import (
        AtlasCell,
        build_atlas,
        build_face_strip,
        build_torso_strip,
    )

    review = profile.get("review", {})
    views = review.get("required_views", ["front", "back", "left", "right", "three_quarter"])
    height = int(bar["cell_character_height_pixels"])
    unique = list(dict.fromkeys(poses))
    if len(unique) * len(views) > 60:
        raise ValueError("Atlas evidence exceeds the bounded review image allowance")
    directory = studio.next_dir("atlas")
    folder = confined(studio.worker.run_root, directory, must_exist=False)
    folder.mkdir(parents=True, exist_ok=True)
    rows: list[Record] = []
    files: list[Path] = []
    for index, (clip, seconds) in enumerate(unique, start=1):
        record, _ = await studio.render(
            asset_id,
            views=views,
            pose={"clip": clip, "time_seconds": seconds, "fps": 24} if clip else None,
            character_height_pixels=height,
        )
        label = f"{clip} {seconds:g}s" if clip else "rest"
        cells = [
            AtlasCell(label, view, confined(studio.worker.run_root, item["path"]))
            for view, item in zip(views, record["images"], strict=True)
        ]
        image, manifest = await asyncio.to_thread(build_atlas, cells)
        row = folder / f"row-{index:02d}.png"
        image.save(row, format="PNG")
        files.append(row)
        rows.append(
            {
                "label": label,
                "path": f"{directory}/row-{index:02d}.png",
                "sha256": digest(row),
                "columns": manifest["columns"],
                "cells": manifest["cells"],
            }
        )
    inspection = int(review.get("inspection_height_pixels", 512))
    face_views = [view for view in ("front", "three_quarter") if view in views]
    face, _ = await studio.render(
        asset_id, views=face_views, pose=None, character_height_pixels=inspection
    )
    face_cells = [
        AtlasCell("face rest", view, confined(studio.worker.run_root, item["path"]))
        for view, item in zip(face_views, face["images"], strict=True)
    ]
    strip, strip_manifest = await asyncio.to_thread(build_face_strip, face_cells)
    strip_path = folder / "face.png"
    strip.save(strip_path, format="PNG")
    files.append(strip_path)
    # Chest to mid-thigh at the torso height, front and back, at rest and in every motion, one
    # image per pose: a one-sided hem or panel that tears out as a joint moves is a few pixels
    # in a row cell and tens of pixels here.
    torso_views = [view for view in ("front", "back") if view in views]
    torso_height = int(bar["torso_character_height_pixels"])
    torso_rows: list[Record] = []
    for index, (clip, seconds) in enumerate(
        [(None, 0.0), *((c, s) for c, s in unique if c)], start=1
    ):
        record, _ = await studio.render(
            asset_id,
            views=torso_views,
            pose={"clip": clip, "time_seconds": seconds, "fps": 24} if clip else None,
            character_height_pixels=torso_height,
        )
        label = f"{clip} {seconds:g}s" if clip else "rest"
        torso_cells = [
            AtlasCell(label, view, confined(studio.worker.run_root, item["path"]))
            for view, item in zip(torso_views, record["images"], strict=True)
        ]
        torso, torso_manifest = await asyncio.to_thread(build_torso_strip, torso_cells)
        torso_path = folder / f"torso-{index:02d}.png"
        torso.save(torso_path, format="PNG")
        files.append(torso_path)
        torso_rows.append(
            {
                "label": label,
                "path": f"{directory}/torso-{index:02d}.png",
                "sha256": digest(torso_path),
                "band": torso_manifest["band"],
                "cells": torso_manifest["cells"],
            }
        )
    evidence = {
        "kind": bar["evidence_kind"],
        "asset_id": asset_id,
        "verdict_character_height_pixels": bar["verdict_character_height_pixels"],
        "cell_character_height_pixels": height,
        "rows": rows,
        "columns": list(views),
        "face_strip": {
            "path": f"{directory}/face.png",
            "sha256": digest(strip_path),
            "source_character_height_pixels": inspection,
            "cells": strip_manifest["cells"],
        },
        "torso": {
            "source_character_height_pixels": torso_height,
            "columns": torso_views,
            "rows": torso_rows,
        },
    }
    return evidence, files
