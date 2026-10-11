"""Portable contracts for bounded fixed-portrait eye and mouth replacement."""

from __future__ import annotations

from typing import Literal, Self

from pydantic import BaseModel, ConfigDict, Field, model_validator

FeatureId = Literal["canvas_left_eye", "canvas_right_eye", "mouth"]
FeatureGroup = Literal["eyes", "mouth"]
TerminalStatus = Literal["complete", "partial", "refused"]
RefusingStage = Literal["face", "admission", "registration", "geometry", "quality"]


class Contract(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True, frozen=True, allow_inf_nan=False)


class MotionState(Contract):
    state_id: str = Field(pattern=r"^[a-z][a-z0-9_]{0,31}$")
    feature_group: FeatureGroup
    instruction: str = Field(min_length=1, max_length=1000)


class PlaybackSegment(Contract):
    eyes: str
    mouth: str
    duration_ms: int = Field(ge=20, le=10000)


class PortraitMotionSpec(Contract):
    schema_version: Literal[1] = 1
    width: int = Field(ge=128, le=2048)
    height: int = Field(ge=128, le=2048)
    columns: int = Field(ge=1, le=4)
    rows: int = Field(ge=1, le=4)
    states: list[MotionState] = Field(min_length=1, max_length=4)
    requested_features: list[FeatureId] = Field(min_length=1, max_length=3)
    feather_panel_px: float = Field(default=3.0, ge=0, le=8)
    playback: list[PlaybackSegment] = Field(min_length=1, max_length=128)

    @model_validator(mode="after")
    def consistent_layout(self) -> Self:
        if self.width % self.columns or self.height % self.rows:
            raise ValueError("Canvas dimensions must divide evenly into the declared grid")
        if self.columns * self.rows != len(self.states):
            raise ValueError("Every grid cell requires exactly one declared state")
        names = [state.state_id for state in self.states]
        if len(set(names)) != len(names) or "rest" in names:
            raise ValueError("State IDs must be unique and cannot replace the source rest state")
        if len(set(self.requested_features)) != len(self.requested_features):
            raise ValueError("Requested features must be unique")
        groups = {"eyes": {"rest"}, "mouth": {"rest"}}
        for state in self.states:
            groups[state.feature_group].add(state.state_id)
        for feature in self.requested_features:
            group = "mouth" if feature == "mouth" else "eyes"
            if len(groups[group]) == 1:
                raise ValueError("Every requested feature group requires an animation state")
        for segment in self.playback:
            if segment.eyes not in groups["eyes"] or segment.mouth not in groups["mouth"]:
                raise ValueError("Playback references an undeclared or wrong-group state")
        if sum(segment.duration_ms for segment in self.playback) > 60000:
            raise ValueError("Preview duration exceeds sixty seconds")
        if any(
            (self.playback[i].eyes, self.playback[i].mouth) != ("rest", "rest") for i in (0, -1)
        ):
            raise ValueError("Playback must begin and end at the unchanged source")
        return self

    @property
    def panel_size(self) -> tuple[int, int]:
        return self.width // self.columns, self.height // self.rows


class PortraitMotionResult(Contract):
    """What a run accepted: every admitted feature, or nothing and the stage that refused."""

    schema_version: Literal[2] = 2
    status: TerminalStatus
    reason: str = Field(min_length=1)
    refused_at: RefusingStage | None
    admitted_features: list[FeatureId]
    accepted_features: list[FeatureId]
    temporal_review: Literal["not_performed"] = "not_performed"
    publication_authorized: Literal[False] = False

    @model_validator(mode="after")
    def consistent_acceptance(self) -> Self:
        for features in (self.admitted_features, self.accepted_features):
            if len(features) != len(set(features)):
                raise ValueError("Terminal feature IDs must be unique")
        if self.status in {"complete", "partial"}:
            if (
                self.refused_at is not None
                or not self.accepted_features
                or self.accepted_features != self.admitted_features
            ):
                raise ValueError("Accepted output requires every admitted feature and no refusal")
        elif self.refused_at is None or self.accepted_features:
            raise ValueError("A refused output names its stage and accepts nothing")
        return self
