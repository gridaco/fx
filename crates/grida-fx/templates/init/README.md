# FX project

A [Grida FX](https://github.com/gridaco/fx) project: workflows that plan, price, cache and record
every step, from your own code to AI models.

## Install the CLI

`init` created this folder; it installed nothing. FX comes with the Grida CLI (Node.js 24 or
later):

```sh
npm install -g grida
grida fx --version
```

Or run any command without installing, as `npx grida fx <command>`. Workflow steps written in
Python need the Python SDK too: `pip install grida` (Python 3.11 or later). Steps written in
TypeScript need Node 24 or later, and `npm install @grida/fx` in the project for type checking.

## Connect an AI provider

`workflows/icon.yaml` draws an image, so it needs a provider. Pick one, then set
`routes.image.generate` in `fx.yaml`:

| Provider | Set up | Route |
|---|---|---|
| [Grida Gateway](https://console.grida.co/_/ai-gateway) | create a key; set `GG_API_KEY`. One key for text, images, video and 3D | `openai/gpt-image-2.5-sunburst@gg` |
| Your own Codex | `codex login` with your ChatGPT account. Free to FX, images only | `imagegen@codex` |
| Your own keys | `OPENAI_API_KEY`, `FAL_KEY`, …: see `.env.example` | `gpt-image-2.5-sunburst@openai` |

Put keys in the environment, or copy `.env.example` to `.env` (kept out of git) and fill it in.
`grida fx doctor` says which keys and routes FX can use. Workflows of plain code need no provider.
More: [AI providers](https://github.com/gridaco/fx/blob/main/docs/guide/13-providers.md).

## Plan and run

```sh
grida fx plan icon                          # the steps and their price; never spends
grida fx run icon --live --max-usd 1        # calls the provider, within $1
grida fx run icon --live --max-usd 1 -- --name "silver key"
```

Without `--live`, nothing is spent: local steps run and cached results replay. A run whose calls are
all on your own Codex needs no `--max-usd`. Run it again and the cache answers.

To browse plans and runs on a canvas, start the local dashboard once and add `--open`:

```sh
grida fx start --background
grida fx run icon --live --max-usd 1 --open
```

## Layout

| Path | |
|---|---|
| `fx.yaml` | project settings: routes, budget, where workflows are |
| `workflows/` | your workflow files |
| `AGENTS.md`, `.agents/skills/` | instructions and the FX skills for coding agents |
| `runs/`, `.fx/` | run records and the cache, written by FX and kept out of git |

## Learn more

- [The guide](https://github.com/gridaco/fx/blob/main/docs/guide/README.md): workflows, nodes, cost
  and cache, running, viewing
- [Examples](https://github.com/gridaco/fx/tree/main/examples)
- Using an agent? It reads `AGENTS.md` and the FX skills in `.agents/skills/`; update them with
  `npx skills add gridaco/fx`
