"""Shared primitives for exact-current prepared game input contracts."""

from __future__ import annotations

import hashlib
import json
import re
import tomllib
import unicodedata
from collections.abc import Iterable
from datetime import date, datetime, time
from pathlib import PurePosixPath
from typing import TypeVar

from pydantic import BaseModel, ValidationError

ContractT = TypeVar("ContractT", bound=BaseModel)

GAME_ID_PATTERN = r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$"
KEBAB_ID_PATTERN = GAME_ID_PATTERN
SNAKE_ID_PATTERN = r"^[a-z][a-z0-9]*(?:_[a-z0-9]+)*$"
#: A package id in either house shape. Game contracts name themselves in kebab and
#: rooms in snake, and a document shared by both — the UI contract is the first —
#: must be able to state which package it belongs to without renaming either.
PACKAGE_ID_PATTERN = r"^[a-z][a-z0-9]*(?:[-_][a-z0-9]+)*$"
SHA256_PATTERN = r"^[a-f0-9]{64}$"


class AuthoredContractLoadError(ValueError):
    """Raised when one prepared-package TOML source is not exact-current."""


def normalized_text(value: str, label: str, *, multiline: bool = False) -> str:
    """Return NFC text and reject blank or padded scalar authoring.

    TOML multiline strings naturally carry one final newline in this repository. For those
    fields the boundary removes surrounding whitespace once; scalar identity and display fields
    remain exact and must already be trimmed.
    """

    normalized = unicodedata.normalize("NFC", value)
    if multiline:
        normalized = normalized.strip()
    if not normalized or (not multiline and normalized != normalized.strip()):
        raise ValueError(f"{label} must be a non-empty trimmed string")
    return normalized


def portable_relative_path(value: str, label: str) -> str:
    normalized = normalized_text(value, label)
    if "\\" in normalized or ":" in normalized:
        raise ValueError(f"{label} must be a portable relative path")
    path = PurePosixPath(normalized)
    if path.is_absolute() or normalized != path.as_posix():
        raise ValueError(f"{label} must be a normalized portable relative path")
    if not path.parts or any(part in {"", ".", ".."} for part in path.parts):
        raise ValueError(f"{label} must not contain empty, dot, or parent segments")
    return normalized


def unique_values(values: Iterable[str], label: str) -> None:
    collected = list(values)
    if len(set(collected)) != len(collected):
        raise ValueError(f"{label} values must be unique")


def parse_toml_contract(
    data: bytes,
    *,
    model: type[ContractT],
    label: str,
) -> ContractT:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise AuthoredContractLoadError(f"invalid {label} UTF-8: {error}") from error
    try:
        value: object = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        raise AuthoredContractLoadError(f"invalid {label} syntax: {error}") from error
    _reject_temporal_values(value, label)
    try:
        return model.model_validate(value)
    except ValidationError as error:
        raise AuthoredContractLoadError(f"invalid {label}: {error}") from error


def canonical_contract_json(contract: BaseModel) -> bytes:
    return json.dumps(
        contract.model_dump(mode="json", exclude_none=True),
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _reject_temporal_values(value: object, location: str) -> None:
    if isinstance(value, (datetime, date, time)):
        raise AuthoredContractLoadError(
            f"{location} must use a string instead of a TOML native date/time value"
        )
    if isinstance(value, dict):
        for key, item in value.items():
            _reject_temporal_values(item, f"{location}.{key}")
    elif isinstance(value, list):
        for index, item in enumerate(value):
            _reject_temporal_values(item, f"{location}[{index}]")


def model_field_names(model: type[BaseModel]) -> frozenset[str]:
    """Return persisted snake-case field names for documentation and closure checks."""

    return frozenset(model.model_fields)


def assert_pattern(value: str, pattern: str, label: str) -> str:
    if re.fullmatch(pattern, value) is None:
        raise ValueError(f"{label} has an invalid identifier")
    return value


__all__ = [
    "AuthoredContractLoadError",
    "GAME_ID_PATTERN",
    "KEBAB_ID_PATTERN",
    "PACKAGE_ID_PATTERN",
    "SHA256_PATTERN",
    "SNAKE_ID_PATTERN",
    "assert_pattern",
    "canonical_contract_json",
    "model_field_names",
    "normalized_text",
    "parse_toml_contract",
    "portable_relative_path",
    "sha256_bytes",
    "unique_values",
]
