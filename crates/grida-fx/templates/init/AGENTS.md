# AGENTS.md

This is a [Grida FX](https://github.com/gridaco/fx) project. Work through the `grida fx` command
(or `npx grida fx` when it is not installed); FX plans, prices, caches and records every step.

Install the FX skill for authoring, running and inspecting workflows:

```sh
npx skills add gridaco/fx
```

## Rules

- **Discover before guessing.** `grida fx nodes`, `grida fx nodes <type>` and
  `grida fx schema workflows/<file>.yaml` list what is installed; `grida fx --help` lists commands.
- **Plan first.** `grida fx plan <workflow>` never spends. Show the user its steps and estimate.
- **Spending is the user's call.** Run `--live` only with the user's go and an agreed
  `--max-usd`. A run on their own Codex needs no ceiling but spends their plan: ask for it too.
- **Keys are the user's.** Never read, print or commit `.env` or a key. Ask the user to create
  keys and set them; check with `grida fx doctor`. What to ask for each provider:
  [onboarding](https://github.com/gridaco/fx/blob/main/skills/grida-fx/references/onboarding.md).
  Grida Gateway's API reference: <https://gateway.grida.ai/llms.txt>.
- **Ask before changing routes.** A new route is a new cache key: earlier results do not carry over.
- **`runs/` and `.fx/` belong to FX.** Inspect runs with `grida fx inspect`; do not edit them.
