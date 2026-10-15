# FX

**Go with the flow.**

![A finished run open in the FX viewer: its steps on a canvas, with each step's pictures](.github/assets/cover.webp)

A workflow engine for images, audio, video, 3D and the code around them.

## Why FX

A build system for generated assets.

- **Priced before it spends.** The whole run is priced up front, and a live run reserves each call
  against its ceiling before making it.
- **Never pays twice.** Results and paid calls are cached by content, and an interrupted run picks
  up where it stopped.
- **Judged, not just generated.** Steps can judge each other's output, draw again when rejected,
  and let you pick the take you want.
- **Any model, or your own subscription.** Switch providers per step, or draw on your Codex or
  Grok plan.
- **One native engine.** No Docker, no server: YAML in git, from your laptop to CI.

## Install

```sh
npm install -g grida     # FX as `grida fx` (Node.js 24+; macOS, Linux, Windows through WSL 2)
pip install grida        # the Python SDK, for nodes in Python
npm install @grida/fx    # the TypeScript SDK, for nodes in TypeScript
```

To run FX without the Grida CLI, see [ways to run FX](CONTRIBUTING.md#ways-to-run-fx).

## AI providers

FX runs your own code with no provider at all. For AI steps, pick a route per capability or per
step, and mix them freely:

| | Set up | You get |
|---|---|---|
| **No provider** | nothing | your own code, cached and recorded |
| **BYOA** (bring your own agent) | Codex or Grok Build, signed in | images on your ChatGPT or Grok subscription, $0 to FX |
| **Provider keys** | `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `FAL_KEY`, `TRIPO_API_KEY`, `ELEVENLABS_API_KEY` | each provider's models, on its own account |
| **Grida Gateway** | `GG_API_KEY` | text, images, video and 3D with one key and one bill |

[AI providers](docs/guide/13-providers.md) says what each one serves and where to get each key. A
model FX does not list runs from a route of your own; a provider FX does not ship, from a
declaration in `fx.yaml` and, unless it speaks OpenAI's chat API, an adapter of your own in Python
or TypeScript ([Community providers](docs/guide/15-community-providers.md)).

## Plan. Run. Inspect.

Start a project, then pick a route for its first workflow in `fx.yaml`:

```sh
npx grida fx init my-assets                  # a starter project, with files for coding agents
cd my-assets
grida fx start --background                  # the local viewer
grida fx plan icon --open                    # the steps and their price; spends nothing
grida fx run icon --live --max-usd 1 --open
```

Without `--live`, nothing is spent: local steps run and cached results are reused. Start with
[the guide](docs/guide/01-getting-started.md), or the [examples](examples/).

### For agents

```sh
npx skills add gridaco/fx
```

[`llms.txt`](llms.txt) is the reference for agents: the whole of FX on one page, checked against
the engine.

## Learn more

- [The guide](docs/guide/README.md): workflows, nodes, cost and cache, running, viewing, sharing
- [Python SDK](python/README.md) and [JavaScript SDK](js/fx/README.md)
- [Specifications](spec/): the language-neutral contracts

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) for source builds and checks, and
[the overview](docs/wg/overview.md) for the project's direction. FX is open source under
[Apache-2.0](LICENSE).
