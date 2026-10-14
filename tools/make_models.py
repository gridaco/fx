"""Writes and checks crates/grida-fx-providers/routes/models.json, the Grida Models facts that FX's
built-in routes use (spec/providers.md section 3).

Grida Models (https://github.com/gridaco/models) is used when FX is built, never when it runs: FX
pins one release, keeps this projection of it, and crates/grida-fx-providers/tests/models.rs holds
the built-in route table to it. The full release is never committed.

Run from the repository root:

    uv run --project python python tools/make_models.py --release <RELEASE>   # write
    uv run --project python python tools/make_models.py --check               # offline

Writing downloads four assets of the release from GitHub (``--from <dir>`` reads them from a
folder instead): ``SHA256SUMS``, ``manifest.json``, ``models.v1.json`` and
``models.v1.schema.json``. Each must match its line of ``SHA256SUMS``, the manifest must name the
release and agree with both files, and ``models.v1.json`` must validate against the schema (draft
2020-12) and name its ``$id``. ``--release`` must be RELEASE, the pin: moving the pin is a change
to this file and a new projection.

The projection is ``{"source", "routes"}``. ``source`` names the repository, release, asset, its
bytes and SHA-256, the schema's ``$id`` and the data's license. ``routes`` has one member per
route id of the built-in table: ``null`` when Grida Models has no card for its model (CARDS), else
``{"section", "card"}`` and, for a rig, ``"rig_check"`` (the section's rig-check operation). Each
holds only the members of its schema object that KEEP lists, and of a card or the operation only
what concerns the route's provider (Grida Models' name for it, PROVIDERS): its
``provider_coverage``, its binding under ``providers``, and the card's own ``endpoints`` when the
provider is the card's ``reference_provider`` (Tripo's cards record Tripo's endpoints there).

Writing also holds CARDS to the release: each card must bind its route's model on its provider
(``binds``), and no card may bind a route CARDS gives none, so a card that appears is not missed.

``--check`` reads neither the release nor the network. It holds the file to its canonical text
(two-space indents, sorted members, a final newline), its ``source`` to the pin, its routes to the
built-in table's route ids and to CARDS, every member to KEEP, and the release spec/providers.md
names to the pin.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import urllib.request
from pathlib import Path
from typing import Any

TOOLS = Path(__file__).resolve().parent
REPO = TOOLS.parent
sys.path.insert(0, str(TOOLS))

import digest  # noqa: E402  (a sibling script, not a package)

ROUTES_DIR = REPO / "crates" / "grida-fx-providers" / "routes"
DEFAULT_ROUTES = ROUTES_DIR / "default.yaml"
OUT = ROUTES_DIR / "models.json"
# Section 3 names the pin.
SPEC = REPO / "spec" / "providers.md"

REPOSITORY = "https://github.com/gridaco/models"
RELEASE = "v1.20261013.6"
ASSET = "models.v1.json"
SCHEMA_ASSET = "models.v1.schema.json"
SUMS = "SHA256SUMS"
MANIFEST = "manifest.json"
# The manifest names files by their catalogue path.
MANIFEST_NAMES = {ASSET: "models.json", SCHEMA_ASSET: "schemas/models.schema.json"}
SCHEMA_ID = "https://grida.co/schemas/ai-model-facts-v1.json"
LICENSE = "MIT"
MAX_ASSET_BYTES = 32 * 1024 * 1024

# Each route id of the built-in table and the Grida Models card of its model, as (section, card
# id); None where Grida Models has none (tests/models.rs gives each reason).
CARDS: dict[str, tuple[str, str] | None] = {
    "openai/gpt-5.6-sol@gg": ("text.catalog", "openai/gpt-5.6-sol"),
    "openai/gpt-5.6-sol@openrouter": ("text.catalog", "openai/gpt-5.6-sol"),
    "openai/gpt-6-astra@gg": ("text.catalog", "openai/gpt-6-astra"),
    "openai/gpt-6-astra@openrouter": ("text.catalog", "openai/gpt-6-astra"),
    "gpt-image-2.5-sunburst@openai": ("image.models", "openai/gpt-image-2.5-sunburst"),
    "openai/gpt-image-2.5-sunburst@gg": ("image.models", "openai/gpt-image-2.5-sunburst"),
    "openai/gpt-image-2.5-sunburst@openrouter": ("image.models", "openai/gpt-image-2.5-sunburst"),
    "openai/gpt-image-2.5/sunburst@fal": ("image.models", "openai/gpt-image-2.5-sunburst"),
    "imagegen@codex": None,
    "imagine@grok": None,
    "P2-20260801@tripo": ("three_d.model_generation.models", "tripo/p2"),
    "tripo/p2@gg": ("three_d.model_generation.models", "tripo/p2"),
    "tripo/rig-v1.0@gg": ("three_d.rigging.models", "tripo/rig-v1.0"),
    "v1.0-20240301@tripo": ("three_d.rigging.models", "tripo/rig-v1.0"),
    "google/lyria-3-pro-preview@openrouter": ("audio.music.models", "google/lyria-3-pro"),
    "eleven_text_to_sound_v2@elevenlabs": (
        "audio.sound_effects.models",
        "eleven_text_to_sound_v2",
    ),
    "eleven_v3@elevenlabs": ("audio.text_to_speech.models", "eleven_v3"),
    "alibaba/wan-3.0@gg": ("video.models", "alibaba/wan-3.0"),
    "google/gemini-omni-flash/v1.1/image-to-video@fal": None,
}
RIGGING = "three_d.rigging.models"
RIG_CHECK = "three_d.rig_check.operation"
# FX's provider names that Grida Models spells otherwise.
PROVIDERS = {"gg": "grida"}

# The members kept of each schema object, by its definition in the schema (`$defs`); `rig_check`
# is the top-level `three_d.rig_check.operation`, and `coverage` one provider's entry of a
# `provider_coverage`. Each is checked against the release's schema when the file is written.
KEEP: dict[str, tuple[str, ...]] = {
    "card": ("id", "reference_provider", "provider_coverage", "providers", "endpoints"),
    "rig_check": ("reference_provider", "provider_coverage", "providers", "endpoints"),
    "coverage": ("disposition",),
    "provider_binding": ("provider", "id", "capabilities", "limits", "token_pricing", "endpoints"),
    "provider_endpoint": (
        "id",
        "input",
        "interface",
        "operation",
        "parameters",
        "image_constraints",
        "published_pricing",
    ),
    "capabilities": ("image_input", "tools", "reasoning", "structured_output"),
    "limits": ("max_output_tokens",),
    "token_pricing": ("unit", "input", "output", "tiers"),
    "token_price_tier": ("meter", "minimum_input_tokens", "maximum_input_tokens", "usd_per_token"),
    "parameter": (
        "type",
        "enum",
        "minimum",
        "maximum",
        "multiple_of",
        "min_items",
        "max_items",
        "min_length",
        "max_length",
    ),
    "image_constraints": (
        "step",
        "min_edge",
        "max_edge",
        "min_pixels",
        "max_pixels",
        "aspect_ratio",
    ),
    "published_pricing": ("currency", "components"),
    "published_price_component": (
        "meter",
        "unit",
        "usd_per_unit",
        "per_quantity",
        "quantity_range",
    ),
}
# The members of each kept member that is itself a kept object (or a list or map of them).
NESTED: dict[tuple[str, str], tuple[str, str]] = {
    ("card", "provider_coverage"): ("map", "coverage"),
    ("card", "providers"): ("map", "provider_binding"),
    ("card", "endpoints"): ("list", "provider_endpoint"),
    ("rig_check", "provider_coverage"): ("map", "coverage"),
    ("rig_check", "providers"): ("map", "provider_binding"),
    ("rig_check", "endpoints"): ("list", "provider_endpoint"),
    ("provider_binding", "capabilities"): ("one", "capabilities"),
    ("provider_binding", "limits"): ("one", "limits"),
    ("provider_binding", "token_pricing"): ("one", "token_pricing"),
    ("provider_binding", "endpoints"): ("list", "provider_endpoint"),
    ("provider_endpoint", "parameters"): ("map", "parameter"),
    ("provider_endpoint", "image_constraints"): ("one", "image_constraints"),
    ("provider_endpoint", "published_pricing"): ("one", "published_pricing"),
    ("token_pricing", "tiers"): ("list", "token_price_tier"),
    ("published_pricing", "components"): ("list", "published_price_component"),
}
SOURCE_KEYS = ("repository", "release", "asset", "bytes", "sha256", "schema", "license")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
RELEASE_TAG = re.compile(r"`(v\d+\.\d{8}\.\d+)`")


def models_provider(route: str) -> str:
    provider = route.rsplit("@", 1)[1]
    return PROVIDERS.get(provider, provider)


def route_ids() -> list[str]:
    table = digest.read_yaml_file(DEFAULT_ROUTES)
    return sorted({route["route"] for route in table["routes"]})


# ---------------------------------------------------------------------------------------------
# Projecting


def pick(value: dict[str, Any], kind: str) -> dict[str, Any]:
    """The members of `value` that KEEP lists for `kind`, each projected as NESTED says. An object
    left with no member is dropped from its parent."""

    out: dict[str, Any] = {}
    for name in KEEP[kind]:
        if name not in value:
            continue
        member = value[name]
        shape = NESTED.get((kind, name))
        if shape is not None:
            form, inner = shape
            if form == "one":
                member = pick(member, inner)
            elif form == "list":
                member = [pick(item, inner) for item in member]
            else:
                member = {key: pick(item, inner) for key, item in member.items()}
            if not member:
                continue
        out[name] = member
    return out


def project_card(card: dict[str, Any], provider: str, kind: str) -> dict[str, Any]:
    """A card (or the rig-check operation) as it concerns `provider`."""

    shown: dict[str, Any] = {key: card[key] for key in ("id", "reference_provider") if key in card}
    coverage = card.get("provider_coverage", {})
    if provider in coverage:
        shown["provider_coverage"] = {provider: coverage[provider]}
    bindings = card.get("providers", {})
    if provider in bindings:
        shown["providers"] = {provider: bindings[provider]}
    if card.get("reference_provider") == provider and "endpoints" in card:
        shown["endpoints"] = card["endpoints"]
    return pick(shown, kind)


def binds(card: dict[str, Any], route: str) -> bool:
    """Whether `card` binds the route's model on its provider: a binding or endpoint id, an
    endpoint's `model`, or, on the card's reference provider, the card's own id."""

    model = route.rsplit("@", 1)[0]
    provider = models_provider(route)
    binding = card.get("providers", {}).get(provider)
    endpoints = list((binding or {}).get("endpoints", []))
    if card.get("reference_provider") == provider:
        endpoints += card.get("endpoints", [])
        if card.get("id") == model:
            return True
    if binding is not None and binding.get("id") == model:
        return True
    for endpoint in endpoints:
        named = endpoint.get("parameters", {}).get("model", {}).get("enum", [])
        if endpoint.get("id") == model or str(endpoint.get("id")).startswith(model + "/"):
            return True
        if model in named:
            return True
    return False


def project(models: dict[str, Any], raw: bytes, release: str) -> dict[str, Any]:
    routes: dict[str, Any] = {}
    for route, place in CARDS.items():
        if place is None:
            for section, cards in models.items():
                if section == RIG_CHECK or not isinstance(cards, dict):
                    continue
                found = [cid for cid, c in cards.items() if isinstance(c, dict) and binds(c, route)]
                if found:
                    raise SystemExit(
                        f"make_models: CARDS gives {route} no card, but {release} binds it in "
                        f"{section} {found[0]}"
                    )
            routes[route] = None
            continue
        section, card_id = place
        card = models.get(section, {}).get(card_id)
        if card is None or not binds(card, route):
            raise SystemExit(f"make_models: {release} has no card {section} {card_id} for {route}")
        provider = models_provider(route)
        entry: dict[str, Any] = {"section": section, "card": project_card(card, provider, "card")}
        if section == RIGGING:
            entry["rig_check"] = project_card(models[RIG_CHECK], provider, "rig_check")
        routes[route] = entry
    source = {
        "repository": REPOSITORY,
        "release": release,
        "asset": ASSET,
        "bytes": len(raw),
        "sha256": hashlib.sha256(raw).hexdigest(),
        "schema": SCHEMA_ID,
        "license": LICENSE,
    }
    return {"source": source, "routes": routes}


def text(projection: dict[str, Any]) -> str:
    return json.dumps(projection, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


# ---------------------------------------------------------------------------------------------
# The release


def fetch(tag: str, name: str, folder: Path | None) -> bytes:
    if folder is not None:
        return (folder / name).read_bytes()
    url = f"{REPOSITORY}/releases/download/{tag}/{name}"
    request = urllib.request.Request(url, headers={"user-agent": "grida-fx-make-models"})
    with urllib.request.urlopen(request, timeout=60) as response:
        data = response.read(MAX_ASSET_BYTES + 1)
    if len(data) > MAX_ASSET_BYTES:
        raise SystemExit(f"make_models: {name} is larger than {MAX_ASSET_BYTES} bytes")
    return data


def verified(tag: str, folder: Path | None) -> tuple[bytes, dict[str, Any], dict[str, Any]]:
    """The release's models.v1.json bytes, parsed, and its schema, each verified."""

    sums: dict[str, str] = {}
    for line in fetch(tag, SUMS, folder).decode("utf-8").splitlines():
        if line.strip():
            digest_hex, name = line.split(maxsplit=1)
            sums[name.lstrip("*")] = digest_hex
    files = {name: fetch(tag, name, folder) for name in (MANIFEST, ASSET, SCHEMA_ASSET)}
    for name, data in files.items():
        if sums.get(name) != hashlib.sha256(data).hexdigest():
            raise SystemExit(f"make_models: {name} does not match {SUMS} of {tag}")
    manifest = json.loads(files[MANIFEST])
    if manifest.get("version") != tag:
        raise SystemExit(f"make_models: {MANIFEST} is {manifest.get('version')!r}, not {tag}")
    for name, path in MANIFEST_NAMES.items():
        entry = manifest.get("files", {}).get(path, {})
        data = files[name]
        stated = (entry.get("sha256"), entry.get("bytes"))
        if stated != (hashlib.sha256(data).hexdigest(), len(data)):
            raise SystemExit(f"make_models: {MANIFEST} disagrees with {name} on {path}")
    from jsonschema import Draft202012Validator

    schema = json.loads(files[SCHEMA_ASSET])
    if schema.get("$id") != SCHEMA_ID:
        raise SystemExit(f"make_models: the schema's $id is {schema.get('$id')!r}")
    Draft202012Validator.check_schema(schema)
    models = json.loads(files[ASSET])
    errors = sorted(Draft202012Validator(schema).iter_errors(models), key=lambda e: e.json_path)
    if errors:
        raise SystemExit(f"make_models: {ASSET} does not validate: {errors[0].message}")
    if models.get("$schema") != SCHEMA_ID:
        raise SystemExit(f"make_models: {ASSET} names schema {models.get('$schema')!r}")
    return files[ASSET], models, schema


def schema_problems(schema: dict[str, Any]) -> list[str]:
    """KEEP's members that the schema does not define."""

    defs = schema["$defs"]
    defined = {
        kind: set(defs[kind]["properties"])
        for kind in KEEP
        if kind in defs and "properties" in defs[kind]
    }
    defined["rig_check"] = set(schema["properties"][RIG_CHECK]["properties"])
    defined["coverage"] = set(defs["provider_coverage"]["additionalProperties"]["properties"])
    problems = []
    for kind, names in KEEP.items():
        for name in names:
            if name not in defined.get(kind, set()):
                problems.append(f"KEEP[{kind!r}]: {name} is not a member the schema defines")
    return problems


# ---------------------------------------------------------------------------------------------
# --check


def kept_problems(value: Any, kind: str, where: str) -> list[str]:
    if not isinstance(value, dict):
        return [f"{where}: not an object"]
    problems = []
    for name, member in value.items():
        if name not in KEEP[kind]:
            problems.append(f"{where}: {name} is not kept of a {kind}")
            continue
        shape = NESTED.get((kind, name))
        if shape is None:
            continue
        form, inner = shape
        if form == "one":
            problems += kept_problems(member, inner, f"{where}.{name}")
        elif form == "list" and isinstance(member, list):
            for i, item in enumerate(member):
                problems += kept_problems(item, inner, f"{where}.{name}[{i}]")
        elif form == "map" and isinstance(member, dict):
            for key, item in member.items():
                problems += kept_problems(item, inner, f"{where}.{name}.{key}")
        else:
            problems.append(f"{where}.{name}: not a {form} of {inner}")
    return problems


def check() -> list[str]:
    where = OUT.relative_to(REPO).as_posix()
    if not OUT.is_file():
        return [f"{where}: missing; run tools/make_models.py --release {RELEASE}"]
    written = OUT.read_text(encoding="utf-8")
    try:
        projection = json.loads(written)
    except json.JSONDecodeError as error:
        return [f"{where}: not JSON: {error}"]
    problems = []
    if not isinstance(projection, dict) or set(projection) != {"source", "routes"}:
        return [f"{where}: holds exactly source and routes"]
    if written != text(projection):
        problems.append(f"{where}: not in its canonical form; run tools/make_models.py")
    source = projection["source"]
    expected = {
        "repository": REPOSITORY,
        "release": RELEASE,
        "asset": ASSET,
        "schema": SCHEMA_ID,
        "license": LICENSE,
    }
    if not isinstance(source, dict) or set(source) != set(SOURCE_KEYS):
        problems.append(f"{where}: source holds exactly {', '.join(SOURCE_KEYS)}")
    else:
        for key, value in expected.items():
            if source[key] != value:
                problems.append(f"{where}: source.{key} is {source[key]!r}, not {value!r}")
        size = source["bytes"]
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            problems.append(f"{where}: source.bytes is not a positive integer")
        if not isinstance(source["sha256"], str) or not SHA256.match(source["sha256"]):
            problems.append(f"{where}: source.sha256 is not 64 lowercase hexadecimal digits")
    routes = projection["routes"]
    ids = route_ids()
    if sorted(CARDS) != ids:
        problems.append(f"CARDS names {sorted(CARDS)}, the built-in table {ids}")
    named = sorted(set(RELEASE_TAG.findall(SPEC.read_text(encoding="utf-8"))))
    if named != [RELEASE]:
        spec = SPEC.relative_to(REPO).as_posix()
        problems.append(f"{spec}: names Grida Models {named}, not the pin {RELEASE}")
    if not isinstance(routes, dict) or sorted(routes) != ids:
        got = sorted(routes) if isinstance(routes, dict) else routes
        return [
            *problems,
            f"{where}: routes are {got}, the built-in table's {ids}; run "
            f"tools/make_models.py --release {RELEASE}",
        ]
    for route, entry in routes.items():
        place = CARDS.get(route)
        at = f"{where}: routes.{route}"
        if place is None or entry is None:
            if place is not None or entry is not None:
                problems.append(f"{at}: is {entry!r}, CARDS {place!r}")
            continue
        section, card_id = place
        keys = {"section", "card"} | ({"rig_check"} if section == RIGGING else set())
        if not isinstance(entry, dict) or set(entry) != keys:
            problems.append(f"{at}: holds exactly {', '.join(sorted(keys))}")
            continue
        card = entry["card"]
        if entry["section"] != section or not isinstance(card, dict) or card.get("id") != card_id:
            problems.append(f"{at}: is not {section} {card_id}")
            continue
        provider = models_provider(route)
        for part, kind in (("card", "card"), ("rig_check", "rig_check")):
            if part not in entry:
                continue
            found = kept_problems(entry[part], kind, f"{at}.{part}")
            problems += found
            if found:
                continue
            for key in ("provider_coverage", "providers"):
                other = sorted(set(entry[part].get(key, {})) - {provider})
                if other:
                    problems.append(f"{at}.{part}.{key}: {other} are not the route's provider")
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--release", help="the Grida Models release to project: RELEASE, the pin")
    mode.add_argument("--check", action="store_true", help="check the projection, offline")
    parser.add_argument(
        "--from",
        dest="folder",
        type=Path,
        help="read the release's assets from this folder instead of downloading them",
    )
    args = parser.parse_args(argv)
    if args.check:
        if args.folder is not None:
            parser.error("--from goes with --release")
        problems = check()
        for problem in problems:
            print(problem, file=sys.stderr)
        if problems:
            return 1
        print(f"make_models: {OUT.relative_to(REPO).as_posix()} matches {RELEASE}")
        return 0
    if args.release != RELEASE:
        parser.error(f"--release must be {RELEASE}, the pin; move RELEASE first")
    ids = route_ids()
    if sorted(CARDS) != ids:
        print(f"make_models: CARDS names {sorted(CARDS)}, the table {ids}", file=sys.stderr)
        return 1
    raw, models, schema = verified(args.release, args.folder)
    problems = schema_problems(schema)
    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        return 1
    OUT.write_text(text(project(models, raw, args.release)), encoding="utf-8")
    problems = check()
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        return 1
    print(f"make_models: wrote {OUT.relative_to(REPO).as_posix()} from {args.release}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
