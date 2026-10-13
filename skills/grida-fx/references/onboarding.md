# Onboarding a project to FX

Read this when setting FX up in a project for the first time, or when a live run
fails because no provider is configured. Most of setup is yours to do; connecting
an AI provider is the user's, because only they can open an account, add credits
and create a key. Ask them for those steps, then check the result with
`grida fx doctor`.

## 1. Install and initialize

For a new project, `init` needs no installed CLI:

```sh
npx grida fx init my-assets
```

In an empty or new folder, recent releases write a starter project: `fx.yaml`
(routes and budget commented out), `README.md`, `AGENTS.md`, `.gitignore`,
`.env.example`, `workflows/icon.yaml` and the FX skills under `.agents/skills/`.
`--json` lists them in `files_created`. To start from an example instead, see
`grida fx init --list-templates --json` and pass `--template NAME`; it downloads
the template from the FX release, so ask the user first if the network is a
concern. Agents always pass `--template` or get the starter: `init` asks no
question without a terminal.
`init` installs nothing: the user, or you with their agreement, installs the CLI
as the main skill describes (`npm install -g grida`), or keeps using `npx grida fx`.

In a folder that has files but no `fx.yaml` (adding FX to an existing repository),
`init` refuses and writes nothing: create `fx.yaml` holding `fx: project/v1`
instead. Then keep `runs/`, `.fx/` and `.env` out of source control: add them to
`.gitignore` when they are missing. An older release writes only `fx.yaml`; do the
same there.

Build and run the workflow's local steps first; they need no provider and no
`--live`.

## 2. Choose how the project reaches AI models

FX needs no provider at all for ordinary code. When a step calls an AI model, ask
the user which path they want. Do not pick one for them when it spends money or
uses their plan.

| Path | The user sets up | Serves | Billed to |
|---|---|---|---|
| No provider | nothing | local code and built-in local steps | nobody |
| Their own Codex | Codex, signed in with ChatGPT | images only | their ChatGPT plan; $0 to FX |
| Grida Gateway (GG) | one `GG_API_KEY` | text, agent turns, images, video, 3D models and rigs | their organization's Grida AI credits |
| Their own keys | each provider's key | that provider's routes | each provider's account |

When the user has no preference and needs more than images, suggest Grida
Gateway: one key and one account cover every modality FX's workflows usually
need. Sound, speech and music have no GG route; those need the provider's own
key (ElevenLabs, OpenRouter). Keep routes the user already chose: a new route is
a new cache key, so earlier results do not carry over.

## 3. Ask the user to connect a provider

Never ask the user to paste a key into the conversation, and never read, print or
write a key yourself. The user puts it in their shell environment or in the
project's `.env` (`NAME=value`, one per line); FX reads both, and the environment
wins.

### Grida Gateway

GG is newer than the other providers and has little written about it on the web
yet, so give the user these steps rather than sending them to search:

1. Open <https://console.grida.co/_/ai-gateway> and sign in to Grida. The link
   goes to AI Gateway in the Grida console.
2. Add Grida AI credits to the organization. Calls are refused without credits.
3. Create an API key with both scopes: `text` (FX's text and agent steps) and
   `media` (images, video and 3D). A key with only one scope refuses the other
   steps. A spending cap on the key is a good extra guard for a first run.
4. Copy the key when it is shown; it is shown once. Set it as `GG_API_KEY` in the
   environment, or add the line `GG_API_KEY=…` to the project's `.env`.

For questions about GG itself (key scopes, credits, spending caps, rate limits,
how a charge is settled), read <https://gateway.grida.ai/llms.txt>. It is GG's
public API reference, written for agents, and needs no key. FX already speaks
GG's API: use the reference to answer the user's questions, not to call GG from
node bodies around FX's spending ceiling.

When a live GG call fails, FX's message says what to do: `check GG_API_KEY` (the
key is wrong or revoked), `add Grida AI credits at …` (out of credits), or `check
the key and organization at …` (a scope, the key's spending cap or the
organization refused it). Relay it to the user; these are theirs to fix in the
console.

### Their own Codex

Ask the user to install Codex and sign in with their ChatGPT account (not an API
key):

```sh
npm install -g @openai/codex
codex login
```

Then set the image routes in `fx.yaml` with the user's agreement:

```yaml
routes:
  image.generate: imagegen@codex
  image.edit: imagegen@codex
```

Codex chooses the model and quality, and has size limits. A run on it needs no
`--max-usd` but spends the user's plan, so it still needs their go. Caveats:
[Images from your own Codex](https://github.com/gridaco/fx/blob/main/docs/guide/12-codex.md).

### Their own provider keys

Ask the user to create keys only for the providers their routes use:

| Provider | Variable | Get a key |
|---|---|---|
| OpenAI | `OPENAI_API_KEY` | <https://platform.openai.com/api-keys> |
| OpenRouter | `OPENROUTER_API_KEY` | <https://openrouter.ai/settings/keys> |
| fal | `FAL_KEY` | <https://fal.ai/dashboard/keys> |
| Tripo | `TRIPO_API_KEY` | <https://platform.tripo3d.ai/api-keys> |
| ElevenLabs | `ELEVENLABS_API_KEY` | <https://elevenlabs.io/app/settings/api-keys> |

## 4. Verify

```sh
grida fx doctor
grida fx doctor workflows/example.yaml
```

Each `key` line says `present` (with its source, `environment` or `.env`) or
`missing`, never the value. Each `route` line says whether a live run could serve
it; with a workflow, `doctor` exits 1 when a route the workflow uses is not
servable. `not installed (codex)` means FX found no Codex: put `codex` on `PATH`
or set `GRIDA_FX_TOOL_CODEX`. `too old (codex …)` means the Codex its `program`
line names is older than 0.160.1: the user updates it or sets
`GRIDA_FX_TOOL_CODEX` to a newer one.

## 5. Pick routes and make the first paid run

List the routes for a capability and set the project's defaults in `fx.yaml`
with the user:

```sh
grida fx nodes image.generate
```

```yaml
fx: project/v1
routes:
  image.generate: openai/gpt-image-2.5-sunburst@gg
```

Then plan, show the user the estimate, agree a dollar cap, and run within it:

```sh
grida fx plan workflows/example.yaml --inputs inputs/example.yaml
grida fx run workflows/example.yaml --inputs inputs/example.yaml --live --max-usd 5
```

Setting up a key is not permission to spend. Each paid run needs the user's go
and a cap, as the main skill describes.

More: [AI providers](https://github.com/gridaco/fx/blob/main/docs/guide/13-providers.md)
lists which provider serves each capability.
