"""The spec gate: checks that spec/, conformance/ and docs/guide/ hold together.

Run from the python/ project, which carries the dependencies (jsonschema, rfc8785, pyyaml):

    uv run --project python python tools/check_spec.py

It prints one line per check and exits non-zero when any check fails:

  (a) every spec/schemas/*.schema.json is a valid draft 2020-12 schema with the right $id and a
      title, and its declared properties are lower_snake_case;
  (b) every authored YAML document under conformance/*/in/ and examples/ is in the
      strict YAML subset and validates against the schema its `fx:` discriminator names;
  (c) the JCS vectors: accept cases canonicalize to their text (with rfc8785 and with
      tools/digest.py), refuse cases are refused by digest.py's I-JSON reader for their reason;
  (d) the YAML vectors are complete (accept/<n>.yaml has a valid <n>.json, refuse/<n>.yaml a
      non-empty <n>.txt) and agree with digest.py's reader of the subset;
  (e) tools/digest.py --check-examples passes: the identity examples, the reserved-marker
      vectors, the JSON output vectors and the route vectors;
  (f) no FORBIDDEN_NAMES appear under spec/, docs/, conformance/, examples/, the default
      routes' folder or the providers crate's source, which holds the adapters' references;
  (g) the worked examples in spec/identity.md agree with spec/vectors/identity/examples.json;
  (h) every expected fx-graph-v1 output of a conformance case passes digest.py --check-graph,
      with the case's in/ as the project, the route tables its step passed, and the built-in
      table (in the catalog when the step passed none, and a base for `extends` either way);
  (i) the identity vectors agree with other readers and writers: every JSON output case that
      identity.md section 5 says Python's json.dumps writes the same is written the same, and
      every reserved-marker case is refused (or read) by the YAML reader as by the JSON reader;
  (j) the built-in route table (crates/grida-fx-providers/routes/default.yaml) is in the strict
      YAML subset and validates against fx-routes-v1; its size is the one spec/providers.md
      section 3 states; every route's capability has its section in spec/capabilities.md and is
      a paid built-in's of the standard library (or agent.turn, the engine's own); every feature
      is in its capability's feature table; every contract's adapter and adapter_behavior are
      the ones the built-in adapters table of spec/providers.md section 2 gives its capability
      and provider, whose every reference exists and heads with the same contracts, and which
      tools/digest.py's own copy (BUILT_IN_ADAPTERS, which completes a contract) agrees with;
      and the guide's table of built-in routes (docs/guide/05-running.md) lists every route
      under its capability with the table's prices;
  (k) the planner's feature vocabulary (crates/grida-fx-core/src/expand/features.json) lists, for
      every capability section of spec/capabilities.md and no other, exactly its feature table;
      and each name it renames is one spec/providers.md section 4 drops, is a feature of no
      capability, and becomes a feature of one;
  (l) every reference to a section of a spec file, as `<file>.md §N` (or `section N`, with the
      file's link, path or backticks between), in the repository's text files names a numbered
      heading that file has.
"""

from __future__ import annotations

import re
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

TOOLS = Path(__file__).resolve().parent
REPO = TOOLS.parent
sys.path.insert(0, str(TOOLS))

import digest  # noqa: E402  (a sibling script, not a package)

SPEC = REPO / "spec"
SCHEMAS = SPEC / "schemas"
VECTORS = SPEC / "vectors"
CONFORMANCE = REPO / "conformance"
GUIDE = REPO / "docs" / "guide"
EXAMPLES = REPO / "examples"
DEFAULT_ROUTES = REPO / "crates" / "grida-fx-providers" / "routes" / "default.yaml"
STD_CATALOG = REPO / "crates" / "grida-fx-core" / "src" / "builtins" / "catalog.json"
FEATURES = REPO / "crates" / "grida-fx-core" / "src" / "expand" / "features.json"
PROVIDERS_SRC = REPO / "crates" / "grida-fx-providers" / "src"

DRAFT_2020_12 = "https://json-schema.org/draft/2020-12/schema"
SCHEMA_ID_PREFIX = "urn:grida-fx:schema:"
DISCRIMINATORS = {
    "project/v1": "fx-project-v1",
    "workflow/v1": "fx-workflow-v1",
    "routes/v1": "fx-routes-v1",
    "lock/v1": "fx-lock-v1",
}
TAKES_SCHEMA = "fx-takes-v1"
FORBIDDEN_NAMES = (
    b"gnode",
    b"Gnode",
    b"GNODE",
    b"stage-gen",
    b"stage_gen",
    b"Stage Gen",
    b"STAGE_GEN",
)
# Where they may not appear: the contracts and their prose, the docs, the cases, the examples,
# and the adapters' references beside their code.
NAME_ROOTS = (SPEC, REPO / "docs", CONFORMANCE, EXAMPLES, DEFAULT_ROUTES.parent, PROVIDERS_SRC)
# Contract fields are lower_snake_case. External vocabulary ($ref, ...) and the x-fx-*
# extension keywords keep their own spelling.
SNAKE_CASE = re.compile(r"[a-z][a-z0-9_]*\Z|\$[A-Za-z]+\Z|x-fx-[a-z0-9-]+\Z")
MAX_DETAILS = 8


class Gate:
    def __init__(self) -> None:
        self.failures = 0

    def report(
        self, status: str, check: str, subject: str, details: list[str] | None = None
    ) -> None:
        if status == "FAIL":
            self.failures += 1
        print(f"{status:<4} ({check}) {subject}")
        shown = details or []
        for line in shown[:MAX_DETAILS]:
            for part in str(line).splitlines():
                print(f"       {part}")
        if len(shown) > MAX_DETAILS:
            print(f"       ... and {len(shown) - MAX_DETAILS} more")

    def result(self, check: str, subject: str, problems: list[str]) -> None:
        self.report("FAIL" if problems else "ok", check, subject, problems)


def rel(path: Path) -> str:
    try:
        return path.relative_to(REPO).as_posix()
    except ValueError:
        return path.as_posix()


# ---------------------------------------------------------------------------------------------
# (a) schemas
# ---------------------------------------------------------------------------------------------


def _property_names(node: Any, where: str = "#") -> Iterator[tuple[str, str]]:
    """Every name declared under a `properties` keyword, anywhere in a schema."""
    if isinstance(node, dict):
        properties = node.get("properties")
        if isinstance(properties, dict):
            for name in properties:
                yield where + "/properties", name
        for key, value in node.items():
            yield from _property_names(value, f"{where}/{key}")
    elif isinstance(node, list):
        for index, value in enumerate(node):
            yield from _property_names(value, f"{where}/{index}")


def load_schemas(gate: Gate) -> dict[str, Any]:
    from jsonschema import Draft202012Validator
    from jsonschema.exceptions import SchemaError

    files = sorted(SCHEMAS.glob("*.schema.json")) if SCHEMAS.is_dir() else []
    if not files:
        gate.report("skip", "a", f"{rel(SCHEMAS)}: no schema files")
        return {}
    schemas: dict[str, Any] = {}
    for path in files:
        stem = path.name[: -len(".schema.json")]
        problems: list[str] = []
        try:
            schema = digest.read_json_file(path)
        except digest.RefusedInput as error:
            gate.result("a", rel(path), [str(error)])
            continue
        if not isinstance(schema, dict):
            gate.result("a", rel(path), ["not a JSON object"])
            continue
        try:
            Draft202012Validator.check_schema(schema)
        except SchemaError as error:
            problems.append(f"not a valid draft 2020-12 schema: {error.message}")
        if not re.fullmatch(r"fx-[a-z0-9]+(?:[.-][a-z0-9]+)*-v[0-9]+", stem):
            problems.append(f"file name {path.name!r} is not fx-<name>-v<N>.schema.json")
        if schema.get("$schema", DRAFT_2020_12) != DRAFT_2020_12:
            problems.append(f"$schema is {schema.get('$schema')!r}, expected {DRAFT_2020_12}")
        if schema.get("$id") != SCHEMA_ID_PREFIX + stem:
            problems.append(f"$id is {schema.get('$id')!r}, expected {SCHEMA_ID_PREFIX + stem}")
        title = schema.get("title")
        if not isinstance(title, str) or not title.strip():
            problems.append("no title")
        for where, name in _property_names(schema):
            if not SNAKE_CASE.match(name):
                problems.append(f"{where}: property {name!r} is not lower_snake_case")
        gate.result("a", rel(path), problems)
        schemas[stem] = schema
    return schemas


def _validator(schemas: dict[str, Any], stem: str) -> Any:
    from jsonschema import Draft202012Validator
    from referencing import Registry, Resource
    from referencing.jsonschema import DRAFT202012

    resources: list[tuple[str, Resource]] = []
    for name, schema in schemas.items():
        resource = Resource(contents=schema, specification=DRAFT202012)
        resources.append((SCHEMA_ID_PREFIX + name, resource))
        resources.append((f"{name}.schema.json", resource))
    registry = Registry().with_resources(resources)
    return Draft202012Validator(schemas[stem], registry=registry)


# ---------------------------------------------------------------------------------------------
# (b) authored YAML documents
# ---------------------------------------------------------------------------------------------


def _authored_documents() -> Iterator[Path]:
    """Every authored YAML document the gate validates."""
    if CONFORMANCE.is_dir():
        for case_dir in sorted(p for p in CONFORMANCE.iterdir() if (p / "in").is_dir()):
            inside = case_dir / "in"
            found = [inside / "fx.yaml", inside / "routes.yaml", inside / "fx.lock"]
            workflows = inside / "workflows"
            if workflows.is_dir():
                found += sorted(workflows.glob("*.yaml")) + sorted(workflows.glob("*.yml"))
            found += sorted(inside.glob("*.takes.yaml"))
            yield from (path for path in found if path.is_file())
    if EXAMPLES.is_dir():
        for path in sorted(EXAMPLES.rglob("*")):
            # What running an example writes is not authored (.gitignore).
            if not path.is_file() or {".fx", "runs", ".venv", "node_modules"} & set(
                path.relative_to(EXAMPLES).parts
            ):
                continue
            if path.name in ("fx.yaml", "routes.yaml", "fx.lock") or path.name.endswith(
                ".takes.yaml"
            ):
                yield path
            elif path.parent.name == "workflows" and path.suffix in (".yaml", ".yml"):
                yield path


def _schema_errors(validator: Any, document: Any) -> list[str]:
    errors = sorted(validator.iter_errors(document), key=lambda e: list(e.absolute_path))
    out = []
    for error in errors:
        where = "/".join(str(part) for part in error.absolute_path) or "(root)"
        out.append(f"{where}: {error.message}")
    return out


def check_documents(gate: Gate, schemas: dict[str, Any]) -> None:
    documents = list(_authored_documents())
    if not documents:
        gate.report("skip", "b", "no authored YAML documents found")
        return
    validators: dict[str, Any] = {}
    if not schemas:
        # Nothing to validate against yet: still hold every document to the YAML subset.
        loaded = 0
        for path in documents:
            try:
                document = digest.read_yaml_file(path)
            except digest.RefusedInput as error:
                gate.result("b", rel(path), [str(error).replace(str(path), rel(path))])
                continue
            tag = document.get("fx") if isinstance(document, dict) else None
            if not path.name.endswith(".takes.yaml") and tag not in DISCRIMINATORS:
                gate.result(
                    "b", rel(path), [f"fx: is {tag!r}, not one of {sorted(DISCRIMINATORS)}"]
                )
                continue
            loaded += 1
        gate.report("skip", "b", f"{loaded} documents load as strict YAML; no schemas to validate")
        return
    for path in documents:
        subject = rel(path)
        try:
            document = digest.read_yaml_file(path)
        except digest.RefusedInput as error:
            gate.result("b", subject, [str(error).replace(str(path), subject)])
            continue
        if path.name.endswith(".takes.yaml"):
            stem = TAKES_SCHEMA
        else:
            tag = document.get("fx") if isinstance(document, dict) else None
            stem = DISCRIMINATORS.get(tag) if isinstance(tag, str) else None
            if stem is None:
                gate.result("b", subject, [f"fx: is {tag!r}, not one of {sorted(DISCRIMINATORS)}"])
                continue
        if stem not in schemas:
            gate.result("b", subject, [f"no schema {stem} in {rel(SCHEMAS)}"])
            continue
        if stem not in validators:
            validators[stem] = _validator(schemas, stem)
        gate.result("b", f"{subject} ({stem})", _schema_errors(validators[stem], document))


# ---------------------------------------------------------------------------------------------
# (c) JCS vectors, (d) YAML vectors
# ---------------------------------------------------------------------------------------------


def check_jcs(gate: Gate) -> None:
    import rfc8785

    folder = VECTORS / "jcs"
    files = sorted(folder.glob("*.json")) if folder.is_dir() else []
    if not files:
        gate.report("skip", "c", f"{rel(folder)}: no vectors")
        return
    for path in files:
        problems: list[str] = []
        accepted = refused = 0
        try:
            cases = digest.read_json_file(path).get("cases")
        except (digest.RefusedInput, AttributeError) as error:
            gate.result("c", rel(path), [f"unreadable: {error}"])
            continue
        if not isinstance(cases, list) or not cases:
            gate.result("c", rel(path), ["no cases"])
            continue
        for case in cases:
            name = case.get("name", "<unnamed>")
            text = case.get("input")
            if not isinstance(text, str):
                problems.append(f"{name}: no input text")
                continue
            if "refuse" in case:
                try:
                    digest.parse_ijson(text)
                    problems.append(f"{name}: accepted, should be refused ({case['refuse']})")
                except digest.RefusedInput as error:
                    if error.code != case["refuse"]:
                        problems.append(
                            f"{name}: refused as {error.code}, expected {case['refuse']}: {error}"
                        )
                    refused += 1
                continue
            want = case.get("canonical")
            try:
                value = digest.parse_ijson(text)
            except digest.RefusedInput as error:
                problems.append(f"{name}: refused ({error}), should canonicalize to {want}")
                continue
            library = rfc8785.dumps(value).decode("utf-8")
            ours = digest.canon(value).decode("utf-8")
            if library != want:
                problems.append(f"{name}: rfc8785 gives {library!r}, expected {want!r}")
            if ours != want:
                problems.append(f"{name}: digest.py gives {ours!r}, expected {want!r}")
            if "digest" in case and digest.digest(value) != case["digest"]:
                problems.append(f"{name}: digest {digest.digest(value)}, expected {case['digest']}")
            accepted += 1
        gate.result("c", f"{rel(path)}: {accepted} accept, {refused} refuse", problems)


def check_yaml_vectors(gate: Gate) -> None:
    folder = VECTORS / "yaml"
    accept, refuse = folder / "accept", folder / "refuse"
    if not accept.is_dir() and not refuse.is_dir():
        gate.report("skip", "d", f"{rel(folder)}: no vectors")
        return
    problems: list[str] = []
    accepted = sorted(accept.glob("*.yaml")) if accept.is_dir() else []
    refused = sorted(refuse.glob("*.yaml")) if refuse.is_dir() else []
    for path in accepted:
        expected = path.with_suffix(".json")
        if not expected.is_file():
            problems.append(f"{rel(path)}: no {expected.name}")
            continue
        try:
            want = digest.read_json_file(expected)
        except digest.RefusedInput as error:
            problems.append(f"{rel(expected)}: not valid JSON: {error}")
            continue
        # Cross-check with digest.py's own reader of the subset: compare values, not text.
        try:
            got = digest.load_yaml(path.read_bytes(), path.name)
        except digest.RefusedInput as error:
            problems.append(f"{rel(path)}: digest.py refuses it: {error}")
            continue
        if digest.canon(got) != digest.canon(want):
            problems.append(f"{rel(path)}: digest.py reads {digest.canon(got).decode('utf-8')}")
    for path in refused:
        reason = path.with_suffix(".txt")
        if not reason.is_file() or not reason.read_text(encoding="utf-8").strip():
            problems.append(f"{rel(path)}: no non-empty {reason.name}")
        try:
            got = digest.load_yaml(path.read_bytes(), path.name)
            problems.append(f"{rel(path)}: digest.py accepts it as {got!r}")
        except digest.RefusedInput:
            pass
    for folder_path, partner in ((accept, ".json"), (refuse, ".txt")):
        if folder_path.is_dir():
            for orphan in sorted(folder_path.glob(f"*{partner}")):
                if not orphan.with_suffix(".yaml").is_file():
                    problems.append(f"{rel(orphan)}: no .yaml beside it")
    if not accepted and not refused:
        problems.append("no .yaml vectors")
    gate.result("d", f"{rel(folder)}: {len(accepted)} accept, {len(refused)} refuse", problems)


# ---------------------------------------------------------------------------------------------
# (e) identity examples, (f) names, (g) identity.md worked examples
# ---------------------------------------------------------------------------------------------


def check_identity_examples(gate: Gate) -> None:
    command = [sys.executable, str(TOOLS / "digest.py"), "--check-examples"]
    done = subprocess.run(command, capture_output=True, text=True, timeout=120)
    output = (done.stdout + done.stderr).strip().splitlines()
    gate.result("e", "digest.py --check-examples", [] if done.returncode == 0 else output)


def check_names(gate: Gate) -> None:
    hits: list[str] = []
    roots = [p for p in NAME_ROOTS if p.is_dir()]
    for root in roots:
        for path in sorted(root.rglob("*")):
            # What running an example writes, and its own environment, are not authored: an
            # example's installed packages, and the @grida/fx a conformance run installs. A
            # case's own node_modules fixtures are authored.
            parts = path.relative_to(root).parts
            generated = bool({"__pycache__", ".fx", "runs", ".venv"} & set(parts)) or (
                "node_modules" in parts
                and (
                    root == EXAMPLES or parts[parts.index("node_modules") + 1 :][:1] == ("@grida",)
                )
            )
            if not path.is_file() or generated:
                continue
            data = path.read_bytes()
            if not any(name in data for name in FORBIDDEN_NAMES):
                continue
            lines = [
                str(number)
                for number, line in enumerate(data.splitlines(), start=1)
                if any(name in line for name in FORBIDDEN_NAMES)
            ]
            hits.append(f"{rel(path)}: line {', '.join(lines)}")
    subject = "no forbidden names in " + ", ".join(rel(r) + "/" for r in roots)
    gate.result("f", subject, hits)


_WORKED = re.compile(r"^\s+(canon|digest|file_digest|type_identity|instance_id)\s*=\s*(\S.*?)\s*$")


def check_worked_examples(gate: Gate) -> None:
    spec = SPEC / "identity.md"
    vectors = VECTORS / "identity" / "examples.json"
    if not spec.is_file() or not vectors.is_file():
        gate.report("skip", "g", "spec/identity.md or its examples are missing")
        return
    examples = digest.read_json_file(vectors).get("examples", [])
    canonicals = {e.get("canonical") for e in examples}
    digests = {e.get("digest") for e in examples}
    instance_ids = {e.get("instance_id") for e in examples}
    type_identities = {e.get("type_identity") for e in examples}
    text = spec.read_text(encoding="utf-8")
    start = text.find("## 14.")
    problems: list[str] = []
    seen = 0
    for line in text[start if start >= 0 else 0 :].splitlines():
        match = _WORKED.match(line)
        if not match:
            continue
        seen += 1
        name, value = match.groups()
        if name == "canon" and value not in canonicals:
            problems.append(f"canon not in the vectors: {value}")
        elif name in ("digest", "file_digest") and value not in digests:
            problems.append(f"{name} not in the vectors: {value}")
        elif name == "type_identity" and value not in type_identities:
            problems.append(f"type_identity not in the vectors: {value}")
        elif name == "instance_id" and value not in instance_ids:
            problems.append(f"instance_id not in the vectors: {value}")
    if not seen:
        problems.append("no worked examples found in section 14")
    gate.result("g", f"{rel(spec)} section 14: {seen} values match {rel(vectors)}", problems)


# ---------------------------------------------------------------------------------------------
# (i) identity vectors against other readers and writers
# ---------------------------------------------------------------------------------------------


def _python_writes_the_same(value: Any) -> bool:
    """identity.md section 5: json.dumps(v, sort_keys=True, indent=1, ensure_ascii=False) writes
    the canonical lines when every number is an integer or a decimal without an exponent, and
    every key lies in the Basic Multilingual Plane. A Python float that is a whole number (1.0)
    is written `1.0`, so only Python ints count as integers here."""
    if isinstance(value, float):
        return not value.is_integer() and "e" not in repr(value)
    if isinstance(value, list):
        return all(_python_writes_the_same(item) for item in value)
    if isinstance(value, dict):
        return all(
            all(ord(char) <= 0xFFFF for char in key) and _python_writes_the_same(item)
            for key, item in value.items()
        )
    return True


def check_identity_cross(gate: Gate) -> None:
    import json

    folder = VECTORS / "identity"
    output_file, marker_file = folder / "json_output.json", folder / "markers.json"
    if output_file.is_file():
        problems: list[str] = []
        compared = 0
        for case in digest.read_json_file(output_file).get("cases", []):
            value = digest.parse_ijson(case["value_json"])
            if not _python_writes_the_same(value):
                continue
            compared += 1
            python = json.dumps(value, sort_keys=True, indent=1, ensure_ascii=False)
            if python != case["bytes_utf8"]:
                problems.append(f"{case['name']}: json.dumps writes {python!r}")
        if not compared:
            problems.append("no case is one that json.dumps writes the same")
        gate.result("i", f"{rel(output_file)}: {compared} cases agree with json.dumps", problems)
    else:
        gate.report("skip", "i", f"{rel(output_file)}: missing")
    if marker_file.is_file():
        problems = []
        cases = digest.read_json_file(marker_file).get("cases", [])
        for case in cases:
            try:
                got = digest.load_yaml(case["json"], case["name"])
            except digest.RefusedInput as error:
                if not case["refuse"] or error.code != "reserved_marker":
                    problems.append(f"{case['name']}: the YAML reader refuses it: {error}")
                continue
            if case["refuse"]:
                problems.append(f"{case['name']}: the YAML reader accepts it as {got!r}")
            elif digest.canon(got) != digest.canon(digest.parse_ijson(case["json"])):
                problems.append(f"{case['name']}: the YAML reader reads {got!r}")
        gate.result("i", f"{rel(marker_file)}: {len(cases)} cases agree in YAML", problems)
    else:
        gate.report("skip", "i", f"{rel(marker_file)}: missing")


def _routes_for(case_dir: Path, saved: str) -> list[str] | None:
    """The --routes files of the case step that saved `saved`, or None if no step did."""
    try:
        case = digest.read_yaml_file(case_dir / "case.yaml")
    except (digest.RefusedInput, OSError):
        return None
    steps = case.get("steps") if isinstance(case, dict) else None
    for step in steps if isinstance(steps, list) else []:
        if not isinstance(step, dict) or step.get("save") != saved:
            continue
        argv = [str(arg) for arg in step.get("argv") or []]
        return [argv[i + 1] for i, arg in enumerate(argv[:-1]) if arg == "--routes"]
    return None


def check_expected_graphs(gate: Gate) -> None:
    found = 0
    cases = sorted(p for p in CONFORMANCE.iterdir() if p.is_dir()) if CONFORMANCE.is_dir() else []
    for case_dir in cases:
        expected = case_dir / "expected"
        if not expected.is_dir() or not (case_dir / "in").is_dir():
            continue
        for path in sorted(expected.glob("*.json")):
            try:
                document = digest.read_json_file(path)
            except digest.RefusedInput as error:
                gate.result("h", rel(path), [str(error)])
                continue
            if not isinstance(document, dict) or document.get("kind") != "fx-graph-v1":
                continue
            found += 1
            routes = _routes_for(case_dir, path.name)
            if routes is None:
                gate.result("h", rel(path), ["no step in case.yaml saves it"])
                continue
            project = case_dir / "in"
            command = [sys.executable, str(TOOLS / "digest.py"), "--check-graph", str(path)]
            command += ["--project", str(project)]
            for route_file in routes:
                command += ["--routes", str(project / route_file)]
            if DEFAULT_ROUTES.is_file():
                command += ["--builtin-routes", str(DEFAULT_ROUTES)]
            done = subprocess.run(command, capture_output=True, text=True, timeout=120)
            output = (done.stdout + done.stderr).strip().splitlines()
            problems = [] if done.returncode == 0 else output
            summary = next((line for line in output if line.startswith("ok:")), "")
            gate.result("h", f"{rel(path)} {summary}".rstrip(), problems)
    if not found:
        gate.report("skip", "h", "no expected fx-graph-v1 outputs under conformance/")


# ---------------------------------------------------------------------------------------------
# (j) the built-in route table
# ---------------------------------------------------------------------------------------------

# A capability section of spec/capabilities.md: `## <n>. `<capability>``.
_CAPABILITY_HEADING = re.compile(r"^## \d+\. `([a-z]+(?:\.[a-z_]+)+)`\s*$")
# A row of a feature table: `| `<feature>` | <meaning> |`.
_FEATURE_ROW = re.compile(r"^\| `([a-z0-9_]+)` \|")
_GUIDE_PRICE = re.compile(r"\$([0-9]+(?:\.[0-9]+)?) – \$([0-9]+(?:\.[0-9]+)?)")


def _capability_features(text: str) -> dict[str, set[str]]:
    """Each capability section of capabilities.md and the features its table lists."""
    found: dict[str, set[str]] = {}
    current: str | None = None
    in_features = False
    for line in text.splitlines():
        heading = _CAPABILITY_HEADING.match(line)
        if heading:
            current, in_features = heading.group(1), False
            found[current] = set()
            continue
        if line.startswith("## "):
            current, in_features = None, False
            continue
        if current is None:
            continue
        if line.startswith("| Feature | Meaning |"):
            in_features = True
            continue
        if in_features:
            row = _FEATURE_ROW.match(line)
            if row:
                found[current].add(row.group(1))
            elif not line.startswith("|"):
                in_features = False
    return found


def _std_capabilities() -> set[str]:
    catalog = digest.read_json_file(STD_CATALOG)
    types = catalog.get("types", []) if isinstance(catalog, dict) else []
    return {t["capability"] for t in types if isinstance(t, dict) and t.get("capability")}


def _guide_rows(text: str) -> list[str]:
    """The rows of docs/guide/05-running.md's table of built-in routes."""
    rows: list[str] = []
    inside = False
    for line in text.splitlines():
        if line.startswith("| Capability | Built-in routes |"):
            inside = True
            continue
        if inside:
            if not line.startswith("|"):
                break
            if not line.startswith("|---"):
                rows.append(line)
    return rows


def _section(text: str, title: str) -> str:
    """The `## <n>. <title>` section of a spec file, up to the next `## ` heading."""
    match = re.search(rf"^## \d+\. {re.escape(title)}[ \t]*$", text, re.M)
    if not match:
        return ""
    end = text.find("\n## ", match.end())
    return text[match.start() : end if end >= 0 else len(text)]


# A row of spec/providers.md's table of built-in adapters: provider, capability, adapter, the
# behaviour as JSON in backticks, and a link to the adapter's reference.
_ADAPTER_ROW = re.compile(
    r"^\| `([a-z0-9-]+)` \| `([a-z]+(?:\.[a-z_]+)+)` \| `([A-Za-z0-9_.-]+)` \| (`[^`]+`) \| "
    r"\[[^\]]+\]\(([^)\s]+)\) \|$"
)
# A row of an adapter reference's own table: capabilities, adapter, behaviour.
_REFERENCE_ROW = re.compile(
    r"^\| ((?:`[a-z]+(?:\.[a-z_]+)+`(?:, )?)+) \| `([A-Za-z0-9_.-]+)` \| (`[^`]+`) \|$"
)


def _builtin_adapters(text: str) -> dict[tuple[str, str], tuple[str, str, str]]:
    """spec/providers.md section 2: (capability, provider) -> (adapter, behaviour, reference),
    the behaviour as its cell writes it (`"1"` or `1`)."""
    rows: dict[tuple[str, str], tuple[str, str, str]] = {}
    for line in _section(text, "The built-in adapters").splitlines():
        row = _ADAPTER_ROW.match(line)
        if row:
            provider, capability, adapter, behavior, reference = row.groups()
            rows[(capability, provider)] = (adapter, behavior, reference)
    return rows


def _behavior_problem(contract: dict[str, Any], cell: str) -> str | None:
    """Why a contract's `adapter_behavior` is not the one a table cell writes, or None."""
    try:
        want = digest.canon(digest.parse_ijson(cell.strip("`")))
    except digest.RefusedInput:
        return f"the table's behaviour {cell} is not JSON"
    if "adapter_behavior" not in contract:
        return f"carries no behaviour, the table {cell}"
    have = digest.canon(contract["adapter_behavior"])
    if have != want:
        return f"carries behaviour {have.decode('utf-8')}, the table {cell}"
    return None


def _check_adapter_references(rows: dict[tuple[str, str], tuple[str, str, str]]) -> list[str]:
    """Every reference the table links exists, and its own table lists the same contracts."""
    problems: list[str] = []
    expected: dict[str, set[tuple[str, str, str]]] = {}
    for (capability, _), (adapter, behavior, reference) in rows.items():
        expected.setdefault(reference, set()).add((capability, adapter, behavior))
    for reference, want in sorted(expected.items()):
        path = (SPEC / reference).resolve()
        if not path.is_file():
            problems.append(f"{reference}: no such file")
            continue
        have: set[tuple[str, str, str]] = set()
        for line in path.read_text(encoding="utf-8").splitlines():
            row = _REFERENCE_ROW.match(line)
            if row:
                cells, adapter, behavior = row.groups()
                for capability in re.findall(r"`([^`]+)`", cells):
                    have.add((capability, adapter, behavior))
        if have != want:
            extra, lacking = sorted(have - want), sorted(want - have)
            problems.append(f"{rel(path)}: its table lists {extra} and lacks {lacking}")
    return problems


def _check_digest_adapters(rows: dict[tuple[str, str], tuple[str, str, str]]) -> list[str]:
    """digest.py's copy of the built-in adapters names each row's adapter and behaviour."""
    problems: list[str] = []
    ours = digest.BUILT_IN_ADAPTERS
    for key in sorted(set(rows) | set(ours)):
        capability, provider = key
        if key not in ours:
            problems.append(f"{capability} on {provider}: not in digest.py's table")
            continue
        if key not in rows:
            problems.append(f"{capability} on {provider}: in digest.py's table only")
            continue
        adapter, behavior = ours[key]
        contract = {"adapter": adapter, "adapter_behavior": behavior}
        if adapter != rows[key][0]:
            problems.append(f"{capability} on {provider}: digest.py names {adapter}")
        problem = _behavior_problem(contract, rows[key][1])
        if problem:
            problems.append(f"{capability} on {provider}: digest.py {problem}")
    return problems


def _same_amount(text: str, value: Any) -> bool:
    from decimal import Decimal

    return Decimal(text) == Decimal(str(value))


def check_default_routes(gate: Gate, schemas: dict[str, Any]) -> None:
    if not DEFAULT_ROUTES.is_file():
        gate.report("skip", "j", f"{rel(DEFAULT_ROUTES)}: missing")
        return
    subject = rel(DEFAULT_ROUTES)
    try:
        table = digest.read_yaml_file(DEFAULT_ROUTES)
    except digest.RefusedInput as error:
        gate.result("j", subject, [str(error).replace(str(DEFAULT_ROUTES), subject)])
        return
    if "fx-routes-v1" in schemas:
        problems = _schema_errors(_validator(schemas, "fx-routes-v1"), table)
        gate.result("j", f"{subject} (fx-routes-v1)", problems)
        if problems:
            return
    routes = [r for r in table.get("routes", []) if isinstance(r, dict)]
    capabilities_md = (SPEC / "capabilities.md").read_text(encoding="utf-8")
    providers_md = (SPEC / "providers.md").read_text(encoding="utf-8")
    sections = _capability_features(capabilities_md)
    std = _std_capabilities() | {"agent.turn"}
    problems = []
    used = sorted({r["capability"] for r in routes})
    stated = f"It holds {len(routes)} routes over {len(used)} capabilities."
    if stated not in _section(providers_md, "The built-in route table"):
        problems.append(f"spec/providers.md section 3 does not say {stated!r}")
    adapters = _builtin_adapters(providers_md)
    if not adapters:
        problems.append("spec/providers.md section 2 has no table of built-in adapters")
    for route in routes:
        capability, name = route["capability"], route["route"]
        where = f"{capability} {name}"
        if capability not in sections:
            problems.append(f"{where}: spec/capabilities.md has no section for {capability}")
        else:
            unknown = sorted(set(route.get("features", [])) - sections[capability])
            if unknown:
                problems.append(f"{where}: features {unknown} are not in its feature table")
        if capability not in std:
            problems.append(f"{where}: no paid built-in of the standard library calls it")
        contract = route.get("contract") or {}
        row = adapters.get((capability, name.rsplit("@", 1)[-1]))
        if row is None:
            problems.append(f"{where}: spec/providers.md section 2 has no adapter for it")
            continue
        if contract.get("adapter") != row[0]:
            problems.append(f"{where}: its contract's adapter is not {row[0]}")
        behavior = _behavior_problem(contract, row[1])
        if behavior:
            problems.append(f"{where}: {behavior}")
    gate.result(
        "j",
        f"{subject}: {len(routes)} routes agree with spec/capabilities.md, the "
        "standard library and spec/providers.md",
        problems,
    )
    gate.result(
        "j",
        f"spec/providers.md section 2: {len(adapters)} built-in adapters and their references",
        _check_adapter_references(adapters),
    )
    gate.result(
        "j",
        "tools/digest.py: its table of built-in adapters is spec/providers.md section 2",
        _check_digest_adapters(adapters),
    )
    guide = GUIDE / "05-running.md"
    rows = _guide_rows(guide.read_text(encoding="utf-8")) if guide.is_file() else []
    problems = []
    if not rows:
        problems.append("no table of built-in routes")
    for route in routes:
        capability, name = route["capability"], route["route"]
        row = next((r for r in rows if f"`{capability}`" in r.split("|")[1]), None)
        if row is None:
            problems.append(f"{capability}: no row")
            continue
        at = row.find(f"`{name}`")
        if at < 0:
            problems.append(f"{capability} {name}: not in its row")
            continue
        price = _GUIDE_PRICE.search(row, at)
        low, high = route["price"].get("low_usd"), route["price"].get("high_usd")
        if price is None or not (
            _same_amount(price.group(1), low) and _same_amount(price.group(2), high)
        ):
            shown = price.group(0) if price else "no price"
            problems.append(
                f"{capability} {name}: the guide says {shown}, the table {low} – {high}"
            )
    gate.result("j", f"{rel(guide)}: the built-in routes and their prices", problems)


def check_feature_vocabulary(gate: Gate) -> None:
    subject = rel(FEATURES)
    if not FEATURES.is_file():
        gate.result("k", subject, ["missing"])
        return
    try:
        table = digest.read_json_file(FEATURES)
    except digest.RefusedInput as error:
        gate.result("k", subject, [str(error).replace(str(FEATURES), subject)])
        return
    problems: list[str] = []
    if not isinstance(table, dict) or set(table) != {"kind", "capabilities", "renamed"}:
        gate.result("k", subject, ["not {kind, capabilities, renamed}"])
        return
    if table["kind"] != "fx-features-v1":
        problems.append(f"kind is {table['kind']!r}, not 'fx-features-v1'")
    capabilities = table["capabilities"]
    renamed = table["renamed"]
    if not isinstance(capabilities, dict) or not all(
        isinstance(features, list) and all(isinstance(f, str) for f in features)
        for features in capabilities.values()
    ):
        gate.result("k", subject, ["capabilities is not {capability: [feature, ...]}"])
        return
    if not isinstance(renamed, dict) or not all(isinstance(v, str) for v in renamed.values()):
        gate.result("k", subject, ["renamed is not {name: feature}"])
        return
    sections = _capability_features((SPEC / "capabilities.md").read_text(encoding="utf-8"))
    for capability in sorted(set(sections) - set(capabilities)):
        problems.append(f"{capability}: spec/capabilities.md defines it, the vocabulary lacks it")
    for capability in sorted(set(capabilities) - set(sections)):
        problems.append(f"{capability}: spec/capabilities.md has no section for it")
    for capability in sorted(set(sections) & set(capabilities)):
        listed = capabilities[capability]
        if len(set(listed)) != len(listed):
            problems.append(f"{capability}: a feature is listed twice")
        if set(listed) != sections[capability]:
            extra = sorted(set(listed) - sections[capability])
            lacking = sorted(sections[capability] - set(listed))
            problems.append(
                f"{capability}: not its feature table (extra {extra}, missing {lacking})"
            )
    every = {feature for features in sections.values() for feature in features}
    changes = _section((SPEC / "providers.md").read_text(encoding="utf-8"), "Renamed features")
    for old, new in sorted(renamed.items()):
        if f"`{old}`" not in changes:
            problems.append(f"renamed {old}: spec/providers.md section 4 does not name it")
        if old in every:
            problems.append(f"renamed {old}: it is still a feature in spec/capabilities.md")
        if new not in every:
            problems.append(f"renamed {old}: {new} is no feature in spec/capabilities.md")
    gate.result(
        "k",
        f"{subject}: {len(capabilities)} capabilities and {len(renamed)} renamed features agree "
        "with spec/capabilities.md and spec/providers.md section 4",
        problems,
    )


# ---------------------------------------------------------------------------------------------
# (l) references to the sections of spec files
# ---------------------------------------------------------------------------------------------

# A numbered heading of a spec file: `## 3. Outcomes`, `### 3.1 The outcomes of one send`.
_NUMBERED_HEADING = re.compile(r"^#{2,6} (\d+(?:\.\d+)*)\.?[ \t]", re.M)
# What a reference may put between a file's name and its section: the rest of a markdown link,
# closing backticks, and spaces, perhaps across a line break and a comment's marker.
_SEPARATOR = r"(?:\]\([^)\s]*\)|`{1,2})?(?:[ \t]+|[ \t]*\n[ \t]*(?:(?://[/!]?|#|\*)[ \t]*)?)"
_NUMBER = r"\d+(?:\.\d+)*"
# The text files a reference may sit in.
_REFERENCE_SUFFIXES = {
    ".md",
    ".rs",
    ".py",
    ".ts",
    ".tsx",
    ".txt",
    ".yaml",
    ".yml",
    ".json",
    ".toml",
}


def _reference_pattern(names: list[str]) -> re.Pattern[str]:
    """`<name> §N[, §M | and §M | –N]` or `<name> section(s) N[, M | and M]`; the name alone or
    under a folder named spec."""
    name = "|".join(re.escape(n) for n in names)
    sections = rf"§(?P<first>{_NUMBER})(?P<more>(?:(?:,| and| or)[ \t]+§{_NUMBER}|–§?{_NUMBER})*)"
    words = rf"sections?[ \t]+(?P<word>{_NUMBER})(?P<words>(?:(?:,| and| or)[ \t]+{_NUMBER})*)"
    return re.compile(
        rf"(?<![\w.-])(?P<folder>(?:[\w.-]+/)*)(?P<name>{name}){_SEPARATOR}(?:{sections}|{words})"
    )


def _text_files() -> list[Path] | None:
    """The repository's text files that git tracks or would add; None without git."""
    try:
        done = subprocess.run(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
            cwd=REPO,
            capture_output=True,
            check=True,
            timeout=60,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    files = [REPO / name for name in done.stdout.decode("utf-8").split("\0") if name]
    return [path for path in files if path.suffix in _REFERENCE_SUFFIXES and path.is_file()]


def check_section_references(gate: Gate) -> None:
    headings = {
        path.name: set(_NUMBERED_HEADING.findall(path.read_text(encoding="utf-8")))
        for path in sorted(SPEC.glob("*.md"))
        if path.name != "README.md"
    }
    files = _text_files()
    if files is None:
        gate.report("skip", "l", "git cannot list the repository's files")
        return
    pattern = _reference_pattern(sorted(headings))
    problems: list[str] = []
    found = 0
    for path in files:
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for match in pattern.finditer(text):
            folder = match.group("folder")
            if folder and not folder.endswith("spec/"):
                continue
            name = match.group("name")
            if match.group("first") is not None:
                numbers = [match.group("first"), *re.findall(_NUMBER, match.group("more"))]
            else:
                numbers = [match.group("word"), *re.findall(_NUMBER, match.group("words"))]
            found += len(numbers)
            line = text.count("\n", 0, match.start("name")) + 1
            for number in numbers:
                if number not in headings[name]:
                    problems.append(f"{rel(path)}:{line}: {name} has no section {number}")
    gate.result(
        "l", f"{found} references to sections of spec files name headings that exist", problems
    )


def main() -> int:
    gate = Gate()
    schemas = load_schemas(gate)
    check_documents(gate, schemas)
    check_jcs(gate)
    check_yaml_vectors(gate)
    check_identity_examples(gate)
    check_names(gate)
    check_worked_examples(gate)
    check_expected_graphs(gate)
    check_identity_cross(gate)
    check_default_routes(gate, schemas)
    check_feature_vocabulary(gate)
    check_section_references(gate)
    if gate.failures:
        print(f"{gate.failures} checks failed")
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
