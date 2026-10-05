"""An independent checker for FX identities.

This script implements the formulas in spec/identity.md from the spec text alone. It shares
no code with the engine and imports nothing from this repository, so a digest that both agree
on was computed twice, two different ways. Canonical JSON (RFC 8785) is implemented here too,
rather than taken from a library.

Usage:
    python tools/digest.py --check-examples [--examples <examples.json>]
    python tools/digest.py --check-graph <graph.json> --project <dir> [--routes <routes.yaml> ...]
    python tools/digest.py --canon <file.json | ->

--check-examples also checks markers.json and json_output.json when they sit beside the
examples file. --check-graph builds the route catalog of identity.md section 7 from fx.yaml's
route_tables and then each --routes file; the built-in default table is the engine's and is
not known here.

Only the standard library is needed, plus PyYAML when a YAML file (a route table, fx.yaml,
fx.lock) has to be read.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
from collections.abc import Iterable, Iterator
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_EXAMPLES = REPO_ROOT / "spec" / "vectors" / "identity" / "examples.json"

# Every integer of magnitude up to 2^53 - 1 is a binary64 number exactly and is its own canonical
# form, so a reader keeps it as a Python int. A larger integer literal that is accepted
# (identity.md section 1) is read as the binary64 number it names, a Python float.
_EXACT_INT = 2**53 - 1
# JCS writes every integral number of 1e21 and above with an exponent, so no integer literal of
# more than 21 digits is ever canonical.
_MAX_INTEGER_DIGITS = 21
_HEX64 = re.compile(r"[0-9a-f]{64}\Z")
_SURROGATE = re.compile("[\ud800-\udfff]")


class RefusedInput(ValueError):
    """A value or document outside the I-JSON domain (identity.md section 1) or the YAML subset.

    `code` names the rule, using the reason names of spec/vectors/jcs (non_finite_number,
    integer_out_of_range, number_overflow, lone_surrogate, duplicate_key, invalid_json, ...),
    reserved_marker for an authored value that holds a runtime marker (identity.md section 3),
    and for YAML ambiguous_scalar, line_break, byte_order_mark, directive, alias, anchor, tag,
    merge_key, complex_key, multiple_documents and invalid_yaml.
    """

    def __init__(self, message: str, code: str = "invalid") -> None:
        super().__init__(message)
        self.code = code


# ---------------------------------------------------------------------------------------------
# Values: the I-JSON domain (identity.md section 1)
# ---------------------------------------------------------------------------------------------


def check_value(value: Any, where: str = "$") -> None:
    """Refuse a Python value that is not an FX value. Never coerces."""
    if value is None or isinstance(value, bool):
        return
    if isinstance(value, int):
        # An integer is a value when it is a binary64 number exactly and its digits are that
        # number's canonical form, so canon writes it as it is and reads it back unchanged.
        if abs(value) <= _EXACT_INT:
            return
        if abs(value) >= 10**_MAX_INTEGER_DIGITS:
            shown = str(value) if abs(value) < 10**40 else f"of {value.bit_length()} bits"
            raise RefusedInput(
                f"{where}: integer {shown} would be rounded: no integer of more than "
                f"{_MAX_INTEGER_DIGITS} digits is a canonical number",
                "integer_out_of_range",
            )
        canonical = _canon_number(float(value))
        if canonical != str(value):
            raise RefusedInput(
                f"{where}: integer {value} would be rounded: as a number it is {canonical}",
                "integer_out_of_range",
            )
        return
    if isinstance(value, float):
        if math.isnan(value) or math.isinf(value):
            raise RefusedInput(f"{where}: {value!r} is not a JSON number", "non_finite_number")
        return
    if isinstance(value, str):
        _check_text(value, where)
        return
    if isinstance(value, list | tuple):
        for index, item in enumerate(value):
            check_value(item, f"{where}[{index}]")
        return
    if isinstance(value, dict):
        for key, item in value.items():
            if not isinstance(key, str):
                raise RefusedInput(f"{where}: object key {key!r} is not a string", "non_string_key")
            _check_text(key, f"{where} key")
            check_value(item, f"{where}.{key}")
        return
    raise RefusedInput(f"{where}: {type(value).__name__} is not a JSON value", "not_a_value")


def _check_text(text: str, where: str) -> None:
    match = _SURROGATE.search(text)
    if match:
        raise RefusedInput(f"{where}: lone surrogate U+{ord(match.group()):04X}", "lone_surrogate")


def _refuse_constant(name: str) -> Any:
    raise RefusedInput(f"{name} is not a JSON number", "non_finite_number")


def _parse_int(text: str, where: str = "") -> int | float:
    """Read an integer literal (a sign and digits) as identity.md section 1 says.

    The literal names a binary64 number. It is refused when reading it would round it: its
    digits must be the canonical form (section 2) of the number they read as, apart from the
    sign of zero, so `-0` is 0, 9007199254740992 is read and 9007199254740993 is refused.
    """
    prefix = f"{where}: " if where else ""
    digits = text.lstrip("-+")
    # Refusing a long literal by its length also keeps int() away from thousands of digits.
    if len(digits) > _MAX_INTEGER_DIGITS:
        shown = text if len(text) <= 40 else f"{text[:20]}... ({len(text)} characters)"
        raise RefusedInput(
            f"{prefix}integer {shown} would be rounded: no integer literal of more than "
            f"{_MAX_INTEGER_DIGITS} digits is a canonical number",
            "integer_out_of_range",
        )
    number = int(text)
    value = float(number)
    canonical = _canon_number(value)
    if canonical.lstrip("-") != digits:
        raise RefusedInput(
            f"{prefix}integer {text} would be rounded: it reads as the number {canonical}",
            "integer_out_of_range",
        )
    return number if abs(number) <= _EXACT_INT else value


def _parse_float(text: str) -> float:
    value = float(text)
    if math.isinf(value):
        raise RefusedInput(f"number {text} overflows to infinity", "number_overflow")
    return value


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        _check_text(key, "object key")
        if key in result:
            raise RefusedInput(f"duplicate object key {key!r}", "duplicate_key")
        result[key] = value
    return result


def parse_ijson(text: str | bytes, *, authored: bool = False) -> Any:
    """Parse JSON text, refusing anything outside the I-JSON domain.

    Bytes must be strict UTF-8. A byte-order mark is not JSON and is refused here; readers of
    files strip one (see read_json_file). `authored` marks a value someone wrote (an inputs
    file, a JSON file a workflow reads): it may not hold a runtime marker (refuse_markers).
    """
    if isinstance(text, bytes):
        try:
            text = text.decode("utf-8")
        except UnicodeDecodeError as error:
            raise RefusedInput(f"not UTF-8: {error}", "invalid_utf8") from None
    try:
        value = json.loads(
            text,
            parse_constant=_refuse_constant,
            parse_int=_parse_int,
            parse_float=_parse_float,
            object_pairs_hook=_unique_object,
        )
    except json.JSONDecodeError as error:
        raise RefusedInput(f"not JSON: {error}", "invalid_json") from None
    check_value(value)
    if authored:
        refuse_markers(value)
    return value


_BOM = "\ufeff"


def read_text_file(path: Path) -> str:
    """Decode a file as identity.md section 5 says: strict UTF-8, one leading BOM removed."""
    data = path.read_bytes()
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise RefusedInput(f"{path}: not UTF-8: {error}", "invalid_utf8") from None
    return text.removeprefix(_BOM)


def read_json_file(path: Path, *, authored: bool = False) -> Any:
    try:
        return parse_ijson(read_text_file(path), authored=authored)
    except RefusedInput as error:
        raise RefusedInput(f"{path}: {error}", error.code) from None


# ---------------------------------------------------------------------------------------------
# Reserved plain markers (identity.md section 3)
# ---------------------------------------------------------------------------------------------


def reserved_marker(value: Any) -> str | None:
    """The marker an object would be mistaken for in the plain projection, or None.

    An object whose only member is one of these is a runtime marker, never user data:
    `file` with 64 lowercase hex, `missing` with true, `failed` with a string, `collection`
    with an array, and `pending` with any value.
    """
    if not isinstance(value, dict) or len(value) != 1:
        return None
    ((name, item),) = value.items()
    if name == "file" and isinstance(item, str) and _HEX64.match(item):
        return name
    if name == "missing" and item is True:
        return name
    if name == "failed" and isinstance(item, str):
        return name
    if name == "collection" and isinstance(item, list):
        return name
    if name == "pending":
        return name
    return None


def refuse_markers(value: Any, where: str = "$") -> None:
    """Refuse an authored value that holds a runtime marker anywhere (identity.md section 3)."""
    marker = reserved_marker(value)
    if marker is not None:
        raise RefusedInput(
            f"{where}: an object whose only member is {marker!r} is a reserved runtime marker",
            "reserved_marker",
        )
    if isinstance(value, dict):
        for key, item in value.items():
            refuse_markers(item, f"{where}.{key}")
    elif isinstance(value, list):
        for index, item in enumerate(value):
            refuse_markers(item, f"{where}[{index}]")


# ---------------------------------------------------------------------------------------------
# Canonical JSON: RFC 8785 (identity.md section 2)
# ---------------------------------------------------------------------------------------------

_SHORT_ESCAPES = {
    '"': '\\"',
    "\\": "\\\\",
    "\b": "\\b",
    "\f": "\\f",
    "\n": "\\n",
    "\r": "\\r",
    "\t": "\\t",
}


def _canon_string(text: str) -> str:
    out = ['"']
    for char in text:
        escaped = _SHORT_ESCAPES.get(char)
        if escaped is not None:
            out.append(escaped)
        elif ord(char) < 0x20:
            out.append(f"\\u{ord(char):04x}")
        else:
            out.append(char)
    out.append('"')
    return "".join(out)


def _canon_number(value: int | float) -> str:
    """Serialize a number as ECMAScript's Number.prototype.toString does."""
    if isinstance(value, int):
        # check_value admits only an int whose digits are its canonical form.
        return str(value)
    if value == 0:
        return "0"  # covers -0.0
    if value < 0:
        return "-" + _canon_number(-value)
    # repr() gives the shortest digit string that round-trips, as ECMAScript requires.
    # Recover it as value = digits * 10^exponent, then lay it out by ECMAScript's rules.
    text = repr(value)
    mantissa, _, exp_text = text.partition("e")
    whole, _, fraction = mantissa.partition(".")
    digits = (whole + fraction).lstrip("0")
    exponent = (int(exp_text) if exp_text else 0) - len(fraction)
    stripped = digits.rstrip("0")
    exponent += len(digits) - len(stripped)
    digits = stripped
    k = len(digits)
    n = exponent + k  # value = 0.<digits> * 10^n
    if k <= n <= 21:
        return digits + "0" * (n - k)
    if 0 < n <= 21:
        return digits[:n] + "." + digits[n:]
    if -6 < n <= 0:
        return "0." + "0" * (-n) + digits
    sign = "+" if n - 1 >= 0 else "-"
    head = digits[0] if k == 1 else digits[0] + "." + digits[1:]
    return f"{head}e{sign}{abs(n - 1)}"


def _utf16_key(text: str) -> bytes:
    # Big-endian UTF-16 bytes compare exactly as the UTF-16 code units do.
    return text.encode("utf-16-be")


def _canon(value: Any) -> str:
    if value is None:
        return "null"
    if value is True:
        return "true"
    if value is False:
        return "false"
    if isinstance(value, int | float):
        return _canon_number(value)
    if isinstance(value, str):
        return _canon_string(value)
    if isinstance(value, list | tuple):
        return "[" + ",".join(_canon(item) for item in value) + "]"
    if isinstance(value, dict):
        keys = sorted(value, key=_utf16_key)
        return "{" + ",".join(_canon_string(k) + ":" + _canon(value[k]) for k in keys) + "}"
    raise RefusedInput(f"{type(value).__name__} is not a JSON value", "not_a_value")


def canon(value: Any) -> bytes:
    """canon(v): the RFC 8785 canonical form, as UTF-8 bytes."""
    check_value(value)
    return _canon(value).encode("utf-8")


def digest(value: Any) -> str:
    """digest(v): SHA-256 of canon(v), 64 lowercase hex characters."""
    return hashlib.sha256(canon(value)).hexdigest()


def file_digest(data: bytes) -> str:
    """file_digest(f): SHA-256 of the raw bytes, never normalized."""
    return hashlib.sha256(data).hexdigest()


# ---------------------------------------------------------------------------------------------
# Writing JSON (identity.md section 5)
# ---------------------------------------------------------------------------------------------


def write_json(value: Any) -> bytes:
    """The bytes the engine writes for a JSON output: the canonical form spread over lines.

    Keys in canonical order, one space of indentation per level, `": "` after a key, `,` at
    the end of a line, strings and numbers exactly as in canon, `{}` and `[]` when empty, and
    no trailing newline.
    """
    check_value(value)
    return _write(value, 0).encode("utf-8")


def _write(value: Any, level: int) -> str:
    inner = " " * (level + 1)
    if isinstance(value, dict):
        if not value:
            return "{}"
        members = [
            inner + _canon_string(key) + ": " + _write(value[key], level + 1)
            for key in sorted(value, key=_utf16_key)
        ]
        return "{\n" + ",\n".join(members) + "\n" + " " * level + "}"
    if isinstance(value, list | tuple):
        if not value:
            return "[]"
        items = [inner + _write(item, level + 1) for item in value]
        return "[\n" + ",\n".join(items) + "\n" + " " * level + "]"
    return _canon(value)


# ---------------------------------------------------------------------------------------------
# Formulas (identity.md sections 6 to 10)
# ---------------------------------------------------------------------------------------------


def split_route(route_id: str) -> tuple[str, str]:
    """A route is <model>@<provider>, split at the last @."""
    model, at, provider = route_id.rpartition("@")
    if not at or not model or not provider:
        raise RefusedInput(f"route {route_id!r} is not <model>@<provider>", "invalid_route")
    return model, provider


def route_object(capability: str, route_id: str, contract: Any = None) -> dict[str, Any]:
    model, provider = split_route(route_id)
    return {
        "kind": "fx-route-v1",
        "capability": capability,
        "model": model,
        "provider": provider,
        "contract": {} if contract is None else contract,
    }


def route_fingerprint(capability: str, route_id: str, contract: Any = None) -> str:
    return digest(route_object(capability, route_id, contract))


def step_object(
    type_identity: str, with_values: dict[str, Any], routes: dict[str, str], take: list[int]
) -> dict[str, Any]:
    return {
        "kind": "fx-step-v1",
        "type": type_identity,
        "with": with_values,
        "routes": routes,
        "take": list(take),
    }


def step_identity(
    type_identity: str, with_values: dict[str, Any], routes: dict[str, str], take: list[int]
) -> str:
    return digest(step_object(type_identity, with_values, routes, take))


def call_object(capability: str, route_fp: str, request: Any, take: list[int]) -> dict[str, Any]:
    return {
        "kind": "fx-call-v1",
        "capability": capability,
        "route": route_fp,
        "request": request,
        "take": list(take),
    }


def call_key(capability: str, route_fp: str, request: Any, take: list[int]) -> str:
    return digest(call_object(capability, route_fp, request, take))


def node_source_object(files: dict[str, str], resources: dict[str, str]) -> dict[str, Any]:
    """files: {label: file digest}; resources: {path: file digest}."""
    return {"kind": "fx-node-source-v1", "files": dict(files), "resources": dict(resources)}


def node_source_digest(files: dict[str, str], resources: dict[str, str]) -> str:
    return digest(node_source_object(files, resources))


def plan_object(
    workflows: dict[str, Any],
    inputs: Any,
    takes: dict[str, Any],
    types: dict[str, str],
    routes: Iterable[str],
) -> dict[str, Any]:
    """routes: the fingerprint of every capability and route the plan binds, in any order."""
    return {
        "kind": "fx-plan-v1",
        "workflows": workflows,
        "inputs": inputs,
        "takes": takes,
        "types": types,
        "routes": sorted(set(routes)),
    }


def plan_digest(
    workflows: dict[str, Any],
    inputs: Any,
    takes: dict[str, Any],
    types: dict[str, str],
    routes: Iterable[str],
) -> str:
    return digest(plan_object(workflows, inputs, takes, types, routes))


# ---------------------------------------------------------------------------------------------
# Instance ids (identity.md section 11): names, not digests
# ---------------------------------------------------------------------------------------------


def repeat_key_text(key: Any) -> str:
    """The key text of a repeat item: a string itself, a boolean, a number in JCS form.

    A file's key text is its name without suffix; a file is not a JSON value, so a caller
    passes that name as a string.
    """
    if isinstance(key, bool):
        return "true" if key else "false"
    if isinstance(key, str):
        _check_text(key, "repeat key")
        return key
    if isinstance(key, int | float):
        check_value(key)
        return _canon_number(key)
    raise RefusedInput(f"repeat key {key!r} is not a string, boolean or number", "invalid_key")


def instance_id(path: list[dict[str, Any]], take: list[int]) -> str:
    """path: one {"step": name} per level, with "key" when that level is a repeated step."""
    parts: list[str] = []
    for segment in path:
        part = segment["step"]
        if "key" in segment:
            text = repeat_key_text(segment["key"])
            part += "['" + text.replace("\\", "\\\\").replace("'", "\\'") + "']"
        parts.append(part)
    return ".".join(parts) + "#" + ".".join(str(number) for number in take)


# ---------------------------------------------------------------------------------------------
# The strict YAML subset (spec/yaml.md), on top of PyYAML's parser
# ---------------------------------------------------------------------------------------------

_YAML_NULL = {"null", "~", ""}
_YAML_INT = re.compile(r"[-+]?(?:0|[1-9][0-9]*)\Z")
_YAML_FLOAT = re.compile(
    r"[-+]?(?:\.[0-9]+|(?P<whole>[0-9]+)\.[0-9]*|(?P<bare>[0-9]+))(?:[eE][-+]?[0-9]+)?\Z"
)
# The ambiguous forms of yaml.md, "Values". Each is a plain scalar that some YAML version or
# library reads as something other than a string.
_YAML_WORDS = re.compile(r"(?:yes|no|on|off|true|false|null)\Z", re.IGNORECASE)
_YAML_RADIX = re.compile(r"[-+]?0(?:[xX][0-9a-fA-F_]+|[oO][0-7_]+|[bB][01_]+)\Z")
_YAML_SEXAGESIMAL = re.compile(r"[-+]?[0-9]+(?::[0-9]+)+(?:\.[0-9]*)?\Z")
_YAML_SPECIAL = re.compile(r"(?:[-+]?\.inf|\.nan)\Z", re.IGNORECASE)
# YAML 1.1's timestamp type: a date with a two-digit month and day, or a date with one- or
# two-digit month and day followed by a time. Space is allowed before the zone, as in YAML 1.1's
# own example `2001-12-14 21:59:43.10 -5` and as PyYAML reads it. A short date alone
# (`2026-1-5`) is a string.
_YAML_TIMESTAMP = re.compile(
    r"(?:[0-9]{4}-[0-9]{2}-[0-9]{2}"
    r"|[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}"
    r"(?:[Tt]|[ \t]+)[0-9]{1,2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]*)?"
    r"(?:[ \t]*(?:Z|[-+][0-9]{1,2}(?::[0-9]{2})?))?)\Z"
)
# YAML 1.1 reads these as line breaks and YAML 1.2 as content, so they have no single meaning.
_YAML_LINE_BREAKS = re.compile("[\u0085\u2028\u2029]")


def _leading_zero(match: re.Match[str]) -> bool:
    whole = match.group("whole") or match.group("bare") or ""
    return len(whole) > 1 and whole.startswith("0")


def _number_like(text: str) -> bool:
    """Any numeric reading of any YAML version: decimal (leading zeros too), radix, sexagesimal."""
    return bool(_YAML_FLOAT.match(text) or _YAML_RADIX.match(text) or _YAML_SEXAGESIMAL.match(text))


def _ambiguity(text: str) -> str | None:
    """Why a plain scalar that is neither null, a boolean nor a number is refused, if it is."""
    if _YAML_WORDS.match(text):
        return "a word that YAML 1.1 or another letter case reads as a boolean or null"
    decimal = _YAML_FLOAT.match(text)
    if decimal and _leading_zero(decimal):
        return "a number with a leading zero"
    if _YAML_RADIX.match(text):
        return "a hexadecimal, octal or binary number"
    if "_" in text and _number_like(text.replace("_", "")):
        return "a number once its underscores are removed"
    if _YAML_SEXAGESIMAL.match(text):
        return "a sexagesimal number"
    if _YAML_SPECIAL.match(text):
        return "an infinity or NaN"
    if _YAML_TIMESTAMP.match(text):
        return "a date or timestamp"
    return None


def _plain_scalar(text: str, where: str) -> Any:
    if text in _YAML_NULL:
        return None
    if text == "true":
        return True
    if text == "false":
        return False
    if _YAML_INT.match(text):
        return _parse_int(text, where)
    decimal = _YAML_FLOAT.match(text)
    if decimal and not _leading_zero(decimal):
        value = float(text)
        if math.isinf(value):
            raise RefusedInput(f"{where}: number {text} overflows to infinity", "number_overflow")
        return value
    reason = _ambiguity(text)
    if reason is not None:
        raise RefusedInput(
            f"{where}: plain scalar {text!r} is ambiguous ({reason}); quote it", "ambiguous_scalar"
        )
    return text


def _mark(node_or_event: Any, label: str) -> str:
    mark = node_or_event.start_mark
    return f"{label}:{mark.line + 1}:{mark.column + 1}"


def _refuse_line_breaks(text: str, label: str) -> None:
    match = _YAML_LINE_BREAKS.search(text)
    if match is None:
        return
    before = text[: match.start()]
    line = len(re.findall(r"\r\n|\r|\n", before)) + 1
    column = len(before) - max(before.rfind("\n"), before.rfind("\r"))
    raise RefusedInput(
        f"{label}:{line}:{column}: U+{ord(match.group()):04X} is refused anywhere in a stream "
        "(YAML 1.1 reads it as a line break, YAML 1.2 as content)",
        "line_break",
    )


def load_yaml(text: str | bytes, label: str = "<yaml>") -> Any:
    """Load one document of the strict YAML subset (spec/yaml.md) as a JSON value.

    Every document FX reads in YAML is authored, so a runtime marker (identity.md section 3)
    is refused too.
    """
    import yaml  # PyYAML parses; every resolution rule below is FX's own.

    if isinstance(text, bytes):
        try:
            text = text.decode("utf-8")
        except UnicodeDecodeError as error:
            raise RefusedInput(f"{label}: not UTF-8: {error}", "invalid_utf8") from None
    text = text.removeprefix(_BOM)
    if text.startswith(_BOM):
        raise RefusedInput(f"{label}:1:1: a second byte-order mark", "byte_order_mark")
    _refuse_line_breaks(text, label)
    try:
        content = False
        for token in yaml.scan(text, Loader=yaml.SafeLoader):
            if not isinstance(
                token, yaml.StreamStartToken | yaml.StreamEndToken | yaml.DocumentEndToken
            ):
                content = True
            if isinstance(token, yaml.DirectiveToken):
                raise RefusedInput(
                    f"{_mark(token, label)}: directives (%{token.name}) are not supported",
                    "directive",
                )
            # An implicit key's token is empty; an explicit `? ` key's token spans the `?`.
            if isinstance(token, yaml.KeyToken) and token.end_mark.index > token.start_mark.index:
                raise RefusedInput(
                    f"{_mark(token, label)}: complex keys (? ) are not supported", "complex_key"
                )
        # A stream holding only document end markers (`...`) is an empty document. YAML 1.2
        # allows an end marker with no document before it; PyYAML does not parse one.
        if not content:
            return {}
        documents = 0
        for event in yaml.parse(text, Loader=yaml.SafeLoader):
            if isinstance(event, yaml.DocumentStartEvent):
                documents += 1
                if documents > 1:
                    raise RefusedInput(
                        f"{_mark(event, label)}: a second document", "multiple_documents"
                    )
            elif isinstance(event, yaml.AliasEvent):
                raise RefusedInput(f"{_mark(event, label)}: aliases are not supported", "alias")
            elif isinstance(event, yaml.NodeEvent):
                if event.anchor is not None:
                    raise RefusedInput(
                        f"{_mark(event, label)}: anchors are not supported", "anchor"
                    )
                if getattr(event, "tag", None) is not None:
                    raise RefusedInput(f"{_mark(event, label)}: tags are not supported", "tag")
        root = yaml.compose(text, Loader=yaml.SafeLoader)
    except yaml.YAMLError as error:
        raise RefusedInput(f"{label}: {error}", "invalid_yaml") from None
    # An empty stream, a stream of comments and an explicit `---` with no content are all {}.
    # A document with no content composes to an empty plain scalar; a written `~` or `null`
    # is not empty, and stays null.
    if root is None or (
        isinstance(root, yaml.ScalarNode) and root.style is None and root.value == ""
    ):
        return {}
    value = _yaml_node(root, label)
    try:
        check_value(value)
        refuse_markers(value)
    except RefusedInput as error:
        raise RefusedInput(f"{label}: {error}", error.code) from None
    return value


def _yaml_node(node: Any, label: str) -> Any:
    import yaml

    if isinstance(node, yaml.ScalarNode):
        if node.style is None:
            return _plain_scalar(node.value, _mark(node, label))
        return node.value
    if isinstance(node, yaml.SequenceNode):
        return [_yaml_node(item, label) for item in node.value]
    if isinstance(node, yaml.MappingNode):
        result: dict[str, Any] = {}
        for key_node, value_node in node.value:
            if not isinstance(key_node, yaml.ScalarNode):
                raise RefusedInput(
                    f"{_mark(key_node, label)}: a key must be a string", "complex_key"
                )
            key = key_node.value
            if key_node.style is None and key == "<<":
                raise RefusedInput(
                    f"{_mark(key_node, label)}: merge keys are not supported", "merge_key"
                )
            if key in result:
                raise RefusedInput(
                    f"{_mark(key_node, label)}: duplicate key {key!r}", "duplicate_key"
                )
            result[key] = _yaml_node(value_node, label)
        return result
    raise RefusedInput(f"{_mark(node, label)}: unsupported node", "invalid_yaml")


def read_yaml_file(path: Path) -> Any:
    return load_yaml(path.read_bytes(), str(path))


# ---------------------------------------------------------------------------------------------
# --check-examples
# ---------------------------------------------------------------------------------------------


def _example_failures(example: dict[str, Any]) -> Iterator[str]:
    name = example.get("name", "<unnamed>")
    want_digest = example.get("digest")
    if "instance_id" in example:
        got_id = instance_id(example["path"], example["take"])
        if got_id != example["instance_id"]:
            yield f"{name}: instance id {got_id!r}, expected {example['instance_id']!r}"
        return
    if "file_utf8" in example:
        data = example["file_utf8"].encode("utf-8")
        got = file_digest(data)
        if got != want_digest:
            yield f"{name}: file digest {got}, expected {want_digest}"
        if "text" in example:
            # The file's content read as text (identity.md section 5).
            decoded = data.decode("utf-8").removeprefix(_BOM)
            if decoded != example["text"]:
                yield f"{name}: decodes to {decoded!r}, expected {example['text']!r}"
        return
    if "object_source" in example:
        value = parse_ijson(example["object_source"])
    elif "object" in example:
        value = example["object"]
    else:
        yield f"{name}: no file_utf8, object_source or object to check"
        return
    got_canon = canon(value).decode("utf-8")
    if "canonical" in example and got_canon != example["canonical"]:
        yield (
            f"{name}: canonical form differs\n"
            f"  got:      {got_canon}\n"
            f"  expected: {example['canonical']}"
        )
    got = digest(value)
    if got != want_digest:
        yield f"{name}: digest {got}, expected {want_digest}"
    # Rebuild the hashed object through its formula, so the formula's shape is checked too.
    kind = value.get("kind") if isinstance(value, dict) else None
    rebuilt: str | None = None
    if kind == "fx-route-v1":
        route_id = f"{value['model']}@{value['provider']}"
        rebuilt = route_fingerprint(value["capability"], route_id, value.get("contract"))
    elif kind == "fx-step-v1":
        rebuilt = step_identity(value["type"], value["with"], value["routes"], value["take"])
    elif kind == "fx-call-v1":
        rebuilt = call_key(value["capability"], value["route"], value["request"], value["take"])
    elif kind == "fx-node-source-v1":
        rebuilt = node_source_digest(value["files"], value["resources"])
    elif kind == "fx-plan-v1":
        rebuilt = plan_digest(
            value["workflows"], value["inputs"], value["takes"], value["types"], value["routes"]
        )
    if rebuilt is not None and rebuilt != want_digest:
        yield f"{name}: the {kind} formula gives {rebuilt}, expected {want_digest}"


def check_examples(path: Path) -> list[str]:
    document = read_json_file(path)
    examples = document.get("examples") if isinstance(document, dict) else None
    if not isinstance(examples, list) or not examples:
        return [f"{path}: no examples"]
    failures: list[str] = []
    for example in examples:
        try:
            failures.extend(_example_failures(example))
        except (RefusedInput, KeyError, TypeError) as error:
            failures.append(f"{example.get('name', '<unnamed>')}: {error}")
    return failures


def _cases(path: Path, kind: str) -> list[dict[str, Any]]:
    document = read_json_file(path)
    if not isinstance(document, dict) or document.get("kind") != kind:
        raise RefusedInput(f"{path}: kind is not {kind!r}", "invalid_document")
    cases = document.get("cases")
    if not isinstance(cases, list) or not cases:
        raise RefusedInput(f"{path}: no cases", "invalid_document")
    return cases


def check_marker_vectors(path: Path) -> list[str]:
    """vectors/identity/markers.json: authored values that hold a reserved marker are refused."""
    failures: list[str] = []
    for case in _cases(path, "fx-marker-vectors-v1"):
        name = case.get("name", "<unnamed>")
        try:
            parse_ijson(case["json"])
        except RefusedInput as error:
            failures.append(f"{name}: not an I-JSON value: {error}")
            continue
        try:
            parse_ijson(case["json"], authored=True)
            refused = None
        except RefusedInput as error:
            refused = error
        if case["refuse"] and (refused is None or refused.code != "reserved_marker"):
            failures.append(f"{name}: should be refused as a reserved marker, got {refused}")
        elif not case["refuse"] and refused is not None:
            failures.append(f"{name}: refused, but it is ordinary data: {refused}")
    return failures


def check_json_output_vectors(path: Path) -> list[str]:
    """vectors/identity/json_output.json: the bytes a JSON output is written as."""
    failures: list[str] = []
    for case in _cases(path, "fx-json-output-vectors-v1"):
        name = case.get("name", "<unnamed>")
        try:
            got = write_json(parse_ijson(case["value_json"]))
        except RefusedInput as error:
            failures.append(f"{name}: {error}")
            continue
        want = case["bytes_utf8"].encode("utf-8")
        if got != want:
            failures.append(f"{name}: writes {got!r}, expected {want!r}")
    return failures


IDENTITY_VECTORS = {
    "markers.json": check_marker_vectors,
    "json_output.json": check_json_output_vectors,
}


# ---------------------------------------------------------------------------------------------
# --check-graph
# ---------------------------------------------------------------------------------------------


class RouteTable:
    """The route catalog of identity.md section 7, as far as this checker can know it.

    Tables are loaded in catalog order. A later table's entry for a capability and route
    replaces an earlier one, contract included; two entries for one capability and route within
    one table are refused. The built-in default table is the engine's, unknown here.
    """

    def __init__(self) -> None:
        self.entries: dict[tuple[str, str], tuple[str, dict[str, Any]]] = {}

    def load(self, path: Path) -> None:
        document = read_yaml_file(path)
        if not isinstance(document, dict) or document.get("fx") != "routes/v1":
            raise RefusedInput(f"{path}: not a route table (fx: routes/v1)", "invalid_document")
        routes = document.get("routes")
        if not isinstance(routes, list):
            raise RefusedInput(f"{path}: routes must be a list", "invalid_document")
        seen: dict[tuple[str, str], int] = {}
        for index, entry in enumerate(routes):
            where = f"{path}: routes[{index}]"
            if not isinstance(entry, dict):
                raise RefusedInput(f"{where}: not a mapping", "invalid_document")
            capability, route_id = entry.get("capability"), entry.get("route")
            if not isinstance(capability, str) or not isinstance(route_id, str):
                raise RefusedInput(f"{where}: needs capability and route", "invalid_document")
            split_route(route_id)
            key = (capability, route_id)
            if key in seen:
                raise RefusedInput(
                    f"{where}: {route_id} for {capability} is already declared by "
                    f"routes[{seen[key]}] of the same table",
                    "duplicate_route",
                )
            seen[key] = index
            contract = entry.get("contract", {})
            if not isinstance(contract, dict):
                raise RefusedInput(f"{where}: contract must be a mapping", "invalid_document")
            self.entries[key] = (where, contract)

    def lookup(self, capability: str, route_id: str) -> tuple[str, dict[str, Any]] | None:
        """(where it was declared, its contract) of the entry in force, or None."""
        return self.entries.get((capability, route_id))


def _contains_pending(value: Any) -> bool:
    if isinstance(value, dict):
        if set(value) == {"pending"}:
            return True
        return any(_contains_pending(item) for item in value.values())
    if isinstance(value, list):
        return any(_contains_pending(item) for item in value)
    return False


def _take_list(instance: dict[str, Any]) -> list[int] | None:
    """The instance's take: a non-empty list of integers >= 1 (identity.md section 8), or None."""
    take = instance.get("take")
    if not isinstance(take, list) or not take:
        return None
    for number in take:
        if isinstance(number, bool) or not isinstance(number, int) or number < 1:
            return None
    return take


_BUILTIN_USES = re.compile(r"fx/[^@\s]+@[0-9]+\Z")
_BUILTIN_TYPE = re.compile(r"fx/[^@\s]+@[0-9]+\.[0-9]+\Z")
_LOCAL_USES = re.compile(r"\./(?P<path>[^#\s]+)#(?P<attr>[A-Za-z_][A-Za-z0-9_]*)\Z")
_LOCAL_TYPE = re.compile(r"(?P<path>[^#\s]+)#(?P<attr>[A-Za-z_][A-Za-z0-9_]*)@[0-9]+\Z")


def _type_problem(uses: Any, type_identity: str) -> str | None:
    """Check a printed type identity against its uses: as identity.md section 6 defines it."""
    if type_identity.startswith("source:"):
        if not _HEX64.match(type_identity[len("source:") :]):
            return f"type {type_identity!r} is not source:<64 hex>"
        if isinstance(uses, str) and not _LOCAL_USES.match(uses):
            return (
                f"type {type_identity!r} is a source identity, "
                f"but uses {uses!r} is not ./<path>#<attr>"
            )
        return None
    if _BUILTIN_TYPE.match(type_identity):
        if isinstance(uses, str) and not (
            _BUILTIN_USES.match(uses) and type_identity.startswith(uses + ".")
        ):
            return f"type {type_identity!r} is not {uses!r} plus a version"
        return None
    local = _LOCAL_TYPE.match(type_identity)
    if local:
        used = _LOCAL_USES.match(uses) if isinstance(uses, str) else None
        if isinstance(uses, str) and not (
            used and used["path"] == local["path"] and used["attr"] == local["attr"]
        ):
            return f"type {type_identity!r} does not name uses {uses!r}"
        if local["path"].startswith(("./", "/")) or "\\" in local["path"]:
            return f"type {type_identity!r}: the path must be POSIX, project-relative, without ./"
        return None
    return f"type {type_identity!r} matches no form in identity.md section 6"


def _unsafe_relative(path: str) -> bool:
    """An absolute path, or one that climbs out of its base."""
    return (
        not path
        or path.startswith(("/", "\\"))
        or Path(path).is_absolute()
        or any(part in ("", ".", "..") for part in path.replace("\\", "/").split("/"))
    )


class GraphCheck:
    def __init__(self, project: Path, route_files: list[Path]) -> None:
        self.project = project
        self.failures: list[str] = []
        self.notes: list[str] = []
        self.counts = {"identities": 0, "routes": 0, "sources": 0, "locks": 0, "plans": 0}
        self.routes = RouteTable()
        # identity.md section 7: the built-in default table heads the catalog unless a
        # --routes file is given. This checker does not know that table.
        self.builtin_table = not route_files
        self.lock: dict[str, str] = {}
        self._load_project(route_files)

    def fail(self, message: str) -> None:
        self.failures.append(message)

    def _load_project(self, route_files: list[Path]) -> None:
        # The catalog, in order: the built-in table (unknown here), fx.yaml's route_tables in
        # their listed order (relative to the project root), then each --routes file.
        tables: list[Path] = []
        project_file = self.project / "fx.yaml"
        if project_file.is_file():
            document = read_yaml_file(project_file)
            listed = document.get("route_tables") if isinstance(document, dict) else None
            if listed is not None and not isinstance(listed, list):
                raise RefusedInput(f"{project_file}: route_tables must be a list", "invalid")
            tables.extend(self.project / str(item) for item in listed or [])
        tables.extend(route_files)
        for path in tables:
            self.routes.load(path)
        lock_file = self.project / "fx.lock"
        if lock_file.is_file():
            document = read_yaml_file(lock_file)
            nodes = document.get("nodes") if isinstance(document, dict) else None
            if isinstance(nodes, dict):
                self.lock = {str(k): str(v) for k, v in nodes.items()}

    def _project_file(self, relative: str, what: str) -> Path | None:
        """The project file at a project-relative path; None (and a failure) if unsafe."""
        if _unsafe_relative(relative):
            self.fail(f"{what}: {relative!r} is not a relative path inside its base")
            return None
        path = self.project / relative
        try:
            path.resolve().relative_to(self.project.resolve())
        except ValueError:
            self.fail(f"{what}: {relative!r} is outside the project")
            return None
        return path

    def _read_project_file(self, relative: str, what: str) -> bytes | None:
        path = self._project_file(relative, what)
        if path is None:
            return None
        if not path.is_file():
            self.fail(f"{what}: {relative!r} does not exist in the project")
            return None
        return path.read_bytes()

    def check(self, graph: Any) -> None:
        if not isinstance(graph, dict):
            self.fail("the graph is not a JSON object")
            return
        if graph.get("kind") != "fx-graph-v1":
            self.fail(f"kind is {graph.get('kind')!r}, expected 'fx-graph-v1'")
        has_problems = bool(graph.get("problems"))
        instances = graph.get("instances")
        if not isinstance(instances, list):
            self.fail("instances is not a list")
            return
        for instance in instances:
            if isinstance(instance, dict):
                self._check_instance(instance, has_problems)
            else:
                self.fail(f"instance {instance!r} is not an object")
        self._check_types(graph.get("types"), instances, has_problems)
        if "plan" in graph:
            self._check_plan(graph, instances)

    def _check_plan(self, graph: dict[str, Any], instances: list[Any]) -> None:
        """Recompute the plan digest of a run folder's plan.json (identity.md section 10)."""
        printed = graph.get("plan")
        workflow = graph.get("workflow")
        source = workflow.get("file") if isinstance(workflow, dict) else None
        workflow_id = workflow.get("id") if isinstance(workflow, dict) else None
        skip = "plan: the plan digest is not checked: "
        if not isinstance(source, str) or not isinstance(workflow_id, str):
            self.notes.append(skip + "the graph names no workflow file")
            return
        if ":" in source:
            self.notes.append(skip + f"{source} is a builder, which this checker cannot run")
            return
        inputs = graph.get("inputs")
        if not isinstance(inputs, dict):
            self.notes.append(skip + "the graph carries no inputs")
            return
        workflows: dict[str, Any] = {}
        wanted = [(source, "workflow.file")]
        steps = graph.get("steps") if isinstance(graph.get("steps"), dict) else {}
        for path, step in sorted(steps.items()):
            uses = step.get("uses") if isinstance(step, dict) else None
            if isinstance(uses, str) and uses.endswith((".yaml", ".yml")):
                wanted.append((uses.removeprefix("./"), f"steps[{path!r}].uses"))
        for relative, what in wanted:
            if relative in workflows:
                continue
            data = self._read_project_file(relative, f"plan: {what}")
            if data is None:
                return
            workflows[relative] = load_yaml(data, relative)
        takes: dict[str, Any] = {}
        takes_file = (Path(source).parent / f"{workflow_id}.takes.yaml").as_posix()
        if (self.project / takes_file).is_file():
            document = read_yaml_file(self.project / takes_file)
            if not isinstance(document, dict):
                self.fail(f"plan: {takes_file} is not a mapping of step paths")
                return
            for path, entry in document.items():
                takes[path] = entry.get("take") if isinstance(entry, dict) else entry
        types: dict[str, str] = {}
        routes: set[str] = set()
        for instance in instances:
            if not isinstance(instance, dict):
                continue
            uses, type_identity = instance.get("uses"), instance.get("type")
            if not isinstance(uses, str) or not isinstance(type_identity, str):
                self.notes.append(skip + f"{instance.get('id')} has no type identity")
                return
            if types.setdefault(uses, type_identity) != type_identity:
                self.fail(f"plan: uses {uses!r} has two type identities")
                return
            # Each capability and route the plan binds, by fingerprint: one route serving two
            # capabilities has two fingerprints, and the list holds both.
            for bound in (instance.get("routes") or {}).values():
                if isinstance(bound, dict) and isinstance(bound.get("fingerprint"), str):
                    routes.add(bound["fingerprint"])
        hashed = plan_object(workflows, inputs, takes, types, routes)
        computed = digest(hashed)
        self.counts["plans"] += 1
        if computed != printed:
            self.fail(
                "plan digest differs\n"
                f"  printed:  {printed}\n"
                f"  computed: {computed}\n"
                f"  hashed:   {canon(hashed).decode('utf-8')}"
            )

    def _check_instance(self, instance: dict[str, Any], has_problems: bool) -> None:
        name = str(instance.get("id", "<no id>"))
        identity = instance.get("identity")
        type_identity = instance.get("type")
        with_values = instance.get("with")

        printed_routes: dict[str, str] = {}
        routes = instance.get("routes") or {}
        if not isinstance(routes, dict):
            self.fail(f"{name}: routes is not an object")
            routes = {}
        for capability, bound in routes.items():
            if not isinstance(bound, dict) or "route" not in bound or "fingerprint" not in bound:
                self.fail(f"{name}: routes.{capability} is not {{route, fingerprint}}: {bound!r}")
                continue
            printed_routes[capability] = bound["fingerprint"]
            self._check_route(name, capability, bound["route"], bound["fingerprint"], has_problems)

        if isinstance(type_identity, str):
            problem = _type_problem(instance.get("uses"), type_identity)
            if problem:
                self.fail(f"{name}: {problem}")

        take = _take_list(instance)
        if take is None:
            self.fail(
                f"{name}: take {instance.get('take')!r} is not a non-empty list of integers >= 1"
            )
            return

        if identity is None:
            if (
                isinstance(type_identity, str)
                and isinstance(with_values, dict)
                and not _contains_pending(with_values)
            ):
                message = f"{name}: identity is null, but nothing in its with: is pending"
                (self.notes.append if has_problems else self.fail)(message)
            return
        if not isinstance(identity, str) or not _HEX64.match(identity):
            self.fail(f"{name}: identity {identity!r} is not 64 lowercase hex characters")
            return
        if not isinstance(type_identity, str):
            self.fail(f"{name}: has an identity but no type")
            return
        if not isinstance(with_values, dict):
            self.fail(f"{name}: has an identity but with is {with_values!r}")
            return
        if _contains_pending(with_values):
            self.fail(f"{name}: has an identity although its with: holds a pending value")
            return
        hashed = step_object(type_identity, with_values, printed_routes, take)
        expected = digest(hashed)
        self.counts["identities"] += 1
        if expected != identity:
            self.fail(
                f"{name}: step identity differs\n"
                f"  printed:  {identity}\n"
                f"  computed: {expected}\n"
                f"  hashed:   {canon(hashed).decode('utf-8')}"
            )

    def _check_route(
        self, name: str, capability: str, route_id: Any, printed: Any, has_problems: bool
    ) -> None:
        if not isinstance(route_id, str):
            self.fail(f"{name}: routes.{capability}.route is {route_id!r}")
            return
        try:
            split_route(route_id)
        except RefusedInput as error:
            self.fail(f"{name}: routes.{capability}: {error}")
            return
        entry = self.routes.lookup(capability, route_id)
        if entry is None:
            if self.builtin_table:
                self.notes.append(
                    f"{name}: {route_id} for {capability} is in no project route table; it "
                    "may come from the built-in default table, which this checker does not "
                    "know, so its fingerprint is not checked"
                )
            else:
                message = f"{name}: {route_id} for {capability} is in none of the route tables"
                (self.notes.append if has_problems else self.fail)(message)
            return
        where, contract = entry
        hashed = route_object(capability, route_id, contract)
        computed = digest(hashed)
        self.counts["routes"] += 1
        if printed != computed:
            self.fail(
                f"{name}: route fingerprint for {capability} on {route_id} differs\n"
                f"  printed:  {printed}\n"
                f"  computed: {computed} (from {where})\n"
                f"  hashed:   {canon(hashed).decode('utf-8')}"
            )

    def _source_digest(self, where: str, source: Any, has_problems: bool) -> str | None:
        """digest(source) from the printed digests, after re-hashing what the project holds."""
        if not isinstance(source, dict) or set(source) != {"files", "resources"}:
            self.fail(f"{where}.source is not {{files, resources}}: {source!r}")
            return None
        files, resources = source["files"], source["resources"]
        if not isinstance(files, dict) or not files or not isinstance(resources, dict):
            self.fail(f"{where}.source: files must be a non-empty object, resources an object")
            return None
        usable = True
        for group, mapping in (("files", files), ("resources", resources)):
            for path, printed in mapping.items():
                what = f"{where}.source.{group}[{path!r}]"
                if not isinstance(printed, str) or not _HEX64.match(printed):
                    self.fail(f"{what}: {printed!r} is not 64 lowercase hex characters")
                    usable = False
                    continue
                found = self._project_file(path, what)
                if found is None:
                    usable = False
                elif found.is_file():
                    actual = file_digest(found.read_bytes())
                    if actual != printed:
                        self.fail(f"{what}: printed {printed}, the project file hashes to {actual}")
                elif group == "files":
                    # A label may be relative to a declared source package, not the project.
                    self.notes.append(f"{what}: not under the project, so it is not re-hashed")
                else:
                    # identity.md section 6: a declared resource missing from the project is a
                    # refusal while planning.
                    message = f"{what}: the declared resource does not exist in the project"
                    (self.notes.append if has_problems else self.fail)(message)
        return node_source_digest(files, resources) if usable else None

    def _check_types(self, types: Any, instances: list[Any], has_problems: bool) -> None:
        if types is None:
            self.notes.append("the graph has no types map: sources and fx.lock are not checked")
            return
        if not isinstance(types, dict):
            self.fail("types is not an object")
            return
        for uses, entry in types.items():
            where = f"types[{uses!r}]"
            if not isinstance(entry, dict) or not isinstance(entry.get("identity"), str):
                self.fail(f"{where} is not an object with an identity: {entry!r}")
                continue
            identity = entry["identity"]
            problem = _type_problem(uses, identity)
            if problem:
                self.fail(f"{where}: {problem}")
                continue
            source = entry.get("source")
            if _BUILTIN_TYPE.match(identity):
                if source is not None:
                    self.fail(f"{where}: a built-in type has no source")
                continue
            if source is None:
                self.fail(f"{where}: a project type must carry its source")
                continue
            computed = self._source_digest(where, source, has_problems)
            if identity.startswith("source:"):
                if computed is None:
                    continue
                self.counts["sources"] += 1
                if "source:" + computed != identity:
                    self.fail(
                        f"{where}: node source differs\n"
                        f"  printed:  {identity}\n"
                        f"  computed: source:{computed}"
                    )
                continue
            # A versioned project type: its source digest is what fx.lock must hold.
            locked = self.lock.get(identity)
            if locked is None:
                self.notes.append(f"{where}: {identity} is unlocked (fx.lock has no entry)")
                continue
            if computed is None:
                continue
            self.counts["locks"] += 1
            if computed != locked:
                message = (
                    f"{where}: fx.lock has {locked} for {identity}, its source hashes to {computed}"
                )
                (self.notes.append if has_problems else self.fail)(message)
        for instance in instances:
            if not isinstance(instance, dict):
                continue
            uses, type_identity = instance.get("uses"), instance.get("type")
            if not isinstance(uses, str) or type_identity is None:
                continue
            entry = types.get(uses)
            if not isinstance(entry, dict):
                self.fail(f"{instance.get('id')}: uses {uses!r} is not in the types map")
            elif entry.get("identity") != type_identity:
                self.fail(
                    f"{instance.get('id')}: type {type_identity!r}, but {f'types[{uses!r}]'} "
                    f"says {entry.get('identity')!r}"
                )


def check_graph(graph_path: Path, project: Path, route_files: list[Path]) -> GraphCheck:
    checker = GraphCheck(project, route_files)
    checker.check(read_json_file(graph_path))
    return checker


# ---------------------------------------------------------------------------------------------
# Command line
# ---------------------------------------------------------------------------------------------


def _print_lines(lines: Iterable[str], stream: Any = None) -> None:
    # sys.stdout is looked up at call time, so a caller that swaps it (a test) sees the lines.
    for line in lines:
        print(line, file=stream or sys.stdout)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Recompute FX identities from spec/identity.md, independently of the engine."
    )
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check-examples", action="store_true", help="check the identity vectors")
    mode.add_argument("--check-graph", metavar="GRAPH", type=Path, help="an fx-graph-v1 document")
    mode.add_argument(
        "--canon", metavar="FILE", help="print a JSON file's canonical form (- for stdin)"
    )
    parser.add_argument("--examples", type=Path, default=DEFAULT_EXAMPLES)
    parser.add_argument("--project", type=Path, help="the project the graph was planned in")
    parser.add_argument("--routes", type=Path, action="append", default=[], help="a route table")
    args = parser.parse_args(argv)

    try:
        if args.canon is not None:
            if args.canon == "-":
                value = parse_ijson(sys.stdin.buffer.read())
            else:
                value = read_json_file(Path(args.canon))
            sys.stdout.buffer.write(canon(value) + b"\n")
            return 0

        if args.check_examples:
            failures = check_examples(args.examples)
            checked = [f"{len(read_json_file(args.examples)['examples'])} identity examples"]
            # The other identity vectors, when they sit beside the examples.
            for file_name, check in IDENTITY_VECTORS.items():
                path = args.examples.parent / file_name
                if path.is_file():
                    failures.extend(f"{file_name}: {line}" for line in check(path))
                    checked.append(file_name)
            if failures:
                _print_lines((f"FAIL {line}" for line in failures), sys.stderr)
                return 1
            print(f"ok: {', '.join(checked)} reproduce")
            return 0

        if args.project is None:
            parser.error("--check-graph needs --project")
        checker = check_graph(args.check_graph, args.project, args.routes)
    except (RefusedInput, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    _print_lines(f"note: {line}" for line in checker.notes)
    sys.stdout.flush()
    counts = checker.counts
    summary = (
        f"{counts['identities']} step identities, {counts['routes']} route fingerprints, "
        f"{counts['sources']} node sources, {counts['locks']} lock entries, "
        f"{counts['plans']} plan digests"
    )
    if checker.failures:
        _print_lines((f"FAIL {line}" for line in checker.failures), sys.stderr)
        print(f"{len(checker.failures)} mismatches; checked {summary}", file=sys.stderr)
        return 1
    print(f"ok: {summary} agree")
    return 0


if __name__ == "__main__":
    sys.exit(main())
