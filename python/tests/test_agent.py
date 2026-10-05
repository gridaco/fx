"""Agents and their tools over a fake channel (spec/protocol.md sections 5.4, 5.5, 6.2)."""

from __future__ import annotations

import asyncio
import base64
import hashlib
import inspect
import json
import traceback
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from grida.fx import (
    Agent,
    Ctx,
    EngineError,
    InputFile,
    NodeFailure,
    Tool,
    ToolInvocationError,
    ToolReply,
    ToolResult,
    tool,
)
from grida.fx._agent import tool_schema
from grida.fx._errors import AgentUnfinished, engine_error
from grida.fx._protocol import check_value

PNG_URL = "data:image/png;base64," + base64.b64encode(b"png bytes").decode()


class FakeChannel:
    """Records requests; ``file.put`` stores into a folder; ``agent.run`` is answered by an async
    function given the params (it may call back into the agent, as the engine does)."""

    def __init__(self, store: Path, work_dir: Path) -> None:
        self.store = store
        self.work_dir = work_dir
        self.requests: list[tuple[str, dict[str, Any]]] = []
        self.agent_run: Callable[[dict[str, Any]], Any] | None = None
        self.run_id = "r1"

    def request(self, method: str, params: dict[str, Any]) -> Any:
        check_value(params)
        self.requests.append((method, json.loads(json.dumps(params))))
        if method == "file.put":
            if "base64" in params:
                data = base64.b64decode(params["base64"], validate=True)
            elif "work_path" in params:
                data = (self.work_dir / params["work_path"]).read_bytes()
            else:
                data = json.dumps(params["json"]).encode()
            digest = hashlib.sha256(data).hexdigest()
            self.store.mkdir(parents=True, exist_ok=True)
            (self.store / digest).write_bytes(data)
            return {
                "digest": digest,
                "kind": params.get("kind", "file"),
                "size": len(data),
                "name": params.get("name", "f"),
                "path": str(self.store / digest),
                "facts": {"bytes": len(data), "kind": params.get("kind", "file")},
            }
        if method in ("fact", "annotate"):
            return {}
        raise AssertionError(f"unexpected request {method}")

    async def request_async(self, method: str, params: dict[str, Any]) -> Any:
        if method == "agent.run":
            check_value(params)
            self.requests.append((method, json.loads(json.dumps(params))))
            assert self.agent_run is not None
            return await self.agent_run(params)
        return self.request(method, params)

    def notify(self, method: str, params: dict[str, Any]) -> None:
        raise AssertionError(method)

    def puts(self) -> list[dict[str, Any]]:
        return [params for method, params in self.requests if method == "file.put"]


@pytest.fixture
def ctx(tmp_path: Path) -> Ctx:
    work = tmp_path / "work"
    work.mkdir()
    channel = FakeChannel(tmp_path / "store", work)
    run = {
        "run_id": "r1",
        "instance": {"id": "a#1", "path": "a", "step": "a", "key": None, "take": [1]},
        "type": "t",
        "body": {"path": "nodes/a.py", "attribute": "a"},
        "params": {},
        "param_files": {},
        "inputs": {},
        "work_dir": str(work),
        "resources": {},
        "tools": {},
        "calls": {"agent.turn": 5},
        "timeout_s": None,
    }
    return Ctx(run, channel)  # type: ignore[arg-type]


def channel_of(ctx: Ctx) -> FakeChannel:
    return ctx.channel  # type: ignore[return-value]


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


# --- schemas ----------------------------------------------------------------------------------


@tool
def look(ctx: Ctx, what: str, n: int = 1) -> str:
    """Look at something."""
    return f"saw {what} x{n}"


def test_the_probed_schema() -> None:
    assert tool_schema(look) == {
        "name": "look",
        "description": "Look at something.",
        "parameters": {
            "type": "object",
            "properties": {"what": {"type": "string"}, "n": {"type": "integer"}},
            "required": ["what"],
        },
    }


def test_schemas_from_annotations() -> None:
    def every(
        context: Any,
        a: str,
        b: int,
        c: float,
        d: bool,
        e: list,  # type: ignore[type-arg]
        f: dict,  # type: ignore[type-arg]
        g,
        h: Path,
        *rest: str,
        i: int = 2,
        **more: Any,
    ) -> None:
        pass

    schema = tool(name="every_kind")(every)
    built = tool_schema(schema)
    assert built["name"] == "every_kind"
    assert built["description"] == "every"
    assert built["parameters"] == {
        "type": "object",
        "properties": {
            "a": {"type": "string"},
            "b": {"type": "integer"},
            "c": {"type": "number"},
            "d": {"type": "boolean"},
            "e": {"type": "array"},
            "f": {"type": "object"},
            "g": {"type": "string"},
            "h": {"type": "string"},
            "i": {"type": "integer"},
        },
        "required": ["a", "b", "c", "d", "e", "f", "g", "h"],
    }
    # String annotations, as `from __future__ import annotations` writes them.
    namespace: dict[str, Any] = {}
    exec(
        "from __future__ import annotations\n"
        "def strings(ctx, a: int, b: bool = False, c: Thing = None):\n"
        "    '''  Strings.\n\n    More.  '''\n",
        namespace,
    )
    assert tool_schema(namespace["strings"]) == {
        "name": "strings",
        "description": "Strings.\n\nMore.  ",  # inspect.getdoc, as the predecessor
        "parameters": {
            "type": "object",
            "properties": {
                "a": {"type": "integer"},
                "b": {"type": "boolean"},
                "c": {"type": "string"},
            },
            "required": ["a"],
        },
    }


# --- declared tools and their results ---------------------------------------------------------


def handler(arguments: Any) -> ToolResult:
    return ToolResult("ok")


def test_tool_validation() -> None:
    made = Tool(name="place_prop", description="Places a prop.", parameters={}, handler=handler)
    assert made.name == "place_prop"
    Tool(name="a" * 64, description="d", parameters={"type": "object"}, handler=handler)
    for name in ("Place", "1a", "a" * 65, "a-b", "", "a\n", 3):
        with pytest.raises(ValueError, match=r"^tool name must be lower_snake_case"):
            Tool(name=name, description="d", parameters={}, handler=handler)  # type: ignore[arg-type]
    with pytest.raises(ValueError, match=r"^submit is reserved for the episode's final answer$"):
        Tool(name="submit", description="d", parameters={}, handler=handler)
    for description in ("", "  \n"):
        with pytest.raises(ValueError, match=r"^tool place must carry a description$"):
            Tool(name="place", description=description, parameters={}, handler=handler)
    with pytest.raises(ValueError, match=r"^tool place parameters must be a JSON Schema object$"):
        Tool(name="place", description="d", parameters=[], handler=handler)  # type: ignore[arg-type]
    with pytest.raises(ValueError, match=r"^tool place handler must be callable$"):
        Tool(name="place", description="d", parameters={}, handler=3)  # type: ignore[arg-type]


def test_tool_result_validation() -> None:
    assert ToolResult("seen").images == ()
    result = ToolResult("seen", images=[PNG_URL, "https://example.com/a.png"])  # type: ignore[arg-type]
    assert result.images == (PNG_URL, "https://example.com/a.png")
    assert ToolResult("x", ("HTTP://EXAMPLE.COM/A", "DATA:IMAGE/PNG;BASE64,AAAA")).images
    for text in ("", " \n "):
        with pytest.raises(ValueError, match=r"^tool result text must be non-empty$"):
            ToolResult(text)
    for image in (
        "a.png",
        "ftp://example.com/a.png",
        "data:text/plain;base64,AAAA",
        "data:image/png,AAAA",
        "data:image/png;base64,",
        "https://",
        3,
    ):
        with pytest.raises(
            ValueError, match=r"^tool result images must be HTTP\(S\) URLs or image data URLs$"
        ):
            ToolResult("x", (image,))  # type: ignore[arg-type]
    with pytest.raises(ValueError, match="not one string"):
        ToolResult("x", PNG_URL)  # type: ignore[arg-type]
    assert issubclass(ToolInvocationError, ValueError)
    assert ToolReply("t").images == ()


def test_agent_tools_must_be_declared(ctx: Ctx) -> None:
    def plain(ctx: Ctx) -> str:
        return ""

    with pytest.raises(NodeFailure, match=r"^plain is not declared with @tool$"):
        ctx.agent(system="s", tools=[plain])
    with pytest.raises(
        NodeFailure, match=r"^submit is the agent's own tool; name yours otherwise$"
    ):
        ctx.agent(system="s", tools=[tool(name="submit")(lambda ctx: "")])
    agent = ctx.agent(system="s", tools=[look])
    assert isinstance(agent, Agent) and agent.tools == (look,)


# --- agent.run --------------------------------------------------------------------------------


def test_run_sends_agent_run(ctx: Ctx, tmp_path: Path) -> None:
    channel = channel_of(ctx)
    declared = Tool(
        name="place_prop",
        description="Places a prop.",
        parameters={"type": "object", "properties": {"x": {"type": "number"}}},
        handler=handler,
    )
    ref = channel.request("file.put", {"base64": base64.b64encode(b"in").decode()})
    inputs_file = InputFile(ref)
    loose = tmp_path / "loose.JPG"
    loose.write_bytes(b"jpeg")
    worked = ctx.work_path("views/front.webp")
    worked.write_bytes(b"webp")
    channel.requests.clear()

    seen: list[dict[str, Any]] = []

    async def answer(params: dict[str, Any]) -> Any:
        seen.append(params)
        assert ctx._agents[params["agent_id"]] is agent  # registered while pending
        return {
            "submitted": {"answer": "good"},
            "transcript": [{"role": "user", "content": "do it"}],
            "turns": 2,
            "cost_usd": 0,
        }

    channel.agent_run = answer
    agent = ctx.agent(system="sys", tools=[look, declared], recent_images=1, max_tokens=50)
    submitted = asyncio.run(
        agent.run(
            "do it",
            max_steps=5,
            images=[inputs_file, PNG_URL, loose, str(worked), ctx.out.bytes(b"o", "image/gif")],
            submit={"type": "object", "properties": {"answer": {"type": "string"}}},
            check=lambda value: None,
        )
    )
    assert submitted == {"answer": "good"}
    assert agent.transcript == [{"role": "user", "content": "do it"}]
    assert ctx._agents == {}
    puts = channel.puts()
    assert puts == [
        {
            "base64": base64.b64encode(b"png bytes").decode(),
            "kind": "image/png",
            "name": "agent/image",
        },
        {"base64": base64.b64encode(b"jpeg").decode(), "kind": "image/jpeg", "name": "loose.JPG"},
        {"work_path": "views/front.webp", "kind": "image/webp", "name": "front.webp"},
        {"base64": base64.b64encode(b"o").decode(), "kind": "image/gif", "name": "agent/image"},
    ]
    (params,) = [params for method, params in channel.requests if method == "agent.run"]
    assert params == {
        "agent_id": "agent-1",
        "system": "sys",
        "instructions": "do it",
        "images": [
            {"file": ref["digest"]},
            {"file": digest(b"png bytes")},
            {"file": digest(b"jpeg")},
            {"file": digest(b"webp")},
            {"file": digest(b"o")},
        ],
        "tools": [
            tool_schema(look),
            {
                "name": "place_prop",
                "description": "Places a prop.",
                "parameters": {"type": "object", "properties": {"x": {"type": "number"}}},
            },
        ],
        "max_steps": 5,
        "submit": {"type": "object", "properties": {"answer": {"type": "string"}}},
        "check": True,
        "recent_images": 1,
        "max_tokens": 50,
    }

    # A text answer, a second agent id, no submit: no check; optional members left out.
    async def text(params: dict[str, Any]) -> Any:
        return {"text": "done", "transcript": [], "turns": 1, "cost_usd": 0.5}

    channel.agent_run = text
    plain = ctx.agent(system="s")
    assert asyncio.run(plain.run("go", max_steps=1, check=lambda value: None)) == "done"
    last = channel.requests[-1][1]
    assert last["agent_id"] == "agent-2"
    assert last["check"] is False and last["submit"] is None and last["tools"] == []
    assert "images" not in last and "recent_images" not in last and "max_tokens" not in last
    for steps in (0, -1, True, 1.5):
        with pytest.raises(ValueError, match="max_steps is a whole number of at least 1"):
            asyncio.run(plain.run("go", max_steps=steps))  # type: ignore[arg-type]


def test_engine_errors_reach_the_body(ctx: Ctx) -> None:
    async def unfinished(params: dict[str, Any]) -> Any:
        raise engine_error(-32020, "the agent did not finish within 2 turns")

    channel_of(ctx).agent_run = unfinished
    agent = ctx.agent(system="s", tools=[look])
    with pytest.raises(AgentUnfinished, match="did not finish within 2 turns"):
        asyncio.run(agent.run("go", max_steps=2))
    assert ctx._agents == {}


# --- tool.invoke ------------------------------------------------------------------------------


def test_serve_tool(ctx: Ctx, tmp_path: Path) -> None:
    calls: list[Any] = []

    @tool
    def sync_tool(ctx: Ctx, what: str) -> dict[str, Any]:
        calls.append(("sync", what, ctx.state.get("mesh")))
        return {"saw": what, "f": 1.0}

    @tool
    async def async_tool(ctx: Ctx) -> ToolReply:
        return ToolReply("here", images=[PNG_URL])

    @tool
    def boom(ctx: Ctx, what: str) -> str:
        raise RuntimeError(f"bad {what}")

    @tool
    def refuses(ctx: Ctx) -> str:
        raise ToolInvocationError("no such prop")

    @tool
    def odd(ctx: Ctx) -> Any:
        return Path("x.txt")

    @tool
    def nothing(ctx: Ctx) -> None:
        return None

    @tool
    def bad_picture(ctx: Ctx) -> ToolReply:
        return ToolReply("x", images=[3])

    @tool
    def url_picture(ctx: Ctx) -> ToolReply:
        return ToolReply("x", images=["https://example.com/a.png"])

    def placed(arguments: Any) -> ToolResult:
        calls.append(("declared", dict(arguments)))
        jpeg = "data:IMAGE/JPEG;base64," + base64.b64encode(b"jp").decode()
        return ToolResult("placed", images=(jpeg,))

    async def text_only(arguments: Any) -> str:
        return "plain text"

    declared = Tool(name="place", description="Place.", parameters={}, handler=placed)
    texted = Tool(name="texted", description="Text.", parameters={}, handler=text_only)
    agent = ctx.agent(
        system="s",
        tools=[sync_tool, async_tool, boom, refuses, odd, nothing, bad_picture, url_picture]
        + [declared, texted],
    )
    ctx.state["mesh"] = "m.glb"

    def serve(name: str, arguments: dict[str, Any]) -> Any:
        return asyncio.run(agent.serve_tool(name, arguments))

    assert serve("sync_tool", {"what": "x"}) == {"content": {"saw": "x", "f": 1.0}}
    assert serve("async_tool", {}) == {
        "content": "here",
        "images": [{"file": digest(b"png bytes")}],
    }
    assert serve("boom", {"what": "look"}) == {"error": "RuntimeError: bad look"}
    assert serve("refuses", {}) == {"error": "ToolInvocationError: no such prop"}
    wrong = serve("sync_tool", {"wrong": 1})["error"]
    assert wrong.startswith("TypeError: ") and "unexpected keyword argument 'wrong'" in wrong
    assert serve("odd", {}) == {"content": "x.txt"}
    assert serve("nothing", {}) == {"content": None}
    assert serve("missing", {}) == {"content": "no tool named missing"}
    assert serve("place", {"x": 0.5}) == {"content": "placed", "images": [{"file": digest(b"jp")}]}
    assert channel_of(ctx).puts()[-1]["kind"] == "image/jpeg"
    assert serve("texted", {}) == {"content": "plain text"}
    assert calls == [("sync", "x", "m.glb"), ("declared", {"x": 0.5})]
    with pytest.raises(NodeFailure, match=r"^an agent picture is a file, not int$"):
        serve("bad_picture", {})
    with pytest.raises(NodeFailure, match=r"^an agent picture is a file, not a URL$"):
        serve("url_picture", {})


def test_data_urls_are_decoded(ctx: Ctx) -> None:
    agent = ctx.agent(system="s")
    for url, data, kind in (
        (PNG_URL, b"png bytes", "image/png"),
        ("data:IMAGE/WEBP;base64," + base64.b64encode(b"w").decode(), b"w", "image/webp"),
        ("data:;base64," + base64.b64encode(b"none").decode(), b"none", "image/png"),
        # Line breaks inside the payload are dropped: the bytes go out re-encoded.
        ("data:image/png;base64,cGlj\ndHVyZQ==", b"picture", "image/png"),
    ):
        value = asyncio.run(agent._picture(url))
        assert value == {"file": digest(data)}
        put = channel_of(ctx).puts()[-1]
        assert put == {
            "base64": base64.b64encode(data).decode(),
            "kind": kind,
            "name": "agent/image",
        }
    assert asyncio.run(agent._picture({"file": "a" * 64})) == {"file": "a" * 64}
    with pytest.raises(NodeFailure, match="not dict"):
        asyncio.run(agent._picture({"file": "a.png"}))


def test_pil_pictures(ctx: Ctx) -> None:
    from PIL import Image

    agent = ctx.agent(system="s")
    value = asyncio.run(agent._picture(Image.new("RGB", (2, 2))))
    assert set(value) == {"file"}
    assert channel_of(ctx).puts()[-1]["kind"] == "image/png"


def test_a_tool_failure_ends_the_loop_with_the_tools_own_error(ctx: Ctx) -> None:
    @tool
    def fails(ctx: Ctx) -> str:
        raise ctx.fail("the mesh is broken")

    async def engine(params: dict[str, Any]) -> Any:
        agent = ctx._agents[params["agent_id"]]
        try:
            await agent.serve_tool("fails", {})
        except NodeFailure as failure:
            # The engine answers agent.run with the tool's node_failure.
            raise engine_error(-32000, str(failure)) from None
        raise AssertionError("the tool did not fail")

    channel_of(ctx).agent_run = engine
    agent = ctx.agent(system="s", tools=[fails])
    with pytest.raises(NodeFailure, match="^the mesh is broken$") as raised:
        asyncio.run(agent.run("go", max_steps=3))
    assert not isinstance(raised.value, EngineError)
    assert "in fails" in "".join(traceback.format_tb(raised.value.__traceback__))


# --- agent.check ------------------------------------------------------------------------------


def test_serve_check(ctx: Ctx) -> None:
    checked: list[Any] = []

    def check(value: Any) -> None:
        checked.append(value)
        if value == "short":
            raise ValueError("not good enough")
        if value == "broken":
            raise ctx.fail("the answer breaks the node")
        if value == "boom":
            raise KeyError("boom")

    results: dict[str, Any] = {}

    async def engine(params: dict[str, Any]) -> Any:
        agent = ctx._agents[params["agent_id"]]
        assert agent.has_check
        for value in ("short", "broken", "fine"):
            results[value] = await agent.serve_check(value)
        try:
            await agent.serve_check("boom")
        except KeyError:
            raise engine_error(-32001, "'boom'") from None
        raise AssertionError("the check did not raise")

    channel_of(ctx).agent_run = engine
    agent = ctx.agent(system="s")
    with pytest.raises(KeyError, match="boom"):
        asyncio.run(agent.run("go", max_steps=3, submit={"type": "string"}, check=check))
    assert results == {
        "short": {"refusal": "not good enough"},
        "broken": {"refusal": "the answer breaks the node"},
        "fine": {"refusal": None},
    }
    assert checked == ["short", "broken", "fine", "boom"]
    assert not agent.has_check  # only while its run is pending


def test_an_async_check(ctx: Ctx) -> None:
    async def check(value: Any) -> None:
        await asyncio.sleep(0)
        raise ToolInvocationError("try again")

    async def engine(params: dict[str, Any]) -> Any:
        agent = ctx._agents[params["agent_id"]]
        assert await agent.serve_check({"a": 1}) == {"refusal": "try again"}
        return {"submitted": {"a": 2}, "transcript": [], "turns": 2, "cost_usd": 0}

    channel_of(ctx).agent_run = engine
    assert asyncio.run(
        ctx.agent(system="s").run("go", max_steps=2, submit={"type": "object"}, check=check)
    ) == {"a": 2}
    assert inspect.iscoroutinefunction(check)
