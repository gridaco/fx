"""Planning and running from Python (``docs/guide/05-running.md`` "From Python"), by driving the
``grida-fx`` binary: the engine is the only place with engine logic (``AGENTS.md``).

- The binary: ``GRIDA_FX_BIN`` when set, else the one packaged with ``grida`` (``grida/fx/bin/
  grida-fx``, step 5), else ``grida-fx`` on ``PATH``; none: ``RuntimeError`` naming
  ``GRIDA_FX_BIN``. The binary's working directory is ``cwd`` (default: the process's), and every
  relative path given here (``input_files``, ``routes``, ``run_dir``, the paths inside
  ``inputs``) is relative to it, as on the command line.
- A target is a workflow file, a workflow id, a builder ``file.py:function`` (``arguments`` as
  ``--arg``), or a :class:`Plan` (run as it was planned: its own target and options; giving
  planning options with it is a ``TypeError``). A :class:`~grida.fx.Workflow` object is refused
  with ``TypeError`` (name its builder target).
- Inputs: ``inputs`` (a mapping) is written to a temporary inputs YAML file (strict subset: JSON is
  YAML) placed in ``cwd`` so its relative paths keep their meaning, passed with ``--inputs``
  after ``input_files``; ``routes`` paths go as ``--routes``; ``max_usd`` as ``--max-usd``.
- :func:`plan` / :func:`plan_async`: ``grida-fx plan --json`` and ``grida-fx price``; a
  :class:`Plan` with ``document`` (fx-graph-v1), ``ok``, ``problems``, ``estimate()`` and
  ``phases()``. Exit 2 raises :class:`FxError` with the engine's stderr line.
- :func:`run` / :func:`run_async`: plans first and raises :class:`PlanRefused` (``the plan is
  refused:`` and its problems) before any folder exists; then ``grida-fx run … --run <folder>``
  (``run_dir``, else a new folder chosen by the engine and read back from its summary), ``--live``
  and ``--yes-up-to`` as given. A failed step does not raise; a run the engine refuses to start
  raises :class:`FxError` (``refused: <message>``). The :class:`RunResult` is read from the
  folder's ``events.jsonl`` (the last ``run_finished``): ``ok``, ``incomplete``, ``cost``
  (``charged_usd``), ``run_dir``, ``failed`` (ids), ``outputs`` (files as
  :class:`~grida.fx._ctx.InputFile`-like objects with ``path`` in the store, collections as dicts,
  lists as lists), ``steps`` (by step path: the last finished take's ``facts`` and outputs), and
  ``deliver(targets, root=None)`` (``{key}`` for each element; returns the names it found no file
  for).
- :func:`run` cannot be called inside a running event loop (use :func:`run_async`).

The store is the cache of the planning project (``spec/store.md``): the nearest folder holding
``fx.yaml`` at or above the workflow file when the target is a ``.yaml``/``.yml`` path, else at or
above ``cwd``; its ``cache`` setting, default ``.fx/cache`` (section 8). A run's files are read
there by digest (``files/<d[:2]>/<d>``).
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import re
import secrets
import shutil
import signal
import subprocess
import tempfile
import unicodedata
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from decimal import Decimal
from pathlib import Path
from typing import Any

from grida.fx._builder import Workflow

#: The environment variable naming the binary to drive.
BINARY_VARIABLE = "GRIDA_FX_BIN"
_EXECUTABLE = "grida-fx.exe" if os.name == "nt" else "grida-fx"
#: The first line of a run's summary: ``run`` padded to 10 columns, then the folder as named.
_RUN_LINE = "run       "
#: How the engine says it would not start a run (on stdout, exit status 1).
_REFUSED = "refused: "
#: How the engine prefixes an error on stderr (exit status 2).
_ERROR = "grida-fx: "
_USAGE_OR_ERROR = 2
_CANCELLED = 130
#: How long a stopped engine gets to finish after its interrupt, before it is killed.
_STOP_GRACE_S = 10.0
#: How long the engine's pipes are still read once it has exited: what it wrote is already there,
#: and a process a node body started may hold them open for as long as it runs.
_DRAIN_S = 0.5
_DIGEST = re.compile(r"[0-9a-f]{64}")
_PROJECT_FILE = "fx.yaml"
_DEFAULT_CACHE = ".fx/cache"


class FxError(RuntimeError):
    """The engine stopped with exit status 2, or would not run: its message (``status`` is the
    exit status)."""

    def __init__(self, message: str, status: int | None = None) -> None:
        super().__init__(message)
        self.message = message
        self.status = status


class PlanRefused(RuntimeError):
    """A plan with problems was asked to run: ``the plan is refused:`` and one ``<where>:
    <message>`` line per problem; ``plan`` is the refused :class:`Plan`."""

    def __init__(self, plan: Plan) -> None:
        lines = [f"{problem['where']}: {problem['message']}" for problem in plan.problems]
        super().__init__("\n".join(["the plan is refused:", *lines]))
        self.plan = plan


# ------------------------------------------------------------------------------------------------
# Plans


@dataclass(frozen=True)
class _Request:
    """A target and its planning options, as the command line takes them."""

    target: str
    inputs: Mapping[str, Any] | None
    input_files: tuple[str, ...]
    max_usd: str | None
    cwd: Path
    routes: tuple[str, ...]
    arguments: tuple[tuple[str, str], ...]

    def args(self, inputs_file: str | None) -> list[str]:
        """The target and the planning options; ``inputs_file`` holds ``inputs``."""
        args = [self.target]
        args += [f"--inputs={path}" for path in self.input_files]
        if inputs_file is not None:
            args.append(f"--inputs={inputs_file}")
        args += [f"--routes={path}" for path in self.routes]
        args += [f"--arg={name}={value}" for name, value in self.arguments]
        if self.max_usd is not None:
            args.append(f"--max-usd={self.max_usd}")
        return args


class Plan:
    """A plan the engine made (module docstring)."""

    def __init__(self, document: Mapping[str, Any], price: Mapping[str, Any]) -> None:
        self.document = dict(document)
        self.price = dict(price)
        self._request: _Request | None = None

    @property
    def ok(self) -> bool:
        """Whether the plan has no problems, so it may run."""
        return not self.problems

    @property
    def problems(self) -> list[dict[str, str]]:
        """Each problem: ``where`` and ``message``."""
        return [
            {"where": str(problem.get("where", "")), "message": str(problem.get("message", ""))}
            for problem in self.document.get("problems") or []
        ]

    def estimate(self) -> tuple[float, float]:
        """``(low, high)`` in US dollars: what the run may spend on what is not cached."""
        estimate = self.price.get("estimate") or self.document.get("estimate") or {}
        return (float(estimate.get("low_usd", 0)), float(estimate.get("high_usd", 0)))

    def phases(self) -> list[dict[str, Any]]:
        """The price by phase, as ``grida-fx price`` prints it: ``phase``, ``steps``, ``calls``
        ``[low, high]``, ``low_usd``, ``high_usd`` and ``then``."""
        return [dict(phase) for phase in self.price.get("phases") or []]

    def __repr__(self) -> str:
        workflow = (self.document.get("workflow") or {}).get("id", "?")
        low, high = self.estimate()
        return f"<Plan {workflow} ok={self.ok} estimate=${low:.2f}-${high:.2f}>"


# ------------------------------------------------------------------------------------------------
# Runs


class RunFile:
    """A file a run made: ``digest``, ``kind``, ``name``, ``size``, ``key``, and ``path``, its copy
    in the project's store (read it, never write it)."""

    def __init__(self, ref: Mapping[str, Any], store: Path) -> None:
        digest = ref.get("digest")
        if not isinstance(digest, str) or not _DIGEST.fullmatch(digest):
            raise FxError(f"the run's record names a file by {digest!r}, which is not a digest")
        self.digest = digest
        self.kind: str = ref.get("kind", "file")
        self.name: str = ref.get("name", "")
        self.size: int = ref.get("size", 0)
        self.key: str | None = ref.get("key")
        self.path = store / "files" / digest[:2] / digest

    def read_bytes(self) -> bytes:
        return self.path.read_bytes()

    def copy_to(self, target: str | Path) -> Path:
        """Copies the file to ``target`` (parents made) and returns ``target``."""
        target = Path(target)
        _place(self.path, target)
        return target

    def __repr__(self) -> str:
        return f"<RunFile {self.name or self.digest[:12]} {self.kind}>"


class StepResult:
    """One step's last finished take: ``id``, ``path``, ``cache`` (``hit`` or ``miss``),
    ``facts`` and ``outputs`` (decoded as :attr:`RunResult.outputs`)."""

    def __init__(self, event: Mapping[str, Any], outputs: dict[str, Any]) -> None:
        self.id: str = event.get("id", "")
        self.path: str = event.get("path", "")
        self.cache: str | None = event.get("cache")
        self.facts: dict[str, Any] = dict(event.get("facts") or {})
        self.outputs = outputs

    def __repr__(self) -> str:
        return f"<StepResult {self.id} facts={self.facts!r}>"


class RunResult:
    """A finished invocation (module docstring)."""

    def __init__(self, run_dir: Path, events: Sequence[Mapping[str, Any]]) -> None:
        self.run_dir = Path(run_dir)
        self.events = list(events)
        #: The store the run's files are in; found from the current directory when not set.
        self._store: Path | None = None

    def _ending(self) -> Mapping[str, Any] | None:
        """The last ``run_finished`` or ``run_cancelled``, whichever came last."""
        for event in reversed(self.events):
            if event.get("event") in ("run_finished", "run_cancelled"):
                return event
        return None

    def _finished(self) -> Mapping[str, Any] | None:
        ending = self._ending()
        return ending if ending is not None and ending.get("event") == "run_finished" else None

    @property
    def ok(self) -> bool:
        """Whether every step that had to run succeeded and the run ended normally."""
        finished = self._finished()
        return bool(finished is not None and finished.get("ok"))

    @property
    def incomplete(self) -> bool:
        """Whether the run stopped before everything ran (re-running continues it)."""
        finished = self._finished()
        return True if finished is None else bool(finished.get("incomplete"))

    @property
    def cost(self) -> float:
        """What the run folder has been charged, in US dollars, over every invocation."""
        ending = self._ending()
        return float(ending.get("charged_usd") or 0) if ending is not None else 0.0

    @property
    def failed(self) -> list[str]:
        """The ids of the instances that failed."""
        finished = self._finished()
        return [str(id_) for id_ in finished.get("failed") or []] if finished else []

    @property
    def outputs(self) -> dict[str, Any]:
        """Each declared output: a :class:`RunFile`, ``{key: value}`` for a keyed collection, a
        list, a plain value, or ``None``."""
        finished = self._finished()
        if finished is None:
            return {}
        store = self._store_root()
        return {
            name: _decode(value, store) for name, value in (finished.get("outputs") or {}).items()
        }

    @property
    def steps(self) -> dict[str, StepResult]:
        """Each step path's last finished take, e.g. ``result.steps["entity['k'].review"]``."""
        store = self._store_root()
        steps: dict[str, StepResult] = {}
        for event in self.events:
            if event.get("event") != "node_finished":
                continue
            outputs = {
                port: _decode(value, store) for port, value in (event.get("outputs") or {}).items()
            }
            steps[str(event.get("path", ""))] = StepResult(event, outputs)
        return steps

    def deliver(self, targets: Mapping[str, str], root: str | Path | None = None) -> list[str]:
        """Copies outputs out of the run: ``{output: path}``, with ``{key}`` in the path for each
        element of a collection or list (keys made safe as ``spec/store.md`` section 8 "Keys as
        paths" says). Paths are relative to ``root`` (default: the current directory). A file
        that already holds the same bytes is left alone. Returns the outputs that hold no file;
        nothing is copied when a path names ``{key}`` for one file, or one path for several."""
        base = Path(root) if root is not None else Path.cwd()
        outputs = self.outputs
        missing: list[str] = []
        copies: list[tuple[RunFile, Path]] = []
        for name, pattern in targets.items():
            value = outputs.get(name)
            files = _files_of(value)
            if not files:
                missing.append(name)
                continue
            keyed = "{key}" in pattern
            if isinstance(value, RunFile) and keyed:
                raise ValueError(f"{name} is one file, so no {{key}} in {pattern}")
            if len(files) > 1 and not keyed:
                raise ValueError(f"{name} holds {len(files)} files: name each one with {{key}}")
            for label, file in files:
                path = pattern.replace("{key}", _key_path(label or "")) if keyed else pattern
                copies.append((file, base / path))
        for file, target in copies:
            _place(file.path, target)
        return missing

    def _store_root(self) -> Path:
        return self._store if self._store is not None else _project_store(Path.cwd())

    def __repr__(self) -> str:
        return f"<RunResult {self.run_dir} ok={self.ok} cost=${self.cost:.2f}>"


def _decode(encoded: Any, store: Path) -> Any:
    """A value of the run's record (fx-run-events-v1 ``encoded``) as Python values."""
    if not isinstance(encoded, Mapping) or len(encoded) != 1:
        raise FxError(f"the run's record holds a value in no known form: {encoded!r}")
    ((form, value),) = encoded.items()
    if form == "file":
        return RunFile(value, store)
    if form == "collection":
        return {str(key): _decode(item, store) for key, item in value}
    if form == "list":
        return [_decode(item, store) for item in value]
    if form == "none":
        return None
    if form == "value":
        return value
    raise FxError(f"the run's record holds a value in no known form: {encoded!r}")


def _files_of(value: Any) -> list[tuple[str | None, RunFile]]:
    """An output's files with their labels: a key, or a list position."""
    if isinstance(value, RunFile):
        return [(None, value)]
    if isinstance(value, Mapping):
        return [(str(key), item) for key, item in value.items() if isinstance(item, RunFile)]
    if isinstance(value, list):
        return [
            (item.key or str(index), item)
            for index, item in enumerate(value)
            if isinstance(item, RunFile)
        ]
    return []


def _safe_name(text: str) -> str:
    """``spec/store.md`` section 8 ``<step>``: a character that is not a letter or a digit
    (Unicode L or N), ``.``, ``_`` or ``-`` becomes ``_``; ``_`` trimmed; empty is ``step``."""
    safe = "".join(
        char if unicodedata.category(char)[0] in "LN" or char in "._-" else "_" for char in text
    )
    return safe.strip("_") or "step"


def _key_path(key: str) -> str:
    """``spec/store.md`` section 8 "Keys as paths": split at ``/``, an empty, ``.`` or ``..``
    segment becomes ``_``, every other one :func:`_safe_name`."""
    return "/".join(
        "_" if segment in ("", ".", "..") else _safe_name(segment) for segment in key.split("/")
    )


def _place(source: Path, target: Path) -> None:
    """Copies ``source`` to ``target`` unless it already holds the same bytes, through a temporary
    name and a rename, so a file linked elsewhere (a store copy) is never written through."""
    data = source.read_bytes()
    if target.is_file() and target.stat().st_size == len(data) and target.read_bytes() == data:
        return
    target.parent.mkdir(parents=True, exist_ok=True)
    # Opened by name, not by mkstemp, so the copy gets the permissions a new file gets.
    temporary = target.parent / f".{target.name}.{secrets.token_hex(8)}.tmp"
    try:
        with open(temporary, "xb") as written:
            written.write(data)
        os.replace(temporary, target)
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(temporary)
        raise


# ------------------------------------------------------------------------------------------------
# The binary, the store, the request


def binary() -> Path:
    """The ``grida-fx`` binary to drive (module docstring)."""
    configured = os.environ.get(BINARY_VARIABLE)
    if configured:
        path = Path(configured).expanduser().absolute()
        if not path.is_file():
            raise RuntimeError(f"{BINARY_VARIABLE} is {configured}, which is not a file")
        return path
    packaged = _packaged_binary()
    if packaged.is_file():
        return packaged
    found = shutil.which("grida-fx")
    if found is not None:
        return Path(found)
    raise RuntimeError(
        f"no grida-fx binary was found: set {BINARY_VARIABLE} to its path, or put grida-fx on PATH"
    )


def _packaged_binary() -> Path:
    """Where the ``grida`` wheel carries the engine."""
    return Path(__file__).resolve().parent / "bin" / _EXECUTABLE


def _planning_start(request: _Request) -> Path:
    """Where the engine looks for the planning project (``spec/store.md``): the workflow file
    for a ``.yaml``/``.yml`` target (suffix compared as written), else ``cwd``."""
    if Path(request.target).suffix in (".yaml", ".yml"):
        return request.cwd / request.target
    return request.cwd


def _project_store(start: Path) -> Path:
    """The store of the project found from ``start`` (module docstring)."""
    here = start.resolve()
    if here.is_file():
        here = here.parent
    for folder in (here, *here.parents):
        candidate = folder / _PROJECT_FILE
        if candidate.is_file():
            return folder / _cache_setting(candidate)
    return here / _DEFAULT_CACHE


_TOP_KEY = re.compile(r"""(?:cache|"cache"|'cache')[ \t]*:(?:[ \t]+(?P<value>.*))?""")
#: What a YAML stream may not hold raw, beyond what JSON escapes itself.
_NOT_PRINTABLE = re.compile(r"[\x7f-\x9f\u2028\u2029\ufeff\ufffe\uffff]")
_DOUBLE_QUOTED = re.compile(r'"(?:[^"\\]|\\.)*"')
_SINGLE_QUOTED = re.compile(r"'(?:[^']|'')*'")


def _cache_setting(project_file: Path) -> str:
    """``cache`` of a project file, a top-level string (fx-project-v1), or the default. Only that
    one setting is read: the engine reads and checks the whole file."""
    try:
        text = project_file.read_text("utf-8-sig")
    except (OSError, UnicodeDecodeError):
        return _DEFAULT_CACHE
    with contextlib.suppress(ValueError):
        document = json.loads(text)
        if isinstance(document, dict):
            cache = document.get("cache")
            return cache if isinstance(cache, str) and cache else _DEFAULT_CACHE
    for line in text.splitlines():
        match = _TOP_KEY.fullmatch(line.rstrip())
        if match is None:
            continue
        value = (match["value"] or "").strip()
        quoted = _DOUBLE_QUOTED.match(value) or _SINGLE_QUOTED.match(value)
        if quoted is not None and value.startswith('"'):
            with contextlib.suppress(ValueError):
                value = json.loads(quoted.group())
        elif quoted is not None:
            value = quoted.group()[1:-1].replace("''", "'")
        else:
            value = re.split(r"[ \t]#", value, maxsplit=1)[0].strip()
            if value in ("", "~", "null"):
                value = ""
        return value or _DEFAULT_CACHE
    return _DEFAULT_CACHE


def _amount(option: str, value: Any) -> str:
    """An amount of US dollars as the command line takes it (decimal digits, no exponent)."""
    if isinstance(value, bool) or not isinstance(value, int | float | Decimal | str):
        raise TypeError(f"{option} is an amount of US dollars, not {type(value).__name__}")
    if isinstance(value, str):
        return value
    return format(Decimal(str(value)), "f")


def _target_text(target: Any) -> str:
    if isinstance(target, Workflow):
        raise TypeError(
            "grida.fx cannot plan a Workflow object yet: name the builder that returns it "
            "(file.py:function, with its arguments) or the workflow file"
        )
    if isinstance(target, os.PathLike):
        return os.fspath(target)
    if isinstance(target, str):
        return target
    raise TypeError(
        "a target is a workflow file, a workflow id, a builder file.py:function or a Plan, "
        f"not {type(target).__name__}"
    )


def _request(
    target: Any,
    *,
    inputs: Mapping[str, Any] | None,
    input_files: Sequence[str | Path],
    max_usd: float | None,
    cwd: str | Path | None,
    routes: Sequence[str | Path] | None,
    arguments: Mapping[str, str] | None,
) -> _Request:
    if isinstance(target, Plan):
        given = [
            name
            for name, value in (
                ("inputs", inputs),
                ("input_files", input_files or None),
                ("max_usd", max_usd),
                ("cwd", cwd),
                ("routes", routes or None),
                ("arguments", arguments or None),
            )
            if value is not None
        ]
        if given:
            raise TypeError(
                "a Plan runs with the target and options it was planned with, so it takes no "
                f"{', '.join(given)}: plan again to change them"
            )
        if target._request is None:
            raise ValueError("this Plan was not made by grida.fx.plan, so it names nothing to run")
        return target._request
    if inputs is not None and not isinstance(inputs, Mapping):
        raise TypeError("inputs is a mapping of input names to values")
    return _Request(
        target=_target_text(target),
        inputs=dict(inputs) if inputs else None,
        input_files=tuple(os.fspath(path) for path in input_files),
        max_usd=None if max_usd is None else _amount("max_usd", max_usd),
        cwd=Path(cwd).absolute() if cwd is not None else Path.cwd(),
        routes=tuple(os.fspath(path) for path in routes or ()),
        arguments=tuple((str(name), str(value)) for name, value in (arguments or {}).items()),
    )


def _plain_inputs(value: Any) -> Any:
    if isinstance(value, Mapping):
        plain = {}
        for key, item in value.items():
            if not isinstance(key, str):
                raise TypeError(f"keys in inputs are strings, not {type(key).__name__}")
            plain[key] = _plain_inputs(item)
        return plain
    if isinstance(value, list | tuple):
        return [_plain_inputs(item) for item in value]
    if isinstance(value, os.PathLike):
        return os.fspath(value)
    return value


def _inputs_text(inputs: Mapping[str, Any]) -> str:
    """``inputs`` as one line of JSON, which the YAML subset reads (``spec/yaml.md``). Characters
    a YAML stream may not hold raw (C1 controls, DEL, U+2028, U+2029, a byte-order mark, U+FFFE,
    U+FFFF) can only sit inside strings here, and are written as escapes."""
    text = json.dumps(_plain_inputs(inputs), ensure_ascii=False, allow_nan=False)
    return _NOT_PRINTABLE.sub(lambda match: f"\\u{ord(match.group()):04x}", text) + "\n"


@contextlib.contextmanager
def _inputs_file(request: _Request) -> Iterator[str | None]:
    """A temporary inputs file in ``cwd`` holding ``request.inputs``; its name, relative."""
    if not request.inputs:
        yield None
        return
    text = _inputs_text(request.inputs)
    handle, path = tempfile.mkstemp(prefix=".grida-fx-inputs-", suffix=".yaml", dir=request.cwd)
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as written:
            written.write(text)
        yield Path(path).name
    finally:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(path)


# ------------------------------------------------------------------------------------------------
# Driving the binary


@dataclass
class _Exit:
    status: int
    stdout: str
    stderr: str


class _Protocol(asyncio.subprocess.SubprocessStreamProtocol):
    """The stream protocol, also telling when the binary itself has exited. ``Process.wait`` and
    ``Process.communicate`` wait for its pipes to close as well, and a process a node body started
    in a session of its own (which outlives its host on purpose) keeps them open."""

    def __init__(self, loop: asyncio.AbstractEventLoop) -> None:
        super().__init__(limit=2**16, loop=loop)
        self.exited: asyncio.Future[None] = loop.create_future()

    def process_exited(self) -> None:
        super().process_exited()
        if not self.exited.done():
            self.exited.set_result(None)


async def _read_into(stream: asyncio.StreamReader | None, sink: bytearray) -> None:
    if stream is None:
        return
    while chunk := await stream.read(1 << 16):
        sink.extend(chunk)


async def _call(args: Sequence[str], cwd: Path) -> _Exit:
    """Runs the binary with ``args`` in ``cwd``. Cancelled, it interrupts the engine (which stops
    the run as Ctrl-C would) and kills it if it has not ended a while later. Its output is what it
    wrote until it exited: the call ends with the binary, not with the last holder of its pipes."""
    options: dict[str, Any] = {}
    if os.name == "posix":
        # Its own session: a Ctrl-C at the terminal reaches it once, through this function.
        options["start_new_session"] = True
    loop = asyncio.get_running_loop()
    transport, protocol = await loop.subprocess_exec(
        lambda: _Protocol(loop),
        str(binary()),
        *args,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        **options,
    )
    process = asyncio.subprocess.Process(transport, protocol, loop)
    stdout, stderr = bytearray(), bytearray()
    readers = [
        asyncio.ensure_future(_read_into(process.stdout, stdout)),
        asyncio.ensure_future(_read_into(process.stderr, stderr)),
    ]
    try:
        try:
            await asyncio.shield(protocol.exited)
        except BaseException:
            await _stop(process, protocol.exited)
            raise
        # What the binary wrote before it exited is in the pipes; read it, then stop reading.
        await asyncio.wait(readers, timeout=_DRAIN_S)
    finally:
        for reader in readers:
            reader.cancel()
        await asyncio.wait(readers)
        # Closing our ends kills nothing: the binary has exited.
        transport.close()
    return _Exit(
        process.returncode if process.returncode is not None else -1,
        stdout.decode("utf-8", "replace"),
        stderr.decode("utf-8", "replace"),
    )


async def _stop(process: asyncio.subprocess.Process, exited: asyncio.Future[None]) -> None:
    """Interrupts the engine, as Ctrl-C would, and kills it if it has not exited a while later.
    Its pipes are read meanwhile, so it never waits on them."""
    if exited.done():
        return
    with contextlib.suppress(ProcessLookupError):
        if os.name == "posix":
            process.send_signal(signal.SIGINT)
        else:
            process.terminate()
    try:
        await asyncio.wait_for(asyncio.shield(exited), _STOP_GRACE_S)
    except TimeoutError:
        with contextlib.suppress(ProcessLookupError):
            process.kill()
        await asyncio.shield(exited)
    except BaseException:
        # Stopped again while waiting: end it now.
        with contextlib.suppress(ProcessLookupError):
            process.kill()
        raise


def _error_of(done: _Exit) -> FxError:
    """The engine's error (exit status 2): its stderr, without the ``grida-fx:`` prefix."""
    text = done.stderr.strip() or done.stdout.strip()
    if text.startswith(_ERROR):
        text = text[len(_ERROR) :]
    return FxError(text or f"grida-fx exited with status {done.status}", done.status)


def _failure(verb: str, done: _Exit) -> FxError:
    if done.status == _USAGE_OR_ERROR:
        return _error_of(done)
    tail = (done.stderr.strip() or done.stdout.strip())[-2000:]
    return FxError(f"grida-fx {verb} exited with status {done.status}: {tail}", done.status)


async def _document(verb: str, args: Sequence[str], cwd: Path) -> dict[str, Any]:
    """The JSON document a planning verb prints (exit 0, or 1 for a plan with problems)."""
    done = await _call([verb, *args], cwd)
    if done.status not in (0, 1):
        raise _failure(verb, done)
    try:
        document = json.loads(done.stdout)
    except ValueError:
        raise _failure(verb, done) from None
    if not isinstance(document, dict):
        raise FxError(f"grida-fx {verb} printed no JSON object", done.status)
    return document


async def _plan(request: _Request) -> Plan:
    with _inputs_file(request) as inputs_file:
        args = request.args(inputs_file)
        graph = await _document("plan", [*args, "--json"], request.cwd)
        price = await _document("price", args, request.cwd)
    planned = Plan(graph, price)
    planned._request = request
    return planned


def _no_running_loop(function: str) -> None:
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return
    raise RuntimeError(
        f"grida.fx.{function} cannot be called inside a running event loop: "
        f"await grida.fx.{function}_async(...) instead"
    )


def _read_events(folder: Path) -> list[dict[str, Any]]:
    """The events of a run folder; a last line cut short (a crash mid-write) is left out."""
    path = folder / "events.jsonl"
    try:
        lines = path.read_text("utf-8").split("\n")
    except FileNotFoundError:
        raise FxError(f"{folder} holds no events.jsonl, so it is not a run folder") from None
    events = []
    last = max((index for index, line in enumerate(lines) if line.strip()), default=-1)
    for index, line in enumerate(lines):
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except ValueError:
            if index == last:
                break
            raise FxError(f"line {index + 1} of {path} is not a run event") from None
        if isinstance(event, dict):
            events.append(event)
    return events


async def plan_async(
    target: str | Plan,
    *,
    inputs: Mapping[str, Any] | None = None,
    input_files: Sequence[str | Path] = (),
    max_usd: float | None = None,
    cwd: str | Path | None = None,
    routes: Sequence[str | Path] | None = None,
    arguments: Mapping[str, str] | None = None,
) -> Plan:
    """Expands, checks and prices ``target``; never spends. ``plan.problems`` says what is
    wrong."""
    request = _request(
        target,
        inputs=inputs,
        input_files=input_files,
        max_usd=max_usd,
        cwd=cwd,
        routes=routes,
        arguments=arguments,
    )
    return await _plan(request)


def plan(
    target: str | Plan,
    *,
    inputs: Mapping[str, Any] | None = None,
    input_files: Sequence[str | Path] = (),
    max_usd: float | None = None,
    cwd: str | Path | None = None,
    routes: Sequence[str | Path] | None = None,
    arguments: Mapping[str, str] | None = None,
) -> Plan:
    """:func:`plan_async`, for code outside an event loop."""
    _no_running_loop("plan")
    return asyncio.run(
        plan_async(
            target,
            inputs=inputs,
            input_files=input_files,
            max_usd=max_usd,
            cwd=cwd,
            routes=routes,
            arguments=arguments,
        )
    )


async def run_async(
    target: str | Plan,
    *,
    inputs: Mapping[str, Any] | None = None,
    input_files: Sequence[str | Path] = (),
    live: bool = False,
    max_usd: float | None = None,
    yes_up_to: float | None = None,
    run_dir: str | Path | None = None,
    cwd: str | Path | None = None,
    routes: Sequence[str | Path] | None = None,
    arguments: Mapping[str, str] | None = None,
) -> RunResult:
    """Runs ``target`` (or a plan); ``live=True`` admits paid calls (module docstring)."""
    request = _request(
        target,
        inputs=inputs,
        input_files=input_files,
        max_usd=max_usd,
        cwd=cwd,
        routes=routes,
        arguments=arguments,
    )
    options = ["--live"] if live else []
    if yes_up_to is not None:
        options.append(f"--yes-up-to={_amount('yes_up_to', yes_up_to)}")
    if run_dir is not None:
        options.append(f"--run={os.fspath(run_dir)}")
    planned = target if isinstance(target, Plan) else await _plan(request)
    if not planned.ok:
        raise PlanRefused(planned)
    with _inputs_file(request) as inputs_file:
        done = await _call(["run", *request.args(inputs_file), *options], request.cwd)
    if done.status == _CANCELLED:
        raise FxError("the run was cancelled", done.status)
    if done.status not in (0, 1):
        raise _failure("run", done)
    named = [
        line[len(_RUN_LINE) :] for line in done.stdout.splitlines() if line.startswith(_RUN_LINE)
    ]
    if not named:
        refused = [line for line in done.stdout.splitlines() if line.startswith(_REFUSED)]
        if refused:
            raise FxError(refused[-1], done.status)
        replanned = await _plan(request)
        if not replanned.ok:
            raise PlanRefused(replanned)
        raise _failure("run", done)
    folder = request.cwd / (os.fspath(run_dir) if run_dir is not None else named[-1])
    result = RunResult(folder, _read_events(folder))
    result._store = _project_store(_planning_start(request))
    return result


def run(
    target: str | Plan,
    *,
    inputs: Mapping[str, Any] | None = None,
    input_files: Sequence[str | Path] = (),
    live: bool = False,
    max_usd: float | None = None,
    yes_up_to: float | None = None,
    run_dir: str | Path | None = None,
    cwd: str | Path | None = None,
    routes: Sequence[str | Path] | None = None,
    arguments: Mapping[str, str] | None = None,
) -> RunResult:
    """:func:`run_async`, for code outside an event loop."""
    _no_running_loop("run")
    return asyncio.run(
        run_async(
            target,
            inputs=inputs,
            input_files=input_files,
            live=live,
            max_usd=max_usd,
            yes_up_to=yes_up_to,
            run_dir=run_dir,
            cwd=cwd,
            routes=routes,
            arguments=arguments,
        )
    )
