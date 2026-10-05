"""What a body receives: :class:`Ctx`, rehosted over the node protocol (``spec/protocol.md``
section 8).

A ``Ctx`` is built by the host for one ``run`` (section 5.3) from its params and a
:class:`~grida.fx._session.Channel` to the engine. Members (every one stage-gen's bodies use):

- ``ctx.instance``: ``id``, ``path``, ``step``, ``key``, ``takes`` (the ``take`` list) and
  ``take`` (its last number).
- ``ctx.params``: ``run.params`` with each ``param_files`` entry put back at its JSON pointer as
  an :class:`InputFile`.
- ``ctx.inputs``: each staged input as an :class:`InputFile`, a list of them, or a ``dict`` by key
  (a keyed collection, in order). An absent optional input is not a key.
- ``ctx.read.bytes/text/json/annotations/image(name)``: local reads of one input file's ``path``
  (``input <name> is not one file`` otherwise, as :class:`NodeFailure`); ``text`` decodes strict
  UTF-8 and removes one leading byte-order mark (``spec/identity.md`` section 5); ``image`` opens
  the file with Pillow (``Image.open``, ``load``, a copy), as ``grida.fx.std.pictures.read``.
- ``ctx.out``: ``bytes(data, kind)``, ``text(text, kind="text/plain")``, ``json(value)``,
  ``png(picture)`` (Pillow, ``grida.fx.std.pictures.png``), ``path(name)`` (``work_path("out/" +
  name)``), ``file(path, kind=None)``. Each returns an :class:`Output`; an output leaves the host
  through ``file.put`` (``json`` for ``ctx.out.json``; a file under ``work_dir`` by
  ``work_path``; other bytes as ``base64``) when it is returned, put in a request, or shown to an
  agent.
- ``ctx.work_path(name)``: a path under ``run.work_dir`` (parents made); a name that leaves it
  raises ``NodeFailure("<name> is outside this node's work folder")``.
- ``ctx.fact(name, value)``, ``ctx.annotate(shape=None, label=None, color=None, tag=None,
  **fields)``, ``ctx.progress(text, fraction=None)``: the ``fact``, ``annotate`` and ``progress``
  requests; ``ctx.facts`` and ``ctx.marks`` mirror what was reported (bodies read them).
  ``annotate`` keeps the predecessor's local refusals (``a mark's shape is one of point, points,
  box``; ``a <shape> mark needs <geometry>=``).
- ``ctx.prompt(path, **variables)``: ``prompt.render`` (path must be a declared resource:
  ``NodeFailure("<path> is not one of this node's declared resources")`` locally first).
- ``ctx.tool(name)``: a :class:`ToolHandle`; ``.executable`` is ``run.tools[name]`` as a ``Path``
  (an attribute), ``.run(argv, *, cwd=None, timeout_s=None, env=None, check=True)`` runs it in a
  session of its own (``<name> ran past <s> seconds``, ``<name> exited <code>: <tail>``); an
  undeclared name or a missing program is a :class:`NodeFailure` (``<name> is not one of this
  node's declared tools``, ``<name> is not installed (see grida-fx doctor)``).
- ``await ctx.capability(name, **request)``, ``ctx.image_generate``, ``ctx.image_edit``,
  ``ctx.structured_generate``: the ``capability`` request; files anywhere in the request (an
  :class:`InputFile`, an :class:`Output`, a ``CallResult`` file) become ``{"file": digest}``
  (outputs ``file.put`` first). Returns a :class:`CallResult`.
- ``ctx.agent(*, system, tools=(), recent_images=None, max_tokens=None)``: an
  :class:`~grida.fx._agent.Agent`.
- ``ctx.state``: a dict shared by the body and its tools; never sent.
- ``ctx.cancelled``: set when the engine sends ``$/cancel`` for this run.
- ``ctx.fail(message)``: returns a :class:`NodeFailure` to raise.

Two things differ from the predecessor by design: an output made by ``ctx.out.file`` of a file
under the work folder is read by the engine when the run is answered (the predecessor read it at
once), and ``ctx.out.json`` keeps a copy of the value, which the engine writes in the format of
``spec/identity.md`` section 5.
"""

from __future__ import annotations

import base64
import contextlib
import copy
import json
import os
import shutil
import signal
import subprocess
from collections.abc import Mapping, Sequence
from pathlib import Path, PurePath
from typing import TYPE_CHECKING, Any

from grida.fx._errors import CallFailed, NodeFailure
from grida.fx._protocol import check_value

if TYPE_CHECKING:
    from grida.fx._agent import Agent
    from grida.fx._session import Channel

#: A file's kind by its suffix, compared case-insensitively (``spec/identity.md`` section 4).
_KINDS = {
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".webp": "image/webp",
    ".gif": "image/gif",
    ".md": "text/markdown",
    ".txt": "text/plain",
    ".json": "json",
    ".yaml": "text/yaml",
    ".yml": "text/yaml",
    ".toml": "text/toml",
    ".html": "text/html",
    ".wav": "audio/wav",
    ".mp3": "audio/mpeg",
    ".ogg": "audio/ogg",
    ".mp4": "video/mp4",
    ".webm": "video/webm",
    ".mkv": "video/x-matroska",
    ".glb": "model/gltf-binary",
    ".gltf": "model/gltf+json",
    ".fbx": "model/fbx",
    ".zip": "file/zip",
}

#: Mark shapes and the field each one needs (``spec/protocol.md`` section 6.4).
SHAPES = ("point", "points", "box")
_GEOMETRY = {"point": "at", "points": "points", "box": "box"}


def kind_of(path: str | PurePath) -> str:
    """The kind of a file by its suffix (``spec/identity.md`` section 4); ``file`` otherwise."""
    return _KINDS.get(PurePath(path).suffix.lower(), "file")


class InputFile:
    """A file the engine handed the run (a file ref, section 3.1)."""

    def __init__(self, ref: Mapping[str, Any]) -> None:
        self.ref = dict(ref)

    @property
    def path(self) -> Path:
        """The store copy: read it, never write it."""
        return Path(self.ref["path"])

    @property
    def kind(self) -> str:
        return str(self.ref["kind"])

    @property
    def digest(self) -> str:
        return str(self.ref["digest"])

    @property
    def name(self) -> str:
        return str(self.ref.get("name", ""))

    @property
    def size(self) -> int:
        return int(self.ref["size"])

    @property
    def key(self) -> str | None:
        return self.ref.get("key")

    @property
    def facts(self) -> dict[str, Any]:
        """The file facts the engine computed (``spec/identity.md`` section 4)."""
        return dict(self.ref.get("facts") or {})

    def read_bytes(self) -> bytes:
        return self.path.read_bytes()

    def copy_to(self, target: str | Path) -> Path:
        """Copies the file to ``target`` (parents made) and returns ``target``."""
        destination = Path(target)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(self.path, destination)
        return destination

    def __eq__(self, other: object) -> bool:
        return isinstance(other, InputFile) and other.ref == self.ref

    def __hash__(self) -> int:
        return hash(self.ref.get("digest"))

    def __repr__(self) -> str:
        return f"InputFile(name={self.name!r}, kind={self.ref.get('kind')!r}, digest={self.digest})"


class Output:
    """A file a body made, held until it leaves the host (module docstring).

    Exactly one source is set: ``data`` (bytes), ``work_path`` (a file under the run's work
    folder, POSIX and relative to it), or else ``json`` (a JSON value, ``None`` included)."""

    def __init__(
        self,
        *,
        kind: str,
        data: bytes | None = None,
        json: Any = None,
        work_path: str | None = None,
    ) -> None:
        self.kind = kind
        self.data = data
        self.json = json
        self.work_path = work_path

    def __repr__(self) -> str:
        if self.work_path is not None:
            source = f"work_path={self.work_path!r}"
        elif self.data is not None:
            source = f"{len(self.data)} bytes"
        else:
            source = "json"
        return f"Output(kind={self.kind!r}, {source})"


class CallResult:
    """A capability's answer (section 6.1): ``files`` by name, ``data``, ``cost_usd``,
    ``cached``, ``key``. ``image``, ``audio`` and ``video`` are the first file of that family
    (``CallFailed("the call returned no <family>")`` otherwise); ``json`` is ``data``; ``pil()``
    opens ``image`` with Pillow."""

    def __init__(self, result: Mapping[str, Any]) -> None:
        self.result = dict(result)

    @property
    def files(self) -> dict[str, InputFile]:
        return {name: InputFile(ref) for name, ref in (self.result.get("files") or {}).items()}

    @property
    def data(self) -> Any:
        return self.result.get("data")

    @property
    def cost_usd(self) -> float | None:
        return self.result.get("cost_usd")

    @property
    def cached(self) -> bool:
        return bool(self.result.get("cached", False))

    @property
    def key(self) -> str | None:
        return self.result.get("key")

    @property
    def image(self) -> InputFile:
        return self._first("image")

    @property
    def audio(self) -> InputFile:
        return self._first("audio")

    @property
    def video(self) -> InputFile:
        return self._first("video")

    @property
    def json(self) -> Any:
        return self.data

    def pil(self) -> Any:
        from grida.fx.std import pictures

        return pictures.read(self.image.path)

    def _first(self, family: str) -> InputFile:
        for file in self.files.values():
            if file.kind.split("/", 1)[0] == family:
                return file
        raise CallFailed(f"the call returned no {family}")

    def __repr__(self) -> str:
        return f"CallResult(files={list(self.result.get('files') or {})}, cached={self.cached})"


class Instance:
    """``ctx.instance``: ``id``, ``path``, ``step``, ``key``, ``takes``, ``take``."""

    def __init__(self, instance: Mapping[str, Any]) -> None:
        self.id: str = instance["id"]
        self.path: str = instance["path"]
        self.step: str = instance["step"]
        self.key: str | None = instance.get("key")
        self.takes: tuple[int, ...] = tuple(instance["take"])
        self.take: int = self.takes[-1] if self.takes else 1


class ToolHandle:
    """``ctx.tool(name)``: ``executable`` and ``run`` (module docstring). ``work_dir`` is where
    ``run`` starts the program when no ``cwd`` is given (the run's work folder)."""

    def __init__(self, name: str, executable: Path | None, *, work_dir: Path | None = None) -> None:
        self.name = name
        self.executable = executable
        self.work_dir = work_dir

    def run(
        self,
        argv: Sequence[str],
        *,
        cwd: str | Path | None = None,
        timeout_s: float | None = None,
        env: Mapping[str, str] | None = None,
        check: bool = True,
    ) -> subprocess.CompletedProcess[str]:
        """Runs the program in a session of its own. At ``timeout_s`` the program and everything
        it started end (``SIGKILL`` to its process group) and the node fails with ``<name> ran
        past <s> seconds``. With ``check`` (the default) a non-zero exit fails the node with
        ``<name> exited <code>: `` and the last 2000 characters of its stderr (or stdout);
        without, the caller reads the exit. ``env`` replaces the environment when given."""
        if self.executable is None:
            raise NodeFailure(f"{self.name} is not installed (see grida-fx doctor)")
        process = subprocess.Popen(
            [str(self.executable), *argv],
            cwd=cwd or self.work_dir,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=None if env is None else dict(env),
            start_new_session=True,
        )
        try:
            stdout, stderr = process.communicate(timeout=timeout_s)
        except subprocess.TimeoutExpired:
            _end_session(process)
            raise NodeFailure(f"{self.name} ran past {timeout_s} seconds") from None
        except BaseException:
            _end_session(process)
            raise
        completed = subprocess.CompletedProcess(process.args, process.returncode, stdout, stderr)
        if check and completed.returncode != 0:
            tail = (completed.stderr or completed.stdout or "").strip()[-2_000:]
            raise NodeFailure(f"{self.name} exited {completed.returncode}: {tail}")
        return completed

    def __repr__(self) -> str:
        return f"ToolHandle({self.name!r}, {str(self.executable)!r})"


def _end_session(process: subprocess.Popen[str]) -> None:
    """Ends a program started in a session of its own, with everything it started."""
    with contextlib.suppress(ProcessLookupError, PermissionError):
        if hasattr(os, "killpg"):
            os.killpg(process.pid, signal.SIGKILL)
        else:  # pragma: no cover - no process groups on this platform
            process.kill()
    process.communicate()


class _Read:
    """``ctx.read``."""

    def __init__(self, ctx: Ctx) -> None:
        self._ctx = ctx

    def _one(self, name: str) -> InputFile:
        value = self._ctx.inputs.get(name)
        if not isinstance(value, InputFile):
            raise NodeFailure(f"input {name} is not one file")
        return value

    def bytes(self, name: str) -> bytes:
        return self._one(name).read_bytes()

    def text(self, name: str) -> str:
        return self.bytes(name).decode("utf-8").removeprefix("﻿")

    def json(self, name: str) -> Any:
        return json.loads(self.bytes(name))

    def annotations(self, name: str) -> Any:
        return self.json(name)

    def image(self, name: str) -> Any:
        from grida.fx.std import pictures

        return pictures.read(self._one(name).path)


class _Out:
    """``ctx.out``."""

    def __init__(self, ctx: Ctx) -> None:
        self._ctx = ctx

    def bytes(self, data: bytes, kind: str) -> Output:
        return Output(kind=kind, data=bytes(data))

    def text(self, text: str, kind: str = "text/plain") -> Output:
        return Output(kind=kind, data=text.encode("utf-8"))

    def json(self, value: Any) -> Output:
        try:
            check_value(value)
        except ValueError as error:
            raise ValueError(f"ctx.out.json takes a JSON value: {error}") from None
        return Output(kind="json", json=copy.deepcopy(value))

    def png(self, picture: Any) -> Output:
        from grida.fx.std import pictures

        return Output(kind="image/png", data=pictures.png(picture))

    def path(self, name: str) -> Path:
        return self._ctx.work_path(f"out/{name}")

    def file(self, path: str | Path, kind: str | None = None) -> Output:
        """A file the body wrote: a file under the work folder leaves by ``work_path`` (read when
        it leaves); any other file's bytes are read now."""
        given = Path(path)
        resolved = given.resolve()
        work_dir = self._ctx._work_dir().resolve()
        if resolved.is_relative_to(work_dir) and resolved != work_dir:
            if not resolved.is_file():
                raise FileNotFoundError(f"{path} is not a file")
            relative = resolved.relative_to(work_dir).as_posix()
            return Output(kind=kind or kind_of(given), work_path=relative)
        return Output(kind=kind or kind_of(given), data=given.read_bytes())


class Ctx:
    """What a body receives (module docstring)."""

    def __init__(self, run: Mapping[str, Any], channel: Channel) -> None:
        self.run = dict(run)
        self.channel = channel
        self.instance = Instance(run["instance"])
        self.state: dict[str, Any] = {}
        self.facts: dict[str, Any] = {}
        self.marks: list[dict[str, Any]] = []
        self.cancelled = False
        self.read = _Read(self)
        self.out = _Out(self)
        self._params: dict[str, Any] | None = None
        self._inputs: dict[str, Any] | None = None
        #: Outputs already stored with ``file.put`` in this run, by identity: ``(output, ref)``.
        self._stored: dict[int, tuple[Output, dict[str, Any]]] = {}
        #: Agents whose ``agent.run`` is pending, by ``agent_id`` (``tool.invoke`` finds them).
        self._agents: dict[str, Agent] = {}
        self._agent_count = 0

    # -- what the run was given ------------------------------------------------------------

    @property
    def params(self) -> dict[str, Any]:
        if self._params is None:
            params = copy.deepcopy(dict(self.run.get("params") or {}))
            for pointer, ref in (self.run.get("param_files") or {}).items():
                _put_at(params, pointer, InputFile(ref))
            self._params = params
        return self._params

    @property
    def inputs(self) -> dict[str, Any]:
        if self._inputs is None:
            self._inputs = {
                name: _staged(value) for name, value in (self.run.get("inputs") or {}).items()
            }
        return self._inputs

    def _work_dir(self) -> Path:
        return Path(self.run["work_dir"])

    def work_path(self, name: str | Path) -> Path:
        work_dir = self._work_dir().resolve()
        path = (work_dir / name).resolve()
        if not path.is_relative_to(work_dir):
            raise NodeFailure(f"{name} is outside this node's work folder")
        path.parent.mkdir(parents=True, exist_ok=True)
        return path

    # -- reporting ---------------------------------------------------------------------------

    def fact(self, name: str, value: Any) -> None:
        """Reports a node fact; the engine records it at once (section 6.3)."""
        if not isinstance(name, str):
            raise TypeError(f"a fact's name is a string, not {type(name).__name__}")
        try:
            check_value(value)
        except ValueError as error:
            raise ValueError(f"fact {name}: {error}") from None
        self.channel.request("fact", {"name": name, "value": value})
        self.facts[name] = value

    def annotate(
        self,
        *,
        shape: str | None = None,
        label: str | None = None,
        color: str | None = None,
        tag: str | None = None,
        **fields: Any,
    ) -> None:
        """Adds one mark to this node's annotations (section 6.4). ``shape`` is ``point``
        (``at=[x, y]``), ``points`` (``points=[[x, y], ...]``, ``closed``) or ``box``
        (``box=[x0, y0, x1, y1]``), in fractions of the image; no shape is a note about the
        whole image."""
        mark: dict[str, Any] = {}
        if shape is not None:
            if shape not in SHAPES:
                raise NodeFailure(f"a mark's shape is one of {', '.join(SHAPES)}")
            mark["shape"] = shape
            geometry = _GEOMETRY[shape]
            if geometry not in fields:
                raise NodeFailure(f"a {shape} mark needs {geometry}=")
        for name, value in (("label", label), ("color", color), ("tag", tag)):
            if value is not None:
                mark[name] = value
        mark.update(fields)
        try:
            check_value(mark)
        except ValueError as error:
            raise ValueError(f"a mark: {error}") from None
        self.channel.request("annotate", {"mark": mark})
        self.marks.append(mark)

    def progress(self, text: str, fraction: float | None = None) -> None:
        params: dict[str, Any] = {"text": text}
        if fraction is not None:
            params["fraction"] = fraction
        self.channel.notify("progress", params)

    # -- the engine's services ---------------------------------------------------------------

    def prompt(self, path: str, **variables: Any) -> str:
        """Renders a declared resource with ``${{ }}`` over the params, ``variables`` on top
        (section 6.6)."""
        if path not in (self.run.get("resources") or {}):
            raise NodeFailure(f"{path} is not one of this node's declared resources")
        outputs = _outputs_in(variables)
        stored = {id(output): self._put(output, "prompt/file")["digest"] for output in outputs}
        answer = self.channel.request(
            "prompt.render", {"path": path, "variables": _with_files(variables, stored)}
        )
        return str(answer["text"])

    def tool(self, name: str) -> ToolHandle:
        tools = self.run.get("tools") or {}
        if name not in tools:
            raise NodeFailure(f"{name} is not one of this node's declared tools")
        executable = tools[name]
        if executable is None:
            raise NodeFailure(f"{name} is not installed (see grida-fx doctor)")
        return ToolHandle(name, Path(executable), work_dir=self.work_path("."))

    async def capability(self, name: str, **request: Any) -> CallResult:
        """A paid call through the engine (section 6.1): routed, priced, budgeted, retried by
        the engine, recorded and call-cached."""
        outputs = _outputs_in(request)
        stored = {}
        for output in outputs:
            stored[id(output)] = (await self._put_async(output, "request/file"))["digest"]
        answer = await self.channel.request_async(
            "capability", {"capability": name, "request": _with_files(request, stored)}
        )
        return CallResult(answer)

    async def image_generate(self, **request: Any) -> CallResult:
        return await self.capability("image.generate", **request)

    async def image_edit(self, **request: Any) -> CallResult:
        return await self.capability("image.edit", **request)

    async def structured_generate(self, **request: Any) -> CallResult:
        return await self.capability("structured.generate", **request)

    def agent(
        self,
        *,
        system: str,
        tools: Sequence[Any] = (),
        recent_images: int | None = None,
        max_tokens: int | None = None,
    ) -> Agent:
        from grida.fx._agent import Agent

        return Agent(
            self, system=system, tools=tools, recent_images=recent_images, max_tokens=max_tokens
        )

    def fail(self, message: str) -> NodeFailure:
        return NodeFailure(message)

    # -- files leaving the host --------------------------------------------------------------

    def _put_params(self, output: Output, name: str) -> dict[str, Any]:
        """The ``file.put`` params that store ``output`` (section 6.7)."""
        params: dict[str, Any]
        if output.work_path is not None:
            params = {"work_path": output.work_path}
        elif output.data is not None:
            params = {"base64": base64.b64encode(output.data).decode("ascii")}
        else:
            params = {"json": output.json}
        params["kind"] = output.kind
        params["name"] = name
        return params

    def _known(self, output: Output) -> dict[str, Any] | None:
        stored = self._stored.get(id(output))
        return stored[1] if stored is not None and stored[0] is output else None

    def _remember(self, output: Output, ref: dict[str, Any]) -> dict[str, Any]:
        # A file under the work folder may change after it is stored: it is stored again.
        if output.work_path is None:
            self._stored[id(output)] = (output, ref)
        return ref

    def _put(self, output: Output, name: str) -> dict[str, Any]:
        """Stores ``output`` with ``file.put`` (once per run) and returns its file ref."""
        known = self._known(output)
        if known is not None:
            return known
        return self._remember(
            output, self.channel.request("file.put", self._put_params(output, name))
        )

    async def _put_async(self, output: Output, name: str) -> dict[str, Any]:
        """As :meth:`_put`, awaited on the body loop."""
        known = self._known(output)
        if known is not None:
            return known
        ref = await self.channel.request_async("file.put", self._put_params(output, name))
        return self._remember(output, ref)

    def _next_agent_id(self) -> str:
        self._agent_count += 1
        return f"agent-{self._agent_count}"


def _staged(value: Mapping[str, Any]) -> Any:
    """A staged input (section 3.3) as the body sees it."""
    if "list" in value and "digest" not in value:
        return [InputFile(ref) for ref in value["list"]]
    if "collection" in value and "digest" not in value:
        return {key: InputFile(ref) for key, ref in value["collection"]}
    return InputFile(value)


def _put_at(params: dict[str, Any], pointer: str, file: InputFile) -> None:
    """Puts ``file`` at the RFC 6901 ``pointer`` into ``params``."""
    tokens = [token.replace("~1", "/").replace("~0", "~") for token in pointer.split("/")[1:]]
    if not pointer.startswith("/") or not tokens:
        raise ValueError(f"param_files names {pointer!r}, which is not a pointer into params")
    target: Any = params
    try:
        for token in tokens[:-1]:
            target = target[int(token)] if isinstance(target, list) else target[token]
        last = tokens[-1]
        if isinstance(target, list):
            target[int(last)] = file
        elif isinstance(target, dict):
            target[last] = file
        else:
            raise TypeError(type(target).__name__)
    except (KeyError, IndexError, ValueError, TypeError):
        raise ValueError(f"param_files names {pointer}, which is not in params") from None


def _outputs_in(value: Any, found: list[Output] | None = None) -> list[Output]:
    """Every :class:`Output` inside lists, tuples and mappings of ``value``, each once."""
    if found is None:
        found = []
    if isinstance(value, Output):
        if all(output is not value for output in found):
            found.append(value)
    elif isinstance(value, Mapping):
        for item in value.values():
            _outputs_in(item, found)
    elif isinstance(value, list | tuple):
        for item in value:
            _outputs_in(item, found)
    return found


def _with_files(value: Any, stored: Mapping[int, str]) -> Any:
    """``value`` with every file in it as a file value (section 3.2): an :class:`InputFile` (an
    input, a param file, a capability result's file) by its digest, an :class:`Output` by the
    digest it was stored under (``stored``, by identity)."""
    if isinstance(value, InputFile):
        return {"file": value.digest}
    if isinstance(value, Output):
        return {"file": stored[id(value)]}
    if isinstance(value, Mapping):
        return {key: _with_files(item, stored) for key, item in value.items()}
    if isinstance(value, list | tuple):
        return [_with_files(item, stored) for item in value]
    return value
