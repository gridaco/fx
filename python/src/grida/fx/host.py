"""The Python node host: ``python -P -m grida.fx.host`` (``spec/protocol.md``).

``-P`` keeps the working directory (the project root) off ``sys.path``, so a project module named
like a standard library module or like ``grida`` cannot replace the host's own imports; the root
goes on ``sys.path`` at ``initialize``, once the host has imported everything it needs. A
``PYTHONSAFEPATH`` the engine set (the value ``grida-fx``) is removed from the environment, so
programs user code starts see the user's. Before importing any user code the host duplicates file
descriptor 1 for the protocol and points descriptor 1 (and ``sys.stdout``) at stderr, so whatever
user code prints never reaches the protocol stream (section 1). It then reads frames from stdin
and answers:

- ``initialize`` (section 2): must come first (anything else before it: ``-32600``); a protocol
  other than ``fx-node-protocol-v1`` is answered ``protocol_mismatch`` with ``data``
  ``{engine_protocol, host_protocol}``; the project root goes first on ``sys.path``; the result is
  ``{protocol, host: {language: "python", version, sdk_version}}`` (``sdk_version`` the ``grida``
  distribution's version).
- ``describe`` (section 5.1): for each target, load the module once per session (as a module
  named after its project-relative path, the project root on ``sys.path``; outside the root is an
  error), then report ``{path, types: [{attribute, spec}], closure: [{label, path}]}`` or
  ``{path, attribute?, error}`` with errors such as ``nodes/x.py failed to import:
  ModuleNotFoundError: No module named 'foo'`` and ``nodes/x.py: echo is not declared with
  @node``. ``builtins: true`` reports none: the engine declares every built-in, and runs the
  bodies of ``grida.fx.std`` through ``run``.
- ``build`` (section 5.2): with the working directory set to ``cwd``, load the builder file and
  call ``function(**arguments)``, then restore the working directory; ``load_failed`` (``no
  builder file …``, ``… has no function …``, import failures) and ``build_failed``
  (``<function>: <Type>: <message>``; a result that is not a ``Workflow``: ``<function>
  returned <type>, not a grida.fx.Workflow``); the result is
  ``{document, takes_anchor}``, the anchor the project-relative path of ``Workflow.source`` (or
  ``path`` when it is unknown or outside the project).
- ``run`` (section 5.3), one at a time (another while one is pending: ``-32600``; so are
  ``describe`` and ``build``): the body is the project module's attribute (loaded once per session
  as ``describe`` loads it; ``load_failed`` when it is missing or not declared with ``@node``) or
  the ``grida.fx.std`` body of a built-in (``load_failed`` for one this host has no body for). It
  gets a :class:`~grida.fx._ctx.Ctx` over the session's channel for the run; an ``async def``
  body runs on the body loop, a ``def`` body in a worker thread (:mod:`grida.fx._session`). Its
  outputs (``None`` is none) become output values (section 3.4): an ``InputFile`` or a
  capability result's file by ``{"file": ref}``; an ``Output`` of a file under the work folder by
  ``{"work_path", "kind"}``, any other ``Output`` stored with ``file.put`` first; a list as
  ``{"list": …}``; a ``dict`` on a keyed port (or any port of a built-in, whose ports the engine
  checks) as ``{"collection": [[key, value], …]}``. Anything else fails the node (``output <label>
  is <type>; use ctx.out to make it``, ``<name> returned <type>, not its outputs``). The answer
  is ``{outputs}`` (facts and marks reached the engine with ``fact`` and ``annotate``), sent only
  once the engine has answered every request of the run. A ``NodeFailure`` is answered
  ``node_failure`` with the body's ``facts`` and ``marks`` in ``data``; an ``EngineError`` the
  body let propagate keeps its code and message; anything else is ``node_error`` with the
  exception's own message (its type when it has none) and ``{exception, traceback, facts,
  marks}``.
- ``tool.invoke`` and ``agent.check`` (sections 5.4, 5.5) for an agent of the pending run, served
  on the body loop by :class:`~grida.fx._agent.Agent` (an unknown run or agent: ``-32602``); a
  tool's ``NodeFailure`` is answered ``node_failure``, a check's unexpected exception
  ``node_error``.
- ``$/cancel`` (section 5.6): for the pending run, sets ``ctx.cancelled`` (the body decides what
  to do); for a pending ``tool.invoke`` or ``agent.check``, answers it ``cancelled`` at once and
  cancels its task; for a pending ``stand_in.answer``, cancels it as
  :class:`~grida.fx._stand_in.Answerer` says.
- ``stand_in.load`` (section 5.7), for a stand-in host, which the engine starts in its own working
  directory: loads the file at the absolute ``path`` as a module of its own, ``_fx_stand_in``, not
  as a project module (it may lie outside the project), with the file's folder first on
  ``sys.path`` as ``python <file>`` has it, and finds ``function``; ``{}``, or ``load_failed``
  (``no stand-in file …``, ``… failed to import: …``, ``… has no function …``), the file named
  relative to the working directory when it is inside it. From then on ``describe``, ``build`` and
  ``run`` are answered ``-32600`` (``this host answers for a stand-in``).
- ``stand_in.answer`` (section 5.7), once a stand-in is loaded: answered on the body loop by an
  :class:`~grida.fx._stand_in.Answerer` (several may be pending; they are answered one at a time,
  in order). A fault of the stand-in is answered ``internal`` and its traceback logged on stderr.
- ``shutdown`` → ``null`` (a pending run still finishes and is answered); then the ``exit``
  notification ends the process with status 0 (1 if no ``shutdown`` came first). End of stdin
  ends the process. The process ends with ``os._exit`` once the protocol stream is flushed and
  closed, so a thread user code left running cannot keep it alive.
- an unknown method: ``-32601``; a host fault: ``internal``.

The host also points descriptor 0 at the null device once it holds its own duplicate of stdin,
so a program user code starts never reads protocol bytes. A process forked from the host (a
body's ``os.fork()``, ``multiprocessing`` with the fork start method) gets the null device in
place of both protocol streams: it can neither write into the protocol nor keep the engine's pipes
open after the host has gone. When the engine that started the host is gone (the host's parent
changed: the engine was killed, so no end of input may ever be read while user code keeps the
host busy), the host ends at once, with its process group when it leads one (the engine starts
each host as the leader of a group of its own), so nothing a body started outlives the engine.
Messages the host writes name project files by their project-relative paths: the project root
is cut from the texts of user errors (and from a ``node_error``'s traceback). Whatever user code
raises while a module loads or a builder runs (``KeyboardInterrupt`` and
``asyncio.CancelledError`` included) is that module's error or that build's failure.
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import hashlib
import importlib.metadata
import importlib.util
import inspect
import os
import platform
import re
import signal
import sys
import threading
import time
import traceback
from collections.abc import Callable, Coroutine, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any, BinaryIO

from grida.fx import std
from grida.fx._agent import Agent
from grida.fx._builder import Workflow
from grida.fx._closure import ClosureError, source_closure
from grida.fx._ctx import Ctx, InputFile, Output
from grida.fx._errors import EngineError, NodeFailure
from grida.fx._protocol import (
    BUILD_FAILED,
    CANCELLED,
    ERROR_CODES,
    INTERNAL,
    INVALID_PARAMS,
    INVALID_REQUEST,
    LOAD_FAILED,
    METHOD_NOT_FOUND,
    NODE_ERROR,
    NODE_FAILURE,
    PARSE_ERROR,
    PROTOCOL,
    PROTOCOL_MISMATCH,
    ProtocolError,
    check_value,
    encodable_text,
    parse_message,
    read_message,
)
from grida.fx._session import Session
from grida.fx._spec import NodeSpec, SpecError, spec_of
from grida.fx._stand_in import Answerer

#: A project path on the wire: POSIX, relative, with no empty, ``.`` or ``..`` segment.
_SEGMENT = r"(?:[^/\\.][^/\\]*|\.[^/\\.][^/\\]*|\.\.[^/\\]+)"
_PROJECT_PATH = re.compile(_SEGMENT + r"(?:/" + _SEGMENT + r")*")
_SOURCE_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_FUNCTION = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
#: The ``PYTHONSAFEPATH`` value the engine sets when the user's environment has none.
SAFE_PATH_MARK = "grida-fx"
#: What a request handler returns when it answers later, from the body loop.
_LATER = object()
#: The name a stand-in file is loaded under (section 5.7).
_STAND_IN_MODULE = "_fx_stand_in"
#: Frames a ``node_error`` traceback starts after: the SDK's own (not the bodies of
#: ``grida.fx.std``), and the machinery that ran the body (the event loop, the worker thread).
_SDK = os.path.dirname(os.path.abspath(__file__)) + os.sep
_STD = os.path.join(os.path.dirname(os.path.abspath(__file__)), "std") + os.sep
_MACHINERY = (
    os.path.dirname(asyncio.__file__) + os.sep,
    os.path.dirname(concurrent.futures.__file__) + os.sep,
)


@dataclass
class _Run:
    """The pending ``run``."""

    request_id: Any
    run_id: str
    ctx: Ctx


class _Refusal(Exception):
    """A request the host answers with an error."""

    def __init__(self, code: int, message: str, data: dict[str, Any] | None = None) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.data = data


class _LoadError(Exception):
    """A module that could not be loaded; its text is the error the engine reports."""


class Host:
    """One host session."""

    def __init__(self, reader: BinaryIO, writer: BinaryIO) -> None:
        self.reader = reader
        self.writer = writer
        self.initialized = False
        self.shutting_down = False
        self.project_root: str | None = None
        self.sources: list[str] = []
        self.modules: dict[str, Any] = {}
        #: Modules that failed to load, by project-relative path: each is loaded at most once.
        self._failures: dict[str, str] = {}
        #: The project root as the engine wrote it and as resolved, longest first: cut from texts.
        self._root_texts: list[str] = []
        self.session = Session(reader, writer)
        #: Guards the pending run and the requests served on the body loop.
        self._lock = threading.Lock()
        self._run: _Run | None = None
        #: ``tool.invoke`` and ``agent.check`` requests being served: id -> (method, task).
        self._serving: dict[Any, tuple[str, concurrent.futures.Future[None] | None]] = {}
        #: The stand-in this host answers for, once ``stand_in.load`` loaded it.
        self._stand_in: Answerer | None = None

    # -- the session -------------------------------------------------------------------------

    def serve(self) -> int:
        """Serves until ``exit`` or end of input; returns the exit status."""
        try:
            while True:
                try:
                    body = read_message(self.reader)
                except ProtocolError as error:
                    _log(f"grida.fx.host: {error}")
                    self._send_error(None, PARSE_ERROR, str(error))
                    return 1
                if body is None:
                    return 0 if self.shutting_down else 1
                try:
                    message = parse_message(body)
                except ProtocolError as error:
                    self._send_error(None, PARSE_ERROR, str(error))
                    continue
                status = self._dispatch(message)
                if status is not None:
                    return status
        finally:
            self.session.close()

    def _dispatch(self, message: Any) -> int | None:
        """Handles one message; an exit status when the session ends."""
        if not isinstance(message, dict):
            self._send_error(None, INVALID_REQUEST, "a message is a JSON object")
            return None
        request_id = message.get("id")
        known_id = request_id if _valid_id(request_id) else None
        if message.get("jsonrpc") != "2.0":
            self._send_error(known_id, INVALID_REQUEST, 'a message carries "jsonrpc": "2.0"')
            return None
        if "method" not in message:
            # A response to one of this host's requests (a run's).
            if not self.session.deliver(message):
                _log(f"grida.fx.host: ignored a response to id {request_id!r}")
            return None
        method = message["method"]
        if not isinstance(method, str):
            self._send_error(known_id, INVALID_REQUEST, "a message's method is a string")
            return None
        if "id" not in message:
            return self._notify(method, message.get("params"))
        if known_id is None:
            self._send_error(None, INVALID_REQUEST, "a request id is an integer or a string")
            return None
        try:
            result = self._request(method, message.get("params"), known_id)
            if result is _LATER:
                return None
        except _Refusal as refusal:
            self._send_error(known_id, refusal.code, refusal.message, refusal.data)
            return None
        except Exception as error:
            _log("".join(traceback.format_exception(error)))
            self._send_error(known_id, INTERNAL, f"the node host failed: {self._error_text(error)}")
            return None
        self._send_result(known_id, result)
        return None

    def _notify(self, method: str, params: Any) -> int | None:
        if method == "exit":
            return 0 if self.shutting_down else 1
        if method == "$/cancel":
            self._cancel(params)
        # Other notifications are ignored.
        return None

    def _request(self, method: str, params: Any, request_id: Any) -> Any:
        if method == "initialize":
            if self.initialized:
                raise _Refusal(INVALID_REQUEST, "initialize was already answered")
            return self.initialize(_params(params))
        if not self.initialized:
            raise _Refusal(INVALID_REQUEST, f"{method} came before initialize")
        # Part of the pending run, which shutdown lets finish.
        if method == "tool.invoke":
            return self.tool_invoke(request_id, _params(params))
        if method == "agent.check":
            return self.agent_check(request_id, _params(params))
        if self.shutting_down:
            raise _Refusal(INVALID_REQUEST, f"{method} came after shutdown")
        if method == "shutdown":
            if params is not None:
                raise _Refusal(INVALID_PARAMS, "shutdown takes no params")
            self.shutting_down = True
            return None
        if method == "stand_in.load":
            return self.stand_in_load(_params(params))
        if method == "stand_in.answer":
            return self.stand_in_answer(request_id, _params(params))
        if method in ("describe", "build", "run") and self._stand_in is not None:
            raise _Refusal(INVALID_REQUEST, "this host answers for a stand-in")
        if method in ("describe", "build", "run") and self._pending_run() is not None:
            raise _Refusal(INVALID_REQUEST, f"{method} came while a run is pending")
        if method == "describe":
            return self.describe(_params(params))
        if method == "build":
            return self.build(_params(params))
        if method == "run":
            return self.run(request_id, _params(params))
        raise _Refusal(METHOD_NOT_FOUND, f"the Python node host has no method {method}")

    def _send_result(self, request_id: Any, result: Any) -> None:
        try:
            self.session.send({"jsonrpc": "2.0", "id": request_id, "result": result})
        except ValueError as error:
            self._send_error(
                request_id, INTERNAL, f"the node host's answer is not an I-JSON value: {error}"
            )

    def _send_error(
        self, request_id: Any, code: int, message: str, data: dict[str, Any] | None = None
    ) -> None:
        error: dict[str, Any] = {"code": code, "message": encodable_text(message or "error")}
        if data is not None:
            error["data"] = data
        self.session.send({"jsonrpc": "2.0", "id": request_id, "error": error})

    def _answer(
        self, request_id: Any, result: Any = None, error: dict[str, Any] | None = None
    ) -> None:
        """Answers a request served on the body loop; a broken stream is logged (the engine
        then sees the host exit)."""
        try:
            if error is None:
                self._send_result(request_id, result)
                return
            try:
                self._send_error(request_id, error["code"], error["message"], error.get("data"))
            except ValueError:
                # data that is not an I-JSON value (a body changed ctx.facts by hand): left out.
                self._send_error(request_id, error["code"], error["message"])
        except ValueError as unsent:
            # The last resort: an answer the engine can read, so it waits for none.
            try:
                self._send_error(request_id, INTERNAL, f"the answer could not be sent: {unsent}")
            except (OSError, ValueError) as broken:
                _log(f"grida.fx.host: could not answer request {request_id!r}: {broken}")
        except OSError as broken:
            _log(f"grida.fx.host: could not answer request {request_id!r}: {broken}")

    # -- initialize --------------------------------------------------------------------------

    def initialize(self, params: dict[str, Any]) -> dict[str, Any]:
        protocol = params.get("protocol")
        if not isinstance(protocol, str):
            raise _Refusal(INVALID_PARAMS, "initialize needs the engine's protocol")
        if protocol != PROTOCOL:
            raise _Refusal(
                PROTOCOL_MISMATCH,
                f"the engine speaks {protocol} and grida {_sdk_version()} speaks {PROTOCOL}",
                {"engine_protocol": protocol, "host_protocol": PROTOCOL},
            )
        root_text = params.get("project_root")
        if not isinstance(root_text, str) or not root_text or not os.path.isabs(root_text):
            raise _Refusal(INVALID_PARAMS, "project_root is the project's absolute path")
        root = Path(root_text).resolve()
        if not root.is_dir():
            raise _Refusal(INVALID_PARAMS, "project_root is not a folder")
        sources = params.get("sources")
        if not isinstance(sources, list) or not all(
            isinstance(name, str) and _SOURCE_NAME.fullmatch(name) for name in sources
        ):
            raise _Refusal(INVALID_PARAMS, "sources is a list of package names")
        self.project_root = str(root)
        given = root_text.rstrip(os.sep) or root_text
        self._root_texts = sorted({given, str(root)}, key=len, reverse=True)
        self.sources = list(sources)
        # Read before the project root goes on sys.path, which the host's own imports never see.
        host = {
            "language": "python",
            "version": platform.python_version(),
            "sdk_version": _sdk_version(),
        }
        # User imports resolve against the project root first.
        sys.path[:] = [entry for entry in sys.path if entry != self.project_root]
        sys.path.insert(0, self.project_root)
        self.initialized = True
        return {"protocol": PROTOCOL, "host": host}

    # -- describe ----------------------------------------------------------------------------

    def describe(self, params: dict[str, Any]) -> dict[str, Any]:
        targets = params.get("targets")
        if not isinstance(targets, list):
            raise _Refusal(INVALID_PARAMS, "describe needs targets: a list")
        if not isinstance(params.get("builtins"), bool):
            raise _Refusal(INVALID_PARAMS, "describe needs builtins: true or false")
        checked: list[tuple[str, str | None]] = []
        for target in targets:
            if not isinstance(target, dict):
                raise _Refusal(INVALID_PARAMS, "a describe target is {path, attribute?}")
            path = _project_path(target.get("path"), "a describe target's path")
            attribute = target.get("attribute")
            if attribute is not None and (not isinstance(attribute, str) or not attribute):
                raise _Refusal(INVALID_PARAMS, "a describe target's attribute is a name")
            checked.append((path, attribute))
        modules = [self._describe_target(path, attribute) for path, attribute in checked]
        # The engine declares every built-in; the bodies of grida.fx.std run through `run`.
        return {"modules": modules, "builtins": []}

    def _describe_target(self, path: str, attribute: str | None) -> dict[str, Any]:
        def failed(error: str) -> dict[str, Any]:
            entry: dict[str, Any] = {"path": path}
            if attribute is not None:
                entry["attribute"] = attribute
            entry["error"] = error
            return entry

        try:
            module, file = self._load(path, f"no module file {path}")
        except _LoadError as error:
            return failed(str(error))
        if attribute is not None:
            try:
                value = getattr(module, attribute, None)
            except BaseException:
                value = None
            spec = spec_of(value)
            if spec is None:
                return failed(f"{path}: {attribute} is not declared with @node")
            declared = [(attribute, spec)]
        else:
            declared = []
            for name, value in list(vars(module).items()):
                spec = spec_of(value)
                if spec is not None:
                    declared.append((name, spec))
        types = []
        for name, spec in declared:
            try:
                spec.validate()
                type_spec = spec.to_type_spec()
                check_value(type_spec)
            except (SpecError, ValueError, TypeError) as error:
                return failed(f"{path}: {name}: {error}")
            types.append({"attribute": name, "spec": type_spec})
        try:
            closure = source_closure(file, self._root(), self.sources)
        except ClosureError as error:
            return failed(f"{path}: its source closure cannot be read: {error}")
        return {
            "path": path,
            "types": types,
            "closure": [{"label": label, "path": str(where)} for label, where in closure],
        }

    # -- build -------------------------------------------------------------------------------

    def build(self, params: dict[str, Any]) -> dict[str, Any]:
        path = _project_path(params.get("path"), "build's path")
        function = params.get("function")
        if not isinstance(function, str) or not _FUNCTION.fullmatch(function):
            raise _Refusal(INVALID_PARAMS, "build's function is a Python name")
        arguments = params.get("arguments")
        if not isinstance(arguments, dict) or not all(
            isinstance(value, str) for value in arguments.values()
        ):
            raise _Refusal(INVALID_PARAMS, "build's arguments map names to strings")
        cwd = params.get("cwd")
        if not isinstance(cwd, str) or not cwd or not os.path.isabs(cwd):
            raise _Refusal(INVALID_PARAMS, "build's cwd is an absolute path")
        previous = os.getcwd()
        try:
            os.chdir(cwd)
        except OSError:
            raise _Refusal(
                INVALID_PARAMS, "build's cwd is not a folder this host can enter"
            ) from None
        # The builder module's top-level code and the builder both run in cwd (section 5.2).
        try:
            built, document = self._build_in_cwd(path, function, arguments)
        finally:
            try:
                os.chdir(previous)
            except OSError:
                os.chdir(self._root())
        anchor = path
        if built.source is not None:
            source = Path(built.source).resolve()
            if source.is_relative_to(self._root()) and source != self._root():
                anchor = source.relative_to(self._root()).as_posix()
        return {"document": document, "takes_anchor": anchor}

    def _build_in_cwd(
        self, path: str, function: str, arguments: dict[str, str]
    ) -> tuple[Workflow, dict[str, Any]]:
        """Loads the builder module, runs the builder and returns its workflow and document;
        the caller has set the working directory."""
        try:
            module, _ = self._load(path, f"no builder file {path}")
        except _LoadError as error:
            raise _Refusal(LOAD_FAILED, str(error)) from None
        try:
            builder = getattr(module, function, None)
        except BaseException:
            builder = None
        if not callable(builder):
            raise _Refusal(LOAD_FAILED, f"{path} has no function {function}")
        try:
            built = builder(**arguments)
        except BaseException as error:
            raise _Refusal(BUILD_FAILED, f"{function}: {self._error_text(error)}") from None
        if not isinstance(built, Workflow):
            raise _Refusal(
                BUILD_FAILED,
                f"{function} returned {type(built).__name__}, not a grida.fx.Workflow",
            )
        try:
            document = built.document()
            check_value(document)
        except BaseException as error:
            raise _Refusal(BUILD_FAILED, f"{function}: {self._error_text(error)}") from None
        return built, document

    # -- run ---------------------------------------------------------------------------------

    def _pending_run(self) -> _Run | None:
        with self._lock:
            return self._run

    def run(self, request_id: Any, params: dict[str, Any]) -> object:
        """Starts the body of a ``run`` on the body loop; it answers the request when the body
        is done (module docstring)."""
        _check_run(params)
        body, spec, name = self._body(params["body"])
        state = _Run(
            request_id, params["run_id"], Ctx(params, self.session.channel(params["run_id"]))
        )
        with self._lock:
            if self._run is not None:
                raise _Refusal(INVALID_REQUEST, "run came while a run is pending")
            self._run = state
        task = asyncio.run_coroutine_threadsafe(
            self._execute(state, body, spec, name), self.session.loop()
        )
        task.add_done_callback(_log_escaped)
        return _LATER

    def _body(self, body: dict[str, Any]) -> tuple[Callable[..., Any], NodeSpec | None, str]:
        """The body a ``run`` names, its declared spec (``None`` for a built-in, whose
        declaration is the engine's), and the name its messages use."""
        if "builtin" in body:
            builtin = body["builtin"]
            if not isinstance(builtin, str) or set(body) != {"builtin"}:
                raise _Refusal(INVALID_PARAMS, "run's body is {path, attribute} or {builtin}")
            function = std.body_of(builtin)
            if function is None:
                raise _Refusal(LOAD_FAILED, f"grida {_sdk_version()} has no body for {builtin}")
            name = builtin.removeprefix("fx/").rsplit("@", 1)[0]
            return function, spec_of(function), name
        path = _project_path(body.get("path"), "run's body path")
        attribute = body.get("attribute")
        if not isinstance(attribute, str) or not attribute:
            raise _Refusal(INVALID_PARAMS, "run's body attribute is a name")
        try:
            module, _ = self._load(path, f"no module file {path}")
        except _LoadError as error:
            raise _Refusal(LOAD_FAILED, str(error)) from None
        try:
            value = getattr(module, attribute, None)
        except BaseException:
            value = None
        spec = spec_of(value)
        if spec is None or not callable(value):
            raise _Refusal(LOAD_FAILED, f"{path}: {attribute} is not declared with @node")
        return value, spec, spec.name

    async def _execute(
        self, state: _Run, body: Callable[..., Any], spec: NodeSpec | None, name: str
    ) -> None:
        """Runs the body and answers the run, once the engine has answered every request of
        the run (section 5.3)."""
        ctx = state.ctx
        result: dict[str, Any] | None = None
        error: dict[str, Any] | None = None
        try:
            returned = await _call_body(body, ctx)
            result = {"outputs": await self._outputs(ctx, spec, name, returned)}
        except NodeFailure as failure:
            error = {"code": NODE_FAILURE, "message": self._message(failure), "data": _kept(ctx)}
        except EngineError as refused:
            if refused.code in ERROR_CODES:
                error = {"code": refused.code, "message": refused.message or type(refused).__name__}
                if isinstance(refused.data, dict):
                    error["data"] = refused.data
            else:
                error = self._node_error(refused, ctx)
        except BaseException as raised:  # whatever the body raised is its node_error
            error = self._node_error(raised, ctx)
        try:
            await self.session.settle(state.run_id)
        except BaseException as broken:  # pragma: no cover - settling waits on futures only
            _log(f"grida.fx.host: waiting for run {state.run_id}'s requests failed: {broken}")
        with self._lock:
            if self._run is state:
                self._run = None
        self._answer(state.request_id, result, error)

    async def _outputs(
        self, ctx: Ctx, spec: NodeSpec | None, name: str, returned: Any
    ) -> dict[str, Any]:
        """A body's outputs as output values (section 3.4)."""
        if returned is None:
            returned = {}
        if not isinstance(returned, Mapping):
            raise NodeFailure(f"{name} returned {type(returned).__name__}, not its outputs")
        outputs: dict[str, Any] = {}
        for port, value in returned.items():
            if not isinstance(port, str):
                raise NodeFailure(f"{name} returned an output named {port!r}, not a port name")
            shape = None
            if spec is not None and port in spec.outputs:
                shape = spec.outputs[port].shape
            outputs[port] = await self._port_output(ctx, port, shape, value)
        return outputs

    async def _port_output(self, ctx: Ctx, port: str, shape: str | None, value: Any) -> Any:
        if value is None:
            return None
        if isinstance(value, list | tuple):
            return {
                "list": [
                    await self._output_value(ctx, item, f"{port}[{index}]")
                    for index, item in enumerate(value)
                ]
            }
        if isinstance(value, Mapping) and shape in ("keyed", None):
            items = []
            for key, item in value.items():
                if not isinstance(key, str):
                    raise NodeFailure(f"output {port} has the key {key!r}, which is not a string")
                items.append([key, await self._output_value(ctx, item, f"{port}[{key}]")])
            return {"collection": items}
        return await self._output_value(ctx, value, port)

    async def _output_value(self, ctx: Ctx, item: Any, label: str) -> dict[str, Any]:
        if isinstance(item, InputFile):
            return {"file": item.ref}
        if isinstance(item, Output):
            if item.work_path is not None:
                return {"work_path": item.work_path, "kind": item.kind}
            return {"file": await ctx._put_async(item, f"{ctx.instance.path}/{label}")}
        raise NodeFailure(f"output {label} is {type(item).__name__}; use ctx.out to make it")

    def _message(self, error: BaseException) -> str:
        return self._unrooted(str(error)) or type(error).__name__

    def _node_error(self, error: BaseException, ctx: Ctx | None = None) -> dict[str, Any]:
        """A ``node_error``: the exception's own message, with its type and traceback (and the
        body's facts and marks) in ``data``. The engine names the failure ``<exception>:
        <message>``, so the message does not repeat the type; an exception with no message
        (``KeyboardInterrupt``) sends its type, since a message is never empty (section 7)."""
        data: dict[str, Any] = {
            "exception": type(error).__name__,
            "traceback": self._traceback(error),
        }
        if ctx is not None:
            data.update(_kept(ctx))
        message = self._error_message(error) or type(error).__name__
        return {"code": NODE_ERROR, "message": message, "data": data}

    def _traceback(self, error: BaseException) -> str:
        """Where ``error`` was raised, from the first frame of user code, with the project root
        cut."""
        frames = error.__traceback__
        while frames is not None and _is_machinery(frames.tb_frame.f_code.co_filename):
            frames = frames.tb_next
        return self._unrooted("".join(traceback.format_exception(type(error), error, frames)))

    # -- agents: tool.invoke, agent.check, $/cancel -------------------------------------------

    def tool_invoke(self, request_id: Any, params: dict[str, Any]) -> object:
        agent = self._agent(params)
        call_id = params.get("call_id")
        if "call_id" not in params or not (call_id is None or isinstance(call_id, str)):
            raise _Refusal(INVALID_PARAMS, "tool.invoke's call_id is a string or null")
        name = params.get("name")
        if not isinstance(name, str) or not name:
            raise _Refusal(INVALID_PARAMS, "tool.invoke's name is a tool's name")
        arguments = params.get("arguments")
        if not isinstance(arguments, dict):
            raise _Refusal(INVALID_PARAMS, "tool.invoke's arguments are a JSON object")
        self._serve(request_id, "tool.invoke", agent.serve_tool(name, arguments))
        return _LATER

    def agent_check(self, request_id: Any, params: dict[str, Any]) -> object:
        agent = self._agent(params)
        if "value" not in params:
            raise _Refusal(INVALID_PARAMS, "agent.check needs the submitted value")
        if not agent.has_check:
            raise _Refusal(INVALID_PARAMS, f"agent {params['agent_id']} has no check")
        self._serve(request_id, "agent.check", agent.serve_check(params["value"]))
        return _LATER

    def _agent(self, params: dict[str, Any]) -> Agent:
        """The agent of the pending run that ``params`` names."""
        run_id = params.get("run_id")
        agent_id = params.get("agent_id")
        state = self._pending_run()
        if state is None or not isinstance(run_id, str) or state.run_id != run_id:
            raise _Refusal(INVALID_PARAMS, f"no run {run_id} is pending on this host")
        agent = state.ctx._agents.get(agent_id) if isinstance(agent_id, str) else None
        if agent is None:
            raise _Refusal(INVALID_PARAMS, f"run {run_id} has no agent {agent_id} running")
        return agent

    def _serve(
        self, request_id: Any, method: str, work: Coroutine[Any, Any, dict[str, Any]]
    ) -> None:
        """Serves a ``tool.invoke`` or ``agent.check`` on the body loop."""
        with self._lock:
            self._serving[request_id] = (method, None)
        task = asyncio.run_coroutine_threadsafe(
            self._answer_served(request_id, method, work), self.session.loop()
        )
        task.add_done_callback(_log_escaped)
        with self._lock:
            if request_id in self._serving:
                self._serving[request_id] = (method, task)

    async def _answer_served(
        self, request_id: Any, method: str, work: Coroutine[Any, Any, dict[str, Any]]
    ) -> None:
        result: dict[str, Any] | None = None
        error: dict[str, Any] | None = None
        try:
            result = await work
        except asyncio.CancelledError:
            error = {"code": CANCELLED, "message": f"the engine cancelled {method}"}
        except NodeFailure as failure:
            error = {"code": NODE_FAILURE, "message": self._message(failure)}
        except BaseException as raised:  # a check's (or a tool's) unexpected exception
            error = self._node_error(raised)
        with self._lock:
            if self._serving.pop(request_id, None) is None:
                return  # answered `cancelled` already
        self._answer(request_id, result, error)

    def _cancel(self, params: Any) -> None:
        """``$/cancel {id}`` (section 5.6)."""
        if not isinstance(params, dict) or not _valid_id(params.get("id")):
            return
        target = params["id"]
        with self._lock:
            state = self._run
            if state is not None and state.request_id == target:
                state.ctx.cancelled = True
                return
            served = self._serving.pop(target, None)
        if served is None:
            if self._stand_in is not None:
                self.session.loop().call_soon_threadsafe(self._stand_in.cancel, target)
            return
        method, task = served
        self._answer(target, error={"code": CANCELLED, "message": f"the engine cancelled {method}"})
        if task is not None:
            task.cancel()

    # -- stand-ins: stand_in.load, stand_in.answer --------------------------------------------

    def stand_in_load(self, params: dict[str, Any]) -> dict[str, Any]:
        """Loads the stand-in this host answers for (module docstring)."""
        path = params.get("path")
        if not isinstance(path, str) or not path or not os.path.isabs(path):
            raise _Refusal(
                INVALID_PARAMS, "stand_in.load's path is the stand-in file's absolute path"
            )
        function = params.get("function")
        if not isinstance(function, str) or not _FUNCTION.fullmatch(function):
            raise _Refusal(INVALID_PARAMS, "stand_in.load's function is a Python name")
        if self._stand_in is not None:
            raise _Refusal(INVALID_REQUEST, "a stand-in is already loaded")
        if self._pending_run() is not None:
            raise _Refusal(INVALID_REQUEST, "stand_in.load came while a run is pending")
        file = Path(path)
        shown = _shown(file)
        module = self._load_stand_in(file.resolve(), shown)
        try:
            value = getattr(module, function, None)
        except BaseException:
            value = None
        if not callable(value):
            raise _Refusal(LOAD_FAILED, f"{shown} has no function {function}")
        self._stand_in = Answerer(value, error_text=self._error_text, on_fault=self._stand_in_fault)
        return {}

    def _load_stand_in(self, file: Path, shown: str) -> Any:
        """The stand-in file as a module of its own, its folder first on ``sys.path``."""
        if not file.is_file():
            raise _Refusal(LOAD_FAILED, f"no stand-in file {shown}")
        spec = importlib.util.spec_from_file_location(_STAND_IN_MODULE, file)
        if spec is None or spec.loader is None:
            raise _Refusal(LOAD_FAILED, f"cannot import {shown}")
        folder = str(file.parent)
        sys.path[:] = [entry for entry in sys.path if entry != folder]
        sys.path.insert(0, folder)
        module = importlib.util.module_from_spec(spec)
        sys.modules[_STAND_IN_MODULE] = module
        try:
            spec.loader.exec_module(module)
        except BaseException as error:
            # As for a project module: whatever its top-level code raised is its error.
            sys.modules.pop(_STAND_IN_MODULE, None)
            raise _Refusal(
                LOAD_FAILED, f"{shown} failed to import: {self._error_text(error)}"
            ) from None
        return module

    def stand_in_answer(self, request_id: Any, params: dict[str, Any]) -> object:
        """Starts answering a ``stand_in.answer`` on the body loop; it is answered from there."""
        answerer = self._stand_in
        if answerer is None:
            raise _Refusal(INVALID_REQUEST, "stand_in.answer came before stand_in.load")

        def reply(answer: dict[str, Any]) -> None:
            self._answer(request_id, answer.get("result"), answer.get("error"))

        # Started by a callback, not a task, so a `$/cancel` read next finds the call.
        self.session.loop().call_soon_threadsafe(answerer.start, request_id, params, reply)
        return _LATER

    def _stand_in_fault(self, error: BaseException) -> None:
        _log(f"grida.fx.host: the stand-in failed:\n{self._traceback(error)}")

    # -- modules -----------------------------------------------------------------------------

    def _root(self) -> Path:
        assert self.project_root is not None
        return Path(self.project_root)

    def _load(self, path: str, missing: str) -> tuple[Any, Path]:
        """The project module at ``path``, loaded once per session, and its resolved file."""
        root = self._root()
        file = (root / path).resolve()
        if not file.is_relative_to(root):
            raise _LoadError(f"{path} is outside the project")
        relative = file.relative_to(root).as_posix()
        if relative in self.modules:
            return self.modules[relative], file
        if relative in self._failures:
            raise _LoadError(self._failures[relative])
        if not file.is_file():
            raise _LoadError(missing)
        name = "_fx_project_" + hashlib.sha256(relative.encode("utf-8")).hexdigest()[:16]
        spec = importlib.util.spec_from_file_location(name, file)
        if spec is None or spec.loader is None:
            raise _LoadError(f"cannot import {relative}")
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        try:
            spec.loader.exec_module(module)
        except BaseException as error:
            # SystemExit, KeyboardInterrupt and asyncio.CancelledError included: user code
            # raised it, so it is this module's error, not the host's.
            sys.modules.pop(name, None)
            text = f"{relative} failed to import: {self._error_text(error)}"
            self._failures[relative] = text
            raise _LoadError(text) from None
        self.modules[relative] = module
        return module, file

    def _error_text(self, error: BaseException) -> str:
        """``<Type>: <message>``, with the project root cut from paths in the message; ``<Type>``
        alone when the message is empty (``KeyboardInterrupt``), as Python prints it."""
        message = self._error_message(error)
        if not message:
            return type(error).__name__
        return f"{type(error).__name__}: {message}"

    def _error_message(self, error: BaseException) -> str:
        """The exception's message, with the project root cut from paths in it."""
        if isinstance(error, SyntaxError) and error.filename:
            return f"{error.msg} ({self._unrooted(error.filename)}, line {error.lineno})"
        return self._unrooted(str(error))

    def _unrooted(self, text: str) -> str:
        """``text`` with every path under the project root made project-relative."""
        for root in self._root_texts:
            text = text.replace(root + os.sep, "")
        return text


def _params(params: Any) -> dict[str, Any]:
    if not isinstance(params, dict):
        raise _Refusal(INVALID_PARAMS, "this method's params are a JSON object")
    return params


def _shown(file: Path) -> str:
    """How messages name a stand-in file: relative to the working directory (the engine's, where
    the user named it) when it is inside it, else by its absolute path."""
    cwd = Path(os.getcwd())
    given = Path(os.path.normpath(file))
    for candidate in (given, given.resolve()):
        if candidate.is_relative_to(cwd) and candidate != cwd:
            return candidate.relative_to(cwd).as_posix()
    return str(given)


def _project_path(value: Any, what: str) -> str:
    if not isinstance(value, str) or not _PROJECT_PATH.fullmatch(value):
        raise _Refusal(
            INVALID_PARAMS, f"{what} is a POSIX path relative to the project root, not {value!r}"
        )
    return value


def _check_run(params: dict[str, Any]) -> None:
    """Refuses ``run`` params that do not have the shape of section 5.3."""
    run_id = params.get("run_id")
    if not isinstance(run_id, str) or not run_id:
        raise _Refusal(INVALID_PARAMS, "run's run_id is a non-empty string")
    instance = params.get("instance")
    take = instance.get("take") if isinstance(instance, dict) else None
    if (
        not isinstance(instance, dict)
        or not all(isinstance(instance.get(field), str) for field in ("id", "path", "step"))
        or not isinstance(instance.get("key", None), str | None)
        or not isinstance(take, list)
        or not take
        or not all(isinstance(number, int) and not isinstance(number, bool) for number in take)
    ):
        raise _Refusal(INVALID_PARAMS, "run's instance is {id, path, step, key, take}")
    if not isinstance(params.get("body"), dict):
        raise _Refusal(INVALID_PARAMS, "run's body is {path, attribute} or {builtin}")
    for field in ("params", "param_files", "inputs", "resources", "tools", "calls"):
        if not isinstance(params.get(field), dict):
            raise _Refusal(INVALID_PARAMS, f"run's {field} is a JSON object")
    work_dir = params.get("work_dir")
    if not isinstance(work_dir, str) or not work_dir or not os.path.isabs(work_dir):
        raise _Refusal(INVALID_PARAMS, "run's work_dir is an absolute path")
    timeout_s = params.get("timeout_s")
    if isinstance(timeout_s, bool) or not isinstance(timeout_s, int | float | None):
        raise _Refusal(INVALID_PARAMS, "run's timeout_s is a number of seconds or null")


async def _call_body(body: Callable[..., Any], ctx: Ctx) -> Any:
    """Runs a body: an ``async def`` on the body loop, a ``def`` in a worker thread."""
    if inspect.iscoroutinefunction(body):
        returned = await body(ctx)
    else:
        returned = await asyncio.to_thread(body, ctx)
    if inspect.isawaitable(returned):
        returned = await returned
    return returned


def _kept(ctx: Ctx) -> dict[str, Any]:
    """The node facts and marks a body reported (``ctx.facts``, ``ctx.marks``), for a failure's
    ``data``; either is left out when the body made it something that is not an I-JSON value."""
    kept: dict[str, Any] = {}
    facts = {name: value for name, value in ctx.facts.items() if name != "cost_usd"}
    marks = list(ctx.marks)
    for key, value in (("facts", facts), ("marks", marks)):
        try:
            check_value(value)
        except ValueError:
            continue
        kept[key] = value
    return kept


def _is_machinery(filename: str) -> bool:
    if filename.startswith(_SDK):
        return not filename.startswith(_STD)
    return filename.startswith(_MACHINERY)


def _log_escaped(task: concurrent.futures.Future[None]) -> None:
    """Logs what escaped a task the host scheduled on the body loop (none should)."""
    if not task.cancelled() and task.exception() is not None:
        error = task.exception()
        assert error is not None
        _log("".join(traceback.format_exception(error)))


def _valid_id(value: Any) -> bool:
    if isinstance(value, bool):
        return False
    if isinstance(value, str | int):
        return True
    return isinstance(value, float) and value.is_integer()


def _sdk_version() -> str:
    try:
        return importlib.metadata.version("grida")
    except importlib.metadata.PackageNotFoundError:
        return "unknown"


def _log(text: str) -> None:
    print(text, file=sys.stderr, flush=True)


def protect_stdout() -> BinaryIO:
    """Returns the protocol stream (a duplicate of fd 1) after pointing fd 1 at stderr."""
    if sys.stdout is not None:
        sys.stdout.flush()
    protocol = os.dup(1)
    os.dup2(2, 1)
    sys.stdout = sys.stderr
    return os.fdopen(protocol, "wb")


def _protect_stdin() -> BinaryIO:
    """Returns the protocol input (a duplicate of fd 0) after pointing fd 0 at the null device."""
    protocol = os.dup(0)
    null = os.open(os.devnull, os.O_RDONLY)
    try:
        os.dup2(null, 0)
    finally:
        os.close(null)
    return os.fdopen(protocol, "rb")


def _detach_forks(*streams: BinaryIO) -> None:
    """Points the protocol streams of every process forked from this one at the null device
    (module docstring). The descriptors stay open, so the stream objects the child inherits never
    write into a file it opens later."""
    descriptors = [stream.fileno() for stream in streams]

    def after_in_child() -> None:
        null = os.open(os.devnull, os.O_RDWR)
        try:
            for descriptor in descriptors:
                os.dup2(null, descriptor, inheritable=False)
        finally:
            os.close(null)

    os.register_at_fork(after_in_child=after_in_child)


#: How often the host checks that the engine that started it still runs, in seconds.
ENGINE_CHECK_INTERVAL = 1.0


def _watch_engine(interval: float = ENGINE_CHECK_INTERVAL) -> None:
    """Ends the host when its parent changes: the engine is gone (module docstring)."""
    engine = os.getppid()

    def watch() -> None:
        while True:
            time.sleep(interval)
            if os.getppid() != engine:
                _abandoned()

    threading.Thread(target=watch, name="grida.fx engine watch", daemon=True).start()


def _abandoned() -> None:
    """Ends a host whose engine is gone: its process group with it when it leads one."""
    _log("grida.fx.host: the engine that started this host is gone; ending")
    killpg = getattr(os, "killpg", None)
    if killpg is not None and os.getpgrp() == os.getpid():
        try:
            killpg(os.getpid(), signal.SIGKILL)
        except OSError:
            pass
    _end(1)


def _forget_safe_path_mark() -> None:
    """Removes the ``PYTHONSAFEPATH`` the engine set, so programs user code starts inherit the
    user's environment; a value the user set stays."""
    if os.environ.get("PYTHONSAFEPATH") == SAFE_PATH_MARK:
        del os.environ["PYTHONSAFEPATH"]


def main() -> int:
    """Serves one session on the process's streams; returns the exit status. The protocol
    stream is flushed and closed when it returns."""
    _forget_safe_path_mark()
    writer = protect_stdout()
    reader = _protect_stdin()
    _detach_forks(reader, writer)
    _watch_engine()
    try:
        return Host(reader, writer).serve()
    except BrokenPipeError:
        return 1
    finally:
        try:
            writer.close()
        except OSError:
            pass


def _end(status: int) -> None:
    """Ends the process now: a thread user code left running is not waited for (section 1)."""
    for stream in (sys.stdout, sys.stderr):
        try:
            if stream is not None:
                stream.flush()
        except (OSError, ValueError):
            pass
    os._exit(status)


if __name__ == "__main__":
    try:
        _status = main()
    except BaseException:
        traceback.print_exc()
        _status = 1
    _end(_status)
