"""Independently selected interface artwork presets and reference bindings."""

from __future__ import annotations

from pathlib import PurePosixPath
from typing import Literal

from pydantic import Field, field_validator, model_validator

from ui_example.atlas import (
    ATLAS_ALPHA_POLICY,
    BUTTON_RECT_LAYOUT,
    PANEL_FRAME_LAYOUT,
)
from ui_example.contracts import PersistedContractModel
from ui_example.cursors import CURSOR_ALPHA_POLICY, CURSOR_SET_LAYOUT
from ui_example.icons import ICON_ALPHA_POLICY, PREVIEW_ICONS_LAYOUT
from ui_example.input_contracts import (
    SHA256_PATTERN,
    SNAKE_ID_PATTERN,
    normalized_text,
    parse_toml_contract,
    portable_relative_path,
    unique_values,
)

UI_ART_SCHEMA_VERSION = 1

INVENTORY_PANEL_LAYOUT = "inventory_grid_4x2_v1"
INVENTORY_PANEL_ALPHA_POLICY = "transparent_exterior_opaque_panel_v1"
INVENTORY_CANVAS_WIDTH = 1536
INVENTORY_CANVAS_HEIGHT = 1024
INVENTORY_PANEL_LEFT = 128
INVENTORY_PANEL_TOP = 160
INVENTORY_PANEL_WIDTH = 1280
INVENTORY_PANEL_HEIGHT = 704
INVENTORY_SLOT_LEFT = 208
INVENTORY_SLOT_TOP = 240
INVENTORY_SLOT_SIZE = 256
INVENTORY_SLOT_GUTTER = 32
INVENTORY_SLOT_COLUMNS = 4
INVENTORY_SLOT_ROWS = 2


class UiReference(PersistedContractModel):
    """One digest-bound visual reference selected by a UI role."""

    reference_id: str = Field(pattern=SNAKE_ID_PATTERN, max_length=96)
    source: str
    source_sha256: str = Field(pattern=SHA256_PATTERN)
    rights_status: Literal["unreviewed", "restricted", "redistribution-approved"]
    rights_basis: list[str] = Field(min_length=1, max_length=16)

    @field_validator("source")
    @classmethod
    def validate_source(cls, value: str) -> str:
        source = portable_relative_path(value, "UI reference source")
        if not source.startswith(("references/", ".showcase-inputs/", ".fixture/")):
            raise ValueError(
                "UI references must live under references/, .showcase-inputs/ or .fixture/"
            )
        if PurePosixPath(source).suffix.lower() not in {".png", ".jpg", ".jpeg", ".webp"}:
            raise ValueError("UI references must use PNG, JPEG, or WebP")
        return source

    @field_validator("rights_basis")
    @classmethod
    def validate_rights_basis(cls, value: list[str]) -> list[str]:
        normalized = [normalized_text(entry, "UI reference rights basis") for entry in value]
        unique_values(normalized, "UI reference rights basis")
        return normalized


class InventoryPanelDirection(PersistedContractModel):
    """Presentation inputs for the current fixed eight-slot inventory panel."""

    layout: Literal["inventory_grid_4x2_v1"]
    alpha_policy: Literal["transparent_exterior_opaque_panel_v1"]
    reference_ids: list[str] = Field(min_length=1, max_length=16)
    prompt: str

    @field_validator("reference_ids")
    @classmethod
    def validate_reference_ids(cls, value: list[str]) -> list[str]:
        unique_values(value, "inventory_panel.reference_ids")
        return value

    @field_validator("prompt")
    @classmethod
    def validate_prompt(cls, value: str) -> str:
        return normalized_text(value, "inventory_panel.prompt", multiline=True)


class AtlasRoleDirection(PersistedContractModel):
    """Presentation inputs for one nine-slice atlas role.

    The layout id is the whole geometry contract: a role never authors cell rectangles or
    insets, because the producer renders its template from the declared geometry and the
    validate node publishes the *detected* geometry beside the artifact.
    """

    layout: str = Field(pattern=SNAKE_ID_PATTERN, max_length=96)
    alpha_policy: Literal["transparent_exterior_opaque_body_v1"]
    reference_ids: list[str] = Field(min_length=1, max_length=16)
    prompt: str

    @field_validator("reference_ids")
    @classmethod
    def validate_reference_ids(cls, value: list[str]) -> list[str]:
        unique_values(value, "atlas role reference_ids")
        return value

    @field_validator("prompt")
    @classmethod
    def validate_prompt(cls, value: str) -> str:
        return normalized_text(value, "atlas role prompt", multiline=True)


class IconSetDirection(PersistedContractModel):
    """Presentation inputs for the fixed preview icon set.

    The glyphs, their order and the grid are the layout's, not the author's: an image model
    draws named, well-known symbols dependably and bespoke ones not, so a game may say how
    the set should look and nothing about what is in it. The prompt is style direction only.
    """

    layout: str = Field(pattern=SNAKE_ID_PATTERN, max_length=96)
    alpha_policy: Literal["transparent_exterior_opaque_glyph_v1"]
    reference_ids: list[str] = Field(min_length=1, max_length=16)
    prompt: str

    @field_validator("reference_ids")
    @classmethod
    def validate_reference_ids(cls, value: list[str]) -> list[str]:
        unique_values(value, "preview_icons.reference_ids")
        return value

    @field_validator("prompt")
    @classmethod
    def validate_prompt(cls, value: str) -> str:
        return normalized_text(value, "preview_icons.prompt", multiline=True)


class CursorSetDirection(PersistedContractModel):
    """Presentation inputs for the fixed cursor set.

    Like the icon set, the pointers, their order, the grid and the hotspot rule per pointer
    are the layout's; the prompt is style direction only. Optional for every game: one whose
    host draws a pointer declares the set, and one played by touch or by a controller omits
    it rather than authoring a picture nothing shows (decision 0062).
    """

    layout: str = Field(pattern=SNAKE_ID_PATTERN, max_length=96)
    alpha_policy: Literal["transparent_exterior_opaque_glyph_v1"]
    reference_ids: list[str] = Field(min_length=1, max_length=16)
    prompt: str

    @field_validator("reference_ids")
    @classmethod
    def validate_reference_ids(cls, value: list[str]) -> list[str]:
        unique_values(value, "cursor_set.reference_ids")
        return value

    @field_validator("prompt")
    @classmethod
    def validate_prompt(cls, value: str) -> str:
        return normalized_text(value, "cursor_set.prompt", multiline=True)


ATLAS_ROLE_LAYOUTS: dict[str, str] = {
    "panel_frame": PANEL_FRAME_LAYOUT,
    "button_rect": BUTTON_RECT_LAYOUT,
}

#: Every sheet role a document must declare, with the layout each is pinned to.
UI_SHEET_LAYOUTS: dict[str, str] = {**ATLAS_ROLE_LAYOUTS, "preview_icons": PREVIEW_ICONS_LAYOUT}

#: The sheet roles a document may declare, with the layout each is pinned to.
OPTIONAL_SHEET_LAYOUTS: dict[str, str] = {"cursor_set": CURSOR_SET_LAYOUT}


class UiArtwork(PersistedContractModel):
    """One or more independently selected UI artwork presets, without gameplay bindings."""

    references: list[UiReference] = Field(min_length=1, max_length=32)
    inventory_panel: InventoryPanelDirection | None = None
    panel_frame: AtlasRoleDirection | None = None
    button_rect: AtlasRoleDirection | None = None
    preview_icons: IconSetDirection | None = None
    cursor_set: CursorSetDirection | None = None

    def required_inventory_panel(self) -> InventoryPanelDirection:
        """The panel, for a recipe whose runtime draws it and cannot proceed without it."""

        if self.inventory_panel is None:
            raise ValueError("this UI document declares no inventory panel")
        return self.inventory_panel

    @model_validator(mode="after")
    def validate_sheet_layouts(self) -> UiArtwork:
        for role, expected in ATLAS_ROLE_LAYOUTS.items():
            direction = getattr(self, role)
            if direction is None:
                continue
            if direction.layout != expected:
                raise ValueError(f"{role}.layout must be {expected!r}, got {direction.layout!r}")
            if direction.alpha_policy != ATLAS_ALPHA_POLICY:
                raise ValueError(f"{role}.alpha_policy must be {ATLAS_ALPHA_POLICY!r}")
        if self.preview_icons is not None and self.preview_icons.layout != PREVIEW_ICONS_LAYOUT:
            raise ValueError(
                f"preview_icons.layout must be {PREVIEW_ICONS_LAYOUT!r}, "
                f"got {self.preview_icons.layout!r}"
            )
        if self.preview_icons is not None and self.preview_icons.alpha_policy != ICON_ALPHA_POLICY:
            raise ValueError(f"preview_icons.alpha_policy must be {ICON_ALPHA_POLICY!r}")
        if self.cursor_set is not None:
            if self.cursor_set.layout != CURSOR_SET_LAYOUT:
                raise ValueError(
                    f"cursor_set.layout must be {CURSOR_SET_LAYOUT!r}, "
                    f"got {self.cursor_set.layout!r}"
                )
            if self.cursor_set.alpha_policy != CURSOR_ALPHA_POLICY:
                raise ValueError(f"cursor_set.alpha_policy must be {CURSOR_ALPHA_POLICY!r}")
        return self

    @model_validator(mode="after")
    def validate_reference_closure(self) -> UiArtwork:
        if not any(
            getattr(self, role) is not None
            for role in ("inventory_panel", *UI_SHEET_LAYOUTS, *OPTIONAL_SHEET_LAYOUTS)
        ):
            raise ValueError("UI art must request at least one artwork role")
        unique_values((entry.reference_id for entry in self.references), "UI reference_id")
        unique_values((entry.source for entry in self.references), "UI reference source")
        declared = {entry.reference_id for entry in self.references}
        selected: set[str] = set()
        for role in ("inventory_panel", *UI_SHEET_LAYOUTS, *OPTIONAL_SHEET_LAYOUTS):
            direction = getattr(self, role)
            if direction is None:
                continue
            unknown = sorted(set(direction.reference_ids) - declared)
            if unknown:
                raise ValueError(f"{role} references unknown IDs: " + ", ".join(unknown))
            selected.update(direction.reference_ids)
        unused = sorted(declared - selected)
        if unused:
            raise ValueError("UI declares unused reference IDs: " + ", ".join(unused))
        return self


class UiArt(UiArtwork):
    """Versioned independent artwork request."""

    schema_version: Literal[1] = 1
    kind: Literal["ui-art-v1"] = "ui-art-v1"


def inventory_panel_layout_contract() -> dict[str, object]:
    """Project the fixed V1 layout without making the web consumer infer geometry."""

    slots = []
    for row in range(INVENTORY_SLOT_ROWS):
        for column in range(INVENTORY_SLOT_COLUMNS):
            slots.append(
                {
                    "slot_id": f"slot_{row * INVENTORY_SLOT_COLUMNS + column}",
                    "x": INVENTORY_SLOT_LEFT
                    + column * (INVENTORY_SLOT_SIZE + INVENTORY_SLOT_GUTTER),
                    "y": INVENTORY_SLOT_TOP + row * (INVENTORY_SLOT_SIZE + INVENTORY_SLOT_GUTTER),
                    "width": INVENTORY_SLOT_SIZE,
                    "height": INVENTORY_SLOT_SIZE,
                }
            )
    return {
        "layout": INVENTORY_PANEL_LAYOUT,
        "alpha_policy": INVENTORY_PANEL_ALPHA_POLICY,
        "canvas": {"width": INVENTORY_CANVAS_WIDTH, "height": INVENTORY_CANVAS_HEIGHT},
        "panel_bounds": {
            "x": INVENTORY_PANEL_LEFT,
            "y": INVENTORY_PANEL_TOP,
            "width": INVENTORY_PANEL_WIDTH,
            "height": INVENTORY_PANEL_HEIGHT,
        },
        "slots": slots,
    }


def load_ui_art_bytes(data: bytes) -> UiArt:
    return parse_toml_contract(data, model=UiArt, label="ui art contract")


__all__ = [
    "ATLAS_ROLE_LAYOUTS",
    "UI_ART_SCHEMA_VERSION",
    "INVENTORY_CANVAS_HEIGHT",
    "INVENTORY_CANVAS_WIDTH",
    "INVENTORY_PANEL_ALPHA_POLICY",
    "INVENTORY_PANEL_HEIGHT",
    "INVENTORY_PANEL_LAYOUT",
    "INVENTORY_PANEL_LEFT",
    "INVENTORY_PANEL_TOP",
    "INVENTORY_PANEL_WIDTH",
    "INVENTORY_SLOT_COLUMNS",
    "INVENTORY_SLOT_GUTTER",
    "INVENTORY_SLOT_LEFT",
    "INVENTORY_SLOT_ROWS",
    "INVENTORY_SLOT_SIZE",
    "INVENTORY_SLOT_TOP",
    "OPTIONAL_SHEET_LAYOUTS",
    "UI_SHEET_LAYOUTS",
    "AtlasRoleDirection",
    "CursorSetDirection",
    "UiArtwork",
    "UiArt",
    "IconSetDirection",
    "InventoryPanelDirection",
    "UiReference",
    "inventory_panel_layout_contract",
    "load_ui_art_bytes",
]
