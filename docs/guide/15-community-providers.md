# Community providers

A **community provider** is a provider your project declares in `fx.yaml`, not one FX ships. FX
ships Grida Gateway, OpenAI, OpenRouter, fal, Tripo and ElevenLabs, and the Codex and Grok Build
runtimes ([AI providers](13-providers.md)). Any other provider (a vendor FX does not know, a
company's own gateway, a model server on your machine) becomes one of yours: you declare it, give
it routes, and either name the wire it speaks or write its adapter in Python or TypeScript. FX
keeps the rest: it checks each request and answer, holds the key, reserves every call against
your ceiling, retries, caches and records, as it does for its own providers.

This page is the whole job, end to end. The examples use `acme`, a made-up provider; every file
you would write is shown in full, and every output is the engine's.

## Choose the path

| You want | You write | Where |
|---|---|---|
| A model FX does not list, on GG, OpenRouter or fal | a route | [below](#a-new-model-needs-only-a-route) |
| A provider that speaks the OpenAI Chat Completions API, such as a model server on this machine | a declaration with `family: openai-compatible`, and routes | [An OpenAI-compatible server](#an-openai-compatible-server) |
| Any other provider | a declaration, routes, and an adapter in Python or TypeScript | [Your own adapter](#your-own-adapter) |

Codex and Grok Build are runtimes FX starts, signed in to your own plan: they take no declaration
and no adapter of yours.

### A new model needs only a route

When the provider is one FX speaks, a new model is a route of your own, not a provider. Name the
model, and give a cap where you do not know its price:

```yaml
# fx.yaml
fx: project/v1
route_tables: [routes/models.yaml]
```

```yaml
# routes/models.yaml
fx: routes/v1
routes:
  - capability: structured.generate
    route: acme/text-b@openrouter         # made up: a chat model OpenRouter serves
    price: { cap_usd: 0.10 }
  - capability: image.generate
    route: acme/img-a@openrouter          # made up: an image model OpenRouter serves
    price: { cap_usd: 0.20 }
    features: [image_input]
    max_input_pictures: 8
    contract:
      provider_options: { openrouter: { seed: 7 } }
```

```json
// schemas/colour.json
{ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }
```

```yaml
# workflows/lantern.yaml
fx: workflow/v1
id: lantern
title: A lantern, on models FX does not list
steps:
  colour:
    uses: fx/structured.generate@1
    route: acme/text-b@openrouter
    with:
      prompt: Name a colour for a copper lantern.
      schema: ./schemas/colour.json
  draw:
    uses: fx/image.generate@1
    route: acme/img-a@openrouter
    with:
      prompt: "A copper lantern, glowing ${{ steps.colour.outputs.json.name }}, flat icon."
outputs:
  icon: ${{ steps.draw.outputs.image }}
```

```sh
grida fx plan lantern --max-usd 1
```

```
lantern  ·  1 phase
phase 1   2 steps   2 provider calls   $0.00 – $0.30
cached    0 of 1 known steps
estimate  $0.00 – $0.30   ceiling $1.00
note      image.generate on acme/img-a@openrouter has no price: each call holds its cap, $0.20, and is charged what the provider reports, else the cap
note      structured.generate on acme/text-b@openrouter has no price: each call holds its cap, $0.10, and is charged what the provider reports, else the cap
```

`grida fx doctor lantern` then says whether a live run could serve each route
(`uses      image.generate acme/img-a@openrouter no key (OPENROUTER_API_KEY) (1 step)`, then
`servable` once the key is set). What each provider's adapter sends for a model FX does not list,
and when to `extends:` a built-in route instead:
[A model FX does not list](13-providers.md#a-model-fx-does-not-list). Caps:
[Cost, cache and takes](04-cost-and-cache.md#before-spending-the-plan).

## Declare the provider

A community provider is declared under `providers:` in the `fx.yaml` of the project you plan
from, by the name its routes give it (`<model>@acme`):

```yaml
providers:
  acme:
    credential: { variable: ACME_API_KEY, header: authorization, scheme: Bearer }
    base_url: { variable: ACME_BASE_URL, default: "https://api.acme.test/v1" }
    download_hosts: [files.acme.test, "*.cdn.acme.test"]
    family: openai-compatible      # or adapter: <module>#<name>
```

| Member | |
|---|---|
| the name | `[a-z0-9._-]+`, the provider part of its routes |
| `credential` | `variable` holds the key. The key is sent as `<header>: <scheme> <key>`: `header` is `authorization` unless you say otherwise, lowercase; `scheme` is `Bearer`, and `""` sends the key bare (`header: x-api-key, scheme: ""` sends `x-api-key: <key>`). Leave `credential` out for a server that needs no key. |
| `base_url` | `default` is required: `https`, unless it names this machine (`localhost`, `127.0.0.1`), with no userinfo, query or fragment. `variable`, optional, moves it, as `OPENAI_BASE_URL` moves OpenAI. |
| `download_hosts` | the hosts its results may be downloaded from: a host, or `*.` and a domain for every host below that domain (not the domain itself). None when left out. |
| `family` | the wire it speaks: `openai-compatible`, the one family. |
| `adapter` | your own adapter, `<path>#<name>`: a module of the project and the name it declares the adapter under. A declaration has a `family` or an `adapter`, not both. |

- **Keys.** FX reads the declared variables as it reads its own: from the environment, else the
  project's `.env` (only the declared names are read from it). Keys never go in `fx.yaml`, a
  route table or a workflow. FX redacts the key wherever it would be shown, and keeps both
  variables from Codex, Grok Build, the background service and the program that opens your
  browser. Your node hosts inherit FX's environment, so a variable set there reaches them; one
  read only from `.env` does not.
- **Billing.** A community provider bills dollars, to the account of its key. No declaration can
  claim a plan, as Codex and Grok Build do, so a live run on it always needs a ceiling, even when
  each of its routes is free.
- **Neither family nor adapter.** Its routes plan and price, and a live call on them is refused
  (`no adapter serves …`; `no adapter` in doctor).
- **Not identity.** A declaration is a setting: changing the key's variable, the header or the
  address moves no cache. Only `family` and `adapter` reach a route's identity, through the
  contract they fill in ([Identity and the cache](#identity-and-the-cache)).

`grida fx doctor` lists each declaration with its address and where its key came from, or
`needs no key`:

```
provider  acme api.acme.test ACME_API_KEY missing
provider  local 127.0.0.1:8080 needs no key
```

Each of these is refused when `fx.yaml` is read (exit 2), naming the declaration:

| Refused | Sentence |
|---|---|
| a name FX ships (`gg`, `openai`, `openrouter`, `fal`, `tripo`, `elevenlabs`, `codex`, `grok`) | `fx.yaml: providers.<name>: <name> is a provider FX ships` |
| a variable FX or another declaration already reads, in any letter case | `fx.yaml: providers.<name>.credential.variable: <VARIABLE> is already read for <provider>` (or `.base_url.variable`) |
| `GRIDA_FX_*`, `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`, `SSL_CERT_FILE`, `SSL_CERT_DIR`, `PATH`, `HOME` | `… <VARIABLE> is reserved for FX and the programs it starts` |
| a default address that breaks the rule above | `fx.yaml: providers.<name>.base_url.default: <name> base_url must use HTTPS unless it targets a loopback host` (or `must be an HTTP(S) URL without credentials, query, or fragment`, `must use a valid network port`) |
| both a family and an adapter | `fx.yaml: providers.<name>: a provider has a family or an adapter, not both` |

The normative rules: [providers.md §1](../../spec/providers.md#1-keys-and-endpoints).

## Routes for it

A community provider's routes go in a route table of your own (`fx: routes/v1`, listed under
`route_tables:` in `fx.yaml`), one entry per capability and model, as for any provider:

```yaml
# routes/acme.yaml
fx: routes/v1
routes:
  - capability: structured.generate
    route: text-a@acme                           # <model>@<provider>, the model as acme names it
    price: { low_usd: 0.001, high_usd: 0.02 }   # what a call may cost
  - capability: video.generate
    route: clip-a@acme
    price: { cap_usd: 1.00 }                    # no known price: each call holds $1
    features: [text_to_video]
    concurrency: 2                              # at most 2 calls at once
    contract:
      provider_options: { acme: { style: flat } }
```

- **`price` or a cap.** A price is what a call may cost, `usd`, or `low_usd` and `high_usd`
  (`usd: 0` for a free one, a model on your machine say). When you do not know it, give
  `cap_usd: N`, alone in `price`: each call holds N against your ceiling and is charged what the
  provider reports, else N. A route with neither is refused; an empty `price` is not $0.
- **`features`** say what the route can do, and planning refuses a step that needs one the route
  lacks: references need `image_input`, an exact `size` needs `exact_size`, a transparent
  `background` needs `alpha`, a `mask` needs `mask`, and a step's `requires:` names its own. Each
  capability's names are in [capabilities.md](../../spec/capabilities.md). `max_input_pictures`
  bounds a step's pictures while planning.
- **`contract.provider_options`** passes the provider's own parameters with every call of the
  route, under the provider's name; a step adds its own in its `with:` ([Parameters FX does not
  expose](13-providers.md#parameters-fx-does-not-expose)). They are part of the route's identity.
- **`concurrency`** and **`requests_per_minute`** pace the route's calls, whichever steps send
  them, to stay under the provider's rate limits.
- **`extends`** names an earlier route of the same capability and provider and states only what
  differs, as for a built-in route ([Name a run or continue
  it](05-running.md#name-a-run-or-continue-it)).

A step names the route (`route: text-a@acme`), or `fx.yaml` makes it the project's default for its
capability (`routes: { structured.generate: text-a@acme }`). `grida fx nodes <type>` lists your
routes beside FX's.

## An OpenAI-compatible server

A server that speaks the OpenAI Chat Completions API needs a declaration and routes, and no code:
`family: openai-compatible` tells FX to serve its `structured.generate` and `agent.turn` routes
with its own family adapters. Here `acme` is a hosted service with a key, and `local` a model server
on this machine, which needs none:

```yaml
# fx.yaml
fx: project/v1
route_tables: [routes/acme.yaml]
providers:
  acme:                                      # its routes are <model>@acme
    credential: { variable: ACME_API_KEY }   # sent as authorization: Bearer <key>
    base_url: { variable: ACME_BASE_URL, default: "https://api.acme.test/v1" }
    family: openai-compatible
  local:                                     # a server on this machine: no key
    base_url: { default: "http://127.0.0.1:8080/v1" }
    family: openai-compatible
```

```yaml
# routes/acme.yaml
fx: routes/v1
routes:
  - capability: structured.generate
    route: acme/text-a@acme          # the model id as the provider names it
    price: { cap_usd: 0.05 }         # no cost is read: each call is booked at its cap
  - capability: agent.turn
    route: acme/text-a@acme
    price: { cap_usd: 0.05 }
  - capability: structured.generate
    route: small-a@local
    price: { usd: 0 }                # your own machine: free
```

```json
// schemas/colour.json
{ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }
```

```yaml
# workflows/colour.yaml
fx: workflow/v1
id: colour
title: A colour from a community provider
steps:
  name:
    uses: fx/structured.generate@1
    route: acme/text-a@acme
    with:
      prompt: Name a colour for a copper lantern.
      schema: ./schemas/colour.json
  draft:
    uses: fx/structured.generate@1
    route: small-a@local
    with:
      prompt: Name a colour for a copper lantern.
      schema: ./schemas/colour.json
outputs:
  colour: ${{ steps.name.outputs.json }}
  draft: ${{ steps.draft.outputs.json }}
```

```sh
grida fx plan colour
grida fx doctor colour
```

```
colour  ·  1 phase
phase 1   2 steps   2 provider calls   $0.00 – $0.05
cached    0 of 2 known steps
estimate  $0.00 – $0.05
note      structured.generate on acme/text-a@acme has no price: each call holds its cap, $0.05, and is charged what the provider reports, else the cap
```

```
uses      structured.generate acme/text-a@acme no key (ACME_API_KEY) (1 step)
uses      structured.generate small-a@local servable (1 step)
…
provider  acme api.acme.test ACME_API_KEY missing
provider  local 127.0.0.1:8080 needs no key
route     agent.turn acme/text-a@acme no key (ACME_API_KEY)
route     structured.generate acme/text-a@acme no key (ACME_API_KEY)
route     structured.generate small-a@local servable
```

- **What is sent.** One `POST <base>/chat/completions` per call: the model, the messages,
  `max_tokens` (the step's, else 16,000 for structured output), a strict `json_schema` response
  format (strict function tools and `tool_choice` for an agent turn), and the provider's member of
  `provider_options`, and nothing chosen for one provider. The key as declared, or none.
- **No cost is read.** Servers report usage differently, or not at all, so each call is booked at
  its whole reservation: the cap, or the route's high price. The tokens a reply counts are kept
  with the call. Give each route a `cap_usd`, or a price you trust.
- **Statuses** are FX's defaults: a 408 or a 429 is sent again, unbilled; any other 4xx fails the
  call, billed; a 5xx is billed and tried again.
- **Other capabilities.** The family serves only these two. A route of another capability on a
  family provider has no adapter: `route     image.generate img-a@acme no adapter` in doctor,
  and `no adapter serves image.generate on img-a@acme` when a call is sent. For those, write an
  adapter in place of the family: a declaration has one or the other, so the adapter then serves
  the chat capabilities too.
- **The contract** of a family route takes only `adapter`, `adapter_behavior` and
  `provider_options`. Another member is refused when a call is sent: `<route> is not a
  structured.generate route this adapter serves: the contract takes no member <member>`.

The [OpenAI-compatible reference](../../crates/grida-fx-providers/src/openai_compatible/README.md)
lists what the family sends and the sentences it refuses with.

## Your own adapter

For any other provider, write its **adapter**: a module of your project, in Python or TypeScript,
that turns FX's request into the provider's and the provider's answer into FX's. FX loads it in a
node host of its language, as it loads your nodes, and keeps everything else.

### A complete adapter in Python

`acme` here answers structured output on one request, and makes videos as a long job: submitted
once, then collected.

```yaml
# fx.yaml
fx: project/v1
route_tables: [routes/acme.yaml]
providers:
  acme:                                      # its routes are <model>@acme
    credential: { variable: ACME_API_KEY }   # attached by FX when the adapter asks for it
    base_url: { variable: ACME_BASE_URL, default: "https://api.acme.test/v1" }
    download_hosts: [files.acme.test]        # where its videos may be downloaded from
    adapter: adapters/acme.py#acme           # <module>#<name>, a module of this project
```

```yaml
# routes/acme.yaml
fx: routes/v1
routes:
  - capability: structured.generate
    route: text-a@acme
    price: { low_usd: 0.001, high_usd: 0.02 }   # acme's published price
  - capability: video.generate
    route: clip-a@acme
    price: { cap_usd: 1.00 }                    # no known price: each call holds $1
    features: [text_to_video]                   # it draws from the prompt alone
    contract:
      provider_options: { acme: { style: flat } }
```

```python
# adapters/acme.py
import asyncio
import json

from grida.fx import (
    Accepted,
    Answered,
    Ended,
    Failed,
    FetchError,
    NotReceived,
    Refused,
    Uncertain,
    Unreachable,
    adapter,
)


def options(call):
    """This provider's provider_options: the route's, then the step's, which win."""
    provider = call.route.provider
    merged = dict((call.route.contract.get("provider_options") or {}).get(provider) or {})
    merged.update((call.request.get("provider_options") or {}).get(provider) or {})
    return merged


async def send(call):  # structured.generate: one request, sent once
    schema = call.request["schema"]
    if not isinstance(schema, dict):  # a schema file
        schema = json.loads(schema.read_bytes())
    body = {"model": call.route.model, "prompt": call.request["prompt"], "schema": schema}
    extra = options(call)
    if extra.keys() & body.keys():  # the adapter's own members stay its own: refused, $0
        return Refused("provider_options name a member the acme adapter sets")
    body |= extra
    try:
        response = await call.fetch(
            f"{call.base_url}/answers", method="POST", json=body, credential=True, timeout_s=120
        )
    except FetchError as error:
        if error.phase == "refused":  # stopped before it left: $0
            return Refused(error.reason)
        if error.phase == "not_sent":  # provably never left: FX sends it again, unbilled
            return NotReceived(error.reason)
        return Failed(error.reason, retryable=True)  # it may have arrived: billed
    if response.status == 429:  # acme took nothing
        return NotReceived("acme is busy (HTTP 429)")
    if response.status == 400:  # acme documents a refused request as unbilled
        return Failed("acme refused the request (HTTP 400)", retryable=False, cost_usd=0)
    if response.status != 200:
        return Failed(f"acme returned HTTP {response.status}", retryable=response.status >= 500)
    answer = response.json()
    return Answered.json(answer["json"], cost_usd=answer.get("cost_usd"))


async def submit(call):  # video.generate: the paid request, sent once
    body = {"model": call.route.model, "prompt": call.request["prompt"]}
    if call.request.get("duration") is not None:
        body["seconds"] = call.request["duration"]
    extra = options(call)
    if extra.keys() & body.keys():
        return Refused("provider_options name a member the acme adapter sets")
    body |= extra
    try:
        response = await call.fetch(
            f"{call.base_url}/videos", method="POST", json=body, credential=True, timeout_s=60
        )
    except FetchError as error:
        if error.phase == "refused":
            return Refused(error.reason)
        if error.phase == "not_sent":
            return NotReceived(error.reason)
        return Uncertain(error.reason)  # acme may have taken the job
    if response.status == 202:
        return Accepted({"id": response.json()["id"]})  # what collect needs, never a key
    if response.status == 429:
        return NotReceived("acme is busy (HTTP 429)")
    if 400 <= response.status < 500:
        return Failed(f"acme refused the video (HTTP {response.status})", retryable=False)
    return Uncertain(f"acme returned HTTP {response.status}")


async def collect(call):  # polls with its own clock; a status read bills nothing
    loop = asyncio.get_running_loop()
    deadline = loop.time() + 600
    while loop.time() < deadline:
        try:
            response = await call.fetch(
                f"{call.base_url}/videos/{call.handle['id']}", credential=True, timeout_s=30
            )
        except FetchError:
            response = None  # a poll that saw nothing
        if response is not None and response.status == 200:
            job = response.json()
            if job["status"] == "failed":
                return Ended("acme could not make the video", cost_usd=job.get("cost_usd"))
            if job["status"] == "done":
                try:
                    clip = await call.fetch(job["url"], lane="download", timeout_s=120)
                except FetchError as error:
                    return Unreachable(error.reason)  # a later run collects it again
                if clip.status != 200:
                    return Unreachable(f"acme's video download returned HTTP {clip.status}")
                return Answered(files={"video": clip.body}, cost_usd=job.get("cost_usd"))
        await asyncio.sleep(5)
    return Unreachable("acme's video is still being made")  # a later run collects it again


acme = adapter(
    requests=["structured.generate"],
    jobs=["video.generate"],
    send=send,
    submit=submit,
    collect=collect,
    version=1,
)
```

```json
// schemas/colour.json
{ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }
```

```yaml
# workflows/lantern.yaml
fx: workflow/v1
id: lantern
title: A lantern from a community provider
steps:
  colour:
    uses: fx/structured.generate@1
    route: text-a@acme
    with:
      prompt: Name a colour for a copper lantern.
      schema: ./schemas/colour.json
  clip:
    uses: fx/video.generate@1
    route: clip-a@acme
    with:
      prompt: A copper lantern on a ship's rail, slowly turning.
      duration: 5
      provider_options:
        acme: { loop: true }   # read only by the adapter of the acme routes
outputs:
  colour: ${{ steps.colour.outputs.json }}
  clip: ${{ steps.clip.outputs.video }}
```

The project needs Python 3.11 or later with `grida` (`pip install grida`, or the project's
`.venv`; `GRIDA_FX_PYTHON` names another), since planning loads the adapter
([When FX needs Python](01-getting-started.md#when-fx-needs-python)).

### The same adapter in TypeScript

In TypeScript the module exports the adapter, and its outcomes are plain objects named by
`outcome`. The declaration names the export (`adapter: adapters/acme.ts#acme`); the routes, schema
and workflow above stay as they are.

```json
// package.json
{ "type": "module", "private": true, "dependencies": { "@grida/fx": "0.7.1" } }
```

```yaml
# fx.yaml
fx: project/v1
route_tables: [routes/acme.yaml]
providers:
  acme:
    credential: { variable: ACME_API_KEY }
    base_url: { variable: ACME_BASE_URL, default: "https://api.acme.test/v1" }
    download_hosts: [files.acme.test]
    adapter: adapters/acme.ts#acme
```

```ts
// adapters/acme.ts
import {
  type AdapterCall,
  AdapterFile,
  type CollectCall,
  type CollectOutcome,
  type JsonValue,
  type SendOutcome,
  type SubmitOutcome,
  adapter,
  Answered,
  FetchError,
} from "@grida/fx";

type Members = { [name: string]: JsonValue };
const OWN = "provider_options name a member the acme adapter sets";

/** This provider's provider_options: the route's, then the step's, which win. */
function options(call: AdapterCall): Members {
  const provider = call.route.provider;
  const mine = (all: unknown) => (all as Record<string, Members> | undefined)?.[provider];
  return { ...mine(call.route.contract.provider_options), ...mine(call.request.provider_options) };
}

/** structured.generate: one request, sent once. */
async function send(call: AdapterCall): Promise<SendOutcome> {
  let schema = call.request.schema;
  if (schema instanceof AdapterFile) {
    schema = JSON.parse(new TextDecoder().decode(await schema.bytes())); // a schema file
  }
  const body = { model: call.route.model, prompt: String(call.request.prompt), schema };
  const extra = options(call);
  // the adapter's own members stay its own: refused, $0
  if (Object.keys(extra).some((name) => name in body)) return { outcome: "refused", reason: OWN };
  let response;
  try {
    response = await call.fetch(`${call.baseUrl}/answers`, {
      method: "POST",
      json: { ...body, ...extra } as JsonValue,
      credential: true,
      timeoutS: 120,
    });
  } catch (error) {
    if (!(error instanceof FetchError)) throw error; // an unknown outcome: billed in full
    // stopped before it left: $0
    if (error.phase === "refused") return { outcome: "refused", reason: error.reason };
    // provably never left: FX sends it again, unbilled
    if (error.phase === "not_sent") return { outcome: "not_received", reason: error.reason };
    // it may have arrived: billed
    return { outcome: "failed", reason: error.reason, retryable: true };
  }
  if (response.status === 429) {
    return { outcome: "not_received", reason: "acme is busy (HTTP 429)" }; // acme took nothing
  }
  if (response.status === 400) {
    // acme documents a refused request as unbilled
    const reason = "acme refused the request (HTTP 400)";
    return { outcome: "failed", reason, retryable: false, costUsd: 0 };
  }
  if (response.status !== 200) {
    const reason = `acme returned HTTP ${response.status}`;
    return { outcome: "failed", reason, retryable: response.status >= 500 };
  }
  const answer = response.json() as { json: JsonValue; cost_usd?: number };
  return Answered.json(answer.json, { costUsd: answer.cost_usd });
}

/** video.generate: the paid request, sent once. */
async function submit(call: AdapterCall): Promise<SubmitOutcome> {
  const body: Members = { model: call.route.model, prompt: String(call.request.prompt) };
  if (typeof call.request.duration === "number") body.seconds = call.request.duration;
  const extra = options(call);
  if (Object.keys(extra).some((name) => name in body)) return { outcome: "refused", reason: OWN };
  let response;
  try {
    response = await call.fetch(`${call.baseUrl}/videos`, {
      method: "POST",
      json: { ...body, ...extra },
      credential: true,
      timeoutS: 60,
    });
  } catch (error) {
    if (!(error instanceof FetchError)) throw error;
    if (error.phase === "refused") return { outcome: "refused", reason: error.reason };
    if (error.phase === "not_sent") return { outcome: "not_received", reason: error.reason };
    return { outcome: "uncertain", reason: error.reason }; // acme may have taken the job
  }
  if (response.status === 202) {
    const { id } = response.json() as { id: string };
    return { outcome: "accepted", handle: { id } }; // what collect needs, never a key
  }
  if (response.status === 429) {
    return { outcome: "not_received", reason: "acme is busy (HTTP 429)" };
  }
  if (response.status >= 400 && response.status < 500) {
    const reason = `acme refused the video (HTTP ${response.status})`;
    return { outcome: "failed", reason, retryable: false };
  }
  return { outcome: "uncertain", reason: `acme returned HTTP ${response.status}` };
}

/** Polls with its own clock; a status read bills nothing. */
async function collect(call: CollectCall): Promise<CollectOutcome> {
  const deadline = Date.now() + 600_000;
  while (Date.now() < deadline) {
    let job: { status: string; url: string; cost_usd?: number } | undefined;
    try {
      const url = `${call.baseUrl}/videos/${call.handle.id}`;
      const response = await call.fetch(url, { credential: true, timeoutS: 30 });
      if (response.status === 200) job = response.json() as typeof job;
    } catch (error) {
      if (!(error instanceof FetchError)) throw error; // a poll that saw nothing
    }
    if (job?.status === "failed") {
      return { outcome: "ended", reason: "acme could not make the video", costUsd: job.cost_usd };
    }
    if (job?.status === "done") {
      let clip;
      try {
        clip = await call.fetch(job.url, { lane: "download", timeoutS: 120 });
      } catch (error) {
        if (!(error instanceof FetchError)) throw error;
        return { outcome: "unreachable", reason: error.reason }; // a later run collects it again
      }
      if (clip.status !== 200) {
        const reason = `acme's video download returned HTTP ${clip.status}`;
        return { outcome: "unreachable", reason };
      }
      return { outcome: "answered", files: { video: clip.body }, costUsd: job.cost_usd };
    }
    await new Promise((done) => setTimeout(done, 5_000));
  }
  return { outcome: "unreachable", reason: "acme's video is still being made" };
}

export const acme = adapter({
  requests: ["structured.generate"],
  jobs: ["video.generate"],
  send,
  submit,
  collect,
  version: 1,
});
```

The project needs Node 24 or later and `@grida/fx` (`npm install @grida/fx`), since planning
loads the adapter ([When FX needs Node](01-getting-started.md#when-fx-needs-node)). `tsc` checks
the module against the SDK's types: an outcome a function's method does not take, or a member an
outcome does not have (`cost_usd` for `costUsd`), does not compile. FX itself never type checks.

### The authoring API

Both SDKs ship the same API ([protocol.md §8](../../spec/protocol.md#8-the-sdks-informative)):

- **`adapter(requests=[…], jobs=[…], send=…, submit=…, collect=…, version=N)`**
  (TypeScript: `adapter({ requests, jobs, send, submit, collect, version })`) declares the
  adapter at a module's top level, under the name the declaration gives after `#`. It names each
  capability it serves on its provider, each its capability's default there. A capability it
  does not name has no adapter on that provider. `adapter()` refuses a declaration with no
  capability, a name that is not a capability's, `requests` without `send`, or `jobs` without
  both `submit` and `collect`, so the module fails to import.
- **The call.** `send`, `submit` and `collect` each take one call, a plain function or an async
  one; FX answers several calls at once, in any order. The call holds:

  | Python | TypeScript | |
  |---|---|---|
  | `call.capability` | `call.capability` | the capability asked for |
  | `call.route.id`, `.model`, `.provider`, `.contract` | the same | the route, its contract completed (`adapter`, `adapter_behavior`, and the route's `provider_options`) |
  | `call.base_url` | `call.baseUrl` | the declaration's address: its variable's value when set, else its default |
  | `call.request` | `call.request` | the request as FX keys it, every file in it a file object |
  | `call.files` | `call.files` | those files by digest: `digest`, `kind`, `size`, `path`, and `read_bytes()` (TypeScript: `await bytes()`) |
  | `call.handle` | `call.handle` | in `collect`, the handle `submit` accepted |
  | `call.key`, `call.attempt`, `call.take`, `call.takes` | the same | the call key, the attempt (1 to 6), and the take |
  | | `call.signal` | aborted when FX drops the call |

  The request is the capability's ([capabilities.md](../../spec/capabilities.md)): for
  `structured.generate`, `prompt`, `schema` (a file or an object) and `matte`, and `system`,
  `max_tokens`, `context` and `provider_options` when the step gives them. A file's store copy is
  read-only.
- **The answer** holds what the capability names and nothing else: `Answered.json(value)` for
  `structured.generate`; `Answered(files={"video": …})` (TypeScript: `{ outcome: "answered",
  files }`) for a capability that answers files, as bytes, an `AdapterFile` or, in Python, a path.
  FX holds it to its shape, then makes the check every route of the capability gets (a
  structured answer against its schema), before it is kept. Where the capability's data comes
  from its file (a video's facts), FX writes it.

### Request or long job

A capability in `requests` is answered by `send`: one exchange with the provider, sent once,
whose outcome FX settles at once. A capability in `jobs` is a **long job**: `submit` sends the
paid request once and returns a handle, and `collect` polls with its own clock until the job
ends. FX keeps the handle in a job record, so a run stopped while the job runs collects it in the
next run, and never submits it again. A capability may be either; a video, a mesh or anything that
takes minutes is a job.

- `submit` may begin with requests the provider does not bill (an upload, a free check). A
  failure there bills nothing: `Refused` when it is deterministic, else `NotReceived`, and FX
  then sends the whole submit again.
- The handle is a JSON object of what collecting needs: the job's id, never a host, a signed URL,
  an upload token or a key. A handle that holds a key's value is not kept: the submit becomes
  `Uncertain`.
- `collect` never submits. A status read that fails is a poll that saw nothing: poll again until
  your own deadline, then return `Unreachable`, and a later run collects again.

### Outcomes, and what each costs

| Method | Outcome (Python; TypeScript `outcome`) | When | Billed |
|---|---|---|---|
| `send`, `collect` | `Answered` (`answered`) | the provider answered | the `cost_usd` it reports, else the whole hold (the cap on a capped route) |
| `send`, `submit` | `NotReceived` (`not_received`) | the provider provably did not receive it (`fetch`'s `not_sent`, a 429) | $0; FX sends it again, after its backoff or `retry_after_s` when that is longer (at most 60 s), at most 6 sends in all |
| `send`, `submit` | `Refused` (`refused`) | nothing was sent | $0; never sent again |
| `send`, `submit` | `Failed` (`failed`) | the provider received it and the call failed | `cost_usd`, else the whole hold; tried again only when `retryable` |
| `submit` | `Accepted` (`accepted`) | the provider took the job | settled when it is collected |
| `submit` | `Uncertain` (`uncertain`) | nobody can say whether it took the job | the whole hold; the next run stops for you (`grida fx jobs`) |
| `collect` | `Ended` (`ended`) | the job ended without a result | `cost_usd`, else the whole hold; a later run submits anew |
| `collect` | `Unreachable` (`unreachable`) | it is not clear the job is over | the whole hold; a later run collects again |

- **`cost_usd`** (TypeScript `costUsd`) is the provider's own figure in dollars, rounded up to
  the micro-dollar. Leave it out when the provider reports none; give `0` only where the provider
  documents that it bills nothing for that case, as the example does for acme's 400.
- **Be honest.** `NotReceived` only when the request provably never arrived; `Failed`, or
  `Uncertain` for a submit, whenever it may have. FX resends a `NotReceived` without a new hold,
  so a wrong one can pay twice.
- **Anything raised** that the adapter does not catch, an outcome its method does not take, or an
  answer FX cannot read ends the call as an unknown outcome: a send is `Failed`, billed in full
  and not sent again; a submit is `Uncertain`; a collect is `Unreachable`. Its reason names the
  adapter, and Python's traceback goes to standard error:

  ```
  failed    colour#1: structured.generate on text-a@acme failed: the adapter adapters/acme.py#acme failed: KeyError: 'json'
  ```

- **Judge in the adapter.** There is no separate check method: a judgement of a well-formed answer
  (a wrong kind of file, a blank result) is a `Failed` from `send` (`retryable=True`, since
  another draw may pass), or an `Ended` from `collect`.

The contract in full: [adapters.md §3](../../spec/adapters.md#3-outcomes) and
[§6](../../spec/adapters.md#6-long-jobs).

### The network: `fetch`

`await call.fetch(url, …)` makes one HTTP exchange through FX's own transport. It is optional:
an adapter may use any HTTP client or a vendor's SDK. `fetch` is the way to keep FX's promises
without writing them yourself:

- **The key stays out of your code.** With `credential=True` (TypeScript `credential: true`) FX
  attaches the declared key, in the declared header, and only to the origin (scheme, host and
  port) of `call.base_url`. Your code never reads it, and setting the declared credential header
  yourself is refused.
- **Downloads go only to `download_hosts`.** `lane="download"` sends no credential and refuses a
  URL off the declaration's hosts, so a declaration that lists none downloads nothing through
  `fetch`. FX adds each downloaded URL to the redactor of the call's reasons.
- **One exchange.** Never retried, no redirect followed, a deadline (`timeout_s`, at most 300 s)
  and a response cap (`max_response_bytes`, at most 64 MiB, the defaults).
- **The network switch.** With `GRIDA_FX_NETWORK=off`, every `fetch` is refused before it leaves.
- **An honest outcome.** A failure raises `FetchError` with its `phase`: `refused` (stopped before
  it left) maps to `Refused`, `not_sent` to `NotReceived`, `after_send` to `Failed` with
  `retryable=True`, or to `Uncertain` in a submit.

It takes `method` (`GET` or `POST`), `headers`, `json` or `body` with its `content_type`, `lane`
(`provider`, `upload`, `download`), `credential`, `timeout_s` and `max_response_bytes`, and returns
the `status`, `headers` and `body`, with `.json()` and `.text()` ([protocol.md
§6.8](../../spec/protocol.md#68-fetch)). An adapter with a client of its own must keep the same
rules itself: one exchange per send with its client's retries off, no redirect followed with the
key, a deadline, nothing sent while `GRIDA_FX_NETWORK` is `off` (FX cannot stop a client it does
not own), and the key read from its declared variable.

### Provider options

FX passes `provider_options` to your adapter as they were written: the route's in
`call.route.contract`, the step's in `call.request`, each keyed by provider. The engine neither
merges nor judges them for a project adapter. The example's `options()` takes this provider's
member of each, the step's winning, and refuses one that would replace a member the adapter sets
itself, as FX's own adapters do. Run against a fake of acme on this machine ([Check it before you
spend](#check-it-before-you-spend)), the clip's request carried `"loop": true` and `"style":
"flat"`: the step's option and the route's.

### Reasons and redaction

A reason is a sentence you return with an outcome. FX records it with the call and prints it in
the run's result, after redacting it: the value of every declared key, and every URL downloaded
through `fetch`, becomes `[redacted]`, and the sentence is cut to 500 characters. Still, build
reasons from fixed text and safe fields (`acme returned HTTP 503`): never a key, a header, a signed
URL, a prompt or a response body. What the adapter prints on its standard error is not redacted;
the paths of FX's private folders are not removed from reasons.

### Its version and `fx.lock`

An adapter's identity is its name, `<path>#<name>`, and its `version`. Without a version it is
the digest of the module and the project modules it imports, so any edit draws its routes' calls
again. With one, only a bump does, and `grida fx lock` pins the source behind it in `fx.lock`, as
for a [node type](03-nodes.md#versions):

```sh
grida fx lock
```

```
fx.lock: 0 versioned node types, 1 versioned adapters
```

Commit `fx.lock`. An edit that keeps the version is refused on each step the adapter serves until
you bump it, or confirm that its behaviour did not change:

```
refused   colour.route: adapters/acme.py#acme: its source changed but version 1 did not; bump the version, or confirm no change in behaviour with grida-fx lock --same adapters/acme.py#acme
refused   clip.route: adapters/acme.py#acme: its source changed but version 1 did not; bump the version, or confirm no change in behaviour with grida-fx lock --same adapters/acme.py#acme
```

```sh
grida fx lock --same adapters/acme.py#acme
```

Bump `version` whenever what the adapter sends or how it reads the answer changes.

### The adapter host

- **Planning** loads the adapter on the planning host of its language, so its top-level code runs
  then, while planning, during `doctor` and `lock`. Keep the top level to declarations. A module
  that cannot load refuses the command (exit 2): `fx.yaml: providers.acme.adapter: …`.
- **A live run**, even with the network off, starts one **adapter host** per declaration on its
  first call, in the project's root, and stops it with the run. A plan, a dry run (`run` without
  `--live`) and a stand-in run start none.
- **Its environment** is FX's, without every other declaration's variables, built-in ones
  included, and with its own declaration's key and base URL as FX read them, from the environment
  or `.env`. `GRIDA_FX_NETWORK` is there too.
- **The plan stands.** If the module changed after planning, or its host cannot start, each send
  and submit is refused at $0 (`the adapter adapters/acme.py#acme changed since the run was
  planned`), and each collect is `Unreachable`; run again, which plans again. A host that exits
  mid-call ends its calls as unknown outcomes, and the next call starts a new host.
- **Pacing and deadlines.** Calls reach the host under the route's `concurrency` and
  `requests_per_minute`, several at once. FX sets no deadline on them: your `fetch` deadlines and
  your collect's own clock are the only ones. A dropped call is cancelled: Python cancels its task,
  TypeScript aborts `call.signal`.
- **The engine still checks.** Before your function sees a call, FX holds the route's contract to
  the adapter's identity, the request to its capability, and the key: a call without its key is
  refused at $0 (`ACME_API_KEY is not set`). After, it checks the answer as above, and redacts the
  reason.

## Check it before you spend

Everything up to the provider runs offline. From the Python project above:

```sh
grida fx plan lantern
```

```
lantern  ·  1 phase
phase 1   2 steps   2 provider calls   $0.00 – $1.02
cached    0 of 2 known steps
estimate  $0.00 – $1.02
note      video.generate on clip-a@acme has no price: each call holds its cap, $1.00, and is charged what the provider reports, else the cap
```

The plan loaded the adapter: a module that does not import, or an attribute that is not
declared with `adapter()`, stops here. `grida fx doctor lantern` loads it again and names what it
serves:

```
uses      structured.generate text-a@acme no key (ACME_API_KEY) (1 step)
uses      video.generate clip-a@acme no key (ACME_API_KEY) (1 step)
…
provider  acme api.acme.test ACME_API_KEY missing
adapter   acme adapters/acme.py#acme version 1: structured.generate (request), video.generate (job)
…
route     structured.generate text-a@acme no key (ACME_API_KEY)
route     video.generate clip-a@acme no key (ACME_API_KEY)
```

Then a live run with the network off, with the key set (a made-up one will do). It starts the
adapter host, runs your `send` and `submit`, and refuses each `fetch` before it leaves, so nothing
reaches the provider and nothing is spent:

```sh
GRIDA_FX_NETWORK=off grida fx run lantern --live --max-usd 2
```

```
…
result    failed   spent $0.00
failed    colour#1: structured.generate on text-a@acme was refused: the network is off (GRIDA_FX_NETWORK=off)
failed    clip#1: video.generate on clip-a@acme was refused: the network is off (GRIDA_FX_NETWORK=off)
```

That is the outcome your `FetchError` mapping gave for `refused`. Without the key set, each call
stops before the adapter instead: `… was refused: ACME_API_KEY is not set`. An adapter that uses
its own HTTP client must check `GRIDA_FX_NETWORK` itself for this test to hold.

A **stand-in** answers the routes' calls with your own function and starts no adapter host, so
the rest of the workflow runs offline with answers you choose:

```python
# tests/stand_in.py
from grida.fx import DECLINE, Answer


def answer(call):
    if call.capability == "structured.generate":
        return Answer(data={"json": {"name": "amber"}})
    return DECLINE
```

```sh
grida fx run lantern --stand-in tests/stand_in.py#answer
```

```
…
stand-in  tests/stand_in.py#answer
result    failed   spent $0.00
failed    clip#1: video.generate on clip-a@acme is a paid call the stand-in declined
```

A stand-in's answer gets the same check every route of its capability gets, the adapter's
judgements aside ([Stand-ins](05-running.md#stand-ins-testing-without-a-provider)).

Two more checks need no provider. A fake of the provider on this machine, with the declared
address moved to it (`ACME_BASE_URL=http://127.0.0.1:<port>/v1`) and a made-up key, runs your
adapter's whole exchange without the network switch; downloads still need an `https` host
from `download_hosts`. And `AdapterCall(params, fetch=…)` (TypeScript: `new AdapterCall(params, {
method, fetch })`) builds a call for a unit test, with a function standing in for FX's `fetch`.

Only then run with the key, the network on, and a ceiling you agreed to.

## Identity and the cache

- **A route's identity** is its capability, its model, its provider and its `contract`. FX
  completes a community route's contract with its adapter's name and behaviour: the family's
  (`openai-compatible-structured` or `openai-compatible-tool-loop`, behaviour `1`), or yours
  (`adapter: adapters/acme.py#acme`, `adapter_behavior: 1`, or `source:<digest>` without a
  version). So declaring a family or an adapter, dropping it, renaming the module or the export,
  or bumping the version changes those routes' identity once, and their steps and paid calls run
  again. The route's `contract.provider_options` are part of it too.
- **Not part of it:** the declaration's key variable, header and address, a route's price, cap,
  features, pacing and `extends`. Repricing a route, or moving the provider to another address,
  keeps every cached result.
- **The cache** keeps a step's result by its identity, and a paid call's answer by its exact
  request, as for any provider. A step answered from the cache asks no adapter and needs no key.

## Share it with another project

A community provider is project files, so another project gets it by copying them:

1. the declaration, under `providers:` in its `fx.yaml`;
2. the route table, listed under its `route_tables:`;
3. the adapter's module and the project modules it imports, at the same paths, and its
   `fx.lock` entry (or run `grida fx lock` there);
4. the SDK the adapter needs: `grida` in its Python, or `@grida/fx` in its `package.json`.

Each project sets its own key. With the same provider name, module path, export and version, the
routes keep their identity, so a cache the projects share (`cache:` in `fx.yaml`) answers both.
The adapter in the example reads `provider_options` under its route's provider, so it also works
under another name, with routes of another identity.

The declaration names a module of the project. That module may import the adapter from a package
you list under `sources:` (`from acme_fx import acme`), whose files then count in its source. An
adapter named in a `sources:` package directly, without a project module, is **planned**.

## Limits

What a community provider cannot do yet, as the
[plan's follow-ups](../wg/provider-contract.md#follow-ups) record:

- **The OpenAI-compatible family** serves `structured.generate` and `agent.turn` only (no images);
  reads no cost, so a server's refusal of a request it never ran is billed at the whole hold or
  cap; resends a 408 unbilled; its contract cannot ask for a picture detail or a reasoning
  effort; and it always sends the token limit as `max_tokens`, so a server that takes only
  `max_completion_tokens` cannot be given one.
- **A route's limits do not reach your adapter.** Planning holds a step to the route's
  `max_input_pictures` and features, but a call carries none of the route's limits (its pictures,
  its price's tiers and `max_units`, its features), so your adapter cannot hold a request to them
  when it is sent.
- **No check method.** A stand-in's answer on your route gets the capability's every-route check
  only, never your adapter's judgements.
- **`fetch`** takes `GET` and `POST` only, and plays no recorded exchange: there is no offline
  replay through it.
- **No deadline from FX** on an adapter's call: one that never answers holds its call until the
  run is stopped.
- **Standard error** of an adapter host is not redacted, and FX's private paths are not scrubbed
  from your reasons.
- **`doctor`** does not report a versioned adapter whose source no longer matches `fx.lock`;
  planning does, on each step.
- **Step-level `provider_options` for an agent run**: an agent run's turns take only their
  route's.
- **No guard on `provider_options`**: FX cannot tell a key from a parameter, and records the
  options with the run, so never put a key in them.

The contract: [adapters.md §9](../../spec/adapters.md#9-project-adapters) for project adapters,
[providers.md §1](../../spec/providers.md#1-keys-and-endpoints) for declarations, and
[protocol.md §5.8](../../spec/protocol.md#58-adapters) for the wire between FX and the adapter
host.
