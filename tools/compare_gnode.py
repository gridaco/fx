"""Compare grida-fx with the Python engine FX grew out of (overview.md, milestone 1 gates).

With digests removed, the expanded graphs, identities and prices of grida-fx (step 2), and what
its runs record and write (step 3), must match the Python engine's output exactly, except where
FX decided otherwise on purpose. This script runs both and diffs them. It uses the standard
library and PyYAML only (through conformance/run.py, whose case format, minimal environment and
process handling it shares) and never spends: both engines plan and run offline, never with
``--live``, and a paid call either replays from a store or is refused.

    FX_GNODE_REPO=/path/to/stage-gen uv run --project python python tools/compare_gnode.py \\
        --command target/debug/grida-fx [case ...]
    uv run --project python python tools/compare_gnode.py --command target/debug/grida-fx \\
        --pair <gnode project dir> <fx project dir> --target <workflow> [--routes r.yaml] \\
        [--inputs i.yaml]

Case mode (default: every case of FX's suite; else the cases named) compares a case in one of two
ways. A case none of whose steps runs (no ``run``, ``reroll``, ``pick``, ``takes``, ``project``,
``inspect`` or ``jobs`` step) is compared verb by verb, each step on its own (plan mode, below);
any other case is compared step by step, every step in one copy of its project (run mode, after
it).

Plan mode: for each step of the FX case's ``case.yaml`` whose verb is ``expand``, ``price`` or
``identity``, copy the ORIGINAL case project from
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

Run mode: each engine gets one fresh copy of the case's project (gnode's original, or FX's
ported) and runs every step of ``case.yaml`` in order in it, as the conformance runner does:
``files`` written (mapped to gnode's names for gnode), then ``argv`` run (likewise mapped), then
``read`` read. Every step that runs a command compares its exit status; every step that saves
compares what it saves (its stdout, or the file it reads): JSON (``json: true``, or the stdout of
``expand``, ``price``, ``identity`` and ``project``) after normalisation, anything else as text
line by line (``$[<line>]``), the gnode side with FX's names (below). Absolute paths into either
copy are made relative to it first, so gnode's ``run       /tmp/.../runs/one`` reads as FX's
``run       runs/one``. A step's ``status`` and ``mentions`` in ``case.yaml`` pin FX; they are
not checked here. A case that cannot run on gnode at all is listed in ``NOT_PORTED``.

A case that gnode's suite lacks (FX wrote it after the move) is PORTED: gnode runs in a copy of
FX's own ``in/`` with the same mapping applied to every file's name and text, and its lines say
``ported``. A case listed in ``NOT_PORTED`` is skipped with the reason given there.

Pair mode runs ``expand``, ``price`` and ``identity`` with the same ``--target``, ``--routes``
and ``--inputs`` (paths inside each project) in copies of two projects, without known
differences. When a pair
differs, the FX decisions that no case exercises (``UNCASED_DECISIONS``) are printed after it, so
a difference can be checked against them before it is reported. Defects FX keeps from gnode
for now, the same in both engines and so never shown by a comparison, are in
``SHARED_DEFECTS``.

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
- problems compared as a list of ``"where: message"`` strings, after the name mapping;
- ``identity`` output (instance id to identity): each identity becomes ``"<digest>"``, so the
  instance ids and whether each identity is known are compared;
- ``project`` output (run mode): the plan digest (FX ``plan``, gnode ``graph_sha256``) and the
  event envelope (``kind``, ``schema_version``, ``invocation_id``, ``offset_ms``,
  ``duration_ms``) are dropped from its ``run`` events, gnode's ``run_canceled`` is
  ``run_cancelled``, and gnode's instance errors get FX's names. Output files keep their
  digests: both engines store the same bytes under the same digest;
- text (run mode, gnode side): ``gnode run|reroll|pick|takes|jobs`` → ``grida-fx …`` and a kind
  ``gnode-<name>-v<n>`` → ``fx-<name>-v<n>``, besides the names above.

A difference is reported as ``<path>: <gnode> != <fx>``. Paths start at ``$`` (the document);
a graph's instances are addressed by id (``$.instances[draw#1].state``), other list items by
index, and the exit status is the path ``status``. Known, recorded differences (decisions FX took
on purpose) are listed in ``KNOWN_DIFFERENCES`` with the case, what is compared (plan mode: the
verb; run mode: the name a step saves, or ``step <n>`` for a step that saves nothing) and a path;
a difference at that path or under it is reported as ``known`` instead of failing. In a known
path, ``[*]`` stands for any one member or item (``$.instances[*].facts.cost_usd``). The path
``$`` is the document as a whole, printed by one engine only: it covers that difference and
nothing under it.

Output: one line per comparison. Plan mode: ``<case> <verb>``, with ``(step <n>)`` when a case
has two steps of that verb. Run mode: ``<case> <saved name>`` (``(step <n>)`` when two steps
save that name), or ``<case> step <n> <verb>`` for a step that saves nothing. ``(ported)`` marks
a ported case. Each says ``same``, ``known (<n>)``, ``DIFFERS``
(followed by the differences that are not known and a unified diff of the normalised JSON) or
``SKIP <reason>`` (gnode missing, a case FX's suite lacks or one not ported, no step to compare).
A known difference that no comparison found any more is noted. Exit 1 when any comparison
differs.
"""

from __future__ import annotations

import argparse
import contextlib
import difflib
import hashlib
import importlib.util
import json
import os
import re
import shutil
import sys
import tempfile
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

import yaml

REPO = Path(__file__).resolve().parents[1]
CONFORMANCE = REPO / "conformance"
GNODE_REPO_ENV = "FX_GNODE_REPO"
#: The verbs plan mode compares, each step on its own.
VERBS = ("expand", "price", "identity")
#: Verbs that read or write what an earlier step left behind (a run folder, a takes file, the
#: store): a case with one is compared in run mode.
RUN_VERBS = ("run", "reroll", "pick", "takes", "project", "inspect", "jobs")
#: Verbs whose stdout is a JSON document that run mode normalises like plan mode's.
JSON_VERBS = (*VERBS, "project")
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
#: What run mode drops from the run events a ``project`` document keeps: the plan digest under
#: either engine's name, and the event envelope.
PROJECT_DROPPED = ("plan", "graph_sha256", "kind", "schema_version", "invocation_id", "offset_ms")
DIGEST_PATTERN = re.compile(r"[0-9a-f]{64}")


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
    # (ported) Lock drift is a problem on the step's declaration path (`b`), not on `uses`.
    ("lock-drift", "expand", "$.problems"),
    # (ported) A declared resource that is missing is a problem on the step's declaration path
    # (`lost`), and the step is absent; gnode stops with a traceback and prints no graph.
    ("resource-missing", "expand", ROOT),
    # Run mode.
    # (ported) A JSON output's whole numbers are written without `.0` (identity.md sections 1
    # and 5): `"half": 2`, where gnode wrote Python's `2.0`; so the file's bytes, digest and size
    # differ too.
    ("run-local", "report.json", "$[2]"),
    ("run-local", "project.json", "$.run.run_finished.outputs.report.file.digest"),
    ("run-local", "project.json", "$.run.run_finished.outputs.report.file.size"),
    # (ported) The engine writes marks as `fx-annotations-v1` and keeps the port's kind
    # `annotations` for the file (store.md section 8); gnode wrote `gnode-annotations-v1` (the
    # text compares the same once named) of kind `json`.
    ("run-local", "project.json", "$.run.run_finished.outputs.marks.file"),
    # (ported) A delivered key is made a safe path (store.md section 8, "Keys as paths"): `y z`
    # is delivered as `y_z.txt`, where gnode wrote `y z.txt`.
    ("run-deliver", "each-y_z.txt", ROOT),
    # (ported) run_finished lists the failures in the expansion's order, so a run's record is
    # the same every time; gnode listed them as they finished.
    ("run-failures", "project.json", "$.run.run_finished.failed"),
    ("run-failures", "events.json", "$[*].failed"),
    # (ported) A node_retry event's error names the exception's type, as node_failed's does;
    # gnode's held the bare message.
    ("run-retry-engine", "events.json", "$[*].error"),
    # (ported) A picked result is the bare digest (fx-takes-v1); gnode wrote `sha256:<digest>`.
    ("run-takes", "takes-picked.txt", "$[0]"),
    ("run-takes", "case.takes.yaml", "$[2]"),
    # (ported) A step past its timeout fails with `ran past <n> seconds` and is never run again
    # (protocol.md section 7); gnode ran a `retry="engine"` body six times, each failing as
    # `TimeoutError: `.
    ("run-timeout", "naps.txt", "$[0]"),
    ("run-timeout", "project.json", '$.instances["nap#1"].error'),
]
# The engine always writes the node fact `cost_usd`, null when the step made no paid call
# (protocol.md section 5.3); gnode wrote it only after an uncached call. So every projected
# instance that succeeded has it, and so does every node_finished event.
KNOWN_DIFFERENCES += [
    ("run-failures", "events.json", "$[*].facts.cost_usd"),
    ("run-retry-engine", "events.json", "$[*].facts.cost_usd"),
]
KNOWN_DIFFERENCES += [
    (case, saved, "$.instances[*].facts.cost_usd")
    for case, saved in (
        ("at-plan-run", "project.json"),
        ("run-cache-hit", "project-one.json"),
        ("run-cache-hit", "project-two.json"),
        ("run-failures", "project.json"),
        ("run-local", "project.json"),
        ("run-project", "project.json"),
        ("run-resume", "project-first.json"),
        ("run-resume", "project-resumed.json"),
        ("run-retry-engine", "project.json"),
        ("run-takes", "project-two.json"),
        ("run-takes", "project-three.json"),
    )
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
    # Run mode: decisions of the runner (protocol.md section 5.3, store.md section 8).
    "a step that needs: a regenerating step runs once that step's takes are decided "
    "(gnode never runs it)",
    "a step that reads a blocked step is skipped as blocked in turn (node_skipped)",
    "node_failed keeps the facts a node_error reported",
    "a body that reports the fact cost_usd is refused: the engine writes it",
    "timeout: bounds an at: plan step too",
    'ctx.fail("") fails with "NodeFailure"',
    "an engine error a body let propagate fails with its own class (NotLive, CallFailed, ...) "
    "and message, without gnode's calls= hint",
    "a refusal names the run folder as typed (runs/one)",
    "the plan digest holds the type identities, so a run whose unversioned type changed is "
    "refused in the same folder",
    "a picked take whose result changed fails the step",
    "a body's stdout is not echoed",
    "each take of a step is placed in a folder of its own, files/<step>#<takes>/ (gnode placed "
    "every take at files/<step>/)",
    "a run-time assertion over a result that arrives after its step ran fails that step",
)

#: Defects FX keeps from gnode for now, the same in both engines (so no comparison shows them).
SHARED_DEFECTS: tuple[str, ...] = (
    "a judge's facts read from outside its step (steps.<judge>.facts) are the last take's, not "
    "those of the take the judged step's reference resolves to (the kept or picked one)",
    "a workflow-level assert: over a value only the run produces is never checked",
)
#: Where the spec lists its own changes (one number type, booleans not numbers, strict YAML, ...).
SPEC_CHANGES = "and the changes spec/identity.md section 13 lists"

#: Cases of FX's suite that are not ported, with why.
NOT_PORTED: dict[str, str] = {
    # Every step refuses, or not, by FX's own strict YAML reader (yaml.md), where gnode reads
    # YAML 1.1: the statuses differ by decision, and the case's own expected statuses pin FX.
    "yaml-strict": "FX's strict YAML subset is its own (yaml.md); gnode reads YAML 1.1",
    # The case seeds the project's store with an FX call record, under FX's call key and in FX's
    # store layout (store.md section 9): gnode's store holds neither, so its run would refuse
    # the paid call (which cache-replay-miss already compares).
    "cache-replay": "its seeded store is FX's (store.md section 9); gnode's cannot read it",
    # The case pins FX's one number type (identity.md sections 1, 5 and 13): gnode reads its
    # inputs files as YAML 1.1, where `1e21` is a string, so no step plans. With inputs gnode
    # can read (`1.0e+21`), `n: 1` and `n: 1.0` give gnode two identities, it renders
    # `e16=1e+16`, and it accepts the integer that FX refuses: each a recorded change.
    "numbers": "it pins FX's one number type (identity.md section 13); gnode reads YAML 1.1",
}

_ABSENT = object()
_NOT_JSON = object()
#: A step that saves nothing: only its exit status is compared.
_NOTHING = object()


def _special(value: Any) -> bool:
    """Whether ``value`` is one of the markers above rather than a document."""
    return value is _ABSENT or value is _NOT_JSON or value is _NOTHING


# gnode's names in texts it prints, with FX's. A name is matched where it starts a word, so a
# path such as src/gnode/... is left alone.
_START = r"(?<![\w/.-])"
_GNODE_NAMES: list[tuple[re.Pattern[str], str]] = [
    (
        re.compile(_START + r"gnode (?=lock\b|nodes\b|takes\b|run\b|reroll\b|pick\b|jobs\b)"),
        "grida-fx ",
    ),
    (re.compile(_START + r"gnode-(?=[a-z]+(?:-[a-z]+)*-v\d)"), "fx-"),
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


def normalise_identity(document: Any) -> Any:
    """``identity`` output, instance id to identity, with every identity ``"<digest>"``."""
    if not isinstance(document, dict):
        return _scrubbed(document)
    return {
        ident: DIGEST if isinstance(value, str) and DIGEST_PATTERN.fullmatch(value) else value
        for ident, value in document.items()
    }


def normalise_project(document: Any, side: str) -> Any:
    """``project`` output: instance states and the run events it keeps, without the plan digest
    and the event envelope; gnode's ``run_canceled`` and instance errors with FX's names."""
    if side not in ("gnode", "fx"):
        raise ValueError(f"side is gnode or fx, not {side!r}")
    if not isinstance(document, dict):
        return _scrubbed(document)
    shaped = dict(document)
    run = shaped.get("run")
    if isinstance(run, dict):
        events: dict[str, Any] = {}
        for name, event in run.items():
            if side == "gnode" and name == "run_canceled":
                name = "run_cancelled"
            if isinstance(event, dict):
                event = {key: item for key, item in event.items() if key not in PROJECT_DROPPED}
                if side == "gnode" and event.get("event") == "run_canceled":
                    event["event"] = "run_cancelled"
            events[name] = event
        shaped["run"] = events
    instances = shaped.get("instances")
    if isinstance(instances, dict) and side == "gnode":
        shaped["instances"] = {
            ident: {
                key: fx_names(item) if key == "error" and isinstance(item, str) else item
                for key, item in entry.items()
            }
            if isinstance(entry, dict)
            else entry
            for ident, entry in instances.items()
        }
    return _scrubbed(shaped)


def normalise_events(events: list[Any], side: str) -> list[Any]:
    """A run's events (``jsonl: true``), each without the plan digest and the envelope, a step
    identity as ``"<digest>"``, gnode's ``run_canceled`` as ``run_cancelled`` and its ``error``
    and ``uses`` with FX's names (``gnode/select@1`` is ``fx/select@1``), sorted as the
    conformance suite sorts them."""
    if side not in ("gnode", "fx"):
        raise ValueError(f"side is gnode or fx, not {side!r}")
    shaped: list[Any] = []
    for event in events:
        if isinstance(event, dict):
            event = {
                key: item
                for key, item in event.items()
                if key not in PROJECT_DROPPED and key != "duration_ms"
            }
            if side == "gnode" and event.get("event") == "run_canceled":
                event["event"] = "run_cancelled"
            if side == "gnode":
                for name in ("error", "uses"):
                    if isinstance(event.get(name), str):
                        event[name] = fx_names(event[name])
            if isinstance(event.get("identity"), str):
                event["identity"] = DIGEST
        shaped.append(_scrubbed(event))
    return sorted(shaped, key=lambda item: json.dumps(item, sort_keys=True))


def normalised(verb: str | None, document: Any, side: str) -> Any:
    """The comparable form of a JSON document a step printed or read; ``verb`` is the step's
    verb (None for a file a step reads)."""
    if verb in ("expand", "price"):
        return normalise(document, side)
    if verb == "identity":
        return normalise_identity(document)
    if verb == "project":
        return normalise_project(document, side)
    return _without_timings(_scrubbed(document))


def _without_timings(value: Any) -> Any:
    """Run-event timings dropped as the conformance suite drops them: from objects that carry an
    ``event`` member, never from data such as ``with`` or ``outputs``."""
    if isinstance(value, list):
        return [_without_timings(item) for item in value]
    if isinstance(value, dict):
        drop = conformance.VOLATILE_KEYS if "event" in value else set()
        return {
            key: item if key in conformance.DATA_KEYS else _without_timings(item)
            for key, item in value.items()
            if key not in drop
        }
    return value


def text_lines(raw: bytes, side: str) -> list[str]:
    """A text a step printed or read, line by line; gnode's with FX's names. Bytes that are not
    UTF-8 compare by their digest."""
    if side not in ("gnode", "fx"):
        raise ValueError(f"side is gnode or fx, not {side!r}")
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        return [f"sha256:{hashlib.sha256(raw).hexdigest()}"]
    if side == "gnode":
        text = fx_names(text)
    return text.splitlines()


def scrub_roots(raw: bytes, roots: list[Path]) -> bytes:
    """``raw`` with absolute paths into a project copy made relative to it: ``<root>/x`` is
    ``x`` and ``<root>`` alone is ``.``, for each spelling of the root (as made, and resolved
    through symbolic links such as macOS's ``/tmp``)."""
    spellings: list[str] = []
    for root in roots:
        for spelling in (str(root), str(root.resolve())):
            if spelling not in spellings:
                spellings.append(spelling)
    # The longest spelling first, so a root never cuts another one short.
    for spelling in sorted(spellings, key=len, reverse=True):
        raw = raw.replace(spelling.encode("utf-8") + b"/", b"")
        raw = re.sub(re.escape(spelling.encode("utf-8")) + rb"(?![\w./-])", b".", raw)
    return raw


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
    if value is _NOTHING:
        return "(nothing saved)"
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
        if known_path in (ROOT, STATUS):
            continue
        if _known_pattern(known_path).match(path):
            return known
    return None


#: One bracketed path segment: a quoted member name (which may hold ``]``) or an index or id.
_SEGMENT = r'\[(?:"(?:[^"\\]|\\.)*"|[^\]]*)\]'


def _known_pattern(known_path: str) -> re.Pattern[str]:
    """A known path as a pattern that matches it and anything under it; ``[*]`` is any one
    member or item."""
    parts = known_path.split("[*]")
    body = _SEGMENT.join(re.escape(part) for part in parts)
    return re.compile(body + r"(?:$|[.\[])")


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
        """A plan-mode comparison: both stdouts parsed as JSON and normalised for ``verb``."""
        g_status, g_out, g_err = gnode
        f_status, f_out, f_err = fx
        g_doc, f_doc = _parsed(g_out), _parsed(f_out)
        if g_doc is not _NOT_JSON:
            g_doc = normalised(verb, g_doc, "gnode")
        if f_doc is not _NOT_JSON:
            f_doc = normalised(verb, f_doc, "fx")
        self.compare_documents(
            label, case, verb, (g_status, g_doc, g_err), (f_status, f_doc, f_err)
        )

    def compare_documents(
        self,
        label: str,
        case: str | None,
        what: str,
        gnode: tuple[int | None, Any, str],
        fx: tuple[int | None, Any, str],
    ) -> None:
        """Compares two normalised documents (``_NOT_JSON`` for an output that is not JSON,
        ``_NOTHING`` when a step saves nothing) and exit statuses (None when a step ran no
        command); ``what`` names the comparison in ``KNOWN_DIFFERENCES``."""
        g_status, g_doc, g_err = gnode
        f_status, f_doc, f_err = fx
        found: list[tuple[str, str]] = []
        if g_status != f_status:
            found.append((STATUS, f"{STATUS}: {g_status} != {f_status}"))
        diff: list[str] = []
        if g_doc is _NOTHING and f_doc is _NOTHING:
            pass
        elif not _special(g_doc) and not _special(f_doc):
            found.extend(_differences(g_doc, f_doc))
            diff = list(difflib.unified_diff(_pretty(g_doc), _pretty(f_doc), "gnode", "grida-fx"))
        elif g_doc is not f_doc:
            _differ(found, ROOT, g_doc, f_doc)
        unknown: list[str] = []
        known = 0
        if case is not None:
            self.compared.add((case, what))
        for path, line in found:
            entry = _is_known(case, what, path) if case is not None else None
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
                if (document is _NOT_JSON or g_status != f_status) and stderr.strip():
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
            steps = conformance.load_case(fx_case)
            plan = _case_plan(fx_case)
        except conformance.CaseFailure as error:
            report.skip(name, str(error))
            continue
        if is_run_case(steps):
            _compare_run_case(report, command, repo, name, ported, steps)
            continue
        if not plan:
            report.skip(name, "no expand, price or identity step")
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


# ------------------------------------------------------------------------------- run mode


@dataclass
class StepRun:
    """What one step of a run-mode case did on one engine."""

    #: The exit status, or None when the step ran no command.
    status: int | None
    stderr: str
    #: What the step saves: its stdout, or the file it reads (None: the file is absent).
    saved: bytes | None


def is_run_case(steps: list[dict[str, Any]]) -> bool:
    """Whether a case's steps are compared in run mode: one of them runs or reads a run."""
    return any(step.get("argv", [""])[0] in RUN_VERBS for step in steps)


def run_gnode_steps(repo: Path, case_in: Path, steps: list[dict[str, Any]]) -> list[StepRun]:
    """Runs every step of a case with gnode, in order, in one copy of ``case_in``."""
    return _run_steps(_gnode_command(repo), case_in, steps, {"GNODE_PLUGINS": "std"}, "gnode")


def run_fx_steps(command: list[str], case_in: Path, steps: list[dict[str, Any]]) -> list[StepRun]:
    """Runs every step of a case with grida-fx, in order, in one copy of ``case_in``."""
    return _run_steps(command, case_in, steps, conformance.passed_through(), "grida-fx")


def _run_steps(
    program: list[str],
    case_in: Path,
    steps: list[dict[str, Any]],
    passed: dict[str, str],
    label: str,
) -> list[StepRun]:
    """Each step as the conformance runner runs it, with gnode's names when ``label`` is
    ``gnode``; absolute paths into the copy made relative in everything kept."""
    gnode = label == "gnode"
    ran: list[StepRun] = []
    with tempfile.TemporaryDirectory(prefix="fx-compare-gnode-") as work:
        project = Path(work) / "project"
        home = Path(work) / "home"
        home.mkdir()
        shutil.copytree(case_in, project, ignore=shutil.ignore_patterns(*COPY_IGNORED))
        env = conformance.step_environment(home, passed)
        for number, step in enumerate(steps, 1):
            files = dict(step.get("files", {}))
            for relative, text in (gnode_files(files) if gnode else files).items():
                target = conformance.project_path(project, relative, f"step {number} files")
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(text.encode("utf-8"))
            status: int | None = None
            stderr = b""
            saved: bytes | None = None
            if "argv" in step:
                argv = [gnode_path(arg) if gnode else arg for arg in step["argv"]]
                stdin = step["stdin"].encode("utf-8") if "stdin" in step else None
                status, stdout, stderr = conformance.run_command(
                    [*program, *argv], project, env, stdin, TIMEOUT_S, f"{label} {' '.join(argv)}"
                )
                saved = scrub_roots(stdout, [project])
            if "read" in step:
                relative = gnode_path(step["read"]) if gnode else step["read"]
                path = conformance.project_path(project, relative, f"step {number} read")
                saved = scrub_roots(path.read_bytes(), [project]) if path.is_file() else None
            ran.append(
                StepRun(
                    status,
                    scrub_roots(stderr, [project]).decode("utf-8", errors="replace"),
                    saved,
                )
            )
    return ran


def step_document(step: dict[str, Any], ran: StepRun, side: str) -> Any:
    """The comparable form of what a step saves: a normalised JSON document, or the lines of a
    text; ``_NOTHING`` for a step that saves nothing, ``_ABSENT`` for a file it could not read."""
    if "save" not in step:
        return _NOTHING
    if ran.saved is None:
        return _ABSENT
    verb = step["argv"][0] if "argv" in step and "read" not in step else None
    if step.get("jsonl"):
        try:
            lines = ran.saved.decode("utf-8").split("\n")
            return normalise_events([json.loads(line) for line in lines if line.strip()], side)
        except (UnicodeDecodeError, ValueError):
            return _NOT_JSON
    if not (step.get("json") or step.get("yaml") or verb in JSON_VERBS):
        return text_lines(ran.saved, side)
    try:
        text = ran.saved.decode("utf-8")
        document = yaml.safe_load(text) if step.get("yaml") else json.loads(text)
    except (UnicodeDecodeError, ValueError, yaml.YAMLError):
        return _NOT_JSON
    return normalised(verb, document, side)


def _compare_run_case(
    report: _Report,
    command: list[str],
    repo: Path,
    name: str,
    ported: bool,
    steps: list[dict[str, Any]],
) -> None:
    gnode_case = repo / "tests" / "conformance" / name
    fx_case = CONFORMANCE / name
    try:
        with _gnode_case_in(gnode_case, fx_case, ported) as original:
            gnode = run_gnode_steps(repo, original, steps)
        fx = run_fx_steps(command, fx_case / "in", steps)
    except conformance.CaseFailure as error:
        _timed_out(report, f"{name}" + (" (ported)" if ported else ""), error)
        return
    saves = [step.get("save") for step in steps]
    for number, (step, g_ran, f_ran) in enumerate(zip(steps, gnode, fx, strict=True), 1):
        if "argv" not in step and "save" not in step:
            continue
        save = step.get("save")
        what = save if save else f"step {number}"
        shown = save if save else f"step {number} {step['argv'][0]}"
        notes = [f"step {number}"] if save and saves.count(save) > 1 else []
        notes += ["ported"] if ported else []
        label = f"{name} {shown}" + (f" ({', '.join(notes)})" if notes else "")
        report.compare_documents(
            label,
            name,
            what,
            (g_ran.status, step_document(step, g_ran, "gnode"), g_ran.stderr),
            (f_ran.status, step_document(step, f_ran, "fx"), f_ran.stderr),
        )


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
