"""Three interface sheets with geometry gates and independent review."""

import hashlib
from pathlib import Path

from ui_example.models import UiArt
from ui_example.nodes import document_roles
from ui_example.steps import add_ui_sheet_steps

from grida.fx import Workflow

ROOT = Path(__file__).resolve().parent


def _build(spec: Path) -> Workflow:
    ui = UiArt.model_validate_json(spec.read_text())
    for reference in ui.references:
        if (
            hashlib.sha256((ROOT / reference.source).read_bytes()).hexdigest()
            != reference.source_sha256
        ):
            raise ValueError("the UI reference differs from its authored digest")
    style = (ROOT / "style.txt").read_text().strip()
    wf = Workflow(
        "showcase-meadow-ui",
        title="Meadow interface kit",
        description=(
            "One panel frame, four button states, and sixteen icons with technical validation "
            "and independent model review evidence."
        ),
        budget={"max_usd": 15},
    )
    files = {}
    outputs = {}
    for role in document_roles(ui):
        steps = add_ui_sheet_steps(
            wf.group(role.role, title=role.role.replace("_", " ").title()),
            nodes="./nodes.py",
            ui=ui,
            role=role,
            style_prompt=lambda task: style + "\n\n" + task,
            reference_path=lambda path: "./" + path,
            review=True,
        )
        published = steps["publish"]
        for suffix, value in (
            ("png", published.outputs.image),
            ("validation.json", published.outputs.validation),
            ("evidence.png", published.outputs.evidence),
            ("review.json", steps["review"].outputs.json),
        ):
            files[f"ui/{role.role}.{suffix}"] = value
        outputs[role.role] = published.outputs.image
        outputs[role.role + "_validation"] = published.outputs.validation
        outputs[role.role + "_evidence"] = published.outputs.evidence
        outputs[role.role + "_review"] = steps["review"].outputs.json
    package = wf.step(
        "package",
        title="UI handoff files",
        uses="fx/package@1",
        with_={
            "files": files,
            "manifest": {
                "kind": "showcase-ui-package-v1",
                "roles": [role.role for role in document_roles(ui)],
                "style": style,
                "review_is_evidence": True,
                "publication_status": "unreviewed",
            },
        },
    )
    wf.outputs(**outputs, package=package.outputs.files)
    return wf


def build() -> Workflow:
    return _build(ROOT / "ui.json")


def build_fixture() -> Workflow:
    """The same graph with a synthesized offline reference; run make_fixture.py first."""
    return _build(ROOT / ".fixture" / "ui.json")
