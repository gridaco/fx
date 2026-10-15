"""tools/digest.py completes a declared adapter's routes as identity.md section 7 says, from the
planning project's fx.yaml and fx.lock alone (adapters.md section 9)."""

from __future__ import annotations

import importlib.util
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

TOOL = Path(__file__).resolve().parents[2] / "tools" / "digest.py"


@pytest.fixture(scope="module")
def digest() -> ModuleType:
    spec = importlib.util.spec_from_file_location("fx_digest_tool", TOOL)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


NAME = "adapters/acme.py#acme"
DIGEST = "a" * 64


def test_a_declared_adapter_completes_its_providers_contracts(digest: ModuleType) -> None:
    adapters = {"acme": (NAME, 3)}
    assert digest.complete_contract("image.generate", "acme", {}, None, adapters) == {
        "adapter": NAME,
        "adapter_behavior": 3,
    }
    # A stated member is kept; another adapter gets no behaviour.
    assert digest.complete_contract(
        "image.generate", "acme", {"adapter_behavior": "3"}, None, adapters
    ) == {"adapter": NAME, "adapter_behavior": "3"}
    assert digest.complete_contract(
        "image.generate", "acme", {"adapter": "acme-v2"}, None, adapters
    ) == {"adapter": "acme-v2"}
    # An unknown behaviour fills the name alone; another provider keeps its contract.
    unknown = {"acme": (NAME, digest.UNKNOWN)}
    assert digest.complete_contract("image.generate", "acme", {}, None, unknown) == {
        "adapter": NAME
    }
    assert digest.complete_contract("image.generate", "other", {}, None, adapters) == {}
    # FX's own providers are completed from FX's adapters, whatever is declared.
    assert digest.complete_contract("image.generate", "gg", {}, None, {"gg": (NAME, 3)}) == {
        "adapter": "gg-image",
        "adapter_behavior": "1",
    }


def test_an_adapters_version_is_its_one_lock_entry(digest: ModuleType) -> None:
    providers: dict[str, Any] = {
        "acme": {"base_url": {"default": "https://api.acme.test"}, "adapter": NAME},
        "chat": {"base_url": {"default": "https://chat.acme.test"}, "family": "openai-compatible"},
    }
    assert digest.declared_adapters(providers, {f"{NAME}@2": DIGEST}) == {"acme": (NAME, 2)}
    for locked in ({}, {f"{NAME}@1": DIGEST, f"{NAME}@2": DIGEST}, {"other.py#x@1": DIGEST}):
        assert digest.declared_adapters(providers, locked) == {"acme": (NAME, digest.UNKNOWN)}
    assert digest.declared_adapters(None, {}) == {}


def _project(tmp_path: Path, lock: str | None) -> Path:
    (tmp_path / "fx.yaml").write_text(
        "fx: project/v1\nroute_tables: [routes.yaml]\nproviders:\n  acme:\n    base_url: "
        '{ default: "https://api.acme.test/v1" }\n'
        f"    adapter: {NAME}\n"
    )
    (tmp_path / "routes.yaml").write_text(
        "fx: routes/v1\nroutes:\n"
        "  - { capability: image.generate, route: img-a@acme, price: { cap_usd: 0.05 } }\n"
        "  - { capability: speech.generate, route: voice-a@acme, price: { cap_usd: 0.05 } }\n"
    )
    if lock is not None:
        (tmp_path / "fx.lock").write_text(lock)
    return tmp_path


def test_a_graphs_routes_are_checked_against_the_declared_adapter(
    digest: ModuleType, tmp_path: Path
) -> None:
    lock = f'fx: lock/v1\nnodes: {{}}\nadapters:\n  "{NAME}@3": "{DIGEST}"\n'
    checker = digest.GraphCheck(_project(tmp_path, lock), [])
    filled = {"adapter": NAME, "adapter_behavior": 3}
    served = digest.route_fingerprint("image.generate", "img-a@acme", filled)
    checker._check_route("draw#1", "image.generate", "img-a@acme", served, False)
    # A capability the adapter does not declare keeps its contract.
    plain = digest.route_fingerprint("speech.generate", "voice-a@acme", {})
    checker._check_route("say#1", "speech.generate", "voice-a@acme", plain, False)
    assert checker.failures == [] and checker.notes == []
    assert checker.counts["routes"] == 2
    # With a current fx.lock, another version is a mismatch.
    current = digest.GraphCheck(_project(tmp_path, lock), [], current_lock=True)
    other = digest.route_fingerprint(
        "image.generate", "img-a@acme", {"adapter": NAME, "adapter_behavior": 4}
    )
    current._check_route("draw#2", "image.generate", "img-a@acme", other, False)
    assert len(current.failures) == 1 and current.notes == []
    assert "route fingerprint for image.generate on img-a@acme differs" in current.failures[0]


@pytest.mark.parametrize(
    "behavior",
    [
        4,  # the version moved on and `grida-fx lock` has not run yet
        f"source:{DIGEST}",  # the version went: fx.lock keeps the old entry
    ],
)
def test_a_version_fx_lock_may_not_hold_is_a_note(
    digest: ModuleType, tmp_path: Path, behavior: Any
) -> None:
    lock = f'fx: lock/v1\nnodes: {{}}\nadapters:\n  "{NAME}@3": "{DIGEST}"\n'
    checker = digest.GraphCheck(_project(tmp_path, lock), [])
    filled = {"adapter": NAME, "adapter_behavior": behavior}
    printed = digest.route_fingerprint("image.generate", "img-a@acme", filled)
    checker._check_route("draw#1", "image.generate", "img-a@acme", printed, False)
    assert checker.failures == []
    assert checker.notes == [
        f"draw#1: img-a@acme for image.generate is served by {NAME}, whose version may not be "
        "fx.lock's 3 (an entry stays after its version changes or goes), so its fingerprint is "
        "not checked"
    ]
    assert checker.counts["routes"] == 0


def test_an_adapter_fx_lock_does_not_version_is_a_note(digest: ModuleType, tmp_path: Path) -> None:
    checker = digest.GraphCheck(_project(tmp_path, None), [])
    source = {"adapter": NAME, "adapter_behavior": f"source:{DIGEST}"}
    printed = digest.route_fingerprint("image.generate", "img-a@acme", source)
    checker._check_route("draw#1", "image.generate", "img-a@acme", printed, False)
    assert checker.failures == []
    assert checker.notes == [
        f"draw#1: img-a@acme for image.generate is served by {NAME}, whose version fx.lock does "
        "not give, so its fingerprint is not checked"
    ]
    assert checker.counts["routes"] == 0
