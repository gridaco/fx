"""Bounded reference-image tools with injected generation and immutable lineage.

No provider client, model selection, pricing, credential loader or retry loop
lives here. Cropping extracts exact atlas pixels; it never repaints an image.
"""

from __future__ import annotations

import asyncio
import base64
import copy
import hashlib
import json
import os
import shutil
import tempfile
from collections.abc import Awaitable, Callable, Mapping
from io import BytesIO
from pathlib import Path
from typing import Any, cast

import jsonschema
from PIL import Image

from _character_tools.io import (
    canonical_digest,
    confined,
    digest,
    verified_input,
    write_bytes,
    write_json,
)
from _character_tools.studio import nullable, object_schema
from grida.fx import Tool, ToolInvocationError, ToolResult

Record = dict[str, Any]
ImageGenerator = Callable[[Record], Awaitable[Record]]
ToolMethod = Callable[[Record], Awaitable[ToolResult]]
VIEWS = ("front", "back", "left", "right", "three_quarter", "detail")
PURPOSES = ("canonical", "part_atlas", "part_view")


#: Each shape a reference may be drawn in, at the exact size the reference route draws it.
ASPECT_SIZES = {
    "1:1": "1024x1024",
    "2:3": "1712x2560",
    "3:2": "2560x1712",
    "16:9": "2560x1440",
    "21:9": "2496x1152",
    "9:21": "1152x2496",
}


def inspect_image(data: bytes) -> Record:
    """A reference picture's format and size, refused unless a PNG or JPEG of usable size."""

    if not data or len(data) > 20000000:
        raise ValueError("Reference image must be nonempty and at most 20 MB")
    with Image.open(BytesIO(data)) as image:
        if image.format not in {"PNG", "JPEG"}:
            raise ValueError("This narrow adapter accepts PNG or JPEG only")
        if min(image.size) < 64 or max(image.size) > 8192:
            raise ValueError("Reference dimensions must be within 64 through 8192 pixels")
        result: Record = {
            "format": image.format.lower(),
            "width": image.width,
            "height": image.height,
        }
        image.verify()
    result["media_type"] = "image/png" if result["format"] == "png" else "image/jpeg"
    return result


class ReferenceGenerationStopped(RuntimeError):
    """Terminal callback outcome requiring host reconciliation, never an agent retry."""


def _text(value: object, label: str, maximum: int = 12000) -> str:
    if not isinstance(value, str) or not value.strip() or len(value) > maximum:
        raise ValueError(f"{label} must be nonempty bounded text")
    return value


class ReferenceStudio:
    def __init__(
        self,
        input_root: Path,
        run_root: Path,
        *,
        profile: Record,
        rights_basis: str,
        generate_image: ImageGenerator,
        max_revisions: int = 6,
        max_crops: int = 64,
    ) -> None:
        self.input_root = Path(input_root).resolve(strict=True)
        self.run_root = Path(run_root).resolve(strict=True)
        if not self.run_root.is_relative_to(self.input_root):
            raise ValueError("Reference run root must be inside the declared input root")
        self.profile = copy.deepcopy(profile)
        self.roles = tuple(profile["required_parts"])
        if not self.roles or len(self.roles) > 16 or len(set(self.roles)) != len(self.roles):
            raise ValueError("Profile needs one through sixteen unique part roles")
        for role in self.roles:
            _text(role, "Part role", 100)
        self.profile_sha256 = canonical_digest(self.profile)
        self.rights_basis = _text(rights_basis, "Rights basis", 4000)
        if type(max_revisions) is not int or not 1 <= max_revisions <= 32:
            raise ValueError("Reference generation limit must be one through 32")
        if type(max_crops) is not int or not 1 <= max_crops <= 128:
            raise ValueError("Crop limit must be one through 128")
        if not callable(generate_image):
            raise ValueError("An async generation callback is required")
        self.generate_image = generate_image
        self.max_revisions, self.max_crops = (max_revisions, max_crops)
        self.directory = confined(
            self.input_root, self._ref(self.run_root / "references"), must_exist=False
        )
        self.directory.mkdir(exist_ok=True)
        self.assets: dict[str, Record] = {}
        self.attempts: list[Record] = []
        self.bundles: dict[str, Record] = {}
        self.crop_count = self.asset_counter = self.state_counter = 0
        self.frozen_bundle_id: str | None = None
        self._lock = asyncio.Lock()
        self._busy = False

    def _ref(self, path: Path) -> str:
        return Path(path).relative_to(self.input_root).as_posix()

    def _descriptor(self, path: Path) -> dict[str, str]:
        return {"path": self._ref(path), "sha256": digest(path)}

    def _image(self, source: Record) -> tuple[bytes, Record]:
        path = verified_input(self.input_root, source)
        payload = path.read_bytes()
        facts = inspect_image(payload)
        with Image.open(BytesIO(payload)) as image:
            if image.getexif().get(274, 1) not in (None, 1):
                raise ValueError(
                    "Image has EXIF rotation; explicitly normalize orientation upst"
                    "ream before pixel labeling"
                )
            image.load()
        return (payload, facts)

    def asset(self, asset_id: str) -> Record:
        if asset_id not in self.assets:
            raise ValueError("Unknown reference asset_id")
        asset = self.assets[asset_id]
        _, facts = self._image(asset["source"])
        if facts != asset["image"]:
            raise ValueError("Registered image facts changed")
        if asset["provenance"] is not None:
            verified_input(self.input_root, asset["provenance"])
        for parent in asset["lineage"].get("inputs", []):
            if parent["asset_id"] not in self.assets:
                raise ValueError("Reference lineage names an unavailable parent")
            current = self.assets[parent["asset_id"]]
            if (
                current["source"]["sha256"] != parent["source_sha256"]
                or current["rights_basis"] != parent["rights_basis"]
            ):
                raise ValueError("Reference parent lineage differs from the frozen record")
            verified_input(self.input_root, current["source"])
        return copy.deepcopy(asset)

    def _validate_label(self, purpose: str, roles: list[str], view: str | None) -> None:
        if purpose not in PURPOSES or not isinstance(roles, list) or len(set(roles)) != len(roles):
            raise ValueError("Invalid reference purpose or duplicate roles")
        if set(roles) - set(self.roles):
            raise ValueError("Reference names a role outside profile.required_parts")
        if purpose == "canonical" and (roles or view is not None):
            raise ValueError("Canonical reference has no isolated part roles or view label")
        if purpose == "part_atlas" and (not roles or view is not None):
            raise ValueError("Part atlas needs declared roles and no single-view label")
        if purpose == "part_view" and (len(roles) != 1 or view not in VIEWS):
            raise ValueError("Part view needs one role and a known view label")

    def _require_mutable(self) -> None:
        if self.frozen_bundle_id is not None:
            raise ValueError(
                "Reference bundle is frozen; only the host can open a reviewed revision"
            )
        if any(item["status"] in {"in_flight", "callback_failed"} for item in self.attempts):
            raise ReferenceGenerationStopped(
                "A failed or unfinished generation requires host reconciliation before more work"
            )

    def _state(self) -> Record:
        return {
            "schema_version": 1,
            "profile_sha256": self.profile_sha256,
            "rights_basis": self.rights_basis,
            "max_revisions": self.max_revisions,
            "max_crops": self.max_crops,
            "assets": copy.deepcopy(self.assets),
            "attempts": copy.deepcopy(self.attempts),
            "bundles": copy.deepcopy(self.bundles),
            "crop_count": self.crop_count,
            "asset_counter": self.asset_counter,
            "state_counter": self.state_counter,
            "frozen_bundle_id": self.frozen_bundle_id,
        }

    def snapshot(self) -> Record:
        state = self._state()
        return {**state, "state_sha256": canonical_digest(state)}

    def _persist(self) -> None:
        self.state_counter += 1
        path = self.directory / "registry" / f"state_{self.state_counter:04d}.json"
        state = self.snapshot()
        if path.exists():
            if json.loads(path.read_text()) != state:
                raise ValueError("Existing immutable registry version differs from recovery state")
        else:
            write_json(path, state)

    def restore(self, state: Record) -> None:
        """Host-only recovery from data; never generates or resubmits anything."""
        if self._busy or self.assets or self.attempts or self.bundles or self.state_counter:
            raise ValueError("Restore requires an unused studio instance")
        expected = set(self._state()) | {"state_sha256"}
        if not isinstance(state, dict) or set(state) != expected:
            raise ValueError("Snapshot fields differ from the reference state contract")
        body = {key: value for key, value in state.items() if key != "state_sha256"}
        if canonical_digest(body) != state["state_sha256"]:
            raise ValueError("Reference snapshot digest mismatch")
        for field in (
            "schema_version",
            "profile_sha256",
            "rights_basis",
            "max_revisions",
            "max_crops",
        ):
            if state[field] != self._state()[field]:
                raise ValueError("Snapshot does not match this reference profile or limits")
        for field in ("crop_count", "asset_counter", "state_counter"):
            if type(state[field]) is not int or state[field] < 0:
                raise ValueError("Snapshot counters must be nonnegative integers")
        if state["crop_count"] > self.max_crops or len(state["attempts"]) > self.max_revisions:
            raise ValueError("Snapshot exceeds configured revision limits")
        self.assets = copy.deepcopy(state["assets"])
        self.attempts = copy.deepcopy(state["attempts"])
        self.bundles = copy.deepcopy(state["bundles"])
        self.crop_count, self.asset_counter, self.state_counter = (
            state[key] for key in ("crop_count", "asset_counter", "state_counter")
        )
        self.frozen_bundle_id = state["frozen_bundle_id"]
        try:
            if self.asset_counter != len(self.assets):
                raise ValueError("Snapshot asset counter does not match immutable asset records")
            for index, (asset_id, value) in enumerate(self.assets.items(), 1):
                if (
                    asset_id != f"ref_{index:04d}"
                    or value["asset_id"] != asset_id
                    or value["semantic_status"] != "unreviewed"
                ):
                    raise ValueError("Invalid immutable asset identity or semantic state")
                self._validate_label(value["purpose"], value["roles"], value["view"])
                _text(value["rights_basis"], "Restored rights basis", 4000)
                path = confined(
                    self.input_root, self._ref(self.directory / "assets" / asset_id / "asset.json")
                )
                if json.loads(path.read_text()) != value:
                    raise ValueError("Snapshot asset differs from its immutable asset record")
                self.asset(asset_id)
            for attempt in self.attempts:
                verified_input(self.input_root, attempt["plan"])
                if attempt["status"] not in {"in_flight", "collected", "callback_failed"}:
                    raise ValueError("Unknown generation attempt state")
            for bundle in self.bundles.values():
                self._verified_bundle(bundle)
            if self.frozen_bundle_id is not None and self.frozen_bundle_id not in self.bundles:
                raise ValueError("Snapshot frozen bundle is unavailable")
        except BaseException:
            self.assets = {}
            self.attempts = []
            self.bundles = {}
            self.crop_count = self.asset_counter = self.state_counter = 0
            self.frozen_bundle_id = None
            raise

    def _commit_asset(
        self,
        receipt: Record,
        *,
        purpose: str,
        roles: list[str],
        view: str | None,
        rights_basis: str,
        lineage: Record,
        extracted_bytes: bytes | None = None,
    ) -> Record:
        if not isinstance(receipt, dict) or set(receipt) != {"source", "provenance"}:
            raise ValueError(
                "Generation receipt needs only source and nullable provenance descriptors"
            )
        asset_id = f"ref_{self.asset_counter + 1:04d}"
        if extracted_bytes is None:
            _, facts = self._image(receipt["source"])
        else:
            expected = {
                "path": self._ref(self.directory / "assets" / asset_id / "image.png"),
                "sha256": hashlib.sha256(extracted_bytes).hexdigest(),
            }
            if receipt != {"source": expected, "provenance": None}:
                raise ValueError("Extracted pixels must target their exact new asset directory")
            facts = inspect_image(extracted_bytes)
        if receipt["provenance"] is not None:
            verified_input(self.input_root, receipt["provenance"])
        self._validate_label(purpose, roles, view)
        record = {
            "asset_id": asset_id,
            "source": copy.deepcopy(receipt["source"]),
            "provenance": copy.deepcopy(receipt["provenance"]),
            "purpose": purpose,
            "roles": list(roles),
            "view": view,
            "image": facts,
            "rights_basis": rights_basis,
            "semantic_status": "unreviewed",
            "lineage": copy.deepcopy(lineage),
        }
        path = confined(
            self.input_root,
            self._ref(self.directory / "assets" / asset_id / "asset.json"),
            must_exist=False,
        )
        if path.exists():
            if json.loads(path.read_text()) != record:
                raise ValueError(
                    "Existing immutable asset differs from recovered generation receipt"
                )
            self._image(receipt["source"])
        elif extracted_bytes is not None:
            parent = path.parent.parent
            parent.mkdir(parents=True, exist_ok=True)
            staged = Path(tempfile.mkdtemp(prefix=".pending_", dir=parent))
            try:
                write_bytes(staged / "image.png", extracted_bytes)
                write_json(staged / "asset.json", record)
                if path.parent.exists():
                    raise ValueError("Refuse an existing incomplete crop asset directory")
                os.rename(staged, path.parent)
            finally:
                shutil.rmtree(staged, ignore_errors=True)
        else:
            write_json(path, record)
        self.asset_counter += 1
        self.assets[asset_id] = record
        return copy.deepcopy(record)

    def adopt_reference(
        self,
        source: Record,
        *,
        purpose: str,
        roles: list[str],
        rights_basis: str,
        view: str | None = None,
        provenance: Record | None = None,
    ) -> Record:
        """Host-only in-place adoption; rights come from explicit host authorization."""
        self._require_mutable()
        record = self._commit_asset(
            {"source": source, "provenance": provenance},
            purpose=purpose,
            roles=roles,
            view=view,
            rights_basis=_text(rights_basis, "Adopted rights basis", 4000),
            lineage={"kind": "adopted", "inputs": []},
        )
        self._persist()
        return record

    def _schemas(self) -> dict[str, Record]:
        text = {"type": "string", "minLength": 1, "maxLength": 12000}
        asset_id = {"type": "string", "pattern": "^ref_[0-9]{4}$"}
        role = {"type": "string", "enum": list(self.roles)}
        view = {"type": "string", "enum": list(VIEWS)}
        return {
            "list_references": object_schema({}),
            "inspect_reference": object_schema({"asset_id": asset_id}),
            "inspect_reference_bundle": object_schema({}),
            "generate_reference": object_schema(
                {
                    "purpose": {"type": "string", "enum": list(PURPOSES)},
                    "roles": {
                        "type": "array",
                        "items": role,
                        "maxItems": len(self.roles),
                        "uniqueItems": True,
                    },
                    "view": nullable(view),
                    "prompt": text,
                    "reference_asset_ids": {
                        "type": "array",
                        "items": asset_id,
                        "maxItems": 4,
                        "uniqueItems": True,
                    },
                    "aspect_ratio": {"type": "string", "enum": list(ASPECT_SIZES)},
                    "reason": {"type": "string", "minLength": 1, "maxLength": 2000},
                }
            ),
            "extract_atlas_views": object_schema(
                {
                    "asset_id": asset_id,
                    "views": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 16,
                        "items": object_schema(
                            {
                                "role": role,
                                "view": view,
                                "box": {
                                    "type": "array",
                                    "items": {"type": "integer", "minimum": 0},
                                    "minItems": 4,
                                    "maxItems": 4,
                                },
                            }
                        ),
                    },
                }
            ),
        }

    async def list_references(self, args: Record) -> ToolResult:
        return ToolResult(
            json.dumps(
                {
                    "assets": [self.asset(key) for key in self.assets],
                    "generation_attempts": len(self.attempts),
                    "generation_limit": self.max_revisions,
                    "crop_count": self.crop_count,
                    "crop_limit": self.max_crops,
                    "frozen_bundle_id": self.frozen_bundle_id,
                }
            )
        )

    async def inspect_reference(self, args: Record) -> ToolResult:
        asset = self.asset(args["asset_id"])
        payload, facts = self._image(asset["source"])
        image = f"data:{facts['media_type']};base64," + base64.b64encode(payload).decode("ascii")
        return ToolResult(json.dumps({"asset": asset}), (image,))

    async def inspect_reference_bundle(self, args: Record) -> ToolResult:
        if self.frozen_bundle_id is None:
            raise ValueError("No immutable reference bundle has been submitted")
        bundle = self._verified_bundle(self.bundles[self.frozen_bundle_id])
        return ToolResult(
            json.dumps(
                {
                    "bundle": bundle,
                    "inspection_instruction": (
                        "Inspect the canonical image and every listed part view with in"
                        "spect_reference before judging these exact hashes. Labels are "
                        "producer assertions, not proven view directions."
                    ),
                }
            )
        )

    async def generate_reference(self, args: Record) -> ToolResult:
        jsonschema.validate(args, self._schemas()["generate_reference"])
        async with self._lock:
            self._require_mutable()
            if len(self.attempts) >= self.max_revisions:
                raise ValueError("Reference semantic generation revision budget exhausted")
            self._validate_label(args["purpose"], args["roles"], args["view"])
            inputs = [self.asset(key) for key in args["reference_asset_ids"]]
            if args["purpose"] != "canonical" and (
                not any(item["purpose"] == "canonical" for item in inputs)
            ):
                raise ValueError(
                    "Part generation must explicitly reference a registered canonical image"
                )
            operation_id = f"ref_image_{len(self.attempts) + 1:03d}"
            plan = {
                "operation_id": operation_id,
                "purpose": args["purpose"],
                "roles": args["roles"],
                "prompt": args["prompt"],
                "inputs": [item["source"] for item in inputs],
                "params": {
                    "aspect_ratio": args["aspect_ratio"],
                    "size": ASPECT_SIZES[args["aspect_ratio"]],
                    "background": "opaque",
                },
                "rights_basis": self.rights_basis,
            }
            plan_path = self.directory / "plans" / f"{operation_id}.json"
            write_json(
                plan_path,
                {
                    "generation": plan,
                    "view": args["view"],
                    "reason": args["reason"],
                    "input_asset_ids": args["reference_asset_ids"],
                },
            )
            attempt = {
                "operation_id": operation_id,
                "status": "in_flight",
                "plan": self._descriptor(plan_path),
                "asset_id": None,
            }
            self.attempts.append(attempt)
            self._persist()
            self._busy = True
            try:
                receipt = await self.generate_image(copy.deepcopy(plan))
                record = self._collect_attempt(attempt, receipt)
            except BaseException as error:
                if isinstance(error, Exception):
                    attempt.update(status="callback_failed", error_type=type(error).__name__)
                    try:
                        self._persist()
                    except Exception:
                        raise ReferenceGenerationStopped(
                            "Generation stopped; host reconciliation and state recovery are"
                            " required"
                        ) from None
                    raise ReferenceGenerationStopped(
                        "Generation callback or receipt failed; host reconciliation is required"
                    ) from None
                raise
            finally:
                self._busy = False
            return await self.inspect_reference({"asset_id": record["asset_id"]})

    def _collect_attempt(self, attempt: Record, receipt: Record) -> Record:
        saved = json.loads(verified_input(self.input_root, attempt["plan"]).read_text())
        parents = [self.asset(key) for key in saved["input_asset_ids"]]
        generation = saved["generation"]
        record = self._commit_asset(
            receipt,
            purpose=generation["purpose"],
            roles=generation["roles"],
            view=saved["view"],
            rights_basis=generation["rights_basis"],
            lineage={
                "kind": "generated",
                "operation_id": attempt["operation_id"],
                "generation_plan": attempt["plan"],
                "inputs": [
                    {
                        "asset_id": item["asset_id"],
                        "source_sha256": item["source"]["sha256"],
                        "rights_basis": item["rights_basis"],
                    }
                    for item in parents
                ],
            },
        )
        attempt.update(status="collected", asset_id=record["asset_id"])
        self._persist()
        return record

    def reconcile_generation(self, operation_id: str, receipt: Record) -> Record:
        """Host-only adoption of an already-reconciled provider receipt; no call."""
        if self._busy or self.frozen_bundle_id is not None:
            raise ValueError("Cannot reconcile while generation is busy or references are frozen")
        attempt = next(
            (item for item in self.attempts if item["operation_id"] == operation_id), None
        )
        if attempt is None or attempt["status"] not in {"in_flight", "callback_failed"}:
            raise ValueError("Generation attempt is not awaiting reconciliation")
        return self._collect_attempt(attempt, receipt)

    async def extract_atlas_views(self, args: Record) -> ToolResult:
        jsonschema.validate(args, self._schemas()["extract_atlas_views"])
        async with self._lock:
            self._require_mutable()
            if self.crop_count + len(args["views"]) > self.max_crops:
                raise ValueError("Atlas extraction limit exhausted")
            atlas = self.asset(args["asset_id"])
            if atlas["purpose"] != "part_atlas":
                raise ValueError("Exact part-view extraction requires a registered part atlas")
            pairs = [(item["role"], item["view"]) for item in args["views"]]
            if len(pairs) != len(set(pairs)):
                raise ValueError("One extraction cannot label duplicate role/view pairs")
            payload, facts = self._image(atlas["source"])
            for item in args["views"]:
                left, top, right, bottom = item["box"]
                if item["role"] not in atlas["roles"]:
                    raise ValueError("Crop role is not declared by its source atlas")
                if not (
                    0 <= left < right <= facts["width"]
                    and 0 <= top < bottom <= facts["height"]
                    and (min(right - left, bottom - top) >= 64)
                ):
                    raise ValueError(
                        "Crop box must be inside the image with at least 64 pixels per side"
                    )
            records = []
            with Image.open(BytesIO(payload)) as image:
                image.load()
                for item in args["views"]:
                    cropped = image.crop(tuple(item["box"]))
                    stream = BytesIO()
                    cropped.save(stream, format="PNG")
                    path = (
                        self.directory
                        / "assets"
                        / f"ref_{self.asset_counter + 1:04d}"
                        / "image.png"
                    )
                    pixels = stream.getvalue()
                    lineage = {
                        "kind": "atlas_crop",
                        "box": list(item["box"]),
                        "coordinate_system": "stored_image_pixels_top_left_half_open_xyxy",
                        "pixel_operation": (
                            "Pillow crop only; decoded pixels preserved without repainting "
                            "or resizing"
                        ),
                        "inputs": [
                            {
                                "asset_id": atlas["asset_id"],
                                "source_sha256": atlas["source"]["sha256"],
                                "rights_basis": atlas["rights_basis"],
                            }
                        ],
                    }
                    record = self._commit_asset(
                        {
                            "source": {
                                "path": self._ref(path),
                                "sha256": hashlib.sha256(pixels).hexdigest(),
                            },
                            "provenance": None,
                        },
                        purpose="part_view",
                        roles=[item["role"]],
                        view=item["view"],
                        rights_basis=atlas["rights_basis"],
                        lineage=lineage,
                        extracted_bytes=pixels,
                    )
                    self.crop_count += 1
                    self._persist()
                    records.append(record)
            return ToolResult(
                json.dumps(
                    {
                        "assets": records,
                        "semantic_status": "unreviewed",
                        "next_check": (
                            "Inspect each extracted image; integer crop validity does not p"
                            "rove the role or view label."
                        ),
                    }
                )
            )

    def freeze_bundle(
        self, canonical_asset_id: str, selections: list[Record], criteria: list[str]
    ) -> Record:
        if self._busy:
            raise ValueError("Cannot submit references during generation")
        self._require_mutable()
        canonical = self.asset(canonical_asset_id)
        if canonical["purpose"] != "canonical":
            raise ValueError("Bundle canonical must be a canonical image asset")
        if (
            not isinstance(criteria, list)
            or not 1 <= len(criteria) <= 64
            or len(set(criteria)) != len(criteria)
        ):
            raise ValueError("Review criteria must be a bounded unique list")
        for criterion in criteria:
            _text(criterion, "Review criterion", 2000)
        if not isinstance(selections, list) or len(selections) > len(self.roles) * len(VIEWS):
            raise ValueError("Part-view selections must be a bounded list")
        parts: dict[str, Record] = {role: {"views": []} for role in self.roles}
        pairs = set()
        for selection in selections:
            if not isinstance(selection, dict) or set(selection) != {"role", "view", "asset_id"}:
                raise ValueError("Selections require exact role, view and asset_id")
            role, view = (selection["role"], selection["view"])
            asset = self.asset(selection["asset_id"])
            if role not in parts or view not in VIEWS or (role, view) in pairs:
                raise ValueError("Unknown or duplicate selected role/view")
            if asset["purpose"] != "part_view" or asset["roles"] != [role] or asset["view"] != view:
                raise ValueError("Selected role/view disagrees with the immutable asset label")
            pairs.add((role, view))
            parts[role]["views"].append(
                {
                    "asset_id": asset["asset_id"],
                    "view": view,
                    **{key: asset[key] for key in ("source", "rights_basis", "provenance")},
                }
            )
        if any((role, view) not in pairs for role in self.roles for view in ("front", "back")):
            raise ValueError("Every declared part needs a selected front and back view")
        for part in parts.values():
            part["views"].sort(key=lambda item: VIEWS.index(item["view"]))
        bundle_id = f"ref_bundle_{len(self.bundles) + 1:03d}"
        payload = {
            "schema_version": 1,
            "bundle_id": bundle_id,
            "profile_sha256": self.profile_sha256,
            "criteria": list(criteria),
            "criteria_sha256": canonical_digest(criteria),
            "canonical": {
                "asset_id": canonical["asset_id"],
                **{key: canonical[key] for key in ("source", "rights_basis", "provenance")},
            },
            "parts": parts,
            "semantic_status": "unreviewed",
        }
        path = self.directory / "bundles" / f"{bundle_id}.json"
        write_json(path, payload)
        result = {
            **payload,
            "bundle_sha256": canonical_digest(payload),
            "manifest": self._descriptor(path),
        }
        self.bundles[bundle_id] = result
        self.frozen_bundle_id = bundle_id
        self._persist()
        return copy.deepcopy(result)

    def _verified_bundle(self, bundle: Record) -> Record:
        payload = {
            key: value for key, value in bundle.items() if key not in {"bundle_sha256", "manifest"}
        }
        manifest = verified_input(self.input_root, bundle["manifest"])
        if (
            canonical_digest(payload) != bundle["bundle_sha256"]
            or json.loads(manifest.read_text()) != payload
        ):
            raise ValueError("Reference bundle digest or persisted manifest differs")
        descriptors = [
            bundle["canonical"],
            *(view for part in bundle["parts"].values() for view in part["views"]),
        ]
        for descriptor in descriptors:
            asset = self.asset(descriptor["asset_id"])
            if any(
                descriptor[key] != asset[key] for key in ("source", "rights_basis", "provenance")
            ):
                raise ValueError("Reference bundle no longer matches immutable assets")
        return copy.deepcopy(bundle)

    def begin_revision(self, expected_bundle_sha256: str) -> None:
        """Host-only release after separate review; old bundles remain immutable."""
        if self._busy or self.frozen_bundle_id is None:
            raise ValueError("No idle frozen reference bundle can begin a revision")
        bundle = self._verified_bundle(self.bundles[self.frozen_bundle_id])
        if bundle["bundle_sha256"] != expected_bundle_sha256:
            raise ValueError("Revision request targets a different frozen bundle")
        self.frozen_bundle_id = None
        self._persist()

    def tools(self, *, read_only: bool = False) -> tuple[Tool, ...]:
        descriptions = {
            "list_references": (
                "List immutable reference versions, roles, image dimensions, ri"
                "ghts and hashes. No semantic acceptance is implied."
            ),
            "inspect_reference": (
                "Inspect one full-resolution registered PNG/JPEG image. Its has"
                "h and raw stored-pixel dimensions ground atlas crop coordinate"
                "s."
            ),
            "inspect_reference_bundle": (
                "Read the complete submitted canonical/part-view manifest and i"
                "ts hash-bound review criteria. Inspect every listed image befo"
                "re independent review."
            ),
            "generate_reference": (
                "Request one original canonical, isolated-part atlas, or isolat"
                "ed named part view through the host's bounded generation callb"
                "ack. Requires explicit canonical input for part generation. Ea"
                "ch invocation consumes one semantic revision; no hidden provid"
                "er retry is performed here. Returned images remain unreviewed."
            ),
            "extract_atlas_views": (
                "Extract exact integer pixel rectangles from an existing part a"
                "tlas into immutable named part views. Boxes are top-left-origi"
                "n half-open [left,top,right,bottom]. This crops only; it canno"
                "t repaint, resize or repair anatomy. Labels still require inde"
                "pendent semantic review."
            ),
        }
        names: tuple[str, ...] = (
            "list_references",
            "inspect_reference",
            "inspect_reference_bundle",
        )
        if not read_only:
            names += ("generate_reference", "extract_atlas_views")
        tools = []
        for name in names:
            schema = self._schemas()[name]
            method = cast(ToolMethod, getattr(self, name))

            async def invoke(
                args: Mapping[str, object], *, schema: Record = schema, method: ToolMethod = method
            ) -> ToolResult:
                try:
                    jsonschema.validate(dict(args), schema)
                    return await method(dict(args))
                except (ValueError, jsonschema.ValidationError) as error:
                    raise ToolInvocationError(str(error)[:2000]) from None

            tools.append(Tool(name, descriptions[name], schema, handler=invoke))
        return tuple(tools)
