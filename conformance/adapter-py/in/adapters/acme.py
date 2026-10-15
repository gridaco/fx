"""A synthetic adapter for the made-up provider acme: it answers from what it is asked and makes
no request. Its one fetch is refused before it leaves, since a conformance step runs with the
network off. `acme` answers at once; `acme_jobs` submits a long job and collects it."""

import os

from grida.fx import Accepted, Answered, FetchError, Refused, adapter

from adapters import colours


async def send(call):
    if call.request["prompt"] == "refuse":
        # The key in a reason reaches no record: the engine redacts it.
        return Refused("no colours for " + os.environ["ACME_API_KEY"])
    try:
        await call.fetch(call.base_url + "/chat", method="POST", json={}, credential=True)
        fetched = None
    except FetchError as error:
        fetched = [error.phase, error.reason]
    answer = {
        "name": colours.ASKED,
        "fetched": fetched,
        "contract": call.route.contract,
        "attempt": call.attempt,
        "key_given": "ACME_API_KEY" in os.environ,
    }
    return Answered.json(answer, cost_usd=0.01, usage={"input_tokens": 5, "output_tokens": 3})


def submit(call):
    return Accepted({"job": call.key[:12]})


async def collect(call):
    answer = {
        "name": colours.WAITED_FOR,
        "job": call.handle["job"],
        "contract": call.route.contract,
        "key_given": "ACME_API_KEY" in os.environ,
    }
    return Answered.json(answer, cost_usd=0.02)


acme = adapter(requests=["structured.generate"], send=send, version=1)
acme_jobs = adapter(jobs=["structured.generate"], submit=submit, collect=collect, version=1)
