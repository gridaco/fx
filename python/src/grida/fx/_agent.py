"""Agents and their tools (``spec/protocol.md`` sections 5.4, 5.5, 6.2).

The engine runs the model loop; the body declares the tools and the check:

- :func:`tool` marks ``function(ctx, **arguments)`` as an agent tool (``fx_tool`` on the
  function). :func:`tool_schema` builds its ``{name, description, parameters}``: the first
  parameter (``ctx``) is skipped; each other one is typed from its annotation (``str`` string,
  ``int`` integer, ``float`` number, ``bool`` boolean, ``list`` array, ``dict`` object, string
  annotations by those names, anything else string); ``required`` lists those without a default,
  in order; the description is the docstring or the function's name.
- :class:`Tool` is a declared tool: ``name`` (``^[a-z][a-z0-9_]{0,63}$``, never ``submit``), a
  non-blank ``description``, a mapping ``parameters`` (a JSON Schema), and
  ``handler(arguments) -> ToolResult`` (sync or async). Construction validates and raises
  ``ValueError`` (the predecessor's ``ToolSpec`` rules).
- :class:`ToolResult` is a declared tool's answer: non-blank ``text`` and ``images``, each an
  image data URL (``data:image/...;base64,...``) or an HTTP(S) URL; :class:`ToolInvocationError`
  is what a handler raises to tell the model it failed.
- :class:`ToolReply` is an ``@tool`` function's answer with pictures: ``text`` and ``images``
  (paths, :class:`~grida.fx._ctx.InputFile`, :class:`~grida.fx._ctx.Output`, PIL images, data
  URLs).
- :class:`Agent` (``ctx.agent(...)``): ``await agent.run(instructions, *, max_steps, images=(),
  submit=None, check=None)`` sends ``agent.run`` and serves ``tool.invoke`` and ``agent.check``
  for it; returns the text, or the submitted value. Each ``run`` starts a fresh transcript;
  ``agent.transcript`` holds the last one, also when the run ended with an error after its loop
  started (the engine sends it in the error's ``data.transcript``), so a body that catches the
  failure can read what the agent tried. In the transcript a tool call's ``arguments`` is the
  JSON text of the object the model gave. ``check(value)`` refuses by raising ``ValueError`` or
  ``NodeFailure``.

Pictures leave the host as file values (section 3.2): a data URL is decoded and stored with
``file.put`` (``base64``, kind from its media type, default ``image/png``); a path is stored by
its bytes (kind by suffix ``.png``, ``.jpg``/``.jpeg``, ``.webp``, else ``image/png``); an
``InputFile`` or ``CallResult`` file passes by digest; anything else is
``NodeFailure("an agent picture is a file, not <type>")``. A tool that raises
:class:`~grida.fx._errors.NodeFailure` ends the loop (``node_failure``); any other exception
answers ``{"error": "<type>: <message>"}`` and the loop goes on. A tool's content is sent as
given (the engine writes ``text(content)``).

The host serves the engine's requests through :meth:`Agent.serve_tool` (``tool.invoke``: the
answer, or :class:`~grida.fx._errors.NodeFailure` raised) and :meth:`Agent.serve_check`
(``agent.check``: the answer, or the check's unexpected exception raised, which the host answers
``node_error``).
When the engine then fails ``agent.run`` with that same error, :meth:`Agent.run` raises the
tool's or the check's own exception, so the body sees what its code raised.
"""

from __future__ import annotations

import asyncio
import base64
import inspect
import os
import re
import sys
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from grida.fx._ctx import InputFile, Output
from grida.fx._errors import EngineError, NodeFailure
from grida.fx._protocol import NODE_ERROR, NODE_FAILURE, check_value

#: The tool an agent calls to finish; the engine adds it when ``submit`` is given.
SUBMIT = "submit"
_TOOL_NAME = re.compile(r"[a-z][a-z0-9_]{0,63}")
#: An image a model can be shown: an HTTP(S) URL, or a base64 image data URL.
_IMAGE_URL = re.compile(r"(?:https?://|data:image/[^;,]+;base64,).", re.IGNORECASE | re.DOTALL)
_HTTP_URL = re.compile(r"https?://", re.IGNORECASE)
_DIGEST = re.compile(r"[0-9a-f]{64}")
_JSON_TYPES = {
    str: "string",
    int: "integer",
    float: "number",
    bool: "boolean",
    list: "array",
    dict: "object",
}
_NAMED_TYPES = {"str": str, "int": int, "float": float, "bool": bool, "list": list, "dict": dict}
_PICTURE_KINDS = {
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".webp": "image/webp",
}


class ToolInvocationError(ValueError):
    """A declared tool's handler failed; the model is told the message."""


@dataclass(frozen=True)
class ToolResult:
    """A declared tool's answer: text, and image URLs (module docstring)."""

    text: str
    images: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if not isinstance(self.text, str) or not self.text.strip():
            raise ValueError("tool result text must be non-empty")
        if isinstance(self.images, str):
            raise ValueError("tool result images are a sequence of URLs, not one string")
        images = tuple(self.images)
        for image in images:
            if not isinstance(image, str) or not _IMAGE_URL.match(image):
                raise ValueError("tool result images must be HTTP(S) URLs or image data URLs")
        object.__setattr__(self, "images", images)


@dataclass(frozen=True)
class Tool:
    """A declared agent tool (module docstring)."""

    name: str
    description: str
    parameters: Mapping[str, Any]
    handler: Callable[[Mapping[str, Any]], Any]

    def __post_init__(self) -> None:
        if not isinstance(self.name, str) or not _TOOL_NAME.fullmatch(self.name):
            raise ValueError("tool name must be lower_snake_case, at most 64 characters")
        if not isinstance(self.description, str) or not self.description.strip():
            raise ValueError(f"tool {self.name} must carry a description")
        if not isinstance(self.parameters, Mapping):
            raise ValueError(f"tool {self.name} parameters must be a JSON Schema object")
        if self.name == SUBMIT:
            raise ValueError(f"{SUBMIT} is reserved for the episode's final answer")
        if not callable(self.handler):
            raise ValueError(f"tool {self.name} handler must be callable")


@dataclass
class ToolReply:
    """An ``@tool`` function's answer with pictures."""

    text: str
    images: Sequence[Any] = field(default_factory=tuple)


def tool(function: Callable[..., Any] | None = None, *, name: str | None = None) -> Any:
    """Declare an agent tool over ``function(ctx, **arguments)``."""

    def wrap(inner: Callable[..., Any]) -> Callable[..., Any]:
        inner.fx_tool = {"name": name or inner.__name__}  # type: ignore[attr-defined]
        return inner

    return wrap(function) if function is not None else wrap


def tool_schema(function: Callable[..., Any]) -> dict[str, Any]:
    """``{name, description, parameters}`` of an ``@tool`` function (module docstring)."""
    signature = inspect.signature(function)
    hints = getattr(function, "__annotations__", {}) or {}
    properties: dict[str, Any] = {}
    required: list[str] = []
    for index, (name, parameter) in enumerate(signature.parameters.items()):
        if index == 0:
            continue  # ctx
        if parameter.kind in (parameter.VAR_POSITIONAL, parameter.VAR_KEYWORD):
            continue  # the model fills in named arguments only
        annotation = hints.get(name, str)
        if isinstance(annotation, str):
            annotation = _NAMED_TYPES.get(annotation, str)
        properties[name] = {"type": _JSON_TYPES.get(annotation, "string")}
        if parameter.default is inspect.Parameter.empty:
            required.append(name)
    return {
        "name": _function_tool_name(function),
        "description": inspect.getdoc(function) or function.__name__,
        "parameters": {"type": "object", "properties": properties, "required": required},
    }


class Agent:
    """A tool-using model loop the engine runs for a body (module docstring)."""

    def __init__(
        self,
        ctx: Any,
        *,
        system: str,
        tools: Sequence[Any] = (),
        recent_images: int | None = None,
        max_tokens: int | None = None,
    ) -> None:
        self.ctx = ctx
        self.system = system
        self.tools = tuple(tools)
        self.recent_images = recent_images
        self.max_tokens = max_tokens
        self.transcript: list[dict[str, Any]] = []
        self._tools: dict[str, Any] = {}
        for item in self.tools:
            name = _tool_name(item)
            if name == SUBMIT:
                raise NodeFailure(f"{SUBMIT} is the agent's own tool; name yours otherwise")
            self._tools[name] = item
        self._check: Callable[[Any], Any] | None = None
        #: What a tool or the check raised that ended the loop, re-raised by :meth:`run`.
        self._ended_by: BaseException | None = None

    @property
    def has_check(self) -> bool:
        """Whether the pending ``run`` has a check function (``agent.check`` is served)."""
        return self._check is not None

    async def run(
        self,
        instructions: str,
        *,
        max_steps: int,
        images: Sequence[Any] = (),
        submit: Mapping[str, Any] | None = None,
        check: Callable[[Any], Any] | None = None,
    ) -> Any:
        """Runs the loop in the engine (``agent.run``) until the model answers, or submits a
        value ``check`` accepts; returns the text, or the submitted value."""
        if isinstance(max_steps, bool) or not isinstance(max_steps, int) or max_steps < 1:
            raise ValueError(f"max_steps is a whole number of at least 1, not {max_steps!r}")
        if not isinstance(instructions, str):
            raise TypeError(f"an agent's instructions are a string, not {type(instructions)}")
        ctx = self.ctx
        pictures = [await self._picture(image) for image in images]
        agent_id = ctx._next_agent_id()
        params: dict[str, Any] = {
            "agent_id": agent_id,
            "system": self.system,
            "instructions": instructions,
        }
        if pictures:
            params["images"] = pictures
        params["tools"] = [_declared_schema(item) for item in self._tools.values()]
        params["max_steps"] = max_steps
        params["submit"] = None if submit is None else _plain(submit)
        params["check"] = submit is not None and check is not None
        if self.recent_images is not None:
            params["recent_images"] = self.recent_images
        if self.max_tokens is not None:
            params["max_tokens"] = self.max_tokens
        self._check = check if submit is not None else None
        self._ended_by = None
        self.transcript = []
        ctx._agents[agent_id] = self
        try:
            result = await ctx.channel.request_async("agent.run", params)
        except EngineError as error:
            self.transcript = _transcript_of(error.data)
            ended_by = self._ended_by
            if ended_by is not None and error.code in (NODE_FAILURE, NODE_ERROR):
                raise ended_by from None
            raise
        finally:
            ctx._agents.pop(agent_id, None)
            self._check = None
            self._ended_by = None
        if not isinstance(result, Mapping):
            raise EngineError(f"the engine answered agent.run with {type(result).__name__}")
        self.transcript = list(result.get("transcript") or [])
        if "submitted" in result:
            return result["submitted"]
        return result.get("text", "")

    async def serve_tool(self, name: str, arguments: Mapping[str, Any]) -> dict[str, Any]:
        """Calls one tool for ``tool.invoke`` (section 5.4) and returns the answer. A
        :class:`NodeFailure` the tool raises is raised (the host answers ``node_failure``)."""
        function = self._tools.get(name)
        if function is None:
            return {"content": f"no tool named {name}"}
        try:
            if _is_declared(function):
                result = await _call(function.handler, dict(arguments))
            else:
                result = await _call(function, self.ctx, **arguments)
            return await self._reply(result)
        except NodeFailure as failure:
            self._ended_by = failure
            raise
        except Exception as error:
            return {"error": f"{type(error).__name__}: {error}"}
        except BaseException as error:
            # KeyboardInterrupt, SystemExit: the host answers node_error, which ends the loop.
            if not isinstance(error, asyncio.CancelledError):
                self._ended_by = error
            raise

    async def serve_check(self, value: Any) -> dict[str, Any]:
        """Asks the check about a submitted value for ``agent.check`` (section 5.5). A
        ``ValueError`` or :class:`NodeFailure` it raises is the refusal; anything else it raises
        is raised (the host answers ``node_error``)."""
        check = self._check
        if check is None:
            return {"refusal": None}
        try:
            await _call(check, value)
        except (ValueError, NodeFailure) as refusal:
            return {"refusal": str(refusal)}
        except BaseException as error:
            if not isinstance(error, asyncio.CancelledError):
                self._ended_by = error
            raise
        return {"refusal": None}

    async def _reply(self, result: Any) -> dict[str, Any]:
        if isinstance(result, ToolReply | ToolResult):
            text = result.text if isinstance(result.text, str) else str(result.text)
            answer: dict[str, Any] = {"content": text}
            images = list(result.images)
            if images:
                answer["images"] = [await self._picture(image) for image in images]
            return answer
        try:
            check_value(result)
        except ValueError:
            result = str(result)
        return {"content": result}

    async def _picture(self, image: Any) -> dict[str, str]:
        """A picture as a file value (module docstring)."""
        ctx = self.ctx
        if isinstance(image, InputFile):
            return {"file": image.digest}
        if isinstance(image, Output):
            return {"file": (await ctx._put_async(image, "agent/image"))["digest"]}
        if isinstance(image, Mapping) and _is_file_value(image):
            return {"file": image["file"]}
        if isinstance(image, str) and image.startswith("data:"):
            header, _, encoded = image.partition(",")
            kind = header.removeprefix("data:").split(";", 1)[0].lower() or "image/png"
            data = base64.b64decode(encoded)
            params = {"base64": base64.b64encode(data).decode("ascii"), "kind": kind}
            return await self._put(params, "agent/image")
        if isinstance(image, str) and _HTTP_URL.match(image):
            raise NodeFailure("an agent picture is a file, not a URL")
        if isinstance(image, str | os.PathLike):
            path = Path(image)
            kind = _PICTURE_KINDS.get(path.suffix.lower(), "image/png")
            work_dir = Path(ctx.run["work_dir"]).resolve()
            resolved = path.resolve()
            if resolved.is_relative_to(work_dir) and resolved.is_file():
                relative = resolved.relative_to(work_dir).as_posix()
                return await self._put({"work_path": relative, "kind": kind}, path.name)
            data = path.read_bytes()
            params = {"base64": base64.b64encode(data).decode("ascii"), "kind": kind}
            return await self._put(params, path.name or "agent/image")
        if _is_picture(image):
            from grida.fx.std import pictures

            png = pictures.png(image)
            params = {"base64": base64.b64encode(png).decode("ascii"), "kind": "image/png"}
            return await self._put(params, "agent/image")
        raise NodeFailure(f"an agent picture is a file, not {type(image).__name__}")

    async def _put(self, params: dict[str, Any], name: str) -> dict[str, str]:
        ref = await self.ctx.channel.request_async("file.put", {**params, "name": name})
        return {"file": ref["digest"]}


async def _call(function: Callable[..., Any], *args: Any, **kwargs: Any) -> Any:
    """Calls a tool, handler or check: a coroutine function on the body loop, anything else in a
    worker thread (it may block on the engine's answers); an awaitable result is awaited."""
    if inspect.iscoroutinefunction(function):
        result = await function(*args, **kwargs)
    else:
        result = await asyncio.to_thread(function, *args, **kwargs)
    if inspect.isawaitable(result):
        result = await result
    return result


def _is_declared(item: Any) -> bool:
    """A tool declared by its schema (a name, description, parameters and handler)."""
    return isinstance(item, Tool) or all(
        hasattr(item, member) for member in ("name", "description", "parameters", "handler")
    )


def _function_tool_name(function: Callable[..., Any]) -> str:
    declared = getattr(function, "fx_tool", None)
    if isinstance(declared, Mapping) and isinstance(declared.get("name"), str):
        return declared["name"]
    return function.__name__


def _tool_name(item: Any) -> str:
    if _is_declared(item):
        return str(item.name)
    if not callable(item) or getattr(item, "fx_tool", None) is None:
        raise NodeFailure(f"{getattr(item, '__name__', item)} is not declared with @tool")
    return _function_tool_name(item)


def _declared_schema(item: Any) -> dict[str, Any]:
    if _is_declared(item):
        return {
            "name": str(item.name),
            "description": str(item.description),
            "parameters": _plain(item.parameters),
        }
    return tool_schema(item)


def _plain(value: Any) -> Any:
    """A JSON Schema given as mappings and sequences, as dicts and lists."""
    if isinstance(value, Mapping):
        return {key: _plain(item) for key, item in value.items()}
    if isinstance(value, list | tuple):
        return [_plain(item) for item in value]
    return value


def _transcript_of(data: Any) -> list[dict[str, Any]]:
    """The transcript an ``agent.run`` error carries in ``data.transcript``, else none."""
    if isinstance(data, Mapping):
        transcript = data.get("transcript")
        if isinstance(transcript, list):
            return list(transcript)
    return []


def _is_file_value(value: Mapping[str, Any]) -> bool:
    return (
        len(value) == 1
        and isinstance(value.get("file"), str)
        and _DIGEST.fullmatch(value["file"]) is not None
    )


def _is_picture(value: Any) -> bool:
    """A Pillow image (only when Pillow is loaded: nothing else can make one)."""
    image_module = sys.modules.get("PIL.Image")
    return image_module is not None and isinstance(value, image_module.Image)
