"""Project adapters against the real engine (``spec/adapters.md`` section 9, ``spec/protocol.md``
sections 5.8, 6.8 and 8.1).

These run the ``grida-fx`` that ``tools/build_engine.py`` put beside the SDK, so planning loads
the adapters on this SDK's planning host and a live run starts an adapter host of this SDK for
each declaration. They are skipped when no engine was built. The project is a scratch one: two
declared providers, ``acme`` (with a made-up key) served by a request adapter and ``acme-jobs``
(keyless) by a long-job adapter of the same module, each on a capped ``structured.generate``
route. The network is off, so every ``fetch`` is refused before it leaves, and no provider key is
in the environment.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

import grida.fx as fx
from grida.fx import Failure, _api

ENGINE = _api._packaged_binary()

pytestmark = pytest.mark.skipif(
    os.name != "posix" or not ENGINE.is_file(),
    reason="needs the engine beside the SDK (python3 tools/build_engine.py) on a POSIX system",
)

#: The keys of FX's built-in providers (``spec/providers.md`` section 1), kept out of the runs.
KEYS = (
    "GG_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "FAL_KEY",
    "TRIPO_API_KEY",
    "ELEVENLABS_API_KEY",
)
#: A made-up key for ``acme``, which the run's reasons must never show.
MADE_UP_KEY = "acme-made-up-key-7c1d"

PROJECT = """\
fx: project/v1
route_tables: [routes.yaml]
providers:
  acme:
    credential: { variable: ACME_API_KEY }
    base_url: { default: "https://api.acme.test/v1" }
    adapter: adapters/acme.py#acme
  acme-jobs:
    base_url: { default: "https://jobs.acme.test/v1" }
    adapter: adapters/acme.py#acme_jobs
"""

ROUTES = """\
fx: routes/v1
routes:
  - { capability: structured.generate, route: text-a@acme, price: { cap_usd: 0.05 } }
  - { capability: structured.generate, route: text-b@acme-jobs, price: { cap_usd: 0.05 } }
"""

WORKFLOW = """\
fx: workflow/v1
id: colours
title: Colours
steps:
  name:
    uses: fx/structured.generate@1
    route: text-a@acme
    with: { prompt: Name a colour., schema: ./schemas/colour.json }
  slowly:
    uses: fx/structured.generate@1
    route: text-b@acme-jobs
    with: { prompt: Name a colour slowly., schema: ./schemas/colour.json }
  refused:
    uses: fx/structured.generate@1
    route: text-a@acme
    with: { prompt: refuse, schema: ./schemas/colour.json }
outputs:
  name: ${{ steps.name.outputs.json }}
  slowly: ${{ steps.slowly.outputs.json }}
"""

ADAPTER = """\
import os

from grida.fx import Accepted, Answered, FetchError, Refused, adapter


def seen():
    return {"key": "ACME_API_KEY" in os.environ, "cwd": os.path.basename(os.getcwd())}


async def send(call):
    if call.request["prompt"] == "refuse":
        return Refused("no colours for " + os.environ["ACME_API_KEY"])
    try:
        url = call.base_url + "/chat"
        await call.fetch(url, method="POST", json={"model": call.route.model}, credential=True)
        fetched = None
    except FetchError as error:
        fetched = [error.phase, error.reason]
    answer = {"name": "teal", "fetched": fetched, "route": call.route.contract, **seen()}
    return Answered.json(answer, cost_usd=0.01, usage={"input_tokens": 5})


def submit(call):
    return Accepted({"job": call.key[:8]})


async def collect(call):
    answer = {"name": "slate", "job": call.handle["job"], "route": call.route.contract, **seen()}
    return Answered.json(answer, cost_usd=0.02)


acme = adapter(requests=["structured.generate"], send=send, version=1)
acme_jobs = adapter(jobs=["structured.generate"], submit=submit, collect=collect, version=3)
"""


@pytest.fixture(autouse=True)
def engine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(_api.BINARY_VARIABLE, str(ENGINE))
    monkeypatch.setenv("GRIDA_FX_NETWORK", "off")
    for key in KEYS:
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setenv("ACME_API_KEY", MADE_UP_KEY)


@pytest.fixture
def project(tmp_path: Path) -> Path:
    root = tmp_path / "project"
    for name, text in {
        "fx.yaml": PROJECT,
        "routes.yaml": ROUTES,
        "workflows/colours.yaml": WORKFLOW,
        "adapters/acme.py": ADAPTER,
        "schemas/colour.json": '{"type": "object", "properties": {"name": {"type": "string"}}}',
    }.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, "utf-8")
    return root


def test_a_live_run_is_served_by_the_projects_adapters(project: Path) -> None:
    # Planning loads both adapters on the planning host; a plan needs no adapter host.
    planned = fx.plan("colours", cwd=project, max_usd=1)
    assert planned.ok, planned.problems

    result = fx.run("colours", cwd=project, live=True, max_usd=1, run_dir="runs/one")
    assert not result.ok and result.failed == ["refused#1"]
    # The refusal's reason reaches the record redacted.
    assert result.failures["refused#1"] == Failure(
        id="refused#1",
        path="refused",
        message="structured.generate on text-a@acme was refused: no colours for [redacted]",
        code="capability_refused",
        facts={},
        skipped=False,
    )
    # The request adapter answered after a fetch the engine refused (the network is off); its
    # host had its own key. The long job was submitted and collected by the other adapter,
    # whose host had no key of another declaration. Both hosts ran in the project's root, and
    # each route's contract was completed with its adapter's name and version.
    name = json.loads(result.steps["name"].outputs["json"].read_bytes())
    assert name == {
        "name": "teal",
        "fetched": ["refused", "the network is off (GRIDA_FX_NETWORK=off)"],
        "route": {"adapter": "adapters/acme.py#acme", "adapter_behavior": 1},
        "key": True,
        "cwd": "project",
    }
    slowly = json.loads(result.steps["slowly"].outputs["json"].read_bytes())
    assert slowly["name"] == "slate" and len(slowly["job"]) == 8
    assert slowly["route"] == {"adapter": "adapters/acme.py#acme_jobs", "adapter_behavior": 3}
    assert (slowly["key"], slowly["cwd"]) == (False, "project")
    # Each answer is billed at the cost its adapter reported; the refusal at nothing.
    assert result.cost == pytest.approx(0.03)
    assert {path: step.facts.get("cost_usd") for path, step in result.steps.items()} == {
        "name": pytest.approx(0.01),
        "slowly": pytest.approx(0.02),
    }
    text = json.dumps(result.events)
    assert MADE_UP_KEY not in text
    records = project / ".fx" / "cache"
    for record in records.rglob("*.json"):
        assert MADE_UP_KEY not in record.read_text("utf-8"), record


def test_an_adapter_that_cannot_be_loaded_refuses_the_plan(project: Path) -> None:
    (project / "adapters" / "acme.py").write_text(
        "from grida.fx import adapter\n\nacme = adapter(requests=['structured.generate'])\n",
        "utf-8",
    )
    with pytest.raises(fx.FxError) as refused:
        fx.plan("colours", cwd=project, max_usd=1)
    assert (
        "fx.yaml: providers.acme.adapter: adapters/acme.py failed to import: ValueError: "
        "adapter(): requests need send"
    ) in str(refused.value)
