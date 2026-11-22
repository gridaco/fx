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


def check_cli(command: Path, project: Path, baseline: Path) -> None:
    def call(*args: str, status: int = 0, binary: Path = command) -> str:
        proc = subprocess.run(
            [binary, *args], cwd=project, capture_output=True, text=True, timeout=20, check=False
        )
        assert proc.returncode == status, (args, proc.stdout, proc.stderr)
        return proc.stdout

    def requests(selector: str = SELECTOR, *, binary: Path = command) -> dict[str, dict]:
        result = json.loads(call("inspect", selector, "--requests", "--json", binary=binary))
        assert result.keys() == {"requests"}
        return by_step(result["requests"])

    def registered_route(selector: str, *, binary: Path = command) -> str:
        receipt = json.loads(call("inspect", selector, "--register", "--json", binary=binary))
        return receipt["route"]

    def invocation_id(*, binary: Path = command) -> str:
        receipt = json.loads(call("inspect", SELECTOR, "--control", "--json", binary=binary))
        return receipt["invocation_id"]

    def submit(request: dict, key: str, status: int = 0, *, selector: str = SELECTOR) -> dict:
        return json.loads(
            call(
                "respond",
                selector,
                request["id"],
                "--json",
                json.dumps({"variant": key}),
                status=status,
            )
        )

    call("plan", "showcase.yaml", "--check", "--max-usd", "0")
    (project / "short.yaml").write_text("seconds: 1\n")
    call("run", "workflow.yaml", "--name", "review", "--no-view", status=1, binary=baseline)
    original_requests = requests(binary=baseline)
    opened = requests()
    assert opened.keys() == original_requests.keys()
    for step, request in original_requests.items():
        # The candidate may return additive request metadata.
        assert {key: opened[step][key] for key in request} == request
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

    # Resume changes the runner invocation while preserving the requests of the saved run.
    invocation = invocation_id(binary=baseline)
    assert invocation and pending["deadline"]
    assert invocation_id() == invocation
    route = registered_route(SELECTOR, binary=baseline)
    assert requests(str(folder)) == opened
    call("resume", SELECTOR, "--no-view", status=1)
    resumed = invocation_id()
    assert resumed and resumed != invocation
    assert requests(SELECTOR) == opened
    assert registered_route(str(folder)) == route
    stale = json.loads(call("cancel", SELECTOR, "--invocation", invocation, "--json", status=2))
    assert stale["code"] == "invocation_mismatch"

    # A second run of the identical plan has its own requests, even when it can reuse results.
    other = "review-variants/independent"
    call("run", "workflow.yaml", "--name", "independent", "--no-view", status=1)
    other_run = json.loads(call("inspect", other, "--json"))["run"]
    other_graph = json.loads((project / other_run["folder"] / "plan.json").read_text())
    assert other_graph["plan"] == graph["plan"]
    assert registered_route(other) != route
    independent = requests(other)
    assert independent.keys() == opened.keys()
    for step, request in independent.items():
        assert request["id"] != opened[step]["id"]
        assert request["state"]["status"] == "pending"
    assert submit(pending, "b", 1, selector=other)["code"] == "not_found"
    assert requests(other) == independent
    assert requests() == opened

    assert submit(pending, "missing", 1)["code"] == "invalid_answer"
    assert requests() == opened
    revision_winner = fx.respond(SELECTOR, revision["id"], "rev-42", cwd=project)
    assert revision_winner["state"]["channel"] == "sdk"
    # Lose the first client's acknowledgment. A fresh client can recover exactly what won.
    subprocess.run(
        [command, "respond", str(folder), pending["id"], "--json", '{"variant":"b"}'],
        cwd=project,
        stdout=subprocess.DEVNULL,
        check=True,
        timeout=10,
    )
    winner = fx.respond(SELECTOR, pending["id"], {"variant": "b"}, cwd=project)
    assert winner["state"]["channel"] == "cli"
    assert submit(pending, "b")["request"] == winner
    assert submit(pending, "a", 1)["request"] == winner
    assert requests(other) == independent
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


def check_pause_with_requests(command: Path, project: Path) -> None:
    folder = project / "paused-review"
    runner = subprocess.Popen(
        [command, "run", "workflow.yaml", "--run", str(folder), "--wait-for-answers", "--no-view"],
        cwd=project,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        deadline = time.monotonic() + 15
        while True:
            if (folder / "awaiting").is_dir():
                opened = by_step(fx.list_requests(folder, cwd=project))
                if len(opened) == 2:
                    break
            assert runner.poll() is None and time.monotonic() < deadline, "reviews failed to open"
            time.sleep(0.03)
        paused = fx.pause(folder, wait=True, cwd=project)
        assert paused.recorded_state == "paused" and paused.cleanup == "complete"
        assert runner.wait(timeout=10) == 1
        assert by_step(fx.list_requests(folder, cwd=project)) == opened
        fx.respond(folder, opened["revision#1"]["id"], "rev-paused", cwd=project)
        chosen = opened["choose#1"]
        fx.respond(folder, chosen["id"], {"variant": "a"}, cwd=project)
        assert not (folder / "outputs/manifest.json").exists()
        result = fx.resume(folder, cwd=project)
        assert result.ok
        check_delivery(folder, "rev-paused", chosen["choices"]["a"]["file"]["digest"])
    finally:
        if runner.poll() is None:
            runner.terminate()
            try:
                runner.wait(timeout=5)
            except subprocess.TimeoutExpired:
                runner.kill()
                runner.wait(timeout=5)


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
    parser.add_argument(
        "--baseline-command",
        type=Path,
        help="Use a released engine to create the original run before testing resume",
    )
    args = parser.parse_args()
    command = args.command.resolve(strict=True)
    baseline = (args.baseline_command or command).resolve(strict=True)
    env = environment()
    os.environ.clear()
    os.environ.update(env, GRIDA_FX_BIN=str(command))
    with tempfile.TemporaryDirectory(prefix="fx-waiting-") as tmp:
        project = Path(tmp) / "project"
        shutil.copytree(ROOT / "fixtures/waiting", project)
        check_cli(command, project, baseline)
        check_pause_with_requests(command, project)
        asyncio.run(check_sdk(project))
    print("Passed CLI and Python SDK waiting-step journeys.")


if __name__ == "__main__":
    main()
