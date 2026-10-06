"""What the SDK's stand-in answerers share (``grida.fx._stand_in``; spec/protocol.md section 5.7):
the call, the answer's encoding, and the :class:`Answerer` driven on a loop without an engine."""

from __future__ import annotations

import asyncio
import base64
import json
import os
import threading
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator

import grida.fx as fx
from grida.fx._protocol import frame
from grida.fx._stand_in import Answerer, Refused, StandInCall, encode, initialize

REPO = Path(__file__).resolve().parents[2]
SCHEMA = json.loads(
    (REPO / "spec" / "schemas" / "fx-node-protocol-v1.schema.json").read_text("utf-8")
)
ANSWER_RESULT = Draft202012Validator(
    {"$ref": "#/$defs/stand_in_answer_result", "$defs": SCHEMA["$defs"]}
)
KEY = "a" * 64
DIGEST = "d" * 64


def _params(**changes: Any) -> dict[str, Any]:
    params: dict[str, Any] = {
        "capability": "image.generate",
        "route": {"id": "img-a@acme", "fingerprint": "e" * 64},
        "request": {"prompt": "a lighthouse"},
        "take": [1],
        "key": KEY,
        "instance": {"id": "draw#1", "path": "draw", "step": "draw"},
        "files": {},
    }
    params.update(changes)
    return params


def _b64(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


# --- the call ---------------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("changes", "message"),
    [
        ({"capability": 3}, "stand_in.answer's capability is a capability's name"),
        ({"route": {"id": "img-a@acme"}}, "stand_in.answer's route is {id, fingerprint}"),
        ({"take": []}, "stand_in.answer's take is a list of take numbers"),
        ({"take": [0]}, "stand_in.answer's take is a list of take numbers"),
        ({"take": [True]}, "stand_in.answer's take is a list of take numbers"),
        ({"key": ""}, "stand_in.answer's key is the call key"),
        ({"instance": {"id": "draw#1"}}, "stand_in.answer's instance is {id, path, step}"),
        (
            {"files": {DIGEST: {"digest": DIGEST}}},
            "stand_in.answer's files are file refs by digest",
        ),
        ({"request": []}, "stand_in.answer's request is a JSON object"),
        (
            {"request": {"image": {"file": DIGEST}}},
            f"the request holds the file {DIGEST}, which files does not hold",
        ),
    ],
)
def test_params_that_are_not_a_calls(changes: dict[str, Any], message: str) -> None:
    with pytest.raises(ValueError) as raised:
        StandInCall(_params(**changes))
    assert str(raised.value) == message


def test_only_a_digest_makes_a_file_value(tmp_path: Path) -> None:
    ref = {"digest": DIGEST, "kind": "image/png", "size": 1, "name": "a.png", "path": "/x"}
    request = {
        "a": {"file": DIGEST},
        "b": {"file": DIGEST, "also": 1},
        "c": {"file": "a.png"},
        "d": [[{"file": DIGEST}]],
    }
    call = StandInCall(_params(request=request, files={DIGEST: ref}))
    assert call.request["a"] is call.files[DIGEST]
    assert call.request["b"] == {"file": DIGEST, "also": 1}
    assert call.request["c"] == {"file": "a.png"}
    assert call.request["d"][0][0] is call.files[DIGEST]
    # The request is a copy: the params keep their file values.
    assert call.params["request"] is request
    assert request["a"] == {"file": DIGEST}


# --- the answer -------------------------------------------------------------------------------


def test_answers_and_what_they_send(tmp_path: Path) -> None:
    picture = tmp_path / "clip.mp4"
    picture.write_bytes(b"clip")
    source = {
        "digest": DIGEST,
        "kind": "audio/mpeg",
        "size": 3,
        "name": "a.mp3",
        "path": str(tmp_path / "a.mp3"),
        "facts": {"bytes": 3, "kind": "audio/mpeg"},
    }
    (tmp_path / "a.mp3").write_bytes(b"mp3")
    answer = fx.Answer(
        files={
            "raw": b"raw",
            "array": bytearray(b"array"),
            "made": fx.Output(kind="model/gltf-binary", data=b"glTF"),
            "echo": fx.InputFile(source),
            "path": picture,
        },
        data={"facts": None},
    )
    sent = encode(answer)
    assert sent == {
        "files": {
            "raw": {"base64": _b64(b"raw")},
            "array": {"base64": _b64(b"array")},
            "made": {"base64": _b64(b"glTF"), "kind": "model/gltf-binary"},
            "echo": {"base64": _b64(b"mp3"), "kind": "audio/mpeg"},
            "path": {"base64": _b64(b"clip"), "kind": "video/mp4"},
        },
        "data": {"facts": None},
    }
    assert list(ANSWER_RESULT.iter_errors(sent)) == []
    assert encode(fx.DECLINE) == {"decline": True}
    assert encode(fx.Answer()) == {"files": {}, "data": None}
    assert repr(fx.DECLINE) == "grida.fx.DECLINE"


def test_answer_helpers() -> None:
    assert fx.Answer.json([1, "two"]) == fx.Answer(data={"json": [1, "two"]})
    assert fx.Answer.turn().data == {"text": "", "tool_calls": []}
    assert fx.Answer.turn("done").data == {"text": "done", "tool_calls": []}
    calls = [{"name": "a"}, {"name": "b", "id": "x", "arguments": {"n": 1}}, {"name": "c"}]
    assert fx.Answer.turn(tool_calls=calls).data["tool_calls"] == [
        {"id": "call_1", "name": "a", "arguments": {}},
        {"id": "x", "name": "b", "arguments": {"n": 1}},
        {"id": "call_3", "name": "c", "arguments": {}},
    ]
    assert fx.Answer.submit().data == {
        "text": "",
        "tool_calls": [{"id": "call_1", "name": "submit", "arguments": {}}],
    }


@pytest.mark.parametrize(
    ("returned", "error", "message"),
    [
        (None, TypeError, "a stand-in returns an Answer or grida.fx.DECLINE, not None"),
        (b"png", TypeError, "a stand-in returns an Answer or grida.fx.DECLINE, not bytes"),
        (
            fx.Answer(files={"data": fx.Output(kind="json", json={"a": 1})}),
            TypeError,
            "an answer's file is bytes: data is Output(kind='json', json)",
        ),
        (
            fx.Answer(files={"out": fx.Output(kind="image/png", work_path="out/a.png")}),
            TypeError,
            "an answer's file is bytes: out is Output(kind='image/png', work_path='out/a.png')",
        ),
        (
            fx.Answer(files={"image": "picture.png"}),
            TypeError,
            "an answer's file is bytes, an Output, an InputFile or a path; image is str",
        ),
        (
            fx.Answer(files={"Image": b"png"}),
            ValueError,
            "an answer's files are named in lowercase ([a-z][a-z0-9_]*), not 'Image'",
        ),
        (
            fx.Answer(files=[b"png"]),  # type: ignore[arg-type]
            TypeError,
            "an answer's files are a mapping of names to files",
        ),
        (
            fx.Answer(data={"score": float("nan")}),
            ValueError,
            "an answer's data is not a JSON value FX can read: at /data/score: nan is not a JSON"
            " number",
        ),
    ],
)
def test_what_a_stand_in_cannot_answer(returned: Any, error: type, message: str) -> None:
    with pytest.raises(error) as raised:
        encode(returned)
    assert str(raised.value) == message


def test_initialize() -> None:
    result = initialize({"protocol": "fx-node-protocol-v1"})
    assert result["protocol"] == "fx-node-protocol-v1"
    assert set(result["host"]) == {"language", "version", "sdk_version"}
    with pytest.raises(Refused) as mismatch:
        initialize({"protocol": "fx-node-protocol-v9"})
    assert mismatch.value.error["code"] == -32003
    assert mismatch.value.error["data"] == {
        "engine_protocol": "fx-node-protocol-v9",
        "host_protocol": "fx-node-protocol-v1",
    }
    with pytest.raises(Refused) as missing:
        initialize(None)
    assert missing.value.error == {
        "code": -32602,
        "message": "initialize needs the engine's protocol",
    }


# --- the answerer -----------------------------------------------------------------------------


def _answer_all(answerer: Answerer, *asks: tuple[Any, Any], cancel: Any = None) -> list[Any]:
    """Starts every ``(request_id, params)`` on a loop (cancelling ``cancel`` right after) and
    returns the replies in the order they came."""
    replies: list[Any] = []

    async def main() -> None:
        done = asyncio.Event()

        def reply(request_id: Any) -> Any:
            def answered(message: dict[str, Any]) -> None:
                replies.append((request_id, message))
                if len(replies) == len(asks):
                    done.set()

            return answered

        for request_id, params in asks:
            answerer.start(request_id, params, reply(request_id))
        if cancel is not None:
            answerer.cancel(cancel)
        await asyncio.wait_for(done.wait(), 10)

    asyncio.run(main())
    return replies


def test_an_answerer_refuses_params_it_cannot_read() -> None:
    answerer = Answerer(lambda call: fx.DECLINE)
    replies = _answer_all(answerer, (1, "not params"), (2, _params()))
    assert replies == [
        (1, {"error": {"code": -32602, "message": "stand_in.answer's params are a JSON object"}}),
        (2, {"result": {"decline": True}}),
    ]
    assert answerer.fault is None


def test_an_answerer_keeps_the_first_fault() -> None:
    told: list[BaseException] = []
    first = RuntimeError("first")

    def answer(call: StandInCall) -> Any:
        raise first

    answerer = Answerer(answer, on_fault=told.append)
    replies = _answer_all(answerer, (1, _params()), (2, _params()))
    assert replies == [
        (1, {"error": {"code": -32099, "message": "RuntimeError: first"}}),
        (2, {"error": {"code": -32099, "message": "the stand-in failed earlier"}}),
    ]
    assert answerer.fault is first
    assert told == [first]


def test_a_cancelled_error_the_stand_in_raises_itself_is_its_fault() -> None:
    async def answer(call: StandInCall) -> Any:
        raise asyncio.CancelledError()

    answerer = Answerer(answer)
    (reply,) = _answer_all(answerer, (1, _params()))
    assert reply == (1, {"error": {"code": -32099, "message": "CancelledError"}})
    assert isinstance(answerer.fault, asyncio.CancelledError)


def test_a_call_cancelled_before_it_starts_is_answered_cancelled() -> None:
    asked: list[str] = []
    answerer = Answerer(lambda call: asked.append(call.key) or fx.DECLINE)
    (reply,) = _answer_all(answerer, ("x", _params()), cancel="x")
    cancelled = {"code": -32002, "message": "the engine cancelled stand_in.answer"}
    assert reply == ("x", {"error": cancelled})
    assert asked == []


def test_cancel_names_nothing_and_close_answers_nothing() -> None:
    replies: list[Any] = []

    async def answer(call: StandInCall) -> Any:
        await asyncio.sleep(30)

    async def main() -> None:
        answerer = Answerer(answer)
        answerer.cancel("nothing")
        answerer.start(1, _params(), replies.append)
        await asyncio.sleep(0.05)
        answerer.close()
        await asyncio.sleep(0.05)

    asyncio.run(main())
    assert replies == []


def test_an_answerer_takes_a_function() -> None:
    with pytest.raises(TypeError) as raised:
        Answerer(3)  # type: ignore[arg-type]
    assert str(raised.value) == "a stand-in is a function of one StandInCall, not int"


def test_an_error_answer_holding_a_lone_surrogate_can_be_sent() -> None:
    name = os.fsdecode(b"gull\xff.png")

    def answer(call: StandInCall) -> Any:
        if call.key == KEY:
            raise fx.CallRefused("no file " + name)
        raise RuntimeError("cannot read " + name)

    answerer = Answerer(answer)
    replies = _answer_all(answerer, (1, _params()), (2, _params(key="b" * 64)))
    assert replies == [
        (1, {"error": {"code": fx.CallRefused.code, "message": "no file gull\\udcff.png"}}),
        (2, {"error": {"code": -32099, "message": "RuntimeError: cannot read gull\\udcff.png"}}),
    ]
    for _, message in replies:
        frame({"jsonrpc": "2.0", "id": 1, **message})  # raises when it cannot be sent


def test_a_cancelled_plain_function_that_raises_is_no_fault() -> None:
    started = threading.Event()
    go_on = threading.Event()

    def answer(call: StandInCall) -> Any:
        if call.key == KEY:
            started.set()
            go_on.wait(10)
            raise RuntimeError("late")
        return fx.DECLINE

    replies: list[Any] = []
    answerer = Answerer(answer)

    async def main() -> None:
        done = asyncio.Event()

        def reply(request_id: Any) -> Any:
            def answered(message: dict[str, Any]) -> None:
                replies.append((request_id, message))
                if len(replies) == 2:
                    done.set()

            return answered

        answerer.start(1, _params(), reply(1))
        await asyncio.to_thread(started.wait, 10)
        answerer.cancel(1)  # the function is already running on its thread
        go_on.set()
        answerer.start(2, _params(key="b" * 64), reply(2))
        await asyncio.wait_for(done.wait(), 10)

    asyncio.run(main())
    cancelled = {"code": -32002, "message": "the engine cancelled stand_in.answer"}
    assert replies == [(1, {"error": cancelled}), (2, {"result": {"decline": True}})]
    assert answerer.fault is None


def test_a_cancelled_plain_function_still_sends_its_answer() -> None:
    started = threading.Event()
    go_on = threading.Event()

    def answer(call: StandInCall) -> Any:
        started.set()
        go_on.wait(10)
        return fx.DECLINE

    replies: list[Any] = []
    answerer = Answerer(answer)

    async def main() -> None:
        done = asyncio.Event()
        answerer.start(1, _params(), lambda message: (replies.append(message), done.set()))
        await asyncio.to_thread(started.wait, 10)
        answerer.cancel(1)
        go_on.set()
        await asyncio.wait_for(done.wait(), 10)

    asyncio.run(main())
    assert replies == [{"result": {"decline": True}}]
