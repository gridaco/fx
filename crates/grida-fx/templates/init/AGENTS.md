# AGENTS.md

This is a [Grida FX](https://github.com/gridaco/fx) project. Work through the `grida fx` command
(or `npx grida fx` when it is not installed); FX plans, prices, caches and records every step.

The FX agent skills are in `.agents/skills/`: read `grida-fx/SKILL.md` before authoring, running
or inspecting workflows. Update them with `npx skills add gridaco/fx`. The full FX reference, with
complete recipes, is <https://grida.co/fx/llms.txt>; it names the FX version it describes, so
compare it with `grida fx --version`.

## Rules

- **Show the live viewer, not a summary of it.** When the project's service is running
  (`grida fx status --json`), give the user the viewer's URL for what you are working on: the
  plan while you design a workflow (`grida fx plan <workflow> --open`), the run while it runs
  (`grida fx run … --open`), and the recorded run afterwards. The viewer updates live, so the
  user sees each step as it happens. Do not try to present the final renders yourself instead.
  Start the service with `grida fx start --background` when the user wants it.
- **Discover before guessing.** `grida fx nodes`, `grida fx nodes <type>` and
  `grida fx schema workflows/<file>.yaml` list what is installed; `grida fx --help` lists commands.
- **Plan first.** `grida fx plan <workflow>` never spends. Show the user its steps and estimate.
- **Spending is the user's call.** Run `--live` only with the user's go and an agreed
  `--max-usd`. A run on their own Codex needs no ceiling but spends their plan: ask for it too.
- **Keys are the user's.** Never read, print or commit `.env` or a key. Ask the user to create
  keys and set them; check with `grida fx doctor`. What to ask for each provider:
  `.agents/skills/grida-fx/references/onboarding.md`. Grida Gateway's API reference:
  <https://gateway.grida.ai/llms.txt>.
- **Ask before changing routes.** A new route is a new cache key: earlier results do not carry over.
- **`runs/` and `.fx/` belong to FX.** Inspect runs with `grida fx inspect`; do not edit them.
