"""Synthetic provider protocol: no network access, including on a live invocation."""

import asyncio
import json
import os
from pathlib import Path

from grida.fx import Accepted, AdapterDiagnostic, Answered, Failed, Uncertain, adapter


def submit(call):
    source = call.request["source"]
    record = {
        "key": call.key,
        "operation": call.request["operation"],
        "future": call.request["future_face_budget"],
        "vendor_extension": call.request["vendor_extension"],
        "source": source.read_bytes().decode(),
    }
    with Path(".native-submits").open("a") as output:
        output.write(json.dumps(record) + "\n")
    mode = call.request["mode"]
    if mode in ("reject", "uncertain"):
        diagnostic = AdapterDiagnostic(
            source="provider",
            phase="submit",
            operation=call.request["operation"],
            http_status=400 if mode == "reject" else 502,
            code="bad_option",
            field="future_face_budget",
            request_id="req-native-1",
            detail="available",
            message="Unsupported future_face_budget; "
            + os.environ["ACME_NATIVE_KEY"]
            + " https://assets.acme.test/a?signature=made-up-signature /private/native/input.txt "
            + source.read_bytes().decode(),
        )
        if mode == "uncertain":
            return Uncertain("Provider receipt is unknown", diagnostic=diagnostic)
        return Failed("Provider rejected the input", retryable=False, diagnostic=diagnostic)
    return Accepted({"task_id": "task-" + call.key[:16]})


async def collect(call):
    Path(".native-collecting").touch()
    while not Path(".native-release").exists():
        await asyncio.sleep(0.02)
    return Answered.json(
        {
            "task_id": call.handle["task_id"],
            "future": call.request["future_face_budget"],
            "operation": call.request["operation"],
        },
        cost_usd=0.01,
    )


native_adapter = adapter(jobs=["acme.native"], submit=submit, collect=collect, version=1)
