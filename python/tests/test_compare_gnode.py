"""Tests for tools/compare_gnode.py, the comparison of grida-fx with the engine FX grew out of."""

from __future__ import annotations

import copy
import importlib.util
import json
import os
import re
import sys
from pathlib import Path
from typing import Any

import jsonschema
import pytest

REPO = Path(__file__).resolve().parents[2]
GRAPH_SCHEMA = REPO / "spec" / "schemas" / "fx-graph-v1.schema.json"
FX_BINARY = REPO / "target" / "debug" / "grida-fx"


def _load_tool() -> Any:
    spec = importlib.util.spec_from_file_location(
        "fx_tools_compare_gnode", REPO / "tools" / "compare_gnode.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


tool = _load_tool()

# --- the linear case, as each engine prints it --------------------------------------------------

# `gnode expand case --routes routes.yaml --inputs inputs.yaml` (graph/v2): count is a local
# type; draw reads count's output, so its identity waits on count.
GNODE_LINEAR: dict[str, Any] = {
    "estimate": {"ceiling_usd": None, "high_usd": 0.04, "low_usd": 0.01},
    "gnode": "graph/v2",
    "instances": [
        {
            "id": "count#1",
            "identity": "db4f53f11462bfbb1e61e3a13616d17f449b6c584bb93bee4ccb3b29251048f1",
            "judged_by": [],
            "judges": None,
            "key": None,
            "needs": [],
            "path": "count",
            "phase": 1,
            "price": {"high_usd": 0, "low_usd": 0},
            "reads": [],
            "routes": {},
            "state": "planned",
            "step": "count",
            "take": [1],
            "uses": "./nodes/cases.py#lines",
            "view": False,
            "waiting_on": [],
        },
        {
            "id": "draw#1",
            "identity": None,
            "judged_by": [],
            "judges": None,
            "key": None,
            "needs": [],
            "path": "draw",
            "phase": 1,
            "price": {"high_usd": 0.04, "low_usd": 0.01},
            "reads": ["count#1"],
            "routes": {"image.generate": "img-a@acme"},
            "state": "planned",
            "step": "draw",
            "take": [1],
            "uses": "gnode/image.generate@1",
            "view": False,
            "waiting_on": ["count#1"],
        },
    ],
    "pending": [],
    "problems": [],
    "workflow": "case",
}

# The same plan as fx-graph-v1: other digests, the type identities, the with-values (a pending
# one names the instances it waits on) and the route fingerprints, and money written as FX
# writes it (0.0 is 0).
FX_LINEAR: dict[str, Any] = {
    "kind": "fx-graph-v1",
    "workflow": {"id": "case", "title": "Linear", "file": "workflows/case.yaml"},
    "types": {
        "./nodes/cases.py#lines": {
            "identity": "nodes/cases.py#lines@1",
            "source": {"files": {"nodes/cases.py": "b2" * 32}, "resources": {}},
        },
        "fx/image.generate@1": {"identity": "fx/image.generate@1.1"},
    },
    "instances": [
        {
            "id": "count#1",
            "path": "count",
            "step": "count",
            "take": [1],
            "uses": "./nodes/cases.py#lines",
            "type": "nodes/cases.py#lines@1",
            "with": {"text": {"file": "c3" * 32}},
            "routes": {},
            "state": "planned",
            "identity": "a1" * 32,
            "phase": 1,
            "key": None,
            "judges": None,
            "judged_by": [],
            "waiting_on": [],
            "needs": [],
            "reads": [],
            "price": {"low_usd": 0.0, "high_usd": 0.0},
            "view": False,
        },
        {
            "id": "draw#1",
            "path": "draw",
            "step": "draw",
            "take": [1],
            "uses": "fx/image.generate@1",
            "type": "fx/image.generate@1.1",
            "with": {"prompt": {"pending": ["count#1"]}, "background": "auto", "vars": {}},
            "routes": {"image.generate": {"route": "img-a@acme", "fingerprint": "d4" * 32}},
            "state": "planned",
            "identity": None,
            "phase": 1,
            "key": None,
            "judges": None,
            "judged_by": [],
            "waiting_on": ["count#1"],
            "needs": [],
            "reads": ["count#1"],
            "price": {"low_usd": 0.01, "high_usd": 0.04},
            "view": False,
        },
    ],
    "pending": [],
    "estimate": {"low_usd": 0.01, "high_usd": 0.04, "ceiling_usd": None},
    "problems": [],
}

GNODE_LINEAR_PRICE: dict[str, Any] = {
    "ceiling_usd": None,
    "estimate": {"high_usd": 0.04, "low_usd": 0.01},
    "phases": [
        {
            "calls": [1, 1],
            "high_usd": 0.04,
            "low_usd": 0.01,
            "phase": 1,
            "steps": 2,
            "then": [],
        }
    ],
}

FX_LINEAR_PRICE: dict[str, Any] = {
    "phases": [
        {"phase": 1, "steps": 2.0, "calls": [1, 1], "low_usd": 0.01, "high_usd": 0.04, "then": []}
    ],
    "estimate": {"low_usd": 0.01, "high_usd": 0.04},
    "ceiling_usd": None,
}

LINEAR_ARGS = ["case", "--routes", "routes.yaml", "--inputs", "inputs.yaml"]


def gnode_linear() -> dict[str, Any]:
    return copy.deepcopy(GNODE_LINEAR)


def fx_linear() -> dict[str, Any]:
    return copy.deepcopy(FX_LINEAR)


# --- normalise ----------------------------------------------------------------------------------


def test_the_fx_sample_is_an_fx_graph() -> None:
    schema = json.loads(GRAPH_SCHEMA.read_text("utf-8"))
    jsonschema.Draft202012Validator(schema).validate(FX_LINEAR)


def test_both_engines_linear_graphs_normalise_alike() -> None:
    gnode = tool.normalise(gnode_linear(), "gnode")
    fx = tool.normalise(fx_linear(), "fx")
    assert tool.compare(gnode, fx) == []
    assert gnode == fx


def test_a_normalised_graph_has_gnodes_shape_and_no_digest() -> None:
    normal = tool.normalise(fx_linear(), "fx")
    assert set(normal) == {"gnode", "workflow", "instances", "pending", "estimate", "problems"}
    assert normal["gnode"] == "graph/v2"
    assert normal["workflow"] == "case"
    count, draw = normal["instances"]
    assert "type" not in draw and "with" not in draw
    assert draw["routes"] == {"image.generate": "img-a@acme"}
    assert draw["uses"] == "fx/image.generate@1"
    assert count["identity"] == tool.DIGEST
    assert draw["identity"] is None
    assert count["price"] == {"low_usd": 0, "high_usd": 0}
    assert type(count["price"]["low_usd"]) is int
    text = json.dumps(normal)
    assert not re.search(r"[0-9a-f]{64}", text)
    assert not re.search(r"[0-9a-f]{64}", json.dumps(tool.normalise(gnode_linear(), "gnode")))


def test_normalise_leaves_its_input_alone() -> None:
    gnode, fx = gnode_linear(), fx_linear()
    tool.normalise(gnode, "gnode")
    tool.normalise(fx, "fx")
    assert gnode == GNODE_LINEAR
    assert fx == FX_LINEAR


def test_whether_an_identity_is_known_is_still_compared() -> None:
    fx = fx_linear()
    fx["instances"][1]["identity"] = "f0" * 32
    lines = tool.compare(tool.normalise(gnode_linear(), "gnode"), tool.normalise(fx, "fx"))
    assert lines == ['$.instances[draw#1].identity: null != "<digest>"']


def test_pending_tokens_are_masked_in_with_values_only() -> None:
    document = {
        "fingerprint": "dropped",
        "with": {
            "a": {"pending": "x#1"},
            "pending": "a with-value named pending is data",
            "fingerprint": "so is one named fingerprint",
            "b": [{"pending": "y#1"}, {"pending": "z#1", "other": 1}],
        },
        "nested": {"fingerprint": "dropped", "kept": {"pending": "not under with"}},
    }
    assert tool.normalise(document, "fx") == {
        "with": {
            "a": {"pending": "*"},
            "pending": "a with-value named pending is data",
            "fingerprint": "so is one named fingerprint",
            "b": [{"pending": "*"}, {"pending": "z#1", "other": 1}],
        },
        "nested": {"kept": {"pending": "not under with"}},
    }


def test_pending_values_that_name_their_instances_are_masked_too() -> None:
    # A pending with-value is printed either as a token or as the instance ids it waits on.
    document = {
        "with": {
            "token": {"pending": "e5" * 32},
            "one": {"pending": ["count#1"]},
            "two": {"pending": ["a#1", "icon['rope'].draw#2"]},
            "none": {"pending": []},
            "nested": {"vars": {"n": {"pending": ["count#1"]}}},
            "listed": [{"pending": ["b#1"]}, 1],
            # Data that only looks like it: never masked.
            "number": {"pending": 5},
            "numbers": {"pending": [1, 2]},
            "mixed": {"pending": ["a#1", None]},
        }
    }
    assert tool.normalise(document, "fx")["with"] == {
        "token": {"pending": "*"},
        "one": {"pending": "*"},
        "two": {"pending": "*"},
        "none": {"pending": "*"},
        "nested": {"vars": {"n": {"pending": "*"}}},
        "listed": [{"pending": "*"}, 1],
        "number": {"pending": 5},
        "numbers": {"pending": [1, 2]},
        "mixed": {"pending": ["a#1", None]},
    }


@pytest.mark.parametrize("waits_on", ["db" * 32, ["count#1"]])
def test_linear_graphs_compare_alike_with_either_pending_form(waits_on: Any) -> None:
    # The token is the form graphs were printed in before they named the instances.
    fx = fx_linear()
    fx["instances"][1]["with"]["prompt"] = {"pending": waits_on}
    assert tool.compare(tool.normalise(gnode_linear(), "gnode"), tool.normalise(fx, "fx")) == []
    masked = tool._scrubbed({"with": fx["instances"][1]["with"]})
    assert masked["with"]["prompt"] == {"pending": "*"}


def test_both_engines_linear_prices_normalise_alike() -> None:
    gnode = tool.normalise(copy.deepcopy(GNODE_LINEAR_PRICE), "gnode")
    fx = tool.normalise(copy.deepcopy(FX_LINEAR_PRICE), "fx")
    assert tool.compare(gnode, fx) == []
    assert gnode == fx


def test_normalise_drops_plan_digests_and_types_on_either_side() -> None:
    for side in ("gnode", "fx"):
        normal = tool.normalise(
            {"plan": "1" * 64, "graph_sha256": "2" * 64, "types": {}, "n": 2.0}, side
        )
        assert normal == {"n": 2}


def test_normalise_knows_two_sides() -> None:
    with pytest.raises(ValueError):
        tool.normalise({}, "other")


# --- names --------------------------------------------------------------------------------------


GNODE_PROBLEMS = [
    (
        "draw.route",
        "no route for image.generate: set route: on the step or routes.image.generate in "
        "gnode.yaml",
        "draw.route: no route for image.generate: set route: on the step or "
        "routes.image.generate in fx.yaml",
    ),
    (
        "uses",
        "./nodes/n.py#pinned: its source changed but version 2 did not; bump the version, or "
        "confirm no change in behaviour with gnode lock --same nodes/n.py#pinned",
        "uses: ./nodes/n.py#pinned: its source changed but version 2 did not; bump the version, "
        "or confirm no change in behaviour with grida-fx lock --same nodes/n.py#pinned",
    ),
    (
        "a",
        "no built-in node type gnode/nosuch@1; see gnode nodes",
        "a: no built-in node type fx/nosuch@1; see grida-fx nodes",
    ),
    (
        "b.with.prompt",
        "a param marked x-gnode-template renders text; gnode.lock pins it",
        "b.with.prompt: a param marked x-fx-template renders text; fx.lock pins it",
    ),
    (
        "uses",
        "a workflow file starts with gnode: workflow/v1",
        "uses: a workflow file starts with fx: workflow/v1",
    ),
]


@pytest.mark.parametrize(("where", "message", "expected"), GNODE_PROBLEMS)
def test_gnode_problems_read_with_fx_names(where: str, message: str, expected: str) -> None:
    normal = tool.normalise({"problems": [{"where": where, "message": message}]}, "gnode")
    assert normal["problems"] == [expected]


def test_fx_names_in_other_texts() -> None:
    note = 'move it with gnode takes mv case "counted" <new path>'
    assert tool.fx_names(note) == 'move it with grida-fx takes mv case "counted" <new path>'
    # A word that only contains the name is left alone.
    for text in ["src/gnode/workflow/cli.py", "my-gnode/x@1", "gnodes lock"]:
        assert tool.fx_names(text) == text


def test_gnode_uses_and_reasons_get_fx_names() -> None:
    gnode = gnode_linear()
    gnode["instances"][1]["reason"] = "gnode/select@1 chose nothing"
    normal = tool.normalise(gnode, "gnode")
    assert normal["instances"][1]["uses"] == "fx/image.generate@1"
    assert normal["instances"][1]["reason"] == "fx/select@1 chose nothing"


def test_fx_texts_are_never_renamed() -> None:
    # FX must print FX's names itself: a gnode name in its output is a difference.
    fx = fx_linear()
    fx["problems"] = [{"where": "a", "message": "see gnode nodes"}]
    fx["instances"][1]["uses"] = "gnode/image.generate@1"
    normal = tool.normalise(fx, "fx")
    assert normal["problems"] == ["a: see gnode nodes"]
    assert tool.compare(tool.normalise(gnode_linear(), "gnode"), normal) == [
        '$.instances[draw#1].uses: "fx/image.generate@1" != "gnode/image.generate@1"',
        '$.problems: (absent) != "a: see gnode nodes"',
    ]


def test_fx_file_names_map_back_to_gnodes() -> None:
    assert tool.gnode_path("fx.yaml") == "gnode.yaml"
    assert tool.gnode_path("sub/fx.lock") == "sub/gnode.lock"
    for text in ["routes.yaml", "--routes", "case", "fx.yaml.bak", "workflows/case.yaml", ""]:
        assert tool.gnode_path(text) == text


def test_fx_case_files_map_back_to_gnodes() -> None:
    files = {
        "fx.yaml": "fx: project/v1\nroutes:\n  image.generate: img-a@acme\n",
        "workflows/w.yml": "fx: workflow/v1\nsteps:\n  a: { uses: fx/image.generate@1 }\n",
        "nodes/n.py": '"""Nodes."""\n\nfrom grida.fx import Ctx, node\n',
        "fx.lock": "fx: lock/v1\nnodes:\n  nodes/n.py#pinned@2: abc\n",
        "notes.txt": "fx: left alone\n",
        "inputs.yaml": "prefix: fx/x\n",
    }
    assert tool.gnode_files(files) == {
        "gnode.yaml": "gnode: project/v1\nroutes:\n  image.generate: img-a@acme\n",
        "workflows/w.yml": "gnode: workflow/v1\nsteps:\n  a: { uses: gnode/image.generate@1 }\n",
        "nodes/n.py": '"""Nodes."""\n\nfrom gnode import Ctx, node\n',
        "gnode.lock": "nodes:\n  nodes/n.py#pinned@2: abc\n",
        "notes.txt": "fx: left alone\n",
        "inputs.yaml": "prefix: fx/x\n",
    }


# --- compare ------------------------------------------------------------------------------------


def _instance(ident: str, state: str = "planned") -> dict[str, Any]:
    return {"id": ident, "state": state, "take": [1]}


def test_compare_reports_paths() -> None:
    gnode = {
        "estimate": {"high_usd": 0.04, "ceiling_usd": None},
        "instances": [_instance("a#1"), _instance("b#1")],
        "problems": ["x: y"],
        "workflow": "case",
        "pending": [{"path": "draw", "max": 6}],
    }
    fx = {
        "estimate": {"high_usd": 0.05, "ceiling_usd": None},
        "instances": [_instance("a#1"), _instance("b#1", "maybe"), _instance("c#1")],
        "problems": [],
        "workflow": "other",
        "pending": [{"path": "draw", "max": 5}],
        "kind": "fx-graph-v1",
    }
    assert tool.compare(gnode, fx) == [
        "$.estimate.high_usd: 0.04 != 0.05",
        '$.instances[c#1]: (absent) != {"id": "c#1", "state": "planned", "take": [1]}',
        '$.instances[b#1].state: "planned" != "maybe"',
        '$.kind: (absent) != "fx-graph-v1"',
        "$.pending[0].max: 6 != 5",
        '$.problems: "x: y" != (absent)',
        '$.workflow: "case" != "other"',
    ]


def test_compare_matches_instances_by_id_and_reports_their_order_once() -> None:
    gnode = {"instances": [_instance("a#1"), _instance("b#1"), _instance("c#1")]}
    fx = {"instances": [_instance("c#1"), _instance("a#1", "absent"), _instance("b#1")]}
    assert tool.compare(gnode, fx) == [
        '$.instances: order ["a#1", "b#1", "c#1"] != ["c#1", "a#1", "b#1"]',
        '$.instances[a#1].state: "planned" != "absent"',
    ]


def test_compare_numbers_as_numbers() -> None:
    assert tool.compare(1, 1.0) == []
    assert tool.compare({"steps": 2, "calls": [1, 3]}, {"steps": 2.0, "calls": [1.0, 3]}) == []
    # Money is equal within 1e-9; nothing else is.
    assert tool.compare({"low_usd": 0.1 + 0.2}, {"low_usd": 0.3}) == []
    assert tool.compare({"low_usd": 0.01}, {"low_usd": 0.0100001}) == [
        "$.low_usd: 0.01 != 0.0100001"
    ]
    assert tool.compare({"phase": 0.1 + 0.2}, {"phase": 0.3}) == [
        "$.phase: 0.30000000000000004 != 0.3"
    ]
    # A boolean is never a number, and null is not zero.
    assert tool.compare({"view": True}, {"view": 1}) == ["$.view: true != 1"]
    assert tool.compare({"view": False}, {"view": 0.0}) == ["$.view: false != 0.0"]
    assert tool.compare({"ceiling_usd": None}, {"ceiling_usd": 0}) == ["$.ceiling_usd: null != 0"]


def test_compare_lists_of_other_lengths() -> None:
    assert tool.compare({"take": [1]}, {"take": [1, 2]}) == ["$.take: [1] != [1, 2]"]
    assert tool.compare({"reads": ["a#1", "b#1"]}, {"reads": ["b#1"]}) == [
        '$.reads: "a#1" != (absent)'
    ]
    assert tool.compare({"then": []}, {"then": ["draw (up to 6)"]}) == [
        '$.then: (absent) != "draw (up to 6)"'
    ]


def test_compare_names_members_that_are_not_identifiers() -> None:
    gnode = {"routes": {"image.generate": "img-a@acme"}}
    fx = {"routes": {"image.generate": "img-b@acme"}}
    assert tool.compare(gnode, fx) == ['$.routes["image.generate"]: "img-a@acme" != "img-b@acme"']


def test_compare_documents_of_other_types() -> None:
    assert tool.compare({"a": 1}, [1]) == ['$: {"a": 1} != [1]']
    assert tool.compare("x", "x") == []


# --- known differences --------------------------------------------------------------------------


def test_a_known_path_covers_what_is_under_it() -> None:
    entry = ("lock-drift", "expand", "$.problems")
    assert tool._is_known("lock-drift", "expand", "$.problems") == entry
    assert tool._is_known("lock-drift", "expand", "$.problems[0]") == entry
    assert tool._is_known("lock-drift", "expand", "$.problems_more") is None
    assert tool._is_known("lock-drift", "price", "$.problems") is None
    assert tool._is_known("linear", "expand", "$.problems") is None


def test_a_known_path_may_stand_for_any_member(monkeypatch: pytest.MonkeyPatch) -> None:
    entry = ("c", "project.json", "$.instances[*].facts.cost_usd")
    monkeypatch.setattr(tool, "KNOWN_DIFFERENCES", [entry])
    for path in (
        "$.instances[a#1].facts.cost_usd",
        "$.instances[\"loud['ada']#1\"].facts.cost_usd",
        '$.instances["x\\"]#1"].facts.cost_usd.deeper',
        "$.instances[0].facts.cost_usd[1]",
    ):
        assert tool._is_known("c", "project.json", path) == entry, path
    for path in (
        "$.instances[a#1].facts",
        "$.instances[a#1].facts.words",
        "$.instances.facts.cost_usd",
        "$.instances[a#1][b].facts.cost_usd",
        "$.instances[a#1].facts.cost_usd_more",
    ):
        assert tool._is_known("c", "project.json", path) is None, path
    assert tool._is_known("c", "other.json", "$.instances[a#1].facts.cost_usd") is None


def test_a_known_document_covers_nothing_inside_it() -> None:
    # resource-missing: gnode stops with a traceback, so only FX prints a graph.
    entry = ("resource-missing", "expand", tool.ROOT)
    assert entry in tool.KNOWN_DIFFERENCES
    assert tool._is_known("resource-missing", "expand", "$") == entry
    assert tool._is_known("resource-missing", "expand", "$.problems") is None
    assert tool._is_known("resource-missing", "expand", "$[0]") is None
    assert tool._is_known("resource-missing", "expand", tool.STATUS) is None


def test_lock_drift_is_known_on_its_problems_only() -> None:
    entry = ("lock-drift", "expand", "$.problems")
    assert entry in tool.KNOWN_DIFFERENCES
    assert tool._is_known("lock-drift", "expand", "$.problems[0]") == entry
    assert tool._is_known("lock-drift", "expand", tool.STATUS) is None
    assert tool._is_known("lock-drift", "expand", "$.instances[b#1].state") is None


@pytest.mark.parametrize(
    "decision",
    [
        "a syntax error in a prompt file is a problem on <step>.with.<param>",
        "an assertion message that evaluates to something other than text renders by FX's text",
        "a negative call bound",
        "a negative duration or max_chars is no length",
        "join, contains, min/max, digest, == and text over a list or object that holds a pending",
    ],
)
def test_decisions_no_case_exercises_are_recorded(decision: str) -> None:
    assert any(entry.startswith(decision) for entry in tool.UNCASED_DECISIONS)


def test_every_known_difference_names_a_case_of_the_suite() -> None:
    for case, what, path in tool.KNOWN_DIFFERENCES:
        assert (REPO / "conformance" / case / "case.yaml").is_file(), case
        steps = tool.conformance.load_case(REPO / "conformance" / case)
        if tool.is_run_case(steps):
            # Run mode: what a step saves, or `step <n>` for one that saves nothing.
            names = {step["save"] for step in steps if "save" in step}
            names |= {f"step {n}" for n, step in enumerate(steps, 1) if "argv" in step}
            assert what in names, (case, what)
        else:
            assert what in tool.VERBS, (case, what)
        assert path in (tool.STATUS, tool.ROOT) or path.startswith(("$.", "$[")), path
        assert case not in tool.NOT_PORTED


# --- cases --------------------------------------------------------------------------------------


def test_case_steps_of_linear() -> None:
    assert tool.case_steps(REPO / "conformance" / "linear") == [
        ["expand", *LINEAR_ARGS],
        ["identity", *LINEAR_ARGS],
        ["price", *LINEAR_ARGS],
    ]


def _write_case(folder: Path, text: str) -> Path:
    (folder / "in").mkdir(parents=True)
    (folder / "case.yaml").write_text(text, "utf-8")
    return folder


def test_case_steps_carry_earlier_files_forward(tmp_path: Path) -> None:
    case = _write_case(
        tmp_path / "c",
        "steps:\n"
        "- files: { a.txt: one }\n"
        "- argv: [lock]\n"
        "- files: { fx.yaml: 'fx: project/v1' }\n"
        "  argv: [expand, case]\n"
        "- argv: [identity, case]\n"
        "- files: { a.txt: two }\n"
        "- argv: [price, case]\n",
    )
    assert tool.case_steps(case) == [["expand", "case"], ["identity", "case"], ["price", "case"]]
    assert tool._case_plan(case) == [
        (3, ["expand", "case"], {"a.txt": "one", "fx.yaml": "fx: project/v1"}),
        (4, ["identity", "case"], {"a.txt": "one", "fx.yaml": "fx: project/v1"}),
        (6, ["price", "case"], {"a.txt": "two", "fx.yaml": "fx: project/v1"}),
    ]


def test_case_steps_refuse_a_case_outside_the_format(tmp_path: Path) -> None:
    case = _write_case(tmp_path / "c", "steps:\n- argv: [expand, case]\n  surprise: 1\n")
    with pytest.raises(tool.conformance.CaseFailure):
        tool.case_steps(case)


def test_files_are_written_into_a_copy(tmp_path: Path) -> None:
    case_in = tmp_path / "in"
    case_in.mkdir()
    (case_in / "a.txt").write_text("one", "utf-8")
    with tool._with_files(case_in, {}) as same:
        assert same == case_in
    with tool._with_files(case_in, {"a.txt": "two", "sub/b.txt": "three"}) as staged:
        assert staged != case_in
        assert (staged / "a.txt").read_text("utf-8") == "two"
        assert (staged / "sub" / "b.txt").read_text("utf-8") == "three"
    assert (case_in / "a.txt").read_text("utf-8") == "one"
    assert not (case_in / "sub").exists()


def _tree(root: Path) -> dict[str, bytes]:
    return {
        path.relative_to(root).as_posix(): path.read_bytes()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def test_a_case_gnode_lacks_is_ported_to_its_names(tmp_path: Path) -> None:
    case_in = tmp_path / "drift" / "in"
    files = {
        "fx.yaml": "fx: project/v1\n",
        "fx.lock": "fx: lock/v1\nnodes:\n  nodes/n.py#pinned@2: abc\n",
        "workflows/case.yaml": "fx: workflow/v1\nsteps:\n  a: { uses: fx/image.generate@1 }\n",
        "nodes/n.py": "from grida.fx import Ctx, node\n",
        "prompts/r.md": "fx: kept as written\n",
    }
    for relative, text in files.items():
        (case_in / relative).parent.mkdir(parents=True, exist_ok=True)
        (case_in / relative).write_text(text, "utf-8")
    picture = b"\x89PNG\r\n\x1a\n" + bytes(range(256))
    (case_in / "art.png").write_bytes(picture)
    # A document that is not UTF-8, as a case refusing it would hold: its bytes are kept.
    latin = "fx: workflow/v1\ntitle: caf\xe9\n".encode("latin-1")
    (case_in / "bad").mkdir()
    (case_in / "bad" / "latin.yaml").write_bytes(latin)
    (case_in / "bad" / "fx.yaml").write_bytes(latin)
    (case_in / "__pycache__").mkdir()
    (case_in / "__pycache__" / "n.pyc").write_bytes(b"\0")
    before = _tree(case_in)

    with tool._ported(case_in) as ported:
        # Named like the case, as a case's own in/ is.
        assert ported.name == "in" and ported.parent.name == "drift"
        assert _tree(ported) == {
            "art.png": picture,
            "bad/gnode.yaml": latin,
            "bad/latin.yaml": latin,
            "gnode.lock": b"nodes:\n  nodes/n.py#pinned@2: abc\n",
            "gnode.yaml": b"gnode: project/v1\n",
            "nodes/n.py": b"from gnode import Ctx, node\n",
            "prompts/r.md": b"fx: kept as written\n",
            "workflows/case.yaml": (
                b"gnode: workflow/v1\nsteps:\n  a: { uses: gnode/image.generate@1 }\n"
            ),
        }
    assert not ported.exists()
    assert _tree(case_in) == before


def test_a_suite_case_ports_to_what_gnode_reads() -> None:
    with tool._ported(REPO / "conformance" / "lock-drift" / "in") as ported:
        assert (ported / "gnode.yaml").is_file() and not (ported / "fx.yaml").exists()
        assert not (ported / "fx.lock").exists()
        assert "fx:" not in (ported / "gnode.lock").read_text("utf-8")
        assert "from gnode import" in (ported / "nodes" / "n.py").read_text("utf-8")


# --- running ------------------------------------------------------------------------------------

# A stand-in engine: says where it ran and with what, writes into its project, exits 3.
PROBE = """\
import json, os, sys
from pathlib import Path
Path("made.txt").write_text("x")
print(json.dumps({
    "argv": sys.argv[1:],
    "cwd": os.getcwd(),
    "env": sorted(os.environ),
    "home": os.environ.get("HOME"),
    "plugins": os.environ.get("GNODE_PLUGINS"),
    "seen": Path("a.txt").read_text(),
}))
sys.exit(3)
"""


@pytest.fixture
def probe(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Path, list[str]]:
    case_in = tmp_path / "in"
    case_in.mkdir()
    (case_in / "a.txt").write_text("from the case", "utf-8")
    script = tmp_path / "probe.py"
    script.write_text(PROBE, "utf-8")
    monkeypatch.setenv("FX_LEAKED", "1")
    monkeypatch.setenv("GRIDA_FX_PYTHON", sys.executable)
    return case_in, [sys.executable, str(script)]


def test_run_fx_runs_in_a_copy_with_a_minimal_environment(
    probe: tuple[Path, list[str]],
) -> None:
    case_in, command = probe
    status, stdout = tool.run_fx(command, case_in, ["expand", "case"])
    assert status == 3
    ran = json.loads(stdout)
    assert ran["argv"] == ["expand", "case"]
    assert ran["seen"] == "from the case"
    assert Path(ran["cwd"]).resolve() != case_in.resolve()
    assert not (case_in / "made.txt").exists()
    assert ran["home"] != os.environ.get("HOME")
    assert "FX_LEAKED" not in ran["env"]
    assert "GRIDA_FX_PYTHON" in ran["env"]
    assert ran["plugins"] is None
    assert {"PATH", "HOME", "NO_COLOR", "LANG", "PYTHONDONTWRITEBYTECODE"} <= set(ran["env"])


def test_run_gnode_runs_in_a_copy_with_the_std_plugins_only(
    probe: tuple[Path, list[str]], monkeypatch: pytest.MonkeyPatch
) -> None:
    case_in, command = probe
    monkeypatch.setattr(tool, "_gnode_command", lambda repo: command)
    status, stdout = tool.run_gnode(Path("unused"), case_in, ["price", "case"])
    assert status == 3
    ran = json.loads(stdout)
    assert ran["argv"] == ["price", "case"]
    assert ran["plugins"] == "std"
    assert "GRIDA_FX_PYTHON" not in ran["env"]
    assert "FX_LEAKED" not in ran["env"]
    assert not (case_in / "made.txt").exists()


def test_gnode_runs_from_its_checkout_without_syncing_it() -> None:
    command = tool._gnode_command(Path("/somewhere/stage-gen"))
    assert command[1:] == ["run", "--project", "/somewhere/stage-gen", "--no-sync", "gnode"]


# --- main, with stand-in engines ----------------------------------------------------------------


@pytest.fixture
def engines(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> dict[str, Any]:
    """A gnode checkout holding some cases, and both engines replaced by canned outputs."""
    repo = tmp_path / "stage-gen"
    for name in ("linear", "at-plan", "run-project"):
        (repo / "tests" / "conformance" / name / "in").mkdir(parents=True)
        (repo / "tests" / "conformance" / name / "case.yaml").write_text("steps: []\n", "utf-8")
    monkeypatch.setenv("FX_GNODE_REPO", str(repo))
    monkeypatch.setattr(tool, "_gnode_unavailable", lambda repo: None)
    state: dict[str, Any] = {"gnode": {}, "fx": {}, "calls": [], "projects": []}

    def fake(side: str) -> Any:
        def run(_: Any, case_in: Path, argv: list[str]) -> tuple[int, str, str]:
            case = case_in.parent.name if case_in.name == "in" else case_in.name
            state["calls"].append((side, case, list(argv)))
            if case_in.is_dir():
                state["projects"].append((side, case, sorted(_tree(case_in))))
            return state[side].get((case, argv[0]), (2, "", f"{side}: no output for {case}"))

        return run

    def fake_steps(side: str) -> Any:
        def run(_: Any, case_in: Path, steps: list[dict[str, Any]]) -> list[Any]:
            case = case_in.parent.name if case_in.name == "in" else case_in.name
            state["calls"].append((side, case, "steps"))
            if case_in.is_dir():
                state["projects"].append((side, case, sorted(_tree(case_in))))
            if (side, case) in state["steps"]:
                return state["steps"][(side, case)]
            # By default every step exits 0 and saves an empty text: both engines alike.
            return [
                tool.StepRun(0 if "argv" in step else None, "", b"" if "save" in step else None)
                for step in steps
            ]

        return run

    monkeypatch.setattr(tool, "_run_gnode", fake("gnode"))
    monkeypatch.setattr(tool, "_run_fx", fake("fx"))
    monkeypatch.setattr(tool, "run_gnode_steps", fake_steps("gnode"))
    monkeypatch.setattr(tool, "run_fx_steps", fake_steps("fx"))
    state["steps"] = {}
    state["gnode"][("linear", "expand")] = (0, json.dumps(GNODE_LINEAR), "")
    state["gnode"][("linear", "price")] = (0, json.dumps(GNODE_LINEAR_PRICE), "")
    state["fx"][("linear", "expand")] = (0, json.dumps(FX_LINEAR), "")
    state["fx"][("linear", "price")] = (0, json.dumps(FX_LINEAR_PRICE), "")
    return state


def _main(*args: str) -> int:
    return tool.main(["--command", sys.executable, *args])


def test_main_reports_same(engines: dict[str, Any], capsys: pytest.CaptureFixture[str]) -> None:
    identities = {"count#1": "a1" * 32, "draw#1": None}
    engines["gnode"][("linear", "identity")] = (0, json.dumps(identities), "")
    engines["fx"][("linear", "identity")] = (
        0,
        json.dumps({**identities, "count#1": "b2" * 32}),
        "",
    )
    assert _main("linear") == 0
    out = capsys.readouterr().out.splitlines()
    assert out == [
        "linear expand: same",
        "linear identity: same",
        "linear price: same",
        "compare_gnode: 3 same",
    ]
    assert engines["calls"] == [
        ("gnode", "linear", ["expand", *LINEAR_ARGS]),
        ("fx", "linear", ["expand", *LINEAR_ARGS]),
        ("gnode", "linear", ["identity", *LINEAR_ARGS]),
        ("fx", "linear", ["identity", *LINEAR_ARGS]),
        ("gnode", "linear", ["price", *LINEAR_ARGS]),
        ("fx", "linear", ["price", *LINEAR_ARGS]),
    ]


def test_identities_compare_by_whether_they_are_known(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    engines["gnode"][("linear", "identity")] = (0, json.dumps({"count#1": "a1" * 32}), "")
    engines["fx"][("linear", "identity")] = (0, json.dumps({"count#1": None}), "")
    assert _main("linear") == 1
    out = capsys.readouterr().out.splitlines()
    assert out[1:3] == ["linear identity: DIFFERS", '  $["count#1"]: "<digest>" != null']


def test_main_compares_every_case_of_the_suite(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    _main()
    out = capsys.readouterr().out.splitlines()
    assert "linear expand: same" in out
    # A run case is compared step by step, in run mode.
    assert "run-project step 1 run: same" in out
    assert "run-local said.txt (ported): same" in out
    assert "cache-replay-miss step 1 run (ported): same" in out
    assert "yaml-strict: SKIP not ported: " + tool.NOT_PORTED["yaml-strict"] in out
    assert "numbers: SKIP not ported: " + tool.NOT_PORTED["numbers"] in out
    # The cases this gnode checkout lacks are ported, every one with a step to compare (here
    # both stand-ins print nothing for them, alike).
    assert "lock-drift expand (step 1, ported): same" in out
    assert "linear expand (ported): same" not in out
    cases = [case.parent for case in (REPO / "conformance").glob("*/case.yaml")]
    run = {case.name for case in cases if tool.is_run_case(tool.conformance.load_case(case))}
    planned = {case.name for case in cases if tool.case_steps(case)} - run
    assert {"linear", "at-plan", "facts", "lock-drift"} <= planned
    assert {"run-project", "run-local", "run-takes", "numbers", "cache-replay-miss"} <= run
    called = {case for _, case, _ in engines["calls"]}
    assert called == (planned | run) - set(tool.NOT_PORTED)


def test_main_reports_a_difference_with_a_diff(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    fx = fx_linear()
    fx["instances"][1]["state"] = "blocked"
    engines["fx"][("linear", "expand")] = (1, json.dumps(fx), "")
    assert _main("linear") == 1
    out = capsys.readouterr().out
    lines = out.splitlines()
    assert lines[:3] == [
        "linear expand: DIFFERS",
        "  status: 0 != 1",
        '  $.instances[draw#1].state: "planned" != "blocked"',
    ]
    assert "--- gnode" in lines and "+++ grida-fx" in lines
    assert '-   "state": "planned",' in lines and '+   "state": "blocked",' in lines
    assert lines[-2:] == ["linear price: same", "compare_gnode: 2 same, 1 differs"]


def test_main_shows_why_an_engine_printed_nothing(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    engines["fx"][("linear", "price")] = (2, "", "grida-fx: price::make_plan is not done yet\n")
    assert _main("linear") == 1
    out = capsys.readouterr().out
    assert "linear price: DIFFERS\n  status: 0 != 2\n  $: {" in out
    assert "!= (no JSON)" in out
    assert "  grida-fx said on stderr:\n    grida-fx: price::make_plan is not done yet" in out


def test_main_reports_known_differences(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    # lock-drift's first expand differs on its problems only, a known difference; its second
    # prints the same graph on both engines, so that step's known difference is not found.
    engines["gnode"][("lock-drift", "expand")] = (1, json.dumps(_lock_drift("uses")), "")
    engines["fx"][("lock-drift", "expand")] = (1, json.dumps(_lock_drift("b")), "")
    engines["gnode"][("resource-missing", "expand")] = (0, json.dumps(GNODE_LINEAR), "")
    engines["fx"][("resource-missing", "expand")] = (0, json.dumps(FX_LINEAR), "")
    assert _main("lock-drift", "resource-missing") == 0
    out = capsys.readouterr().out.splitlines()
    assert out[:3] == [
        "lock-drift expand (step 1, ported): known (1)",
        "lock-drift expand (step 6, ported): known (1)",
        "resource-missing expand (ported): same",
    ]
    # The known differences no comparison found any more are noted.
    assert "note: the known difference resource-missing expand $ was not found" in out
    assert not any(line.startswith("note: the known difference lock-drift") for line in out)
    assert out[-1] == "compare_gnode: 1 same, 2 known"


def test_a_known_difference_does_not_cover_a_crash(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    engines["gnode"][("lock-drift", "expand")] = (1, json.dumps(_lock_drift("uses")), "")
    engines["fx"][("lock-drift", "expand")] = (2, "", "grida-fx: boom\n")
    assert _main("lock-drift") == 1
    out = capsys.readouterr().out
    assert "lock-drift expand (step 1, ported): DIFFERS\n  status: 1 != 2" in out
    assert "  grida-fx said on stderr:\n    grida-fx: boom" in out


def test_main_skips_cases_it_cannot_compare(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    assert _main("nosuch", "yaml-strict", "../linear") == 0
    out = capsys.readouterr().out.splitlines()
    assert out == [
        "nosuch: SKIP no such case in conformance/",
        "yaml-strict: SKIP not ported: FX's strict YAML subset is its own (yaml.md); gnode reads "
        "YAML 1.1",
        "../linear: SKIP not a case name",
        "compare_gnode: 3 skipped",
    ]
    assert engines["calls"] == []


def _lock_drift(where: str) -> dict[str, Any]:
    """lock-drift's graph as either engine prints it, its problem on ``where``."""
    message = (
        "./nodes/n.py#pinned: its source changed but version 2 did not; bump the version, or "
        "confirm no change in behaviour with {} lock --same nodes/n.py#pinned"
    )
    if where == "uses":
        graph = gnode_linear()
        graph["problems"] = [{"where": where, "message": message.format("gnode")}]
    else:
        graph = fx_linear()
        graph["problems"] = [{"where": where, "message": message.format("grida-fx")}]
    return graph


def test_main_ports_a_case_gnode_lacks(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    engines["gnode"][("lock-drift", "expand")] = (1, json.dumps(_lock_drift("uses")), "")
    engines["fx"][("lock-drift", "expand")] = (1, json.dumps(_lock_drift("b")), "")
    assert _main("lock-drift") == 0
    out = capsys.readouterr().out.splitlines()
    # Lock drift on the step's declaration path is a known difference; both expand steps show it.
    assert out == [
        "lock-drift expand (step 1, ported): known (1)",
        "lock-drift expand (step 6, ported): known (1)",
        "compare_gnode: 2 known",
    ]
    argv = ["expand", "case", "--routes", "routes.yaml"]
    assert engines["calls"] == [
        ("gnode", "lock-drift", argv),
        ("fx", "lock-drift", argv),
        ("gnode", "lock-drift", argv),
        ("fx", "lock-drift", argv),
    ]
    # gnode planned FX's project with gnode's names; FX planned its own.
    gnode_files = engines["projects"][0][2]
    fx_files = engines["projects"][1][2]
    assert "gnode.yaml" in gnode_files and "gnode.lock" in gnode_files
    assert "fx.yaml" not in gnode_files and "fx.lock" not in gnode_files
    assert "fx.yaml" in fx_files and "fx.lock" in fx_files
    assert [name.replace("gnode.", "fx.") for name in gnode_files] == fx_files


def test_a_ported_difference_outside_the_known_paths_still_differs(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    fx = _lock_drift("b")
    fx["instances"][1]["state"] = "blocked"
    engines["gnode"][("lock-drift", "expand")] = (1, json.dumps(_lock_drift("uses")), "")
    engines["fx"][("lock-drift", "expand")] = (1, json.dumps(fx), "")
    assert _main("lock-drift") == 1
    out = capsys.readouterr().out.splitlines()
    assert out[:2] == [
        "lock-drift expand (step 1, ported): DIFFERS",
        '  $.instances[draw#1].state: "planned" != "blocked"',
    ]


def test_main_knows_a_graph_only_fx_prints(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    # resource-missing: gnode stops with a traceback; FX refuses the plan cleanly, both exit 1.
    engines["gnode"][("resource-missing", "expand")] = (1, "", "Traceback (most recent call)\n")
    engines["fx"][("resource-missing", "expand")] = (1, json.dumps(FX_LINEAR), "")
    assert _main("resource-missing") == 0
    out = capsys.readouterr().out.splitlines()
    assert out == ["resource-missing expand (ported): known (1)", "compare_gnode: 1 known"]


@pytest.mark.parametrize(
    "fx",
    [(101, "", "thread 'main' panicked\n"), (2, "", "grida-fx: boom\n")],
    ids=["panic", "usage error"],
)
def test_a_graph_only_fx_prints_does_not_cover_its_crash(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str], fx: tuple[int, str, str]
) -> None:
    engines["gnode"][("resource-missing", "expand")] = (1, "", "Traceback (most recent call)\n")
    engines["fx"][("resource-missing", "expand")] = fx
    assert _main("resource-missing") == 1
    out = capsys.readouterr().out.splitlines()
    assert out[:2] == ["resource-missing expand (ported): DIFFERS", f"  status: 1 != {fx[0]}"]


def test_main_skips_without_gnode(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.delenv("FX_GNODE_REPO", raising=False)
    assert _main("linear") == 0
    out = capsys.readouterr().out.splitlines()
    assert out[0] == "compare_gnode: SKIP FX_GNODE_REPO is not set: no gnode to compare with"


def test_main_pair_mode(
    engines: dict[str, Any], tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    gnode_dir, fx_dir = tmp_path / "linear", tmp_path / "fx-linear"
    gnode_dir.mkdir()
    fx_dir.mkdir()
    engines["gnode"][("linear", "expand")] = (0, json.dumps(GNODE_LINEAR), "")
    engines["fx"][("fx-linear", "expand")] = (0, json.dumps(FX_LINEAR), "")
    engines["gnode"][("linear", "price")] = (0, json.dumps(GNODE_LINEAR_PRICE), "")
    engines["fx"][("fx-linear", "price")] = (0, json.dumps(FX_LINEAR_PRICE), "")
    pair = ["--pair", str(gnode_dir), str(fx_dir), "--target", "case"]
    flags = ["--routes", "routes.yaml", "--inputs", "inputs.yaml"]
    assert _main(*pair, *flags) == 0
    out = capsys.readouterr().out.splitlines()
    # Neither stand-in prints identities: both exit 2, alike.
    assert out == [
        "pair expand: same",
        "pair price: same",
        "pair identity: same",
        "compare_gnode: 3 same",
    ]
    assert engines["calls"][:2] == [
        ("gnode", "linear", ["expand", *LINEAR_ARGS]),
        ("fx", "fx-linear", ["expand", *LINEAR_ARGS]),
    ]


def test_a_pair_that_differs_is_shown_the_decisions_no_case_exercises(
    engines: dict[str, Any], tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    gnode_dir, fx_dir = tmp_path / "gn", tmp_path / "fx"
    gnode_dir.mkdir()
    fx_dir.mkdir()
    # An assertion message that evaluates to a list: Python's str() against FX's text rules.
    gnode, fx = gnode_linear(), fx_linear()
    gnode["problems"] = [{"where": "workflow.assert[0]", "message": "['a', 'b']"}]
    fx["problems"] = [{"where": "workflow.assert[0]", "message": '["a","b"]'}]
    engines["gnode"][("gn", "expand")] = (1, json.dumps(gnode), "")
    engines["fx"][("fx", "expand")] = (1, json.dumps(fx), "")
    engines["gnode"][("gn", "price")] = (0, json.dumps(GNODE_LINEAR_PRICE), "")
    engines["fx"][("fx", "price")] = (0, json.dumps(FX_LINEAR_PRICE), "")
    assert _main("--pair", str(gnode_dir), str(fx_dir), "--target", "case") == 1
    out = capsys.readouterr().out.splitlines()
    shown = (
        json.dumps("workflow.assert[0]: ['a', 'b']"),
        json.dumps('workflow.assert[0]: ["a","b"]'),
    )
    assert out[:2] == ["pair expand: DIFFERS", f"  $.problems[0]: {shown[0]} != {shown[1]}"]
    heading = "note: pair mode marks nothing known; FX decisions that no case exercises yet:"
    note = out.index(heading)
    listed = [*tool.UNCASED_DECISIONS, tool.SPEC_CHANGES]
    assert out[note + 1 : note + 1 + len(listed)] == [f"  - {entry}" for entry in listed]
    assert out[-1] == "compare_gnode: 2 same, 1 differs"


@pytest.mark.parametrize(
    "args",
    [
        ["--target", "case"],
        ["--routes", "r.yaml", "linear"],
        ["--pair", "a", "b"],
        ["--pair", "a", "b", "--target", "case", "linear"],
    ],
)
def test_main_refuses_mixed_modes(args: list[str]) -> None:
    with pytest.raises(SystemExit) as refused:
        _main(*args)
    assert refused.value.code == 2


# --- run mode -----------------------------------------------------------------------------------

# `project runs/one` of the run-project case, as each engine prints it (abbreviated).
GNODE_PROJECT: dict[str, Any] = {
    "instances": {
        "joined#1": {"cache": "miss", "facts": {}, "path": "joined", "state": "succeeded"},
        "bang#1": {"error": "gnode/x@1 failed; see gnode nodes", "path": "bang", "state": "failed"},
    },
    "run": {
        "run_finished": {
            "charged_usd": 0.0,
            "event": "run_finished",
            "failed": ["bang#1"],
            "graph_sha256": "8d" * 32,
            "invocation_id": "6ec40721facb4db5",
            "offset_ms": 5,
            "ok": False,
            "outputs": {"all": {"file": {"digest": "d2" * 32, "kind": "text/plain"}}},
        },
        "run_canceled": {"event": "run_canceled", "charged_usd": 0.0, "graph_sha256": "8d" * 32},
    },
}
FX_PROJECT: dict[str, Any] = {
    "instances": {
        "joined#1": {
            "cache": "miss",
            "facts": {"cost_usd": None},
            "path": "joined",
            "state": "succeeded",
        },
        "bang#1": {"error": "fx/x@1 failed; see grida-fx nodes", "path": "bang", "state": "failed"},
    },
    "run": {
        "run_finished": {
            "charged_usd": 0,
            "event": "run_finished",
            "failed": ["bang#1"],
            "plan": "26" * 32,
            "ok": False,
            "outputs": {"all": {"file": {"digest": "d2" * 32, "kind": "text/plain"}}},
        },
        "run_cancelled": {"event": "run_cancelled", "charged_usd": 0, "plan": "26" * 32},
    },
}


def test_projects_normalise_alike_but_for_the_engines_cost_fact() -> None:
    gnode = tool.normalise_project(copy.deepcopy(GNODE_PROJECT), "gnode")
    fx = tool.normalise_project(copy.deepcopy(FX_PROJECT), "fx")
    assert gnode["run"]["run_cancelled"] == {"event": "run_cancelled", "charged_usd": 0}
    assert "graph_sha256" not in gnode["run"]["run_finished"]
    assert "plan" not in fx["run"]["run_finished"]
    # Output digests stay: both engines store the same bytes under the same digest.
    assert fx["run"]["run_finished"]["outputs"]["all"]["file"]["digest"] == "d2" * 32
    assert tool.compare(gnode, fx) == ['$.instances["joined#1"].facts.cost_usd: (absent) != null']


def test_fx_project_errors_keep_their_names() -> None:
    fx = copy.deepcopy(FX_PROJECT)
    fx["instances"]["bang#1"]["error"] = "see gnode nodes"
    assert tool.normalise_project(fx, "fx")["instances"]["bang#1"]["error"] == "see gnode nodes"
    with pytest.raises(ValueError):
        tool.normalise_project({}, "other")


def test_identities_normalise_to_whether_they_are_known() -> None:
    assert tool.normalise_identity({"a#1": "ab" * 32, "b#1": None, "c#1": "not a digest"}) == {
        "a#1": tool.DIGEST,
        "b#1": None,
        "c#1": "not a digest",
    }


def test_events_normalise_without_envelope_and_sorted() -> None:
    gnode = [
        {
            "event": "node_retry",
            "id": "a#1",
            "attempt": 1,
            "error": "see gnode nodes",
            "kind": "gnode-run-events-v1",
            "schema_version": 1,
            "graph_sha256": "8d" * 32,
            "invocation_id": "x",
            "offset_ms": 3,
        },
        {"event": "node_started", "id": "a#1", "identity": "ab" * 32, "with": {"offset_ms": 1}},
        {"event": "run_canceled", "charged_usd": 0.0, "duration_ms": 9},
    ]
    # Sorted by their compact text, as the conformance suite sorts them.
    assert tool.normalise_events(gnode, "gnode") == [
        {"attempt": 1, "error": "see grida-fx nodes", "event": "node_retry", "id": "a#1"},
        {"charged_usd": 0, "event": "run_cancelled"},
        # Data is never touched: a member of `with` may have any name.
        {"event": "node_started", "id": "a#1", "identity": tool.DIGEST, "with": {"offset_ms": 1}},
    ]
    fx = [{"event": "run_cancelled", "kind": "fx-run-events-v1", "plan": "26" * 32}]
    assert tool.normalise_events(fx, "fx") == [{"event": "run_cancelled"}]


def test_texts_compare_by_line_with_fx_names() -> None:
    reroll = b"case.takes.yaml: a uses take 2 from now on\nnext      gnode run case\n"
    assert tool.text_lines(reroll, "gnode") == [
        "case.takes.yaml: a uses take 2 from now on",
        "next      grida-fx run case",
    ]
    header = b"# t.takes.yaml: written by `gnode reroll` and `gnode pick`; commit it\n"
    assert tool.text_lines(header, "gnode") == [
        "# t.takes.yaml: written by `grida-fx reroll` and `grida-fx pick`; commit it"
    ]
    assert tool.text_lines(b'{"kind": "gnode-annotations-v1"}', "gnode") == [
        '{"kind": "fx-annotations-v1"}'
    ]
    # FX's own texts keep what they say.
    assert tool.text_lines(b"next      gnode run case", "fx") == ["next      gnode run case"]
    picture = b"\x89PNG\r\n\x1a\n\xff"
    assert tool.text_lines(picture, "fx") == [
        "sha256:" + __import__("hashlib").sha256(picture).hexdigest()
    ]


def test_absolute_paths_into_a_copy_are_made_relative(tmp_path: Path) -> None:
    project = tmp_path / "project"
    project.mkdir()
    real = project.resolve()
    raw = (
        f"run       {project}/runs/one\n"
        f"delivered {real}/out/x.txt\n"
        f"in {project}\n"
        f"kept {project}-other/x and {project}x\n"
    ).encode()
    assert tool.scrub_roots(raw, [project]).decode().splitlines() == [
        "run       runs/one",
        "delivered out/x.txt",
        "in .",
        f"kept {project}-other/x and {project}x",
    ]


def test_a_case_with_a_run_verb_is_a_run_case() -> None:
    assert tool.is_run_case([{"argv": ["expand", "case"]}, {"argv": ["project", "runs/one"]}])
    assert tool.is_run_case([{"files": {"a": "b"}}, {"argv": ["takes", "list", "case"]}])
    assert not tool.is_run_case([{"argv": ["expand", "case"]}, {"argv": ["lock"]}])
    assert not tool.is_run_case([{"files": {"a": "b"}}, {"read": "fx.lock", "save": "x"}])


def test_what_a_step_saves_is_compared_by_its_kind() -> None:
    def ran(saved: bytes | None) -> Any:
        return tool.StepRun(0, "", saved)

    project = json.dumps(FX_PROJECT).encode()
    run = {"argv": ["run", "case"]}
    assert tool.step_document(run, ran(b"run       runs/one\n"), "fx") is tool._NOTHING
    saved_project = {"argv": ["project", "runs/one"], "save": "p.json"}
    assert tool.step_document(saved_project, ran(project), "fx") == tool.normalise_project(
        FX_PROJECT, "fx"
    )
    assert tool.step_document(saved_project, ran(b"oops"), "fx") is tool._NOT_JSON
    read = {"read": "runs/one/outputs/said.txt", "save": "said.txt"}
    assert tool.step_document(read, ran(b"one\ntwo\n"), "fx") == ["one", "two"]
    assert tool.step_document(read, ran(None), "fx") is tool._ABSENT
    # A file read with json: true is plain JSON, its run-event timings dropped.
    report = {"read": "r.json", "save": "r.json", "json": True}
    assert tool.step_document(report, ran(b'{"half": 2.0, "offset_ms": 1}'), "fx") == {
        "half": 2,
        "offset_ms": 1,
    }
    events = {"read": "runs/one/events.jsonl", "save": "events.json", "jsonl": True}
    lines = b'{"event":"run_started","offset_ms":3}\n\n{"event":"node_started","id":"a#1"}\n'
    assert tool.step_document(events, ran(lines), "fx") == [
        {"event": "node_started", "id": "a#1"},
        {"event": "run_started"},
    ]
    assert tool.step_document(events, ran(b"{not json\n"), "fx") is tool._NOT_JSON
    lock = {"read": "fx.lock", "save": "fx.lock.json", "yaml": True}
    assert tool.step_document(lock, ran(b"nodes:\n  a: 1.0\n"), "fx") == {"nodes": {"a": 1}}


# A stand-in engine for run mode: keeps a counter in its project, so each step sees the last.
STEP_PROBE = """\
import json, os, sys
from pathlib import Path
counter = Path("count.txt")
runs = int(counter.read_text()) + 1 if counter.exists() else 1
counter.write_text(str(runs))
Path("out").mkdir(exist_ok=True)
Path("out", "seen.txt").write_text(f"{os.getcwd()}/runs/one")
print(json.dumps({"argv": sys.argv[1:], "runs": runs, "where": os.getcwd(),
                  "names": sorted(p.name for p in Path(".").iterdir())}))
sys.exit(runs)
"""


def test_run_mode_runs_every_step_in_one_copy(tmp_path: Path) -> None:
    case_in = tmp_path / "in"
    case_in.mkdir()
    (case_in / "fx.yaml").write_text("fx: project/v1\n", "utf-8")
    script = tmp_path / "probe.py"
    script.write_text(STEP_PROBE, "utf-8")
    steps = [
        {"argv": ["run", "case"]},
        {"files": {"fx.lock": "fx: lock/v1\nnodes: {}\n"}, "argv": ["project", "fx.yaml"]},
        {"read": "out/seen.txt", "save": "seen.txt"},
        {"read": "out/missing.txt", "save": "missing.txt"},
    ]
    command = [sys.executable, str(script)]
    fx = tool._run_steps(command, case_in, steps, {}, "grida-fx")
    gnode = tool._run_steps(command, case_in, steps, {}, "gnode")
    assert [ran.status for ran in fx] == [1, 2, None, None]
    first, second = (json.loads(ran.saved) for ran in fx[:2])
    # Every step runs in the same copy: the counter goes on, and nothing is left in the case.
    assert first["runs"] == 1 and second["runs"] == 2
    assert first["where"] == "." and second["argv"] == ["project", "fx.yaml"]
    assert second["names"] == ["count.txt", "fx.lock", "fx.yaml", "out"]
    assert fx[2].saved == b"runs/one" and fx[3].saved is None
    # gnode gets its names: in the files written, the arguments and the paths read.
    assert json.loads(gnode[1].saved)["argv"] == ["project", "gnode.yaml"]
    assert json.loads(gnode[1].saved)["names"] == ["count.txt", "fx.yaml", "gnode.lock", "out"]
    assert sorted(path.name for path in case_in.iterdir()) == ["fx.yaml"]


def _project_steps(
    gnode: dict[str, Any], fx: dict[str, Any], statuses: tuple[int, int] = (0, 0)
) -> tuple[list[Any], list[Any]]:
    """run-project's two steps (run, then project) as each engine did them."""
    return (
        [
            tool.StepRun(statuses[0], "", b"run       runs/one\n"),
            tool.StepRun(0, "", json.dumps(gnode).encode()),
        ],
        [
            tool.StepRun(statuses[1], "", b"run       runs/one\n"),
            tool.StepRun(0, "", json.dumps(fx).encode()),
        ],
    )


def test_main_compares_a_run_case_step_by_step(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    gnode = {"instances": {"joined#1": {"facts": {}, "state": "succeeded"}}, "run": {}}
    fx = {"instances": {"joined#1": {"facts": {"cost_usd": None}, "state": "succeeded"}}}
    fx["run"] = {}
    g_steps, f_steps = _project_steps(gnode, fx)
    engines["steps"][("gnode", "run-project")] = g_steps
    engines["steps"][("fx", "run-project")] = f_steps
    assert _main("run-project") == 0
    out = capsys.readouterr().out.splitlines()
    # run-project is in gnode's suite: its original is run, not a port.
    assert out == [
        "run-project step 1 run: same",
        "run-project project.json: known (1)",
        "compare_gnode: 1 same, 1 known",
    ]
    assert engines["calls"] == [("gnode", "run-project", "steps"), ("fx", "run-project", "steps")]


def test_main_reports_a_run_case_that_differs(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    gnode = {"instances": {"joined#1": {"state": "succeeded"}}}
    fx = {"instances": {"joined#1": {"state": "failed", "error": "kaboom"}}}
    g_steps, f_steps = _project_steps(gnode, fx, statuses=(0, 1))
    f_steps[0] = tool.StepRun(1, "grida-fx: kaboom\n", b"")
    engines["steps"][("gnode", "run-project")] = g_steps
    engines["steps"][("fx", "run-project")] = f_steps
    assert _main("run-project") == 1
    out = capsys.readouterr().out.splitlines()
    assert out[:4] == [
        "run-project step 1 run: DIFFERS",
        "  status: 0 != 1",
        "  grida-fx said on stderr:",
        "    grida-fx: kaboom",
    ]
    assert "run-project project.json: DIFFERS" in out
    assert '  $.instances["joined#1"].state: "succeeded" != "failed"' in out
    assert out[-1] == "compare_gnode: 2 differs"


def test_steps_that_save_one_name_are_told_apart(
    engines: dict[str, Any], capsys: pytest.CaptureFixture[str]
) -> None:
    # run-local saves parts-1.txt twice (an output and a step file), which must match.
    assert _main("run-local") == 0
    out = capsys.readouterr().out.splitlines()
    assert "run-local parts-1.txt (step 6, ported): same" in out
    assert "run-local parts-1.txt (step 10, ported): same" in out
    assert "run-local step 1 run (ported): same" in out


def test_a_run_case_that_times_out_differs(
    engines: dict[str, Any], monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    def hangs(*_: Any) -> Any:
        raise tool.conformance.CaseFailure("gnode run case timed out after 120 s")

    monkeypatch.setattr(tool, "run_gnode_steps", hangs)
    assert _main("run-local") == 1
    out = capsys.readouterr().out.splitlines()
    assert out[:2] == ["run-local (ported): DIFFERS", "  gnode run case timed out after 120 s"]


# --- end to end ---------------------------------------------------------------------------------


@pytest.mark.skipif(
    not os.environ.get("FX_GNODE_REPO") or not FX_BINARY.is_file(),
    reason="needs FX_GNODE_REPO and a built target/debug/grida-fx",
)
def test_linear_and_run_project_compare_on_both_engines(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    if not os.environ.get("GRIDA_FX_PYTHON"):
        # This interpreter has the grida package: the Python host plans the case's node types.
        monkeypatch.setenv("GRIDA_FX_PYTHON", sys.executable)
    status = tool.main(["--command", str(FX_BINARY), "linear", "run-project"])
    out = capsys.readouterr().out
    assert status == 0, out
    assert "linear expand: same" in out
    assert "linear identity: same" in out
    assert "linear price: same" in out
    # A run: the same states, outputs and events, but for the engine's cost_usd fact.
    assert "run-project step 1 run: same" in out
    assert "run-project project.json: known (3)" in out
