"""The stand-in of the stand-in-flags case: it counts the call in `count.txt`, in its working
directory (the project the case runs in), and then breaks, as a test harness with a bug would.
A stand-in that raises anything but CallRefused or CallFailed is a fault of the stand-in, and
the engine stops the run."""

from pathlib import Path

from grida.fx import StandInCall

COUNT = Path("count.txt")


def answer(call: StandInCall) -> None:
    count = int(COUNT.read_text()) if COUNT.is_file() else 0
    COUNT.write_text(f"{count + 1}\n")
    raise RuntimeError(f"the test harness broke on {call.capability}")
