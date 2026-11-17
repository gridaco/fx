"""Declared custom-node defaults and data views are available without running bodies."""

import os
import shutil
from pathlib import Path

import pytest

from grida.fx import _api, plan

ENGINE = _api._packaged_binary()
ROOT = Path(__file__).resolve().parents[2]


@pytest.mark.skipif(os.name != "posix" or not ENGINE.is_file(), reason="needs built local engine")
def test_custom_node_defaults_and_input_values_before_execution(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    shutil.copytree(ROOT / "conformance/values/in", project)
    module = project / "nodes/echo.py"
    module.write_text(
        module.read_text().replace(
            "return {\"text\": ctx.out.text(f\"{ctx.params['p1']}; {ctx.params['p2']}\")}",
            'raise RuntimeError("body must not run while planning")',
        )
    )
    monkeypatch.setenv("GRIDA_FX_BIN", str(ENGINE))
    monkeypatch.setenv("GRIDA_FX_NETWORK", "off")
    monkeypatch.setenv("GRIDA_FX_DISABLE_DOTENV", "1")
    result = plan("case", cwd=project)
    assert result.ok
    assert all(
        instance["with"]["p2"] == "schema default" for instance in result.document["instances"]
    )
    values = result.document["value_views"]
    assert any(value["kind"] == "input" and value["name"] == "prompt" for value in values)
    assert any(value["kind"] == "output" and value["state"] == "pending" for value in values)
    assert not any(value["name"] == "unused" for value in values), "unused let stays unevaluated"
