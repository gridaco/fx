"""Deterministic agent-facing views; persisted worker evidence stays complete.

Only known, repeated presentation fields are removed. Numerical findings are
never sampled or truncated, and unknown fields survive. Full camera transforms,
clip binding inventories and previous material definitions remain in the linked
reports. Actual world camera axes/position and final materials stay in the view.
This is response serialization, not a change to the agent's conversation loop.
"""

from __future__ import annotations

from collections.abc import Collection, Sequence
from copy import deepcopy
from typing import Any

Record = dict[str, Any]
RENDER_VIEW_PROJECTION = "render_view_v1"


def _diagnostic_key(key: object) -> bool:
    words = str(key).lower().split("_")
    return bool(
        set(words)
        & {
            "warning",
            "warnings",
            "error",
            "errors",
            "failure",
            "failures",
            "findings",
            "advisory",
            "advisories",
            "limitations",
            "missing",
        }
    )


def _has_diagnostics(value: object) -> bool:
    if isinstance(value, dict):
        return any((_diagnostic_key(key) or _has_diagnostics(item) for key, item in value.items()))
    if isinstance(value, list):
        return any(_has_diagnostics(item) for item in value)
    return False


def _without(value: Record, fields: Collection[str]) -> Record:
    """Drop known boilerplate only; unexpected nested diagnostics veto removal."""
    return {
        key: deepcopy(item)
        for key, item in value.items()
        if key not in fields or _has_diagnostics(item)
    }


def _diagnostics(value: object, path: str = "") -> Record:
    """Expose report diagnostics outside the normal response, with exact locations."""
    found: Record = {}
    if isinstance(value, dict):
        for key, item in value.items():
            escaped = str(key).replace("~", "~0").replace("/", "~1")
            pointer = path + "/" + escaped
            if _diagnostic_key(key):
                found[pointer] = deepcopy(item)
            else:
                found.update(_diagnostics(item, pointer))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            found.update(_diagnostics(item, path + "/" + str(index)))
    return found


def _clip_views(clips: Sequence[Record]) -> list[Record]:
    return [_without(clip, {"bindings"}) for clip in clips]


def render_view(
    record: Record, *, report: Record | None = None, request: Record | None = None
) -> Record:
    """Project tool/reviewer text, preserving image evidence and diagnostic facts.

    Initial reviewer prompts declare RENDER_VIEW_PROJECTION; stored observations
    and review artifacts retain the full original records.
    """
    result = deepcopy(record)
    pose = result.get("pose")
    if isinstance(pose, dict) and "available_clips" in pose:
        pose["available_clips"] = _clip_views(pose["available_clips"])
    for image in result.get("images", []):
        if isinstance(image.get("camera"), dict):
            actual = image["camera"].get("actual_camera_matrix_world")
            has_actual_frame = (
                isinstance(actual, list)
                and len(actual) == 4
                and all(isinstance(row, list) and len(row) == 4 for row in actual)
            )
            omitted = {"pixel_ray"}
            if has_actual_frame:
                omitted.update(
                    {"camera_matrix_world", "actual_camera_matrix_world", "camera_local_frame"}
                )
            image["camera"] = _without(image["camera"], omitted)
            if has_actual_frame:
                image["camera"]["actual_world_frame"] = {
                    "position": [actual[row][3] for row in range(3)],
                    "right": [actual[row][0] for row in range(3)],
                    "up": [actual[row][1] for row in range(3)],
                    "back": [actual[row][2] for row in range(3)],
                }
    if request is not None:
        result["requested_render"] = deepcopy(request)
    if report is not None:
        if "coordinate_system" in report:
            result["coordinate_system"] = _without(
                report["coordinate_system"], {"gltf_to_blender", "blender_to_gltf"}
            )
        for key in ("status", "source_unchanged", "blender_version"):
            if key in report:
                result[key] = deepcopy(report[key])
        result["render_settings"] = _without(report.get("result", {}), {"images", "framing_bounds"})
        diagnostics = _diagnostics(report)
        if diagnostics:
            result["report_diagnostics"] = diagnostics
    return result


def review_view(review: Record | None) -> Record | None:
    """Compact prior-review render records only when handing feedback to repair.

    Verdicts, criteria, issues and unknown review fields remain complete. This
    must not replace the stored review or the independent reviewer's input.
    """
    if review is None:
        return None
    if not isinstance(review, dict):
        raise ValueError("Previous review must be an object or absent")

    def compact_record(record: Record) -> Record:
        if (
            not isinstance(record, dict)
            or not isinstance(record.get("images", []), list)
            or any(not isinstance(image, dict) for image in record.get("images", []))
        ):
            raise ValueError("Review evidence requires render records with image objects")
        return render_view(record)

    result = deepcopy(review)
    if "initial_evidence" in result:
        evidence = result["initial_evidence"]
        if isinstance(evidence, dict):
            result["initial_evidence"] = compact_record(evidence)
        elif isinstance(evidence, list):
            result["initial_evidence"] = [compact_record(record) for record in evidence]
        else:
            raise ValueError("Review evidence must be one render object or a list of them")
    return result


def rig_build_view(
    payload: Record, *, metrics: Record | None = None, render_report: Record | None = None
) -> Record:
    """Keep rig lineage and complete findings while removing repeated definitions."""
    result = deepcopy(payload)
    report = result.get("rig_report", {})
    for binding in report.get("bindings", []):
        for key in ("eligible_bones", "weighted_bones"):
            if key in binding and (not _has_diagnostics(binding[key])):
                binding[key + "_count"] = len(binding.pop(key))
    report["material_edits"] = [
        _without(edit, {"before"}) for edit in report.get("material_edits", [])
    ]
    report["clips"] = [_without(clip, {"axis_semantics"}) for clip in report.get("clips", [])]
    if "available_clips" in result:
        result["available_clips"] = _clip_views(result["available_clips"])
    if "render" in result:
        result["render"] = render_view(result["render"], report=render_report)
    if metrics is not None:
        result["numeric_findings"] = deepcopy(metrics["blocking_findings"])
        result["numeric_advisories"] = deepcopy(metrics.get("advisories", []))
        for key in ("status", "limitations", "visual_review"):
            if key in metrics:
                result["numeric_" + key] = deepcopy(metrics[key])
        diagnostics = _diagnostics(
            {
                key: value
                for key, value in metrics.items()
                if key not in {"blocking_findings", "advisories", "limitations"}
            }
        )
        if diagnostics:
            result["metrics_diagnostics"] = diagnostics
    return result
