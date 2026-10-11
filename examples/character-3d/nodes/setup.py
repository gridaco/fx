"""The character as a profile: anatomy, parts, criteria, the bar it is judged at.

Runs while planning, so the parts a brief makes are known before anything is paid for,
and a profile or partition the rules refuse stops the plan, not the run.
"""

from __future__ import annotations

import json
from typing import Any

from _character_harness import (
    LANES,
    PROFILE_FILES,
    compile_profile,
    experiment,
)
from _character_tools.quality_bar import quality_bar, upstream_policy

from grida.fx import Ctx, node


@node(
    "setup",
    inputs={"parts": "model{}?"},
    params={
        "profile": tuple(PROFILE_FILES),
        "partition": ("whole", "head_body_hair"),
        "review": ("required", "none"),
        "quality_bar": ("low", "medium"),
        "entry": ("brief", "parts"),
    },
    outputs={"profile": "json", "bar": "json"},
    resources=list(PROFILE_FILES.values()),
    version=3,
)
def setup(ctx: Ctx) -> dict[str, Any]:
    """Compile the profile and the bars its reviews are held to."""

    profile_id = ctx.params["profile"]
    entry = ctx.params["entry"]
    supplied = sorted(ctx.inputs.get("parts") or {})
    settings = {
        "pipeline_mode": "brief_to_rig" if entry == "brief" else "parts_to_rig",
        "review": ctx.params["review"],
        "quality_bar": ctx.params["quality_bar"],
        "partition": ctx.params["partition"],
        "lane": LANES[profile_id],
        "roles": supplied,
    }
    try:
        profile = compile_profile(json.loads(ctx.prompt(PROFILE_FILES[profile_id])), settings)
    except ValueError as error:
        raise ctx.fail(f"the profile refuses this character: {error}") from error
    roles = list(profile.get("required_parts", []))
    if entry == "parts" and set(supplied) != set(roles):
        raise ctx.fail(f"supplied parts are {sorted(supplied)}; the profile needs {roles}")
    rules = experiment(settings)
    bar = quality_bar(rules, profile)
    ctx.fact("roles", roles)
    ctx.fact("lane", LANES[profile_id])
    ctx.fact("level", bar["level"])
    return {
        "profile": ctx.out.json(profile),
        "bar": ctx.out.json({"rig": bar, "upstream": upstream_policy(rules, profile)}),
    }
