#!/usr/bin/env python3
"""Exercise promptless SDK input, image review, and expiry in the same provider-free workflow."""

import argparse
import asyncio
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

from check_control import environment, validator
from grida import fx

ROOT = Path(__file__).resolve().parents[1]
SELECTOR = "review-variants/review"


def by_step(requests: list[dict]) -> dict[str, dict]:
    return {request["step"]: request for request in requests}


def check_delivery(folder: Path, revision: str, image_digest: str) -> None:
    assert hashlib.sha256((folder / "outputs/image.png").read_bytes()).hexdigest() == image_digest
    assert json.loads((folder / "outputs/manifest.json").read_text()) == {
        "revision": revision,
        "image_digest": image_digest,
    }


def check_cli(command: Path, project: Path) -> None:
    def call(*args: str, status: int = 0) -> str:
        proc = subprocess.run(
            [command, *args], cwd=project, capture_output=True, text=True, timeout=20, check=False
        )
        assert proc.returncode == status, (args, proc.stdout, proc.stderr)
        return proc.stdout

    def requests() -> dict[str, dict]:
        result = json.loads(call("inspect", SELECTOR, "--requests", "--json"))
        assert result.keys() == {"requests"}
        return by_step(result["requests"])

    def submit(request: dict, key: str, status: int = 0) -> dict:
        return json.loads(
            call(
                "respond",
                SELECTOR,
                request["id"],
                "--json",
                json.dumps({"variant": key}),
                status=status,
            )
        )

    call("plan", "showcase.yaml", "--check", "--max-usd", "0")
    (project / "short.yaml").write_text("seconds: 1\n")
    call("run", "workflow.yaml", "--name", "review", "--no-view", status=1)
    opened = requests()
    pending = opened["choose#1"]
    revision = opened["revision#1"]
    assert "message" not in revision and revision["schema"] == {"type": "string"}
    assert pending["state"]["status"] == "pending"
    run = json.loads(call("inspect", SELECTOR, "--json"))["run"]
    assert run["state"] == "awaiting" and len(pending["choices"]) == 3
    folder = project / run["folder"]
    graph = json.loads((folder / "plan.json").read_text())
    validator(graph["kind"]).validate(graph)
    for request in opened.values():
        stored = folder / "awaiting" / (request["id"] + ".json")
        validator("fx-await-request-v1").validate(json.loads(stored.read_text()))
    assert submit(pending, "missing", 1)["code"] == "invalid_answer"
    assert requests() == opened
    revision_winner = fx.respond(SELECTOR, revision["id"], "rev-42", cwd=project)
    assert revision_winner["state"]["channel"] == "sdk"
    # Lose the first client's acknowledgment. A fresh client can recover exactly what won.
    subprocess.run(
        [command, "respond", SELECTOR, pending["id"], "--json", '{"variant":"b"}'],
        cwd=project,
        stdout=subprocess.DEVNULL,
        check=True,
        timeout=10,
    )
    winner = fx.respond(SELECTOR, pending["id"], {"variant": "b"}, cwd=project)
    assert winner["state"]["channel"] == "cli"
    assert submit(pending, "b")["request"] == winner
    assert submit(pending, "a", 1)["request"] == winner
    call("run", "workflow.yaml", "--resume", "review", "--no-view")
    check_delivery(folder, "rev-42", pending["choices"]["b"]["file"]["digest"])
    assert requests() == {"choose#1": winner, "revision#1": revision_winner}
    event_schema = validator("fx-run-events-v1")
    for line in (folder / "events.jsonl").read_text().splitlines():
        event_schema.validate(json.loads(line))

    call(
        "run",
        "workflow.yaml",
        "--name",
        "expired",
        "--inputs",
        "short.yaml",
        "--no-view",
        status=1,
    )
    expired_selector = "review-variants/expired"
    opened = by_step(fx.list_requests(expired_selector, cwd=project))
    short = opened["choose#1"]
    fx.respond(expired_selector, opened["revision#1"]["id"], "rev-43", cwd=project)
    assert short["id"] != pending["id"]
    limit = time.monotonic() + 10
    while (
        by_step(fx.list_requests(expired_selector, cwd=project))["choose#1"]["state"]["status"]
        == "pending"
    ):
        assert time.monotonic() < limit, "the detached request did not expire"
        time.sleep(0.05)
    try:
        fx.respond(expired_selector, short["id"], {"variant": "b"}, cwd=project)
    except fx.AnswerError as error:
        assert error.code == "expired" and error.request["state"]["at"] == short["deadline"]
    else:
        raise AssertionError("late approval was accepted")
    expired = json.loads(call("inspect", expired_selector, "--json"))["run"]
    expired_folder = project / expired["folder"]
    assert not (expired_folder / "outputs/image.png").exists(), (
        "a detached response started the fallback"
    )
    call("run", "workflow.yaml", "--resume", "expired", "--inputs", "short.yaml", "--no-view")
    check_delivery(expired_folder, "rev-43", short["choices"]["c"]["file"]["digest"])

    # A frozen choice may carry a file inside an ordinary object. Verification must
    # check the selected step's placement independently of the producer's intact file.
    (project / "bundle.yaml").write_text("""fx: workflow/v1
id: bundle
title: Review an image bundle
steps:
  draw: {uses: "./nodes.py#draw"}
  choose:
    uses: fx/await@1
    with:
      schema: {type: string}
      choices:
        cover: {image: "${{ steps.draw.outputs.a }}"}
""")
    target = "bundle/proof"
    call("run", "bundle.yaml", "--name", "proof", "--no-view", status=1)
    (request,) = fx.list_requests(target, cwd=project)
    fx.respond(target, request["id"], "cover", cwd=project)
    call("run", "bundle.yaml", "--resume", "proof", "--no-view")
    inspected = json.loads(call("inspect", target, "--verify", "--json"))["run"]
    selected = next(step for step in inspected["steps"] if step["id"] == "choose#1")
    (placed,) = selected["files"]
    assert placed["digest"] == request["choices"]["cover"]["object"]["image"]["file"]["digest"]
    (project / inspected["folder"] / placed["path"]).rename(project / "saved-cover.png")
    verified = json.loads(call("inspect", target, "--verify", "--json", status=1))["verification"]
    assert verified == {"verified": False, "problems": [f"choose#1: {placed['path']} is missing"]}


async def check_sdk(project: Path) -> None:
    folder = project / "sdk-run"
    async with asyncio.timeout(30):
        async with asyncio.TaskGroup() as group:
            task = group.create_task(
                fx.run_async("workflow.yaml", cwd=project, run_dir=folder, wait_for_answers=True)
            )
            while not (folder / "plan.json").is_file():
                await asyncio.sleep(0.03)
            while len(items := await fx.list_requests_async(folder, cwd=project)) < 2:
                await asyncio.sleep(0.03)
            opened = by_step(items)
            request = opened["choose#1"]
            revision = opened["revision#1"]
            assert "message" not in revision and "choices" not in revision
            info = await fx.inspect_requests_async(folder, cwd=project, register=True)
            assert info == {"requests": items, "viewer_url": None}
            try:
                await fx.respond_async(folder, revision["id"], False, cwd=project)
            except fx.AnswerError as error:
                assert error.code == "invalid_answer"
            else:
                raise AssertionError("the promptless string request accepted a boolean")
            await fx.respond_async(folder, revision["id"], "rev-44", cwd=project)
            winner = await fx.respond_async(folder, request["id"], {"variant": "a"}, cwd=project)
            assert winner["state"]["channel"] == "sdk"
        assert task.result().ok
        check_delivery(folder, "rev-44", request["choices"]["a"]["file"]["digest"])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--command", type=Path, required=True)
    command = parser.parse_args().command.resolve(strict=True)
    env = environment()
    os.environ.clear()
    os.environ.update(env, GRIDA_FX_BIN=str(command))
    with tempfile.TemporaryDirectory(prefix="fx-waiting-") as tmp:
        project = Path(tmp) / "project"
        shutil.copytree(ROOT / "fixtures/waiting", project)
        check_cli(command, project)
        asyncio.run(check_sdk(project))
    print("Passed CLI and Python SDK waiting-step journeys.")


if __name__ == "__main__":
    main()
