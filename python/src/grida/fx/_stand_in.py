"""Stand-ins (``spec/protocol.md`` sections 5.7 and 8): what the SDK's two answerers share.

A stand-in is a function of one :class:`StandInCall`, a plain one or a coroutine function, that
answers the paid calls of a stand-in run in place of a provider. ``grida.fx.run(...,
stand_in=answer)`` serves it in the caller's own process, over a socket pair on the engine's
standard input (:mod:`grida.fx._api`); ``grida-fx run --stand-in file.py#answer`` serves it from a
stand-in host (:mod:`grida.fx.host`). Both answer ``stand_in.answer`` through an
:class:`Answerer`:

- **The call.** The params become a :class:`StandInCall`: ``capability``, ``route`` (``id``,
  ``fingerprint``), ``key``, ``takes`` (the ``take`` list) and ``take`` (its last number, as
  ``ctx.instance.take``), ``instance`` (``id``, ``path``, ``step``), ``files`` (an
  :class:`~grida.fx.InputFile` by digest), ``request`` (a copy of the request in which every file
  value, an object of exactly the shape ``{"file": <digest>}`` at any depth, ``spec/identity.md``
  section 3, is the ``InputFile`` of ``files``) and ``params`` (the params as they came). Params
  of another shape, or a file value ``files`` does not hold, are answered ``-32602``.
- **One at a time.** Calls are answered one at a time, in the order their requests came (a lock
  that wakes its waiters in order). A coroutine function is awaited on the answerer's loop; a plain
  function runs on a worker thread while the loop goes on reading. The thread is a daemon thread of
  its own, so a function that never returns cannot hold the process when it ends.
- **The answer.** An :class:`Answer` goes out as ``{files: {name: {base64, kind?}}, data}``:
  ``bytes`` leave ``kind`` out (the engine gives the file the kind its capability names), an
  :class:`~grida.fx.Output` of bytes or an ``InputFile`` gives its own kind, and a path
  (``os.PathLike``) its bytes with the kind of its suffix (``spec/identity.md`` section 4).
  :data:`DECLINE` goes out as ``{"decline": true}``. ``CallRefused`` is answered
  ``capability_refused``, and ``CallFailed`` ``call_failed``, each with its message.
- **Faults.** Anything else the function raises (``BaseException`` included), a return value that
  is neither an ``Answer`` nor ``DECLINE`` (``None`` too), and an answer the SDK cannot send (a file
  that is not bytes, a file name that is not ``[a-z][a-z0-9_]*``, ``data`` the engine cannot read)
  are faults of the stand-in: answered ``internal`` with ``<Type>: <message>``, which stops the
  run. The first fault is kept as :attr:`Answerer.fault`, and every later call is answered
  ``internal`` (``the stand-in failed earlier``) without calling the function.
- **Cancelling.** ``$/cancel`` of a call waiting for its turn, or of a call a coroutine function is
  answering, cancels it, answered ``cancelled``. A plain function already running finishes, and its
  answer goes out; the engine discards it. What it raises then is no fault, since the engine no
  longer waits for the call: it is answered ``cancelled``, as a coroutine function's is.
- **Sending.** Every error answer can be sent: a lone surrogate in its message (a file name that
  is not UTF-8, say) is written as its escape (``\\udcff``), so no call goes unanswered.
"""

from __future__ import annotations

import asyncio
import base64
import contextvars
import importlib.metadata
import inspect
import os
import platform
import re
import threading
from collections.abc import Awaitable, Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Final

from grida.fx._ctx import InputFile, Output, kind_of
from grida.fx._errors import CallFailed, CallRefused
from grida.fx._protocol import (
    CANCELLED,
    INTERNAL,
    INVALID_PARAMS,
    PROTOCOL,
    PROTOCOL_MISMATCH,
    check_value,
    encodable_text,
)

#: A file's name in an answer (fx-node-protocol-v1 ``stand_in_answer_result``).
_FILE_NAME = re.compile(r"[a-z][a-z0-9_]*")
#: A file digest: what a file value holds (``spec/identity.md`` section 3).
_DIGEST = re.compile(r"[0-9a-f]{64}")
#: The answer to a call the engine cancelled, as the host words ``$/cancel``'s other answers.
_CANCELLED = "the engine cancelled stand_in.answer"


# ------------------------------------------------------------------------------------------------
# The call


@dataclass(frozen=True)
class StandInRoute:
    """``call.route``: the route's ``id`` (``model@provider``) and its ``fingerprint``."""

    id: str
    fingerprint: str


@dataclass(frozen=True)
class StandInInstance:
    """``call.instance``: the ``id``, ``path`` and ``step`` of the instance making the call."""

    id: str
    path: str
    step: str


class StandInCall:
    """One paid call a stand-in is asked to answer (module docstring). ``ValueError`` for params
    that are not ``stand_in.answer``'s."""

    def __init__(self, params: Mapping[str, Any]) -> None:
        if not isinstance(params, Mapping):
            raise ValueError("stand_in.answer's params are a JSON object")
        capability = params.get("capability")
        if not isinstance(capability, str) or not capability:
            raise ValueError("stand_in.answer's capability is a capability's name")
        route = params.get("route")
        if not isinstance(route, Mapping) or not all(
            isinstance(route.get(name), str) for name in ("id", "fingerprint")
        ):
            raise ValueError("stand_in.answer's route is {id, fingerprint}")
        take = params.get("take")
        if (
            not isinstance(take, list)
            or not take
            or not all(isinstance(n, int) and not isinstance(n, bool) and n >= 1 for n in take)
        ):
            raise ValueError("stand_in.answer's take is a list of take numbers")
        key = params.get("key")
        if not isinstance(key, str) or not key:
            raise ValueError("stand_in.answer's key is the call key")
        instance = params.get("instance")
        if not isinstance(instance, Mapping) or not all(
            isinstance(instance.get(name), str) for name in ("id", "path", "step")
        ):
            raise ValueError("stand_in.answer's instance is {id, path, step}")
        files = params.get("files")
        if not isinstance(files, Mapping) or not all(
            isinstance(digest, str)
            and isinstance(ref, Mapping)
            and isinstance(ref.get("path"), str)
            for digest, ref in files.items()
        ):
            raise ValueError("stand_in.answer's files are file refs by digest")
        request = params.get("request")
        if not isinstance(request, Mapping):
            raise ValueError("stand_in.answer's request is a JSON object")
        #: The params as they came, the request's file values as file values.
        self.params: dict[str, Any] = dict(params)
        self.capability: str = capability
        self.route = StandInRoute(id=route["id"], fingerprint=route["fingerprint"])
        self.key: str = key
        self.takes: tuple[int, ...] = tuple(take)
        self.take: int = self.takes[-1]
        self.instance = StandInInstance(
            id=instance["id"], path=instance["path"], step=instance["step"]
        )
        self.files: dict[str, InputFile] = {digest: InputFile(ref) for digest, ref in files.items()}
        self.request: dict[str, Any] = _with_input_files(request, self.files)

    def __repr__(self) -> str:
        return (
            f"<StandInCall {self.capability} on {self.route.id} for {self.instance.id}"
            f" take {self.take}>"
        )


def _with_input_files(value: Any, files: Mapping[str, InputFile]) -> Any:
    """A copy of a JSON value with every file value (an object whose only member is ``file``
    with a digest) as the :class:`InputFile` of its digest."""
    if isinstance(value, Mapping):
        digest = value.get("file")
        if len(value) == 1 and isinstance(digest, str) and _DIGEST.fullmatch(digest):
            if digest not in files:
                raise ValueError(f"the request holds the file {digest}, which files does not hold")
            return files[digest]
        return {key: _with_input_files(item, files) for key, item in value.items()}
    if isinstance(value, list):
        return [_with_input_files(item, files) for item in value]
    return value


# ------------------------------------------------------------------------------------------------
# The answer


class Decline:
    """The type of :data:`DECLINE`."""

    __slots__ = ()

    def __repr__(self) -> str:
        return "grida.fx.DECLINE"


#: What a stand-in returns to decline a call: it goes on as in a run without a stand-in, and
#: fails ``not_live`` (``spec/protocol.md`` section 6.1).
DECLINE: Final[Decline] = Decline()


@dataclass(frozen=True)
class Answer:
    """A stand-in's answer: ``files`` by name (``bytes``, an :class:`~grida.fx.Output` of bytes,
    an :class:`~grida.fx.InputFile`, or a path) and ``data``, a JSON value or ``None``
    (module docstring)."""

    files: Mapping[str, bytes | Output | InputFile | os.PathLike[str]] = field(default_factory=dict)
    data: Any = None

    @classmethod
    def json(cls, value: Any) -> Answer:
        """A ``structured.generate`` answer: ``data`` of ``{"json": value}``."""
        return cls(data={"json": value})

    @classmethod
    def turn(cls, text: str = "", tool_calls: Sequence[Mapping[str, Any]] = ()) -> Answer:
        """An ``agent.turn`` answer (``spec/protocol.md`` section 6.2): ``text``, and the tool
        calls, each ``{name, arguments?, id?}``; a call without an ``id`` is ``call_<n>``, counted
        from 1 in the turn."""
        calls = [
            {
                "id": call.get("id") or f"call_{number}",
                "name": call["name"],
                "arguments": dict(call.get("arguments") or {}),
            }
            for number, call in enumerate(tool_calls, 1)
        ]
        return cls(data={"text": text, "tool_calls": calls})

    @classmethod
    def submit(cls, **arguments: Any) -> Answer:
        """An ``agent.turn`` answer that calls ``submit`` with ``arguments``."""
        return cls.turn(tool_calls=[{"name": "submit", "arguments": arguments}])


#: A stand-in: a function of one call, plain or a coroutine function.
StandIn = Callable[[StandInCall], Answer | Decline | Awaitable[Answer | Decline]]


def encode(returned: Any) -> dict[str, Any]:
    """What a stand-in returned as ``stand_in.answer``'s result. ``TypeError`` or ``ValueError``
    for anything the SDK cannot send (module docstring)."""
    if isinstance(returned, Decline):
        return {"decline": True}
    if not isinstance(returned, Answer):
        given = "None" if returned is None else type(returned).__name__
        raise TypeError(f"a stand-in returns an Answer or grida.fx.DECLINE, not {given}")
    if not isinstance(returned.files, Mapping):
        raise TypeError("an answer's files are a mapping of names to files")
    files: dict[str, Any] = {}
    for name, value in returned.files.items():
        if not isinstance(name, str) or not _FILE_NAME.fullmatch(name):
            raise ValueError(
                f"an answer's files are named in lowercase ([a-z][a-z0-9_]*), not {name!r}"
            )
        files[name] = _encode_file(name, value)
    try:
        # The data sits in the result of a response: two levels inside its message.
        check_value(returned.data, "/data", depth=2)
    except ValueError as error:
        raise ValueError(f"an answer's data is not a JSON value FX can read: {error}") from None
    return {"files": files, "data": returned.data}


def _encode_file(name: str, value: Any) -> dict[str, Any]:
    if isinstance(value, bytes | bytearray | memoryview):
        return {"base64": _base64(bytes(value))}
    if isinstance(value, Output):
        if value.data is None:
            raise TypeError(f"an answer's file is bytes: {name} is {value!r}")
        return {"base64": _base64(value.data), "kind": value.kind}
    if isinstance(value, InputFile):
        return {"base64": _base64(value.read_bytes()), "kind": value.kind}
    if isinstance(value, os.PathLike):
        path = Path(os.fsdecode(os.fspath(value)))
        return {"base64": _base64(path.read_bytes()), "kind": kind_of(path)}
    raise TypeError(
        f"an answer's file is bytes, an Output, an InputFile or a path; {name} is"
        f" {type(value).__name__}"
    )


def _base64(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


# ------------------------------------------------------------------------------------------------
# Answering


class Refused(Exception):
    """An error answer to a request: ``error`` is its JSON-RPC error object, whose message can
    always be sent (a lone surrogate in it is written as its escape)."""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.error: dict[str, Any] = {"code": code, "message": encodable_text(message or "error")}


def error_text(error: BaseException) -> str:
    """``<Type>: <message>``; ``<Type>`` alone when the message is empty, as Python prints it."""
    message = str(error)
    return f"{type(error).__name__}: {message}" if message else type(error).__name__


class Answerer:
    """Answers ``stand_in.answer`` for one stand-in on the loop that calls :meth:`start` (module
    docstring). ``error_text`` names a fault in its answer; ``on_fault`` is told of the first."""

    def __init__(
        self,
        function: StandIn,
        *,
        error_text: Callable[[BaseException], str] = error_text,
        on_fault: Callable[[BaseException], None] | None = None,
    ) -> None:
        if not callable(function):
            raise TypeError(
                f"a stand-in is a function of one StandInCall, not {type(function).__name__}"
            )
        self.function = function
        #: The first fault of the stand-in, kept for the caller to raise.
        self.fault: BaseException | None = None
        self._error_text = error_text
        self._on_fault = on_fault
        self._lock = asyncio.Lock()
        #: The calls being answered, by request id; those on a worker thread; those the engine
        #: cancelled.
        self._asked: dict[Any, asyncio.Task[dict[str, Any]]] = {}
        self._on_thread: set[Any] = set()
        self._cancelled: set[Any] = set()

    def start(self, request_id: Any, params: Any, reply: Callable[[dict[str, Any]], None]) -> None:
        """Starts answering one ``stand_in.answer``. ``reply`` gets ``{"result": …}`` or
        ``{"error": …}`` once, unless :meth:`close` ends the call first. Called on the loop."""
        task = asyncio.get_running_loop().create_task(self._answer(request_id, params))
        self._asked[request_id] = task
        task.add_done_callback(lambda done: self._answered(request_id, done, reply))

    def cancel(self, request_id: Any) -> None:
        """``$/cancel``: cancels the call unless a plain function is answering it, which is only
        marked cancelled (module docstring); a request id that names no call is ignored. Called on
        the loop."""
        task = self._asked.get(request_id)
        if task is None or task.done():
            return
        self._cancelled.add(request_id)
        if request_id not in self._on_thread:
            task.cancel()

    def close(self) -> None:
        """Ends every call being answered, answering none of them: the session is over."""
        for task in list(self._asked.values()):
            task.cancel()

    def _answered(
        self, request_id: Any, task: asyncio.Task[dict[str, Any]], reply: Callable[..., None]
    ) -> None:
        self._asked.pop(request_id, None)
        self._on_thread.discard(request_id)
        cancelled = request_id in self._cancelled
        self._cancelled.discard(request_id)
        if task.cancelled():
            if cancelled:
                reply({"error": {"code": CANCELLED, "message": _CANCELLED}})
            return
        error = task.exception()
        if isinstance(error, Refused):
            reply({"error": error.error})
        elif error is not None:  # pragma: no cover - _answer turns every exception into one
            reply({"error": {"code": INTERNAL, "message": error_text(error)}})
        else:
            reply({"result": task.result()})

    async def _answer(self, request_id: Any, params: Any) -> dict[str, Any]:
        try:
            call = StandInCall(params)
        except ValueError as error:
            raise Refused(INVALID_PARAMS, str(error)) from None
        async with self._lock:
            if self.fault is not None:
                raise Refused(INTERNAL, "the stand-in failed earlier")
            try:
                return encode(await self._call(request_id, call))
            except CallRefused as refused:
                raise Refused(CallRefused.code, refused.message or type(refused).__name__) from None
            except CallFailed as failed:
                raise Refused(CallFailed.code, failed.message or type(failed).__name__) from None
            except asyncio.CancelledError as cancelled:
                task = asyncio.current_task()
                if task is not None and task.cancelling():
                    raise  # cancelled by the engine, or the session ended
                raise self._faulted(cancelled) from None
            except BaseException as error:  # whatever else the stand-in raised is its fault
                if request_id in self._cancelled:
                    # Raised as the engine cancelled stand_in.answer, which nobody waits for now.
                    raise asyncio.CancelledError() from None
                raise self._faulted(error) from None

    def _faulted(self, error: BaseException) -> Refused:
        if self.fault is None:
            self.fault = error
            if self._on_fault is not None:
                self._on_fault(error)
        return Refused(INTERNAL, self._error_text(error))

    async def _call(self, request_id: Any, call: StandInCall) -> Any:
        if inspect.iscoroutinefunction(self.function):
            returned = await self.function(call)
        else:
            self._on_thread.add(request_id)
            try:
                returned = await _on_thread(self.function, call)
            finally:
                self._on_thread.discard(request_id)
        if inspect.isawaitable(returned):
            returned = await returned
        return returned


def _on_thread(function: Callable[[StandInCall], Any], call: StandInCall) -> asyncio.Future[Any]:
    """Calls ``function(call)`` on a daemon thread of its own (module docstring), in a copy of
    the caller's context; the future the loop awaits."""
    loop = asyncio.get_running_loop()
    future: asyncio.Future[Any] = loop.create_future()
    context = contextvars.copy_context()

    def settle(result: Any, error: BaseException | None) -> None:
        if future.done():
            return  # the call was ended meanwhile
        if error is not None:
            future.set_exception(error)
        else:
            future.set_result(result)

    def work() -> None:
        try:
            result, error = context.run(function, call), None
        except BaseException as raised:  # the stand-in's, handed to the loop
            result, error = None, raised
        try:
            loop.call_soon_threadsafe(settle, result, error)
        except RuntimeError:
            pass  # the loop has closed: nobody waits for the answer

    threading.Thread(target=work, name="grida.fx stand-in", daemon=True).start()
    return future


def initialize(params: Any) -> dict[str, Any]:
    """``initialize``'s result for an answerer that loads nothing (``spec/protocol.md`` section
    2); :class:`Refused` for a protocol other than this SDK's."""
    protocol = params.get("protocol") if isinstance(params, Mapping) else None
    if not isinstance(protocol, str):
        raise Refused(INVALID_PARAMS, "initialize needs the engine's protocol")
    version = _sdk_version()
    if protocol != PROTOCOL:
        refused = Refused(
            PROTOCOL_MISMATCH, f"the engine speaks {protocol} and grida {version} speaks {PROTOCOL}"
        )
        refused.error["data"] = {"engine_protocol": protocol, "host_protocol": PROTOCOL}
        raise refused
    host = {"language": "python", "version": platform.python_version(), "sdk_version": version}
    return {"protocol": PROTOCOL, "host": host}


def _sdk_version() -> str:
    try:
        return importlib.metadata.version("grida")
    except importlib.metadata.PackageNotFoundError:
        return "unknown"
