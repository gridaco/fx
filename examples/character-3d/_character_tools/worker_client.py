"""Async bounded Blender invocations with provider-free worker environments."""

from __future__ import annotations

import asyncio
import json
import math
import os
import signal
import time
from pathlib import Path
from typing import Any

from _character_tools.io import (
    confined,
    read_json,
    write_bytes,
    write_json,
)


class WorkerRefusal(ValueError):
    """A worker terminated by raising a declared error, not by crashing.

    The worker entrypoints print one ``WORKER_ERROR`` JSON line before exiting
    nonzero. Callers that own a bounded semantic recovery may classify the
    message; everything else remains a terminal failure.
    """

    def __init__(self, error_type: str, message: str, log: str) -> None:
        super().__init__(f"Worker refused: {error_type}: {message}")
        self.error_type, self.message, self.log = (error_type, message, log)


def worker_refusal(log: str) -> WorkerRefusal | None:
    """Parse the worker's declared error from its captured output, if any."""
    for line in reversed(log.splitlines()):
        if line.startswith("WORKER_ERROR "):
            try:
                payload = json.loads(line.removeprefix("WORKER_ERROR "))
            except ValueError:
                return None
            if (
                isinstance(payload, dict)
                and isinstance(payload.get("error_type"), str)
                and isinstance(payload.get("message"), str)
            ):
                return WorkerRefusal(payload["error_type"], payload["message"], log[-3000:])
            return None
    return None


class WorkerClient:
    def __init__(
        self,
        *,
        blender: Path,
        package_root: Path,
        input_root: Path,
        run_root: Path,
        max_calls: int = 100,
        timeout_seconds: float = 180,
    ) -> None:
        self.blender = blender.resolve(strict=True)
        self.package_root = package_root.resolve(strict=True)
        self.input_root = input_root.resolve(strict=True)
        self.run_root = run_root.resolve(strict=True)
        if type(max_calls) is not int or not 1 <= max_calls <= 1000:
            raise ValueError("Worker call limit must be an integer from 1 through 1000")
        if (
            type(timeout_seconds) not in {int, float}
            or not math.isfinite(timeout_seconds)
            or (not 0 < timeout_seconds <= 1800)
        ):
            raise ValueError("Worker timeout must be positive, finite and at most 1800 seconds")
        self.max_calls = max_calls
        self.timeout_seconds = timeout_seconds
        self.calls = 0

    async def execute(
        self, request: dict[str, Any], *, script: str = "main.py"
    ) -> tuple[dict[str, Any], str]:
        if script not in {
            "main.py",
            "assemble.py",
            "rig_cli.py",
            "rig_metrics.py",
            "provider_rig_cli.py",
        }:
            raise ValueError("Undeclared worker entrypoint")
        if self.calls >= self.max_calls:
            raise ValueError("Run exhausted its worker operation budget")
        self.calls += 1
        operation_id = f"worker-{self.calls:04d}"
        output = confined(self.run_root, request["output_dir"], must_exist=False)
        if output.exists():
            raise ValueError("Worker output already exists")
        request_path = confined(self.run_root, f"requests/{operation_id}.json", must_exist=False)
        write_json(request_path, request)
        environment = {
            key: os.environ[key]
            for key in ("PATH", "LANG", "LC_ALL", "TMPDIR", "SYSTEMROOT")
            if key in os.environ
        }
        environment["PYTHONNOUSERSITE"] = "1"
        command = [
            str(self.blender),
            "--background",
            "--factory-startup",
            "--disable-autoexec",
            "--python",
            str(self.package_root / "worker" / script),
            "--",
            "--request",
            str(request_path),
            "--input-root",
            str(self.input_root),
            "--output-root",
            str(self.run_root),
        ]
        started = time.monotonic()
        process = await asyncio.create_subprocess_exec(
            *command,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
            env=environment,
            start_new_session=True,
        )
        try:
            assert process.stdout is not None
            async with asyncio.timeout(self.timeout_seconds):
                stdout = bytearray()
                while chunk := (await process.stdout.read(65536)):
                    stdout.extend(chunk)
                    if len(stdout) > 4 * 1024 * 1024:
                        del stdout[: len(stdout) - 4 * 1024 * 1024]
                await process.wait()
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGTERM)
                async with asyncio.timeout(5):
                    await process.wait()
            except (ProcessLookupError, TimeoutError):
                if process.returncode is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    await process.wait()
            write_json(
                self.run_root / "operations" / f"{operation_id}.json",
                {
                    "status": "interrupted",
                    "request": f"requests/{operation_id}.json",
                    "duration_seconds": time.monotonic() - started,
                },
            )
            raise
        log = stdout.decode("utf-8", errors="replace")
        for root, label in (
            (self.run_root, "run://"),
            (self.input_root, "input://"),
            (self.package_root, "package://"),
        ):
            log = log.replace(str(root) + "/", label)
        write_bytes(
            confined(self.run_root, f"logs/{operation_id}.txt", must_exist=False), log.encode()
        )
        write_json(
            self.run_root / "operations" / f"{operation_id}.json",
            {
                "status": "succeeded" if process.returncode == 0 else "failed",
                "request": f"requests/{operation_id}.json",
                "exit_code": process.returncode,
                "duration_seconds": time.monotonic() - started,
                "log": f"logs/{operation_id}.txt",
            },
        )
        if process.returncode != 0:
            refusal = worker_refusal(log)
            if refusal is not None:
                raise refusal
            raise ValueError(f"Worker failed: {log[-3000:]}")
        return (read_json(output / "report.json"), request["output_dir"])
