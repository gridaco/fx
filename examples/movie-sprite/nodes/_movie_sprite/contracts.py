"""Strict example-owned settings base, adapted under the adjacent BSD license."""

from pydantic import BaseModel, ConfigDict


class PersistedContractModel(BaseModel):
    model_config = ConfigDict(extra="forbid", populate_by_name=True, strict=True)
