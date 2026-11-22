#!/usr/bin/env python3
"""Verify local run control through public commands, without providers or a service."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "fixtures/control"
SCHEMAS = ROOT / "spec/schemas"
KEYS = (
    "GG_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "FAL_KEY",
    "TRIPO_API_KEY",
    "ELEVENLABS_API_KEY",
)


def expect(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def environment() -> dict[str, str]:
    env = {key: value for key, value in os.environ.items() if key not in KEYS}
    env.update(GRIDA_FX_DISABLE_DOTENV="1", GRIDA_FX_NETWORK="off", GRIDA_FX_PYTHON=sys.executable)
    return env


def validator(name: str) -> Draft202012Validator:
    return Draft202012Validator(json.loads((SCHEMAS / f"{name}.schema.json").read_text()))


CONTROL = validator("fx-run-control-v1")
EVENT = validator("fx-run-events-v1")


class Harness:
    def __init__(self, command: Path, output: Path, name: str) -> None:
        self.command = command
        self.home = output / name
        shutil.copytree(
            SOURCE,
            self.home,
            ignore=shutil.ignore_patterns(".control-*", ".fx", "runs", "__pycache__"),
        )
        self.run = self.home / "runs/test"
        self.child: subprocess.Popen | None = None
        self.log = self.home / "command.log"

    def call(self, *args: str, status: int = 0) -> dict[str, Any]:
        result = subprocess.run(
            [str(self.command), *args],
            cwd=self.home,
            env=environment(),
            capture_output=True,
            text=True,
            timeout=35,
        )
        expect(
            result.returncode == status,
            f"{args}: exit {result.returncode}, expected {status}: {result.stdout} {result.stderr}",
        )
        expect(len(result.stdout.encode()) <= 16384, "control result exceeded its payload bound")
        document = json.loads(result.stdout)
        CONTROL.validate(document)
        return document

    def start(self) -> None:
        with self.log.open("w") as stream:
            self.child = subprocess.Popen(
                [
                    str(self.command),
                    "run",
                    "workflow.yaml",
                    "--run",
                    str(self.run),
                    "--max-usd",
                    "0",
                    "--no-view",
                ],
                cwd=self.home,
                env=environment(),
                stdout=stream,
                stderr=subprocess.STDOUT,
            )

    def entered(self) -> None:
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if (self.home / ".control-entered").exists():
                return
            expect(self.child is not None and self.child.poll() is None, self.log.read_text())
            time.sleep(0.025)
        raise ValueError("control fixture did not enter its gate")

    def finish(self, status: int) -> None:
        assert self.child is not None
        expect(self.child.wait(timeout=15) == status, self.log.read_text())

    def events(self) -> list[dict[str, Any]]:
        events = [json.loads(line) for line in (self.run / "events.jsonl").read_text().splitlines()]
        for event in events:
            EVENT.validate(event)
        expect(
            not any(event["event"] == "call" for event in events),
            "fixture attempted a provider call",
        )
        expect(
            all(event.get("charged_usd", 0) == 0 for event in events), "fixture recorded a charge"
        )
        return events

    def close(self) -> None:
        if self.child is not None and self.child.poll() is None:
            self.child.terminate()
            try:
                self.child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait(timeout=5)


def journey(command: Path, output: Path) -> dict[str, Any]:
    harness = Harness(command, output, "journey")
    try:
        harness.start()
        harness.entered()
        inspected = harness.call("inspect", str(harness.run), "--control", "--json")
        expect(
            inspected["availability"] == "available" and inspected["can_cancel"],
            "no-view run has no control",
        )
        invocation = inspected["invocation_id"]
        refused = harness.call(
            "cancel", str(harness.run), "--invocation", "wrong", "--json", status=2
        )
        expect(refused["code"] == "invocation_mismatch", "guard did not refuse stale intent")
        refused = harness.call("cancel", "control-harness", "--json", status=2)
        expect(refused["code"] == "invalid_target", "bare workflow selected an implicit run")
        timed_out = harness.call(
            "cancel",
            str(harness.run),
            "--invocation",
            invocation,
            "--wait",
            "--timeout",
            "1s",
            "--json",
            status=1,
        )
        expect(
            timed_out["code"] == "wait_timeout" and timed_out["request_status"] == "accepted",
            "wait timeout lost acceptance",
        )
        completed = harness.call(
            "cancel", str(harness.run), "--invocation", invocation, "--wait", "--json"
        )
        expect(
            completed["outcome"] == "completed" and completed["cleanup"] == "complete",
            "cleanup is unverified",
        )
        expect(completed["recorded_state"] == "cancelled", "selected invocation did not cancel")
        harness.finish(130)
        events = harness.events()
        requested = [event for event in events if event["event"] == "cancel_requested"]
        expect(
            len(requested) == 1 and requested[0]["source"] == "cli",
            "duplicate/missing cancellation acceptance",
        )
        expect(
            not any(
                event["event"] == "node_started" and event.get("path") == "after"
                for event in events
            ),
            "downstream step admitted after cancellation",
        )
        expect(
            sum(event["event"] in {"run_cancelled", "run_finished"} for event in events) == 1,
            "duplicate terminal record",
        )
        legacy = harness.home / "legacy"
        shutil.copytree(harness.run, legacy)
        old = harness.call("cancel", str(legacy), "--json")
        expect(
            old["outcome"] == "already_terminal" and old["cleanup"] == "unknown",
            "portable terminal implies cleanup",
        )
        old = harness.call("cancel", str(legacy), "--wait", "--json", status=1)
        expect(
            old["code"] == "completion_unverified",
            "legacy terminal fabricated a completion receipt",
        )
        (harness.home / ".control-release").touch()
        harness.start()
        harness.finish(0)
        newer = harness.call("inspect", str(harness.run), "--control", "--json")
        expect(newer["invocation_id"] != invocation, "resume reused invocation identity")
        old = harness.call(
            "cancel", str(harness.run), "--invocation", invocation, "--json", status=2
        )
        expect(
            old["code"] == "invocation_mismatch", "ended successor bypassed expected-current guard"
        )
        done = harness.call("cancel", str(harness.run), "--wait", "--json")
        expect(done["recorded_state"] == "succeeded", "wait synthesized cancelled from a no-op")
        events = harness.events()
        expect(
            any(
                event["event"] == "node_finished" and event.get("path") == "checkpoint"
                for event in events
            )
            and not any(
                event["invocation_id"] == newer["invocation_id"]
                and event["event"] == "node_started"
                and event.get("path") == "checkpoint"
                for event in events
            ),
            "resume reran the saved checkpoint",
        )
        return {
            "timeout_kept_request": True,
            "guarded_resume": True,
            "checkpoint_reused": True,
            "charged_usd": 0,
        }
    finally:
        harness.close()


def signals(command: Path, output: Path) -> dict[str, Any]:
    for name, first, second in (
        ("term", signal.SIGTERM, signal.SIGTERM),
        ("interrupt", signal.SIGINT, signal.SIGTERM),
    ):
        harness = Harness(command, output, name)
        try:
            harness.start()
            harness.entered()
            assert harness.child is not None
            harness.child.send_signal(first)
            deadline = time.monotonic() + 5
            while not any(event["event"] == "cancel_requested" for event in harness.events()):
                expect(time.monotonic() < deadline, "signal did not record acceptance")
                time.sleep(0.025)
            harness.child.send_signal(second)
            harness.finish(130)
            events = harness.events()
            expect(
                sum(event["event"] == "cancel_requested" for event in events) == 1,
                "repeated termination duplicated acceptance",
            )
            expect(
                events[-1]["event"] == "run_cancelled",
                "repeated termination forced the bookkeeping owner",
            )
            done = harness.call("cancel", str(harness.run), "--wait", "--json")
            expect(done["cleanup"] == "complete", "signal cleanup receipt missing")
        finally:
            harness.close()
    return {"repeated_sigterm_is_idempotent": True, "sigint_uses_same_acceptance": True}


def pause_journey(command: Path, output: Path) -> None:
    harness = Harness(command, output, "pause")
    try:
        workflow = harness.home / "workflow.yaml"
        workflow.write_text(
            workflow.read_text() + "  early: ${{ steps.checkpoint.outputs.text }}\n"
        )
        harness.start()
        harness.entered()
        target = str(harness.run)
        inspected = harness.call("inspect", target, "--control", "--json")
        assert inspected["can_pause"]
        invocation = inspected["invocation_id"]
        accepted = harness.call("pause", target, "--invocation", invocation, "--json")
        assert accepted["outcome"] == "accepted" and accepted["recorded_state"] == "pausing"
        timeout = harness.call("pause", target, "--wait", "--timeout", "1s", "--json", status=1)
        assert timeout["code"] == "wait_timeout" and timeout["request_status"] == "accepted"
        (harness.home / ".control-release").touch()
        paused = harness.call("pause", target, "--wait", "--json")
        assert paused["outcome"] == "completed" and paused["recorded_state"] == "paused"
        assert paused["cleanup"] == "complete" and paused["invocation_id"] == invocation
        harness.finish(1)
        events = harness.events()
        assert sum(event["event"] == "pause_requested" for event in events) == 1
        assert events[-1]["event"] == "run_paused"
        assert set(events[-1]["outputs"]) == {"early"}
        early = next(
            event
            for event in events
            if event.get("id") == "checkpoint#1" and event["event"] == "node_finished"
        )
        assert events[-1]["outputs"]["early"] == early["outputs"]["text"]
        assert (harness.run / "outputs/early.txt").is_file()
        assert any(
            event["event"] == "node_finished" and event.get("path") == "hold" for event in events
        )
        assert not any(
            event["event"] == "node_started" and event.get("path") == "after" for event in events
        )

        resumed = subprocess.run(
            [command, "resume", target, "--no-view"],
            cwd=harness.home,
            env=environment(),
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )
        expect(resumed.returncode == 0, resumed.stdout + resumed.stderr)
        newest = harness.call("inspect", target, "--control", "--json")
        assert newest["recorded_state"] == "succeeded" and newest["invocation_id"] != invocation
        assert [
            event["path"]
            for event in harness.events()
            if event["event"] == "node_started"
            and event["invocation_id"] == newest["invocation_id"]
        ] == ["after"]
        stale = harness.call("pause", target, "--invocation", invocation, "--json", status=2)
        assert stale["code"] == "invocation_mismatch"
    finally:
        harness.close()


def pause_interrupted(command: Path, output: Path) -> None:
    for crash in (False, True):
        harness = Harness(command, output, "pause-crash" if crash else "pause-cancel")
        try:
            harness.start()
            harness.entered()
            target = str(harness.run)
            harness.call("pause", target, "--json")
            if crash:
                assert harness.child is not None
                harness.child.kill()
                harness.finish(-signal.SIGKILL)
                inspected = harness.call("inspect", target, "--control", "--json")
                assert inspected["recorded_state"] == "unfinished"
            else:
                result = harness.call("cancel", target, "--wait", "--json")
                assert result["recorded_state"] == "cancelled"
                harness.finish(130)
                observed = harness.call("pause", target, "--wait", "--json")
                assert (
                    observed["recorded_state"] == "cancelled"
                    and observed["request_status"] == "accepted"
                )
            events = harness.events()
            assert any(event["event"] == "pause_requested" for event in events)
            assert not any(event["event"] == "run_paused" for event in events)
        finally:
            harness.close()


def pause_terminal_outcomes(command: Path, output: Path) -> None:
    for outcome in ("succeeded", "failed"):
        harness = Harness(command, output, f"pause-{outcome}")
        try:
            workflow = harness.home / "workflow.yaml"
            workflow.write_text(
                workflow.read_text().split("  after:", 1)[0]
                + "outputs:\n  text: ${{ steps.hold.outputs.text }}\n"
            )
            if outcome == "failed":
                nodes = harness.home / "nodes.py"
                nodes.write_text(
                    nodes.read_text().replace(
                        'return {"text": ctx.inputs["text"]}',
                        'raise ctx.fail("The released gate failed")',
                        1,
                    )
                )
            harness.start()
            harness.entered()
            target = str(harness.run)
            harness.call("pause", target, "--json")
            (harness.home / ".control-release").touch()
            result = harness.call("pause", target, "--wait", "--json")
            assert result["outcome"] == "completed" and result["recorded_state"] == outcome
            harness.finish(0 if outcome == "succeeded" else 1)
            assert harness.events()[-1]["event"] == "run_finished"
        finally:
            harness.close()


async def python_sdk(command: Path, output: Path, *, pausing: bool = False) -> dict[str, Any]:
    from grida.fx import (
        FxError,
        cancel_async,
        inspect_control_async,
        pause_async,
        resume_async,
        run_async,
    )

    harness = Harness(command, output, "python-sdk-pause" if pausing else "python-sdk")
    task = asyncio.create_task(
        run_async("workflow.yaml", run_dir=harness.run, cwd=harness.home, max_usd=0)
    )
    try:
        deadline = time.monotonic() + 15
        while not (harness.home / ".control-entered").exists():
            expect(not task.done() and time.monotonic() < deadline, "SDK run never entered gate")
            await asyncio.sleep(0.025)
        current = await inspect_control_async(harness.run, cwd=harness.home)
        expect(current.availability == "available", "Python SDK run has no control endpoint")
        if pausing:
            assert current.can_pause
            accepted = await pause_async(harness.run, cwd=harness.home)
            assert accepted.outcome == "accepted" and accepted.recorded_state == "pausing"
            (harness.home / ".control-release").touch()
            result = await pause_async(
                harness.run, invocation=current.invocation_id, wait=True, cwd=harness.home
            )
            assert result.cleanup == "complete" and result.recorded_state == "paused"
            assert any(
                event["event"] == "pause_requested" and event["source"] == "sdk"
                for event in harness.events()
            )
            paused = await task
            assert paused.paused and paused.incomplete and not paused.ok
            assert "hold" in paused.steps and "after" not in paused.steps
            resumed = await resume_async(harness.run, cwd=harness.home)
            assert resumed.ok and not resumed.paused
            assert resumed.run_dir.resolve() == paused.run_dir.resolve()
            assert resumed.invocation_id != paused.invocation_id
            assert resumed.outputs["text"].path.read_text() == "saved checkpoint"
            return {"cleanup_verified": True, "resumed_same_attempt": True, "source": "sdk"}
        result = await cancel_async(
            harness.run, invocation=current.invocation_id, wait=True, cwd=harness.home
        )
        expect(result.cleanup == "complete", "Python SDK wrapper did not verify cleanup")
        try:
            await task
        except FxError as error:
            expect(error.status == 130, "cancelled Python SDK run lost exit 130")
        else:
            raise ValueError("cancelled Python SDK run unexpectedly succeeded")
        events = harness.events()
        expect(
            any(
                event["event"] == "cancel_requested" and event["source"] == "sdk"
                for event in events
            ),
            "Python control wrapper did not record SDK source",
        )
        return {"control_available": True, "cleanup_verified": True, "source": "sdk"}
    finally:
        if not task.done():
            task.cancel()
            try:
                await task
            except (asyncio.CancelledError, FxError):
                pass


def javascript_sdk(command: Path, output: Path, *, pausing: bool = False) -> dict[str, Any]:
    harness = Harness(command, output, "javascript-sdk-pause" if pausing else "javascript-sdk")
    script = harness.home / "check-sdk.ts"
    script.write_text(
        "import {run, inspectControl, cancel, pause, resume} from "
        + json.dumps(str(ROOT / "js/fx/src/index.ts"))
        + ";\n"
        + "import {existsSync, realpathSync, writeFileSync} from 'node:fs';\n"
        + "const home = "
        + json.dumps(str(harness.home))
        + ";\n"
        + "const folder = "
        + json.dumps(str(harness.run))
        + ";\n"
        + "const pausing = "
        + json.dumps(pausing)
        + ";\n"
        + "const execution = run('workflow.yaml', {cwd:home, runDir:folder, maxUsd:0})"
        + ".catch(error => {if(pausing || error.exitCode !== 130) throw error; return null;});\n"
        + "const deadline = Date.now()+15000;\n"
        + "while(!existsSync(home+'/.control-entered')) {"
        + "if(Date.now()>deadline) throw new Error('gate never entered'); await Bun.sleep(25);}\n"
        + "const current = await inspectControl(folder, {cwd:home});\n"
        + "if(current.availability !== 'available') throw new Error('control unavailable');\n"
        + "if(pausing) {\n"
        + "  if(!current.can_pause) throw new Error('pause unavailable');\n"
        + "  const accepted = await pause(folder, {cwd:home});\n"
        + "  if(accepted.recorded_state !== 'pausing') throw new Error('pause not accepted');\n"
        + "  writeFileSync(home+'/.control-release', '');\n"
        + "}\n"
        + "const result = await (pausing ? pause : cancel)(folder, "
        + "{cwd:home, invocation:current.invocation_id, wait:true});\n"
        + "if(result.cleanup !== 'complete') throw new Error('cleanup unverified');\n"
        + "const outcome = await execution;\n"
        + "if(pausing) {\n"
        + "  if(result.recorded_state !== 'paused' || !outcome?.paused || !outcome.incomplete)"
        + " throw new Error('run did not pause');\n"
        + "  if(outcome.steps['hold#1']?.state !== 'succeeded' || outcome.steps['after#1'])"
        + " throw new Error('unexpected pause boundary');\n"
        + "  const resumed = await resume(folder, {cwd:home});\n"
        + "  if(!resumed.ok || resumed.paused"
        + " || realpathSync(resumed.runDir) !== realpathSync(outcome.runDir)"
        + " || resumed.invocationId === outcome.invocationId)"
        + " throw new Error('resume did not continue the same attempt');\n"
        + "  if((await resumed.outputs.text.read()).toString() !== 'saved checkpoint')"
        + " throw new Error('resume lost the checkpoint');\n"
        + "} else if(outcome !== null) throw new Error('cancelled run succeeded');\n"
        + "console.log(JSON.stringify(result));\n"
    )
    result = subprocess.run(
        ["bun", "--no-env-file", str(script)],
        cwd=harness.home,
        env=environment() | {"GRIDA_FX_BIN": str(command)},
        capture_output=True,
        text=True,
        timeout=25,
    )
    expect(result.returncode == 0, f"JavaScript SDK integration failed: {result.stderr}")
    CONTROL.validate(json.loads(result.stdout))
    events = harness.events()
    request = "pause_requested" if pausing else "cancel_requested"
    expect(
        any(event["event"] == request and event["source"] == "sdk" for event in events),
        "JavaScript control wrapper did not record SDK source",
    )
    return {
        "control_available": True,
        "cleanup_verified": True,
        "source": "sdk",
        **({"resumed_same_attempt": True} if pausing else {}),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--command", type=Path, default=ROOT / "target/debug/grida-fx")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        command = args.command.resolve(strict=True)
        with tempfile.TemporaryDirectory(prefix="fx-control-check-") as scratch:
            output = args.output.absolute() if args.output else Path(scratch) / "evidence"
            expect(not output.is_symlink(), "output is a symbolic link")
            expect(not output.exists() or not any(output.iterdir()), "output must be new or empty")
            output.mkdir(parents=True, exist_ok=True)
            for key in KEYS:
                os.environ.pop(key, None)
            os.environ.update(environment(), GRIDA_FX_BIN=str(command))
            pause_journey(command, output)
            pause_interrupted(command, output)
            pause_terminal_outcomes(command, output)
            report = {
                "pause": {
                    "resume_reused_step": True,
                    "cancel_and_crash": True,
                    "terminal_outcomes": True,
                },
                "journey": journey(command, output),
                "signals": signals(command, output),
                "python_sdk": asyncio.run(python_sdk(command, output)),
                "javascript_sdk": javascript_sdk(command, output),
                "python_sdk_pause": asyncio.run(python_sdk(command, output, pausing=True)),
                "javascript_sdk_pause": javascript_sdk(command, output, pausing=True),
            }
            (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, sort_keys=True))
        return 0
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"control check failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
