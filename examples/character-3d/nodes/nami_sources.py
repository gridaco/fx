"""Retain the exact published Nami source artifacts as portable node outputs."""

from __future__ import annotations

import hashlib

from grida.fx import Ctx, node

SOURCES = {
    "reference": (
        "image/png",
        "c5149034bfd086f23a47cf481485c4f9803f043d504431337b549e8b0d840bd6",
        ".png",
    ),
    "front": (
        "image/png",
        "e7c4bced48b895beb13264252dce17efaab7bf6b6fb689aaec4df07a4e46cec5",
        ".png",
    ),
    "back": (
        "image/png",
        "9810662ee4ce826e157360b5de725bf781dd428189f0ded15c35111cbf629aed",
        ".png",
    ),
    "assembly": (
        "model/gltf-binary",
        "013667face278a1dbbbb8be1bb3ef49274e75a413226959d7b0f88a658d511a5",
        ".glb",
    ),
    "rigged": (
        "model/gltf-binary",
        "2a77277b4ec5fcccdac89507354cf1c3400765279d8ff3d54261316185fb94a8",
        ".glb",
    ),
    "review": ("json", "2071cc1d9978e4df63ed8c30f6991f5b35e84ed1939f3ef94917ab649738c0fe", ".json"),
}


@node(
    "retain_sources",
    inputs={key: kind for key, (kind, _, _) in SOURCES.items()},
    outputs={key: kind for key, (kind, _, _) in SOURCES.items()},
    version=1,
)
def retain_sources(ctx: Ctx) -> dict:
    outputs = {}
    for name, (kind, digest, extension) in SOURCES.items():
        data = ctx.read.bytes(name)
        if hashlib.sha256(data).hexdigest() != digest:
            raise ctx.fail(f"The retained Nami {name} differs from its published exact source")
        path = ctx.out.path(name + extension)
        path.write_bytes(data)
        outputs[name] = ctx.out.file(path, kind)
    ctx.fact("provider_calls", 0)
    return outputs
