"""Holds llms.txt to the engine: what CI checks so that the file stays the reference agents can
trust (AGENTS.md, "llms.txt").

    uv run --project python python tools/check_llms.py [--command <grida-fx>]

- **Commands.** Every ``grida fx …`` in llms.txt, in a code block or a code span, names a verb
  the engine has, and each ``--flag`` before a bare ``--`` is one that verb's ``--help`` lists.
- **Recipes.** Each ``###`` section under ``## Recipes`` is a project: every code block whose
  first line is a comment naming a path (``# workflows/icon.yaml``, ``// package.json``) is
  written there without that line, with a bare ``fx.yaml`` when the recipe has none and the
  checkout's built ``@grida/fx`` when it has a ``package.json``. Then each of its ``grida fx
  plan`` lines, and each ``grida fx run`` line without ``--live``, runs in it and must exit 0.
  A recipe without a plan line fails.
- **Project files elsewhere.** A ``# fx.yaml`` block outside the recipes is planned with a
  one-step workflow, so its settings are ones the engine accepts.
- **Names.** Every node type (``fx/<name>@<major>``) and route (``model@provider``) the file
  names in a code span is one ``grida fx nodes`` lists.
- **Links.** A relative link resolves in the checkout, and so does a link to
  ``https://github.com/gridaco/fx/blob/main/…`` or ``…/tree/main/…``.

Nothing is spent: the provider keys are removed, ``GRIDA_FX_NETWORK`` is off and ``.env`` is
ignored. The engine is ``--command``, else ``GRIDA_FX_BIN``, else the one
``tools/build_engine.py`` put beside the SDK; Python bodies run in this interpreter
(``GRIDA_FX_PYTHON``, unless set). The version line is tools/check_versions.py's.

Prints one line per check and exits 1 when any fails.
"""

from __future__ import annotations

import argparse
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LLMS = ROOT / "llms.txt"
JS_SDK = ROOT / "js" / "fx"
ENGINE = ROOT / "python" / "src" / "grida" / "fx" / "_bin" / "grida-fx"
REPOSITORY = "https://github.com/gridaco/fx/"

#: Every variable FX reads a key or a provider address from (spec/providers.md §3).
KEYS = (
    "GG_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "FAL_KEY",
    "TRIPO_API_KEY",
    "ELEVENLABS_API_KEY",
    "GG_BASE_URL",
    "OPENAI_BASE_URL",
    "OPENROUTER_BASE_URL",
    "FAL_BASE_URL",
    "ELEVENLABS_BASE_URL",
)
#: Verbs whose first argument is a subcommand with its own help.
NESTED = {"runs", "cache", "takes"}
#: A path comment on a block's first line: ``# fx.yaml``, ``// nodes/caption.ts``.
PATH_LINE = re.compile(r"^(?:#|//)\s*([A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*\.[A-Za-z0-9]+)\s*$")
FENCE = re.compile(r"^```(\S*)\s*$")
COMMAND = re.compile(r"grida fx\b[^`\n]*")
FLAG = re.compile(r"--[a-z][a-z0-9-]*")
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
NODE_TYPE = re.compile(r"\bfx/[a-z_]+(?:\.[a-z_]+)*@\d+")
#: A route, ``model@provider``; ``fx/…@1`` is a node type, and ``model@provider`` the form itself.
ROUTE = re.compile(r"^(?!fx/)[A-Za-z0-9][A-Za-z0-9._/-]*@[a-z][a-z0-9._-]*$")


@dataclass
class Block:
    language: str
    lines: list[str]
    section: str  # the ``##`` heading above it
    recipe: str | None  # the ``###`` heading under ``## Recipes``, else None


@dataclass
class Recipe:
    name: str
    files: dict[str, str] = field(default_factory=dict)
    commands: list[str] = field(default_factory=list)


def parse(text: str) -> tuple[list[Block], list[str]]:
    """The fenced blocks, and the prose lines outside them."""
    blocks: list[Block] = []
    prose: list[str] = []
    section, recipe = "", None
    open_block: Block | None = None
    for line in text.splitlines():
        if open_block is not None:
            if line.startswith("```"):
                blocks.append(open_block)
                open_block = None
            else:
                open_block.lines.append(line)
            continue
        fence = FENCE.match(line)
        if fence:
            open_block = Block(fence.group(1), [], section, recipe)
            continue
        if line.startswith("## "):
            section, recipe = line[3:].strip(), None
        elif line.startswith("### ") and section == "Recipes":
            recipe = line[4:].strip()
        prose.append(line)
    if open_block is not None:
        raise SystemExit("llms.txt: a code block is never closed")
    return blocks, prose


def commands(blocks: list[Block], prose: list[str]) -> list[str]:
    """Every ``grida fx …`` the file shows: whole lines of shell blocks, and code spans."""
    found: list[str] = []
    for block in blocks:
        if block.language in {"sh", "bash"}:
            found += [m.group(0) for line in block.lines for m in COMMAND.finditer(strip(line))]
    for line in prose:
        for span in re.findall(r"`([^`]+)`", line):
            found += [m.group(0) for m in COMMAND.finditer(span)]
    return found


def strip(line: str) -> str:
    """A shell line without its trailing ``# comment`` (a ``#`` after a space)."""
    return re.split(r"\s+#\s", line, maxsplit=1)[0].rstrip()


class Engine:
    """The engine's verbs and each one's flags, read from its help once."""

    def __init__(self, command: str) -> None:
        self.command = command
        self.verbs = set(self._commands([]))
        self._flags: dict[tuple[str, ...], set[str]] = {}

    def _help(self, words: list[str]) -> str:
        done = subprocess.run(
            [self.command, *words, "--help"], capture_output=True, text=True, check=False
        )
        if done.returncode != 0:
            raise ValueError(f"`grida fx {' '.join(words)} --help` failed: {done.stderr.strip()}")
        return done.stdout

    def _commands(self, words: list[str]) -> list[str]:
        listing = self._help(words).split("Commands:", 1)[-1].split("Options:", 1)[0]
        return [m.group(1) for m in re.finditer(r"^  ([a-z][a-z-]*)\s", listing, re.MULTILINE)]

    def catalog(self) -> tuple[set[str], set[str]]:
        """The node types and the routes ``grida fx nodes`` lists."""
        listing = self._run(["nodes"])
        types = set(re.findall(r"^(fx/\S+)", listing, re.MULTILINE))
        routes: set[str] = set()
        for name in types:
            for line in self._run(["nodes", name]).splitlines():
                if line.strip().startswith("routes "):
                    routes.update(r.strip() for r in line.strip()[len("routes ") :].split(","))
        return types, routes

    def _run(self, words: list[str]) -> str:
        done = subprocess.run(
            [self.command, *words], capture_output=True, text=True, check=False, env=environment()
        )
        if done.returncode != 0:
            raise ValueError(f"`grida fx {' '.join(words)}` failed: {done.stderr.strip()}")
        return done.stdout

    def flags(self, words: tuple[str, ...]) -> set[str]:
        if words not in self._flags:
            self._flags[words] = set(FLAG.findall(self._help(list(words))))
        return self._flags[words]

    def problem(self, shown: str) -> str | None:
        """Why a ``grida fx …`` the file shows is not one this engine takes, else None."""
        words = shown.split()[2:]
        if not words or words[0].startswith("<"):  # `grida fx`, `grida fx <verb> --help`
            return None
        verb = words[0]
        if verb.startswith("-"):
            known = self.flags(())
            return None if verb in known else f"`{shown}`: grida fx has no {verb}"
        if verb not in self.verbs:
            return f"`{shown}`: no verb {verb}"
        path: tuple[str, ...] = (verb,)
        if verb in NESTED and len(words) > 1 and re.fullmatch(r"[a-z]+", words[1]):
            if words[1] not in self._commands([verb]):
                return f"`{shown}`: no command {verb} {words[1]}"
            path = (verb, words[1])
        known = self.flags(path)
        for word in words[1:]:
            if word == "--":
                break
            for flag in FLAG.findall(word):
                if flag not in known:
                    return f"`{shown}`: grida fx {' '.join(path)} has no {flag}"
        return None


def recipes(blocks: list[Block]) -> list[Recipe]:
    found: dict[str, Recipe] = {}
    for block in blocks:
        if block.recipe is None:
            continue
        recipe = found.setdefault(block.recipe, Recipe(block.recipe))
        named = PATH_LINE.match(block.lines[0]) if block.lines else None
        if named:
            recipe.files[named.group(1)] = "\n".join(block.lines[1:]) + "\n"
        elif block.language in {"sh", "bash"}:
            for line in block.lines:
                line = strip(line)
                if re.match(r"grida fx (plan|run)\b", line):
                    recipe.commands.append(line)
    return list(found.values())


def environment() -> dict[str, str]:
    env = {name: value for name, value in os.environ.items() if name not in KEYS}
    env["GRIDA_FX_NETWORK"] = "off"
    env["GRIDA_FX_DISABLE_DOTENV"] = "1"
    env.setdefault("GRIDA_FX_PYTHON", sys.executable)
    return env


def install_sdk(home: Path) -> None:
    """The checkout's built ``@grida/fx`` in ``node_modules``, as ``npm install`` puts it."""
    if not (JS_SDK / "dist" / "host-entry.js").is_file():
        raise ValueError("js/fx is not built: bun run build there")
    package = home / "node_modules" / "@grida" / "fx"
    package.mkdir(parents=True)
    shutil.copy2(JS_SDK / "package.json", package / "package.json")
    shutil.copytree(JS_SDK / "dist", package / "dist")


def run_recipe(engine: Engine, recipe: Recipe) -> None:
    if not any(line.startswith("grida fx plan") for line in recipe.commands):
        raise ValueError("no `grida fx plan` line")
    with tempfile.TemporaryDirectory(prefix="fx-llms-") as scratch:
        home = Path(scratch)
        files = {"fx.yaml": "fx: project/v1\n", **recipe.files}
        for name, content in files.items():
            (home / name).parent.mkdir(parents=True, exist_ok=True)
            (home / name).write_text(content, encoding="utf-8")
        if "package.json" in files:
            install_sdk(home)
        for line in recipe.commands:
            words = shlex.split(line)[2:]
            if words[0] == "run" and "--live" in words:
                continue
            done = subprocess.run(
                [engine.command, *words],
                cwd=home,
                env=environment(),
                capture_output=True,
                text=True,
                check=False,
            )
            if done.returncode != 0:
                said = (done.stdout + done.stderr).strip().splitlines()[-6:]
                raise ValueError(f"`{line}` exited {done.returncode}:\n    " + "\n    ".join(said))


def check_project_file(engine: Engine, content: str) -> None:
    with tempfile.TemporaryDirectory(prefix="fx-llms-") as scratch:
        home = Path(scratch)
        (home / "fx.yaml").write_text(content, encoding="utf-8")
        (home / "probe.yaml").write_text(
            "fx: workflow/v1\nid: probe\ntitle: Probe\nsteps:\n"
            "  draw:\n    uses: fx/image.generate@1\n    with: { prompt: probe }\n",
            encoding="utf-8",
        )
        done = subprocess.run(
            [engine.command, "plan", "probe.yaml", "--check"],
            cwd=home,
            env=environment(),
            capture_output=True,
            text=True,
            check=False,
        )
        if done.returncode != 0:
            raise ValueError(f"fx.yaml does not plan: {(done.stdout + done.stderr).strip()}")


def link_problem(target: str) -> str | None:
    if target.startswith(REPOSITORY):
        rest = target[len(REPOSITORY) :]
        match = re.match(r"(?:blob|tree)/main/([^#?]*)", rest)
        if match is None:
            return f"{target}: not a link to main"
        path = match.group(1).rstrip("/")
    elif re.match(r"[a-z]+:", target) or target.startswith("#"):
        return None
    else:
        path = target.split("#", 1)[0]
    return None if (ROOT / path).exists() else f"{target}: {path} is not in the checkout"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--command", help="the grida-fx engine to check against")
    options = parser.parse_args(argv)
    command = options.command or os.environ.get("GRIDA_FX_BIN") or str(ENGINE)
    found = Path(command).resolve() if Path(command).is_file() else shutil.which(command)
    if found is None:
        print(f"check_llms: no engine at {command}: run tools/build_engine.py or pass --command")
        return 1
    command = str(found)

    engine = Engine(command)
    text = LLMS.read_text(encoding="utf-8")
    blocks, prose = parse(text)
    failures = 0

    def report(name: str, problem: str | None) -> None:
        nonlocal failures
        failures += problem is not None
        print(f"ok   {name}" if problem is None else f"FAIL {name}: {problem}")

    shown = commands(blocks, prose)
    problems = [p for p in dict.fromkeys(engine.problem(c) for c in shown) if p]
    report(f"commands ({len(shown)})", "\n    ".join(problems) or None)

    types, routes = engine.catalog()
    spans = [span for line in prose for span in re.findall(r"`([^`]+)`", line)]
    named_types = {m.group(0) for span in spans for m in NODE_TYPE.finditer(span)}
    named_routes = {span for span in spans if ROUTE.match(span) and span not in {"model@provider"}}
    problems = [f"no node type {name}" for name in sorted(named_types - types)]
    problems += [f"no route {name}" for name in sorted(named_routes - routes)]
    report(
        f"names ({len(named_types)} types, {len(named_routes)} routes)",
        "\n    ".join(problems) or None,
    )

    links = LINK.findall(text)
    problems = [p for p in dict.fromkeys(link_problem(link) for link in links) if p]
    report(f"links ({len(links)})", "\n    ".join(problems) or None)

    for block in blocks:
        if block.recipe is None and block.lines and PATH_LINE.match(block.lines[0]):
            if PATH_LINE.match(block.lines[0]).group(1) == "fx.yaml":
                try:
                    check_project_file(engine, "\n".join(block.lines[1:]) + "\n")
                    report(f"fx.yaml in {block.section}", None)
                except ValueError as error:
                    report(f"fx.yaml in {block.section}", str(error))

    for recipe in recipes(blocks):
        try:
            run_recipe(engine, recipe)
            report(f"recipe: {recipe.name}", None)
        except ValueError as error:
            report(f"recipe: {recipe.name}", str(error))

    print(f"check_llms: {'ok' if failures == 0 else f'{failures} failed'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
