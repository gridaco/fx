"""Tests for tools/digest.py, the independent identity checker."""

from __future__ import annotations

import importlib.util
import json
import random
import struct
import sys
from pathlib import Path
from typing import Any

import pytest
import rfc8785

REPO = Path(__file__).resolve().parents[2]
EXAMPLES_FILE = REPO / "spec" / "vectors" / "identity" / "examples.json"
JCS_FILE = REPO / "spec" / "vectors" / "jcs" / "cases.json"


def _load_digest() -> Any:
    spec = importlib.util.spec_from_file_location("fx_tools_digest", REPO / "tools" / "digest.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


digest = _load_digest()

EXAMPLES = {e["name"]: e for e in json.loads(EXAMPLES_FILE.read_text("utf-8"))["examples"]}
FILE = EXAMPLES["file"]["digest"]
ROUTE = EXAMPLES["route"]["digest"]
EDIT_ROUTE = EXAMPLES["route_with_contract"]["digest"]
LOCAL_STEP = EXAMPLES["local_step"]["digest"]
BUILTIN_STEP = EXAMPLES["builtin_step"]["digest"]
CALL = EXAMPLES["call"]["digest"]
NODE_SOURCE = EXAMPLES["node_source"]["digest"]
MARKERS_FILE = REPO / "spec" / "vectors" / "identity" / "markers.json"
JSON_OUTPUT_FILE = REPO / "spec" / "vectors" / "identity" / "json_output.json"
MARKER_CASES = json.loads(MARKERS_FILE.read_text("utf-8"))["cases"]
JSON_OUTPUT_CASES = json.loads(JSON_OUTPUT_FILE.read_text("utf-8"))["cases"]


# --- the identity vectors ---------------------------------------------------------------------


@pytest.mark.parametrize("name", sorted(EXAMPLES))
def test_identity_example_reproduces(name: str) -> None:
    example = EXAMPLES[name]
    if "instance_id" in example:
        got = digest.instance_id(example["path"], example["take"])
        assert got == example["instance_id"]
        return
    if "file_utf8" in example:
        data = example["file_utf8"].encode("utf-8")
        assert digest.file_digest(data) == example["digest"]
        if "text" in example:
            path_text = data.decode("utf-8").removeprefix("\ufeff")
            assert path_text == example["text"]
        return
    if "object_source" in example:
        value = digest.parse_ijson(example["object_source"])
    else:
        value = example["object"]
    assert digest.canon(value).decode("utf-8") == example["canonical"]
    assert digest.digest(value) == example["digest"]
    # An independent canonicalizer agrees.
    assert rfc8785.dumps(value).decode("utf-8") == example["canonical"]


def test_check_examples_reports_nothing() -> None:
    assert digest.check_examples(EXAMPLES_FILE) == []
    assert digest.check_marker_vectors(MARKERS_FILE) == []
    assert digest.check_json_output_vectors(JSON_OUTPUT_FILE) == []


def test_check_examples_covers_every_formula() -> None:
    kinds = {e["object"]["kind"] for e in EXAMPLES.values() if "object" in e}
    assert kinds == {
        "fx-route-v1",
        "fx-step-v1",
        "fx-call-v1",
        "fx-node-source-v1",
        "fx-plan-v1",
    }
    assert sum("instance_id" in e for e in EXAMPLES.values()) >= 2


def test_check_examples_catches_a_wrong_plan_and_instance_id(tmp_path: Path) -> None:
    document = json.loads(EXAMPLES_FILE.read_text("utf-8"))
    for example in document["examples"]:
        if example["name"] == "plan":
            # An unsorted routes list is not the plan object of section 10.
            example["object"]["routes"].reverse()
            example["canonical"] = rfc8785.dumps(example["object"]).decode("utf-8")
            example["digest"] = digest.digest(example["object"])
        if example["name"] == "instance_id_quote":
            example["instance_id"] = "entity['it's'].draw#1"
    path = tmp_path / "examples.json"
    path.write_text(json.dumps(document), "utf-8")
    failures = digest.check_examples(path)
    assert any(line.startswith("plan: the fx-plan-v1 formula") for line in failures)
    assert any(line.startswith("instance_id_quote: instance id") for line in failures)


def test_check_examples_catches_a_wrong_digest(tmp_path: Path) -> None:
    document = json.loads(EXAMPLES_FILE.read_text("utf-8"))
    document["examples"][2]["digest"] = "0" * 64
    path = tmp_path / "examples.json"
    path.write_text(json.dumps(document), "utf-8")
    failures = digest.check_examples(path)
    assert failures and all(line.startswith("route:") for line in failures)
    assert digest.main(["--check-examples", "--examples", str(path)]) == 1


# --- the formulas -----------------------------------------------------------------------------


def test_route_fingerprint() -> None:
    assert digest.route_fingerprint("image.generate", "img-a@acme") == ROUTE
    assert digest.route_fingerprint("image.generate", "img-a@acme", {}) == ROUTE
    assert digest.route_fingerprint("image.generate", "img-a@acme", {"size": 1024}) != ROUTE
    assert digest.route_fingerprint("image.edit", "img-a@acme") != ROUTE


def test_route_splits_at_the_last_at() -> None:
    assert digest.split_route("acme/img-a@2@other") == ("acme/img-a@2", "other")
    with pytest.raises(digest.RefusedInput):
        digest.split_route("img-a")
    with pytest.raises(digest.RefusedInput):
        digest.split_route("img-a@")


def test_step_identity_local() -> None:
    with_values = {"text": {"file": FILE}}
    assert digest.step_identity("nodes/cases.py#lines@1", with_values, {}, [1]) == LOCAL_STEP


def test_step_identity_builtin() -> None:
    with_values = {"prompt": "A picture of 2 lines", "background": "auto", "vars": {}}
    routes = {"image.generate": ROUTE}
    assert digest.step_identity("fx/image.generate@1.1", with_values, routes, [1]) == BUILTIN_STEP
    assert digest.step_identity("fx/image.generate@1.1", with_values, routes, [2]) != BUILTIN_STEP
    assert digest.step_identity("fx/image.generate@1.2", with_values, routes, [1]) != BUILTIN_STEP


def test_call_key() -> None:
    request = {"prompt": "A picture of 2 lines", "background": "auto"}
    assert digest.call_key("image.generate", ROUTE, request, [1]) == CALL


def test_node_source_from_file_digests() -> None:
    files = {"nodes/n.py": digest.file_digest(b"x = 1\n")}
    resources = {"prompts/r.md": digest.file_digest(b"Hello\n")}
    assert digest.node_source_digest(files, resources) == NODE_SOURCE
    # A resource is not a module: the same file under files gives another identity.
    assert digest.node_source_digest({**files, **resources}, {}) != NODE_SOURCE


def test_file_digest_never_normalizes() -> None:
    assert digest.file_digest(b"one\ntwo\n") == FILE
    assert digest.file_digest(b"one\r\ntwo\r\n") != FILE
    assert digest.file_digest(b"\xef\xbb\xbfone\ntwo\n") != FILE


def test_plan_routes_are_a_sorted_distinct_list() -> None:
    object_ = digest.plan_object({}, {}, {}, {}, [EDIT_ROUTE, ROUTE, EDIT_ROUTE])
    assert object_["routes"] == sorted([ROUTE, EDIT_ROUTE])
    assert digest.plan_digest({}, {}, {}, {}, [ROUTE, EDIT_ROUTE]) == digest.plan_digest(
        {}, {}, {}, {}, (EDIT_ROUTE, ROUTE)
    )


@pytest.mark.parametrize(
    ("path", "take", "expected"),
    [
        ([{"step": "draw"}], [1], "draw#1"),
        ([{"step": "build"}, {"step": "draw"}], [2, 1], "build.draw#2.1"),
        ([{"step": "entity", "key": "ada"}, {"step": "draw"}], [1], "entity['ada'].draw#1"),
        ([{"step": "e", "key": "it's"}], [1], "e['it\\'s']#1"),
        ([{"step": "e", "key": "a\\b"}], [1], "e['a\\\\b']#1"),
        ([{"step": "e", "key": True}], [3], "e['true']#3"),
        ([{"step": "e", "key": 1.0}], [1], "e['1']#1"),
        ([{"step": "e", "key": 1e21}], [1], "e['1e+21']#1"),
    ],
)
def test_instance_id(path: list[dict[str, Any]], take: list[int], expected: str) -> None:
    assert digest.instance_id(path, take) == expected


def test_instance_id_refuses_a_key_that_is_not_a_scalar() -> None:
    with pytest.raises(digest.RefusedInput):
        digest.instance_id([{"step": "e", "key": {"a": 1}}], [1])


# --- writing JSON (identity.md section 5) -----------------------------------------------------


@pytest.mark.parametrize("case", JSON_OUTPUT_CASES, ids=lambda case: case["name"])
def test_json_output_vector(case: dict[str, Any]) -> None:
    value = digest.parse_ijson(case["value_json"])
    assert digest.write_json(value) == case["bytes_utf8"].encode("utf-8")
    # The written file reads back as the same value: 1e16 is written 10000000000000000, an
    # integer literal that section 1 reads without rounding.
    assert digest.canon(digest.parse_ijson(digest.write_json(value))) == digest.canon(value)


def _python_writes_the_same(value: Any) -> bool:
    if isinstance(value, float):
        return not value.is_integer() and "e" not in repr(value)
    if isinstance(value, list):
        return all(_python_writes_the_same(item) for item in value)
    if isinstance(value, dict):
        return all(
            all(ord(c) <= 0xFFFF for c in k) and _python_writes_the_same(v)
            for k, v in value.items()
        )
    return True


def test_json_output_agrees_with_python_where_section_5_says_so() -> None:
    compared = 0
    for case in JSON_OUTPUT_CASES:
        value = digest.parse_ijson(case["value_json"])
        if _python_writes_the_same(value):
            compared += 1
            python = json.dumps(value, sort_keys=True, indent=1, ensure_ascii=False)
            assert digest.write_json(value) == python.encode("utf-8"), case["name"]
    assert compared >= 5


def test_json_output_on_random_values() -> None:
    rng = random.Random(5)

    def make(depth: int) -> Any:
        pick = rng.randrange(7 if depth < 3 else 4)
        if pick == 0:
            return rng.randrange(-(10**6), 10**6)
        if pick == 1:
            return rng.randrange(-(10**6), 10**6) / 8 + 1 / 16
        if pick == 2:
            return "".join(
                chr(rng.choice([0x22, 0x5C, 0x0A, 0x41, 0xE9, 0x20AC])) for _ in range(3)
            )
        if pick == 3:
            return rng.choice([None, True, False])
        if pick in (4, 5):
            return {f"k{rng.randrange(100)}": make(depth + 1) for _ in range(rng.randrange(4))}
        return [make(depth + 1) for _ in range(rng.randrange(4))]

    for _ in range(500):
        value = make(0)
        python = json.dumps(value, sort_keys=True, indent=1, ensure_ascii=False)
        assert digest.write_json(value) == python.encode("utf-8")


# --- reserved plain markers (identity.md section 3) -------------------------------------------


@pytest.mark.parametrize("case", MARKER_CASES, ids=lambda case: case["name"])
def test_marker_vector(case: dict[str, Any]) -> None:
    # Every case is an I-JSON value: a reader of runtime values (a graph) reads it.
    plain = digest.parse_ijson(case["json"])
    for read in (
        lambda: digest.parse_ijson(case["json"], authored=True),
        lambda: digest.load_yaml(case["json"]),
    ):
        if case["refuse"]:
            with pytest.raises(digest.RefusedInput) as refused:
                read()
            assert refused.value.code == "reserved_marker"
        else:
            assert digest.canon(read()) == digest.canon(plain)


def test_read_json_file_refuses_markers_only_when_authored(tmp_path: Path) -> None:
    path = tmp_path / "inputs.json"
    path.write_text(json.dumps({"brief": {"file": FILE}}), "utf-8")
    assert digest.read_json_file(path) == {"brief": {"file": FILE}}
    with pytest.raises(digest.RefusedInput) as refused:
        digest.read_json_file(path, authored=True)
    assert refused.value.code == "reserved_marker"


# --- canonical JSON ---------------------------------------------------------------------------

NUMBERS = [
    (0.0, "0"),
    (-0.0, "0"),
    (1.0, "1"),
    (-1.5, "-1.5"),
    (0.1 + 0.2, "0.30000000000000004"),
    (1e16, "10000000000000000"),
    (1e20, "100000000000000000000"),
    (1e21, "1e+21"),
    (1e-6, "0.000001"),
    (1e-7, "1e-7"),
    (123e-20, "1.23e-18"),
    (5e-324, "5e-324"),
    (1.7976931348623157e308, "1.7976931348623157e+308"),
    (2.9514790517935283e20, "295147905179352830000"),
    (9.999999999999997e22, "9.999999999999997e+22"),
    (333333333.3333333, "333333333.3333333"),
    (9007199254740991, "9007199254740991"),
    (-9007199254740991, "-9007199254740991"),
]


@pytest.mark.parametrize(("value", "text"), NUMBERS)
def test_numbers_serialize_as_ecmascript(value: float, text: str) -> None:
    assert digest.canon(value).decode("ascii") == text


def _reads_back(value: Any) -> None:
    """What canon and write_json write, the JSON and YAML readers read back as the same value."""
    text = digest.canon(value)
    assert digest.canon(digest.parse_ijson(text)) == text, repr(value)
    assert digest.canon(digest.parse_ijson(digest.write_json(value))) == text, repr(value)


def test_every_canonical_number_reads_back() -> None:
    # Identity.md section 1 refuses an integer literal only where reading it would round it, so
    # every number canon writes reads back, whole numbers from 2^53 to 1e21 included.
    rng = random.Random(53)
    edges = [2.0**53, -(2.0**53), 2.0**53 + 2, 2.0**60, 1e16, 1e20, 9.999999999999999e20, 1e21]
    integral = [float(rng.randrange(2**53, 10**21)) for _ in range(5000)]
    doubles = []
    while len(doubles) < 20000:
        (value,) = struct.unpack("<d", rng.getrandbits(64).to_bytes(8, "little"))
        if value == value and value not in (float("inf"), float("-inf")):
            doubles.append(value)
    for value in edges + integral + doubles:
        _reads_back(value)
        _reads_back({"n": [value, -value]})
    for value in edges + integral[:500] + doubles[:500]:
        text = digest.canon(value).decode("ascii")
        assert digest.canon(digest.load_yaml(f"n: {text}\n")["n"]) == text.encode(), text
    # A Python int is a value when canon writes its own digits, so it reads back too.
    for number in [0, 2**53 - 1, 2**53, -(2**53), 10**16, 10**20, 999999999999999900000]:
        assert digest.canon(number) == str(number).encode()
        _reads_back(number)


def test_numbers_agree_with_rfc8785_on_random_doubles() -> None:
    rng = random.Random(8785)
    for _ in range(20000):
        (value,) = struct.unpack("<d", rng.getrandbits(64).to_bytes(8, "little"))
        if value != value or value in (float("inf"), float("-inf")):
            continue
        assert digest.canon(value) == rfc8785.dumps(value), repr(value)


def test_strings_escape_minimally() -> None:
    text = "".join(chr(c) for c in range(0x80)) + "\u2028\u2029\u00e9\U0001f600\ufeff"
    assert digest.canon(text) == rfc8785.dumps(text)
    assert digest.canon("\x1f\x7f") == b'"\\u001f\x7f"'
    assert digest.canon('"\\/') == b'"\\"\\\\/"'


def test_keys_sort_by_utf16_code_units() -> None:
    value = {"\ufb33": 1, "\U0001f600": 2, "\u20ac": 3, "\r": 4, "1": 5, "\u0080": 6}
    # U+1F600 is the surrogate pair D83D DE00, which sorts before U+FB33 in UTF-16.
    canonical = digest.canon(value).decode("utf-8")
    assert canonical == '{"\\r":4,"1":5,"\u0080":6,"\u20ac":3,"\U0001f600":2,"\ufb33":1}'
    assert digest.canon(value) == rfc8785.dumps(value)


def _jcs_cases() -> list[dict[str, Any]]:
    if not JCS_FILE.is_file():
        return []
    return json.loads(JCS_FILE.read_text("utf-8"))["cases"]


@pytest.mark.parametrize("case", _jcs_cases(), ids=lambda case: case["name"])
def test_jcs_vector(case: dict[str, Any]) -> None:
    if "refuse" in case:
        with pytest.raises(digest.RefusedInput) as refused:
            digest.parse_ijson(case["input"])
        assert refused.value.code == case["refuse"]
    else:
        value = digest.parse_ijson(case["input"])
        assert digest.canon(value).decode("utf-8") == case["canonical"]


# --- I-JSON refusals --------------------------------------------------------------------------

REFUSED_TEXT = [
    ("NaN", "non_finite_number"),
    ("[Infinity]", "non_finite_number"),
    ('{"a": -Infinity}', "non_finite_number"),
    ("9007199254740993", "integer_out_of_range"),
    ("-9007199254740993", "integer_out_of_range"),
    ('[{"a": 12345678901234567891}]', "integer_out_of_range"),
    # 2^60 exactly, but its canonical form is 1152921504606847000.
    ("1152921504606846976", "integer_out_of_range"),
    # 21 digits that round to 1e21, whose canonical form has an exponent; 22 digits by length.
    ("999999999999999999999", "integer_out_of_range"),
    ("1000000000000000000000", "integer_out_of_range"),
    ("1" + "0" * 5000, "integer_out_of_range"),
    ("-" + "9" * 5000, "integer_out_of_range"),
    ("1e400", "number_overflow"),
    ("-1.8e308", "number_overflow"),
    ('"\\ud800"', "lone_surrogate"),
    ('"\\ude00\\ud83d"', "lone_surrogate"),
    ('{"\\udc00": 1}', "lone_surrogate"),
    ('{"a": 1, "a": 1}', "duplicate_key"),
    ('{"a": 1, "\\u0061": 2}', "duplicate_key"),
    ("[1,]", "invalid_json"),
    ("01", "invalid_json"),
    ("\ufeff{}", "invalid_json"),
]


@pytest.mark.parametrize(("text", "code"), REFUSED_TEXT)
def test_ijson_reader_refuses(text: str, code: str) -> None:
    with pytest.raises(digest.RefusedInput) as refused:
        digest.parse_ijson(text)
    assert refused.value.code == code


def test_ijson_reader_refuses_invalid_utf8() -> None:
    with pytest.raises(digest.RefusedInput) as refused:
        digest.parse_ijson(b'"\xff"')
    assert refused.value.code == "invalid_utf8"


def test_ijson_reader_accepts_the_edges() -> None:
    assert digest.parse_ijson("[9007199254740991, -9007199254740991]") == [
        9007199254740991,
        -9007199254740991,
    ]
    # An integer literal that reads without rounding is accepted, however large: its digits
    # are the canonical form of the number it reads as.
    for text in [
        "9007199254740992",
        "-9007199254740992",
        "10000000000000000",
        "1152921504606847000",
        "999999999999999900000",
    ]:
        assert digest.canon(digest.parse_ijson(text)) == text.encode()
    assert digest.canon(digest.parse_ijson("-0")) == b"0"
    # A float literal is a binary64 number, rounded as usual.
    assert digest.canon(digest.parse_ijson("9007199254740992.0")) == b"9007199254740992"
    assert digest.canon(digest.parse_ijson("9007199254740993.0")) == b"9007199254740992"
    assert digest.canon(digest.parse_ijson('"\\ud83d\\ude00"')) == '"\U0001f600"'.encode()


@pytest.mark.parametrize(
    ("value", "code"),
    [
        (float("nan"), "non_finite_number"),
        ([float("inf")], "non_finite_number"),
        ({"a": 2**53 + 1}, "integer_out_of_range"),
        (-(2**53) - 1, "integer_out_of_range"),
        (2**60, "integer_out_of_range"),
        (10**21, "integer_out_of_range"),
        (10**400, "integer_out_of_range"),
        ({1: "a"}, "non_string_key"),
        ("\ud800", "lone_surrogate"),
        ({"\udfff": 1}, "lone_surrogate"),
        ({"a"}, "not_a_value"),
    ],
)
def test_canon_refuses_values_outside_ijson(value: Any, code: str) -> None:
    with pytest.raises(digest.RefusedInput) as refused:
        digest.canon(value)
    assert refused.value.code == code


def test_canon_refuses_an_int_of_thousands_of_digits() -> None:
    # Refused by its size, before Python would have to print or convert it.
    with pytest.raises(digest.RefusedInput) as refused:
        digest.canon([10**5000])
    assert refused.value.code == "integer_out_of_range"


def test_read_json_file_strips_one_bom(tmp_path: Path) -> None:
    path = tmp_path / "v.json"
    path.write_bytes(b"\xef\xbb\xbf[1.0]")
    assert digest.read_json_file(path) == [1.0]
    path.write_bytes(b"\xef\xbb\xbf\xef\xbb\xbf[1.0]")
    with pytest.raises(digest.RefusedInput):
        digest.read_json_file(path)


# --- the strict YAML subset -------------------------------------------------------------------


@pytest.mark.parametrize(
    ("text", "value"),
    [
        ("", {}),
        ("# only a comment\n", {}),
        ("a: 1\nb: 1.0\nc: 1e3\nd: .5\n", {"a": 1, "b": 1.0, "c": 1000.0, "d": 0.5}),
        ("on: 1\ntrue: 2\n1: 3\n", {"on": 1, "true": 2, "1": 3}),
        ("a:\nb: ~\nc: null\n", {"a": None, "b": None, "c": None}),
        ("a: y\nb: n\n", {"a": "y", "b": "n"}),
        ("a: \"on\"\nb: '017'\n", {"a": "on", "b": "017"}),
        ("a: |\n  x\n", {"a": "x\n"}),
        ("---\na: 1\n", {"a": 1}),
        ("\ufeffa: 1\n", {"a": 1}),
        ("---\n", {}),
        ("--- # c\n", {}),
        ("---\n...\n", {}),
        ("a: 1\n...\n", {"a": 1}),
        ("--- ~\n", None),
        ("- a\n- 1\n", ["a", 1]),
        ("--- 5\n", 5),
        ('"<<": 1\n', {"<<": 1}),
        ("a: -0\nb: +0\nc: -0.0\n", {"a": 0, "b": 0, "c": 0}),
        (
            "a: 0bad\nb: 0ops\nc: -.nan\nd: +.nan\ne: 0x\n",
            {"a": "0bad", "b": "0ops", "c": "-.nan", "d": "+.nan", "e": "0x"},
        ),
        ("a: _\nb: 1_a\nc: v1_2\n", {"a": "_", "b": "1_a", "c": "v1_2"}),
        ("a: 0.5\nb: 0e5\nc: 0.\n", {"a": 0.5, "b": 0, "c": 0}),
        ("a: y\nb: Truth\nc: yesterday\n", {"a": "y", "b": "Truth", "c": "yesterday"}),
        # Integers that read without rounding, beyond 2^53 - 1 too.
        (
            "a: 9007199254740992\nb: -9007199254740992\nc: +10000000000000000\n"
            "d: 1152921504606847000\n",
            {"a": 2**53, "b": -(2**53), "c": 10**16, "d": 2.0**60},
        ),
        # A short date alone is not a YAML 1.1 timestamp.
        ("a: 2026-1-5\nb: 2026-10-5\n", {"a": "2026-1-5", "b": "2026-10-5"}),
        # A stream holding only end markers is an empty document.
        ("...\n", {}),
        ("...", {}),
        ("# c\n...\n", {}),
        ("\ufeff...\n", {}),
    ],
)
def test_yaml_accepts(text: str, value: Any) -> None:
    got = digest.load_yaml(text)
    assert got == value
    assert digest.canon(got) == digest.canon(value)


@pytest.mark.parametrize(
    ("text", "code"),
    [
        ("a: on\n", "ambiguous_scalar"),
        ("a: Yes\n", "ambiguous_scalar"),
        ("a: TRUE\n", "ambiguous_scalar"),
        ("a: 017\n", "ambiguous_scalar"),
        ("a: 0x1f\n", "ambiguous_scalar"),
        ("a: 1_000\n", "ambiguous_scalar"),
        ("a: 1:30\n", "ambiguous_scalar"),
        ("a: .inf\n", "ambiguous_scalar"),
        ("a: -.Inf\n", "ambiguous_scalar"),
        ("a: .NaN\n", "ambiguous_scalar"),
        ("a: 2026-10-05\n", "ambiguous_scalar"),
        ("a: 9007199254740993\n", "integer_out_of_range"),
        ("a: -9007199254740993\n", "integer_out_of_range"),
        ("a: +9007199254740993\n", "integer_out_of_range"),
        ("a: 1e400\n", "number_overflow"),
        ("a: 1\na: 2\n", "duplicate_key"),
        ("a: &x 1\n", "anchor"),
        ("a: !!str 1\n", "tag"),
        ("%YAML 1.2\n---\na: 1\n", "directive"),
        ("a: 1\n---\nb: 2\n", "multiple_documents"),
        ("<<: {a: 1}\n", "merge_key"),
        ("? [a]\n: 1\n", "complex_key"),
        ("? name\n: acme\n", "complex_key"),
        ("{[a]: 1}\n", "complex_key"),
        (b"a: \xff\n", "invalid_utf8"),
        # Words in any letter case; true, false and null in any but all lower case.
        ("a: oN\n", "ambiguous_scalar"),
        ("a: yEs\n", "ambiguous_scalar"),
        ("a: nO\n", "ambiguous_scalar"),
        ("a: oFF\n", "ambiguous_scalar"),
        ("a: tRUE\n", "ambiguous_scalar"),
        ("a: fAlse\n", "ambiguous_scalar"),
        ("a: nULL\n", "ambiguous_scalar"),
        # Leading zeros, in any number form.
        ("a: 01.5\n", "ambiguous_scalar"),
        ("a: 00.5\n", "ambiguous_scalar"),
        ("a: 01e3\n", "ambiguous_scalar"),
        ("a: -017\n", "ambiguous_scalar"),
        ("a: 00\n", "ambiguous_scalar"),
        # 0x, 0o, 0b followed only by digits of that base, either case of the prefix letter.
        ("a: 0X1F\n", "ambiguous_scalar"),
        ("a: 0O17\n", "ambiguous_scalar"),
        ("a: 0B101\n", "ambiguous_scalar"),
        ("a: 0xff_ff\n", "ambiguous_scalar"),
        # ... with an optional sign.
        ("a: -0x1F\n", "ambiguous_scalar"),
        ("a: +0b1\n", "ambiguous_scalar"),
        ("a: -0o17\n", "ambiguous_scalar"),
        # A number once its underscores are removed.
        ("a: .5_0\n", "ambiguous_scalar"),
        ("a: 1_000.5\n", "ambiguous_scalar"),
        ("a: 1_\n", "ambiguous_scalar"),
        ("a: 1e1_0\n", "ambiguous_scalar"),
        ("a: 1_0:30\n", "ambiguous_scalar"),
        # Sexagesimal (with an optional sign and fraction), infinity and NaN.
        ("a: 16:9\n", "ambiguous_scalar"),
        ("a: -1:30\n", "ambiguous_scalar"),
        ("a: +1:30\n", "ambiguous_scalar"),
        ("a: 1:30.5\n", "ambiguous_scalar"),
        ("a: -1:30:00.25\n", "ambiguous_scalar"),
        ("a: +.inf\n", "ambiguous_scalar"),
        ("a: .iNf\n", "ambiguous_scalar"),
        ("a: .nan\n", "ambiguous_scalar"),
        ("a: 1" + "0" * 5000 + "\n", "integer_out_of_range"),
        ("a: 12345678901234567891\n", "integer_out_of_range"),
        # YAML 1.1 timestamps: a two-digit date, or a short date followed by a time.
        ("a: 2026-1-5 9:30:00\n", "ambiguous_scalar"),
        ("a: 2026-1-5T9:30:00.5Z\n", "ambiguous_scalar"),
        ("a: 2026-10-05t10:00:00+09:00\n", "ambiguous_scalar"),
        ("a: 2001-12-14 21:59:43.10 -5\n", "ambiguous_scalar"),
        # Streams.
        ("%FOO bar\n---\na: 1\n", "directive"),
        ("%TAG ! tag:acme.test,2026:\n---\na: 1\n", "directive"),
        ("\ufeff\ufeffa: 1\n", "byte_order_mark"),
        ('a: "x\x85y"\n', "line_break"),
        ("# note\x85\na: 1\n", "line_break"),
        ("a: x\u2028y\n", "line_break"),
        ("a: |\n  x\u2029y\n", "line_break"),
        # Runtime markers are not authored values.
        ("a: {file: " + "a" * 64 + "}\n", "reserved_marker"),
        ("- {missing: true}\n", "reserved_marker"),
        ("a: {pending: x}\n", "reserved_marker"),
    ],
)
def test_yaml_refuses(text: str | bytes, code: str) -> None:
    with pytest.raises(digest.RefusedInput) as refused:
        digest.load_yaml(text)
    assert refused.value.code == code


@pytest.mark.parametrize(
    ("text", "where"),
    [
        ('a: 1\nb: "x\x85y"\n', ":2:6:"),
        ("a: 1\r\nb: x\u2028\n", ":2:5:"),
        ("\ufeffa: |\n  x\u2029\n", ":2:4:"),
    ],
)
def test_yaml_line_breaks_are_refused_with_line_and_column(text: str, where: str) -> None:
    with pytest.raises(digest.RefusedInput) as refused:
        digest.load_yaml(text, "f.yaml")
    assert f"f.yaml{where}" in str(refused.value)


YAML_VECTORS = REPO / "spec" / "vectors" / "yaml"


def _yaml_vectors(kind: str) -> list[Path]:
    folder = YAML_VECTORS / kind
    return sorted(folder.glob("*.yaml")) if folder.is_dir() else []


@pytest.mark.parametrize("path", _yaml_vectors("accept"), ids=lambda path: path.stem)
def test_yaml_accept_vector(path: Path) -> None:
    want = json.loads(path.with_suffix(".json").read_text("utf-8"))
    assert digest.canon(digest.load_yaml(path.read_bytes(), path.name)) == digest.canon(want)


@pytest.mark.parametrize("path", _yaml_vectors("refuse"), ids=lambda path: path.stem)
def test_yaml_refuse_vector(path: Path) -> None:
    with pytest.raises(digest.RefusedInput):
        digest.load_yaml(path.read_bytes(), path.name)


# --- --check-graph ----------------------------------------------------------------------------

ROUTES_YAML = """\
fx: routes/v1
routes:
  - { capability: image.generate, route: img-a@acme, price: { usd: 0.01 } }
  - capability: image.edit
    route: img-a@acme
    price: { usd: 0.02 }
    contract: { mask: true, sizes: [1024x1024, 1536x1024] }
"""
OTHER_ROUTES_YAML = """\
fx: routes/v1
routes:
  - { capability: speech.generate, route: voice-a@acme, price: { usd: 0.01 } }
"""


def _instance(
    name: str,
    uses: str,
    type_identity: str | None,
    with_values: dict[str, Any],
    identity: str | None,
    routes: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "id": name,
        "path": name.split("#")[0],
        "step": name.split("#")[0],
        "take": [1],
        "uses": uses,
        "type": type_identity,
        "with": with_values,
        "routes": routes or {},
        "state": "planned",
        "identity": identity,
        "phase": 1,
        "key": None,
        "judges": None,
        "judged_by": [],
        "waiting_on": [],
        "needs": [],
        "reads": [],
        "price": {"low_usd": 0, "high_usd": 0},
    }


SOURCE_TYPE = "nodes/n.py#echo@source:" + NODE_SOURCE
DRAW_ROUTES = {"image.generate": {"route": "img-a@acme", "fingerprint": ROUTE}}
DRAW_WITH = {"prompt": "A picture of 2 lines", "background": "auto", "vars": {}}
CASES_PY = b"def lines(text):\n    return len(text.splitlines())\n"


@pytest.fixture
def project(tmp_path: Path) -> Path:
    (tmp_path / "nodes").mkdir()
    (tmp_path / "nodes" / "n.py").write_bytes(b"x = 1\n")
    (tmp_path / "nodes" / "cases.py").write_bytes(CASES_PY)
    (tmp_path / "prompts").mkdir()
    (tmp_path / "prompts" / "r.md").write_bytes(b"Hello\n")
    (tmp_path / "routes.yaml").write_text(ROUTES_YAML, "utf-8")
    (tmp_path / "fx.yaml").write_text("fx: project/v1\nroute_tables: [routes.yaml]\n", "utf-8")
    return tmp_path


def _types() -> dict[str, Any]:
    return {
        "./nodes/cases.py#lines": {
            "identity": "nodes/cases.py#lines@1",
            "source": {"files": {"nodes/cases.py": digest.file_digest(CASES_PY)}, "resources": {}},
        },
        "fx/image.generate@1": {"identity": "fx/image.generate@1.1"},
        "./nodes/n.py#echo": {
            "identity": SOURCE_TYPE,
            "source": {
                "files": {"nodes/n.py": digest.file_digest(b"x = 1\n")},
                "resources": {"prompts/r.md": digest.file_digest(b"Hello\n")},
            },
        },
    }


def _graph() -> dict[str, Any]:
    echo = digest.step_identity(SOURCE_TYPE, {"name": "ada"}, {}, [1])
    pending = {**DRAW_WITH, "prompt": {"pending": "draw#1"}}
    return {
        "kind": "fx-graph-v1",
        "workflow": {"id": "case", "title": "Case"},
        "types": _types(),
        "instances": [
            _instance(
                "count#1",
                "./nodes/cases.py#lines",
                "nodes/cases.py#lines@1",
                {"text": {"file": FILE}},
                LOCAL_STEP,
            ),
            _instance(
                "draw#1",
                "fx/image.generate@1",
                "fx/image.generate@1.1",
                dict(DRAW_WITH),
                BUILTIN_STEP,
                json.loads(json.dumps(DRAW_ROUTES)),
            ),
            _instance("echo#1", "./nodes/n.py#echo", SOURCE_TYPE, {"name": "ada"}, echo),
            _instance(
                "again#1",
                "fx/image.generate@1",
                "fx/image.generate@1.1",
                pending,
                None,
                json.loads(json.dumps(DRAW_ROUTES)),
            ),
        ],
        "pending": [],
        "estimate": {"low_usd": 0.01, "high_usd": 0.01, "ceiling_usd": None},
        "problems": [],
    }


def _run_check(project: Path, graph: dict[str, Any], *extra: str) -> int:
    path = project / "graph.json"
    path.write_text(json.dumps(graph), "utf-8")
    return digest.main(["--check-graph", str(path), "--project", str(project), *extra])


def test_check_graph_accepts_a_consistent_graph(
    project: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    assert _run_check(project, _graph()) == 0
    out = capsys.readouterr().out
    assert "3 step identities, 2 route fingerprints, 1 node sources, 0 lock entries" in out
    assert "nodes/cases.py#lines@1 is unlocked" in out


def test_check_graph_without_a_types_map_notes_it(
    project: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    graph = _graph()
    del graph["types"]
    assert _run_check(project, graph) == 0
    assert "no types map" in capsys.readouterr().out


# Route tables, identity.md section 7: fx.yaml's route_tables in order, then each --routes file;
# a later entry for the same capability and route replaces an earlier one.


def _with_fingerprint(graph: dict[str, Any], fingerprint: str) -> dict[str, Any]:
    for instance in graph["instances"]:
        for bound in instance["routes"].values():
            bound["fingerprint"] = fingerprint
    graph["instances"][1]["identity"] = digest.step_identity(
        "fx/image.generate@1.1", DRAW_WITH, {"image.generate": fingerprint}, [1]
    )
    return graph


def test_check_graph_keeps_project_tables_when_routes_are_given(project: Path) -> None:
    other = project / "other.yaml"
    other.write_text(OTHER_ROUTES_YAML, "utf-8")
    assert _run_check(project, _graph(), "--routes", str(other)) == 0
    # fx.yaml's table still holds img-a@acme, so a wrong fingerprint is caught.
    assert _run_check(project, _with_fingerprint(_graph(), "1" * 64), "--routes", str(other)) == 1


def test_check_graph_lets_a_later_table_replace_an_entry(project: Path) -> None:
    other = project / "other.yaml"
    other.write_text(
        "fx: routes/v1\nroutes:\n"
        "  - { capability: image.generate, route: img-a@acme, price: {}, contract: { v: 2 } }\n",
        "utf-8",
    )
    replaced = digest.route_fingerprint("image.generate", "img-a@acme", {"v": 2})
    assert _run_check(project, _graph(), "--routes", str(other)) == 1
    assert _run_check(project, _with_fingerprint(_graph(), replaced), "--routes", str(other)) == 0
    assert _run_check(project, _with_fingerprint(_graph(), replaced)) == 1


def test_check_graph_reads_route_tables_in_listed_order(project: Path) -> None:
    (project / "late.yaml").write_text(
        "fx: routes/v1\nroutes:\n"
        "  - { capability: image.generate, route: img-a@acme, price: {}, contract: { v: 3 } }\n",
        "utf-8",
    )
    replaced = digest.route_fingerprint("image.generate", "img-a@acme", {"v": 3})
    fx_yaml = project / "fx.yaml"
    fx_yaml.write_text("fx: project/v1\nroute_tables: [routes.yaml, late.yaml]\n", "utf-8")
    assert _run_check(project, _with_fingerprint(_graph(), replaced)) == 0
    fx_yaml.write_text("fx: project/v1\nroute_tables: [late.yaml, routes.yaml]\n", "utf-8")
    assert _run_check(project, _graph()) == 0


def test_check_graph_refuses_a_duplicate_within_one_table(
    project: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    other = project / "other.yaml"
    other.write_text(
        "fx: routes/v1\nroutes:\n"
        "  - { capability: image.generate, route: img-a@acme, price: {} }\n"
        "  - { capability: image.generate, route: img-a@acme, price: {} }\n",
        "utf-8",
    )
    assert _run_check(project, _graph(), "--routes", str(other)) == 1
    assert "already declared" in capsys.readouterr().err


def test_check_graph_route_in_no_table(project: Path, capsys: pytest.CaptureFixture[str]) -> None:
    (project / "fx.yaml").write_text("fx: project/v1\n", "utf-8")
    # Without --routes the built-in table, unknown here, may hold it: a note.
    assert _run_check(project, _graph()) == 0
    assert "built-in default table" in capsys.readouterr().out
    # With --routes the catalog is fully known: a bound route in no table is a mismatch.
    other = project / "other.yaml"
    other.write_text(OTHER_ROUTES_YAML, "utf-8")
    assert _run_check(project, _graph(), "--routes", str(other)) == 1


def _retype_echo(graph: dict[str, Any], identity: str) -> None:
    """Give `echo` another type identity everywhere it is printed, consistently."""
    graph["types"]["./nodes/n.py#echo"]["identity"] = identity
    graph["instances"][2]["type"] = identity
    graph["instances"][2]["identity"] = digest.step_identity(identity, {"name": "ada"}, {}, [1])


@pytest.mark.parametrize(
    "tamper",
    [
        lambda g: g["instances"][0].update(identity="0" * 64),
        lambda g: g["instances"][1]["with"].update(background="opaque"),
        lambda g: g["instances"][1].update(take=[2]),
        lambda g: g["instances"][1].update(take=1),
        lambda g: g["instances"][1].update(take=[]),
        lambda g: g["instances"][1].update(take=[0]),
        lambda g: g["instances"][1].update(take=[True]),
        lambda g: g["instances"][1].update(take=[1.0]),
        lambda g: g["instances"][1].update(takes=g["instances"][1].pop("take")),
        lambda g: g["instances"][1]["routes"]["image.generate"].update(fingerprint="1" * 64),
        lambda g: g["instances"][1].update(type="fx/image.generate@2.1"),
        lambda g: g["instances"][0].update(type="nodes/other.py#lines@1"),
        lambda g: g["instances"][3].update(identity=BUILTIN_STEP),
        lambda g: g["instances"][3]["with"].update(prompt="known"),
        lambda g: g.update(kind="fx-graph-v2"),
        lambda g: g["types"]["./nodes/n.py#echo"].update(
            identity="nodes/n.py#echo@source:" + "4" * 64
        ),
        # identity.md section 6: the identity names the export; the form without it is retired.
        lambda g: _retype_echo(g, "source:" + NODE_SOURCE),
        lambda g: _retype_echo(g, "nodes/n.py#shout@source:" + NODE_SOURCE),
        lambda g: _retype_echo(g, "nodes/m.py#echo@source:" + NODE_SOURCE),
        lambda g: g["types"]["./nodes/n.py#echo"]["source"]["files"].update(
            {"nodes/n.py": "5" * 64}
        ),
        lambda g: g["types"]["./nodes/n.py#echo"]["source"]["resources"].update(
            {"prompts/gone.md": "6" * 64}
        ),
        lambda g: g["types"]["./nodes/n.py#echo"].pop("source"),
        lambda g: g["types"]["fx/image.generate@1"].update(
            source={"files": {"a.py": "7" * 64}, "resources": {}}
        ),
        lambda g: g["types"].pop("./nodes/cases.py#lines"),
        lambda g: g["types"]["./nodes/cases.py#lines"].update(identity="nodes/cases.py#lines@2"),
        lambda g: g["types"]["./nodes/cases.py#lines"]["source"]["files"].update(
            {"../outside.py": "8" * 64}
        ),
    ],
    ids=[
        "identity",
        "with",
        "take",
        "take-integer",
        "take-empty",
        "take-zero",
        "take-bool",
        "take-float",
        "takes-key",
        "fingerprint",
        "builtin-type",
        "local-type",
        "identity-while-pending",
        "null-identity-without-pending",
        "kind",
        "source-identity",
        "source-identity-without-export",
        "source-identity-of-another-export",
        "source-identity-of-another-module",
        "source-file-digest",
        "missing-resource",
        "project-type-without-source",
        "builtin-with-source",
        "uses-not-in-types",
        "types-identity-differs-from-instance",
        "label-outside-the-project",
    ],
)
def test_check_graph_catches(project: Path, tamper: Any) -> None:
    graph = _graph()
    tamper(graph)
    assert _run_check(project, graph) == 1


def test_check_graph_catches_a_changed_source(project: Path) -> None:
    (project / "prompts" / "r.md").write_bytes(b"Hello!\n")
    assert _run_check(project, _graph()) == 1


def test_check_graph_notes_a_label_outside_the_project(
    project: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    # A closure file found under a declared source package is not re-hashed, but its printed
    # digest still enters digest(source).
    graph = _graph()
    source = graph["types"]["./nodes/n.py#echo"]["source"]
    source["files"]["acme_lib/util.py"] = "9" * 64
    identity = "nodes/n.py#echo@source:" + digest.node_source_digest(
        source["files"], source["resources"]
    )
    _retype_echo(graph, identity)
    assert _run_check(project, graph) == 0
    assert "acme_lib/util.py" in capsys.readouterr().out


def test_check_graph_compares_the_lock(project: Path, capsys: pytest.CaptureFixture[str]) -> None:
    source = digest.node_source_digest({"nodes/cases.py": digest.file_digest(CASES_PY)}, {})
    lock = project / "fx.lock"
    lock.write_text(f"fx: lock/v1\nnodes:\n  nodes/cases.py#lines@1: {source}\n", "utf-8")
    assert _run_check(project, _graph()) == 0
    assert "1 lock entries" in capsys.readouterr().out
    lock.write_text(f"fx: lock/v1\nnodes:\n  nodes/cases.py#lines@1: '{'2' * 64}'\n", "utf-8")
    assert _run_check(project, _graph()) == 1
    # With the drift reported as a problem, the graph is consistent: a note.
    graph = _graph()
    graph["problems"] = [{"where": "count", "message": "the source no longer matches fx.lock"}]
    assert _run_check(project, graph) == 0
    lock.write_text("fx: lock/v1\nnodes: {}\n", "utf-8")
    assert _run_check(project, _graph()) == 0
    assert "unlocked" in capsys.readouterr().out


def test_check_graph_recomputes_the_plan_digest(project: Path) -> None:
    workflow_text = (
        "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n"
        "  draw: { uses: fx/image.generate@1 }\n"
        "  tint: { uses: fx/image.edit@1, with: { prompt: blue } }\n"
    )
    (project / "workflows").mkdir()
    (project / "workflows" / "case.yaml").write_text(workflow_text, "utf-8")
    (project / "workflows" / "case.takes.yaml").write_text("draw: { take: 2 }\n", "utf-8")
    graph = _graph()
    # img-a@acme serves two capabilities, so the plan binds it under two fingerprints.
    tint_with = {"prompt": "blue", "background": "auto", "vars": {}}
    tint_routes = {"image.edit": {"route": "img-a@acme", "fingerprint": EDIT_ROUTE}}
    tint = digest.step_identity("fx/image.edit@1.1", tint_with, {"image.edit": EDIT_ROUTE}, [1])
    graph["instances"].append(
        _instance("tint#1", "fx/image.edit@1", "fx/image.edit@1.1", tint_with, tint, tint_routes)
    )
    graph["types"]["fx/image.edit@1"] = {"identity": "fx/image.edit@1.1"}
    graph["workflow"]["file"] = "workflows/case.yaml"
    graph["inputs"] = {"brief": {"file": FILE}}
    graph["steps"] = {"draw": {"uses": "fx/image.generate@1"}, "tint": {"uses": "fx/image.edit@1"}}
    expected = digest.digest(
        {
            "kind": "fx-plan-v1",
            "workflows": {
                "workflows/case.yaml": {
                    "fx": "workflow/v1",
                    "id": "case",
                    "title": "Case",
                    "steps": {
                        "draw": {"uses": "fx/image.generate@1"},
                        "tint": {"uses": "fx/image.edit@1", "with": {"prompt": "blue"}},
                    },
                }
            },
            "inputs": {"brief": {"file": FILE}},
            "takes": {"draw": 2},
            "types": {
                "./nodes/cases.py#lines": "nodes/cases.py#lines@1",
                "fx/image.generate@1": "fx/image.generate@1.1",
                "./nodes/n.py#echo": SOURCE_TYPE,
                "fx/image.edit@1": "fx/image.edit@1.1",
            },
            "routes": sorted([ROUTE, EDIT_ROUTE]),
        }
    )
    graph["plan"] = expected
    assert _run_check(project, graph) == 0
    graph["plan"] = "3" * 64
    assert _run_check(project, graph) == 1


def test_canon_command(tmp_path: Path, capsysbinary: pytest.CaptureFixture[bytes]) -> None:
    path = tmp_path / "v.json"
    path.write_text('{"b": [1.0, -0.0, 1e21], "a": "\\u00e9"}', "utf-8")
    assert digest.main(["--canon", str(path)]) == 0
    assert capsysbinary.readouterr().out == '{"a":"\u00e9","b":[1,0,1e+21]}\n'.encode()
    path.write_text('{"a": NaN}', "utf-8")
    assert digest.main(["--canon", str(path)]) == 1


# --- tools/check_spec.py: expected graphs (h) --------------------------------------------------


def _load_check_spec() -> Any:
    path = REPO / "tools" / "check_spec.py"
    spec = importlib.util.spec_from_file_location("fx_tools_check_spec", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_check_spec_checks_expected_graphs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    check_spec = _load_check_spec()
    case = tmp_path / "conformance" / "linear"
    (case / "in").mkdir(parents=True)
    (case / "expected").mkdir()
    (case / "in" / "routes.yaml").write_text(ROUTES_YAML, "utf-8")
    (case / "case.yaml").write_text(
        "steps:\n- argv: [expand, case, --routes, routes.yaml]\n  save: expand.json\n", "utf-8"
    )
    graph = _graph()
    del graph["instances"][2]  # its source lives in no project here
    del graph["types"]["./nodes/n.py#echo"]
    (case / "expected" / "expand.json").write_text(json.dumps(graph), "utf-8")
    monkeypatch.setattr(check_spec, "CONFORMANCE", tmp_path / "conformance")

    gate = check_spec.Gate()
    check_spec.check_expected_graphs(gate)
    assert gate.failures == 0, capsys.readouterr().out

    graph["instances"][0]["identity"] = "0" * 64
    (case / "expected" / "expand.json").write_text(json.dumps(graph), "utf-8")
    gate = check_spec.Gate()
    check_spec.check_expected_graphs(gate)
    assert gate.failures == 1
    assert "count#1: step identity differs" in capsys.readouterr().out


def test_check_spec_names_check_covers_contracts_not_prose(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    check_spec = _load_check_spec()
    name = check_spec.FORBIDDEN_NAMES[0]
    spec = tmp_path / "spec"
    (spec / "schemas").mkdir(parents=True)
    (spec / "vectors").mkdir()
    (spec / "identity.md").write_bytes(b"## Changes from " + name + b"\n")
    roots = (spec / "schemas", spec / "vectors")
    monkeypatch.setattr(check_spec, "NAME_ROOTS", roots)
    gate = check_spec.Gate()
    check_spec.check_names(gate)
    assert gate.failures == 0
    (spec / "vectors" / "case.json").write_bytes(b'{"engine": "' + name + b'"}')
    gate = check_spec.Gate()
    check_spec.check_names(gate)
    assert gate.failures == 1


# --- tools/check_spec.py: the planner's feature vocabulary (k) ---------------------------------


@pytest.fixture
def vocabulary(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Any, Path, Path]:
    """check_spec with its spec folder and feature vocabulary copied into `tmp_path`."""
    check_spec = _load_check_spec()
    spec = tmp_path / "spec"
    spec.mkdir()
    for name in ("capabilities.md", "providers.md"):
        (spec / name).write_bytes((REPO / "spec" / name).read_bytes())
    features = tmp_path / "features.json"
    features.write_bytes(check_spec.FEATURES.read_bytes())
    monkeypatch.setattr(check_spec, "SPEC", spec)
    monkeypatch.setattr(check_spec, "FEATURES", features)
    return check_spec, spec, features


def _check_vocabulary(check_spec: Any) -> int:
    gate = check_spec.Gate()
    check_spec.check_feature_vocabulary(gate)
    return gate.failures


def test_check_spec_feature_vocabulary_agrees_with_the_spec(
    vocabulary: tuple[Any, Path, Path], capsys: pytest.CaptureFixture[str]
) -> None:
    check_spec, _, _ = vocabulary
    assert _check_vocabulary(check_spec) == 0, capsys.readouterr().out
    assert "11 capabilities and 4 renamed features agree" in capsys.readouterr().out


def _drop_pbr(table: dict[str, Any]) -> None:
    table["capabilities"]["mesh.generate"].remove("pbr")


def _twice(table: dict[str, Any]) -> None:
    table["capabilities"]["sound.generate"].append("exact_duration")


def _unknown_capability(table: dict[str, Any]) -> None:
    table["capabilities"]["vision.review"] = []


def _lacking_capability(table: dict[str, Any]) -> None:
    del table["capabilities"]["background.remove"]


def _renamed_to_nothing(table: dict[str, Any]) -> None:
    table["renamed"]["masked_edit"] = "masks"


def _renamed_but_still_a_feature(table: dict[str, Any]) -> None:
    table["renamed"]["alpha"] = "alpha"


def _renamed_but_never_dropped(table: dict[str, Any]) -> None:
    table["renamed"]["old_alpha"] = "alpha"


def _kind(table: dict[str, Any]) -> None:
    table["kind"] = "fx-features-v2"


@pytest.mark.parametrize(
    ("tamper", "expected"),
    [
        (_drop_pbr, "mesh.generate: not its feature table (extra [], missing ['pbr'])"),
        (_twice, "sound.generate: a feature is listed twice"),
        (_unknown_capability, "vision.review: spec/capabilities.md has no section for it"),
        (
            _lacking_capability,
            "background.remove: spec/capabilities.md defines it, the vocabulary lacks it",
        ),
        (_renamed_to_nothing, "renamed masked_edit: masks is no feature in spec/capabilities.md"),
        (_renamed_but_still_a_feature, "renamed alpha: it is still a feature"),
        (
            _renamed_but_never_dropped,
            "renamed old_alpha: spec/providers.md section 11 does not name it",
        ),
        (_kind, "kind is 'fx-features-v2', not 'fx-features-v1'"),
    ],
)
def test_check_spec_feature_vocabulary_catches(
    vocabulary: tuple[Any, Path, Path],
    capsys: pytest.CaptureFixture[str],
    tamper: Any,
    expected: str,
) -> None:
    check_spec, _, features = vocabulary
    table = json.loads(features.read_text("utf-8"))
    tamper(table)
    features.write_text(json.dumps(table), "utf-8")
    assert _check_vocabulary(check_spec) == 1
    assert expected in capsys.readouterr().out


def test_check_spec_feature_vocabulary_follows_the_spec_text(
    vocabulary: tuple[Any, Path, Path], capsys: pytest.CaptureFixture[str]
) -> None:
    check_spec, spec, _ = vocabulary
    # A feature row added to the spec, and a rename that section 11 no longer names.
    capabilities = spec / "capabilities.md"
    text = capabilities.read_text("utf-8")
    row = "| `exact_duration` |"
    assert text.count(row) == 1
    capabilities.write_text(
        text.replace(row, "| `loop_seamless` | the sound loops |\n" + row), "utf-8"
    )
    providers = spec / "providers.md"
    providers.write_text(
        providers.read_text("utf-8").replace("`data_url_reference_input`", "an alias"), "utf-8"
    )
    assert _check_vocabulary(check_spec) == 1
    out = capsys.readouterr().out
    assert "sound.generate: not its feature table (extra [], missing ['loop_seamless'])" in out
    assert "renamed data_url_reference_input: spec/providers.md section 11 does not name it" in out


@pytest.mark.parametrize(
    ("content", "expected"),
    [
        (None, "missing"),
        ('{"kind": "fx-features-v1"}', "not {kind, capabilities, renamed}"),
        (
            '{"kind": "fx-features-v1", "capabilities": {"a.b": "x"}, "renamed": {}}',
            "capabilities is not {capability: [feature, ...]}",
        ),
        (
            '{"kind": "fx-features-v1", "capabilities": {}, "renamed": {"a": 1}}',
            "renamed is not {name: feature}",
        ),
    ],
)
def test_check_spec_feature_vocabulary_refuses_a_malformed_table(
    vocabulary: tuple[Any, Path, Path],
    capsys: pytest.CaptureFixture[str],
    content: str | None,
    expected: str,
) -> None:
    check_spec, _, features = vocabulary
    if content is None:
        features.unlink()
    else:
        features.write_text(content, "utf-8")
    assert _check_vocabulary(check_spec) == 1
    assert expected in capsys.readouterr().out
