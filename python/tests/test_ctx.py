"""``Ctx`` over a fake channel: the requests it makes and what it does locally (spec/protocol.md
sections 3, 6 and 8)."""

from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import os
import shutil
import sys
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from grida.fx import (
    CallFailed,
    CallRefused,
    CallResult,
    CapabilityError,
    CeilingExceeded,
    Ctx,
    EngineError,
    InputFile,
    NodeFailure,
    Output,
)
from grida.fx._ctx import ToolHandle, kind_of
from grida.fx._errors import (
    AgentUnfinished,
    Cancelled,
    CapabilityUndeclared,
    ExpressionError,
    JobUnsettled,
    NoRoute,
    NotLive,
    OutsideWorkDir,
    OverBound,
    UndeclaredResource,
    UnknownFile,
    engine_error,
)
from grida.fx._protocol import check_value


class FakeChannel:
    """Records every request and notification; answers ``file.put`` by storing the file in a
    folder, ``fact`` and ``annotate`` with ``{}``, and anything else from ``answers``."""

    def __init__(self, store: Path, answers: dict[str, Any] | None = None) -> None:
        self.store = store
        self.answers = dict(answers or {})
        self.requests: list[tuple[str, dict[str, Any]]] = []
        self.notifications: list[tuple[str, dict[str, Any]]] = []
        self.run_id = "r1"

    def request(self, method: str, params: dict[str, Any]) -> Any:
        check_value(params)
        # As the engine reads it: tuples are arrays.
        self.requests.append((method, json.loads(json.dumps(params))))
        answer = self.answers.get(method)
        if answer is not None:
            return answer(params) if callable(answer) else answer
        if method == "file.put":
            return self.put(params)
        if method in ("fact", "annotate"):
            return {}
        raise AssertionError(f"unexpected request {method}")

    async def request_async(self, method: str, params: dict[str, Any]) -> Any:
        return self.request(method, params)

    def notify(self, method: str, params: dict[str, Any]) -> None:
        check_value(params)
        self.notifications.append((method, params))

    def put(self, params: dict[str, Any]) -> dict[str, Any]:
        if "base64" in params:
            data = base64.b64decode(params["base64"], validate=True)
        elif "json" in params:
            data = json.dumps(params["json"], sort_keys=True, indent=1).encode("utf-8")
        else:
            data = (self.work_dir / params["work_path"]).read_bytes()
        return file_ref(self.store, data, params.get("kind", "file"), params.get("name", "f"))

    @property
    def methods(self) -> list[str]:
        return [method for method, _ in self.requests]


def file_ref(
    store: Path, data: bytes, kind: str, name: str, key: str | None = None
) -> dict[str, Any]:
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    store.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    ref: dict[str, Any] = {
        "digest": digest,
        "kind": kind,
        "size": len(data),
        "name": name,
        "path": str(path),
        "facts": {"bytes": len(data), "kind": kind},
    }
    if key is not None:
        ref["key"] = key
    return ref


def run_params(tmp_path: Path, **changes: Any) -> dict[str, Any]:
    work = tmp_path / "work"
    work.mkdir(exist_ok=True)
    params: dict[str, Any] = {
        "run_id": "r1",
        "instance": {"id": "draw#1", "path": "draw", "step": "draw", "key": None, "take": [1, 3]},
        "type": "nodes/n.py#draw@1",
        "body": {"path": "nodes/n.py", "attribute": "draw"},
        "params": {},
        "param_files": {},
        "inputs": {},
        "work_dir": str(work),
        "resources": {},
        "tools": {},
        "calls": {},
        "timeout_s": None,
    }
    params.update(changes)
    return params


@pytest.fixture
def store(tmp_path: Path) -> Path:
    return tmp_path / "store"


@pytest.fixture
def make(tmp_path: Path, store: Path) -> Callable[..., tuple[Ctx, FakeChannel]]:
    def build(answers: dict[str, Any] | None = None, **changes: Any) -> tuple[Ctx, FakeChannel]:
        params = run_params(tmp_path, **changes)
        channel = FakeChannel(store, answers)
        channel.work_dir = Path(params["work_dir"])  # type: ignore[attr-defined]
        return Ctx(params, channel), channel  # type: ignore[arg-type]

    return build


# --- what the run was given -------------------------------------------------------------------


def test_the_instance(make: Callable[..., tuple[Ctx, FakeChannel]]) -> None:
    ctx, _ = make()
    assert ctx.instance.id == "draw#1"
    assert ctx.instance.path == "draw"
    assert ctx.instance.step == "draw"
    assert ctx.instance.key is None
    assert ctx.instance.takes == (1, 3)
    assert ctx.instance.take == 3


def test_params_get_their_files_back(
    make: Callable[..., tuple[Ctx, FakeChannel]], store: Path
) -> None:
    ref = file_ref(store, b"\x89PNG", "image/png", "ref.png")
    other = file_ref(store, b"zip", "file/zip", "a.zip")
    params = {
        "prompt": "a cat",
        "refs": [None, {"deep": None}],
        "a/b": None,
        "n": 3,
    }
    ctx, _ = make(
        params=params,
        param_files={"/refs/0": ref, "/refs/1/deep": other, "/a~1b": ref},
    )
    got = ctx.params
    assert got["prompt"] == "a cat" and got["n"] == 3
    assert isinstance(got["refs"][0], InputFile) and got["refs"][0].digest == ref["digest"]
    assert got["refs"][1]["deep"].kind == "file/zip"
    assert got["a/b"].name == "ref.png"
    # The run's own params are not changed, and the same dict comes back each time.
    assert params["refs"] == [None, {"deep": None}]
    assert ctx.params is got


def test_inputs_by_shape(make: Callable[..., tuple[Ctx, FakeChannel]], store: Path) -> None:
    one = file_ref(store, b"one", "text/plain", "one.txt")
    a = file_ref(store, b"a", "text/plain", "a.txt")
    b = file_ref(store, b"b", "text/plain", "b.txt")
    ctx, _ = make(
        inputs={
            "image": one,
            "refs": {"list": [a, b]},
            "lines": {"collection": [["zeta", {**b, "key": "zeta"}], ["alpha", a]]},
        }
    )
    image = ctx.inputs["image"]
    assert isinstance(image, InputFile)
    assert image.path == Path(one["path"])
    assert (image.kind, image.digest, image.name, image.size, image.key) == (
        "text/plain",
        one["digest"],
        "one.txt",
        3,
        None,
    )
    assert image.facts == {"bytes": 3, "kind": "text/plain"}
    assert image.read_bytes() == b"one"
    assert [item.name for item in ctx.inputs["refs"]] == ["a.txt", "b.txt"]
    lines = ctx.inputs["lines"]
    assert list(lines) == ["zeta", "alpha"]  # collection order
    assert lines["zeta"].key == "zeta"
    assert "missing" not in ctx.inputs


def test_copy_to_makes_a_writable_copy(
    make: Callable[..., tuple[Ctx, FakeChannel]], store: Path, tmp_path: Path
) -> None:
    ref = file_ref(store, b"mesh", "model/gltf-binary", "m.glb")
    os.chmod(ref["path"], 0o444)
    ctx, _ = make(inputs={"mesh": ref})
    target = ctx.inputs["mesh"].copy_to(ctx.work_path("deep/m.glb"))
    assert target == tmp_path / "work" / "deep" / "m.glb"
    assert target.read_bytes() == b"mesh"
    target.write_bytes(b"changed")  # the copy is the body's
    assert Path(ref["path"]).read_bytes() == b"mesh"


# --- ctx.read ---------------------------------------------------------------------------------


def test_read(make: Callable[..., tuple[Ctx, FakeChannel]], store: Path) -> None:
    text = file_ref(store, b"\xef\xbb\xbf\xef\xbb\xbfBOM text\r\nline2\r\n", "text/plain", "t")
    data = file_ref(store, b'{"b": [1, 2.5], "a": null}', "json", "d.json")
    marks = file_ref(store, b'{"kind": "fx-annotations-v1", "annotations": []}', "json", "m")
    a = file_ref(store, b"a", "text/plain", "a.txt")
    ctx, channel = make(inputs={"text": text, "data": data, "marks": marks, "refs": {"list": [a]}})
    # One byte-order mark is removed; line endings are kept.
    assert ctx.read.text("text") == "﻿BOM text\r\nline2\r\n"
    assert ctx.read.bytes("text").startswith(b"\xef\xbb\xbf\xef\xbb\xbf")
    assert ctx.read.json("data") == {"b": [1, 2.5], "a": None}
    assert ctx.read.annotations("marks") == {"kind": "fx-annotations-v1", "annotations": []}
    for name in ("refs", "absent"):
        with pytest.raises(NodeFailure, match=f"^input {name} is not one file$"):
            ctx.read.bytes(name)
    bad = file_ref(store, b"\xff\xfe", "text/plain", "bad.txt")
    ctx, _ = make(inputs={"bad": bad})
    with pytest.raises(UnicodeDecodeError):
        ctx.read.text("bad")
    assert channel.requests == []  # reads are local


def test_read_image_and_out_png(make: Callable[..., tuple[Ctx, FakeChannel]], store: Path) -> None:
    from PIL import Image

    picture = Image.new("RGBA", (3, 2), (255, 0, 0, 128))
    ctx, _ = make()
    output = ctx.out.png(picture)
    assert output.kind == "image/png" and output.data is not None
    ref = file_ref(store, output.data, "image/png", "p.png")
    ctx, _ = make(inputs={"image": ref})
    read = ctx.read.image("image")
    assert read.size == (3, 2) and read.getpixel((0, 0)) == (255, 0, 0, 128)


# --- ctx.out and work paths -------------------------------------------------------------------


def test_out_makes_outputs(make: Callable[..., tuple[Ctx, FakeChannel]], tmp_path: Path) -> None:
    ctx, channel = make()
    data = ctx.out.bytes(bytearray(b"\x00\x01"), "file/zip")
    assert isinstance(data, Output)
    assert (data.kind, data.data, data.work_path) == ("file/zip", b"\x00\x01", None)
    text = ctx.out.text("héllo")
    assert (text.kind, text.data) == ("text/plain", "héllo".encode())
    assert ctx.out.text("# h", "text/markdown").kind == "text/markdown"
    value = {"b": [1, 2.0], "a": None}
    made = ctx.out.json(value)
    value["b"].append(3)  # the output keeps the value it was given
    assert (made.kind, made.json, made.data, made.work_path) == (
        "json",
        {"b": [1, 2.0], "a": None},
        None,
        None,
    )
    assert ctx.out.json(None).json is None
    with pytest.raises(ValueError, match="ctx.out.json takes a JSON value"):
        ctx.out.json({"x": float("nan")})
    with pytest.raises(ValueError, match="not a JSON value"):
        ctx.out.json({"x": object()})
    path = ctx.out.path("sub/caption.txt")
    assert path == (tmp_path / "work" / "out" / "sub" / "caption.txt").resolve()
    assert path.parent.is_dir()
    assert channel.requests == []  # nothing leaves the host until it is returned or sent


def test_out_file(make: Callable[..., tuple[Ctx, FakeChannel]], tmp_path: Path) -> None:
    ctx, _ = make()
    inside = ctx.out.path("caption.TXT")
    inside.write_text("words\n", "utf-8")
    output = ctx.out.file(inside)
    assert (output.work_path, output.kind, output.data) == ("out/caption.TXT", "text/plain", None)
    assert ctx.out.file(str(inside), "text/markdown").kind == "text/markdown"
    nested = ctx.work_path("a/b/mesh.glb")
    nested.write_bytes(b"glb")
    assert ctx.out.file(nested).work_path == "a/b/mesh.glb"
    assert ctx.out.file(nested).kind == "model/gltf-binary"
    # A file elsewhere is read now.
    outside = tmp_path / "elsewhere.bin"
    outside.write_bytes(b"bytes")
    output = ctx.out.file(outside)
    assert (output.work_path, output.data, output.kind) == (None, b"bytes", "file")
    with pytest.raises(FileNotFoundError):
        ctx.out.file(ctx.work_path("never-written.txt"))


def test_work_path(make: Callable[..., tuple[Ctx, FakeChannel]], tmp_path: Path) -> None:
    ctx, _ = make()
    work = (tmp_path / "work").resolve()
    assert ctx.work_path("x/y.txt") == work / "x" / "y.txt"
    assert (work / "x").is_dir()
    assert ctx.work_path(".") == work
    for name in ("../x", str(tmp_path / "other.txt"), "a/../../b"):
        with pytest.raises(NodeFailure, match=r" is outside this node's work folder$"):
            ctx.work_path(name)


def test_kinds_follow_the_suffix_table() -> None:
    assert kind_of("a.PNG") == "image/png"
    assert kind_of("a.jpeg") == "image/jpeg"
    assert kind_of("dir.d/a") == "file"
    assert kind_of("a.tar.gz") == "file"
    assert kind_of(Path("x.mkv")) == "video/x-matroska"
    assert kind_of("x.yml") == "text/yaml"


# --- reporting --------------------------------------------------------------------------------


def test_fact(make: Callable[..., tuple[Ctx, FakeChannel]]) -> None:
    ctx, channel = make()
    ctx.fact("score", 0.5)
    ctx.fact("score", 0.75)
    ctx.fact("sizes", (1, 2))
    assert channel.requests == [
        ("fact", {"name": "score", "value": 0.5}),
        ("fact", {"name": "score", "value": 0.75}),
        ("fact", {"name": "sizes", "value": [1, 2]}),
    ]
    assert ctx.facts == {"score": 0.75, "sizes": (1, 2)}
    with pytest.raises(ValueError, match=r"^fact bad: at /x: nan is not a JSON number$"):
        ctx.fact("bad", {"x": float("nan")})
    with pytest.raises(TypeError):
        ctx.fact(3, 1)  # type: ignore[arg-type]
    assert len(channel.requests) == 3 and "bad" not in ctx.facts


def test_a_refused_fact_is_not_mirrored(make: Callable[..., tuple[Ctx, FakeChannel]]) -> None:
    def refuse(params: dict[str, Any]) -> Any:
        raise engine_error(-32602, "cost_usd is the engine's")

    ctx, _ = make({"fact": refuse})
    with pytest.raises(EngineError) as raised:
        ctx.fact("cost_usd", 1)
    assert raised.value.code == -32602
    assert ctx.facts == {}


def test_annotate(make: Callable[..., tuple[Ctx, FakeChannel]]) -> None:
    ctx, channel = make()
    ctx.annotate(shape="box", box=[0.1, 0.2, 0.3, 0.4], label="seam", color="red", extra=1)
    ctx.annotate(label="note only")
    ctx.annotate(shape="points", points=[[0, 0], [1, 1]], closed=True, tag="t")
    assert channel.requests == [
        (
            "annotate",
            {
                "mark": {
                    "shape": "box",
                    "label": "seam",
                    "color": "red",
                    "box": [0.1, 0.2, 0.3, 0.4],
                    "extra": 1,
                }
            },
        ),
        ("annotate", {"mark": {"label": "note only"}}),
        (
            "annotate",
            {"mark": {"shape": "points", "tag": "t", "points": [[0, 0], [1, 1]], "closed": True}},
        ),
    ]
    assert [mark.get("shape") for mark in ctx.marks] == ["box", None, "points"]
    with pytest.raises(NodeFailure, match=r"^a mark's shape is one of point, points, box$"):
        ctx.annotate(shape="circle")
    for shape, geometry in (("box", "box"), ("point", "at"), ("points", "points")):
        with pytest.raises(NodeFailure, match=f"^a {shape} mark needs {geometry}=$"):
            ctx.annotate(shape=shape)
    with pytest.raises(ValueError, match="^a mark: "):
        ctx.annotate(label="x", at=object())
    assert len(channel.requests) == 3 and len(ctx.marks) == 3


def test_progress_is_a_notification(make: Callable[..., tuple[Ctx, FakeChannel]]) -> None:
    ctx, channel = make()
    ctx.progress("half way", 0.5)
    ctx.progress("going")
    assert channel.notifications == [
        ("progress", {"text": "half way", "fraction": 0.5}),
        ("progress", {"text": "going"}),
    ]
    assert channel.requests == []


def test_fail_and_state(make: Callable[..., tuple[Ctx, FakeChannel]]) -> None:
    ctx, _ = make()
    failure = ctx.fail("on purpose")
    assert isinstance(failure, NodeFailure) and str(failure) == "on purpose"
    ctx.state["mesh"] = 1
    assert ctx.state == {"mesh": 1}
    assert ctx.cancelled is False


# --- prompts and capabilities -----------------------------------------------------------------


def test_prompt(make: Callable[..., tuple[Ctx, FakeChannel]], store: Path) -> None:
    image = file_ref(store, b"img", "image/png", "i.png")
    ctx, channel = make(
        {"prompt.render": lambda params: {"text": f"rendered {params['path']}"}},
        resources={"prompts/seam.md": "/abs/prompts/seam.md"},
        inputs={"image": image},
    )
    with pytest.raises(
        NodeFailure, match=r"^prompts/other.md is not one of this node's declared resources$"
    ):
        ctx.prompt("prompts/other.md")
    made = ctx.out.text("made")
    text = ctx.prompt("prompts/seam.md", who="W", n=3, pic=ctx.inputs["image"], made=[made, made])
    assert text == "rendered prompts/seam.md"
    put = channel.requests[0]
    assert put[0] == "file.put" and put[1]["base64"] == base64.b64encode(b"made").decode()
    assert put[1]["name"] == "prompt/file"
    made_digest = hashlib.sha256(b"made").hexdigest()
    assert channel.requests[1:] == [
        (
            "prompt.render",
            {
                "path": "prompts/seam.md",
                "variables": {
                    "who": "W",
                    "n": 3,
                    "pic": {"file": image["digest"]},
                    "made": [{"file": made_digest}, {"file": made_digest}],
                },
            },
        )
    ]


def test_capability_requests_carry_files(
    make: Callable[..., tuple[Ctx, FakeChannel]], store: Path, tmp_path: Path
) -> None:
    image = file_ref(store, b"img", "image/png", "i.png")
    a = file_ref(store, b"a", "image/png", "a.png")
    picture = file_ref(store, b"answer", "image/png", "out.png")
    answer = {
        "key": "c" * 64,
        "cached": False,
        "cost_usd": 0.04,
        "files": {"image": picture},
        "data": {"revised": "x"},
    }
    ctx, channel = make({"capability": answer}, inputs={"image": image, "refs": {"list": [a]}})
    mask = ctx.out.bytes(b"mask", "image/png")
    written = ctx.out.path("guide.png")
    written.write_bytes(b"guide")
    guide = ctx.out.file(written)
    data = ctx.out.json({"k": 1})
    result = asyncio.run(
        ctx.image_edit(
            image=ctx.inputs["image"],
            mask=mask,
            references=ctx.inputs["refs"],
            nested={"a": (mask, guide), "data": data, "keep": {"file": "a.png"}},
            prompt="paint",
        )
    )
    methods = channel.methods
    assert methods == ["file.put", "file.put", "file.put", "capability"]  # each output once
    puts = [params for method, params in channel.requests if method == "file.put"]
    assert puts[0] == {
        "base64": base64.b64encode(b"mask").decode(),
        "kind": "image/png",
        "name": "request/file",
    }
    assert puts[1] == {"work_path": "out/guide.png", "kind": "image/png", "name": "request/file"}
    assert puts[2] == {"json": {"k": 1}, "kind": "json", "name": "request/file"}
    capability = channel.requests[-1][1]
    mask_digest = hashlib.sha256(b"mask").hexdigest()
    assert capability == {
        "capability": "image.edit",
        "request": {
            "image": {"file": image["digest"]},
            "mask": {"file": mask_digest},
            "references": [{"file": a["digest"]}],
            "nested": {
                "a": [{"file": mask_digest}, {"file": hashlib.sha256(b"guide").hexdigest()}],
                "data": {"file": hashlib.sha256(b'{\n "k": 1\n}').hexdigest()},
                "keep": {"file": "a.png"},
            },
            "prompt": "paint",
        },
    }
    assert isinstance(result, CallResult)
    assert result.image.digest == picture["digest"] and result.image.read_bytes() == b"answer"
    assert result.files["image"].name == "out.png"
    assert (result.cost_usd, result.cached, result.key) == (0.04, False, "c" * 64)
    assert result.json == result.data == {"revised": "x"}
    structured = CallResult(
        {
            "key": "d" * 64,
            "cached": True,
            "cost_usd": 0,
            "files": {},
            "data": {"json": {"caption": "a kite"}},
        }
    )
    assert structured.json == {"caption": "a kite"}
    with pytest.raises(CallFailed, match=r"^the call returned no audio$"):
        _ = result.audio
    with pytest.raises(CallFailed, match=r"^the call returned no video$"):
        _ = result.video
    # A second call stores the output again only when it is a work file (it may have changed).
    asyncio.run(ctx.capability("image.generate", mask=mask, guide=guide))
    assert channel.methods[4:] == ["file.put", "capability"]
    assert channel.requests[4][1]["work_path"] == "out/guide.png"


def test_capability_sugar_and_errors(make: Callable[..., tuple[Ctx, FakeChannel]]) -> None:
    seen: list[str] = []

    def answer(params: dict[str, Any]) -> Any:
        seen.append(params["capability"])
        if params["capability"] == "structured.generate":
            raise engine_error(-32014, "run ceiling reached", {"needed_usd": 1, "remaining_usd": 0})
        return {"key": "d" * 64, "cached": True, "cost_usd": 0, "files": {}, "data": None}

    ctx, _ = make({"capability": answer})
    asyncio.run(ctx.image_generate(prompt="p"))
    with pytest.raises(CeilingExceeded) as raised:
        asyncio.run(ctx.structured_generate(prompt="p"))
    assert raised.value.code == -32014
    assert raised.value.data == {"needed_usd": 1, "remaining_usd": 0}
    assert str(raised.value) == "run ceiling reached"
    assert seen == ["image.generate", "structured.generate"]
    with pytest.raises(ValueError, match="at /request/p: a PosixPath is not a JSON value"):
        ctx2, channel = make({"capability": answer})
        asyncio.run(ctx2.capability("image.generate", p=Path("x")))


def test_engine_errors_by_code() -> None:
    expected = {
        -32002: Cancelled,
        -32010: CapabilityUndeclared,
        -32011: OverBound,
        -32012: NoRoute,
        -32013: NotLive,
        -32014: CeilingExceeded,
        -32015: CallRefused,
        -32016: CallFailed,
        -32017: JobUnsettled,
        -32020: AgentUnfinished,
        -32021: UndeclaredResource,
        -32022: ExpressionError,
        -32023: OutsideWorkDir,
        -32024: UnknownFile,
    }
    for code, kind in expected.items():
        error = engine_error(code, f"message {code}", {"key": "k"})
        assert type(error) is kind
        assert (error.code, error.message, error.data, str(error)) == (
            code,
            f"message {code}",
            {"key": "k"},
            f"message {code}",
        )
        assert isinstance(error, EngineError)
    assert isinstance(engine_error(-32014, "x"), CapabilityError)
    other = engine_error(-32602, "bad params")
    assert type(other) is EngineError and other.code == -32602 and other.data is None
    assert engine_error(-32000, "failed").code == -32000


# --- external programs ------------------------------------------------------------------------


@pytest.fixture
def sh() -> str:
    found = shutil.which("sh")
    if found is None:
        pytest.skip("no sh")
    return found


def test_tool_handles(make: Callable[..., tuple[Ctx, FakeChannel]], sh: str) -> None:
    ctx, _ = make(tools={"sh": sh, "blender": None})
    with pytest.raises(NodeFailure, match=r"^ffmpeg is not one of this node's declared tools$"):
        ctx.tool("ffmpeg")
    with pytest.raises(NodeFailure, match=r"^blender is not installed \(see grida-fx doctor\)$"):
        ctx.tool("blender")
    handle = ctx.tool("sh")
    assert isinstance(handle, ToolHandle)
    assert handle.executable == Path(sh)  # an attribute, read directly by bodies
    with pytest.raises(NodeFailure, match="is not installed"):
        ToolHandle("x", None).run([])


def test_tool_run(make: Callable[..., tuple[Ctx, FakeChannel]], sh: str, tmp_path: Path) -> None:
    ctx, _ = make(tools={"sh": sh})
    shell = ctx.tool("sh")
    done = shell.run(["-c", "pwd; echo err >&2"])
    assert done.returncode == 0
    assert Path(done.stdout.strip()).resolve() == (tmp_path / "work").resolve()
    assert done.stderr == "err\n"
    assert done.args[0] == sh
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    assert Path(shell.run(["-c", "pwd"], cwd=elsewhere).stdout.strip()).resolve() == (
        elsewhere.resolve()
    )
    with pytest.raises(NodeFailure, match=r"^sh exited 3: bad$"):
        shell.run(["-c", "echo out; echo '  bad  ' >&2; exit 3"])
    with pytest.raises(NodeFailure, match=r"^sh exited 4: only stdout$"):
        shell.run(["-c", "echo only stdout; exit 4"])
    kept = shell.run(["-c", "exit 5"], check=False)
    assert kept.returncode == 5
    # env replaces the environment.
    replaced = shell.run(["-c", 'echo "[$HOME][$ONLY]"'], env={"ONLY": "this"})
    assert replaced.stdout == "[][this]\n"
    # stdin is empty, never the protocol stream.
    assert shell.run(["-c", "cat"]).stdout == ""


def test_tool_run_tail(make: Callable[..., tuple[Ctx, FakeChannel]]) -> None:
    ctx, _ = make(tools={"python": sys.executable})
    with pytest.raises(NodeFailure) as raised:
        ctx.tool("python").run(
            ["-c", "import sys; sys.stderr.write('a' * 2500 + 'END'); sys.exit(2)"]
        )
    message = str(raised.value)
    assert message.startswith("python exited 2: ")
    tail = message.removeprefix("python exited 2: ")
    assert len(tail) == 2000 and tail.endswith("END")


def test_tool_timeout_ends_the_program_and_what_it_started(
    make: Callable[..., tuple[Ctx, FakeChannel]], sh: str, tmp_path: Path
) -> None:
    ctx, _ = make(tools={"sh": sh})
    pid_file = tmp_path / "child.pid"
    started = time.monotonic()
    with pytest.raises(NodeFailure, match=r"^sh ran past 1.5 seconds$"):
        ctx.tool("sh").run(
            ["-c", f"sleep 30 & echo $! > {pid_file}; wait"],
            timeout_s=1.5,
        )
    assert time.monotonic() - started < 10
    child = int(pid_file.read_text().strip())
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            os.kill(child, 0)
        except ProcessLookupError:
            break
        time.sleep(0.05)
    else:
        pytest.fail("the program's child outlived the timeout")
