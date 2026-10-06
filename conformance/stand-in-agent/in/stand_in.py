"""The stand-in of the stand-in-agent case.

- agent.turn, for the task `Measure the lantern, then submit its height.`: the first turn calls
  the tool `measure`; the next one, which sees the tool's answer as the last message, submits
  the height it read there. For the task `Garble the reply.`: data that is not
  `{"text", "tool_calls"}` (its `text` is a number), which the engine's check of agent.turn
  refuses.
- structured.generate: `Estimate the height of the lantern.` gets a value that meets the
  schema; `Answer with the wrong type.` one whose `height` is text, which the schema refuses.

It counts its calls in `count.txt` and keeps what it was asked in `asked.json`, in its working
directory, which is the engine's: the project the case runs in. `asked.json` holds no path, only
whether each file the engine handed over can be read where its ref says.
"""

import json
from pathlib import Path
from typing import Any

from grida.fx import DECLINE, Answer, InputFile, StandInCall

COUNT = Path("count.txt")
ASKED = Path("asked.json")


def answer(call: StandInCall) -> Any:
    note(call)
    if call.capability == "agent.turn":
        return turn(call)
    if call.capability == "structured.generate":
        if call.request["prompt"] == "Answer with the wrong type.":
            return Answer.json({"height": "three", "unit": "hands"})
        return Answer.json({"height": 3, "unit": "hands"})
    return DECLINE


def turn(call: StandInCall) -> Answer:
    messages = call.request["messages"]
    if messages[0]["content"] == "Garble the reply.":
        return Answer(data={"text": 7, "tool_calls": []})
    last = messages[-1]
    if last["role"] == "tool":
        measured = json.loads(last["content"])
        return Answer.submit(height=measured["height"], unit=measured["unit"])
    return Answer.turn(
        "I measure it first.",
        [{"id": "call_measure", "name": "measure", "arguments": {"thing": "lantern"}}],
    )


# ------------------------------------------------------------------------------ the notes


def note(call: StandInCall) -> None:
    """Counts the call in count.txt and adds what it was asked to asked.json, sorted, so the
    file does not depend on the order calls arrive in."""

    count = int(COUNT.read_text()) if COUNT.is_file() else 0
    COUNT.write_text(f"{count + 1}\n")
    asked = json.loads(ASKED.read_text()) if ASKED.is_file() else []
    asked.append(
        {
            "capability": call.capability,
            "route": {"id": call.route.id, "fingerprint": call.route.fingerprint},
            "request": plain(call.request),
            "take": list(call.takes),
            "key": call.key,
            "instance": {
                "id": call.instance.id,
                "path": call.instance.path,
                "step": call.instance.step,
            },
            "files": {
                digest: {
                    "name": file.name,
                    "kind": file.kind,
                    "size": file.size,
                    "facts": file.facts,
                    "path_is_file": file.path.is_file(),
                }
                for digest, file in sorted(call.files.items())
            },
        }
    )
    asked.sort(key=lambda entry: json.dumps(entry, sort_keys=True))
    ASKED.write_text(json.dumps(asked, indent=1, sort_keys=True) + "\n")


def plain(value: Any) -> Any:
    """A request as the wire carries it: each file as {"file": digest}."""

    if isinstance(value, InputFile):
        return {"file": value.digest}
    if isinstance(value, dict):
        return {key: plain(item) for key, item in value.items()}
    if isinstance(value, list):
        return [plain(item) for item in value]
    return value
