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
[Keys and live runs](05-running.md#keys-and-live-runs). A provider FX does not ship is declared
in `fx.yaml` with the variables of its own key and address
([below](#a-provider-fx-does-not-ship)).

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
([providers.md §2](../../spec/providers.md#2-the-built-in-adapters)), and the name picks the
adapter: a provider may have more than one for a capability. On a provider and capability FX
serves, FX fills in what a project route's contract leaves out: the provider's default for the
capability as `adapter`, and the `adapter_behavior` of the adapter the contract names. So such a
route always names its adapter. A contract that names an adapter FX does not have is served by
none: `grida fx doctor` says `no adapter`, and a live call on it is refused. Every adapter checks
its behaviour: up to 0.6.1, fal's and Tripo's checked only `adapter`, and served a route that
named another `adapter_behavior`.

## A model FX does not list

New models arrive faster than FX's built-in table. On a provider FX speaks, a route of your own is
enough: name the model, and give a cap when you do not know its price.

```yaml
# routes/models.yaml, listed under route_tables: in fx.yaml
fx: routes/v1
routes:
  - capability: image.generate
    route: acme/img-a@openrouter        # made up: any image model OpenRouter serves
    price: { cap_usd: 0.20 }            # each call holds $0.20 and is booked at what OpenRouter reports
    features: [image_input]
    max_input_pictures: 8
  - capability: image.edit
    route: acme/img-a/edit@fal          # made up: fal's edit endpoint of a model
    price: { cap_usd: 0.20 }
    contract:
      provider_options: { fal: { seed: 7 } }
```

- **OpenRouter and fal images.** A route whose `contract` names no `adapter` gets FX's generic
  image adapter for the provider. It sends what the provider's image models share, the prompt and the input
  pictures, then your `provider_options`, and nothing FX chose for one model: no size, quality or
  background. The picture it gets back becomes a PNG, the same pixels. On fal the model is the
  endpoint id as fal publishes it, the generate endpoint for `image.generate` and the edit
  endpoint for `image.edit`. An exact `size`, a `mask` or a transparent `background` is refused
  before anything is sent: leave it out and give the model's own parameter in `provider_options`
  ([Parameters FX does not expose](#parameters-fx-does-not-expose)).
- **GPT Image.** The built-in OpenRouter and fal image routes name GPT Image's own adapters,
  which send what FX chose for it: maximum quality, its exact sizes and, on OpenRouter, its
  moderation. A route that `extends` one of them keeps that adapter, so extend one for a GPT
  Image model FX does not list, and declare a route whose contract names no adapter for any other
  model.
- **GG.** A new model id is enough for images, video, structured output and agent turns: GG
  checks each model's own request. A 3D route also names GG's `endpoint` in its `contract` (and a
  rig route may name a `check_endpoint`), as the built-in GG 3D routes do. GG image edits take at
  most the pictures the route declares (`max_input_pictures`, 16 on the built-in route).
- **fal video.** A video route of your own on fal draws the `resolution` tiers its price names and
  up to its `max_units` seconds; a capped route, whose price states neither, draws `360p`, `720p`,
  `1080p` and `4k`, from 3 to 10 seconds. It needs a first frame unless it declares the feature
  `text_to_video` (`features: [text_to_video]`), for a model that draws from the prompt alone;
  the built-in route does not. It sends the built-in route's request (`image_url`,
  `end_image_url`, a whole-number `duration`, `aspect_ratio`, `resolution`), so it fits only a
  fal endpoint that takes that shape.

Each adapter's reference lists what it sends and refuses
([providers.md §2](../../spec/providers.md#2-the-built-in-adapters)).

## A provider FX does not ship

A provider FX has no adapter for is declared in `fx.yaml`, under `providers:`, by the name its
routes give it:

```yaml
# fx.yaml
fx: project/v1
providers:
  acme:
    credential: { variable: ACME_API_KEY }   # sent as authorization: Bearer <key>
    base_url: { variable: ACME_BASE_URL, default: "https://api.acme.test/v1" }
    download_hosts: [cdn.acme.test]
    family: openai-compatible
```

- **`credential`** names the variable its key comes from, and how the key is sent: `header`
  (`authorization` unless you say otherwise) and `scheme` (`Bearer`, or `""` for the bare key).
  Leave `credential` out for a server that needs no key, such as one on this machine.
- **`base_url`** is its address, `https` unless it is this machine (`localhost`, `127.0.0.1`),
  and the variable that may move it, as `OPENAI_BASE_URL` moves OpenAI's.
- **`download_hosts`** are the hosts its results may be downloaded from: a host, or `*.` and a
  domain for every host below it.
- **`family`** is the wire it speaks: `openai-compatible`, the OpenAI chat API.

FX reads a declared provider's key and address as it reads its own: from the environment, else
the project's `.env`. It redacts the key wherever it would be shown, keeps both variables from
Codex, Grok Build, the background service and the program that opens your browser, and
`grida fx doctor` lists the provider with its address and where its key came from:

```
provider  acme api.acme.test ACME_API_KEY present (.env)
```

A declared provider bills dollars, to its own account. Declarations are settings, not part of any
identity: changing one moves no cache. No adapter serves a declared provider yet, so its routes
plan and price, and a live call on one is refused (`no adapter serves …`; `no adapter` in
doctor).

A declaration may not take the name of a provider FX ships (`gg`, `openai`, `openrouter`, `fal`,
`tripo`, `elevenlabs`, `codex`, `grok`), a variable FX or another declaration already reads, or a
variable FX and the programs it starts read for themselves: `GRIDA_FX_*`, the proxy and certificate
variables, `PATH` and `HOME`. Each is refused when `fx.yaml` is read, naming the declaration
([providers.md §1](../../spec/providers.md#1-keys-and-endpoints)).

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
- **FX's own settings stay FX's.** A parameter FX sets itself (`prompt`, `model`, `n`, `stream`,
  …) is refused before anything is sent, at $0, and so is a file: send files through the
  type's inputs. Each provider's list is in its adapter's reference
  ([providers.md §2](../../spec/providers.md#2-the-built-in-adapters)). Codex and Grok Build take
  none.
- **Options are part of the call.** Another value is another paid call, and the route's are part of
  its identity. They are recorded with the run, so never put a key in them: keys come only from the
  environment or `.env`.
- **What the provider does with them is the provider's.** Beyond those refusals, FX does not
  check a parameter's name or value: a provider that refuses it fails the call, and one that
  changes its answer so FX's checks refuse it fails the attempt, billed.
