"""The Python builder: the documents it writes and the references it renders."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator

from grida.fx import Group, StepRef, Workflow

REPO = Path(__file__).resolve().parents[2]
WORKFLOW_SCHEMA = Draft202012Validator(
    json.loads((REPO / "spec" / "schemas" / "fx-workflow-v1.schema.json").read_text("utf-8"))
)


def _schema_errors(document: dict[str, Any]) -> list[str]:
    return [error.message for error in WORKFLOW_SCHEMA.iter_errors(document)]


def test_the_guide_example() -> None:
    pickups = [{"id": "ada", "name": "a lantern"}, {"id": "bo", "name": "a rope"}]
    wf = Workflow("level-art", title="Level art")
    plate = wf.step("plate", uses="./nodes/plate.py#ground_plate", with_={"material": "sand"})
    icons = wf.step(
        "icon",
        for_each=pickups,
        key="${{ item.id }}",
        uses="./workflows/icon.yaml",
        with_={"name": "${{ item.name }}"},
    )
    wf.outputs(plate=plate.outputs.image, icons=icons.all.outputs.icon)
    document = wf.document()
    assert document == {
        "fx": "workflow/v1",
        "id": "level-art",
        "title": "Level art",
        "steps": {
            "plate": {"uses": "./nodes/plate.py#ground_plate", "with": {"material": "sand"}},
            "icon": {
                "for_each": pickups,
                "key": "${{ item.id }}",
                "uses": "./workflows/icon.yaml",
                "with": {"name": "${{ item.name }}"},
            },
        },
        "outputs": {
            "plate": "${{ steps.plate.outputs.image }}",
            "icons": "${{ steps.icon.*.outputs.icon }}",
        },
    }
    assert list(document) == ["fx", "id", "title", "steps", "outputs"]
    assert _schema_errors(document) == []
    assert json.loads(json.dumps(document)) == document


def test_head_fields_appear_only_when_given_and_in_order() -> None:
    wf = Workflow(
        "full-head",
        title="Every head field",
        description="A workflow with all of them.",
        inputs={"count": "integer", "brief": {"type": "string", "default": "x"}},
        tables={"sizes": [1, 2, 3]},
        let={"double": "${{ inputs.count * 2 }}"},
        budget={"max_usd": 1.5},
        assert_=[{"check": "${{ inputs.count > 0 }}", "message": "count is positive"}],
        view="views/w.html",
    )
    wf.step("a", uses="fx/files.copy@1")
    document = wf.document()
    assert list(document) == [
        "fx",
        "id",
        "title",
        "description",
        "inputs",
        "tables",
        "let",
        "budget",
        "assert",
        "view",
        "steps",
        "outputs",
    ]
    assert document["assert"] == [
        {"check": "${{ inputs.count > 0 }}", "message": "count is positive"}
    ]
    assert document["outputs"] == {}
    empty = Workflow("empty-head", title="T", description="", inputs={}, tables={}, assert_=[])
    empty.step("a", uses="fx/files.copy@1")
    assert list(empty.document()) == ["fx", "id", "title", "steps", "outputs"]


def test_every_renamed_field() -> None:
    wf = Workflow("renamed", title="Renamed fields")
    draw = wf.step(
        "draw",
        uses="fx/image.generate@1",
        with_={"prompt": "a lantern"},
        if_="${{ inputs.go }}",
        assert_=[{"check": True, "message": "never"}],
        for_each=[1, 2],
        as_="n",
        takes=2,
    )
    wf.step(
        "review", uses="./nodes/j.py#judge", judges="draw", with_={"subject": draw.outputs.image}
    )
    step = wf.document()["steps"]["draw"]
    assert step == {
        "uses": "fx/image.generate@1",
        "with": {"prompt": "a lantern"},
        "if": "${{ inputs.go }}",
        "assert": [{"check": True, "message": "never"}],
        "for_each": [1, 2],
        "as": "n",
        "takes": 2,
    }
    assert wf.document()["steps"]["review"]["with"] == {
        "subject": "${{ steps.draw.outputs.image }}"
    }
    # Fields Python does not reserve pass through under their own names.
    wf.step("other", uses="x", on_reject="skip", needs=["draw"], route="img-a@acme")
    assert wf.document()["steps"]["other"] == {
        "uses": "x",
        "on_reject": "skip",
        "needs": ["draw"],
        "route": "img-a@acme",
    }


def test_nested_groups() -> None:
    wf = Workflow("nested", title="Nested groups")
    seed = wf.step("seed", uses="./nodes/s.py#seed")
    entity = wf.group("entity", for_each="${{ inputs.entities }}", key="${{ item.id }}")
    draw = entity.step("draw", uses="fx/image.generate@1", with_={"prompt": seed.outputs.text})
    polish = entity.group("polish", regenerate={"max": 2, "until": "${{ true }}"})
    edit = polish.step("edit", uses="fx/image.edit@1", with_={"image": draw.outputs.image})
    polish.step("check", uses="./nodes/c.py#check", with_={"image": edit.outputs.image})
    wf.outputs(final=edit.all.outputs.image)
    assert isinstance(entity, Group) and isinstance(polish, Group)
    assert draw.path == "entity.draw"
    assert edit.path == "entity.polish.edit"
    document = wf.document()
    assert document["steps"] == {
        "seed": {"uses": "./nodes/s.py#seed"},
        "entity": {
            "for_each": "${{ inputs.entities }}",
            "key": "${{ item.id }}",
            "steps": {
                "draw": {
                    "uses": "fx/image.generate@1",
                    "with": {"prompt": "${{ steps.seed.outputs.text }}"},
                },
                "polish": {
                    "regenerate": {"max": 2, "until": "${{ true }}"},
                    "steps": {
                        "edit": {
                            "uses": "fx/image.edit@1",
                            "with": {"image": "${{ steps.entity.draw.outputs.image }}"},
                        },
                        "check": {
                            "uses": "./nodes/c.py#check",
                            "with": {"image": "${{ steps.entity.polish.edit.outputs.image }}"},
                        },
                    },
                },
            },
        },
    }
    assert document["outputs"] == {"final": "${{ steps.entity.polish.edit.*.outputs.image }}"}
    assert _schema_errors(document) == []


def test_members_added_after_document_appear_in_the_next_one() -> None:
    wf = Workflow("later", title="Later")
    group = wf.group("g")
    group.step("a", uses="x")
    first = wf.document()
    group.step("b", uses="y")
    assert list(first["steps"]["g"]["steps"]) == ["a"]
    assert list(wf.document()["steps"]["g"]["steps"]) == ["a", "b"]
    # The document is a copy: changing it does not change the builder.
    first["steps"]["g"]["steps"]["a"]["uses"] = "changed"
    assert wf.document()["steps"]["g"]["steps"]["a"]["uses"] == "x"


def test_references() -> None:
    plate = StepRef("plate")
    assert str(plate.outputs.image) == "${{ steps.plate.outputs.image }}"
    assert str(plate.facts.verdict) == "${{ steps.plate.facts.verdict }}"
    assert str(plate.take) == "${{ steps.plate.take }}"
    assert str(plate.all) == "${{ steps.plate.* }}"
    assert str(plate.all.outputs.image) == "${{ steps.plate.*.outputs.image }}"
    assert str(plate["ada"]) == "${{ steps.plate['ada'] }}"
    assert str(plate["ada"].outputs.image) == "${{ steps.plate['ada'].outputs.image }}"
    assert str(plate.inner.outputs.x) == "${{ steps.plate.inner.outputs.x }}"
    assert str(plate["ada"].inner.outputs.x) == "${{ steps.plate['ada'].inner.outputs.x }}"
    assert str(plate.outputs.items["first"]) == "${{ steps.plate.outputs.items['first'] }}"
    assert repr(plate.outputs.image) == "${{ steps.plate.outputs.image }}"
    assert f"{plate.outputs.image}" == "${{ steps.plate.outputs.image }}"


@pytest.mark.parametrize(
    ("key", "quoted"),
    [
        ("it's", "['it\\'s']"),
        ("a\\b", "['a\\\\b']"),
        ("plain", "['plain']"),
        ('say "hi"', "['say \"hi\"']"),
        (True, "['true']"),
        (False, "['false']"),
        (3, "['3']"),
        (1.0, "['1']"),
        (0.5, "['0.5']"),
        (1e21, "['1e+21']"),
        (10**21, "['1e+21']"),
        (2**60, "['1152921504606847000']"),
        (1e-7, "['1e-7']"),
        (-0.000015, "['-0.000015']"),
    ],
)
def test_keys_are_quoted_like_instance_paths(key: Any, quoted: str) -> None:
    assert str(StepRef("e")[key]) == "${{ steps.e" + quoted + " }}"
    assert str(StepRef("e").outputs[key]) == "${{ steps.e.outputs" + quoted + " }}"


@pytest.mark.parametrize("key", [None, [1], float("nan"), float("inf"), 10**400])
def test_other_keys_are_refused(key: Any) -> None:
    with pytest.raises(TypeError, match="a step key is text, a number or a boolean"):
        StepRef("e")[key]


def test_private_names_are_attributes_not_references() -> None:
    plate = StepRef("plate")
    with pytest.raises(AttributeError):
        plate.__wrapped__  # noqa: B018
    with pytest.raises(AttributeError):
        plate.outputs._private  # noqa: B018
    assert not hasattr(plate.outputs, "__iter_me__")


def test_references_nested_in_values_become_strings() -> None:
    wf = Workflow("values", title="Values")
    a = wf.step("a", uses="x")
    wf.step(
        "b",
        uses="y",
        with_={
            "list": [a.outputs.one, (a.outputs.two, "lit")],
            "map": {"inner": {"deep": a.facts.score}},
            "text": f"before {a.outputs.text} after",
        },
    )
    assert wf.document()["steps"]["b"]["with"] == {
        "list": ["${{ steps.a.outputs.one }}", ["${{ steps.a.outputs.two }}", "lit"]],
        "map": {"inner": {"deep": "${{ steps.a.facts.score }}"}},
        "text": "before ${{ steps.a.outputs.text }} after",
    }


def test_a_name_is_declared_once() -> None:
    wf = Workflow("twice", title="Twice")
    wf.step("a", uses="x")
    with pytest.raises(ValueError, match="^a is declared twice$"):
        wf.step("a", uses="y")
    with pytest.raises(ValueError, match="^a is declared twice$"):
        wf.group("a")
    group = wf.group("g")
    group.step("b", uses="x")
    with pytest.raises(ValueError, match="^g.b is declared twice$"):
        group.step("b", uses="y")


def test_source_is_the_calling_file() -> None:
    wf = Workflow("here", title="Here")
    assert wf.source == Path(__file__).resolve()
    made: dict[str, Any] = {}
    exec("made['wf'] = Workflow('nowhere', title='Nowhere')", {"Workflow": Workflow, "made": made})
    assert made["wf"].source is None


def test_source_is_the_module_that_called_workflow(tmp_path: Path) -> None:
    lib = tmp_path / "lib.py"
    lib.write_text(
        "from grida.fx import Workflow\n\n\ndef make():\n    return Workflow('w', title='W')\n",
        "utf-8",
    )
    namespace: dict[str, Any] = {}
    exec(compile(lib.read_text("utf-8"), str(lib), "exec"), namespace)
    assert namespace["make"]().source == lib.resolve()
