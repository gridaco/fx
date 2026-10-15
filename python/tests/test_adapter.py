"""Project adapters (``grida.fx._adapter``; spec/adapters.md section 9, spec/protocol.md sections
5.8, 6.8 and 8.1): the declaration, the call, ``fetch``, the outcomes on the wire, and the
:class:`Server` driven on a loop without an engine. ``test_host.py`` drives them through a host
process, and ``test_adapter_engine.py`` through the engine."""

from __future__ import annotations

import asyncio
import base64
import json
import threading
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator

import grida.fx as fx
from grida.fx._adapter import (
    COLLECT,
    SEND,
    SUBMIT,
    Server,
    encode,
    fetch_params,
    function_for,
    read_response,
)
from grida.fx._errors import Cancelled, EngineError

REPO = Path(__file__).resolve().parents[2]
SCHEMA = json.loads(
    (REPO / "spec" / "schemas" / "fx-node-protocol-v1.schema.json").read_text("utf-8")
)
KEY = "a" * 64
DIGEST = "d" * 64


def _validator(name: str) -> Draft202012Validator:
    return Draft202012Validator({"$ref": f"#/$defs/{name}", "$defs": SCHEMA["$defs"]})


RESULTS = {
    SEND: _validator("adapter_send_result"),
    SUBMIT: _validator("adapter_submit_result"),
    COLLECT: _validator("adapter_collect_result"),
}
FETCH_PARAMS = _validator("fetch_params")
SEND_PARAMS = _validator("adapter_send_params")
COLLECT_PARAMS = _validator("adapter_collect_params")


def _valid(validator: Draft202012Validator, value: Any) -> None:
    errors = [f"{list(error.path)}: {error.message}" for error in validator.iter_errors(value)]
    assert errors == [], errors


def _b64(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


def _params(**changes: Any) -> dict[str, Any]:
    params: dict[str, Any] = {
        "call_id": "c1",
        "adapter": {"path": "adapters/acme.py", "attribute": "acme"},
        "capability": "image.generate",
        "route": {
            "id": "img-a@acme",
            "model": "img-a",
            "provider": "acme",
            "contract": {"adapter": "adapters/acme.py#acme", "adapter_behavior": 1},
        },
        "base_url": "https://api.acme.test/v1",
        "request": {"prompt": "a kite"},
        "files": {},
        "take": [1],
        "key": KEY,
        "attempt": 1,
    }
    params.update(changes)
    return params


async def _noop(call: fx.AdapterCall) -> fx.Answered:
    return fx.Answered()


# --- declaring --------------------------------------------------------------------------------


def test_an_adapter_declares_its_capabilities_in_order() -> None:
    declared = fx.adapter(
        requests=["structured.generate", "image.generate"],
        jobs=("acme.render",),
        send=_noop,
        submit=_noop,
        collect=_noop,
        version=3,
    )
    assert isinstance(declared, fx.Adapter)
    assert declared.requests == ("structured.generate", "image.generate")
    assert list(declared.capabilities.items()) == [
        ("structured.generate", "request"),
        ("image.generate", "request"),
        ("acme.render", "job"),
    ]
    assert declared.version == 3
    # The function for each method, for a capability it serves with it.
    assert function_for(declared, SEND, "image.generate") is _noop
    assert function_for(declared, SUBMIT, "acme.render") is _noop
    assert function_for(declared, COLLECT, "acme.render") is _noop
    assert function_for(declared, SEND, "acme.render") is None
    assert function_for(declared, SUBMIT, "image.generate") is None
    assert function_for(declared, SEND, "video.generate") is None
    unversioned = fx.adapter(jobs=["video.generate"], submit=_noop, collect=_noop)
    assert unversioned.version is None
    assert unversioned.capabilities == {"video.generate": "job"}


@pytest.mark.parametrize(
    ("declaration", "error", "message"),
    [
        ({}, ValueError, "adapter() declares no capability: give it requests or jobs"),
        ({"requests": [], "send": _noop}, ValueError, "adapter() declares no capability"),
        ({"requests": "image.generate"}, TypeError, "adapter(): requests is a list of"),
        ({"jobs": [3]}, TypeError, "adapter(): jobs is a list of capability names"),
        ({"requests": ["Image"]}, ValueError, 'adapter(): "Image" is not a capability name'),
        ({"requests": ["image."]}, ValueError, 'adapter(): "image." is not a capability name'),
        (
            {
                "requests": ["image.generate"],
                "jobs": ["image.generate"],
                "send": _noop,
                "submit": _noop,
                "collect": _noop,
            },
            ValueError,
            "adapter(): image.generate is named twice",
        ),
        ({"requests": ["image.generate"]}, ValueError, "adapter(): requests need send"),
        (
            {"jobs": ["video.generate"], "submit": _noop},
            ValueError,
            "adapter(): jobs need submit and collect",
        ),
        (
            {"requests": ["image.generate"], "send": 3},
            TypeError,
            "adapter(): send is a function",
        ),
        (
            {"requests": ["image.generate"], "send": _noop, "version": -1},
            ValueError,
            "adapter(): version is an integer of at least 0, or None",
        ),
        (
            {"requests": ["image.generate"], "send": _noop, "version": True},
            ValueError,
            "adapter(): version is an integer of at least 0, or None",
        ),
        (
            {"requests": ["image.generate"], "send": _noop, "version": "1"},
            ValueError,
            "adapter(): version is an integer of at least 0, or None",
        ),
    ],
)
def test_declarations_that_are_refused(
    declaration: dict[str, Any], error: type[Exception], message: str
) -> None:
    with pytest.raises(error) as raised:
        fx.adapter(**declaration)
    assert str(raised.value).startswith(message)


# --- the call ---------------------------------------------------------------------------------


def test_a_call_reads_its_params(tmp_path: Path) -> None:
    (tmp_path / "kite.png").write_bytes(b"kite")
    file = {"digest": DIGEST, "kind": "image/png", "size": 4, "path": str(tmp_path / "kite.png")}
    request = {"prompt": "a kite", "references": [{"file": DIGEST}], "other": {"file": "x.png"}}
    params = _params(request=request, files={DIGEST: file}, take=[1, 3], attempt=2)
    _valid(SEND_PARAMS, params)
    call = fx.AdapterCall(params)
    assert call.capability == "image.generate"
    assert call.route == fx.AdapterRoute(
        id="img-a@acme",
        model="img-a",
        provider="acme",
        contract={"adapter": "adapters/acme.py#acme", "adapter_behavior": 1},
    )
    assert (call.base_url, call.key, call.attempt) == ("https://api.acme.test/v1", KEY, 2)
    assert (call.takes, call.take, call.handle) == ((1, 3), 3, None)
    kite = call.files[DIGEST]
    assert kite == fx.AdapterFile(
        digest=DIGEST, kind="image/png", size=4, path=tmp_path / "kite.png"
    )
    assert kite.read_bytes() == b"kite"
    # The request is a copy, its file values the call's files.
    assert call.request["references"][0] is kite
    assert call.request["other"] == {"file": "x.png"}
    assert params["request"]["references"] == [{"file": DIGEST}]
    assert repr(call) == "<AdapterCall adapter.send image.generate on img-a@acme attempt 2 take 3>"

    collecting = _params(handle={"job": "j-1"})
    _valid(COLLECT_PARAMS, collecting)
    assert fx.AdapterCall(collecting, COLLECT).handle == {"job": "j-1"}


@pytest.mark.parametrize(
    ("method", "changes", "message"),
    [
        (SEND, {"call_id": ""}, "adapter.send's call_id is a string"),
        (SEND, {"adapter": {"path": "a.py"}}, "adapter.send's adapter is {path, attribute}"),
        (SEND, {"capability": 3}, "adapter.send's capability is a capability's name"),
        (
            SEND,
            {"route": {"id": "img-a@acme", "model": "img-a", "provider": "acme"}},
            "adapter.send's route is {id, model, provider, contract}",
        ),
        (SEND, {"base_url": ""}, "adapter.send's base_url is a URL"),
        (SEND, {"request": []}, "adapter.send's request is a JSON object"),
        (
            SEND,
            {"files": {DIGEST: {"digest": DIGEST, "kind": "image/png", "size": -1, "path": "/x"}}},
            "adapter.send's files are {digest, kind, size, path} by digest",
        ),
        (SEND, {"take": []}, "adapter.send's take is a list of take numbers"),
        (SEND, {"take": [True]}, "adapter.send's take is a list of take numbers"),
        (SEND, {"key": None}, "adapter.send's key is the call key"),
        (SEND, {"attempt": 7}, "adapter.send's attempt is from 1 to 6"),
        (SEND, {"attempt": True}, "adapter.send's attempt is from 1 to 6"),
        (SEND, {"handle": {}}, "adapter.send has no handle"),
        (SUBMIT, {"handle": {}}, "adapter.submit has no handle"),
        (COLLECT, {}, "adapter.collect's handle is a JSON object"),
        (COLLECT, {"handle": ["j-1"]}, "adapter.collect's handle is a JSON object"),
        (
            SEND,
            {"request": {"image": {"file": DIGEST}}},
            f"the request holds the file {DIGEST}, which files does not hold",
        ),
    ],
)
def test_params_that_are_not_a_calls(method: str, changes: dict[str, Any], message: str) -> None:
    with pytest.raises(ValueError) as raised:
        fx.AdapterCall(_params(**changes), method)
    assert str(raised.value) == message


def test_params_that_are_not_an_object() -> None:
    with pytest.raises(ValueError) as raised:
        fx.AdapterCall([], SEND)  # type: ignore[arg-type]
    assert str(raised.value) == "adapter.send's params are a JSON object"
    with pytest.raises(ValueError) as raised:
        fx.AdapterCall(_params(), "adapter.load")
    assert str(raised.value) == "adapter.load is not an adapter call"


# --- fetch ------------------------------------------------------------------------------------


class _Engine:
    """Answers each ``fetch`` with the next of ``answers``, keeping what it was sent."""

    def __init__(self, *answers: Any) -> None:
        self.answers = list(answers)
        self.sent: list[dict[str, Any]] = []

    async def __call__(self, params: dict[str, Any]) -> Any:
        self.sent.append(params)
        answer = self.answers.pop(0)
        if isinstance(answer, BaseException):
            raise answer
        return answer


def test_fetch_sends_the_call_id_and_reads_the_response() -> None:
    engine = _Engine(
        {
            "status": 200,
            "headers": [["content-type", "application/json"], ["x-acme", "1"]],
            "base64": _b64(b'{"ok": true}'),
        }
    )
    call = fx.AdapterCall(_params(call_id="c7"), fetch=engine)
    response = asyncio.run(call.fetch("https://api.acme.test/v1/models"))
    assert engine.sent == [
        {
            "call_id": "c7",
            "method": "GET",
            "url": "https://api.acme.test/v1/models",
            "lane": "provider",
        }
    ]
    assert response == fx.FetchResponse(
        status=200,
        headers=(("content-type", "application/json"), ("x-acme", "1")),
        body=b'{"ok": true}',
    )
    assert response.json() == {"ok": True}
    assert response.text() == '{"ok": true}'


def test_fetch_options_on_the_wire() -> None:
    sent = [
        fetch_params(
            "c1",
            "https://api.acme.test/v1/draw",
            method="post",
            headers={"Accept": "application/json", "x-trace": "on"},
            json={"prompt": "a kite"},
            credential=True,
            timeout_s=30,
            max_response_bytes=1024,
        ),
        fetch_params(
            "c1",
            "https://api.acme.test/v1/files",
            method="POST",
            headers=[("X-Kind", "png")],
            body=b"\x89PNG",
            content_type="image/png",
            lane="upload",
            timeout_s=0.5,
        ),
        fetch_params("c1", "https://files.acme.test/a.png", lane="download", headers=[]),
    ]
    assert sent == [
        {
            "call_id": "c1",
            "method": "POST",
            "url": "https://api.acme.test/v1/draw",
            "headers": [["accept", "application/json"], ["x-trace", "on"]],
            "body": {"json": {"prompt": "a kite"}},
            "lane": "provider",
            "credential": True,
            "timeout_s": 30,
            "max_response_bytes": 1024,
        },
        {
            "call_id": "c1",
            "method": "POST",
            "url": "https://api.acme.test/v1/files",
            "headers": [["x-kind", "png"]],
            "body": {"base64": _b64(b"\x89PNG"), "content_type": "image/png"},
            "lane": "upload",
            "timeout_s": 0.5,
        },
        {
            "call_id": "c1",
            "method": "GET",
            "url": "https://files.acme.test/a.png",
            "lane": "download",
        },
    ]
    for params in sent:
        _valid(FETCH_PARAMS, params)


@pytest.mark.parametrize(
    ("options", "error", "message"),
    [
        ({"url": 3}, TypeError, "fetch's url is a string"),
        ({"url": ""}, ValueError, "fetch's url is an absolute URL"),
        ({"method": "PUT"}, ValueError, "fetch takes GET or POST, not PUT"),
        ({"json": {"a": 1}}, ValueError, "a GET has no body"),
        ({"body": b"x", "content_type": "text/plain"}, ValueError, "a GET has no body"),
        (
            {"method": "POST", "json": 1, "body": b"x", "content_type": "text/plain"},
            ValueError,
            "fetch takes json or body, not both",
        ),
        ({"method": "POST", "body": b"x"}, ValueError, "a body needs its content_type"),
        ({"method": "POST", "body": "x", "content_type": "a/b"}, TypeError, "fetch's body is"),
        ({"content_type": "text/plain"}, ValueError, "content_type goes with a body"),
        ({"lane": "cdn"}, ValueError, "fetch's lane is provider, upload or download, not 'cdn'"),
        ({"credential": "yes"}, TypeError, "credential is True or False"),
        ({"headers": {"x y": "1"}}, ValueError, 'the header name "x y" is not a token'),
        ({"headers": [("a",)]}, TypeError, "fetch's headers are a mapping or (name, value)"),
        ({"headers": {"a": 1}}, TypeError, "a header's name and value are strings"),
        ({"timeout_s": 0}, ValueError, "timeout_s is above 0 and at most 300"),
        ({"timeout_s": 301}, ValueError, "timeout_s is above 0 and at most 300"),
        ({"timeout_s": float("nan")}, ValueError, "timeout_s is above 0 and at most 300"),
        ({"timeout_s": 10**400}, ValueError, "timeout_s is above 0 and at most 300"),
        ({"max_response_bytes": 0}, ValueError, "max_response_bytes is from 1 to 67108864"),
        (
            {"max_response_bytes": 64 * 1024 * 1024 + 1},
            ValueError,
            "max_response_bytes is from 1 to 67108864",
        ),
    ],
)
def test_fetch_refusals_before_anything_is_sent(
    options: dict[str, Any], error: type[Exception], message: str
) -> None:
    engine = _Engine()
    call = fx.AdapterCall(_params(), fetch=engine)
    url = options.pop("url", "https://api.acme.test/v1")
    with pytest.raises(error) as raised:
        asyncio.run(call.fetch(url, **options))
    assert str(raised.value).startswith(message)
    assert engine.sent == []


def test_a_failed_exchange_and_what_the_sdk_cannot_read() -> None:
    for phase in ("refused", "not_sent", "after_send"):
        with pytest.raises(fx.FetchError) as raised:
            read_response({"error": {"phase": phase, "reason": "the network is off"}})
        assert (raised.value.phase, raised.value.reason) == (phase, "the network is off")
        assert str(raised.value) == "the network is off"
        assert repr(raised.value) == f"FetchError({phase!r}, 'the network is off')"
    for unreadable in (
        None,
        {"error": {"phase": "lost", "reason": "x"}},
        {"status": 200, "headers": [], "base64": "not base64!"},
        {"status": True, "headers": [], "base64": ""},
        {"status": 200, "headers": [["a"]], "base64": ""},
    ):
        with pytest.raises(EngineError) as engine_error:
            read_response(unreadable)
        assert engine_error.value.code == -32099
        assert str(engine_error.value) == (
            "the engine answered fetch with something the SDK cannot read"
        )
    # An error answer reaches the adapter as the engine's error.
    call = fx.AdapterCall(_params(), fetch=_Engine(Cancelled("the adapter call was dropped")))
    with pytest.raises(Cancelled):
        asyncio.run(call.fetch("https://api.acme.test/v1"))


def test_a_call_without_an_engine_cannot_fetch() -> None:
    with pytest.raises(RuntimeError) as raised:
        asyncio.run(fx.AdapterCall(_params()).fetch("https://api.acme.test/v1"))
    assert str(raised.value) == "this call has no engine to fetch through"


# --- outcomes ---------------------------------------------------------------------------------


def test_outcomes_on_the_wire(tmp_path: Path) -> None:
    clip = tmp_path / "clip.mp4"
    clip.write_bytes(b"clip")
    (tmp_path / "kite.png").write_bytes(b"kite")
    kite = fx.AdapterFile(digest=DIGEST, kind="image/png", size=4, path=tmp_path / "kite.png")
    cases: list[tuple[str, Any, dict[str, Any]]] = [
        (
            SEND,
            fx.Answered(
                files={
                    "raw": b"raw",
                    "made": fx.Output(kind="model/gltf-binary", data=b"glTF"),
                    "echo": kite,
                    "path": clip,
                },
                data={"seed": 7},
                cost_usd=0.04,
                usage={"input_tokens": 12, "output_tokens": 3},
            ),
            {
                "outcome": "answered",
                "files": {
                    "raw": {"base64": _b64(b"raw")},
                    "made": {"base64": _b64(b"glTF"), "kind": "model/gltf-binary"},
                    "echo": {"base64": _b64(b"kite"), "kind": "image/png"},
                    "path": {"base64": _b64(b"clip"), "kind": "video/mp4"},
                },
                "data": {"seed": 7},
                "cost_usd": 0.04,
                "usage": {"input_tokens": 12, "output_tokens": 3},
            },
        ),
        (
            SEND,
            fx.Answered.json({"name": "teal"}, cost_usd=0),
            {"outcome": "answered", "files": {}, "data": {"json": {"name": "teal"}}, "cost_usd": 0},
        ),
        (
            SEND,
            fx.NotReceived("acme is busy", retry_after_s=2.5),
            {"outcome": "not_received", "reason": "acme is busy", "retry_after_s": 2.5},
        ),
        (SUBMIT, fx.NotReceived("no route"), {"outcome": "not_received", "reason": "no route"}),
        (SEND, fx.Refused("a blank prompt"), {"outcome": "refused", "reason": "a blank prompt"}),
        (
            SEND,
            fx.Failed("acme returned HTTP 500", retryable=True),
            {"outcome": "failed", "reason": "acme returned HTTP 500", "retryable": True},
        ),
        (
            SUBMIT,
            fx.Failed("acme returned HTTP 400", retryable=False, cost_usd=0.01),
            {
                "outcome": "failed",
                "reason": "acme returned HTTP 400",
                "retryable": False,
                "cost_usd": 0.01,
            },
        ),
        (
            SUBMIT,
            fx.Accepted({"job": "j-1", "poll": [1, 2]}),
            {"outcome": "accepted", "handle": {"job": "j-1", "poll": [1, 2]}},
        ),
        (SUBMIT, fx.Uncertain("timed out"), {"outcome": "uncertain", "reason": "timed out"}),
        (COLLECT, fx.Answered(), {"outcome": "answered", "files": {}, "data": None}),
        (
            COLLECT,
            fx.Ended("the job was cancelled", cost_usd=0),
            {"outcome": "ended", "reason": "the job was cancelled", "cost_usd": 0},
        ),
        (COLLECT, fx.Ended("expired"), {"outcome": "ended", "reason": "expired"}),
        (
            COLLECT,
            fx.Unreachable("still running"),
            {"outcome": "unreachable", "reason": "still running"},
        ),
    ]
    for method, outcome, wire in cases:
        sent = encode(method, outcome)
        assert sent == wire
        _valid(RESULTS[method], sent)


@pytest.mark.parametrize(
    ("make", "error", "message"),
    [
        (lambda: fx.Refused(""), ValueError, "a reason is a sentence, not a blank string"),
        (lambda: fx.Uncertain("  \n"), ValueError, "a reason is a sentence, not a blank string"),
        (lambda: fx.Unreachable(None), TypeError, "a reason is a sentence, not NoneType"),
        (lambda: fx.Failed("x", retryable=1), TypeError, "retryable is True or False"),
        (lambda: fx.Failed("x", retryable=True, cost_usd=-1), ValueError, "cost_usd is a number"),
        (lambda: fx.Ended("x", cost_usd=float("inf")), ValueError, "cost_usd is a number of"),
        (lambda: fx.Ended("x", cost_usd=-(10**400)), ValueError, "cost_usd is a number of"),
        (lambda: fx.Answered(cost_usd="0.1"), TypeError, "cost_usd is a number, not str"),
        (lambda: fx.Answered(cost_usd=True), TypeError, "cost_usd is a number, not bool"),
        (lambda: fx.NotReceived("x", retry_after_s=-1), ValueError, "retry_after_s is a number"),
        (lambda: fx.Answered(usage={"tokens": 1}), ValueError, "usage counts input_tokens"),
        (
            lambda: fx.Answered(usage={"input_tokens": -1}),
            ValueError,
            "usage input_tokens is an integer of at least 0, not -1",
        ),
        (lambda: fx.Answered(usage=[1]), TypeError, "usage is a mapping of token counts"),
        (lambda: fx.Accepted(["j-1"]), TypeError, "a handle is a mapping, not list"),
    ],
)
def test_outcomes_hold_their_members(make: Any, error: type[Exception], message: str) -> None:
    with pytest.raises(error) as raised:
        make()
    assert str(raised.value).startswith(message)


def test_failed_names_whether_it_may_be_made_again() -> None:
    with pytest.raises(TypeError):
        fx.Failed("acme returned HTTP 500")  # type: ignore[call-arg]
    with pytest.raises(TypeError):
        fx.Failed("acme returned HTTP 500", True)  # type: ignore[misc]


def test_each_method_takes_its_outcomes() -> None:
    for method, outcome, message in (
        (SEND, fx.Accepted({}), "send returns Answered, NotReceived, Refused or Failed, not"),
        (SEND, None, "send returns Answered, NotReceived, Refused or Failed, not None"),
        (
            SUBMIT,
            fx.Answered(),
            "submit returns Accepted, Uncertain, NotReceived, Refused or Failed, not Answered",
        ),
        (COLLECT, fx.Refused("x"), "collect returns Answered, Ended or Unreachable, not Refused"),
        (COLLECT, {"outcome": "answered"}, "collect returns Answered, Ended or Unreachable"),
    ):
        with pytest.raises(TypeError) as raised:
            encode(method, outcome)
        assert str(raised.value).startswith(message)


def test_what_cannot_be_sent(tmp_path: Path) -> None:
    for outcome, error, message in (
        (
            fx.Answered(data=float("nan")),
            ValueError,
            "an answer's data is not a JSON value FX can read: at /data: nan is not a JSON number",
        ),
        (
            fx.Answered(files={"Image": b"x"}),
            ValueError,
            "an answer's files are named in lowercase ([a-z][a-z0-9_]*), not 'Image'",
        ),
        (
            fx.Answered(files={"image": "x.png"}),
            TypeError,
            "an answer's file is bytes, an Output, an AdapterFile or a path; image is str",
        ),
        (
            fx.Answered(files={"image": fx.Output(kind="image/png", work_path="out/a.png")}),
            TypeError,
            "an answer's file is bytes: image is",
        ),
        (
            fx.Accepted({"at": float("inf")}),
            ValueError,
            "a handle is not a JSON object FX can read: at /handle/at: inf is not a JSON number",
        ),
    ):
        with pytest.raises(error) as raised:
            encode(SEND if not isinstance(outcome, fx.Accepted) else SUBMIT, outcome)
        assert str(raised.value).startswith(message)
    with pytest.raises(OSError):
        encode(SEND, fx.Answered(files={"image": tmp_path / "missing.png"}))


# --- serving ----------------------------------------------------------------------------------


class _Replies:
    """What a :class:`Server` answered, by request id, in order."""

    def __init__(self) -> None:
        self.answers: list[tuple[Any, dict[str, Any]]] = []
        self.arrived = asyncio.Event()

    def to(self, request_id: Any) -> Any:
        def reply(answer: dict[str, Any]) -> None:
            self.answers.append((request_id, answer))
            self.arrived.set()

        return reply

    async def wait_for(self, count: int) -> None:
        while len(self.answers) < count:
            self.arrived.clear()
            await asyncio.wait_for(self.arrived.wait(), 10)


def test_the_server_answers_calls_at_once_and_in_any_order() -> None:
    async def scenario() -> list[tuple[Any, dict[str, Any]]]:
        release = asyncio.Event()
        threads: list[str] = []

        async def slow(call: fx.AdapterCall) -> fx.Answered:
            await release.wait()
            return fx.Answered.json(call.request["prompt"])

        def plain(call: fx.AdapterCall) -> fx.Refused:
            threads.append(threading.current_thread().name)
            return fx.Refused(f"no {call.request['prompt']} today")

        async def accepts(call: fx.AdapterCall) -> fx.Accepted:
            return fx.Accepted({"job": call.key[:4]})

        server = Server()
        replies = _Replies()
        server.start(1, SEND, slow, fx.AdapterCall(_params()), replies.to(1), "a.py#a")
        server.start(
            2,
            SEND,
            plain,
            fx.AdapterCall(_params(request={"prompt": "kites"})),
            replies.to(2),
            "a.py#a",
        )
        server.start(3, SUBMIT, accepts, fx.AdapterCall(_params(), SUBMIT), replies.to(3), "a.py#a")
        await replies.wait_for(2)
        release.set()
        await replies.wait_for(3)
        assert threads == ["grida.fx adapter"]
        return replies.answers

    answers = asyncio.run(scenario())
    assert sorted(answers[:2], key=lambda answer: answer[0]) == [
        (2, {"result": {"outcome": "refused", "reason": "no kites today"}}),
        (3, {"result": {"outcome": "accepted", "handle": {"job": "aaaa"}}}),
    ]
    assert answers[2] == (
        1,
        {"result": {"outcome": "answered", "files": {}, "data": {"json": "a kite"}}},
    )


def test_the_server_cancels_calls() -> None:
    async def scenario() -> tuple[list[tuple[Any, dict[str, Any]]], list[str]]:
        seen: list[str] = []
        finishing = threading.Event()
        finished = asyncio.Event()
        loop = asyncio.get_running_loop()

        async def waits(call: fx.AdapterCall) -> fx.Answered:
            try:
                await asyncio.sleep(30)
            except asyncio.CancelledError:
                seen.append("cancelled")
                raise
            return fx.Answered()

        def blocks(call: fx.AdapterCall) -> fx.Answered:
            finishing.wait(10)
            loop.call_soon_threadsafe(finished.set)
            return fx.Answered()

        server = Server()
        replies = _Replies()
        server.start(1, SEND, waits, fx.AdapterCall(_params()), replies.to(1), "a.py#a")
        server.start(2, SEND, blocks, fx.AdapterCall(_params()), replies.to(2), "a.py#a")
        await asyncio.sleep(0.05)
        # Each is answered `cancelled` at once; the coroutine function's task is cancelled.
        server.cancel(1)
        server.cancel(2)
        server.cancel(2)  # once only
        server.cancel(99)  # names no call
        await replies.wait_for(2)
        await asyncio.sleep(0.05)
        # The plain function runs to its end, and what it returns is discarded.
        finishing.set()
        await asyncio.wait_for(finished.wait(), 10)
        await asyncio.sleep(0.05)
        return replies.answers, seen

    answers, seen = asyncio.run(scenario())
    assert answers == [
        (1, {"error": {"code": -32002, "message": "the engine cancelled adapter.send"}}),
        (2, {"error": {"code": -32002, "message": "the engine cancelled adapter.send"}}),
    ]
    assert seen == ["cancelled"]


def test_a_fault_ends_only_its_own_call() -> None:
    faults: list[tuple[str, str]] = []

    async def scenario() -> list[tuple[Any, dict[str, Any]]]:
        async def breaks(call: fx.AdapterCall) -> Any:
            prompt = call.request["prompt"]
            if prompt == "boom":
                raise RuntimeError("acme broke")
            if prompt == "wrong":
                return fx.Accepted({})
            if prompt == "cancelled":
                raise asyncio.CancelledError()
            if prompt == "exit":
                raise SystemExit(3)
            return fx.Answered.json(prompt)

        def plain_breaks(call: fx.AdapterCall) -> Any:
            raise KeyError("model")

        server = Server(on_fault=lambda error, name: faults.append((type(error).__name__, name)))
        replies = _Replies()
        prompts = ["boom", "wrong", "cancelled", "exit", "fine"]
        for number, prompt in enumerate(prompts, 1):
            call = fx.AdapterCall(_params(request={"prompt": prompt}))
            server.start(number, SEND, breaks, call, replies.to(number), "adapters/acme.py#acme")
        server.start(6, SEND, plain_breaks, fx.AdapterCall(_params()), replies.to(6), "x.py#x")
        await replies.wait_for(6)
        return sorted(replies.answers, key=lambda answer: answer[0])

    answers = asyncio.run(scenario())
    internal = -32099
    assert answers == [
        (1, {"error": {"code": internal, "message": "RuntimeError: acme broke"}}),
        (
            2,
            {
                "error": {
                    "code": internal,
                    "message": "TypeError: send returns Answered, NotReceived, Refused or Failed,"
                    " not Accepted",
                }
            },
        ),
        (3, {"error": {"code": internal, "message": "CancelledError"}}),
        (4, {"error": {"code": internal, "message": "SystemExit: 3"}}),
        (5, {"result": {"outcome": "answered", "files": {}, "data": {"json": "fine"}}}),
        (6, {"error": {"code": internal, "message": "KeyError: 'model'"}}),
    ]
    assert sorted(faults) == [
        ("CancelledError", "adapters/acme.py#acme"),
        ("KeyError", "x.py#x"),
        ("RuntimeError", "adapters/acme.py#acme"),
        ("SystemExit", "adapters/acme.py#acme"),
        ("TypeError", "adapters/acme.py#acme"),
    ]


def test_closing_the_server_answers_nothing() -> None:
    async def scenario() -> list[tuple[Any, dict[str, Any]]]:
        async def waits(call: fx.AdapterCall) -> fx.Answered:
            await asyncio.sleep(30)
            return fx.Answered()

        server = Server()
        replies = _Replies()
        server.start(1, SEND, waits, fx.AdapterCall(_params()), replies.to(1), "a.py#a")
        await asyncio.sleep(0.05)
        server.close()
        await asyncio.sleep(0.05)
        return replies.answers

    assert asyncio.run(scenario()) == []
