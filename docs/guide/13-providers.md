# AI providers

FX runs a workflow's own code with no provider at all. A step that calls an AI model (an image, a
clip, a 3D model, structured text) runs on a **route**, `model@provider`, and a live run needs
that provider's key, or its program for Codex and Grok Build. This page lists the ways to set one up and what each
one serves.

| Path | What you set up | What it serves | Who bills |
|---|---|---|---|
| No provider | nothing | your own code and the built-in local steps | nobody |
| [Your own Codex](#your-own-codex) | Codex, signed in to ChatGPT | images | your ChatGPT plan; $0 to FX |
| [Your own Grok Build](#your-own-grok-build) | Grok Build, signed in to a Grok subscription | images | your Grok subscription; $0 to FX |
| [Grida Gateway](#grida-gateway) | `GG_API_KEY` | text, agent turns, images, video, 3D models and rigs | your organization's Grida AI credits |
| [Your own keys](#your-own-provider-keys) | each provider's key | that provider's routes | each provider's account |

You can mix them: a project picks a route per capability in `fx.yaml`, and a step can name its own
`route:`. `grida fx doctor` says which keys FX found, where, and which routes a live run could
serve ([Running](05-running.md#keys-and-live-runs)).

## No provider

FX is a workflow engine first. Your own nodes, in Python or TypeScript, the built-in local steps
(`image.resize`, `json.merge`, `package`, …) and file facts run with no key and without `--live`,
and a run of them is cached and recorded like any other. Without `--live`, a step that would call
a provider is refused for nothing, so you can build and test the rest of a workflow first, or
answer paid calls with [stand-ins](05-running.md#stand-ins-testing-without-a-provider).

## Your own Codex

With [Codex](https://developers.openai.com/codex) installed and signed in to your ChatGPT account,
the route `imagegen@codex` draws and edits images with no API key. Codex counts each image against
your plan; FX books it at $0, and a run whose calls are all on Codex needs no `--max-usd`.

It serves images only (`image.generate` and `image.edit`), and Codex picks the model and the
quality. Setup and caveats, sizes above all: [Images from your own Codex](12-codex.md).

## Your own Grok Build

With [Grok Build](https://x.ai/cli) installed and signed in to a Grok account whose subscription
includes image generation, the route `imagine@grok` draws and edits images with no API key. Grok
Build counts each image against your subscription; FX books it at $0, and a run whose calls are all
on Grok Build (or on it and Codex) needs no `--max-usd`.

It serves images only (`image.generate` and `image.edit`), opaque ones, and Grok Build picks the
model and the size. Setup and caveats: [Images from your own Grok Build](14-grok.md).

## Grida Gateway

One key serves every route on Grida Gateway (`@gg`). Create it at
[console.grida.co](https://console.grida.co/_/ai-gateway), where you also add credits and see
what each call was charged, then set it:

```sh
GG_API_KEY=…
```

in the environment, or in a `.env` file in your project (keep it out of version control). FX books
a GG call at what GG charged for it ([GG's adapter reference](../../crates/grida-fx-providers/src/gg/README.md)).

The key needs both scopes, `text` for text and agent steps and `media` for everything else.
GG's own API reference, written for people and agents alike, is
[gateway.grida.ai/llms.txt](https://gateway.grida.ai/llms.txt): key scopes, credits, spending
caps and limits.

GG serves text and agent turns, images, video, and 3D models and rigs. It does not serve sound,
speech or music: those routes need their provider's own key (below).

## Your own provider keys

Every other route takes its provider's key under its usual name. Set only the ones your routes use;
a missing key refuses only the calls that need it, before anything is sent.

| Provider | Variable | Built-in routes for | Get a key |
|---|---|---|---|
| OpenAI | `OPENAI_API_KEY` | images | [platform.openai.com](https://platform.openai.com/api-keys) |
| OpenRouter | `OPENROUTER_API_KEY` | text, agent turns, images, music | [openrouter.ai](https://openrouter.ai/settings/keys) |
| fal | `FAL_KEY` | images, video | [fal.ai](https://fal.ai/dashboard/keys) |
| Tripo | `TRIPO_API_KEY` | 3D models and rigs | [platform.tripo3d.ai](https://platform.tripo3d.ai/api-keys) |
| ElevenLabs | `ELEVENLABS_API_KEY` | sound effects, speech | [elevenlabs.io](https://elevenlabs.io/app/settings/api-keys) |

`OPENAI_BASE_URL`, `OPENROUTER_BASE_URL`, `FAL_BASE_URL`, `ELEVENLABS_BASE_URL` and `GG_BASE_URL`
point a provider at another address, such as a proxy. How FX reads keys and the `.env` file:
[Keys and live runs](05-running.md#keys-and-live-runs).

## The built-in routes

| Capability | Grida Gateway | Codex, Grok Build | Your own keys |
|---|---|---|---|
| `structured.generate`, `agent.turn` | ✓ | | OpenRouter |
| `image.generate`, `image.edit` | ✓ | ✓ | OpenAI, OpenRouter, fal |
| `video.generate` | ✓ | | fal |
| `mesh.generate`, `mesh.rig` | ✓ | | Tripo |
| `music.generate` | | | OpenRouter |
| `sound.generate`, `speech.generate` | | | ElevenLabs |

The routes themselves, with their models and planning prices, are in FX's built-in route table:
`grida fx nodes <type>` lists a capability's routes, and `grida fx doctor` says which ones a live
run could serve. A project can add or restate routes in its own `route_tables:`
([where prices come from](04-cost-and-cache.md#before-spending-the-plan)). An entry that
`extends:` a built-in route states only what differs, a price say, and keeps the rest, contract
included; restating the same route this way keeps its identity, so its cached results stay
([route tables](05-running.md#name-a-run-or-continue-it)).

A route's `contract` names the adapter that serves it, as `adapter` and `adapter_behavior`
([providers.md §2](../../spec/providers.md#2-the-built-in-adapters)). On a provider and
capability FX serves, FX fills in what a project route's contract leaves out: its adapter's name
as `adapter`, and that adapter's `adapter_behavior` unless the contract names another adapter.
So such a route always names its adapter. Every adapter checks both: up to 0.6.1, fal's and
Tripo's checked only `adapter`, and served a route that named another `adapter_behavior`.

## Parameters FX does not expose

A provider often takes parameters FX has no setting for: a seed, a style, a safety switch. Give
them in `provider_options`, by provider, on a step or in a route's `contract`:

```yaml
steps:
  facts:
    uses: fx/structured.generate@1
    with:
      prompt: Name a colour.
      schema: ./schemas/colour.json
      provider_options:
        openrouter: { seed: 7 }
```

- **Only the provider the call runs on reads its member.** The others are kept, so a step that may
  run on several routes can give each provider its own. The parameters go in the request after
  FX's own, as the provider's API names them.
- **A route can carry them too**, in its `contract` (`contract: { provider_options: { openrouter:
  { seed: 7 } } }`). A step's parameter replaces the route's of the same name, and adds the rest.
  An agent's turns take only their route's, since FX builds each turn's request itself.
- **FX's own settings stay FX's.** A parameter FX sets itself (`prompt`, `size`, `model`, `n`,
  `stream`, …) is refused before anything is sent, at $0, and so is a file: send files through the
  type's inputs. Each provider's list is in its adapter's reference
  ([providers.md §2](../../spec/providers.md#2-the-built-in-adapters)). Codex and Grok Build take
  none.
- **Options are part of the call.** Another value is another paid call, and the route's are part of
  its identity. They are recorded with the run, so never put a key in them: keys come only from the
  environment or `.env`.
- **What the provider does with them is the provider's.** Beyond those refusals, FX does not
  check a parameter's name or value: a provider that refuses it fails the call, and one that
  changes its answer so FX's checks refuse it fails the attempt, billed.
