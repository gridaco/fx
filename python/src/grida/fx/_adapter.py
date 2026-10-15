"""Project adapters (``spec/adapters.md`` section 9; ``spec/protocol.md`` sections 5.8, 6.8 and
8.1): a declared provider's paid calls, served by the project's own Python code in a node host.

- **Declaring.** ``acme = adapter(requests=[...], jobs=[...], send=..., submit=..., collect=...,
  version=1)`` at a module's top level makes an :class:`Adapter`, which a host loads by its module
  and attribute (``adapter.load``, :mod:`grida.fx.host`). Each capability of ``requests`` is
  served by ``send``, and each of ``jobs`` by ``submit`` and ``collect``, in that order;
  ``version`` is an integer of at least 0, or ``None``. The declaration is refused where the
  module makes it, with a ``ValueError`` or a ``TypeError``: ``adapter() declares no capability:
  give it requests or jobs``; ``adapter(): requests is a list of capability names`` (or
  ``jobs``); ``adapter(): "Image" is not a capability name``; ``adapter(): image.generate is
  named twice``; ``adapter(): requests need send``; ``adapter(): jobs need submit and collect``;
  ``adapter(): send is a function`` (or ``submit``, ``collect``); ``adapter(): version is an
  integer of at least 0, or None``.
- **The call.** The params of ``adapter.send``, ``adapter.submit`` and ``adapter.collect`` become
  an :class:`AdapterCall`: ``capability``, ``route`` (an :class:`AdapterRoute`: ``id``,
  ``model``, ``provider``, ``contract``), ``base_url``, ``key``, ``attempt``, ``handle`` (``None``
  outside ``collect``), ``takes`` (the ``take`` list) and ``take`` (its last number), ``files``
  (an :class:`AdapterFile` by digest: ``digest``, ``kind``, ``size``, ``path``, ``read_bytes()``)
  and ``request`` (a copy whose file values, ``{"file": <digest>}`` at any depth, are those
  ``AdapterFile`` objects). Params of another shape are a ``ValueError``, which the host answers
  ``-32602``.
- **fetch.** ``await call.fetch(url, ...)`` asks the engine for one exchange (section 6.8), with
  the call's ``call_id`` and no ``run_id``. Header names are lowercased. Refused before anything is
  sent, with a ``ValueError`` or a ``TypeError``: a method other than GET and POST (``fetch takes
  GET or POST, not PUT``), a body on a GET (``a GET has no body``), ``json`` and ``body`` both
  (``fetch takes json or body, not both``), a ``body`` without its ``content_type`` (``a body
  needs its content_type``) or the reverse (``content_type goes with a body``), another lane
  (``fetch's lane is provider, upload or download, not 'cdn'``), a header name that is not a token
  (``the header name "x y" is not a token``), and the engine's ranges (``timeout_s is above 0 and
  at most 300``, ``max_response_bytes is from 1 to 67108864``). The answer is a
  :class:`FetchResponse` (``status``, ``headers`` as ``(name, value)`` pairs in order, ``body``
  bytes, ``json()``, ``text()``); a failed exchange raises :class:`FetchError` (``phase``:
  ``refused``, ``not_sent`` or ``after_send``; ``reason``); an error answer raises the
  :class:`~grida.fx.EngineError` of its code (``Cancelled`` for a call the engine dropped).
- **Outcomes.** ``send`` returns :class:`Answered`, :class:`NotReceived`, :class:`Refused` or
  :class:`Failed`; ``submit`` :class:`Accepted`, :class:`Uncertain`, ``NotReceived``,
  ``Refused`` or ``Failed``; ``collect`` ``Answered``, :class:`Ended` or :class:`Unreachable`.
  Each holds its scalar members to the protocol when it is made: a ``reason`` is a string with a
  character other than white space, ``cost_usd`` and ``retry_after_s`` finite numbers of at least
  0, ``retryable`` a boolean, ``usage`` a mapping of ``input_tokens``, ``cached_input_tokens`` and
  ``output_tokens`` to integers of at least 0. Files, ``data`` and a handle are held when the
  outcome is sent (:func:`encode`): an answer's files as a stand-in's (``bytes`` leave the kind
  out, an ``Output`` of bytes and an ``AdapterFile`` give theirs, a path the kind of its suffix),
  named ``[a-z][a-z0-9_]*``; its ``data``, and a handle, a mapping, values the engine can read.
- **Serving.** :class:`Server` answers calls on the loop that starts them, concurrently and in
  any order: a coroutine function as a task, a plain function on a daemon thread of its own (as a
  stand-in's, :func:`grida.fx._stand_in.on_thread`). ``$/cancel`` answers the call ``cancelled``
  at once and cancels a coroutine function's task; a plain function runs to its end, and what it
  returns is discarded. Anything the function raises (``BaseException`` too), a value that is not
  an outcome its method takes (``send returns Answered, NotReceived, Refused or Failed, not
  None``) and an outcome the SDK cannot send are faults of the adapter, answered ``internal`` with
  ``<Type>: <message>``, which ends the call as the loss of its host would; unlike a stand-in's,
  a fault ends only its own call.
"""

from __future__ import annotations

import asyncio
import base64
import binascii
import inspect
import json as jsonlib
import math
import os
import re
from collections.abc import Awaitable, Callable, Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Final

from grida.fx._ctx import Output, kind_of
from grida.fx._errors import EngineError
from grida.fx._protocol import CANCELLED, INTERNAL, check_value
from grida.fx._stand_in import Refused as ErrorAnswer
from grida.fx._stand_in import error_text, on_thread, with_files

#: The methods an adapter host answers for a call, the function each calls, and the kind of
#: capability it serves (section 5.8).
SEND: Final = "adapter.send"
SUBMIT: Final = "adapter.submit"
COLLECT: Final = "adapter.collect"
METHODS: Final = {SEND: ("send", "request"), SUBMIT: ("submit", "job"), COLLECT: ("collect", "job")}

#: A capability's name, an answer file's name and a header's (fx-node-protocol-v1).
_CAPABILITY = re.compile(r"[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*")
_FILE_NAME = re.compile(r"[a-z][a-z0-9_]*")
_HEADER_NAME = re.compile(r"[a-z0-9!#$%&'*+.^_`|~-]+")
#: The longest deadline and the largest response a ``fetch`` may ask for (section 6.8).
MAX_TIMEOUT_S: Final = 300
MAX_RESPONSE_BYTES: Final = 64 * 1024 * 1024
_LANES: Final = ("provider", "upload", "download")
_PHASES: Final = ("refused", "not_sent", "after_send")
_USAGE: Final = ("input_tokens", "cached_input_tokens", "output_tokens")


# ------------------------------------------------------------------------------------------------
# Declaring

#: What serves a call: a function of one :class:`AdapterCall`, plain or a coroutine function.
AdapterFunction = Callable[["AdapterCall"], Any]


@dataclass(frozen=True)
class Adapter:
    """A project adapter, as :func:`adapter` declares it (module docstring)."""

    requests: tuple[str, ...] = ()
    jobs: tuple[str, ...] = ()
    send: AdapterFunction | None = None
    submit: AdapterFunction | None = None
    collect: AdapterFunction | None = None
    version: int | None = None

    def __post_init__(self) -> None:
        named: list[str] = []
        for member in ("requests", "jobs"):
            value = getattr(self, member)
            if isinstance(value, str | bytes) or not isinstance(value, Iterable):
                raise TypeError(f"adapter(): {member} is a list of capability names")
            names = tuple(value)
            for name in names:
                if not isinstance(name, str):
                    raise TypeError(f"adapter(): {member} is a list of capability names")
                if not _CAPABILITY.fullmatch(name):
                    shown = jsonlib.dumps(name, ensure_ascii=False)
                    raise ValueError(f"adapter(): {shown} is not a capability name")
                if name in named:
                    raise ValueError(f"adapter(): {name} is named twice")
                named.append(name)
            object.__setattr__(self, member, names)
        if not named:
            raise ValueError("adapter() declares no capability: give it requests or jobs")
        for member in ("send", "submit", "collect"):
            value = getattr(self, member)
            if value is not None and not callable(value):
                raise TypeError(f"adapter(): {member} is a function")
        if self.requests and self.send is None:
            raise ValueError("adapter(): requests need send")
        if self.jobs and (self.submit is None or self.collect is None):
            raise ValueError("adapter(): jobs need submit and collect")
        version = self.version
        if version is not None and (
            isinstance(version, bool) or not isinstance(version, int) or version < 0
        ):
            raise ValueError("adapter(): version is an integer of at least 0, or None")

    @property
    def capabilities(self) -> dict[str, str]:
        """Each capability it serves and its kind, ``request`` or ``job``, in the order it
        declares them: ``adapter.load``'s ``capabilities``."""
        kinds = {name: "request" for name in self.requests}
        kinds.update((name, "job") for name in self.jobs)
        return kinds


def adapter(
    *,
    requests: Iterable[str] = (),
    jobs: Iterable[str] = (),
    send: AdapterFunction | None = None,
    submit: AdapterFunction | None = None,
    collect: AdapterFunction | None = None,
    version: int | None = None,
) -> Adapter:
    """Declares a project adapter (module docstring): ``send`` serves each capability of
    ``requests``, ``submit`` and ``collect`` each of ``jobs``. A module holds it at its top
    level, under the name a declaration's ``adapter: <path>#<name>`` gives."""
    return Adapter(
        requests=requests,  # type: ignore[arg-type]  # made a tuple by Adapter
        jobs=jobs,  # type: ignore[arg-type]
        send=send,
        submit=submit,
        collect=collect,
        version=version,
    )


def function_for(declared: Adapter, method: str, capability: str) -> AdapterFunction | None:
    """The function that answers ``method`` for ``capability``, when the adapter serves the
    capability with it."""
    function, kind = METHODS[method]
    if declared.capabilities.get(capability) != kind:
        return None
    return getattr(declared, function)


# ------------------------------------------------------------------------------------------------
# The call


@dataclass(frozen=True)
class AdapterRoute:
    """``call.route``: the route's ``id`` (``model@provider``), ``model``, ``provider``, and its
    ``contract`` as its table's reading completed it."""

    id: str
    model: str
    provider: str
    contract: dict[str, Any]


@dataclass(frozen=True)
class AdapterFile:
    """A file a call's request names: its ``digest``, ``kind`` and ``size``, and the ``path`` of
    its store copy, which an adapter reads and never writes, moves or deletes."""

    digest: str
    kind: str
    size: int
    path: Path

    def read_bytes(self) -> bytes:
        return self.path.read_bytes()


#: How a call reaches the engine for ``fetch``: the params, sent; the result, awaited.
Fetch = Callable[[dict[str, Any]], Awaitable[Any]]


class AdapterCall:
    """One call an adapter is asked to send, submit or collect (module docstring). ``method`` is
    the request that carries it; ``fetch`` reaches the engine (a test gives its own). Params that
    are not the method's are a ``ValueError``."""

    def __init__(
        self, params: Mapping[str, Any], method: str = SEND, *, fetch: Fetch | None = None
    ) -> None:
        if method not in METHODS:
            raise ValueError(f"{method} is not an adapter call")
        if not isinstance(params, Mapping):
            raise ValueError(f"{method}'s params are a JSON object")

        def refused(what: str) -> ValueError:
            return ValueError(f"{method}'s {what}")

        call_id = params.get("call_id")
        if not isinstance(call_id, str) or not call_id:
            raise refused("call_id is a string")
        target = params.get("adapter")
        if not isinstance(target, Mapping) or not all(
            isinstance(target.get(name), str) for name in ("path", "attribute")
        ):
            raise refused("adapter is {path, attribute}")
        capability = params.get("capability")
        if not isinstance(capability, str) or not capability:
            raise refused("capability is a capability's name")
        route = params.get("route")
        if (
            not isinstance(route, Mapping)
            or not all(isinstance(route.get(name), str) for name in ("id", "model", "provider"))
            or not isinstance(route.get("contract"), Mapping)
        ):
            raise refused("route is {id, model, provider, contract}")
        base_url = params.get("base_url")
        if not isinstance(base_url, str) or not base_url:
            raise refused("base_url is a URL")
        request = params.get("request")
        if not isinstance(request, Mapping):
            raise refused("request is a JSON object")
        files = params.get("files")
        if not isinstance(files, Mapping) or not all(
            isinstance(digest, str) and _adapter_file(ref) for digest, ref in files.items()
        ):
            raise refused("files are {digest, kind, size, path} by digest")
        take = params.get("take")
        if (
            not isinstance(take, list)
            or not take
            or not all(isinstance(n, int) and not isinstance(n, bool) and n >= 1 for n in take)
        ):
            raise refused("take is a list of take numbers")
        key = params.get("key")
        if not isinstance(key, str) or not key:
            raise refused("key is the call key")
        attempt = params.get("attempt")
        if isinstance(attempt, bool) or not isinstance(attempt, int) or not 1 <= attempt <= 6:
            raise refused("attempt is from 1 to 6")
        handle = params.get("handle")
        if method == COLLECT and not isinstance(handle, Mapping):
            raise refused("handle is a JSON object")
        if method != COLLECT and "handle" in params:
            raise ValueError(f"{method} has no handle")
        self.capability: str = capability
        self.route = AdapterRoute(
            id=route["id"],
            model=route["model"],
            provider=route["provider"],
            contract=dict(route["contract"]),
        )
        self.base_url: str = base_url
        self.key: str = key
        self.attempt: int = attempt
        self.takes: tuple[int, ...] = tuple(take)
        self.take: int = self.takes[-1]
        #: The job's handle, as ``submit`` accepted it; ``None`` outside ``collect``.
        self.handle: dict[str, Any] | None = dict(handle) if method == COLLECT else None
        self.files: dict[str, AdapterFile] = {
            digest: AdapterFile(
                digest=ref["digest"], kind=ref["kind"], size=ref["size"], path=Path(ref["path"])
            )
            for digest, ref in files.items()
        }
        self.request: dict[str, Any] = with_files(request, self.files)
        self._method = method
        self._call_id = call_id
        self._fetch = fetch

    async def fetch(
        self,
        url: str,
        *,
        method: str = "GET",
        headers: Mapping[str, str] | Iterable[tuple[str, str]] | None = None,
        json: Any = None,
        body: bytes | None = None,
        content_type: str | None = None,
        lane: str = "provider",
        credential: bool = False,
        timeout_s: float | None = None,
        max_response_bytes: int | None = None,
    ) -> FetchResponse:
        """One HTTP exchange through the engine's transport (module docstring): the engine
        attaches the declaration's key when ``credential`` is true, on the provider and upload
        lanes and only to the base URL's origin."""
        params = fetch_params(
            self._call_id,
            url,
            method=method,
            headers=headers,
            json=json,
            body=body,
            content_type=content_type,
            lane=lane,
            credential=credential,
            timeout_s=timeout_s,
            max_response_bytes=max_response_bytes,
        )
        if self._fetch is None:
            raise RuntimeError("this call has no engine to fetch through")
        return read_response(await self._fetch(params))

    def __repr__(self) -> str:
        return (
            f"<AdapterCall {self._method} {self.capability} on {self.route.id}"
            f" attempt {self.attempt} take {self.take}>"
        )


def _adapter_file(ref: Any) -> bool:
    if not isinstance(ref, Mapping):
        return False
    size = ref.get("size")
    return (
        isinstance(ref.get("digest"), str)
        and isinstance(ref.get("kind"), str)
        and isinstance(size, int)
        and not isinstance(size, bool)
        and size >= 0
        and isinstance(ref.get("path"), str)
        and bool(ref["path"])
    )


# ------------------------------------------------------------------------------------------------
# fetch


@dataclass(frozen=True)
class FetchResponse:
    """What an exchange through ``fetch`` answered: its ``status``, its ``headers`` as
    ``(name, value)`` pairs in order, names lowercase, and its ``body``."""

    status: int
    headers: tuple[tuple[str, str], ...]
    body: bytes

    def json(self) -> Any:
        return jsonlib.loads(self.body)

    def text(self) -> str:
        return self.body.decode("utf-8")


class FetchError(Exception):
    """An exchange through ``fetch`` that failed: its ``phase`` (``refused``, ``not_sent`` or
    ``after_send``, which ``spec/adapters.md`` section 3.2 maps onto an outcome) and its
    ``reason``, which names no URL path, header value or body."""

    def __init__(self, phase: str, reason: str) -> None:
        super().__init__(reason)
        self.phase = phase
        self.reason = reason

    def __repr__(self) -> str:
        return f"FetchError({self.phase!r}, {self.reason!r})"


def fetch_params(
    call_id: str,
    url: str,
    *,
    method: str = "GET",
    headers: Mapping[str, str] | Iterable[tuple[str, str]] | None = None,
    json: Any = None,
    body: bytes | None = None,
    content_type: str | None = None,
    lane: str = "provider",
    credential: bool = False,
    timeout_s: float | None = None,
    max_response_bytes: int | None = None,
) -> dict[str, Any]:
    """``fetch``'s params for the call ``call_id`` (module docstring)."""
    if not isinstance(url, str):
        raise TypeError("fetch's url is a string")
    if not url:
        raise ValueError("fetch's url is an absolute URL")
    verb = method.upper() if isinstance(method, str) else None
    if verb not in ("GET", "POST"):
        raise ValueError(f"fetch takes GET or POST, not {method}")
    params: dict[str, Any] = {"call_id": call_id, "method": verb, "url": url}
    listed = _headers(headers)
    if listed:
        params["headers"] = listed
    if json is not None and body is not None:
        raise ValueError("fetch takes json or body, not both")
    if body is not None and not isinstance(body, bytes | bytearray | memoryview):
        raise TypeError(f"fetch's body is bytes, not {type(body).__name__}")
    if body is not None and not (isinstance(content_type, str) and content_type):
        raise ValueError("a body needs its content_type")
    if body is None and content_type is not None:
        raise ValueError("content_type goes with a body")
    if verb == "GET" and (json is not None or body is not None):
        raise ValueError("a GET has no body")
    if json is not None:
        params["body"] = {"json": json}
    elif body is not None:
        encoded = base64.b64encode(bytes(body)).decode("ascii")
        params["body"] = {"base64": encoded, "content_type": content_type}
    if lane not in _LANES:
        raise ValueError(f"fetch's lane is provider, upload or download, not {lane!r}")
    params["lane"] = lane
    if not isinstance(credential, bool):
        raise TypeError("credential is True or False")
    if credential:
        params["credential"] = True
    if timeout_s is not None:
        if (
            isinstance(timeout_s, bool)
            or not isinstance(timeout_s, int | float)
            or (isinstance(timeout_s, float) and not math.isfinite(timeout_s))
            or not 0 < timeout_s <= MAX_TIMEOUT_S
        ):
            raise ValueError(f"timeout_s is above 0 and at most {MAX_TIMEOUT_S}")
        params["timeout_s"] = timeout_s
    if max_response_bytes is not None:
        if (
            isinstance(max_response_bytes, bool)
            or not isinstance(max_response_bytes, int)
            or not 1 <= max_response_bytes <= MAX_RESPONSE_BYTES
        ):
            raise ValueError(f"max_response_bytes is from 1 to {MAX_RESPONSE_BYTES}")
        params["max_response_bytes"] = max_response_bytes
    return params


def _headers(headers: Mapping[str, str] | Iterable[tuple[str, str]] | None) -> list[list[str]]:
    if headers is None:
        return []
    pairs = headers.items() if isinstance(headers, Mapping) else headers
    listed: list[list[str]] = []
    for pair in pairs:
        if not isinstance(pair, tuple | list) or len(pair) != 2:
            raise TypeError("fetch's headers are a mapping or (name, value) pairs")
        name, value = pair
        if not isinstance(name, str) or not isinstance(value, str):
            raise TypeError("a header's name and value are strings")
        if not _HEADER_NAME.fullmatch(name.lower()):
            shown = jsonlib.dumps(name, ensure_ascii=False)
            raise ValueError(f"the header name {shown} is not a token")
        listed.append([name.lower(), value])
    return listed


def read_response(result: Any) -> FetchResponse:
    """``fetch``'s result: the response, or :class:`FetchError` for a failed exchange."""
    if isinstance(result, Mapping) and set(result) == {"error"}:
        error = result["error"]
        if (
            isinstance(error, Mapping)
            and error.get("phase") in _PHASES
            and isinstance(error.get("reason"), str)
        ):
            raise FetchError(error["phase"], error["reason"])
    elif isinstance(result, Mapping):
        status = result.get("status")
        headers = result.get("headers")
        encoded = result.get("base64")
        if (
            isinstance(status, int)
            and not isinstance(status, bool)
            and isinstance(headers, list)
            and all(
                isinstance(pair, list)
                and len(pair) == 2
                and all(isinstance(part, str) for part in pair)
                for pair in headers
            )
            and isinstance(encoded, str)
        ):
            try:
                body = base64.b64decode(encoded, validate=True)
            except (binascii.Error, ValueError):
                pass
            else:
                return FetchResponse(status, tuple((name, value) for name, value in headers), body)
    raise EngineError("the engine answered fetch with something the SDK cannot read", code=INTERNAL)


# ------------------------------------------------------------------------------------------------
# Outcomes


def _reason(value: Any) -> str:
    if not isinstance(value, str):
        raise TypeError(f"a reason is a sentence, not {type(value).__name__}")
    if not value.strip():
        raise ValueError("a reason is a sentence, not a blank string")
    return value


def _amount(name: str, value: Any) -> None:
    if value is None:
        return
    if isinstance(value, bool) or not isinstance(value, int | float):
        raise TypeError(f"{name} is a number, not {type(value).__name__}")
    if (isinstance(value, float) and not math.isfinite(value)) or value < 0:
        raise ValueError(f"{name} is a number of at least 0, not {value!r}")


def _usage(usage: Any) -> None:
    if usage is None:
        return
    if not isinstance(usage, Mapping):
        raise TypeError(f"usage is a mapping of token counts, not {type(usage).__name__}")
    for name, count in usage.items():
        if name not in _USAGE:
            raise ValueError(f"usage counts {', '.join(_USAGE)}, not {name!r}")
        if isinstance(count, bool) or not isinstance(count, int) or count < 0:
            raise ValueError(f"usage {name} is an integer of at least 0, not {count!r}")


@dataclass(frozen=True)
class Answered:
    """``answered``: the provider's answer, its ``files`` by name (``bytes``, an
    :class:`~grida.fx.Output` of bytes, an :class:`AdapterFile` or a path) and its ``data``, a
    JSON value or ``None``; ``cost_usd`` the cost it reported, and ``usage`` its token counts."""

    files: Mapping[str, bytes | Output | AdapterFile | os.PathLike[str]] = field(
        default_factory=dict
    )
    data: Any = None
    cost_usd: float | None = field(default=None, kw_only=True)
    usage: Mapping[str, int] | None = field(default=None, kw_only=True)

    def __post_init__(self) -> None:
        _amount("cost_usd", self.cost_usd)
        _usage(self.usage)

    @classmethod
    def json(
        cls, value: Any, *, cost_usd: float | None = None, usage: Mapping[str, int] | None = None
    ) -> Answered:
        """A ``structured.generate`` answer: ``data`` of ``{"json": value}``."""
        return cls(data={"json": value}, cost_usd=cost_usd, usage=usage)

    def _wire(self) -> dict[str, Any]:
        if not isinstance(self.files, Mapping):
            raise TypeError("an answer's files are a mapping of names to files")
        files: dict[str, Any] = {}
        for name, value in self.files.items():
            if not isinstance(name, str) or not _FILE_NAME.fullmatch(name):
                raise ValueError(
                    f"an answer's files are named in lowercase ([a-z][a-z0-9_]*), not {name!r}"
                )
            files[name] = _file(name, value)
        try:
            # The data sits in the result of a response: two levels inside its message.
            check_value(self.data, "/data", depth=2)
        except ValueError as error:
            raise ValueError(f"an answer's data is not a JSON value FX can read: {error}") from None
        wire: dict[str, Any] = {"outcome": "answered", "files": files, "data": self.data}
        if self.cost_usd is not None:
            wire["cost_usd"] = self.cost_usd
        if self.usage is not None:
            wire["usage"] = dict(self.usage)
        return wire


def _file(name: str, value: Any) -> dict[str, Any]:
    if isinstance(value, bytes | bytearray | memoryview):
        return {"base64": _base64(bytes(value))}
    if isinstance(value, Output):
        if value.data is None:
            raise TypeError(f"an answer's file is bytes: {name} is {value!r}")
        return {"base64": _base64(value.data), "kind": value.kind}
    if isinstance(value, AdapterFile):
        return {"base64": _base64(value.read_bytes()), "kind": value.kind}
    if isinstance(value, os.PathLike):
        path = Path(os.fsdecode(os.fspath(value)))
        return {"base64": _base64(path.read_bytes()), "kind": kind_of(path)}
    raise TypeError(
        f"an answer's file is bytes, an Output, an AdapterFile or a path; {name} is"
        f" {type(value).__name__}"
    )


def _base64(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


@dataclass(frozen=True)
class NotReceived:
    """``not_received``: the provider provably did not receive the request; the engine may send
    it again, after ``retry_after_s`` seconds when the provider asked for a wait."""

    reason: str
    retry_after_s: float | None = field(default=None, kw_only=True)

    def __post_init__(self) -> None:
        _reason(self.reason)
        _amount("retry_after_s", self.retry_after_s)

    def _wire(self) -> dict[str, Any]:
        wire: dict[str, Any] = {"outcome": "not_received", "reason": self.reason}
        if self.retry_after_s is not None:
            wire["retry_after_s"] = self.retry_after_s
        return wire


@dataclass(frozen=True)
class Refused:
    """``refused``: refused before anything was sent, at $0."""

    reason: str

    def __post_init__(self) -> None:
        _reason(self.reason)

    def _wire(self) -> dict[str, Any]:
        return {"outcome": "refused", "reason": self.reason}


@dataclass(frozen=True)
class Failed:
    """``failed``: the provider may have received the request, which failed; billed at
    ``cost_usd``, else the whole hold, and made again only when ``retryable``."""

    reason: str
    retryable: bool = field(kw_only=True)
    cost_usd: float | None = field(default=None, kw_only=True)

    def __post_init__(self) -> None:
        _reason(self.reason)
        if not isinstance(self.retryable, bool):
            raise TypeError("retryable is True or False")
        _amount("cost_usd", self.cost_usd)

    def _wire(self) -> dict[str, Any]:
        wire: dict[str, Any] = {
            "outcome": "failed",
            "reason": self.reason,
            "retryable": self.retryable,
        }
        if self.cost_usd is not None:
            wire["cost_usd"] = self.cost_usd
        return wire


@dataclass(frozen=True)
class Accepted:
    """``accepted``: the provider took the job; the ``handle``, a mapping of JSON values, holds
    only what collecting needs, never a host, a signed URL or a key."""

    handle: Mapping[str, Any]

    def __post_init__(self) -> None:
        if not isinstance(self.handle, Mapping):
            raise TypeError(f"a handle is a mapping, not {type(self.handle).__name__}")

    def _wire(self) -> dict[str, Any]:
        handle = dict(self.handle)
        try:
            check_value(handle, "/handle", depth=2)
        except ValueError as error:
            raise ValueError(f"a handle is not a JSON object FX can read: {error}") from None
        return {"outcome": "accepted", "handle": handle}


@dataclass(frozen=True)
class Uncertain:
    """``uncertain``: nobody can say whether the provider took the job."""

    reason: str

    def __post_init__(self) -> None:
        _reason(self.reason)

    def _wire(self) -> dict[str, Any]:
        return {"outcome": "uncertain", "reason": self.reason}


@dataclass(frozen=True)
class Ended:
    """``ended``: the job ended without a usable result; settled at ``cost_usd``, else the whole
    hold."""

    reason: str
    cost_usd: float | None = field(default=None, kw_only=True)

    def __post_init__(self) -> None:
        _reason(self.reason)
        _amount("cost_usd", self.cost_usd)

    def _wire(self) -> dict[str, Any]:
        wire: dict[str, Any] = {"outcome": "ended", "reason": self.reason}
        if self.cost_usd is not None:
            wire["cost_usd"] = self.cost_usd
        return wire


@dataclass(frozen=True)
class Unreachable:
    """``unreachable``: it is not clear that the job is over; a later run collects it again."""

    reason: str

    def __post_init__(self) -> None:
        _reason(self.reason)

    def _wire(self) -> dict[str, Any]:
        return {"outcome": "unreachable", "reason": self.reason}


#: The outcomes each method takes, in the order a refusal names them.
_TAKES: Final[dict[str, tuple[type, ...]]] = {
    SEND: (Answered, NotReceived, Refused, Failed),
    SUBMIT: (Accepted, Uncertain, NotReceived, Refused, Failed),
    COLLECT: (Answered, Ended, Unreachable),
}


def encode(method: str, returned: Any) -> dict[str, Any]:
    """What a function returned as ``method``'s result. ``TypeError`` or ``ValueError`` (or the
    ``OSError`` of a path that cannot be read) for anything the SDK cannot send."""
    takes = _TAKES[method]
    if not isinstance(returned, takes):
        names = [outcome.__name__ for outcome in takes]
        given = "None" if returned is None else type(returned).__name__
        raise TypeError(
            f"{METHODS[method][0]} returns {', '.join(names[:-1])} or {names[-1]}, not {given}"
        )
    return returned._wire()


# ------------------------------------------------------------------------------------------------
# Serving


class Server:
    """Answers adapter calls on the loop that calls :meth:`start` (module docstring).
    ``error_text`` names a fault in its answer; ``on_fault`` is told of each, with the adapter's
    name."""

    def __init__(
        self,
        *,
        error_text: Callable[[BaseException], str] = error_text,
        on_fault: Callable[[BaseException, str], None] | None = None,
    ) -> None:
        self._error_text = error_text
        self._on_fault = on_fault
        #: The calls being answered, by request id: the method, its task, and its reply.
        self._calls: dict[Any, tuple[str, asyncio.Task[dict[str, Any]], Callable[..., None]]] = {}
        #: Those a plain function answers on its thread; those answered ``cancelled``.
        self._on_thread: set[Any] = set()
        self._cancelled: set[Any] = set()

    def start(
        self,
        request_id: Any,
        method: str,
        function: AdapterFunction,
        call: AdapterCall,
        reply: Callable[[dict[str, Any]], None],
        name: str,
    ) -> None:
        """Starts answering one call of the adapter ``name``. ``reply`` gets ``{"result": …}``
        or ``{"error": …}`` once, unless :meth:`close` ends the call first. Called on the loop."""
        task = asyncio.get_running_loop().create_task(
            self._answer(request_id, method, function, call, name)
        )
        self._calls[request_id] = (method, task, reply)
        task.add_done_callback(lambda done: self._answered(request_id, done, reply))

    def cancel(self, request_id: Any) -> None:
        """``$/cancel``: answers the call ``cancelled`` and cancels it, unless a plain function is
        answering it (module docstring); a request id that names no call is ignored. Called on
        the loop."""
        entry = self._calls.get(request_id)
        if entry is None or request_id in self._cancelled:
            return
        method, task, reply = entry
        if task.done():
            return
        self._cancelled.add(request_id)
        reply({"error": {"code": CANCELLED, "message": f"the engine cancelled {method}"}})
        if request_id not in self._on_thread:
            task.cancel()

    def close(self) -> None:
        """Ends every call being answered, answering none of them: the session is over."""
        for _, task, _ in list(self._calls.values()):
            task.cancel()

    def _answered(
        self, request_id: Any, task: asyncio.Task[dict[str, Any]], reply: Callable[..., None]
    ) -> None:
        self._calls.pop(request_id, None)
        if request_id in self._cancelled:
            self._cancelled.discard(request_id)
            return  # answered `cancelled` already: what it returned is discarded
        if task.cancelled():
            return  # the session ended
        error = task.exception()
        if isinstance(error, ErrorAnswer):
            reply({"error": error.error})
        elif error is not None:  # pragma: no cover - _answer turns every exception into one
            reply({"error": {"code": INTERNAL, "message": error_text(error)}})
        else:
            reply({"result": task.result()})

    async def _answer(
        self, request_id: Any, method: str, function: AdapterFunction, call: AdapterCall, name: str
    ) -> dict[str, Any]:
        try:
            if inspect.iscoroutinefunction(function):
                returned = await function(call)
            else:
                self._on_thread.add(request_id)
                try:
                    returned = await on_thread(function, call, "grida.fx adapter")
                finally:
                    self._on_thread.discard(request_id)
            if inspect.isawaitable(returned):
                returned = await returned
            return encode(method, returned)
        except asyncio.CancelledError as cancelled:
            task = asyncio.current_task()
            if task is not None and task.cancelling():
                raise  # cancelled by the engine, or the session ended
            raise self._faulted(cancelled, name) from None
        except BaseException as error:  # whatever else the adapter raised is its fault
            raise self._faulted(error, name) from None

    def _faulted(self, error: BaseException, name: str) -> ErrorAnswer:
        if self._on_fault is not None:
            self._on_fault(error, name)
        return ErrorAnswer(INTERNAL, self._error_text(error))
