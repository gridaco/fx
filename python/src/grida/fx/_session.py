"""The host's side of a session with requests both ways (``spec/protocol.md`` sections 1, 5, 6).

Step 2's host answered one request at a time on one thread. A ``run`` needs more: while it is
pending the host sends ``capability``, ``fact`` and the rest and reads their answers, and the
engine sends ``tool.invoke``, ``agent.check`` and ``$/cancel`` for the same run.

- **The reader** is the host's main thread: it reads every frame. A response resolves the
  :class:`concurrent.futures.Future` of the host request with its id (set directly by the
  reader, so a body blocked on a sync call never needs the event loop). A ``run`` starts the body
  on the body loop; ``tool.invoke`` and ``agent.check`` are scheduled on that loop;
  ``$/cancel`` marks the run's ``ctx.cancelled`` (and answers a pending ``tool.invoke`` or
  ``agent.check`` it names with ``cancelled``); ``describe``, ``build``, ``initialize`` and
  ``shutdown`` are answered on the reader as in step 2.
- **The body loop** is an asyncio event loop on a thread of its own. An ``async def`` body runs
  on it; a plain ``def`` body runs in a worker thread (``asyncio.to_thread``).
- **Writes** go through one lock, one whole frame at a time.
- **Host requests** are numbered from 1. :meth:`Channel.request` blocks the calling thread
  (any thread: the loop thread too, since the reader resolves futures itself);
  :meth:`Channel.request_async` awaits on the loop. An error answer raises the
  :class:`~grida.fx._errors.EngineError` of its code. Params the engine could not read (outside
  I-JSON, or nested deeper than its reader goes) raise ``ValueError`` and nothing is sent.
- A ``run`` is answered only after its own requests are answered (section 5.3).

When the session ends (end of input, ``exit``, a broken frame), :meth:`Session.close` fails every
host request still pending with :class:`~grida.fx._errors.Cancelled`, so no thread waits for an
answer that cannot come.
"""

from __future__ import annotations

import asyncio
import itertools
import threading
from concurrent.futures import Future, InvalidStateError, ThreadPoolExecutor
from dataclasses import dataclass
from typing import Any, BinaryIO

from grida.fx._errors import Cancelled, EngineError, engine_error
from grida.fx._protocol import INTERNAL, check_value, write_message


@dataclass
class _Pending:
    """A host request the engine has not answered yet."""

    run_id: str
    method: str
    #: What the caller waits on: the result, or the engine's error. A caller that stops waiting
    #: may cancel it.
    future: Future[Any]
    #: Resolved when the answer arrives (or the session ends), whatever the caller did: a run is
    #: answered only after every one of these for it.
    answered: Future[None]


class Channel:
    """Requests from a body to the engine, for one run (module docstring)."""

    def __init__(self, session: Session, run_id: str) -> None:
        self.session = session
        self.run_id = run_id

    def request(self, method: str, params: dict[str, Any]) -> Any:
        """Sends a request with this run's ``run_id`` and blocks for its result."""
        return self.session.request(self.run_id, method, params).result()

    async def request_async(self, method: str, params: dict[str, Any]) -> Any:
        """As :meth:`request`, awaited on the body loop."""
        return await asyncio.wrap_future(self.session.request(self.run_id, method, params))

    def notify(self, method: str, params: dict[str, Any]) -> None:
        """Sends a notification with this run's ``run_id`` (``progress``)."""
        message = {"jsonrpc": "2.0", "method": method, "params": {"run_id": self.run_id, **params}}
        _check(method, message["params"])
        self.session.send(message)


class Session:
    """The host's protocol stream: framed writes under a lock, pending host requests, and the body
    loop (module docstring)."""

    def __init__(self, reader: BinaryIO, writer: BinaryIO) -> None:
        self.reader = reader
        self.writer = writer
        self._write_lock = threading.Lock()
        self._lock = threading.Lock()
        self._ids = itertools.count(1)
        self._pending: dict[int, _Pending] = {}
        self._loop: asyncio.AbstractEventLoop | None = None
        self._closed = False

    def send(self, message: dict[str, Any]) -> None:
        """Writes one message as one frame."""
        with self._write_lock:
            write_message(self.writer, message)

    def request(self, run_id: str, method: str, params: dict[str, Any]) -> Future[Any]:
        """Sends the host request ``method`` for ``run_id`` and returns the future its answer
        resolves. ``ValueError`` (nothing sent) when the params are not an I-JSON value."""
        sent = {"run_id": run_id, **params}
        _check(method, sent)
        future: Future[Any] = Future()
        answered: Future[None] = Future()
        with self._lock:
            if self._closed:
                future.set_exception(Cancelled("the engine ended the session"))
                return future
            request_id = next(self._ids)
            self._pending[request_id] = _Pending(run_id, method, future, answered)
        try:
            self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": sent})
        except BaseException:
            with self._lock:
                self._pending.pop(request_id, None)
            answered.set_result(None)
            raise
        return future

    def deliver(self, message: dict[str, Any]) -> bool:
        """Resolves the pending host request a response answers; ``False`` when none is pending."""
        request_id = message.get("id")
        if isinstance(request_id, bool) or not isinstance(request_id, int | float | str):
            return False
        with self._lock:
            pending = self._pending.pop(request_id, None)  # type: ignore[arg-type]
        if pending is None:
            return False
        if "error" in message:
            _resolve(pending.future, error=_engine_error(pending.method, message["error"]))
        elif "result" in message:
            _resolve(pending.future, result=message["result"])
        else:
            _resolve(
                pending.future,
                error=EngineError(
                    f"the engine answered {pending.method} with neither a result nor an error",
                    code=INTERNAL,
                ),
            )
        _resolve(pending.answered, result=None)
        return True

    def pending(self, run_id: str) -> int:
        """How many host requests of ``run_id`` the engine has not answered."""
        with self._lock:
            return sum(1 for pending in self._pending.values() if pending.run_id == run_id)

    async def settle(self, run_id: str) -> None:
        """Waits until the engine has answered every host request of ``run_id`` (section 5.3: a
        run is not answered while one of its own requests is pending)."""
        while True:
            with self._lock:
                waiting = [
                    pending.answered
                    for pending in self._pending.values()
                    if pending.run_id == run_id
                ]
            if not waiting:
                return
            await asyncio.wait([asyncio.wrap_future(answered) for answered in waiting])

    def loop(self) -> asyncio.AbstractEventLoop:
        """The body loop, started on a thread of its own the first time it is needed."""
        with self._lock:
            if self._loop is None:
                loop = asyncio.new_event_loop()
                # The worker threads of `def` bodies and tools. Their module came in with this
                # one, before the project root went on sys.path: imported lazily, on the first
                # run, its `queue` could be a project module.
                loop.set_default_executor(ThreadPoolExecutor(thread_name_prefix="grida.fx body"))
                threading.Thread(
                    target=_serve_loop, args=(loop,), name="grida.fx body loop", daemon=True
                ).start()
                self._loop = loop
            return self._loop

    def close(self) -> None:
        """Ends the session: every pending host request fails with ``Cancelled`` and later ones
        fail at once."""
        with self._lock:
            self._closed = True
            pending = list(self._pending.values())
            self._pending.clear()
        for entry in pending:
            _resolve(entry.future, error=Cancelled("the engine ended the session"))
            _resolve(entry.answered, result=None)

    def channel(self, run_id: str) -> Channel:
        return Channel(self, run_id)


def _serve_loop(loop: asyncio.AbstractEventLoop) -> None:
    asyncio.set_event_loop(loop)
    loop.run_forever()


def _check(method: str, params: dict[str, Any]) -> None:
    """Refuses params the engine could not read: outside I-JSON, or nested deeper than it reads
    (params sit one level inside their message)."""
    try:
        check_value(params, depth=1)
    except ValueError as error:
        raise ValueError(f"{method}: {error}") from None


def _engine_error(method: str, error: Any) -> EngineError:
    if isinstance(error, dict):
        code = error.get("code")
        message = error.get("message")
        if isinstance(code, int) and not isinstance(code, bool) and isinstance(message, str):
            return engine_error(code, message, error.get("data"))
    return EngineError(f"the engine answered {method} with a malformed error", code=INTERNAL)


def _resolve(
    future: Future[Any], *, result: Any = None, error: BaseException | None = None
) -> None:
    """Resolves ``future`` unless its waiter already gave up on it (cancelled it)."""
    try:
        if error is not None:
            future.set_exception(error)
        else:
            future.set_result(result)
    except InvalidStateError:  # cancelled by its waiter, or already resolved
        pass
