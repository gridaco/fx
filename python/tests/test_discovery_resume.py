"""Resume recorded discovered models through the SDK without repeating metadata discovery.

The only server is loopback metadata; runs are dry and never send a generation request.
"""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Any

import pytest

import grida.fx as fx
from grida.fx import _api

ENGINE = _api._packaged_binary()
pytestmark = pytest.mark.skipif(
    os.name != "posix" or not ENGINE.is_file(), reason="needs the packaged engine on POSIX"
)


@pytest.mark.parametrize("named", [False, True])
def test_sdk_resumes_discovered_model_after_metadata_cache_removal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, named: bool
) -> None:
    requests: list[str] = []

    class Metadata(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            requests.append(self.path)
            body = json.dumps(
                {
                    "id": "acme/img-v2",
                    "endpoints": [
                        {
                            "supported_parameters": {},
                            "pricing": [
                                {"billable": "output_image", "unit": "image", "cost_usd": "0.05"}
                            ],
                        }
                    ],
                }
            ).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.send_header("connection", "close")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: Any) -> None:
            pass

        def do_POST(self) -> None:
            requests.append(f"POST {self.path}")
            self.send_error(500, "a dry run must not generate")

    server = HTTPServer(("127.0.0.1", 0), Metadata)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        for name in (
            "GG_API_KEY",
            "OPENAI_API_KEY",
            "OPENROUTER_API_KEY",
            "FAL_KEY",
            "TRIPO_API_KEY",
            "ELEVENLABS_API_KEY",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
        ):
            monkeypatch.delenv(name, raising=False)
        monkeypatch.setenv("GRIDA_FX_BIN", str(ENGINE))
        monkeypatch.setenv("GRIDA_FX_DISABLE_DOTENV", "1")
        monkeypatch.setenv("GRIDA_FX_NETWORK", "on")
        monkeypatch.setenv("OPENROUTER_API_KEY", "synthetic-resume-key")
        monkeypatch.setenv("OPENROUTER_BASE_URL", f"http://127.0.0.1:{server.server_port}/v1")
        (tmp_path / "fx.yaml").write_text(
            "fx: project/v1\nroutes:\n  image.generate: acme/img-v2\n"
            "provider_preferences:\n  image.generate: openrouter\n",
            "utf-8",
        )
        (tmp_path / "workflow.yaml").write_text(
            "fx: workflow/v1\nid: discovery\ntitle: Recorded model\nsteps:\n  draw:\n"
            "    uses: fx/image.generate@1\n    with: {prompt: a lantern}\n",
            "utf-8",
        )
        first_options = {"name": "offline"} if named else {"run_dir": "runs/offline"}
        first = fx.run("workflow.yaml", cwd=tmp_path, max_usd=1, **first_options)
        assert not first.ok  # A dry run records its refused paid call, without generating anything.
        assert first.cost == 0
        snapshot = (first.run_dir / "resolved-models.json").read_bytes()
        shutil.rmtree(tmp_path / ".fx/models")
        monkeypatch.setenv("GRIDA_FX_NETWORK", "off")
        monkeypatch.delenv("OPENROUTER_API_KEY")
        resumed_options = {"resume": "offline"} if named else {"run_dir": "runs/offline"}
        if named:
            resumed = asyncio.run(
                fx.run_async("workflow.yaml", cwd=tmp_path, max_usd=1, **resumed_options)
            )
        else:
            resumed = fx.run("workflow.yaml", cwd=tmp_path, max_usd=1, **resumed_options)
        assert resumed.run_dir == first.run_dir
        assert not resumed.ok
        assert resumed.cost == 0
        assert (first.run_dir / "resolved-models.json").read_bytes() == snapshot
        assert sum(event.get("event") == "run_started" for event in resumed.events) == 2
        assert not (tmp_path / ".fx/models").exists()
        assert requests == ["/v1/images/models/acme/img-v2/endpoints"]
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)
        assert not thread.is_alive()
