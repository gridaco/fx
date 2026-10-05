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
  @node``. ``builtins: true`` reports the types of ``grida.fx.std`` (none in step 2).
- ``build`` (section 5.2): with the working directory set to ``cwd``, load the builder file and
  call ``function(**arguments)``, then restore the working directory; ``load_failed`` (``no
  builder file …``, ``… has no function …``, import failures) and ``build_failed``
  (``<function>: <Type>: <message>``; a result that is not a ``Workflow``: ``<function>
  returned <type>, not a grida.fx.Workflow``); the result is
  ``{document, takes_anchor}``, the anchor the project-relative path of ``Workflow.source`` (or
  ``path`` when it is unknown or outside the project).
- ``shutdown`` → ``null``; then the ``exit`` notification ends the process with status 0 (1 if no
  ``shutdown`` came first). End of stdin ends the process. The process ends with ``os._exit``
  once the protocol stream is flushed and closed, so a thread user code left running cannot keep
  it alive.
- an unknown method: ``-32601``; a host fault: ``internal``.

The host also points descriptor 0 at the null device once it holds its own duplicate of stdin,
so a program user code starts never reads protocol bytes. Messages the host writes name project
files by their project-relative paths: the project root is cut from the texts of user errors.
Whatever user code raises while a module loads or a builder runs (``KeyboardInterrupt`` and
``asyncio.CancelledError`` included) is that module's error or that build's failure.
"""

from __future__ import annotations

import hashlib
import importlib.metadata
import importlib.util
import os
import platform
import re
import sys
import traceback
from pathlib import Path
from typing import Any, BinaryIO

from grida.fx._builder import Workflow
from grida.fx._closure import ClosureError, source_closure
from grida.fx._protocol import (
    BUILD_FAILED,
    INTERNAL,
    INVALID_PARAMS,
    INVALID_REQUEST,
    LOAD_FAILED,
    METHOD_NOT_FOUND,
    PARSE_ERROR,
    PROTOCOL,
    PROTOCOL_MISMATCH,
    ProtocolError,
    check_value,
    parse_message,
    read_message,
    write_message,
)
from grida.fx._spec import SpecError, spec_of

#: A project path on the wire: POSIX, relative, with no empty, ``.`` or ``..`` segment.
_SEGMENT = r"(?:[^/\\.][^/\\]*|\.[^/\\.][^/\\]*|\.\.[^/\\]+)"
_PROJECT_PATH = re.compile(_SEGMENT + r"(?:/" + _SEGMENT + r")*")
_SOURCE_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_FUNCTION = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
#: The ``PYTHONSAFEPATH`` value the engine sets when the user's environment has none.
SAFE_PATH_MARK = "grida-fx"


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

    # -- the session -------------------------------------------------------------------------

    def serve(self) -> int:
        """Serves until ``exit`` or end of input; returns the exit status."""
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
            # A response to a request of this host's: it sends none in this version.
            _log(f"grida.fx.host: ignored a response to id {request_id!r}")
            return None
        method = message["method"]
        if not isinstance(method, str):
            self._send_error(known_id, INVALID_REQUEST, "a message's method is a string")
            return None
        if "id" not in message:
            return self._notify(method)
        if known_id is None:
            self._send_error(None, INVALID_REQUEST, "a request id is an integer or a string")
            return None
        try:
            result = self._request(method, message.get("params"))
        except _Refusal as refusal:
            self._send_error(known_id, refusal.code, refusal.message, refusal.data)
            return None
        except Exception as error:
            _log("".join(traceback.format_exception(error)))
            self._send_error(known_id, INTERNAL, f"the node host failed: {self._error_text(error)}")
            return None
        self._send_result(known_id, result)
        return None

    def _notify(self, method: str) -> int | None:
        if method == "exit":
            return 0 if self.shutting_down else 1
        # `$/cancel` has nothing to stop while one job runs at a time and none is pending.
        return None

    def _request(self, method: str, params: Any) -> Any:
        if method == "initialize":
            if self.initialized:
                raise _Refusal(INVALID_REQUEST, "initialize was already answered")
            return self.initialize(_params(params))
        if not self.initialized:
            raise _Refusal(INVALID_REQUEST, f"{method} came before initialize")
        if self.shutting_down:
            raise _Refusal(INVALID_REQUEST, f"{method} came after shutdown")
        if method == "shutdown":
            if params is not None:
                raise _Refusal(INVALID_PARAMS, "shutdown takes no params")
            self.shutting_down = True
            return None
        if method == "describe":
            return self.describe(_params(params))
        if method == "build":
            return self.build(_params(params))
        raise _Refusal(METHOD_NOT_FOUND, f"the Python node host has no method {method}")

    def _send_result(self, request_id: Any, result: Any) -> None:
        try:
            write_message(self.writer, {"jsonrpc": "2.0", "id": request_id, "result": result})
        except ValueError as error:
            self._send_error(
                request_id, INTERNAL, f"the node host's answer is not an I-JSON value: {error}"
            )

    def _send_error(
        self, request_id: Any, code: int, message: str, data: dict[str, Any] | None = None
    ) -> None:
        error: dict[str, Any] = {"code": code, "message": message or "error"}
        if data is not None:
            error["data"] = data
        write_message(self.writer, {"jsonrpc": "2.0", "id": request_id, "error": error})

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
        # The built-in bodies of grida.fx.std arrive with the runner: this host carries none yet.
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
        if isinstance(error, SyntaxError) and error.filename:
            message = f"{error.msg} ({self._unrooted(error.filename)}, line {error.lineno})"
        else:
            message = self._unrooted(str(error))
        if not message:
            return type(error).__name__
        return f"{type(error).__name__}: {message}"

    def _unrooted(self, text: str) -> str:
        """``text`` with every path under the project root made project-relative."""
        for root in self._root_texts:
            text = text.replace(root + os.sep, "")
        return text


def _params(params: Any) -> dict[str, Any]:
    if not isinstance(params, dict):
        raise _Refusal(INVALID_PARAMS, "this method's params are a JSON object")
    return params


def _project_path(value: Any, what: str) -> str:
    if not isinstance(value, str) or not _PROJECT_PATH.fullmatch(value):
        raise _Refusal(
            INVALID_PARAMS, f"{what} is a POSIX path relative to the project root, not {value!r}"
        )
    return value


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
