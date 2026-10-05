"""Source closures: which project files a module's identity covers, and their labels."""

from __future__ import annotations

import importlib
import shutil
import sys
from collections.abc import Iterator
from pathlib import Path

import pytest

from grida.fx._closure import ClosureError, label_of, source_closure

REPO = Path(__file__).resolve().parents[2]
LOCAL_IDENTITY = REPO / "conformance" / "local-identity" / "in"


def _write(root: Path, files: dict[str, str]) -> None:
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, "utf-8")


def _labels(module: Path, root: Path, sources: list[str] | None = None) -> list[str]:
    closure = source_closure(module, root, sources or [])
    for label, path in closure:
        assert path.is_absolute() and path.is_file()
        assert label_of(path, root, sources or []) == label
    return [label for label, _ in closure]


@pytest.fixture
def project(tmp_path: Path) -> Path:
    root = tmp_path / "project"
    root.mkdir()
    return root


@pytest.fixture
def site(tmp_path: Path) -> Iterator[Path]:
    """A folder on sys.path standing for installed packages."""
    folder = tmp_path / "site"
    folder.mkdir()
    sys.path.insert(0, str(folder))
    importlib.invalidate_caches()
    try:
        yield folder
    finally:
        sys.path.remove(str(folder))
        for name in [key for key in sys.modules if key.split(".")[0] == "acme_lib"]:
            del sys.modules[name]


def test_the_local_identity_fixture(tmp_path: Path) -> None:
    root = tmp_path / "local-identity"
    shutil.copytree(LOCAL_IDENTITY, root)
    closure = source_closure(root / "nodes" / "n.py", root, [])
    assert closure == [
        ("nodes/helper.py", (root / "nodes" / "helper.py").resolve()),
        ("nodes/n.py", (root / "nodes" / "n.py").resolve()),
    ]
    # The helper imports nothing from the project.
    assert _labels(root / "nodes" / "helper.py", root) == ["nodes/helper.py"]


def test_the_closure_is_transitive_and_counts_every_import(project: Path) -> None:
    _write(
        project,
        {
            "nodes/main.py": (
                "import nodes.a\n"
                "def later():\n"
                "    if False:\n"
                "        from nodes import b\n"
                "try:\n"
                "    import nodes.c as c\n"
                "except ImportError:\n"
                "    pass\n"
            ),
            "nodes/a.py": "from nodes.deep import thing\n",
            "nodes/b.py": "",
            "nodes/c.py": "import nodes.main\n",
            "nodes/deep/__init__.py": "",
            "nodes/deep/thing.py": "x = 1\n",
            "nodes/unused.py": "",
        },
    )
    assert _labels(project / "nodes" / "main.py", project) == [
        "nodes/a.py",
        "nodes/b.py",
        "nodes/c.py",
        "nodes/deep/__init__.py",
        "nodes/deep/thing.py",
        "nodes/main.py",
    ]


def test_import_a_b_names_only_a_b(project: Path) -> None:
    _write(
        project,
        {
            "m.py": "import pkg.sub\n",
            "pkg/__init__.py": "",
            "pkg/sub.py": "",
        },
    )
    # The parent package's __init__ enters only when something names it.
    assert _labels(project / "m.py", project) == ["m.py", "pkg/sub.py"]
    _write(project, {"n.py": "from pkg import sub\n"})
    assert _labels(project / "n.py", project) == ["n.py", "pkg/__init__.py", "pkg/sub.py"]


def test_from_import_names_the_module_and_its_members(project: Path) -> None:
    _write(
        project,
        {
            "m.py": "from helpers import tools, CONSTANT\nfrom other import *\n",
            "helpers.py": "",
            "helpers/tools.py": "",
            "other.py": "",
        },
    )
    # `helpers` is helpers.py (a module wins over a folder of the same name); `helpers.tools`
    # is helpers/tools.py; `CONSTANT` names nothing; `*` names nothing beyond the module.
    assert _labels(project / "m.py", project) == [
        "helpers.py",
        "helpers/tools.py",
        "m.py",
        "other.py",
    ]


def test_relative_imports(project: Path) -> None:
    _write(
        project,
        {
            "nodes/__init__.py": "",
            "nodes/main.py": (
                "from . import sibling\nfrom .pkg import inner\nfrom ..lib import util\n"
            ),
            "nodes/sibling.py": "",
            "nodes/pkg/__init__.py": "",
            "nodes/pkg/inner.py": "from .. import sibling\nfrom ...lib import other\n",
            "lib/util.py": "",
            "lib/other.py": "",
        },
    )
    assert _labels(project / "nodes" / "main.py", project) == [
        "lib/other.py",
        "lib/util.py",
        "nodes/__init__.py",
        "nodes/main.py",
        "nodes/pkg/__init__.py",
        "nodes/pkg/inner.py",
        "nodes/sibling.py",
    ]


def test_a_relative_import_in_a_root_module_resolves_against_the_root(project: Path) -> None:
    _write(
        project,
        {
            "m.py": "from . import helper\nfrom .lib import util\n",
            "helper.py": "",
            "lib/util.py": "",
        },
    )
    assert _labels(project / "m.py", project) == ["helper.py", "lib/util.py", "m.py"]


def test_a_relative_import_above_the_root_names_nothing(project: Path) -> None:
    (project.parent / "outside.py").write_text("", "utf-8")
    _write(project, {"m.py": "from .. import outside\n", "nodes/n.py": "from ... import outside\n"})
    assert _labels(project / "m.py", project) == ["m.py"]
    assert _labels(project / "nodes" / "n.py", project) == ["nodes/n.py"]


def test_the_root_shadows_a_standard_library_name(project: Path) -> None:
    _write(
        project,
        {
            "nodes/n.py": "import json\nimport os.path\nfrom collections import abc\n",
            "json.py": "",
            "collections/__init__.py": "",
        },
    )
    assert _labels(project / "nodes" / "n.py", project) == [
        "collections/__init__.py",
        "json.py",
        "nodes/n.py",
    ]


def test_a_source_package(project: Path, site: Path) -> None:
    _write(
        site,
        {
            "acme_lib/__init__.py": "from .colors import palette\n",
            "acme_lib/colors.py": "import json\n",
            "acme_lib/shapes/__init__.py": "",
            "acme_lib/shapes/circle.py": "from .. import colors\nfrom ..unused import x\n",
        },
    )
    _write(
        project,
        {
            "nodes/n.py": "import acme_lib.shapes.circle\nfrom acme_lib import colors\n",
            # The root shadows the package's own `import json`.
            "json.py": "",
        },
    )
    importlib.invalidate_caches()
    closure = source_closure(project / "nodes" / "n.py", project, ["acme_lib"])
    assert [label for label, _ in closure] == [
        "acme_lib/__init__.py",
        "acme_lib/colors.py",
        "acme_lib/shapes/circle.py",
        "json.py",
        "nodes/n.py",
    ]
    paths = dict(closure)
    assert paths["acme_lib/colors.py"] == (site / "acme_lib" / "colors.py").resolve()
    assert paths["json.py"] == (project / "json.py").resolve()
    # Without the declaration the package is not project source.
    assert _labels(project / "nodes" / "n.py", project) == ["nodes/n.py"]


def test_a_source_package_inside_the_project_is_labelled_from_the_root(project: Path) -> None:
    _write(
        project,
        {
            "nodes/n.py": "from vendored import tool\n",
            "vendored/__init__.py": "",
            "vendored/tool.py": "",
        },
    )
    sys.path.insert(0, str(project))
    importlib.invalidate_caches()
    try:
        labels = _labels(project / "nodes" / "n.py", project, ["vendored"])
    finally:
        sys.path.remove(str(project))
        sys.modules.pop("vendored", None)
    assert labels == ["nodes/n.py", "vendored/__init__.py", "vendored/tool.py"]


def test_a_declared_source_that_is_not_a_package_counts_for_nothing(project: Path) -> None:
    # `abc` is a single-file module, never a package.
    _write(project, {"m.py": "import abc\nimport no_such_package_anywhere\n"})
    assert _labels(project / "m.py", project, ["abc", "no_such_package_anywhere"]) == ["m.py"]


def test_a_file_that_does_not_parse_is_named(project: Path) -> None:
    _write(
        project,
        {
            "m.py": "def later():\n    import broken\n",
            "broken.py": "def (:\n",
        },
    )
    with pytest.raises(ClosureError) as refused:
        source_closure(project / "m.py", project, [])
    message = str(refused.value)
    assert message.startswith("broken.py does not parse: SyntaxError: ")
    assert str(project) not in message


def test_source_encodings_follow_the_coding_declaration(project: Path) -> None:
    (project / "m.py").write_bytes(b"# -*- coding: latin-1 -*-\nname = '\xe9'\nimport helper\n")
    (project / "helper.py").write_bytes(b"\xef\xbb\xbfx = 1\n")
    assert _labels(project / "m.py", project) == ["helper.py", "m.py"]


def test_label_of_a_file_outside_every_base(project: Path, tmp_path: Path) -> None:
    outside = tmp_path / "elsewhere.py"
    outside.write_text("", "utf-8")
    with pytest.raises(ClosureError, match="elsewhere.py is outside the project and its sources"):
        label_of(outside, project, [])


def test_a_name_counts_only_in_its_exact_case(project: Path, site: Path) -> None:
    _write(
        project,
        {
            "nodes/n.py": (
                "def later():\n"
                "    from Lib import Helper\n"
                "    import lib.helper\n"
                "    import NODES.other\n"
                "    from nodes import Other\n"
                "    from acme_lib import Colors\n"
            ),
            "lib/helper.py": "",
            "nodes/other.py": "",
            "Pkg/__init__.py": "",
            "pkg_user.py": "import pkg\nimport Pkg\n",
        },
    )
    _write(site, {"acme_lib/__init__.py": "", "acme_lib/colors.py": ""})
    importlib.invalidate_caches()
    # On a file system that ignores case the miscased names find the same files: they still
    # name nothing, as on one that does not, and as Python's own import refuses them.
    assert _labels(project / "nodes" / "n.py", project, ["acme_lib"]) == [
        "acme_lib/__init__.py",
        "lib/helper.py",
        "nodes/n.py",
    ]
    # A package folder named in its exact case counts through its `__init__.py`.
    assert _labels(project / "pkg_user.py", project) == ["Pkg/__init__.py", "pkg_user.py"]
