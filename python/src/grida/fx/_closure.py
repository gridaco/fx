"""Source closures (``spec/protocol.md`` section 5.1, ``spec/identity.md`` section 6).

The closure of a module is the module and every project module it imports, transitively. Every
``import a.b`` in the syntax tree names ``a.b``; every ``from m import x`` names ``m`` and ``m.x``;
relative imports resolve against the importing file's package; imports count wherever they appear,
run or not. A dotted name is in the closure when it resolves to ``<base>/a/b.py`` or
``<base>/a/b/__init__.py``, the base being the project root first, then, for a name whose first
part is a declared source package, that package's parent folder. A file counts only when every part
of its path below the base matches its directory entry exactly, case included (as Python's own
import does), so the closure is the same on file systems that ignore case and on those that do
not: ``from Lib import Helper`` names nothing when the file is ``lib/helper.py``. Labels are POSIX
paths relative to the nearer base. FX decisions: a relative import whose package is the project
root itself resolves against the root (no crash); a file that does not parse is an error naming it.

A relative import that climbs above every base (out of the project and its source packages) names
no project module, so it adds nothing. The project root shadows everything, standard library
modules included: a root ``json.py`` is in the closure of every module that imports ``json``.
"""

from __future__ import annotations

import ast
import importlib.util
import os
from pathlib import Path


class ClosureError(ValueError):
    """A file of a closure that cannot be read or parsed, or that lies outside every base."""


def source_closure(module: Path, root: Path, sources: list[str]) -> list[tuple[str, Path]]:
    """``[(label, absolute path)]`` sorted by label, the module included."""
    walk = _Walk(root, sources)
    seen: dict[Path, str] = {}
    frontier = [module.resolve()]
    while frontier:
        current = frontier.pop()
        if current in seen:
            continue
        label = walk.label(current)
        seen[current] = label
        for dotted in walk.imported_names(current, label):
            found = walk.project_file(dotted)
            if found is not None and found not in seen:
                frontier.append(found)
    return sorted(((label, path) for path, label in seen.items()), key=lambda entry: entry[0])


def label_of(path: Path, root: Path, sources: list[str]) -> str:
    """A file's label: relative to the nearer of the root and its source package's parent."""
    return _Walk(root, sources).label(path.resolve())


class _Walk:
    """One closure computation: the bases, and the source packages' folders found once."""

    def __init__(self, root: Path, sources: list[str]) -> None:
        self.root = root.resolve()
        self.sources = list(sources)
        self._folders: dict[str, Path | None] = {}
        #: Each folder's entry names, listed once.
        self._entries: dict[Path, frozenset[str]] = {}

    def label(self, path: Path) -> str:
        base = self._base_of(path)
        if base is None:
            raise ClosureError(f"{path.name} is outside the project and its sources")
        return path.relative_to(base).as_posix()

    def _base_of(self, path: Path) -> Path | None:
        bases = [self.root] if path.is_relative_to(self.root) else []
        for name in self.sources:
            folder = self._folder(name)
            if folder is not None and path.is_relative_to(folder):
                bases.append(folder.parent)
        if not bases:
            return None
        return max(bases, key=lambda base: len(base.parts))

    def _folder(self, name: str) -> Path | None:
        """A declared source package's folder, or ``None`` when it is not an importable
        package (a single-file module never counts)."""
        if name not in self._folders:
            folder: Path | None = None
            try:
                found = importlib.util.find_spec(name)
            except (ImportError, ValueError):
                found = None
            locations = None if found is None else found.submodule_search_locations
            if locations:
                folder = Path(next(iter(locations))).resolve()
            self._folders[name] = folder
        return self._folders[name]

    def imported_names(self, path: Path, label: str) -> list[str]:
        """Every dotted name the file's imports name, in syntax tree order."""
        try:
            source = path.read_bytes()
        except OSError as error:
            raise ClosureError(f"{label} cannot be read: {error.strerror or error}") from None
        try:
            tree = ast.parse(source, filename=label)
        except (SyntaxError, ValueError) as error:
            raise ClosureError(f"{label} does not parse: {type(error).__name__}: {error}") from None
        names: list[str] = []
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names.extend(alias.name for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                base = node.module or ""
                if node.level:
                    package = self._package_name(path, node.level)
                    if package is None:
                        continue
                    base = ".".join(part for part in (package, base) if part)
                if base:
                    names.append(base)
                names.extend(
                    f"{base}.{alias.name}" if base else alias.name
                    for alias in node.names
                    if alias.name != "*"
                )
        return names

    def _package_name(self, path: Path, level: int) -> str | None:
        """The dotted package a relative import of ``level`` dots resolves against: ``""`` for
        a base itself, ``None`` when it climbs out of every base."""
        package = path.parent
        for _ in range(level - 1):
            package = package.parent
        base = self._base_of(package)
        if base is None:
            return None
        return ".".join(package.relative_to(base).parts)

    def project_file(self, dotted: str) -> Path | None:
        parts = dotted.split(".")
        if not all(parts):
            return None
        bases = [self.root]
        if parts[0] in self.sources:
            folder = self._folder(parts[0])
            if folder is not None:
                bases.append(folder.parent)
        for base in bases:
            for names in ((*parts[:-1], parts[-1] + ".py"), (*parts, "__init__.py")):
                candidate = base.joinpath(*names)
                if candidate.is_file() and self._named_exactly(base, names):
                    return candidate.resolve()
        return None

    def _named_exactly(self, base: Path, names: tuple[str, ...]) -> bool:
        """Whether each of ``names`` below ``base`` is an entry of its folder, case included."""
        folder = base
        for name in names:
            if name not in self._listing(folder):
                return False
            folder = folder / name
        return True

    def _listing(self, folder: Path) -> frozenset[str]:
        if folder not in self._entries:
            try:
                self._entries[folder] = frozenset(os.listdir(folder))
            except OSError:
                self._entries[folder] = frozenset()
        return self._entries[folder]
