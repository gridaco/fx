"""``grida.fx.std``: the bodies of FX's standard node types that run in Python (decision 8 of
``docs/wg/overview.md``).

The engine owns every built-in's declaration (its catalog) and runs ``fx/select@1`` and the paid
built-ins itself. Nine built-ins have Python bodies, ported byte for byte from FX's predecessor so
their output files (and every identity and request downstream of them) stay the same; Pillow is
pinned for that reason. The host runs one when ``run``'s ``body`` is ``{"builtin":
"fx/<name>@1"}``: :data:`BODIES` maps ``<name>`` to its body.

| name | body |
|---|---|
| ``image.mirror_repeat`` | :func:`grida.fx.std.media.mirror_repeat` |
| ``image.check_alpha`` | :func:`grida.fx.std.media.check_alpha` (judge) |
| ``image.check_size`` | :func:`grida.fx.std.media.check_size` (judge) |
| ``image.resize`` | :func:`grida.fx.std.media.resize` |
| ``image.crop`` | :func:`grida.fx.std.media.crop` |
| ``image.pad`` | :func:`grida.fx.std.media.pad` |
| ``json.merge`` | :func:`grida.fx.std.media.json_merge` |
| ``files.copy`` | :func:`grida.fx.std.media.files_copy` |
| ``package`` | :func:`grida.fx.std.media.package` |
"""

from __future__ import annotations

import re
from collections.abc import Callable
from typing import Any

from grida.fx.std import media

#: ``fx/<name>@<major>``, as ``run``'s ``body.builtin`` names a built-in.
_BUILTIN = re.compile(r"fx/(?P<name>[^@]+)@(?P<major>[0-9]+)")
#: The major version every body here implements.
_MAJOR = "1"

#: The Python bodies by built-in name.
BODIES: dict[str, Callable[[Any], Any]] = {
    "image.mirror_repeat": media.mirror_repeat,
    "image.check_alpha": media.check_alpha,
    "image.check_size": media.check_size,
    "image.resize": media.resize,
    "image.crop": media.crop,
    "image.pad": media.pad,
    "json.merge": media.json_merge,
    "files.copy": media.files_copy,
    "package": media.package,
}


def body_of(builtin: str) -> Callable[[Any], Any] | None:
    """The body of ``fx/<name>@1``; ``None`` for any other built-in or major version."""
    match = _BUILTIN.fullmatch(builtin) if isinstance(builtin, str) else None
    if match is None or match["major"] != _MAJOR:
        return None
    return BODIES.get(match["name"])
