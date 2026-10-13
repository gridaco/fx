# Images from your own Grok Build

If you have [Grok Build](https://x.ai/cli) installed and signed in with a Grok account whose
subscription includes image generation, FX can draw and edit images with it: no provider API key,
and nothing to buy. Grok Build counts each image against your own subscription, and FX books it at
$0.

This page says how to set it up, what it does well, and where it differs from the API routes.

## Set up

1. Install Grok Build and sign in, once:

   ```sh
   curl -fsSL https://x.ai/cli/install.sh | bash
   grok login
   ```

   Grok Build 1.0.52 or later is needed; `grok update` brings an older one up to date.

2. Check that FX can see it:

   ```sh
   grida fx doctor
   ```

   ```text
   route     image.edit imagine@grok servable
   route     image.generate imagine@grok servable
   ```

   `not installed (grok)` means FX found no Grok Build. Put `grok` on your `PATH` (the installer
   puts it in `~/.grok/bin`, where FX also looks), or name the program with
   `GRIDA_FX_TOOL_GROK=/path/to/grok`.

3. Pick the route for your project's images in `fx.yaml`:

   ```yaml
   routes:
     image.generate: imagine@grok
     image.edit: imagine@grok
   ```

   Or for one step only, with `route: imagine@grok` on that step.

4. Run:

   ```sh
   grida fx run kite --live
   ```

   A run whose paid calls are all on Grok Build, or on Grok Build and [your own Codex](12-codex.md),
   needs no `--max-usd`. FX runs it under a ceiling of $0 and refuses a call on any other route
   ([Cost, cache and takes](04-cost-and-cache.md)). It still spends your subscription: an agent
   running FX for you should ask you first, as for any live run.

## What it honours

| `fx/image.generate@1` and `fx/image.edit@1` setting | On `imagine@grok` |
|---|---|
| `prompt` | yes, wrapped in short instructions to Grok Build (below) |
| `background: auto` or `opaque` | yes: Grok Build's images are opaque |
| `background: transparent` | **no**: refused while planning |
| `size: auto`, or no `size` | yes: Grok Build picks the size |
| `size: <W>x<H>` | **no**: refused while planning |
| `references`, and an edit's `image` | yes: at most 3 pictures in all, PNG or JPEG |
| `mask` | **no**: refused while planning |

### Drawing from pictures

Give a step pictures and Grok Build draws from them with its image edit tool: the same subject in a
new scene, two pictures combined, a picture restyled. It takes at most 3 pictures in one image;
more are refused while planning, at $0. In the prompt, call them by their place in
the list, `input image 1`, `input image 2`, ...; an edit's `image` is input image 1, then its
`references`.

```yaml
steps:
  kite:
    uses: fx/image.generate@1
    with: { prompt: "A red kite over green hills, flat illustration." }
  boat:
    uses: fx/image.generate@1
    with: { prompt: "A blue paper boat on a pond, flat illustration." }
  both:
    uses: fx/image.edit@1
    with:
      image: "${{ steps.kite.outputs.image }}"
      references: ["${{ steps.boat.outputs.image }}"]
      prompt: "Put the paper boat from input image 2 on a lake below the hills of input image 1."
```

## Caveats

### Grok Build picks the size

Grok Build's image tools take an aspect ratio, not a size, and FX leaves it to Grok Build. Its
images are about one megapixel: in our tests, a square image came back 1024x1024, wide and tall
ones 1280x720 and 720x1280, and one left to choose 864x1152. With pictures it picks the shape too:
an edit of square pictures came back 1024x1024 once and 864x1152 another time.

So the plan refuses a step with an exact `size` on this route, at $0. Say the shape you want in the
prompt, leave `size` out, and size the picture with a free local step after it
(`fx/image.resize@1`, then `fx/image.pad@1` for an exact canvas; see
[Images from your own Codex](12-codex.md#codex-picks-the-size) for the pattern).

### Opaque only

Grok Build draws opaque JPEG images. FX keeps the same pixels as a PNG, as every image route
answers, but there is no transparency to keep: a step that asks for `background: transparent` is
refused while planning. Use a route with `alpha` for cut-outs, or remove the background after it.

### Your subscription's limits apply

- Each image counts against your Grok subscription, which FX can neither see nor enforce. A run's
  `spent $0.00` is FX's bill, not your subscription's. What Grok Build did count is kept in each
  call's record in the cache (`usage`: the Grok Build version, its agent model, and the tokens of
  the turn), marked as billed to your plan.
- An account whose subscription does not include image generation fails its first Grok step with
  `Grok Build's image tools need a SuperGrok subscription on the signed-in account`, and the run's
  other Grok steps then start nothing.
- FX draws only on a subscription sign-in (`grok login`). It turns Grok Build's API-key sign-in
  off, hides `XAI_API_KEY` and your provider keys from it, and refuses an account Grok Build says
  is billed through the xAI API.

### Three at a time

An image takes about 10 to 15 seconds, and the route draws three at a time. Both Grok routes share
those three, and steps that wait for one are shown as `waiting`. The first image of a run also
starts Grok Build, which the run then keeps for the others. Set the route's `concurrency` in
`fx.yaml` to draw more or fewer at once.

### Not the same picture as an API route

- **The prompt is wrapped.** FX asks Grok Build, in fixed instructions, to call one image tool once
  with your prompt as written and your pictures; Grok Build's agent may still word it differently.
- **The models are Grok Build's.** Grok Build chooses its image model; FX does not pick it.
- **Switching a step to or from Grok Build draws it anew**: the route is part of the step's
  identity ([Cost, cache and takes](04-cost-and-cache.md)).

### A failure is not repeated

If Grok Build exits, stalls, or answers without an image, the step fails and FX does not ask again
on its own: once Grok Build has been asked to draw, an image may already have been made and
counted. Run again to try again; steps that already drew are cached.

The one exception is Grok Build's image tool itself reporting that it failed, so no image was
saved: in our runs, now and then, a passing network error between Grok Build and xAI. FX then
makes a new attempt, as for any provider's retryable failure, and records each
one; at most six attempts in all.

### Local only

The route uses the Grok Build signed in on the machine that runs FX. It is meant for your own runs,
not CI or a server. With `GRIDA_FX_NETWORK=off`, FX does not start Grok Build at all.

### What runs on your machine

- FX starts one `grok agent --no-leader stdio` for a run, on its first Grok image, and ends it with
  the run. It talks to it over the Agent Client Protocol, and does not join a Grok Build you
  already have open.
- Each image gets a session of its own whose agent is offered Grok Build's two image tools and
  nothing else: no shell, no files, no web, no subagents, no MCP tools. The session asks before
  every tool call, even if your own settings always approve, and FX grants a permission only for
  an image tool. The session runs at low
  reasoning effort, so it spends your subscription no faster than it must.
- Nothing is imported from your Claude, Cursor or Codex settings, and Grok Build's telemetry,
  managed MCP servers, auto-update, web fetch, video and file writing are off for these sessions.
- FX deletes each session when its image is taken, or when its step fails or is cancelled
  (`grida fx cancel`), so FX's images do not pile up in your Grok Build history. A run stopped
  with Ctrl-C ends Grok Build at once and may leave its unfinished sessions. Grok Build may also
  keep its prompt history for the run's scratch folder.
- Grok Build runs in a process group of its own, with everything it starts; FX ends the whole
  group when the run ends, is cancelled, or FX exits.
- FX never reads Grok Build's sign-in files.
- Your prompts and images go to xAI under your Grok account's terms, as when you use Grok Build
  yourself.
- It was tested with Grok Build 1.0.52; a Grok Build update can change it.

## When it fails

| The step says | What to do |
|---|---|
| `Grok Build is not installed: …` | Install Grok Build and run `grok login`, or set `GRIDA_FX_TOOL_GROK`. |
| `Grok Build … is older than 1.0.52: run grok update` | Run `grok update`. |
| `Grok Build is not signed in: run grok login` | Run `grok login`. |
| `Grok Build bills this account through the xAI API, …` | Sign in with `grok login` to an account with a subscription. |
| `Grok Build's image tools need a SuperGrok subscription …` | Use an account whose subscription includes images, or another route. |
| `imagine@grok does not support exact_size` | Leave `size` out, and size the picture after it (above). |
| `imagine@grok does not support alpha` | Use a route with transparency for this step. |
| `… Grok Build's image tool failed` | FX already tried again; run again later. |
| `… Grok Build's image tool was refused (HTTP 400)` | xAI refused the request as sent; change the step (fewer pictures, another prompt). |
| `Grok Build was not started: GRIDA_FX_NETWORK is off` | Unset `GRIDA_FX_NETWORK`. |

How the route is specified, sentence by sentence, is
[the Grok Build adapter's reference](../../crates/grida-fx-providers/src/grok/README.md).
