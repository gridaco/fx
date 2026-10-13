# Providers

The providers FX speaks: where their keys and endpoints come from (§1), the adapters FX ships for them (§2), the route table FX ships (§3), and the feature names FX renamed (§4). How an adapter talks to its provider, and how each answer becomes an attempt's outcome and its bill, is the adapter contract, [adapters.md](adapters.md). What a capability's request and answer hold is [capabilities.md](capabilities.md).

## 1. Keys and endpoints

| Provider | Key | Base URL | Default base |
|---|---|---|---|
| `gg` | `GG_API_KEY` | `GG_BASE_URL` | `https://gateway.grida.ai/v1` |
| `openai` | `OPENAI_API_KEY` | `OPENAI_BASE_URL` | `https://api.openai.com/v1` |
| `openrouter` | `OPENROUTER_API_KEY` | `OPENROUTER_BASE_URL` | `https://openrouter.ai/api/v1` |
| `fal` | `FAL_KEY` | `FAL_BASE_URL` (the run host) | `https://fal.run`; queue `https://queue.fal.run` |
| `tripo` | `TRIPO_API_KEY` | none | `https://openapi.tripo3d.ai/v3` |
| `elevenlabs` | `ELEVENLABS_API_KEY` | `ELEVENLABS_BASE_URL` | `https://api.elevenlabs.io/v1` |

- **Only these eleven variables are read.** A variable comes from the process environment when it is set and not blank (trimmed). Otherwise it comes from the planning project's `.env` file, if there is one. `GRIDA_FX_DISABLE_DOTENV=1` turns the file off.
- **The `.env` file is optional**, and it is read for these eleven names only. Lines that name other variables are skipped without reading their values. It is refused when it is a symlink or not a regular file (`.env must be a regular file`), cannot be read (`.env could not be read`), or is not UTF-8 (`.env must be valid UTF-8`). Blank lines and `#` comments are skipped.
  - An assignment is `[export] NAME = value`. A line that starts with an allowed name and is not an assignment is refused: `.env contains malformed assignment for <NAME> on line <n>`.
  - An allowed name may appear only once: `.env contains duplicate key: <NAME>`.
  - A value may be unquoted (a trailing ` #` comment is removed, and a quote inside it is refused), `'single quoted'` (literal), or `"double quoted"` (a JSON string literal: `.env contains malformed quoted value for <NAME> on line <n>`). An empty value is refused: `.env contains an empty value for <NAME>`. A control character in the decoded value is refused: `.env contains an unsafe value for <NAME>`.
  - Every refusal names the variable and the line, never the value.
- **A base URL** is trimmed and loses a trailing `/`. It must be an `http(s)` URL with a host, no userinfo, no query, no fragment and a valid port, and `http` is allowed only for a loopback host. The refusals are `<Provider> base_url must be an HTTP(S) URL without credentials, query, or fragment`, `… must use a valid network port` and `… must use HTTPS unless it targets a loopback host`. A base URL that contains a key's value is refused: `<VARIABLE> holds a credential`. A base URL is configuration: it is not part of a route's fingerprint.
- **Key values are never printed.** `grida-fx doctor` says which keys are present and where each came from (environment or `.env`), never a value.

## 2. The built-in adapters

FX registers one adapter for each capability and provider below ([adapters.md](adapters.md) §1). The table is normative. A route's contract names the adapter that serves it as `adapter`, and its behaviour as `adapter_behavior`, written in JSON exactly as here: the string `"1"` and the number `1` are different contracts, with different fingerprints ([identity.md](identity.md) §7). Each adapter's reference, beside its code, is normative for that adapter under the adapter contract: its wire, refusals, answers, checks, statuses, costs, labels and limits. A reference's status rows override [adapters.md](adapters.md) §3.3 only where they say so.

| Provider | Capability | Contract `adapter` | `adapter_behavior` | Reference |
|---|---|---|---|---|
| `openai` | `image.generate` | `fx-openai-image-v1` | `"1"` | [OpenAI](../crates/grida-fx-providers/src/openai/README.md) |
| `openai` | `image.edit` | `fx-openai-image-v1` | `"1"` | [OpenAI](../crates/grida-fx-providers/src/openai/README.md) |
| `openrouter` | `image.generate` | `fx-openrouter-image-v1` | `"3"` | [OpenRouter](../crates/grida-fx-providers/src/openrouter/README.md) |
| `openrouter` | `image.edit` | `fx-openrouter-image-v1` | `"3"` | [OpenRouter](../crates/grida-fx-providers/src/openrouter/README.md) |
| `openrouter` | `structured.generate` | `openrouter-structured` | `1` | [OpenRouter](../crates/grida-fx-providers/src/openrouter/README.md) |
| `openrouter` | `agent.turn` | `openrouter-tool-loop` | `1` | [OpenRouter](../crates/grida-fx-providers/src/openrouter/README.md) |
| `openrouter` | `music.generate` | `openrouter-music` | `1` | [OpenRouter](../crates/grida-fx-providers/src/openrouter/README.md) |
| `fal` | `image.generate` | `fx-fal-image-v1` | `"1"` | [fal](../crates/grida-fx-providers/src/fal/README.md) |
| `fal` | `image.edit` | `fx-fal-image-v1` | `"1"` | [fal](../crates/grida-fx-providers/src/fal/README.md) |
| `fal` | `video.generate` | `fal-queue` | `1` | [fal](../crates/grida-fx-providers/src/fal/README.md) |
| `fal` | `background.remove` | `fal-run-birefnet` | `1` | [fal](../crates/grida-fx-providers/src/fal/README.md) |
| `tripo` | `mesh.generate` | `tripo-multiview` | `1` | [Tripo](../crates/grida-fx-providers/src/tripo/README.md) |
| `tripo` | `mesh.rig` | `tripo-rig` | `1` | [Tripo](../crates/grida-fx-providers/src/tripo/README.md) |
| `elevenlabs` | `sound.generate` | `elevenlabs-sound-effect` | `1` | [ElevenLabs](../crates/grida-fx-providers/src/elevenlabs/README.md) |
| `elevenlabs` | `speech.generate` | `elevenlabs-speech` | `1` | [ElevenLabs](../crates/grida-fx-providers/src/elevenlabs/README.md) |
| `codex` | `image.generate` | `fx-codex-image-v1` | `"2"` | [Codex](../crates/grida-fx-providers/src/codex/README.md) |
| `codex` | `image.edit` | `fx-codex-image-v1` | `"2"` | [Codex](../crates/grida-fx-providers/src/codex/README.md) |
| `gg` | `image.generate` | `gg-image` | `"1"` | [Grida Gateway](../crates/grida-fx-providers/src/gg/README.md) |
| `gg` | `image.edit` | `gg-image` | `"1"` | [Grida Gateway](../crates/grida-fx-providers/src/gg/README.md) |
| `gg` | `structured.generate` | `gg-structured` | `1` | [Grida Gateway](../crates/grida-fx-providers/src/gg/README.md) |
| `gg` | `agent.turn` | `gg-tool-loop` | `1` | [Grida Gateway](../crates/grida-fx-providers/src/gg/README.md) |
| `gg` | `video.generate` | `gg-video` | `1` | [Grida Gateway](../crates/grida-fx-providers/src/gg/README.md) |
| `gg` | `mesh.generate` | `gg-mesh` | `1` | [Grida Gateway](../crates/grida-fx-providers/src/gg/README.md) |
| `gg` | `mesh.rig` | `gg-rig` | `1` | [Grida Gateway](../crates/grida-fx-providers/src/gg/README.md) |
| `grok` | `image.generate` | `fx-grok-image-v1` | `"1"` | [Grok Build](../crates/grida-fx-providers/src/grok/README.md) |
| `grok` | `image.edit` | `fx-grok-image-v1` | `"1"` | [Grok Build](../crates/grida-fx-providers/src/grok/README.md) |

- An adapter refuses a route whose contract names another `adapter`, or another `adapter_behavior` than its row's, when present ([adapters.md](adapters.md) §4 step 1). fal's and Tripo's adapters check only `adapter`, for every capability they serve: the behaviour in their rows is the one the built-in routes carry, and a route that names another is still served. `background.remove` on fal has no route (§3); its behaviour is the integer `1`, as for every route outside images.
- The `codex` and `grok` adapters work through a runtime and bill the user's own plan ([adapters.md](adapters.md) §1, §5). Every other adapter bills dollars, to the account of its provider's key (§1).

## 3. The built-in route table

FX ships a default route table, embedded in `grida-fx`. It is the first table of the catalog ([identity.md](identity.md) §7): a project's `route_tables` override its entries, and any `--routes` file leaves it out.

- It holds 29 routes over 10 capabilities. Every route MUST be served by an adapter of §2, and every capability is defined in [capabilities.md](capabilities.md).
- **Prices are planning allowances** in US dollars, not provider quotes. What a call costs is settled from what the provider reports, or else from the whole hold ([adapters.md](adapters.md) §5). The video routes are priced per second, in tiers by `resolution`.
- **The Codex routes, `imagegen@codex` for `image.generate` and for `image.edit`, are free:** $0 to $0, untiered. They run on the user's own Codex plan, which FX neither sees nor bills ([Codex](../crates/grida-fx-providers/src/codex/README.md)). Their adapter bills the user's plan ([adapters.md](adapters.md) §5), so a live run whose calls are all on them needs no ceiling. They declare `image_input`, `flexible_size`, `alpha`, `auto_background`, `opaque_background` and `png_output`, and the `image.generate` route also `text_to_image`: the controls their adapter honours, so no exact size and no mask. Both take at most 5 input pictures.
- **The Grok Build routes, `imagine@grok` for `image.generate` and for `image.edit`, are free:** $0 to $0, untiered. They run on the user's own Grok subscription, which FX neither sees nor bills ([Grok Build](../crates/grida-fx-providers/src/grok/README.md)). Their adapter bills the user's plan ([adapters.md](adapters.md) §5), so a live run whose calls are all on them, or on them and the Codex routes, needs no ceiling. They declare `image_input`, `flexible_size`, `auto_background`, `opaque_background`, `jpeg_output` and `png_output` (Grok Build draws JPEG; the answer is the same pixels as a PNG), and the `image.generate` route also `text_to_image`: so no transparency, no exact size and no mask. Both take at most 3 input pictures.
- **The Grida Gateway routes (`@gg`) take the prices of the same model on another provider.** GG charges a model's regular tariff ([GG](../crates/grida-fx-providers/src/gg/README.md)), so `openai/gpt-image-2.5-sunburst@gg` has OpenAI's image tiers, the text routes OpenRouter's prices for the same models, `alibaba/wan-3.0@gg` $0.05 a second at `480p`, $0.10 to $0.20 at `720p` and $0.20 at `1080p`, up to 30 seconds, and the 3D routes Tripo's prices for the same models. A GG call is booked at GG's charge once it settles, which GG reports for every call; one whose charge has not settled in time is booked at its whole hold, as any call without a reported cost is ([adapters.md](adapters.md) §5).
- **The other image routes are priced per call, in tiers by `size`** ([identity.md](identity.md) §12). A tier is named by the exact size text, such as `1024x1024`. A request whose `size` no tier names (`auto`, none, or any other `<W>x<H>`) is priced at the route's whole range. The plan prices a step by its `size`, and each attempt's hold is the high price of the request's own tier ([protocol.md](protocol.md) §6.1 step 6).
  - A tier's low is the image output at quality max for that size, at $30 per million output tokens, rounded down to the cent. The output is measured or, where only a call's bill is known, derived from that bill less an estimate of its input. The estimate is the same for every such bill: the measured input of one comparable call, a prompt and one input picture ($0.006314). Each such bill is below every high of its tier. A tier's high adds an allowance for the input, rounded up to the cent: $0.07 for a generate's prompt, and $0.21 for an edit's 16 input pictures and long prompt.
  - The whole range contains every tier: $0.09 to $0.70 for a generate and to $0.85 for an edit, from $0.13 on OpenRouter.
  - **OpenAI's highs are what an OpenAI call is booked at.** OpenAI reports token usage but no cost ([OpenAI](../crates/grida-fx-providers/src/openai/README.md)), so every OpenAI image call is booked at its whole hold, and a high below what a call can cost would under-count it. Each OpenAI high leaves room for one image output at its size, measured or derived as above. The fal and GG routes serve the same model in the same envelope, and take the same prices.
  - **OpenRouter's highs cover twice the output.** OpenRouter reports its cost ([OpenRouter](../crates/grida-fx-providers/src/openrouter/README.md)), so its high is only the hold. It has billed a 1024x1024 generate $0.42164: twice the output the same size cost at OpenAI directly (2 × $0.21072 = $0.42144), plus its prompt. Its tiers name only the sizes it serves ([OpenRouter](../crates/grida-fx-providers/src/openrouter/README.md)).
  - **An open risk.** That one bill does not show whether OpenRouter counts the output twice or the model now draws twice as much. If the model does, an OpenAI call, and a fal call whose answer reports no cost, can cost one more output than its high, and is under-counted by up to that much: a 1024x1024 generate would cost about $0.42 and be booked at $0.29. So the OpenAI and fal highs are not known to be worst cases. OpenAI's usage report for a call made since that bill settles it: one output means the doubling is OpenRouter's; two mean these highs need the same room as OpenRouter's.
  - A project that knows its calls better restates a route in its own route table, with tighter prices ([identity.md](identity.md) §7).
- **Contracts are identity.** Each contract object enters its route fingerprint, and so every cache key the route serves. Changing a contract changes every key under it.
  - A contract's `adapter` is the name §2 gives the adapter that serves the route, and its `adapter_behavior` is the behaviour §2 names.
  - The image routes carry `adapter_behavior` as a string (`"1"`, `"3"`), and every other route carries it as the integer `1`. Each adapter serves its behaviour only as spelled here: the string and the number are different contracts, with different fingerprints.
  - The two `openai/gpt-6-astra@openrouter` routes carry the `request_policy` of [OpenRouter's structured and agent adapters](../crates/grida-fx-providers/src/openrouter/README.md), and the structured one also `pictures: unchanged`. The two `openai/gpt-6-astra@gg` routes carry the same choices in [GG's](../crates/grida-fx-providers/src/gg/README.md) members: `image_detail: high` and `reasoning_effort: high`, and the structured one also `pictures: unchanged`.
  - The GG image routes carry `quality: max`, which the [OpenAI adapter](../crates/grida-fx-providers/src/openai/README.md) always sends, the GG 3D routes the `endpoint` they submit to, and the GG rig route its `check_endpoint`.
- **Features** are the names [capabilities.md](capabilities.md) lists for each capability, and a route declares only what its adapter honours. So the OpenRouter image routes declare neither `alpha` nor `mask`, the GG image routes no `mask`, the GG video route no `first_last_frame` (it takes a first frame only), the GG mesh route no `quad`, and no `image.generate` route but Codex's and Grok Build's declares `image_input`, since every other one refuses references (§2).
- **Input-picture limits are declared.** A route that takes input pictures declares `max_input_pictures`, the most it takes in one call, as its adapter refuses more: 16 on the OpenAI, OpenRouter and fal `image.edit` routes, 5 on both Codex routes, and 3 on both Grok Build routes. The GG routes declare none: GG checks each model's own limits ([GG](../crates/grida-fx-providers/src/gg/README.md)). The plan refuses a step that would send more ([capabilities.md](capabilities.md) §1).
- **Pacing is not identity.** The engine applies a route's pacing per route id, so routes of one id for several capabilities share it. The OpenAI and OpenRouter image routes declare `requests_per_minute: 150`. The 3D routes and the video routes declare `concurrency: 1`, and so do the GG image routes: GG runs one GPT Image call per organization at a time until it settles, and answers a second with a 429 ([GG](../crates/grida-fx-providers/src/gg/README.md)). The Codex routes declare `concurrency: 3`, shared by their one id: one Codex runs several turns at once (five at once drew in about the time of one), and three keeps the user's plan from being spent faster than they would by hand; a project sets its own in `fx.yaml`. The Grok Build routes declare `concurrency: 3` likewise, on their one Grok Build. A call waiting for its slot is recorded as waiting ([store.md](store.md) §8). The structured routes declare `concurrency: 4`, and the `openai/gpt-6-astra` agent routes declare `concurrency: 1`.
- **Shipped without a route:** `background.remove` has an adapter on fal, but no route until fal's price for it is recorded. `vision.annotate`, `vision.review` and `structured.review` have no adapter. Calls on any of these are `no_route` unless a project's table adds a route.

## 4. Renamed features

A feature has one name. These names are not features, and a step that requires one is refused
while planning as no feature of its capability, naming FX's instead
([capabilities.md](capabilities.md) §1): `masked_edit` is `mask`, `transparent_background` is
`alpha`, and `reference_images` and `data_url_reference_input` are `image_input`.
