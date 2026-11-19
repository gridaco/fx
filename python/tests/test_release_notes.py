"""The announcement uses the tagged entry and each registry's version spelling."""

import importlib
from pathlib import Path

import pytest


@pytest.fixture
def render(monkeypatch):
    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parents[2] / "tools"))
    return importlib.import_module("release_notes").release_notes


def test_announces_only_the_selected_version(render):
    changelog = """## [Unreleased]

- Future change.

## [1.0.0] - 2026-11-20

### Added

- Stable change.

## [1.0.0-rc.1] - 2026-11-19

### Fixed

- Preview change.

[Unreleased]: https://example.com/compare
"""
    assert render(changelog, "1.0.0") == (
        "### Added\n\n- Stable change.\n\n"
        "[npm](https://www.npmjs.com/package/@grida/fx/v/1.0.0) · "
        "[PyPI](https://pypi.org/project/grida/1.0.0/)"
    )
    assert render(changelog, "1.0.0-rc.1") == (
        "### Fixed\n\n- Preview change.\n\n"
        "[npm](https://www.npmjs.com/package/@grida/fx/v/1.0.0-rc.1) · "
        "[PyPI](https://pypi.org/project/grida/1.0.0rc1/)"
    )


@pytest.mark.parametrize(
    "changelog",
    [
        "## [0.11.1] - 2026-11-20\n\n- Another version.",
        "## [0.11.0] - 2026-11-20\n\n## [0.10.0] - 2026-11-19\n\n- Older entry.",
        "## [0.11.0] - 2026-11-20\n\n- First.\n\n## [0.11.0] - 2026-11-20\n\n- Second.",
    ],
)
def test_refuses_missing_empty_or_duplicate_notes(render, changelog):
    with pytest.raises(ValueError, match="CHANGELOG.md"):
        render(changelog, "0.11.0")
