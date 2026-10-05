#!/usr/bin/env python3
r"""Run every conformance case through an FX command line and compare what it prints.

The command is the interface under test: this script never imports an engine. It needs only
the standard library and PyYAML.

    python conformance/run.py --command target/debug/grida-fx            # check every case
    python conformance/run.py --command target/debug/grida-fx linear     # check some cases
    python conformance/run.py --command target/debug/grida-fx --write    # record expected/

The command comes from --command, else GRIDA_FX_CONFORMANCE_COMMAND, else `grida-fx` on PATH.
It is split like a shell would split it, so it can carry arguments. Every step runs in a
temporary copy of its case's project, so a Cargo command needs the workspace's manifest by
absolute path (build first: a step's timeout includes any build):

    python conformance/run.py \
        --command "cargo run -q --manifest-path '$PWD/Cargo.toml' --bin grida-fx --"
"""

from __future__ import annotations

import argparse
import difflib
import json
import math
import os
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from decimal import Decimal
from pathlib import Path, PurePosixPath
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parent
COMMAND_ENV = "GRIDA_FX_CONFORMANCE_COMMAND"
DEFAULT_COMMAND = "grida-fx"
DEFAULT_TIMEOUT_S = 60.0
CASE_KEYS = {"steps", "invalid_inputs"}
STEP_KEYS = {"argv", "status", "save", "json", "yaml", "read", "files", "stdin", "mentions"}
# Run-event members whose values differ between machines and invocations. They are dropped only
# from run events (objects with an "event" member), never from the data under DATA_KEYS.
VOLATILE_KEYS = {"offset_ms", "invocation_id", "duration_ms"}
DATA_KEYS = {"with", "inputs", "outputs", "request", "data", "facts", "params", "value", "contract"}
# Which Python hosts node bodies: passed through from the caller when set.
PYTHON_HOST = "GRIDA_FX_PYTHON"
# Where rustup and Cargo keep their toolchains: HOME is replaced, so a `cargo run` command finds
# them only through these. From the caller, else the default folder in the caller's home.
TOOLCHAIN_HOMES = {"RUSTUP_HOME": ".rustup", "CARGO_HOME": ".cargo"}
# JCS writes every integral number of 1e21 and above with an exponent, so no integer literal of
# more than 21 digits is the canonical form of a number (identity.md section 1).
MAX_INTEGER_DIGITS = 21
SHOWN_OUTPUT = 4000


class CaseFailure(Exception):
    """A case did not hold: a wrong status, a timeout, unreadable output, a bad case file."""


@dataclass
class Outcome:
    name: str
    verdict: str  # PASS, FAIL, PENDING or WROTE
    detail: str = ""
    saved: dict[str, bytes] = field(default_factory=dict)


# ----------------------------------------------------------------------------- the command


def resolve_command(given: str | None) -> list[str]:
    """The command under test, with its program resolved before any step changes directory."""

    text = given if given is not None else os.environ.get(COMMAND_ENV) or DEFAULT_COMMAND
    argv = shlex.split(text)
    if not argv:
        raise SystemExit("conformance: the command is empty")
    program = argv[0]
    if _names_a_path(program):
        argv[0] = _executable(program, program)
    else:
        found = shutil.which(program)
        if found is None:
            raise SystemExit(
                f"conformance: no {program} command on PATH; pass --command or set {COMMAND_ENV}"
            )
        argv[0] = found
    return argv


def _names_a_path(text: str) -> bool:
    return os.sep in text or bool(os.altsep and os.altsep in text)


def _executable(text: str, what: str) -> str:
    """A program path made absolute from where the runner was started, before any step changes
    directory. Symbolic links are kept: a virtual environment's python is a link, and following
    it would leave the environment."""

    path = os.path.abspath(os.path.expanduser(text))
    if not os.path.isfile(path) or not os.access(path, os.X_OK):
        raise SystemExit(f"conformance: {what} is not an executable file")
    return path


def passed_through() -> dict[str, str]:
    """What the caller's environment gives every step: the Python host and the toolchain homes."""

    passed: dict[str, str] = {}
    python = os.environ.get(PYTHON_HOST)
    if python:
        # A path is resolved now: every step runs in a temporary folder, where a relative path
        # would name nothing. A bare name is left for the engine to find on PATH.
        if _names_a_path(python):
            python = _executable(python, f"{PYTHON_HOST}={python}")
        passed[PYTHON_HOST] = python
    for name, folder in TOOLCHAIN_HOMES.items():
        value = os.environ.get(name)
        if value:
            passed[name] = os.path.abspath(os.path.expanduser(value))
        elif (default := Path.home() / folder).is_dir():
            passed[name] = str(default)
    return passed


def step_environment(home: Path, passed: dict[str, str]) -> dict[str, str]:
    """A minimal environment: nothing of the caller's leaks in but PATH and `passed`.

    No provider key, tool override, FX cache location or locale can change what a case prints.
    """

    return {
        "PATH": os.environ.get("PATH", os.defpath),
        "HOME": str(home),
        "NO_COLOR": "1",
        "LANG": "C.UTF-8",
        # Node hosts must not write bytecode into the project the case reads.
        "PYTHONDONTWRITEBYTECODE": "1",
        **passed,
    }


def run_command(
    argv: list[str],
    cwd: Path,
    env: dict[str, str],
    stdin: bytes | None,
    timeout_s: float,
    label: str,
) -> tuple[int, bytes, bytes]:
    """Run one step; on timeout, stop it and everything it started."""

    process = subprocess.Popen(
        argv,
        cwd=cwd,
        env=env,
        stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(input=stdin, timeout=timeout_s)
    except subprocess.TimeoutExpired:
        _kill(process)
        stdout, stderr = process.communicate()
        raise CaseFailure(
            f"{label} did not finish within {timeout_s:g} s\n{_shown(stdout + stderr)}"
        ) from None
    return process.returncode, stdout, stderr


def _kill(process: subprocess.Popen[bytes]) -> None:
    if hasattr(os, "killpg"):
        try:
            os.killpg(process.pid, signal.SIGKILL)
            return
        except ProcessLookupError:
            return
        except PermissionError:
            pass
    process.kill()


# --------------------------------------------------------------------------------- a case


def project_path(project: Path, given: object, what: str) -> Path:
    """A path a case names inside its project: relative, POSIX, never leaving the project."""

    if not isinstance(given, str) or not given:
        raise CaseFailure(f"{what}: a path must be a non-empty string")
    pure = PurePosixPath(given)
    if pure.is_absolute() or ".." in pure.parts or "\\" in given:
        raise CaseFailure(f"{what}: {given} must be a relative path inside the project")
    path = project.joinpath(*pure.parts)
    if not path.resolve().is_relative_to(project.resolve()):
        raise CaseFailure(f"{what}: {given} leaves the project through a symbolic link")
    return path


def _is_path(given: object) -> bool:
    if not isinstance(given, str) or not given or "\\" in given:
        return False
    pure = PurePosixPath(given)
    return not pure.is_absolute() and ".." not in pure.parts


def _file_inside(base: Path, given: str) -> bool:
    """A file at a relative path under `base` that no symbolic link takes out of it."""

    path = base.joinpath(*PurePosixPath(given).parts)
    return path.is_file() and path.resolve().is_relative_to(base.resolve())


def load_case(case: Path) -> list[dict[str, Any]]:
    """The steps of case.yaml, refusing anything the format (README.md, "A case") does not have."""

    try:
        spec = yaml.safe_load((case / "case.yaml").read_text(encoding="utf-8"))
    except (OSError, yaml.YAMLError) as error:
        raise CaseFailure(f"case.yaml: {error}") from None
    if not isinstance(spec, dict):
        raise CaseFailure("case.yaml: a case is a mapping with a steps list")
    unknown = set(spec) - CASE_KEYS
    if unknown:
        raise CaseFailure(f"case.yaml: unknown keys {sorted(map(str, unknown))}")
    invalid_inputs = spec.get("invalid_inputs", [])
    if not isinstance(invalid_inputs, list) or not all(map(_is_path, invalid_inputs)):
        raise CaseFailure("case.yaml: invalid_inputs is a list of relative paths")
    for given in invalid_inputs:
        if not any(_file_inside(base, given) for base in (case / "in", case)):
            raise CaseFailure(
                f"case.yaml: invalid_inputs: {given} is no file under in/ or the case folder"
            )
    steps = spec.get("steps")
    if not isinstance(steps, list) or not steps:
        raise CaseFailure("case.yaml: needs a non-empty steps list")
    for number, step in enumerate(steps, 1):
        where = f"case.yaml step {number}"
        if not isinstance(step, dict):
            raise CaseFailure(f"{where}: a step is a mapping")
        unknown = set(step) - STEP_KEYS
        if unknown:
            raise CaseFailure(f"{where}: unknown keys {sorted(map(str, unknown))}")
        if not ({"argv", "read", "files"} & set(step)):
            raise CaseFailure(f"{where}: needs argv, read or files")
        argv = step.get("argv")
        if argv is not None and (
            not isinstance(argv, list) or not all(isinstance(item, str) for item in argv)
        ):
            raise CaseFailure(f"{where}: argv is a list of strings")
        if "read" in step and "save" not in step:
            raise CaseFailure(f"{where}: read needs save")
        if "read" in step and not _is_path(step["read"]):
            raise CaseFailure(f"{where}: read is a relative path inside the project")
        if ("status" in step or "stdin" in step or "mentions" in step) and argv is None:
            raise CaseFailure(f"{where}: status, stdin and mentions need argv")
        if "save" in step and argv is None and "read" not in step:
            raise CaseFailure(f"{where}: save needs argv or read")
        save = step.get("save")
        if save is not None and (
            not isinstance(save, str) or not save or "/" in save or save.startswith(".")
        ):
            raise CaseFailure(f"{where}: save is a plain file name")
        for flag in ("json", "yaml"):
            if not isinstance(step.get(flag, False), bool):
                raise CaseFailure(f"{where}: {flag} is true or false")
            if flag in step and "save" not in step:
                raise CaseFailure(f"{where}: {flag} needs save")
        if step.get("json") and step.get("yaml"):
            raise CaseFailure(f"{where}: json and yaml exclude each other")
        status = step.get("status", 0)
        if not isinstance(status, int) or isinstance(status, bool):
            raise CaseFailure(f"{where}: status is an integer")
        files = step.get("files", {})
        if not isinstance(files, dict) or not all(
            _is_path(path) and isinstance(text, str) for path, text in files.items()
        ):
            raise CaseFailure(f"{where}: files maps relative paths to text")
        if "stdin" in step and not isinstance(step["stdin"], str):
            raise CaseFailure(f"{where}: stdin is text")
        mentions = step.get("mentions", [])
        if not isinstance(mentions, list) or not all(isinstance(m, str) for m in mentions):
            raise CaseFailure(f"{where}: mentions is a list of strings")
    return steps


def run_case(
    case: Path, work: Path, command: list[str], passed: dict[str, str], timeout_s: float
) -> dict[str, bytes]:
    steps = load_case(case)
    project = work / "project"
    home = work / "home"
    home.mkdir()
    shutil.copytree(case / "in", project, ignore=shutil.ignore_patterns("__pycache__"))
    env = step_environment(home, passed)
    saved: dict[str, bytes] = {}
    saved_by: dict[str, int] = {}
    for number, step in enumerate(steps, 1):
        label = f"step {number}"
        for relative, text in step.get("files", {}).items():
            target = project_path(project, relative, f"{label} files")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(text.encode("utf-8"))
        output: bytes | None = None
        if "argv" in step:
            label = f"step {number} ({shlex.join(step['argv'])})"
            stdin = step["stdin"].encode("utf-8") if "stdin" in step else None
            status, stdout, stderr = run_command(
                [*command, *step["argv"]], project, env, stdin, timeout_s, label
            )
            expected_status = step.get("status", 0)
            if status != expected_status:
                raise CaseFailure(
                    f"{label} exited {status}, not {expected_status}\n{_shown(stdout + stderr)}"
                )
            both = (stdout + b"\n" + stderr).decode("utf-8", errors="replace")
            missing = [text for text in step.get("mentions", []) if text not in both]
            if missing:
                raise CaseFailure(f"{label} never mentions {missing}\n{_shown(stdout + stderr)}")
            output = stdout
        if "read" in step:
            path = project_path(project, step["read"], f"{label} read")
            if not path.is_file():
                raise CaseFailure(f"{label}: {step['read']} was not written")
            output = path.read_bytes()
            label = f"step {number} (read {step['read']})"
        if "save" not in step:
            continue
        assert output is not None
        if step.get("json"):
            output = normalised_json(output, label)
        elif step.get("yaml"):
            output = normalised_yaml(output, label)
        name = step["save"]
        if name in saved and saved[name] != output:
            raise CaseFailure(
                f"{label} saved {name} unlike step {saved_by[name]}, which must match\n"
                + _diff(saved[name], output, f"step {saved_by[name]}", f"step {number}")
            )
        saved.setdefault(name, output)
        saved_by.setdefault(name, number)
    return saved


def normalised_json(raw: bytes, label: str) -> bytes:
    """JSON as the suite compares it: keys sorted, run-event timings dropped, numbers as values."""

    try:
        value = json.loads(
            raw.decode("utf-8"),
            parse_constant=_refuse_constant,
            parse_float=_finite,
            parse_int=_exact_integer,
        )
    except (UnicodeDecodeError, ValueError) as error:
        raise CaseFailure(f"{label}: not JSON: {error}\n{_shown(raw)}") from None
    return _dumped(value, label)


def normalised_yaml(raw: bytes, label: str) -> bytes:
    """A YAML document the command wrote, compared by meaning: as normalised JSON."""

    try:
        value = yaml.safe_load(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError, yaml.YAMLError) as error:
        # ValueError: PyYAML converts an integer of thousands of digits before anyone checks it.
        raise CaseFailure(f"{label}: not YAML: {error}\n{_shown(raw)}") from None
    return _dumped(value, label)


def _dumped(value: object, label: str) -> bytes:
    try:
        text = json.dumps(_normal(value), indent=1, sort_keys=True, allow_nan=False)
    except (TypeError, ValueError) as error:
        raise CaseFailure(f"{label} holds a value outside FX's values: {error}") from None
    return (text + "\n").encode("utf-8")


def _refuse_constant(name: str) -> object:
    raise ValueError(f"{name} is not a JSON value")


def _finite(text: str) -> float:
    value = float(text)
    if not math.isfinite(value):
        raise ValueError(f"{text} overflows a binary64 number")
    return value


def _exact_integer(text: str) -> int:
    """An integer literal that reads as a number without rounding (identity.md section 1).

    A literal longer than any canonical number is refused by its length, before Python converts
    it."""

    digits = len(text.lstrip("-"))
    if digits > MAX_INTEGER_DIGITS:
        raise ValueError(f"the integer {text[:40]}... would be rounded: it has {digits} digits")
    value = int(text)
    _check_exact(value)
    return value


def _check_exact(value: int) -> None:
    """Refuse an integer whose digits are not the canonical form of the binary64 number they
    read as: 9007199254740992 and 10000000000000000 are exact, 9007199254740993 is not."""

    if abs(value) >= 10**MAX_INTEGER_DIGITS:
        raise ValueError(f"an integer of more than {MAX_INTEGER_DIGITS} digits would be rounded")
    canonical = _jcs_integer(float(value))
    if canonical != value:
        raise ValueError(f"the integer {value} would be rounded to {canonical}")


def _jcs_integer(value: float) -> int:
    """The integer JCS writes for an integral number below 1e21: its shortest round-trip digits,
    padded with zeros (1e16 is 10000000000000000)."""

    return int(Decimal(repr(value)))


def _normal(value: object, in_data: bool = False) -> object:
    """Drop what differs between invocations; `1` and `1.0` are one number.

    Timings and invocation ids are dropped from run events only (objects with an "event"
    member), never inside the data a record carries (`with`, `outputs`, `facts`, ...), where a
    member may have any name.
    """

    if isinstance(value, dict):
        event = not in_data and "event" in value
        return {
            key: _normal(item, in_data or key in DATA_KEYS)
            for key, item in value.items()
            if not (event and key in VOLATILE_KEYS)
        }
    if isinstance(value, list):
        return [_normal(item, in_data) for item in value]
    if isinstance(value, bool) or value is None or isinstance(value, str):
        return value
    if isinstance(value, int):
        # A YAML integer is held to the rule a JSON one is: written as it reads.
        _check_exact(value)
        return value
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ValueError(f"{value} is not a number FX has")
        # JCS writes an integral number below 1e21 as digits, whatever the input looked like.
        if value.is_integer() and abs(value) < 1e21:
            return _jcs_integer(value)
        return value
    raise ValueError(f"{type(value).__name__} is not a JSON value")


# ------------------------------------------------------------------------------ comparing


def compare(case: Path, saved: dict[str, bytes], strict: bool) -> Outcome:
    expected = case / "expected"
    if not expected.is_dir():
        if not saved:
            return Outcome(case.name, "PASS")
        if strict:
            return Outcome(case.name, "FAIL", "no expected/ yet (--strict)")
        return Outcome(case.name, "PENDING", f"{len(saved)} outputs, no expected/ yet")
    problems: list[str] = []
    for name, actual in saved.items():
        path = expected / name
        if not path.is_file():
            problems.append(f"expected/{name} is missing")
            continue
        want = path.read_bytes()
        if want != actual:
            problems.append(f"{name} differs\n{_diff(want, actual, f'expected/{name}', 'actual')}")
    for stale in sorted(stale_files(expected, saved)):
        problems.append(f"expected/{stale} is stale: no step saves it")
    if problems:
        return Outcome(case.name, "FAIL", "\n".join(problems))
    return Outcome(case.name, "PASS")


def stale_files(expected: Path, saved: dict[str, bytes]) -> list[str]:
    return [
        path.relative_to(expected).as_posix()
        for path in expected.rglob("*")
        if path.is_file() and path.relative_to(expected).as_posix() not in saved
    ]


def write(case: Path, saved: dict[str, bytes]) -> Outcome:
    expected = case / "expected"
    removed = stale_files(expected, saved) if expected.is_dir() else []
    for stale in removed:
        (expected / stale).unlink()
    if saved:
        expected.mkdir(exist_ok=True)
    for name, data in saved.items():
        (expected / name).write_bytes(data)
    detail = f"{len(saved)} outputs"
    if removed:
        detail += f"; removed stale {', '.join(removed)}"
    return Outcome(case.name, "WROTE", detail)


def _diff(want: bytes, actual: bytes, want_label: str, actual_label: str) -> str:
    text = "".join(
        difflib.unified_diff(
            want.decode("utf-8", errors="replace").splitlines(True),
            actual.decode("utf-8", errors="replace").splitlines(True),
            want_label,
            actual_label,
        )
    )
    if not text:
        text = f"(the bytes differ: {len(want)} expected, {len(actual)} actual)\n"
    return text[:SHOWN_OUTPUT]


def _shown(output: bytes) -> str:
    text = output.decode("utf-8", errors="replace")
    if len(text) > SHOWN_OUTPUT:
        text = text[:SHOWN_OUTPUT] + f"\n... ({len(text) - SHOWN_OUTPUT} more characters)"
    return text


# ----------------------------------------------------------------------------------- main


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--command",
        help=f"the command under test (default: ${COMMAND_ENV}, else {DEFAULT_COMMAND} on PATH)",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT_S,
        help=f"seconds each step may take (default {DEFAULT_TIMEOUT_S:g})",
    )
    parser.add_argument(
        "--write",
        action="store_true",
        help="record what the command prints as expected/; a person reviews it before commit",
    )
    parser.add_argument("--strict", action="store_true", help="a case with no expected/ yet fails")
    parser.add_argument("cases", nargs="*", help="case names (default: all)")
    args = parser.parse_args(argv)
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    command = resolve_command(args.command)
    passed = passed_through()
    cases = sorted(path.parent for path in ROOT.glob("*/case.yaml"))
    if args.cases:
        known = {case.name for case in cases}
        unknown = sorted(set(args.cases) - known)
        if unknown:
            parser.error(f"no such cases: {', '.join(unknown)}")
        cases = [case for case in cases if case.name in args.cases]

    outcomes: list[Outcome] = []
    for case in cases:
        with tempfile.TemporaryDirectory(prefix=f"fx-conformance-{case.name}-") as work:
            try:
                saved = run_case(case, Path(work), command, passed, args.timeout)
            except CaseFailure as error:
                outcome = Outcome(case.name, "FAIL", str(error))
            else:
                outcome = write(case, saved) if args.write else compare(case, saved, args.strict)
        outcomes.append(outcome)
        line = f"{outcome.verdict} {case.name}"
        if outcome.detail and outcome.verdict != "PASS":
            separator = "\n" if outcome.verdict == "FAIL" else ": "
            line += f"{separator}{outcome.detail}"
        print(line, flush=True)

    counts = {verdict: 0 for verdict in ("PASS", "FAIL", "PENDING", "WROTE")}
    for outcome in outcomes:
        counts[outcome.verdict] += 1
    summary = ", ".join(f"{count} {verdict.lower()}" for verdict, count in counts.items() if count)
    print(f"conformance: {summary} ({len(outcomes)} cases, command: {shlex.join(command)})")
    if args.write and counts["WROTE"]:
        print(
            "conformance: expected/ now holds whatever this command printed. Review every "
            "changed file (git diff conformance/) before committing it: --write records, it "
            "never judges."
        )
    return 1 if counts["FAIL"] else 0


if __name__ == "__main__":
    sys.exit(main())
