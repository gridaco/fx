# Grida FX skills

Installable agent skills for authoring workflows, running them with FX, and
inspecting plans and recorded results in your own project.

The canonical [grida-fx](grida-fx/SKILL.md) skill covers the installed CLI and SDKs:
workflow authoring, offline plans and tests, bounded paid runs, caches, artifacts,
and the local viewer. Installing a skill does not install the FX engine or SDK;
the skill includes package installation guidance.

New or revised skills become available through the remote commands below after
their files are pushed to this repository.

## Install

From the project where you want to use FX:

```sh
# Discover available skills without installing them.
npx skills add gridaco/fx --list

# Choose skills and an agent interactively.
npx skills add gridaco/fx
```

Install the canonical skill directly:

```sh
npx skills add gridaco/fx --skill grida-fx
```

Add `--global` to install for your user instead of the current project. See the
[skills CLI documentation](https://github.com/vercel-labs/skills) for agent
selection and installation options.

## Layout

Each skill lives in its own directory:

```text
skills/
  README.md
  <skill-name>/
    SKILL.md
    references/    # optional focused documentation
    scripts/       # optional helper scripts
    assets/        # optional templates or fixtures
```

`SKILL.md` is the entry point, with YAML frontmatter containing `name` and
`description`. The description says when the agent should use the skill; the body
contains the instructions. Keep supporting files inside that skill's directory
and reference them relative to `SKILL.md`.

## Authoring conventions

- Skills work in a user's project without an FX repository checkout. Use the
  public `grida-fx` command or `grida.fx` SDK, and document required dependencies.
- Keep each skill self-contained. Do not depend on sibling skills, repository
  tooling, contributor instructions, or local development paths.
- Teach the existing FX contracts rather than implementing another engine.
  Keep runnable templates small, original, and provider-neutral.
- Planning and offline examples are the default. Paid execution requires the
  user's explicit intent and a dollar ceiling. Installation grants no spending
  or publication authorization.
- Never request, print, or bundle secrets. Let FX manage provider credentials.
- Before adding a skill, check discovery from a local path with
  `npx skills add ./skills --list`, and exercise its instructions in a fresh
  project using the public FX interface and provider-free fixtures.

This directory contains skills distributed to FX users. Repository contributor
rules remain in [AGENTS.md](../AGENTS.md).
