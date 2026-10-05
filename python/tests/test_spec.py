"""Node type declarations: shorthand, validation, and the protocol's type spec."""

from __future__ import annotations

import importlib.util
import json
import sys
from collections.abc import Iterator
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
from jsonschema import Draft202012Validator

from grida.fx import NodeSpec, PortSpec, SpecError, node, param, spec_of
from grida.fx._spec import param_schema

REPO = Path(__file__).resolve().parents[2]
PROTOCOL_SCHEMA = json.loads(
    (REPO / "spec" / "schemas" / "fx-node-protocol-v1.schema.json").read_text("utf-8")
)
CONFORMANCE = REPO / "conformance"


def _validator(name: str) -> Draft202012Validator:
    schema = {"$ref": f"#/$defs/{name}", "$defs": PROTOCOL_SCHEMA["$defs"]}
    return Draft202012Validator(schema)


TYPE_SPEC = _validator("type_spec")


def _errors(validator: Draft202012Validator, value: Any) -> list[str]:
    return [error.message for error in validator.iter_errors(value)]


def _load(path: Path, root: Path) -> ModuleType:
    """Loads a fixture module the way the host does, with its project root on sys.path."""
    name = f"_fx_test_{abs(hash(str(path)))}"
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.path.insert(0, str(root))
    try:
        spec.loader.exec_module(module)
    finally:
        sys.path.remove(str(root))
        for loaded in [key for key in sys.modules if key == "nodes" or key.startswith("nodes.")]:
            del sys.modules[loaded]
    return module


def _declared(module: ModuleType) -> Iterator[tuple[str, NodeSpec]]:
    for attribute, value in vars(module).items():
        spec = spec_of(value)
        if spec is not None:
            yield attribute, spec


# --- the conformance fixtures -----------------------------------------------------------------


def test_linear_cases_serialize_to_valid_type_specs() -> None:
    root = CONFORMANCE / "linear" / "in"
    module = _load(root / "nodes" / "cases.py", root)
    specs = dict(_declared(module))
    assert list(specs) == ["shout", "join", "lines", "verdict"]
    for attribute, spec in specs.items():
        type_spec = spec.to_type_spec()
        assert _errors(TYPE_SPEC, type_spec) == [], attribute
        # The wire form is plain JSON.
        assert json.loads(json.dumps(type_spec, allow_nan=False)) == type_spec
    assert specs["shout"].to_type_spec() == {
        "name": "shout",
        "inputs": {},
        "params": {"text": {"type": "string"}},
        "outputs": {"text": "text"},
        "judge": False,
        "calls": {},
        "resources": [],
        "tools": [],
        "view": None,
        "version": 1,
        "retry": "service",
    }
    assert specs["join"].to_type_spec()["inputs"] == {"parts": "text{}"}
    verdict = specs["verdict"].to_type_spec()
    assert verdict["judge"] is True
    assert verdict["params"] == {"accept_take": {"type": "integer"}}
    assert verdict["inputs"] == {"subject": "file"}
    assert list(verdict) == [
        "name",
        "inputs",
        "params",
        "outputs",
        "judge",
        "calls",
        "resources",
        "tools",
        "view",
        "version",
        "retry",
    ]


def _fixture_modules() -> list[Path]:
    return sorted(CONFORMANCE.glob("*/in/nodes/*.py"))


@pytest.mark.parametrize(
    "path", _fixture_modules(), ids=lambda path: path.relative_to(CONFORMANCE).as_posix()
)
def test_every_conformance_node_type_is_a_valid_type_spec(path: Path) -> None:
    module = _load(path, path.parents[1])
    for attribute, spec in _declared(module):
        assert _errors(TYPE_SPEC, spec.to_type_spec()) == [], attribute


def test_local_identity_types_keep_their_resources_and_versions() -> None:
    root = CONFORMANCE / "local-identity" / "in"
    specs = dict(_declared(_load(root / "nodes" / "n.py", root)))
    assert specs["echo"].to_type_spec()["resources"] == ["prompts/r.md"]
    assert specs["echo"].to_type_spec()["version"] is None
    assert specs["pinned"].to_type_spec()["version"] == 2


# --- the decorator ----------------------------------------------------------------------------


def test_the_docstring_becomes_the_description() -> None:
    @node("documented", outputs={"text": "text"})
    def documented(ctx: Any) -> dict:
        """Says hello.

        Twice, on two lines.
        """
        return {}

    spec = spec_of(documented)
    assert spec is not None
    assert spec.description == "Says hello.\n\nTwice, on two lines."
    type_spec = spec.to_type_spec()
    assert list(type_spec)[:2] == ["name", "description"]
    assert type_spec["description"] == "Says hello.\n\nTwice, on two lines."
    assert _errors(TYPE_SPEC, type_spec) == []


@pytest.mark.parametrize("doc", [None, "", "   \n\n   "])
def test_no_docstring_means_no_description(doc: str | None) -> None:
    def body(ctx: Any) -> dict:
        return {}

    body.__doc__ = doc
    declared = node("plain")(body)
    spec = spec_of(declared)
    assert spec is not None and spec.description is None
    assert "description" not in spec.to_type_spec()


def test_node_returns_the_body_with_its_spec() -> None:
    async def body(ctx: Any) -> dict:
        return {}

    declared = node("async_kind", version=0, retry="engine")(body)
    assert declared is body
    spec = spec_of(body)
    assert spec is not None
    assert spec.body is body
    assert spec.version == 0
    assert spec.to_type_spec()["retry"] == "engine"
    assert spec_of(object()) is None
    assert spec_of(None) is None


def test_spec_of_survives_objects_that_raise_on_any_attribute() -> None:
    class Strange:
        def __getattr__(self, name: str) -> Any:
            raise RuntimeError(name)

    assert spec_of(Strange()) is None


def test_a_full_declaration() -> None:
    @node(
        "layer.repaint",
        inputs={"image": "image", "refs": "image[]", "lines": "audio{}", "spec": "json?"},
        params={
            "opaque": bool,
            "strength": float,
            "labels": list,
            "options": dict,
            "count": int,
            "name": str,
            "mode": ("fast", "slow"),
            "steps": param(int, minimum=1, maximum=8, default=2),
            "prompt": param(str, template=True),
            "note": param(str, optional=True),
            "schema": {"type": "object", "optional": True},
        },
        outputs={"image": "image/png", "report": "json", "extra": "file/zip[]?"},
        calls={"image.edit": 1, "agent.turn": "steps"},
        resources=["prompts/seam.md", ".config/x.json", "..data/y"],
        tools=["ffmpeg", "blender>=4.2", "magick ~= 7"],
        view="views/repaint.html",
        version=3,
    )
    def repaint(ctx: Any) -> dict:
        return {}

    spec = spec_of(repaint)
    assert spec is not None
    type_spec = spec.to_type_spec()
    assert _errors(TYPE_SPEC, type_spec) == []
    assert type_spec["inputs"] == {
        "image": "image",
        "refs": "image[]",
        "lines": "audio{}",
        "spec": "json?",
    }
    assert type_spec["outputs"] == {"image": "image/png", "report": "json", "extra": "file/zip[]?"}
    assert type_spec["params"] == {
        "opaque": {"type": "boolean"},
        "strength": {"type": "number"},
        "labels": {"type": "array"},
        "options": {"type": "object"},
        "count": {"type": "integer"},
        "name": {"type": "string"},
        "mode": {"enum": ["fast", "slow"]},
        "steps": {"type": "integer", "minimum": 1, "maximum": 8, "default": 2},
        "prompt": {"type": "string", "x-fx-template": True},
        "note": {"type": "string", "x-fx-optional": True},
        "schema": {"type": "object", "x-fx-optional": True},
    }
    assert type_spec["calls"] == {"image.edit": 1, "agent.turn": "steps"}
    assert type_spec["resources"] == ["prompts/seam.md", ".config/x.json", "..data/y"]
    assert type_spec["tools"] == ["ffmpeg", "blender>=4.2", "magick ~= 7"]
    assert type_spec["view"] == "views/repaint.html"
    assert type_spec["version"] == 3


# --- shorthand --------------------------------------------------------------------------------


def test_param_shorthand() -> None:
    assert param() == {"type": "string"}
    assert param(str, optional=True) == {"type": "string", "x-fx-optional": True}
    assert param(str, template=True) == {"type": "string", "x-fx-template": True}
    assert param(int, default=3, minimum=1, maximum=8) == {
        "type": "integer",
        "minimum": 1,
        "maximum": 8,
        "default": 3,
    }
    assert param(("a", "b"), default=None) == {"enum": ["a", "b"], "default": None}
    assert param({"type": "array", "items": {"type": "string"}}, optional=True) == {
        "type": "array",
        "items": {"type": "string"},
        "x-fx-optional": True,
    }


def test_param_schema_renames_the_predecessors_keys() -> None:
    assert param_schema({"type": "string", "optional": True, "template": False}) == {
        "type": "string",
        "x-fx-optional": True,
        "x-fx-template": False,
    }
    # Only boolean values are the predecessor's flags; anything else is the schema's own.
    assert param_schema({"optional": "maybe"}) == {"optional": "maybe"}
    assert param_schema({"optional": True, "x-fx-optional": True}) == {"x-fx-optional": True}
    with pytest.raises(SpecError, match="sets both optional and x-fx-optional"):
        param_schema({"optional": True, "x-fx-optional": False})
    original = {"type": "string", "optional": True}
    param_schema(original)
    assert original == {"type": "string", "optional": True}


@pytest.mark.parametrize("declared", [None, bytes, object, 3, "string", [str], set])
def test_param_schema_refuses_other_declarations(declared: Any) -> None:
    with pytest.raises(SpecError, match="is a type, a tuple of choices or a schema"):
        param_schema(declared)


# --- ports ------------------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("notation", "port"),
    [
        ("image", PortSpec("image")),
        ("image/png", PortSpec("image/png")),
        ("image[]", PortSpec("image", "list")),
        ("text{}", PortSpec("text", "keyed")),
        ("json?", PortSpec("json", optional=True)),
        ("audio[]?", PortSpec("audio", "list", True)),
        ("model/gltf+json{}?", PortSpec("model/gltf+json", "keyed", True)),
        ("annotations", PortSpec("annotations")),
        ("file/x-a.b-c", PortSpec("file/x-a.b-c")),
    ],
)
def test_port_notation_round_trips(notation: str, port: PortSpec) -> None:
    assert PortSpec.parse(notation) == port
    assert port.notation() == notation


@pytest.mark.parametrize("notation", ["image?[]", "Image", "image/", "image[]{}", "", " image"])
def test_port_refuses_bad_notation(notation: str) -> None:
    with pytest.raises(SpecError) as refused:
        PortSpec.parse(notation)
    assert str(refused.value) == (
        f"port {notation!r} is not kind, kind[], kind{{}} with an optional ?"
    )


def test_port_refuses_an_unknown_family() -> None:
    with pytest.raises(SpecError) as refused:
        PortSpec.parse("picture/png")
    assert str(refused.value) == (
        "port kind 'picture/png' is not one of ['annotations', 'audio', 'file', 'image', "
        "'json', 'model', 'text', 'video']"
    )


# --- validation -------------------------------------------------------------------------------


def _declare(name: str = "kind", **declaration: Any) -> NodeSpec:
    def body(ctx: Any) -> dict:
        return {}

    spec = spec_of(node(name, **declaration)(body))
    assert spec is not None
    return spec


@pytest.mark.parametrize(
    ("name", "declaration", "message"),
    [
        ("Kind", {}, "node type name 'Kind' must be lower_snake words joined by ."),
        ("a..b", {}, "node type name 'a..b' must be lower_snake words joined by ."),
        ("a.", {}, "node type name 'a.' must be lower_snake words joined by ."),
        ("1a", {}, "node type name '1a' must be lower_snake words joined by ."),
        (
            "k",
            {"inputs": {"x": "image"}, "params": {"x": str, "a": int}},
            "k: ['x'] declared as both input and param",
        ),
        (
            "k",
            {"calls": {"agent.turn": "steps"}, "params": {"steps": int}},
            "k: calls['agent.turn'] names 'steps', which is not an integer setting with a "
            "minimum of at least 1",
        ),
        (
            "k",
            {"calls": {"agent.turn": "steps"}, "params": {"steps": param(int, minimum=0)}},
            "k: calls['agent.turn'] names 'steps', which is not an integer setting with a "
            "minimum of at least 1",
        ),
        (
            "k",
            {"calls": {"agent.turn": "steps"}, "params": {"steps": param(str, minimum=1)}},
            "k: calls['agent.turn'] names 'steps', which is not an integer setting with a "
            "minimum of at least 1",
        ),
        (
            "k",
            {"calls": {"agent.turn": "missing"}},
            "k: calls['agent.turn'] names 'missing', which is not an integer setting with a "
            "minimum of at least 1",
        ),
        (
            "k",
            {"calls": {"agent.turn": "steps"}, "params": {"steps": param(int, minimum=1)}},
            "k: calls['agent.turn'] needs 'steps' to set a maximum",
        ),
        ("k", {"calls": {"image.edit": 0}}, "k: calls['image.edit'] must be at least 1"),
        ("k", {"calls": {"image.edit": -2}}, "k: calls['image.edit'] must be at least 1"),
        (
            "k",
            {"calls": {"image.edit": True}},
            "k: calls['image.edit'] is a whole number of at least 1 or the name of an "
            "integer param",
        ),
        (
            "k",
            {"calls": {"image.edit": 1.5}},
            "k: calls['image.edit'] is a whole number of at least 1 or the name of an "
            "integer param",
        ),
    ],
)
def test_declaration_messages(name: str, declaration: dict[str, Any], message: str) -> None:
    with pytest.raises(SpecError) as refused:
        _declare(name, **declaration)
    assert str(refused.value) == message


@pytest.mark.parametrize(
    ("declaration", "message"),
    [
        (
            {"inputs": {"Image": "image"}},
            "k: input name 'Image' must be one lower_snake word (a letter, then letters, "
            "digits or _)",
        ),
        (
            {"params": {"a-b": str}},
            "k: param name 'a-b' must be one lower_snake word (a letter, then letters, "
            "digits or _)",
        ),
        (
            {"outputs": {"_x": "text"}},
            "k: output name '_x' must be one lower_snake word (a letter, then letters, "
            "digits or _)",
        ),
        ({"inputs": {"x": "imag"}}, "port kind 'imag' is not one of"),
        ({"outputs": {"x": "text??"}}, "port 'text??' is not kind"),
        (
            {"params": {"x": {"x-fx-optional": "yes"}}},
            "k: param 'x': x-fx-optional must be true or false",
        ),
        (
            {"params": {"x": {"default": float("nan")}}},
            "k: param 'x' is not a JSON value: at /default: nan is not a JSON number",
        ),
        (
            {"params": {"x": {"default": 2**60}}},
            "k: param 'x' is not a JSON value: at /default: the integer 1152921504606846976 "
            "cannot be read exactly",
        ),
        (
            {"resources": ["/abs/path.md"]},
            "k: resource '/abs/path.md' must be a POSIX path relative to the project root",
        ),
        ({"resources": ["../up.md"]}, "k: resource '../up.md' must be"),
        ({"resources": ["./here.md"]}, "k: resource './here.md' must be"),
        ({"resources": ["a//b.md"]}, "k: resource 'a//b.md' must be"),
        ({"resources": ["a/b/"]}, "k: resource 'a/b/' must be"),
        ({"resources": ["a\\b.md"]}, "k: resource 'a\\\\b.md' must be"),
        ({"resources": ["a/../b.md"]}, "k: resource 'a/../b.md' must be"),
        ({"resources": ["r.md", "r.md"]}, "k: resource 'r.md' is declared twice"),
        ({"resources": "r.md"}, "k: resources is a list of strings, not one string"),
        (
            {"tools": ["Blender"]},
            "k: tool 'Blender' must be a program name, optionally followed by a version bound",
        ),
        ({"tools": ["blender 4.2"]}, "k: tool 'blender 4.2' must be"),
        ({"tools": ["-x"]}, "k: tool '-x' must be"),
        ({"tools": ["ffmpeg", "ffmpeg"]}, "k: tool 'ffmpeg' is declared twice"),
        ({"tools": "ffmpeg"}, "k: tools is a list of strings, not one string"),
        ({"version": -1}, "k: version must be a whole number of at least 0, or None"),
        ({"version": True}, "k: version must be a whole number of at least 0, or None"),
        ({"version": 1.0}, "k: version must be a whole number of at least 0, or None"),
        ({"retry": "always"}, "k: retry is 'service' or 'engine', not 'always'"),
        ({"view": 3}, "k: view must be a string or None"),
        ({"judge": 1}, "k: judge must be True or False"),
    ],
)
def test_fx_validation(declaration: dict[str, Any], message: str) -> None:
    with pytest.raises(SpecError) as refused:
        _declare("k", **declaration)
    assert str(refused.value).startswith(message)


def test_validate_checks_a_spec_built_by_hand() -> None:
    good = NodeSpec("hand", inputs={"a": PortSpec("image", "list")}, version=1)
    good.validate()
    assert _errors(TYPE_SPEC, good.to_type_spec()) == []
    with pytest.raises(SpecError, match="port 'Image' is not kind"):
        NodeSpec("hand", outputs={"a": PortSpec("Image")}).validate()
    with pytest.raises(SpecError, match="hand: input 'a' is not a port"):
        NodeSpec("hand", inputs={"a": "image"}).validate()  # type: ignore[dict-item]
    with pytest.raises(SpecError, match="hand: param 'p' must be a JSON Schema object"):
        NodeSpec("hand", params={"p": "string"}).validate()  # type: ignore[dict-item]


def test_valid_specs_validate_against_the_wire_schema() -> None:
    # The validation agrees with the protocol's schema on what it accepts.
    spec = _declare(
        "edge.cases",
        params={"n": param(int, minimum=1, maximum=1)},
        calls={"a": "n", "b": 24},
        resources=[".hidden/a", "...x"],
        tools=["a", "b_c-d", "e!=1", "f<2"],
        version=0,
    )
    assert _errors(TYPE_SPEC, spec.to_type_spec()) == []


def test_body_is_not_part_of_the_specs_equality() -> None:
    def one(ctx: Any) -> dict:
        return {}

    def two(ctx: Any) -> dict:
        return {}

    first = spec_of(node("same", outputs={"a": "text"})(one))
    second = spec_of(node("same", outputs={"a": "text"})(two))
    assert first == second
    assert "body" not in repr(first)
