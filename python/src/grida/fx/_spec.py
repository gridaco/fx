"""Node type declarations (``spec/protocol.md`` section 4).

A node type declares its inputs (files, as port notation), params (JSON Schemas), outputs, whether
it judges, the paid calls it makes at most, the project files it reads, the programs it runs, a
version (``None``: identified by its export and its source) and its retry mode. The SDK expands
shorthand:

- a param given as ``bool``/``int``/``float``/``str``/``list``/``dict`` becomes
  ``{"type": "boolean"|"integer"|"number"|"string"|"array"|"object"}``; a tuple of choices
  ``{"enum": [...]}``; a mapping is a JSON Schema used as is, except that a boolean ``optional``
  key is written ``x-fx-optional`` and ``template`` ``x-fx-template`` (the predecessor's spelling);
- :func:`param` builds one: ``param(str, optional=True)``, ``param(str, template=True)``,
  ``param(int, default=3, minimum=1, maximum=8)``.

Declaring validates what the engine validates (names, ports, call bounds, resources, tools) and
raises :class:`SpecError`; an import of a module whose declaration is invalid then fails with that
error, which ``describe`` reports. The spec is kept on the body as ``body.fx_spec``.
"""

from __future__ import annotations

import inspect
import re
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Literal

from grida.fx._protocol import check_value

#: Port families (``spec/protocol.md`` section 4).
FAMILIES = frozenset({"image", "audio", "video", "model", "text", "json", "annotations", "file"})

Shape = Literal["one", "list", "keyed"]

_PORT = re.compile(r"(?P<kind>[a-z0-9]+(?:/[a-z0-9.+-]+)?)(?P<shape>\[\]|\{\})?(?P<optional>\?)?")
_TYPE_NAME = re.compile(r"[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*")
_NAME = re.compile(r"[a-z][a-z0-9_]*")
#: A project path: POSIX, relative, with no empty, ``.`` or ``..`` segment and no backslash.
_SEGMENT = r"(?:[^/\\.][^/\\]*|\.[^/\\.][^/\\]*|\.\.[^/\\]+)"
_PROJECT_PATH = re.compile(_SEGMENT + r"(?:/" + _SEGMENT + r")*")
_TOOL = re.compile(r"[a-z0-9][a-z0-9_-]*(?:\s*[<>=!~].*)?")
_SHAPES: dict[str | None, Shape] = {None: "one", "[]": "list", "{}": "keyed"}
_SUFFIXES: dict[str, str] = {"one": "", "list": "[]", "keyed": "{}"}
_SIMPLE: dict[type, str] = {
    bool: "boolean",
    int: "integer",
    float: "number",
    str: "string",
    list: "array",
    dict: "object",
}
_RENAMED_KEYS = {"optional": "x-fx-optional", "template": "x-fx-template"}


class SpecError(ValueError):
    """A node type declaration that cannot be planned against."""


@dataclass(frozen=True, slots=True)
class PortSpec:
    """One input or output: a file kind, how many, and whether it may be absent."""

    kind: str
    shape: Shape = "one"
    optional: bool = False

    @classmethod
    def parse(cls, notation: str) -> PortSpec:
        """Parses ``kind``, ``kind[]``, ``kind{}`` with an optional ``?``."""
        match = _PORT.fullmatch(notation) if isinstance(notation, str) else None
        if match is None:
            raise SpecError(f"port {notation!r} is not kind, kind[], kind{{}} with an optional ?")
        kind = match["kind"]
        if kind.split("/", 1)[0] not in FAMILIES:
            raise SpecError(f"port kind {kind!r} is not one of {sorted(FAMILIES)}")
        return cls(kind, _SHAPES[match["shape"]], match["optional"] is not None)

    @property
    def family(self) -> str:
        return self.kind.split("/", 1)[0]

    def notation(self) -> str:
        """The port notation."""
        return f"{self.kind}{_SUFFIXES[self.shape]}{'?' if self.optional else ''}"


def param(
    declared: Any = str,
    *,
    optional: bool = False,
    template: bool = False,
    default: Any = ...,
    **schema: Any,
) -> dict[str, Any]:
    """A param schema from shorthand plus JSON Schema keywords (``minimum=1``)."""
    built = param_schema(declared)
    built.update(schema)
    if default is not ...:
        built["default"] = default
    if optional:
        built["x-fx-optional"] = True
    if template:
        built["x-fx-template"] = True
    return built


def param_schema(declared: Any) -> dict[str, Any]:
    """A param declaration as JSON Schema (module docstring)."""
    if isinstance(declared, Mapping):
        built: dict[str, Any] = {}
        for key, value in declared.items():
            renamed = _RENAMED_KEYS.get(key) if isinstance(value, bool) else None
            if renamed is not None:
                if renamed in declared and declared[renamed] != value:
                    raise SpecError(f"param schema sets both {key} and {renamed}")
                built[renamed] = value
            elif key in built and key in _RENAMED_KEYS.values():
                continue  # already written from the predecessor's spelling, with the same value
            else:
                built[key] = value
        return built
    if isinstance(declared, tuple):
        return {"enum": list(declared)}
    if isinstance(declared, type) and declared in _SIMPLE:
        return {"type": _SIMPLE[declared]}
    raise SpecError(f"param declaration {declared!r} is a type, a tuple of choices or a schema")


def _is_whole(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_number(value: Any) -> bool:
    return isinstance(value, int | float) and not isinstance(value, bool)


@dataclass(frozen=True)
class NodeSpec:
    """A declared node type."""

    name: str
    inputs: Mapping[str, PortSpec] = field(default_factory=dict)
    params: Mapping[str, dict[str, Any]] = field(default_factory=dict)
    outputs: Mapping[str, PortSpec] = field(default_factory=dict)
    judge: bool = False
    calls: Mapping[str, int | str] = field(default_factory=dict)
    resources: tuple[str, ...] = ()
    tools: tuple[str, ...] = ()
    view: str | None = None
    version: int | None = None
    retry: Literal["service", "engine"] = "service"
    description: str | None = None
    body: Callable[..., Any] | None = field(default=None, compare=False, repr=False)

    def validate(self) -> None:
        """Raises :class:`SpecError` for anything ``spec/protocol.md`` section 4 refuses."""
        if not isinstance(self.name, str) or not _TYPE_NAME.fullmatch(self.name):
            raise SpecError(f"node type name {self.name!r} must be lower_snake words joined by .")
        name = self.name
        for role, entries in (
            ("input", self.inputs),
            ("param", self.params),
            ("output", self.outputs),
        ):
            if not isinstance(entries, Mapping):
                raise SpecError(f"{name}: {role}s map names to declarations")
            for key in entries:
                if not isinstance(key, str) or not _NAME.fullmatch(key):
                    raise SpecError(
                        f"{name}: {role} name {key!r} must be one lower_snake word "
                        "(a letter, then letters, digits or _)"
                    )
        for role, ports in (("input", self.inputs), ("output", self.outputs)):
            for key, port in ports.items():
                if not isinstance(port, PortSpec):
                    raise SpecError(f"{name}: {role} {key!r} is not a port")
                if port.shape not in _SUFFIXES or not isinstance(port.optional, bool):
                    raise SpecError(f"{name}: {role} {key!r} is not a port")
                PortSpec.parse(port.notation())
        overlap = set(self.inputs) & set(self.params)
        if overlap:
            raise SpecError(f"{name}: {sorted(overlap)} declared as both input and param")
        for key, schema in self.params.items():
            self._check_param(key, schema)
        self._check_calls()
        self._check_paths(
            "resource",
            self.resources,
            _PROJECT_PATH,
            "a POSIX path relative to the project root, without ./ or .. segments",
        )
        self._check_paths(
            "tool",
            self.tools,
            _TOOL,
            "a program name, optionally followed by a version bound such as blender>=4.2",
        )
        if not isinstance(self.judge, bool):
            raise SpecError(f"{name}: judge must be True or False")
        if self.view is not None and not isinstance(self.view, str):
            raise SpecError(f"{name}: view must be a string or None")
        if self.version is not None and (not _is_whole(self.version) or self.version < 0):
            raise SpecError(f"{name}: version must be a whole number of at least 0, or None")
        if self.retry not in ("service", "engine"):
            raise SpecError(f"{name}: retry is 'service' or 'engine', not {self.retry!r}")
        if self.description is not None and not isinstance(self.description, str):
            raise SpecError(f"{name}: description must be a string or None")

    def _check_param(self, key: str, schema: Any) -> None:
        if not isinstance(schema, Mapping):
            raise SpecError(f"{self.name}: param {key!r} must be a JSON Schema object")
        for keyword in ("x-fx-optional", "x-fx-template"):
            if keyword in schema and not isinstance(schema[keyword], bool):
                raise SpecError(f"{self.name}: param {key!r}: {keyword} must be true or false")
        try:
            check_value(dict(schema))
        except ValueError as error:
            raise SpecError(f"{self.name}: param {key!r} is not a JSON value: {error}") from None

    def _check_calls(self) -> None:
        if not isinstance(self.calls, Mapping):
            raise SpecError(f"{self.name}: calls map capabilities to bounds")
        for capability, count in self.calls.items():
            if not isinstance(capability, str) or not capability:
                raise SpecError(f"{self.name}: calls are keyed by capability names")
            if isinstance(count, str):
                setting = self.params.get(count, {})
                minimum = setting.get("minimum", 0)
                if (
                    not _NAME.fullmatch(count)
                    or setting.get("type") != "integer"
                    or not _is_number(minimum)
                    or minimum < 1
                ):
                    raise SpecError(
                        f"{self.name}: calls[{capability!r}] names {count!r}, which is not an "
                        "integer setting with a minimum of at least 1"
                    )
                if not _is_number(setting.get("maximum")):
                    raise SpecError(
                        f"{self.name}: calls[{capability!r}] needs {count!r} to set a maximum"
                    )
            elif not _is_whole(count):
                raise SpecError(
                    f"{self.name}: calls[{capability!r}] is a whole number of at least 1 or "
                    "the name of an integer param"
                )
            elif count < 1:
                raise SpecError(f"{self.name}: calls[{capability!r}] must be at least 1")
            else:
                try:
                    check_value(count)
                except ValueError as error:
                    raise SpecError(f"{self.name}: calls[{capability!r}]: {error}") from None

    def _check_paths(
        self, role: str, values: Sequence[str], pattern: re.Pattern[str], what: str
    ) -> None:
        if isinstance(values, str) or not isinstance(values, Sequence):
            raise SpecError(f"{self.name}: {role}s are a list of strings")
        seen: set[str] = set()
        for value in values:
            if not isinstance(value, str) or not pattern.fullmatch(value):
                raise SpecError(f"{self.name}: {role} {value!r} must be {what}")
            if value in seen:
                raise SpecError(f"{self.name}: {role} {value!r} is declared twice")
            seen.add(value)

    def to_type_spec(self) -> dict[str, Any]:
        """The protocol's type spec: ``name``, ``description`` (when set), ``inputs`` and
        ``outputs`` as notation, ``params``, ``judge``, ``calls``, ``resources``, ``tools``,
        ``view``, ``version``, ``retry``."""
        spec: dict[str, Any] = {"name": self.name}
        if self.description is not None:
            spec["description"] = self.description
        spec["inputs"] = {key: port.notation() for key, port in self.inputs.items()}
        spec["params"] = {key: dict(schema) for key, schema in self.params.items()}
        spec["outputs"] = {key: port.notation() for key, port in self.outputs.items()}
        spec["judge"] = self.judge
        spec["calls"] = dict(self.calls)
        spec["resources"] = list(self.resources)
        spec["tools"] = list(self.tools)
        spec["view"] = self.view
        spec["version"] = self.version
        spec["retry"] = self.retry
        return spec


def _sequence(name: str, role: str, values: Sequence[str]) -> tuple[str, ...]:
    if isinstance(values, str):
        raise SpecError(f"{name}: {role} is a list of strings, not one string")
    return tuple(values)


def _ports(name: str, role: str, declared: Mapping[str, str] | None) -> dict[str, PortSpec]:
    if declared is None:
        return {}
    if not isinstance(declared, Mapping):
        raise SpecError(f"{name}: {role} map names to port notation")
    return {key: PortSpec.parse(value) for key, value in declared.items()}


def _description(body: Callable[..., Any]) -> str | None:
    text = getattr(body, "__doc__", None)
    if not isinstance(text, str):
        return None
    cleaned = inspect.cleandoc(text)
    return cleaned if cleaned.strip() else None


def node(
    name: str,
    *,
    inputs: Mapping[str, str] | None = None,
    params: Mapping[str, Any] | None = None,
    outputs: Mapping[str, str] | None = None,
    judge: bool = False,
    calls: Mapping[str, int | str] | None = None,
    resources: Sequence[str] = (),
    tools: Sequence[str] = (),
    view: str | None = None,
    version: int | None = None,
    retry: Literal["service", "engine"] = "service",
) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
    """Declare a node type over a ``def`` or ``async def`` body taking ``ctx``. The body's
    docstring becomes the spec's ``description``."""

    def declare(body: Callable[..., Any]) -> Callable[..., Any]:
        if params is not None and not isinstance(params, Mapping):
            raise SpecError(f"{name}: params map names to declarations")
        if calls is not None and not isinstance(calls, Mapping):
            raise SpecError(f"{name}: calls map capabilities to bounds")
        spec = NodeSpec(
            name=name,
            inputs=_ports(name, "inputs", inputs),
            params={key: param_schema(value) for key, value in (params or {}).items()},
            outputs=_ports(name, "outputs", outputs),
            judge=judge,
            calls=dict(calls or {}),
            resources=_sequence(name, "resources", resources),
            tools=_sequence(name, "tools", tools),
            view=view,
            version=version,
            retry=retry,
            description=_description(body),
            body=body,
        )
        spec.validate()
        body.fx_spec = spec  # type: ignore[attr-defined]
        return body

    return declare


def spec_of(value: Any) -> NodeSpec | None:
    """The node spec a value carries, if it is a declared node type."""
    try:
        spec = getattr(value, "fx_spec", None)
    except BaseException:  # an object whose __getattr__ raises anything: not a node type
        return None
    return spec if isinstance(spec, NodeSpec) else None
