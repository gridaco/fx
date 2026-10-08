"""Prove run observation through public CLI and HTTP contracts, entirely offline.

    uv run --project python python tools/check_observation.py --command target/debug/grida-fx
    uv run --project python python tools/check_observation.py --output .fx/observation/demo

This consumer imports no engine or viewer implementation. Fresh copies of the local
fixture prevent cached waits. A fixture-local release gate makes attachment checks
deterministic; all waits and subprocess cleanup are bounded. Generated evidence is
temporary unless --output names a new or empty directory.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from contextlib import ExitStack
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "fixtures" / "viewer"
KEYS = ("OPENAI_API_KEY", "OPENROUTER_API_KEY", "FAL_KEY", "TRIPO_API_KEY", "ELEVENLABS_API_KEY")
ADDRESS = re.compile(r"^(?:view\s+)?(http://127\.0\.0\.1:\d+/)\s*$", re.MULTILINE)
CLIENT = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def expect(condition: object, message: str) -> None:
    if not condition:
        raise ValueError(message)


def environment() -> dict[str, str]:
    env = os.environ.copy()
    for key in KEYS:
        env.pop(key, None)
    env["GRIDA_FX_DISABLE_DOTENV"] = "1"
    env["GRIDA_FX_NETWORK"] = "off"
    env["GRIDA_FX_PYTHON"] = sys.executable
    return env


def prepare_output(path: Path) -> Path:
    destination = path.absolute()
    for parent in [destination, *destination.parents]:
        expect(not parent.is_symlink(), "the output path contains a symbolic link")
    destination = destination.resolve(strict=False)
    source = SOURCE.resolve()
    expect(
        not destination.is_relative_to(source) and not source.is_relative_to(destination),
        "the output directory must not overlap the fixture source",
    )
    expect(not destination.exists() or destination.is_dir(), "the output must be a directory")
    expect(
        not destination.exists() or not any(destination.iterdir()),
        "the output directory must be new or empty",
    )
    destination.mkdir(parents=True, exist_ok=True)
    return destination


def write_json(path: Path, document: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def http_json(base: str, route: str) -> dict[str, Any]:
    with CLIENT.open(base + route, timeout=5) as response:
        return json.load(response)


class Child:
    """Keep public-command output as evidence and always reap the foreground command."""

    def __init__(self, command: list[str], home: Path, logs: Path) -> None:
        logs.parent.mkdir(parents=True, exist_ok=True)
        self.logs = logs
        with logs.open("w", encoding="utf-8") as stream:
            self.process = subprocess.Popen(
                command,
                cwd=home,
                env=environment(),
                stdout=stream,
                stderr=subprocess.STDOUT,
            )

    def output(self) -> str:
        return self.logs.read_text(encoding="utf-8")

    def address(self) -> str:
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            text = self.output()
            match = ADDRESS.search(text)
            if match:
                address = match.group(1)
                # The advertised address must already answer; no startup retry here.
                http_json(address, "api/snapshot")
                return address
            expect(self.process.poll() is None, f"command exited before its viewer URL:\n{text}")
            time.sleep(0.025)
        raise ValueError("command did not advertise a viewer within twenty seconds")

    def finish(self, expected: int = 0) -> None:
        status = self.process.wait(timeout=20)
        expect(
            status == expected, f"command exited {status}, expected {expected}:\n{self.output()}"
        )

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)


def copied_project(output: Path, name: str) -> Path:
    home = output / name / "project"
    shutil.copytree(
        SOURCE,
        home,
        ignore=shutil.ignore_patterns(".env*", ".fx", "runs", "__pycache__", "*.pyc"),
    )
    return home


def cli_json(command: Path, run: Path, *args: str) -> dict[str, Any]:
    result = subprocess.run(
        [str(command), "observe", str(run), *args],
        env=environment(),
        capture_output=True,
        text=True,
        timeout=15,
        check=True,
    )
    return json.loads(result.stdout)


def batch_route(cursor: str, limit: int = 2) -> str:
    return "api/events?" + urllib.parse.urlencode({"after": cursor, "limit": limit})


def catch_up(read, cursor: str, limit: int = 2) -> tuple[list[dict[str, Any]], str]:
    caught: list[dict[str, Any]] = []
    for _ in range(1000):
        batch = read(cursor, limit)
        expect(batch["kind"] == "fx-run-event-batch-v1", "wrong event-batch contract")
        expect(len(batch["events"]) <= limit, "event batch exceeded the requested bound")
        expect(not batch["events"] or batch["cursor"] != cursor, "events did not advance cursor")
        caught.extend(batch["events"])
        cursor = batch["cursor"]
        if not batch["has_more"]:
            return caught, cursor
    raise ValueError("bounded catch-up did not reach the retained log frontier")


def wait_running(base: str, child: Child) -> dict[str, Any]:
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        snapshot = http_json(base, "api/snapshot")
        view = snapshot["view"]
        states = {node["path"]: node["state"] for node in view["nodes"]}
        if states.get("wait") == "running":
            expect(states.get("seed") == "succeeded", "seed did not finish before the wait")
            expect(len(states) == 7, "the delayed demo must expose all seven nodes")
            expect(
                all(
                    state == "pending"
                    for path, state in states.items()
                    if path not in {"seed", "wait"}
                ),
                "a later demo stage ran before its first wait was released",
            )
            expect(view["charged_usd"] == 0, "running fixture recorded a charge")
            expect(child.process.poll() is None, "running snapshot has no running command")
            return snapshot
        expect(child.process.poll() is None, f"fixture ended before attachment:\n{child.output()}")
        time.sleep(0.025)
    raise ValueError("fixture did not enter its gated wait")


def recorded_events(run: Path) -> list[dict[str, Any]]:
    return [json.loads(line) for line in (run / "events.jsonl").read_text().splitlines()]


def check_zero_cost(events: list[dict[str, Any]]) -> None:
    expect(not any(event["event"] == "call" for event in events), "fixture attempted a paid call")
    expect(
        all(event.get("charged_usd", 0) == 0 for event in events),
        "fixture recorded nonzero spending",
    )


def check_closed(base: str) -> None:
    try:
        http_json(base, "api/snapshot")
    except urllib.error.HTTPError as error:
        raise ValueError("run's embedded viewer still answers after its command exited") from error
    except urllib.error.URLError as error:
        expect(
            isinstance(error.reason, ConnectionRefusedError),
            "viewer shutdown could not be proven by a refused connection",
        )
        return
    except ConnectionResetError:
        return
    raise ValueError("run's embedded viewer still serves after its command exited")


def check_invalid_cursor(command: Path, run: Path, base: str) -> None:
    result = subprocess.run(
        [str(command), "observe", str(run), "--after", "invalid-cursor"],
        env=environment(),
        capture_output=True,
        text=True,
        timeout=15,
    )
    expect(result.returncode == 2, "invalid CLI cursor did not return exit two")
    cli_error = json.loads(result.stdout)
    expect(cli_error["kind"] == "fx-run-observation-error-v1", "wrong observation error contract")
    expect(cli_error["code"] == "invalid_cursor", "invalid CLI cursor has no explicit error code")
    try:
        http_json(base, batch_route("invalid-cursor"))
    except urllib.error.HTTPError as error:
        expect(error.code == 409, "invalid HTTP cursor did not return conflict")
        http_error = json.load(error)
        expect(http_error == cli_error, "CLI and HTTP cursor errors differ")
    else:
        raise ValueError("invalid HTTP cursor was accepted")


def check_delayed_demo(snapshot: dict[str, Any]) -> dict[str, int]:
    """Check the authored chain using recorded ordering and bytes, without timed sleeps."""
    instances = snapshot["plan"]["instances"]
    expect(len(instances) == 7, "delayed demo must contain seven planned instances")
    waits = [node for node in instances if node["uses"] == "./nodes/observation.py#wait"]
    transforms = [node for node in instances if node["uses"] == "./nodes/media.py#tint"]
    expect(len(waits) == 3, "delayed demo must contain three waits")
    expect(len(transforms) == 3, "delayed demo must contain three image transforms")
    seed = next(node for node in instances if node["path"] == "seed")
    expect(not seed["reads"], "seed unexpectedly depends on another instance")
    children: dict[str, dict[str, Any]] = {}
    for node in instances:
        if node is seed:
            continue
        expect(len(node["reads"]) == 1, "each later demo step must consume one prior image")
        source = node["reads"][0]
        expect(source not in children, "delayed demo branched instead of forming one chain")
        children[source] = node
    chain = [seed]
    seen = {seed["id"]}
    while chain[-1]["id"] in children:
        following = children[chain[-1]["id"]]
        expect(following["id"] not in seen, "delayed demo contains a dependency cycle")
        seen.add(following["id"])
        chain.append(following)
    expect(len(chain) == 7 and chain[-1]["path"] == "finished", "demo chain is disconnected")
    expect(
        [node["uses"] for node in chain[1:]]
        == ["./nodes/observation.py#wait", "./nodes/media.py#tint"] * 3,
        "waits and transforms must alternate along the data-dependency chain",
    )
    events = snapshot["events"]
    lifecycle = [event for event in events if event["event"] in {"node_started", "node_finished"}]
    expect(
        [(event["event"], event["id"]) for event in lifecycle]
        == [(event, node["id"]) for node in chain for event in ("node_started", "node_finished")],
        "demo steps did not start and finish in dependency order",
    )
    finished = {event["id"]: event for event in lifecycle if event["event"] == "node_finished"}
    digests = [finished[node["id"]]["outputs"]["image"]["file"]["digest"] for node in chain]
    expect(
        all(digests[index] == digests[index - 1] for index in (1, 3, 5)),
        "a wait changed the image instead of passing it through",
    )
    expect(
        all(digests[index] != digests[index - 1] for index in (2, 4, 6)),
        "an image transform produced no visible byte change",
    )
    expect(len(set(digests)) == 4, "demo must provide four distinct preview images")
    expect(
        all(event["cache"] == "miss" for event in finished.values()),
        "a fresh demo reused a cached step instead of exercising its body",
    )
    expect(
        all(node["state"] == "succeeded" for node in snapshot["view"]["nodes"]),
        "a completed demo has an unfinished viewer node",
    )
    return {"nodes": len(chain), "waits": len(waits), "distinct_images": len(set(digests))}


def check_follow(command: Path, output: Path) -> dict[str, Any]:
    home = copied_project(output, "follow")
    run = home.parent / "run"
    with ExitStack() as cleanup:
        child = Child(
            [
                str(command),
                "run",
                "viewer-observation",
                "--standalone",
                "--run",
                str(run),
                "--max-usd",
                "0",
                "--gate",
                "true",
                "--wait-seconds",
                "30",
            ],
            home,
            home.parent / "run.log",
        )
        cleanup.callback(child.close)
        base = child.address()
        attached = wait_running(base, child)
        expect(attached["kind"] == "fx-run-snapshot-v1", "wrong snapshot contract")
        prefix = attached["events"]
        cursor = attached["cursor"]
        expect(prefix == recorded_events(run), "snapshot cursor and event prefix differ")
        independent = cli_json(command, run, "--snapshot")
        expect(independent["events"] == prefix, "CLI and HTTP do not share recorded evidence")
        empty = cli_json(command, run, "--after", cursor, "--limit", "2")
        expect(not empty["events"] and empty["cursor"] == cursor, "idle poll changed frontier")
        check_invalid_cursor(command, run, base)
        expect(child.process.poll() is None, "an invalid observer request stopped execution")
        write_json(home.parent / "running.json", attached)

        # This observer deliberately reads nothing until the invocation has completed.
        viewer = Child(
            [str(command), "view", "--run", str(run), "--no-open"],
            home,
            home.parent / "view.log",
        )
        cleanup.callback(viewer.close)
        retained_base = viewer.address()
        (home / ".observation-release").write_text("release\n", encoding="utf-8")
        child.finish()
        check_closed(base)
        completed = cli_json(command, run, "--snapshot")
        all_events = recorded_events(run)
        expect(completed["events"] == all_events, "CLI snapshot differs from retained record")
        followed, frontier = catch_up(
            lambda after, limit: cli_json(command, run, "--after", after, "--limit", str(limit)),
            cursor,
        )
        expect(prefix + followed == all_events, "snapshot-to-follow lost or duplicated an event")
        http_followed, http_frontier = catch_up(
            lambda after, limit: http_json(retained_base, batch_route(after, limit)), cursor
        )
        expect(http_followed == followed, "HTTP and independent CLI catch-up differ")
        expect(http_frontier == frontier, "HTTP and CLI cursors diverged")
        replay = http_json(retained_base, batch_route(cursor))
        expect(replay["events"] == followed[:2], "retrying the same cursor did not replay events")
        finished = http_json(retained_base, "api/snapshot")
        expect(finished["view"]["state"] == "succeeded", "retained viewer missed completion")
        expect(finished["view"]["charged_usd"] == 0, "finished fixture recorded a charge")
        demo = check_delayed_demo(finished)
        for artifact in finished["view"]["artifacts"]:
            expect(artifact["available"], "completed fixture has an unavailable artifact")
            with CLIENT.open(retained_base.rstrip("/") + artifact["url"], timeout=5) as response:
                data = response.read()
            expect(
                hashlib.sha256(data).hexdigest() == artifact["digest"], "artifact digest differs"
            )
        write_json(home.parent / "finished.json", finished)

        # A finished invocation does not permanently close its selected run record.
        resumed = Child(
            [
                str(command),
                "run",
                "viewer-observation",
                "--run",
                str(run),
                "--max-usd",
                "0",
                "--gate",
                "true",
                "--wait-seconds",
                "30",
                "--no-view",
            ],
            home,
            home.parent / "resume.log",
        )
        cleanup.callback(resumed.close)
        resumed.finish()
        appended, _ = catch_up(
            lambda after, limit: cli_json(command, run, "--after", after, "--limit", str(limit)),
            frontier,
        )
        starts = [event for event in appended if event["event"] == "run_started"]
        expect(len(starts) == 1 and starts[0]["resumed"], "resume did not append one invocation")
        expect(
            starts[0]["invocation_id"] != prefix[0]["invocation_id"],
            "resumed invocation reused its predecessor's invocation id",
        )
        expect(all_events + appended == recorded_events(run), "resume cursor lost appended events")
        check_zero_cost(recorded_events(run))
        return {
            "attached_events": len(prefix),
            "followed_events": len(followed),
            "resumed_events": len(appended),
            **demo,
            "charged_usd": 0,
        }


def check_endings(command: Path, output: Path) -> dict[str, Any]:
    for name, expected in (("failure", 1), ("cancelled", 130), ("no-view", 0)):
        home = copied_project(output, name)
        run = home.parent / "run"
        args = [str(command), "run", "viewer-observation", "--run", str(run), "--max-usd", "0"]
        if name == "cancelled":
            args += ["--gate", "true", "--wait-seconds", "30"]
        else:
            args += ["--wait-seconds", "0"]
        if name == "failure":
            args += ["--fail-after-wait", "true"]
        if name == "no-view":
            args += ["--no-view"]
        else:
            args += ["--standalone"]
        child = Child(args, home, home.parent / "run.log")
        try:
            if name == "cancelled":
                base = child.address()
                wait_running(base, child)
                child.process.send_signal(signal.SIGINT)
            child.finish(expected)
            match = ADDRESS.search(child.output())
            expect(bool(match) == (name != "no-view"), f"{name}: unexpected viewer URL behavior")
            if match:
                check_closed(match.group(1))
            events = cli_json(command, run, "--snapshot")["events"]
            check_zero_cost(events)
            terminals = [
                event["event"]
                for event in events
                if event["event"] in {"run_finished", "run_cancelled"}
            ]
            expect(terminals, f"{name}: no recorded terminal event")
            if name == "cancelled":
                expect("run_cancelled" in terminals, "cancellation has no cancellation event")
            if name == "failure":
                expect(any(event["event"] == "node_failed" for event in events), "failure missing")
        finally:
            child.close()
    return {"failure_exit": 1, "cancelled_exit": 130, "no_view_exit": 0, "charged_usd": 0}


def generate(command: Path, output: Path) -> dict[str, Any]:
    report = {"follow": check_follow(command, output), "endings": check_endings(command, output)}
    write_json(output / "report.json", report)
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--command", type=Path, default=ROOT / "python/src/grida/fx/_bin/grida-fx")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    command = args.command.resolve()
    try:
        expect(command.is_file(), "build the engine first or pass --command")
        if args.output is not None:
            report = generate(command, prepare_output(args.output))
        else:
            with tempfile.TemporaryDirectory(prefix="fx-observation-") as scratch:
                report = generate(command, prepare_output(Path(scratch) / "evidence"))
        print(json.dumps(report, sort_keys=True))
        return 0
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"observation check failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
