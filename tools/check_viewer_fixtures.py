"""Generate and check the canonical viewer development fixtures, entirely offline.

    uv run --project python python tools/check_viewer_fixtures.py
    uv run --project python python tools/check_viewer_fixtures.py --output .fx/viewer-fixtures/demo

Without --output, all generated data is temporary. With it, keep real plans, runs,
read-only viewer responses and a report for manual UI development. The destination
must be empty. No provider keys, dotenv, live mode or paid calls are used.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
import wave
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

from PIL import Image

import grida.fx as fx

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "fixtures" / "viewer"
PROVIDER_KEYS = (
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "FAL_KEY",
    "TRIPO_API_KEY",
    "ELEVENLABS_API_KEY",
)
WORKFLOWS = {
    "topology": "viewer-topology",
    "failure": "viewer-failure",
    "running": "viewer-running",
    "takes": "viewer-takes",
    "ports": "viewer-ports",
    "dynamic-failure": "viewer-dynamic-failure",
    "nesting": "viewer-nesting",
    "nesting-failure": "viewer-nesting-failure",
}


def expect(condition: object, message: str) -> None:
    if not condition:
        raise ValueError(message)


def offline() -> None:
    for key in PROVIDER_KEYS:
        os.environ.pop(key, None)
    os.environ["GRIDA_FX_DISABLE_DOTENV"] = "1"
    os.environ["GRIDA_FX_NETWORK"] = "off"
    os.environ["GRIDA_FX_PYTHON"] = sys.executable


def write_json(path: Path, document: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def events(path: Path) -> list[dict[str, Any]]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]


def prepare_output(path: Path) -> Path:
    destination = path.absolute()
    for parent in [destination, *destination.parents]:
        expect(not parent.is_symlink(), f"the output path contains a symbolic link: {parent}")
    destination = destination.resolve(strict=False)
    source = SOURCE.resolve()
    expect(
        not destination.is_relative_to(source) and not source.is_relative_to(destination),
        "the output directory must not overlap the fixture source",
    )
    expect(not destination.exists() or destination.is_dir(), "the output must be a directory")
    expect(
        not destination.exists() or not any(destination.iterdir()),
        f"the output directory is not empty: {destination}; choose a new directory",
    )
    destination.mkdir(parents=True, exist_ok=True)
    return destination


@contextmanager
def viewer(run: Path, logs: Path) -> Iterator[tuple[str, urllib.request.OpenerDirector]]:
    """Use the public CLI and HTTP contract, without importing viewer internals."""
    logs.parent.mkdir(parents=True, exist_ok=True)
    with logs.open("w", encoding="utf-8") as stream:
        process = subprocess.Popen(
            [sys.executable, "-m", "grida.fx", "view", "--run", str(run), "--no-open"],
            cwd=run.parent,
            env=os.environ.copy(),
            stdout=stream,
            stderr=subprocess.STDOUT,
        )
    try:
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            logged = logs.read_text(encoding="utf-8")
            address = re.search(r"^http://127\.0\.0\.1:\d+/\s*$", logged, re.MULTILINE)
            if address:
                yield (
                    address.group().strip(),
                    urllib.request.build_opener(urllib.request.ProxyHandler({})),
                )
                return
            expect(process.poll() is None, f"viewer exited before listening: {logged.strip()}")
            time.sleep(0.025)
        raise ValueError("viewer did not report a loopback URL within 15 seconds")
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def project_run(run: Path, output: Path, name: str) -> dict[str, Any]:
    with viewer(run, output / "logs" / f"{name}.log") as (base, client):
        with client.open(base + "api/view", timeout=5) as response:
            document = json.load(response)
        expect(document.get("kind") == "fx-viewer-run-v1", f"{name}: no run response")
        for artifact in document["artifacts"]:
            if artifact["available"]:
                expect(
                    artifact["url"] == f"/api/artifacts/{artifact['digest']}",
                    f"{name}: a file has a noncanonical artifact URL",
                )
                with client.open(base.rstrip("/") + artifact["url"], timeout=5) as response:
                    content = response.read()
                    expect(len(content) == artifact["size"], f"{name}: wrong file size")
                    expect(
                        hashlib.sha256(content).hexdigest() == artifact["digest"],
                        f"{name}: served bytes differ from their recorded digest",
                    )
        write_json(output / "views" / f"{name}.json", document)
        return document


def running_prefix(completed: Path, target: Path, path: str) -> None:
    """Keep a genuine event prefix, not a fabricated or still-running workflow."""
    recorded = events(completed / "events.jsonl")
    boundary = next(
        (
            index
            for index, event in enumerate(recorded)
            if event.get("event") == "node_started" and event.get("path") == path
        ),
        None,
    )
    expect(boundary is not None, f"the running fixture did not start its {path} step")
    shutil.copytree(completed, target)
    lines = (completed / "events.jsonl").read_bytes().splitlines(keepends=True)
    (target / "events.jsonl").write_bytes(b"".join(lines[: boundary + 1]))


def missing_artifact(completed: Path, target: Path, digest: str) -> None:
    """Damage only a copy of a real run to exercise unavailable-artifact presentation."""
    shutil.copytree(completed, target)
    removed = 0
    for path in target.rglob("*"):
        if path.is_file() and hashlib.sha256(path.read_bytes()).hexdigest() == digest:
            path.unlink()
            removed += 1
    expect(removed > 0, "the missing-artifact fixture removed no placed image bytes")


def legacy_graph(document: dict[str, Any]) -> dict[str, Any]:
    """Remove optional port metadata, preserving the preexisting graph and authored values."""
    old = copy.deepcopy(document)
    old.pop("scopes", None)
    for definition in old.get("types", {}).values():
        definition.pop("ports", None)
    for instance in old.get("instances", []):
        instance.pop("bindings", None)
        instance.pop("interface_bindings", None)
    return old


def legacy_run(completed: Path, target: Path) -> None:
    """Copy a genuine run without the new display metadata to check old-record fallback."""
    shutil.copytree(completed, target)
    plan = json.loads((target / "plan.json").read_text(encoding="utf-8"))
    write_json(target / "plan.json", legacy_graph(plan))
    recorded = [
        event for event in events(target / "events.jsonl") if event.get("event") != "scopes_updated"
    ]
    for event in recorded:
        if event.get("event") in {"node_started", "node_failed", "node_skipped"}:
            for field in ("ports", "bindings", "interface_bindings", "needs", "judges"):
                event.pop(field, None)
    (target / "events.jsonl").write_text(
        "".join(json.dumps(event, sort_keys=True) + "\n" for event in recorded), encoding="utf-8"
    )


def checked_run(
    home: Path,
    target: str,
    folder: Path,
    *,
    succeeded: bool = True,
    inputs: dict[str, Any] | None = None,
) -> fx.RunResult:
    result = fx.run(target, cwd=home, run_dir=folder, max_usd=0, inputs=inputs)
    expect(result.ok == succeeded, f"{target}: unexpected success/failure: {result.failures}")
    expect(result.cost == 0, f"{target}: a fixture recorded nonzero spend")
    expect(
        not any(event.get("event") == "call" for event in result.events),
        f"{target}: a fixture attempted a paid call",
    )
    return result


def generate(output: Path) -> dict[str, Any]:
    home = output / "project"
    shutil.copytree(
        SOURCE,
        home,
        ignore=shutil.ignore_patterns(".env*", ".fx", "runs", "__pycache__", "*.pyc"),
    )
    expected = json.loads((SOURCE / "expected.json").read_text(encoding="utf-8"))
    write_json(output / "expected.json", expected)
    for name, workflow in WORKFLOWS.items():
        planned = fx.plan(workflow, cwd=home, max_usd=0)
        expect(planned.ok, f"{workflow}: planning problems: {planned.problems}")
        expect(planned.estimate() == (0.0, 0.0), f"{workflow}: nonzero plan estimate")
        write_json(output / "plans" / f"{name}.json", planned.document)
        if name in {"ports", "nesting"}:
            write_json(output / "plans" / f"{name}-legacy.json", legacy_graph(planned.document))

    cold = checked_run(home, WORKFLOWS["topology"], output / "runs" / "cold")
    cached = checked_run(home, WORKFLOWS["topology"], output / "runs" / "cached")
    expect(cold.steps, "the topology fixture completed no steps")
    expect(
        sorted(path for path, step in cached.steps.items() if step.cache != "hit")
        == expected["warm_cache_miss_paths"],
        "the warm topology run changed which steps reuse the cache",
    )
    expect(cold.outputs.keys() == cached.outputs.keys(), "the warm run changed its output ports")
    checked_run(home, WORKFLOWS["failure"], output / "runs" / "failure", succeeded=False)
    dynamic_failure = checked_run(
        home, WORKFLOWS["dynamic-failure"], output / "runs" / "dynamic-failure", succeeded=False
    )
    takes = checked_run(home, WORKFLOWS["takes"], output / "runs" / "takes")
    ports = checked_run(home, WORKFLOWS["ports"], output / "runs" / "ports")
    nesting = checked_run(home, WORKFLOWS["nesting"], output / "runs" / "nesting")
    nesting_failure = checked_run(
        home, WORKFLOWS["nesting-failure"], output / "runs" / "nesting-failure", succeeded=False
    )
    legacy_run(output / "runs" / "ports", output / "runs" / "ports-legacy")
    legacy_run(output / "runs" / "nesting", output / "runs" / "nesting-legacy")
    alternative = checked_run(
        home, WORKFLOWS["topology"], output / "runs" / "alternative", inputs={"enabled": False}
    )
    checked_run(home, WORKFLOWS["running"], output / "runs" / "running-completed")
    running_prefix(
        output / "runs" / "running-completed",
        output / "runs" / "running",
        expected["running_step"],
    )
    missing_artifact(
        output / "runs" / "cold", output / "runs" / "missing", cold.outputs["sheet"].digest
    )

    documents = {
        name: project_run(output / "runs" / name, output, name)
        for name in [
            "cold",
            "cached",
            "alternative",
            "failure",
            "dynamic-failure",
            "nesting",
            "nesting-failure",
            "nesting-legacy",
            "takes",
            "ports",
            "ports-legacy",
            "running",
            "missing",
        ]
    }
    validate_scenarios(output, documents, cold, cached, alternative, takes)
    validate_ports(ports)
    validate_dynamic_failure(dynamic_failure, documents["dynamic-failure"])
    validate_nesting(nesting, nesting_failure)
    subprocess.run(
        [
            "bun",
            "--no-env-file",
            "run",
            str(ROOT / "tools" / "check_viewer_fixtures.ts"),
            str(output),
        ],
        cwd=ROOT,
        check=True,
    )
    report = {
        "kind": "fx-viewer-fixture-report-v1",
        "provider_calls": 0,
        "charged_usd": 0,
        "running_is_recorded_prefix": True,
        "legacy_is_metadata_omission": True,
        "views": {
            name: {
                "state": document["state"],
                "nodes": len(document["nodes"]),
                "available_artifacts": sum(item["available"] for item in document["artifacts"]),
            }
            for name, document in documents.items()
        },
    }
    write_json(output / "report.json", report)
    return report


def validate_nesting(result: fx.RunResult, failure: fx.RunResult) -> None:
    """Imported scopes preserve keyed outputs, alias identity and real descendant failures."""
    expect(list(result.outputs["previews"]) == ["warm", "cool"], "imported workflow keys changed")
    expect(list(result.outputs["accents"]) == ["warm", "cool"], "inline group outputs lost keys")
    expect(
        result.outputs["previews"]["warm"].digest != result.outputs["previews"]["cool"].digest,
        "the two imported instances did not keep distinct image results",
    )
    expect(
        json.loads(result.outputs["same_aliases"].read_bytes())
        == {"same_bytes": True, "same_digest": True},
        "the distinct public aliases did not preserve their shared internal output",
    )
    with Image.open(result.outputs["sheet"].path) as image:
        expect(image.size == (120, 52), "the imported thumbnails did not join at the expected size")
    expect(
        set(failure.failed)
        == {"broken_wrapper.inner.fail#1", "broken_wrapper.inner.blocked#1", "dependent#1"},
        "the imported failure did not block its inner and outer consumers",
    )
    expect(set(failure.steps) == {"seed", "independent"}, "the independent branch did not finish")


def validate_dynamic_failure(result: fx.RunResult, document: dict[str, Any]) -> None:
    """A child absent from the initial plan can fail input resolution without starting."""
    failed = "branch['broken'].fail#1"
    blocked = "branch['broken'].blocked#1"
    expect(set(result.failed) == {failed, blocked}, "the dynamic failure IDs changed")
    started = {event["id"] for event in result.events if event.get("event") == "node_started"}
    expect(failed in started, "the intentional dynamic failure never started")
    expect(blocked not in started, "the blocked dynamic consumer falsely recorded a start")
    nodes = {node["id"]: node for node in document["nodes"]}
    expect(
        nodes[blocked]["uses"] == "./nodes/dynamic_failure.py#blocked",
        "the terminal-only dynamic child lost its node type",
    )
    expect(
        nodes[blocked]["ports"]
        == {"inputs": {"report": "json"}, "outputs": {"report": "json"}, "params": {}},
        "the terminal-only dynamic child lost its declared ports",
    )
    expect(
        nodes[blocked]["bindings"]
        == [
            {
                "source": failed,
                "source_port": "report",
                "target_port": "report",
                "source_kind": "output",
            }
        ],
        "the terminal-only dynamic child lost its exact failed-input binding",
    )


def validate_ports(result: fx.RunResult) -> None:
    """Distinct declared outputs remain distinct even when their file digests are identical."""
    split = result.steps["split"].outputs
    expect(split["color"].digest == split["copy"].digest, "the duplicate-content ports differ")
    expect(split["color"].digest != split["mask"].digest, "the image and alpha mask are identical")
    expect(
        json.loads(result.outputs["same"].read_bytes())
        == {"same_bytes": True, "same_digest": True},
        "the two named inputs did not receive their equal-content outputs",
    )
    expect(
        json.loads(result.outputs["settings"].read_bytes())
        == {
            "width": 161,
            "label": "Size 160 x 120",
            "pixels": 19200,
            "file_width": 160,
            "literal": "A literal setting has no upstream socket",
        },
        "computed output, node-fact, file-fact or literal settings changed",
    )
    expect(
        json.loads(result.steps["keyed"].outputs["keys"].read_bytes()) == ["seed", "warm", "cool"],
        "the keyed output collection lost its keys",
    )
    expect(
        json.loads(result.steps["wildcard"].outputs["keys"].read_bytes()) == ["amber", "teal"],
        "the wildcard image input lost its repeat keys",
    )


def validate_scenarios(
    output: Path,
    documents: dict[str, dict[str, Any]],
    cold: fx.RunResult,
    cached: fx.RunResult,
    alternative: fx.RunResult,
    takes: fx.RunResult,
) -> None:
    """Assert declared scenario behavior in addition to generic browser invariants."""
    # Specific topology and take assertions are kept alongside the source fixture.
    expected = json.loads((output / "expected.json").read_text(encoding="utf-8"))
    for case in expected["views"]:
        document = documents[Path(case["file"]).stem]
        nodes = {node["id"]: node for node in document["nodes"]}
        for identifier, state in case.get("node_states", {}).items():
            expect(
                identifier in nodes and nodes[identifier]["state"] == state,
                f"{case['file']}: {identifier} did not have expected state {state}",
            )
    for port in expected.get("stable_output_ports", []):
        expect(
            cold.outputs[port].digest == cached.outputs[port].digest,
            f"cache reuse changed the {port} artifact",
        )
    sheet = cold.outputs["sheet"].digest
    missing = next(item for item in documents["missing"]["artifacts"] if item["digest"] == sheet)
    expect(not missing["available"] and missing["url"] is None, "missing image was still served")
    expect(
        len([path for path in cold.steps if path.startswith("matrix[")]) == 6,
        "the 2 by 3 matrix did not execute all six combinations",
    )
    expect(set(cold.outputs["badges"]) == {"sun", "leaf", "stone"}, "badge keys changed")
    expect(set(cold.outputs["dynamic"]) == {"amber", "fern", "slate"}, "dynamic keys changed")
    expect(set(cold.outputs["collection"]) == {"seed", "warm", "cool"}, "collection keys changed")
    expect(len(cold.outputs["images"]) == 3, "the output image list changed")
    expect(
        len({cold.steps[path].outputs["image"].digest for path in ["seed", "warm", "cool"]}) == 3,
        "the two image branches did not produce distinct results",
    )
    with Image.open(cold.outputs["sheet"].path) as image:
        expect(image.size == (512, 136), "the join did not assemble its three images")
    with wave.open(str(cold.outputs["audio"].path), "rb") as audio:
        expect(
            (audio.getnchannels(), audio.getsampwidth(), audio.getframerate(), audio.getnframes())
            == (1, 2, 8000, 1200),
            "the synthesized audio contract changed",
        )
    expect(
        alternative.outputs["chosen"].digest == alternative.steps["skipped"].outputs["image"].digest
        and alternative.outputs["chosen"].digest != cold.outputs["chosen"].digest,
        "the reversed condition did not select its alternative image",
    )
    reviewed = [
        event
        for event in takes.events
        if event.get("event") == "node_finished" and event.get("path") == "review"
    ]
    expect(
        [(event["id"], event["facts"]["verdict"]) for event in reviewed]
        == [("review#1", "reject"), ("review#2", "accept")],
        "the local judge did not reject take one and accept take two",
    )
    rendered = {
        event["id"]: event["outputs"]["image"]["file"]["digest"]
        for event in takes.events
        if event.get("event") == "node_finished" and event.get("path") == "render"
    }
    expect(
        takes.outputs["image"].digest == rendered["render#2"] != rendered["render#1"],
        "downstream did not receive the second, accepted take",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--output", type=Path, help="keep generated evidence in a new empty directory"
    )
    args = parser.parse_args()
    offline()
    try:
        if args.output is not None:
            destination = prepare_output(args.output)
            report = generate(destination)
            print(f"check_viewer_fixtures: passed; evidence at {destination}")
        else:
            with tempfile.TemporaryDirectory(prefix="fx-viewer-fixtures-") as temporary:
                report = generate(Path(temporary))
            print("check_viewer_fixtures: passed; temporary evidence removed")
        print(f"{len(report['views'])} real run views checked, provider calls 0, recorded spend $0")
        return 0
    except (OSError, ValueError, KeyError, fx.FxError, subprocess.SubprocessError) as error:
        print(f"check_viewer_fixtures: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
