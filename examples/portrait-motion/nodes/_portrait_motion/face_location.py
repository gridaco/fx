"""Face-location answers: where the face is, never whether it can move.

The question itself is the portrait-motion workflow's ``prompts/locate.md``.
"""

from __future__ import annotations

from typing import Any


def locator_schema() -> dict[str, Any]:
    return {
        "type": "object",
        "additionalProperties": False,
        "properties": {
            "status": {"type": "string", "enum": ["located", "not_locatable"]},
            "bbox_xyxy": {
                "anyOf": [
                    {
                        "type": "array",
                        "items": {"type": "integer", "minimum": 0, "maximum": 1000},
                        "minItems": 4,
                        "maxItems": 4,
                    },
                    {"type": "null"},
                ]
            },
            "reason": {"type": "string", "minLength": 1, "maxLength": 600},
        },
        "required": ["status", "bbox_xyxy", "reason"],
    }


def validate_location(value: object) -> dict[str, Any]:
    """Validate syntax and bounds only; never add a semantic admission veto."""
    if not isinstance(value, dict) or set(value) != {"status", "bbox_xyxy", "reason"}:
        raise ValueError("Expected exactly status, bbox_xyxy, and reason")
    if value["status"] not in ("located", "not_locatable"):
        raise ValueError("Unknown location status")
    if not isinstance(value["reason"], str) or not 1 <= len(value["reason"].strip()) <= 600:
        raise ValueError("Expected a short nonempty reason")
    bbox = value["bbox_xyxy"]
    if value["status"] == "not_locatable":
        if bbox is not None:
            raise ValueError("Unlocated face must have a null box")
    elif (
        not isinstance(bbox, list)
        or len(bbox) != 4
        or any(type(number) is not int or not 0 <= number <= 1000 for number in bbox)
        or not bbox[0] < bbox[2]
        or not bbox[1] < bbox[3]
    ):
        raise ValueError("Located face requires a nonempty in-bounds box")
    return value.copy()
