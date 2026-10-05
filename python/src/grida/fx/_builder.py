"""The Python builder: write a workflow in code, as an ``fx: workflow/v1`` document.

``Workflow`` takes every field a workflow file has, under the same names (``with_``, ``if_``,
``assert_`` and ``as_`` where Python reserves the word), and produces the same document::

    wf = Workflow("level-art", title="Level art")
    plate = wf.step("plate", uses="./nodes/plate.py#ground_plate", with_={"material": "sand"})
    icons = wf.step("icon", for_each=pickups, key="${{ item.id }}", uses="./workflows/icon.yaml",
                    with_={"name": "${{ item.name }}"})
    wf.outputs(plate=plate.outputs.image, icons=icons.all.outputs.icon)

References render as expressions: ``plate.outputs.image`` is ``"${{ steps.plate.outputs.image }}"``,
``icons.all`` is ``steps.icon.*``, ``icons["ada"]`` is ``steps.icon['ada']`` (the key quoted as an
instance path quotes it: backslash and quote escaped). ``Workflow.document()`` returns the
document as a plain ``dict``: ``fx``, ``id``, ``title``, then the optional head fields only when
truthy, then ``steps`` and ``outputs``; no defaults filled in. The engine validates it.
``Workflow.source`` is the file of the frame that called ``Workflow(...)`` (the takes anchor).
"""

from __future__ import annotations

import copy
import math
import sys
from collections.abc import Mapping
from decimal import Decimal
from pathlib import Path
from typing import Any

#: Step and head fields whose names Python reserves.
_RENAMED = {"with_": "with", "if_": "if", "assert_": "assert", "as_": "as"}


def _number_text(value: float) -> str:
    """A number as JSON's canonical form writes it (ECMAScript ``Number.prototype.toString``)."""
    if value == 0:
        return "0"
    sign = "-" if value < 0 else ""
    # repr writes the shortest digits that read back as the same number.
    _, digit_tuple, exponent = Decimal(repr(abs(value))).normalize().as_tuple()
    assert isinstance(exponent, int)
    digits = "".join(str(digit) for digit in digit_tuple)
    k = len(digits)
    n = exponent + k  # the number is 0.<digits> times 10**n
    if k <= n <= 21:
        return sign + digits + "0" * (n - k)
    if 0 < n <= 21:
        return sign + digits[:n] + "." + digits[n:]
    if -6 < n <= 0:
        return sign + "0." + "0" * (-n) + digits
    mantissa = digits[0] + ("." + digits[1:] if k > 1 else "")
    return f"{sign}{mantissa}e{'+' if n > 1 else '-'}{abs(n - 1)}"


def _key_text(key: Any) -> str:
    """A repeat key's text (``spec/identity.md`` section 11): text, a boolean or a number."""
    if isinstance(key, str):
        return key
    if isinstance(key, bool):
        return "true" if key else "false"
    if isinstance(key, int) and abs(key) <= 2**53:
        return str(key)
    if isinstance(key, int | float):
        number = float(key) if abs(key) <= sys.float_info.max else math.inf
        if math.isfinite(number):
            return _number_text(number)
    raise TypeError(f"a step key is text, a number or a boolean, not {type(key).__name__}")


def _quoted(key: Any) -> str:
    text = _key_text(key).replace("\\", "\\\\").replace("'", "\\'")
    return f"['{text}']"


class _Reference:
    """``plate.outputs.image``: an expression string, built by attribute access."""

    def __init__(self, path: str) -> None:
        self._path = path

    def __getattr__(self, name: str) -> _Reference:
        if name.startswith("_"):
            raise AttributeError(name)
        return _Reference(f"{self._path}.{name}")

    def __getitem__(self, key: str) -> _Reference:
        return _Reference(self._path + _quoted(key))

    def __str__(self) -> str:
        return "${{ " + self._path + " }}"

    def __repr__(self) -> str:
        return str(self)


class StepRef:
    """A step added to a builder: refer to its results from later steps and outputs."""

    def __init__(self, path: str) -> None:
        self.path = path

    @property
    def outputs(self) -> _Reference:
        return _Reference(f"steps.{self.path}.outputs")

    @property
    def facts(self) -> _Reference:
        return _Reference(f"steps.{self.path}.facts")

    @property
    def take(self) -> _Reference:
        return _Reference(f"steps.{self.path}.take")

    @property
    def all(self) -> _Reference:
        """Every instance of a repeated step: ``icons.all.outputs.icon``."""
        return _Reference(f"steps.{self.path}.*")

    def __getitem__(self, key: str) -> _Reference:
        return _Reference(f"steps.{self.path}{_quoted(key)}")

    def __getattr__(self, name: str) -> StepRef:
        if name.startswith("_"):
            raise AttributeError(name)
        return StepRef(f"{self.path}.{name}")

    def __repr__(self) -> str:
        return f"StepRef({self.path!r})"


def _plain(value: Any) -> Any:
    if isinstance(value, _Reference):
        return str(value)
    if isinstance(value, Mapping):
        return {str(key): _plain(item) for key, item in value.items()}
    if isinstance(value, list | tuple):
        return [_plain(item) for item in value]
    return value


def _fields(fields: Mapping[str, Any]) -> dict[str, Any]:
    return {_RENAMED.get(name, name): _plain(value) for name, value in fields.items()}


class Group:
    """The steps of a group, added the same way as a workflow's."""

    def __init__(self, path: str) -> None:
        self._path = path
        self._steps: dict[str, dict[str, Any]] = {}

    def step(self, name: str, **fields: Any) -> StepRef:
        """Adds a step; ``ValueError`` when the name is taken."""
        self._claim(name)
        self._steps[name] = _fields(fields)
        return StepRef(f"{self._path}{name}")

    def group(self, name: str, **fields: Any) -> Group:
        """Adds a group step and returns it for its members."""
        self._claim(name)
        inner = Group(f"{self._path}{name}.")
        self._steps[name] = {**_fields(fields), "steps": inner._steps}
        return inner

    def _claim(self, name: str) -> None:
        if name in self._steps:
            raise ValueError(f"{self._path}{name} is declared twice")


class Workflow(Group):
    """A workflow built in Python: the same document a workflow file would be."""

    def __init__(
        self,
        id: str,
        *,
        title: str,
        description: str | None = None,
        inputs: Mapping[str, Any] | None = None,
        tables: Mapping[str, Any] | None = None,
        let: Mapping[str, Any] | None = None,
        budget: Mapping[str, Any] | None = None,
        assert_: list[Mapping[str, Any]] | None = None,
        view: str | None = None,
    ) -> None:
        super().__init__("")
        #: The module that built this workflow: its takes file lives next to it.
        self.source: Path | None = _caller_file()
        self._head: dict[str, Any] = {"fx": "workflow/v1", "id": id, "title": title}
        optional = {
            "description": description,
            "inputs": inputs,
            "tables": tables,
            "let": let,
            "budget": budget,
            "assert": assert_,
            "view": view,
        }
        for name, value in optional.items():
            if value:
                self._head[name] = _plain(value)
        self._outputs: dict[str, Any] = {}

    def outputs(self, **values: Any) -> None:
        self._outputs.update({name: _plain(value) for name, value in values.items()})

    def document(self) -> dict[str, Any]:
        """The ``fx: workflow/v1`` document."""
        return copy.deepcopy({**self._head, "steps": self._steps, "outputs": self._outputs})


def _caller_file() -> Path | None:
    """The file of the frame that called ``Workflow(...)``, when it is a real file."""
    frame = sys._getframe(2)
    name = frame.f_code.co_filename
    try:
        path = Path(name)
        return path.resolve() if path.is_file() else None
    except (OSError, ValueError):
        return None


__all__ = ["Group", "StepRef", "Workflow"]
