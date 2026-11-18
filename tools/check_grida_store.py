"""Check FX's reader against Grida's fixture-only provider-store writer.

Build `grida-auth-conformance` in Grida at the revision pinned in spec/providers.md:

    cargo build --locked -p grida-auth --features conformance --bin grida-auth-conformance

Then, from FX:

    python tools/check_grida_store.py --engine target/debug/grida-fx --driver /path/to/driver

This check uses synthetic GG data in a disposable home. It exercises actual Grida
storage and the shared SQLite lock without contacting a provider. Verification of
the public Grida GG configure/remove commands is a separate integration check.
"""

from __future__ import annotations

import argparse
import json
import os
import selectors
import subprocess
import sys
import tempfile
from pathlib import Path

KEY = "synthetic-gg-api-key-for-fx-compatibility"
PRESENT = "key       GG_API_KEY present (grida)"
MISSING = "key       GG_API_KEY missing"
BUSY = "keys      Grida provider credentials: store_busy."


def expect(condition: object, message: str) -> None:
    if not condition:
        raise ValueError(message)


def request(home: Path, operation: str, **fields: str) -> str:
    return json.dumps({"home": str(home), "operation": operation, **fields}) + "\n"


def read_doctor(output: str, status: int, expected: str, home: Path) -> None:
    expect(status == 0 and expected in output.splitlines(), f"FX did not report {expected!r}")
    expect(KEY not in output and str(home) not in output, "FX printed private custody data")
    print(expected, flush=True)


def doctor(
    engine: Path, project: Path, env: dict[str, str], *, timeout: int = 10
) -> tuple[str, int]:
    result = subprocess.run(
        [engine, "doctor"],
        cwd=project,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    return result.stdout + result.stderr, result.returncode


def check_lock(driver: Path, engine: Path, home: Path, project: Path, env: dict[str, str]) -> None:
    with subprocess.Popen(
        [driver],
        cwd=project,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    ) as holder:
        assert holder.stdin is not None and holder.stdout is not None
        try:
            holder.stdin.write(request(home, "lock"))
            holder.stdin.flush()
            with selectors.DefaultSelector() as selector:
                selector.register(holder.stdout, selectors.EVENT_READ)
                expect(selector.select(timeout=10), "Grida did not acquire the lock")
                expect(
                    json.loads(holder.stdout.readline()) == {"event": "locked"}, "wrong lock event"
                )

            # Keep the writer transaction open through the reader's real 30-second deadline.
            output, status = doctor(engine, project, env, timeout=50)
            expect(holder.poll() is None, "Grida exited before releasing the lock")
            expect(status == 1 and BUSY in output, "FX did not report store_busy under contention")
            expect(PRESENT not in output, "FX reported the key while Grida held the lock")
            expect(KEY not in output and str(home) not in output, "FX printed private custody data")
            print(BUSY, flush=True)
            holder.stdin.write("{}\n")
            holder.stdin.flush()
            expect(holder.wait(timeout=10) == 0, "Grida did not release the lock")
            expect(json.loads(holder.stdout.readline()).get("ok"), "Grida lock release failed")
            output, status = doctor(engine, project, env)
            read_doctor(output, status, PRESENT, home)
        finally:
            if holder.poll() is None:
                holder.kill()


def check(engine: Path, driver: Path, scratch: Path) -> None:
    home = scratch / "grida-home"
    home.mkdir(mode=0o700)
    (home / ".grida-auth-conformance").write_text("synthetic-fixture-only\n", encoding="utf-8")
    project = scratch / "project"
    project.mkdir()
    (project / "fx.yaml").write_text("fx: project/v1\n", encoding="utf-8")
    env = {
        "PATH": os.defpath,
        "HOME": str(home),
        "GRIDA_HOME": str(home),
        "GRIDA_FX_NETWORK": "off",
        "GRIDA_FX_DISABLE_DOTENV": "1",
        "GRIDA_FX_PYTHON": sys.executable,
    }

    def change(operation: str, **fields: str) -> None:
        result = subprocess.run(
            [driver],
            cwd=project,
            env=env,
            input=request(home, operation, provider="gg", **fields),
            capture_output=True,
            text=True,
            timeout=10,
        )
        expect(
            result.returncode == 0 and json.loads(result.stdout).get("ok"), f"{operation} failed"
        )

    change("provider-set", key=KEY)
    credentials = home / "providers" / "credentials.toml"
    saved = credentials.read_bytes()
    lock = home / "providers" / "profile.lock.sqlite"
    before = lock.stat()
    check_lock(driver, engine, home, project, env)
    after = lock.stat()
    expect(credentials.read_bytes() == saved, "FX changed Grida's credential file")
    expect(
        (after.st_dev, after.st_ino, after.st_size) == (before.st_dev, before.st_ino, 0),
        "FX replaced or changed the lock file",
    )

    change("provider-remove")
    output, status = doctor(engine, project, env)
    read_doctor(output, status, MISSING, home)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", type=Path, required=True, help="newly built grida-fx")
    parser.add_argument(
        "--driver", type=Path, required=True, help="Grida's fixture-only auth driver"
    )
    args = parser.parse_args()
    if sys.platform not in {"darwin", "linux"}:
        parser.error("the provider credential store requires macOS or Linux")
    try:
        with tempfile.TemporaryDirectory(prefix="fx-grida-store-") as folder:
            check(args.engine.resolve(), args.driver.resolve(), Path(folder).resolve())
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"check_grida_store: {error}", file=sys.stderr)
        return 1
    print("check_grida_store: shared lock, Grida write/remove and FX diagnosis passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
