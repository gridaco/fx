"""Compare grida-fx with the Python engine FX grew out of (overview.md, milestone 1 step 2 gate).

With digests removed, the expanded graphs and prices of grida-fx must match the Python engine's
output exactly. This script runs both and diffs them. It uses the standard library and PyYAML only
(through conformance/run.py, whose case format, minimal environment and process handling it
shares) and never spends: both engines plan offline.

    FX_GNODE_REPO=/path/to/stage-gen uv run --project python python tools/compare_gnode.py \\
        --command target/debug/grida-fx [case ...]
    uv run --project python python tools/compare_gnode.py --command target/debug/grida-fx \\
        --pair <gnode project dir> <fx project dir> --target <workflow> [--routes r.yaml] \\
        [--inputs i.yaml]

Case mode (default: every case of FX's suite; else the cases named): for each step of the FX
case's ``case.yaml`` whose verb is ``expand`` or ``price``, copy the ORIGINAL case project from
``$FX_GNODE_REPO/tests/conformance/<case>/in`` to a fresh temporary folder and run, there,
``uv run --project $FX_GNODE_REPO --no-sync gnode <verb> ...`` with ``GNODE_PLUGINS=std``
(``--no-sync``: the gnode checkout's environment is used as it is and never written); copy FX's
``conformance/<case>/in`` to another and run ``<command> <verb> ...`` there. Both commands get the
conformance suite's minimal environment (``PATH``, a fresh ``HOME``, ``NO_COLOR``, ``LANG``,
``PYTHONDONTWRITEBYTECODE``); FX also gets ``GRIDA_FX_PYTHON`` and the Rust toolchain homes, as
the suite passes them through. The ``files`` of that step and of every step before it are written
into both copies first. For gnode, FX file names are mapped back (``fx.yaml`` → ``gnode.yaml``,
``fx.lock`` → ``gnode.lock``), in those files and in the verb's arguments, and so are the renames
the FX suite made to gnode's cases: a document's ``fx:`` key, ``fx/`` in ``uses``, and
``from grida.fx import``. What other earlier steps did (a lock written, a run) is not reproduced.
Exit statuses must agree; both stdouts are parsed and normalised, then compared.

A case that gnode's suite lacks (FX wrote it after the move) is PORTED: gnode runs in a copy of
FX's own ``in/`` with the same mapping applied to every file's name and text, and its lines say
``ported``. A case listed in ``NOT_PORTED`` is skipped with the reason given there.

Pair mode runs ``expand`` and ``price`` with the same ``--target``, ``--routes`` and ``--inputs``
(paths inside each project) in copies of two projects, without known differences. When a pair
differs, the FX decisions that no case exercises (``UNCASED_DECISIONS``) are printed after it, so
a difference can be checked against them before it is reported.

Normalisation (both sides):
- drop ``plan``, ``graph_sha256``, every ``fingerprint`` and the ``types`` map; an instance's
  ``identity`` becomes ``"<digest>"`` (null stays null, so whether an identity is known is still
  compared); a pending value inside ``with`` (``{"pending": <token>}``, or
  ``{"pending": [<instance id>, ...]}``) becomes ``{"pending": "*"}``;
- FX → gnode shape: ``kind: fx-graph-v1`` → ``gnode: graph/v2``; ``workflow`` object → its
  ``id``; instance ``routes`` ``{cap: {route, fingerprint}}`` → ``{cap: route}``; drop the
  instance fields gnode lacks (``type``, ``with``);
- names (gnode side only, so FX can never hide a gnode name it prints): ``gnode/`` → ``fx/`` in
  ``uses``, problems and reasons, ``gnode.yaml`` → ``fx.yaml``, ``gnode.lock`` → ``fx.lock``,
  ``gnode lock`` / ``see gnode nodes`` / ``gnode takes mv`` → ``grida-fx …``,
  ``x-gnode-`` → ``x-fx-``, a document key ``gnode: <doc>/v1`` → ``fx: <doc>/v1``;
- numbers compared as numbers (``1`` == ``1.0``; money, a ``*_usd`` member, within 1e-9; a
  boolean is never a number);
- problems compared as a list of ``"where: message"`` strings, after the name mapping.

A difference is reported as ``<path>: <gnode> != <fx>``. Paths start at ``$`` (the document);
a graph's instances are addressed by id (``$.instances[draw#1].state``), other list items by
index, and the exit status is the path ``status``. Known, recorded differences (decisions FX took
on purpose) are listed in ``KNOWN_DIFFERENCES`` with the case, the verb and a path; a difference
at that path or under it is reported as ``known`` instead of failing. The path ``$`` is the
document as a whole, printed by one engine only: it covers that difference and nothing under it.

Output: one line per case and verb (``<case> <verb>``, with ``(step <n>)`` when a case has two
steps of that verb and ``(ported)`` for a ported case): ``same``, ``known (<n>)``, ``DIFFERS``
(followed by the differences that are not known and a unified diff of the normalised JSON) or
``SKIP <reason>`` (gnode missing, a case FX's suite lacks or one not ported, no step to compare).
A known difference that no comparison found any more is noted. Exit 1 when any comparison
differs.
"""

from __future__ import annotations

import argparse
import contextlib
import difflib
import importlib.util
import json
import os
import re
import shutil
import sys
import tempfile
from collections.abc import Iterator
from pathlib import Path, PurePosixPath
from typing import Any

REPO = Path(__file__).resolve().parents[1]
CONFORMANCE = REPO / "conformance"
GNODE_REPO_ENV = "FX_GNODE_REPO"
VERBS = ("expand", "price")
TIMEOUT_S = 120.0
MONEY_TOLERANCE = 1e-9
#: The path of an exit status difference; ``$`` is the root of the printed document.
STATUS = "status"
ROOT = "$"
#: What a digest and a pending value become.
DIGEST = "<digest>"
PENDING = "*"
SHOWN_VALUE = 200
SHOWN_STDERR = 2000
#: Folders a copied project leaves out: nothing an engine reads to plan.
COPY_IGNORED = ("__pycache__", ".git", ".venv", "node_modules", "target")
#: FX file names, by the gnode name they were renamed from.
GNODE_FILE_NAMES = {"fx.yaml": "gnode.yaml", "fx.lock": "gnode.lock"}


def _load_conformance() -> Any:
    """conformance/run.py, the suite's runner: its case format and environment are reused."""
    spec = importlib.util.spec_from_file_location("fx_conformance_run", CONFORMANCE / "run.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


conformance = _load_conformance()

#: (case, verb, JSON path) of differences FX made on purpose, in the cases that show them. FX's
#: decisions that no case exercises yet are in UNCASED_DECISIONS below.
KNOWN_DIFFERENCES: list[tuple[str, str, str]] = [
    # Until the runner lands, an at: plan step is a planning problem instead of running: exit 1.
    ("at-plan", "expand", STATUS),
    # The at: plan step stays planned, and the repeat over its output never expands.
    ("at-plan", "expand", "$.instances"),
    # That repeat waits on a list no run made yet, so it is pending.
    ("at-plan", "expand", "$.pending"),
    # The estimate holds the pending repeat instead of the expanded instances.
    ("at-plan", "expand", "$.estimate"),
    # The at: plan step's problem, and any that the unexpanded repeat brings.
    ("at-plan", "expand", "$.problems"),
    # The price of that plan exits 1 for the same problem.
    ("at-plan", "price", STATUS),
    # Its phases price the pending repeat instead of the expanded instances.
    ("at-plan", "price", "$.phases"),
    # So does its estimate.
    ("at-plan", "price", "$.estimate"),
    # (ported) Lock drift is a problem on the step's declaration path (`b`), not on `uses`.
    ("lock-drift", "expand", "$.problems"),
    # (ported) A declared resource that is missing is a problem on the step's declaration path
    # (`lost`), and the step is absent; gnode stops with a traceback and prints no graph.
    ("resource-missing", "expand", ROOT),
]

#: FX decisions that no case exercises yet: a comparison cannot mark them known, so pair mode
#: prints them after a difference. Once a case shows one, record it in KNOWN_DIFFERENCES.
UNCASED_DECISIONS: tuple[str, ...] = (
    "a failed or pending part of a mixed template makes the whole string failed or pending, "
    'and a missing part renders as ""',
    "a node reading itself through with:, or a with: cycle, is refused 'refers back to itself', "
    "and so is a recursive let",
    "select is keyed on fx/select@1 only",
    "the expression lexer takes ASCII digits and whitespace only",
    "an at: plan step of a paid type is refused",
    "a syntax error in a prompt file is a problem on <step>.with.<param> that names the file; "
    "the instance stays planned and priced (gnode: a problem on <step>, and no instance)",
    "an assertion message that evaluates to something other than text renders by FX's text "
    'rules: ["a","b"], true, and "" for null (gnode: Python\'s str())',
    "a negative call bound (the param a type's calls: names) bills the param's maximum",
    "a negative duration or max_chars is no length: the call is priced as of unknown length",
    "join, contains, min/max, digest, == and text over a list or object that holds a pending "
    "value are pending, so a pending token never ends up inside a known value",
)
#: Where the spec lists its own changes (one number type, booleans not numbers, strict YAML, ...).
SPEC_CHANGES = "and the changes spec/identity.md section 13 lists"

#: Cases of FX's suite that are not ported, with why.
NOT_PORTED: dict[str, str] = {
    # Every step refuses, or not, by FX's own strict YAML reader (yaml.md), where gnode reads
    # YAML 1.1: the statuses differ by decision, and the case's own expected statuses pin FX.
    "yaml-strict": "FX's strict YAML subset is its own (yaml.md); gnode reads YAML 1.1",
}

_ABSENT = object()
_NOT_JSON = object()

# gnode's names in texts it prints, with FX's. A name is matched where it starts a word, so a
# path such as src/gnode/... is left alone.
_START = r"(?<![\w/.-])"
_GNODE_NAMES: list[tuple[re.Pattern[str], str]] = [
    (re.compile(_START + r"gnode (?=lock\b|nodes\b|takes mv\b)"), "grida-fx "),
    (re.compile(_START + r"gnode\.yaml\b"), "fx.yaml"),
    (re.compile(_START + r"gnode\.lock\b"), "fx.lock"),
    (re.compile(r"\bx-gnode-"), "x-fx-"),
    (re.compile(_START + r"gnode: (?=[a-z]+/v\d)"), "fx: "),
    (re.compile(_START + r"gnode/"), "fx/"),
]


# --------------------------------------------------------------------------------- running


def run_gnode(repo: Path, case_in: Path, argv: list[str]) -> tuple[int, str]:
    """Runs gnode in a temporary copy of ``case_in``; returns (status, stdout)."""
    status, stdout, _ = _run_gnode(repo, case_in, argv)
    return status, stdout


def run_fx(command: list[str], case_in: Path, argv: list[str]) -> tuple[int, str]:
    """Runs grida-fx in a temporary copy of ``case_in``; returns (status, stdout)."""
    status, stdout, _ = _run_fx(command, case_in, argv)
    return status, stdout


def _gnode_command(repo: Path) -> list[str]:
    """gnode from its checkout's own environment, which ``--no-sync`` never writes."""
    return [shutil.which("uv") or "uv", "run", "--project", str(repo), "--no-sync", "gnode"]


def _run_gnode(repo: Path, case_in: Path, argv: list[str]) -> tuple[int, str, str]:
    return _run_in_copy(_gnode_command(repo), case_in, argv, {"GNODE_PLUGINS": "std"}, "gnode")


def _run_fx(command: list[str], case_in: Path, argv: list[str]) -> tuple[int, str, str]:
    return _run_in_copy(command, case_in, argv, conformance.passed_through(), "grida-fx")


def _run_in_copy(
    program: list[str], case_in: Path, argv: list[str], passed: dict[str, str], label: str
) -> tuple[int, str, str]:
    """Runs ``program argv`` in a fresh copy of ``case_in``, with a fresh HOME and nothing of
    the caller's environment but PATH and ``passed``; returns (status, stdout, stderr)."""
    with tempfile.TemporaryDirectory(prefix="fx-compare-gnode-") as work:
        project = Path(work) / "project"
        home = Path(work) / "home"
        home.mkdir()
        shutil.copytree(case_in, project, ignore=shutil.ignore_patterns(*COPY_IGNORED))
        env = conformance.step_environment(home, passed)
        status, stdout, stderr = conformance.run_command(
            [*program, *argv], project, env, None, TIMEOUT_S, f"{label} {' '.join(argv)}"
        )
    return (
        status,
        stdout.decode("utf-8", errors="replace"),
        stderr.decode("utf-8", errors="replace"),
    )


def _gnode_unavailable(repo: Path) -> str | None:
    """Why gnode cannot be run from ``repo``, or None."""
    if not repo.is_dir():
        return f"{GNODE_REPO_ENV} names no folder"
    if shutil.which("uv") is None:
        return "no uv on PATH to run gnode with"
    with tempfile.TemporaryDirectory(prefix="fx-compare-gnode-probe-") as work:
        try:
            status, _, stderr = conformance.run_command(
                [*_gnode_command(repo), "--help"],
                Path(work),
                conformance.step_environment(Path(work), {"GNODE_PLUGINS": "std"}),
                None,
                TIMEOUT_S,
                "gnode --help",
            )
        except conformance.CaseFailure as error:
            return str(error).splitlines()[0]
    if status != 0:
        reason = stderr.decode("utf-8", errors="replace").strip().splitlines()
        return f"gnode does not run from {repo.name}: {reason[-1] if reason else status}"
    return None


@contextlib.contextmanager
def _with_files(case_in: Path, files: dict[str, str]) -> Iterator[Path]:
    """``case_in``, or a copy of it holding ``files`` (relative path to UTF-8 text), at
    ``<temporary>/<case>/in`` like the original."""
    if not files:
        yield case_in
        return
    with tempfile.TemporaryDirectory(prefix="fx-compare-gnode-files-") as work:
        staged = Path(work) / case_in.parent.name / case_in.name
        shutil.copytree(case_in, staged, ignore=shutil.ignore_patterns(*COPY_IGNORED))
        for relative, text in files.items():
            target = conformance.project_path(staged, relative, "files")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(text.encode("utf-8"))
        yield staged


@contextlib.contextmanager
def _ported(case_in: Path) -> Iterator[Path]:
    """A copy of an FX case's project as gnode reads it, at ``<temporary>/<case>/in``: the FX
    suite's renames undone in every file's name and in the text of every file ``gnode_files``
    rewrites (YAML documents, Python modules, the lock). Other files, and one that is not UTF-8
    (a case may hold such a file on purpose), keep their bytes; only their name is mapped."""
    with tempfile.TemporaryDirectory(prefix="fx-compare-gnode-ported-") as work:
        staged = Path(work) / case_in.parent.name / "in"
        shutil.copytree(case_in, staged, ignore=shutil.ignore_patterns(*COPY_IGNORED))
        texts: dict[str, str] = {}
        for path in sorted(staged.rglob("*")):
            relative = path.relative_to(staged).as_posix()
            if not (path.is_file() and _names_fx(relative)):
                continue
            try:
                texts[relative] = path.read_bytes().decode("utf-8")
            except UnicodeDecodeError:
                path.rename(conformance.project_path(staged, gnode_path(relative), "ported"))
                continue
            path.unlink()
        for relative, text in gnode_files(texts).items():
            target = conformance.project_path(staged, relative, "ported")
            target.write_bytes(text.encode("utf-8"))
        yield staged


def _names_fx(relative: str) -> bool:
    """Whether ``gnode_files`` maps this file's name or text: what a port rewrites."""
    pure = PurePosixPath(relative)
    return pure.name in GNODE_FILE_NAMES or pure.suffix in (".yaml", ".yml", ".py")


# ------------------------------------------------------------------------- gnode's names


def gnode_path(text: str) -> str:
    """A path or argument with an FX file name mapped back to gnode's (``fx.yaml``,
    ``fx.lock``); anything else unchanged."""
    pure = PurePosixPath(text)
    renamed = GNODE_FILE_NAMES.get(pure.name)
    if renamed is None or not text.endswith(pure.name):
        return text
    return text[: len(text) - len(pure.name)] + renamed


def gnode_files(files: dict[str, str]) -> dict[str, str]:
    """FX case files as gnode reads them: the FX suite's renames undone, names and texts."""
    mapped: dict[str, str] = {}
    for relative, text in files.items():
        path = gnode_path(relative)
        mapped[path] = _gnode_text(path, text)
    return mapped


def _gnode_text(path: str, text: str) -> str:
    pure = PurePosixPath(path)
    if pure.name == "gnode.lock":
        # gnode's lock is the nodes map alone, under no document key.
        return re.sub(r"(?m)^fx:[ \t]+lock/v1[ \t]*(?:\n|$)", "", text)
    if pure.suffix in (".yaml", ".yml"):
        text = re.sub(r"(?m)^fx:(?=[ \t])", "gnode:", text)
        return re.sub(r"(?<![\w/.-])fx/(?=[A-Za-z_][\w.]*@)", "gnode/", text)
    if pure.suffix == ".py":
        return re.sub(r"(?m)^from grida\.fx import\b", "from gnode import", text)
    return text


def fx_names(text: str) -> str:
    """A text gnode printed, with FX's names for gnode's."""
    for pattern, replacement in _GNODE_NAMES:
        text = pattern.sub(replacement, text)
    return text


# --------------------------------------------------------------------------- normalising


def normalise(document: Any, side: str) -> Any:
    """The comparable form of an expand or price document; ``side`` is ``gnode`` or ``fx``."""
    if side not in ("gnode", "fx"):
        raise ValueError(f"side is gnode or fx, not {side!r}")
    if not isinstance(document, dict):
        return _scrubbed(document)
    shaped = dict(document)
    if side == "fx" and shaped.get("kind") == "fx-graph-v1":
        del shaped["kind"]
        shaped["gnode"] = "graph/v2"
    workflow = shaped.get("workflow")
    if isinstance(workflow, dict):
        shaped["workflow"] = workflow.get("id")
    for dropped in ("plan", "graph_sha256", "types"):
        shaped.pop(dropped, None)
    instances = shaped.get("instances")
    if isinstance(instances, list):
        shaped["instances"] = [_instance(item, side) for item in instances]
    problems = shaped.get("problems")
    if isinstance(problems, list):
        shaped["problems"] = [_problem(item, side) for item in problems]
    return _scrubbed(shaped)


def _instance(item: Any, side: str) -> Any:
    if not isinstance(item, dict):
        return item
    instance = dict(item)
    if side == "fx":
        instance.pop("type", None)
        instance.pop("with", None)
        routes = instance.get("routes")
        if isinstance(routes, dict):
            instance["routes"] = {
                capability: bound["route"]
                if isinstance(bound, dict) and "route" in bound
                else bound
                for capability, bound in routes.items()
            }
    if isinstance(instance.get("identity"), str):
        instance["identity"] = DIGEST
    if side == "gnode":
        for name in ("uses", "reason"):
            if isinstance(instance.get(name), str):
                instance[name] = fx_names(instance[name])
    return instance


def _problem(item: Any, side: str) -> Any:
    if not (
        isinstance(item, dict)
        and isinstance(item.get("where"), str)
        and isinstance(item.get("message"), str)
    ):
        return item
    text = f"{item['where']}: {item['message']}"
    return fx_names(text) if side == "gnode" else text


def _is_pending(value: dict[str, Any]) -> bool:
    """A pending with-value: ``{"pending": <token>}``, or ``{"pending": [<instance id>, ...]}``,
    the form that names the instances it waits on."""
    if set(value) != {"pending"}:
        return False
    waits_on = value["pending"]
    if isinstance(waits_on, str):
        return True
    return isinstance(waits_on, list) and all(isinstance(item, str) for item in waits_on)


def _scrubbed(value: Any, in_with: bool = False) -> Any:
    """Every fingerprint dropped, pending values in with-values masked, numbers as numbers.

    The values under ``with`` are data: a member there is never dropped, whatever its name."""
    if isinstance(value, dict):
        if in_with:
            if _is_pending(value):
                return {"pending": PENDING}
            return {key: _scrubbed(item, True) for key, item in value.items()}
        scrubbed: dict[str, Any] = {}
        for key, item in value.items():
            if key == "fingerprint":
                continue
            if key == "with" and isinstance(item, dict):
                scrubbed[key] = {name: _scrubbed(given, True) for name, given in item.items()}
            else:
                scrubbed[key] = _scrubbed(item)
        return scrubbed
    if isinstance(value, list):
        return [_scrubbed(item, in_with) for item in value]
    if isinstance(value, float) and value.is_integer() and abs(value) < 1e21:
        # 1 and 1.0 are one number; written the same, they diff the same.
        return int(value)
    return value


# ---------------------------------------------------------------------------- comparing


def compare(gnode: Any, fx: Any) -> list[str]:
    """The differences between two normalised documents, as ``<json path>: <gnode> != <fx>``."""
    return [line for _, line in _differences(gnode, fx)]


def _differences(gnode: Any, fx: Any) -> list[tuple[str, str]]:
    """(path, line) of each difference, in document order."""
    found: list[tuple[str, str]] = []
    _walk(gnode, fx, ROOT, None, found)
    return found


def _walk(a: Any, b: Any, path: str, key: str | None, found: list[tuple[str, str]]) -> None:
    if isinstance(a, dict) and isinstance(b, dict):
        for name in sorted(set(a) | set(b)):
            where = _member(path, name)
            if name not in b:
                _differ(found, where, a[name], _ABSENT)
            elif name not in a:
                _differ(found, where, _ABSENT, b[name])
            else:
                _walk(a[name], b[name], where, name, found)
        return
    if isinstance(a, list) and isinstance(b, list):
        by_id_a, by_id_b = _by_id(a), _by_id(b)
        if by_id_a is not None and by_id_b is not None:
            _walk_by_id(by_id_a, by_id_b, path, found)
        elif len(a) == len(b):
            for index, (x, y) in enumerate(zip(a, b, strict=True)):
                _walk(x, y, f"{path}[{index}]", None, found)
        elif all(isinstance(item, str) for item in a + b):
            _walk_texts(a, b, path, found)
        else:
            _differ(found, path, a, b)
        return
    if not _same(a, b, key):
        _differ(found, path, a, b)


def _walk_by_id(
    a: dict[str, Any], b: dict[str, Any], path: str, found: list[tuple[str, str]]
) -> None:
    """Items with ids (instances), matched by id; their order is compared once."""
    for ident, item in a.items():
        if ident not in b:
            _differ(found, f"{path}[{ident}]", item, _ABSENT)
    for ident, item in b.items():
        if ident not in a:
            _differ(found, f"{path}[{ident}]", _ABSENT, item)
    order_a = [ident for ident in a if ident in b]
    order_b = [ident for ident in b if ident in a]
    if order_a != order_b:
        found.append((path, f"{path}: order {_shown(order_a)} != {_shown(order_b)}"))
    for ident in order_a:
        _walk(a[ident], b[ident], f"{path}[{ident}]", None, found)


def _walk_texts(a: list[str], b: list[str], path: str, found: list[tuple[str, str]]) -> None:
    """Lists of texts of different lengths (problems): what each side has that the other lacks."""
    rest = list(b)
    for item in a:
        if item in rest:
            rest.remove(item)
        else:
            _differ(found, path, item, _ABSENT)
    for item in rest:
        _differ(found, path, _ABSENT, item)


def _by_id(items: list[Any]) -> dict[str, Any] | None:
    """The items by their ``id`` when every item is an object with a distinct text id."""
    keyed: dict[str, Any] = {}
    for item in items:
        if not isinstance(item, dict) or not isinstance(item.get("id"), str):
            return None
        if item["id"] in keyed:
            return None
        keyed[item["id"]] = item
    return keyed


def _same(a: Any, b: Any, key: str | None) -> bool:
    if isinstance(a, bool) or isinstance(b, bool):
        return a is b
    if isinstance(a, int | float) and isinstance(b, int | float):
        if a == b:
            return True
        money = key is not None and key.endswith("_usd")
        return money and abs(a - b) <= MONEY_TOLERANCE
    return type(a) is type(b) and a == b


def _member(path: str, name: str) -> str:
    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
        return f"{path}.{name}"
    return f"{path}[{json.dumps(name, ensure_ascii=False)}]"


def _differ(found: list[tuple[str, str]], path: str, a: Any, b: Any) -> None:
    found.append((path, f"{path}: {_shown(a)} != {_shown(b)}"))


def _shown(value: Any) -> str:
    if value is _ABSENT:
        return "(absent)"
    if value is _NOT_JSON:
        return "(no JSON)"
    text = json.dumps(value, ensure_ascii=False, sort_keys=True)
    return text if len(text) <= SHOWN_VALUE else text[: SHOWN_VALUE - 1] + "…"


def _is_known(case: str, verb: str, path: str) -> tuple[str, str, str] | None:
    for known in KNOWN_DIFFERENCES:
        known_case, known_verb, known_path = known
        if (known_case, known_verb) != (case, verb):
            continue
        if path == known_path:
            return known
        # $ is the document printed by one engine only, never everything in it.
        if known_path != ROOT and path.startswith((known_path + ".", known_path + "[")):
            return known
    return None


def _parsed(stdout: str) -> Any:
    try:
        return json.loads(stdout)
    except ValueError:
        return _NOT_JSON


def _pretty(value: Any) -> list[str]:
    return (json.dumps(value, indent=1, sort_keys=True, ensure_ascii=False) + "\n").splitlines(True)


# -------------------------------------------------------------------------------- cases


def case_steps(case_dir: Path) -> list[list[str]]:
    """The argv of each ``expand``/``price`` step of an FX case."""
    return [argv for _, argv, _ in _case_plan(case_dir)]


def _case_plan(case_dir: Path) -> list[tuple[int, list[str], dict[str, str]]]:
    """(step number, argv, files written by then) of each ``expand``/``price`` step."""
    files: dict[str, str] = {}
    plan: list[tuple[int, list[str], dict[str, str]]] = []
    for number, step in enumerate(conformance.load_case(case_dir), 1):
        files.update(step.get("files", {}))
        argv = step.get("argv")
        if argv and argv[0] in VERBS:
            plan.append((number, list(argv), dict(files)))
    return plan


class _Report:
    """Prints each comparison and counts the outcomes."""

    def __init__(self) -> None:
        self.counts = {"same": 0, "known": 0, "differs": 0, "skipped": 0}
        self.matched: set[tuple[str, str, str]] = set()
        self.compared: set[tuple[str, str]] = set()

    def skip(self, label: str, reason: str) -> None:
        self.counts["skipped"] += 1
        print(f"{label}: SKIP {reason}", flush=True)

    def compare(
        self,
        label: str,
        case: str | None,
        verb: str,
        gnode: tuple[int, str, str],
        fx: tuple[int, str, str],
    ) -> None:
        g_status, g_out, g_err = gnode
        f_status, f_out, f_err = fx
        g_doc, f_doc = _parsed(g_out), _parsed(f_out)
        found: list[tuple[str, str]] = []
        if g_status != f_status:
            found.append((STATUS, f"{STATUS}: {g_status} != {f_status}"))
        diff: list[str] = []
        if g_doc is not _NOT_JSON and f_doc is not _NOT_JSON:
            g_doc, f_doc = normalise(g_doc, "gnode"), normalise(f_doc, "fx")
            found.extend(_differences(g_doc, f_doc))
            diff = list(difflib.unified_diff(_pretty(g_doc), _pretty(f_doc), "gnode", "grida-fx"))
        elif (g_doc is _NOT_JSON) != (f_doc is _NOT_JSON):
            _differ(found, ROOT, g_doc, f_doc)
        unknown: list[str] = []
        known = 0
        if case is not None:
            self.compared.add((case, verb))
        for path, line in found:
            entry = _is_known(case, verb, path) if case is not None else None
            if entry is None:
                unknown.append(line)
            else:
                known += 1
                self.matched.add(entry)
        if unknown:
            self.counts["differs"] += 1
            print(f"{label}: DIFFERS", flush=True)
            for line in unknown:
                print(f"  {line}")
            for side, document, stderr in (("gnode", g_doc, g_err), ("grida-fx", f_doc, f_err)):
                if document is _NOT_JSON and stderr.strip():
                    print(f"  {side} said on stderr:")
                    for text in _clipped(stderr).splitlines():
                        print(f"    {text}")
            sys.stdout.writelines(diff)
        elif known:
            self.counts["known"] += 1
            print(f"{label}: known ({known})", flush=True)
        else:
            self.counts["same"] += 1
            print(f"{label}: same", flush=True)

    def notes(self) -> None:
        for entry in KNOWN_DIFFERENCES:
            case, verb, path = entry
            if (case, verb) in self.compared and entry not in self.matched:
                print(f"note: the known difference {case} {verb} {path} was not found")

    def decisions(self) -> None:
        """After a pair differs: the decisions to check it against, which no case marks known."""
        if not self.counts["differs"]:
            return
        print("note: pair mode marks nothing known; FX decisions that no case exercises yet:")
        for decision in (*UNCASED_DECISIONS, SPEC_CHANGES):
            print(f"  - {decision}")

    def summary(self) -> int:
        shown = ", ".join(f"{count} {name}" for name, count in self.counts.items() if count)
        print(f"compare_gnode: {shown or 'nothing compared'}")
        return 1 if self.counts["differs"] else 0


def _clipped(text: str) -> str:
    text = text.strip()
    if len(text) > SHOWN_STDERR:
        return text[:SHOWN_STDERR] + f"\n... ({len(text) - SHOWN_STDERR} more characters)"
    return text


def _run_pair(
    report: _Report,
    command: list[str],
    repo: Path,
    projects: tuple[Path, Path],
    flags: list[str],
    target: str,
) -> None:
    gnode_dir, fx_dir = projects
    for verb in VERBS:
        argv = [verb, target, *flags]
        try:
            gnode = _run_gnode(repo, gnode_dir, [gnode_path(arg) for arg in argv])
            fx = _run_fx(command, fx_dir, argv)
        except conformance.CaseFailure as error:
            _timed_out(report, f"pair {verb}", error)
            continue
        report.compare(f"pair {verb}", None, verb, gnode, fx)


def _run_cases(report: _Report, command: list[str], repo: Path, names: list[str]) -> None:
    gnode_root = repo / "tests" / "conformance"
    for name in names:
        if name in ("", ".", "..") or "/" in name or os.sep in name:
            report.skip(name, "not a case name")
            continue
        fx_case, gnode_case = CONFORMANCE / name, gnode_root / name
        if not (fx_case / "case.yaml").is_file():
            report.skip(name, "no such case in conformance/")
            continue
        ported = not (gnode_case / "in").is_dir()
        if ported and name in NOT_PORTED:
            report.skip(name, f"not ported: {NOT_PORTED[name]}")
            continue
        try:
            plan = _case_plan(fx_case)
        except conformance.CaseFailure as error:
            report.skip(name, str(error))
            continue
        if not plan:
            report.skip(name, "no expand or price step")
            continue
        verbs = [argv[0] for _, argv, _ in plan]
        for number, argv, files in plan:
            verb = argv[0]
            notes = [f"step {number}"] if verbs.count(verb) > 1 else []
            notes += ["ported"] if ported else []
            label = f"{name} {verb}" + (f" ({', '.join(notes)})" if notes else "")
            try:
                with _gnode_case_in(gnode_case, fx_case, ported) as original:
                    with _with_files(original, gnode_files(files)) as gnode_in:
                        gnode = _run_gnode(repo, gnode_in, [gnode_path(arg) for arg in argv])
                with _with_files(fx_case / "in", files) as fx_in:
                    fx = _run_fx(command, fx_in, argv)
            except conformance.CaseFailure as error:
                _timed_out(report, label, error)
                continue
            report.compare(label, name, verb, gnode, fx)


@contextlib.contextmanager
def _gnode_case_in(gnode_case: Path, fx_case: Path, ported: bool) -> Iterator[Path]:
    """The project gnode plans a case in: its own suite's original, or FX's project ported."""
    if not ported:
        yield gnode_case / "in"
        return
    with _ported(fx_case / "in") as staged:
        yield staged


def _timed_out(report: _Report, label: str, error: Exception) -> None:
    report.counts["differs"] += 1
    print(f"{label}: DIFFERS")
    for text in _clipped(str(error)).splitlines():
        print(f"  {text}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--command", required=True)
    parser.add_argument("--pair", nargs=2, metavar=("GNODE_DIR", "FX_DIR"))
    parser.add_argument("--target")
    parser.add_argument("--routes", action="append", default=[])
    parser.add_argument("--inputs", action="append", default=[])
    parser.add_argument("cases", nargs="*")
    args = parser.parse_args(argv)
    if args.pair is None and (args.target or args.routes or args.inputs):
        parser.error("--target, --routes and --inputs go with --pair")
    if args.pair is not None and args.cases:
        parser.error("--pair compares two projects, not cases")
    if args.pair is not None and not args.target:
        parser.error("--pair needs --target")
    try:
        command = conformance.resolve_command(args.command)
        conformance.passed_through()
    except SystemExit as error:
        parser.error(str(error).removeprefix("conformance: "))

    report = _Report()
    given = os.environ.get(GNODE_REPO_ENV)
    if not given:
        report.skip("compare_gnode", f"{GNODE_REPO_ENV} is not set: no gnode to compare with")
        return report.summary()
    repo = Path(given).expanduser().resolve()
    unavailable = _gnode_unavailable(repo)
    if unavailable is not None:
        report.skip("compare_gnode", unavailable)
        return report.summary()

    if args.pair is not None:
        projects = (Path(args.pair[0]).resolve(), Path(args.pair[1]).resolve())
        for project in projects:
            if not project.is_dir():
                parser.error(f"--pair: {project.name or project} is not a folder")
        flags = [flag for path in args.routes for flag in ("--routes", path)]
        flags += [flag for path in args.inputs for flag in ("--inputs", path)]
        _run_pair(report, command, repo, projects, flags, args.target)
        report.decisions()
        return report.summary()

    names = list(args.cases)
    if not names:
        names = sorted(path.parent.name for path in CONFORMANCE.glob("*/case.yaml"))
    _run_cases(report, command, repo, names)
    report.notes()
    return report.summary()


if __name__ == "__main__":
    sys.exit(main())
