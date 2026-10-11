"""Strict persisted contracts owned by this example."""

from pydantic import BaseModel, ConfigDict


class PersistedContractModel(BaseModel):
    model_config = ConfigDict(extra="forbid", populate_by_name=True, strict=True)
