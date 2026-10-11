# Images from your own Codex

If you have [Codex](https://developers.openai.com/codex) installed and signed in with your ChatGPT
account, FX can draw images with it: no provider API key, and nothing to buy. Codex counts each
image against your own plan, and FX books it at $0.

This page says how to set it up, what it does well, and where it differs from the API routes.

## Set up

1. Install Codex and sign in, once:

   ```sh
   npm install -g @openai/codex
   codex login
   ```

   The ChatGPT app for macOS carries its own copy of Codex, and FX finds it there too when
   `codex` is not on your `PATH`.

2. Check that FX can see it:

   ```sh
   grida fx doctor
   ```

   ```text
   route     image.generate imagegen@codex servable
   ```

   `not installed (codex)` means FX found no Codex. Put `codex` on your `PATH`, or name the
   program with `GRIDA_FX_TOOL_CODEX=/path/to/codex`.

3. Pick the route for your project's images in `fx.yaml`:

   ```yaml
   routes:
     image.generate: imagegen@codex
   ```

   Or for one step only, with `route: imagegen@codex` on that step: a project can draw its
   drafts on Codex and its finals on an API route.

4. Run:

   ```sh
   grida fx run kite --live
   ```

   A run whose steps are all free needs no `--max-usd`: its plan is priced at $0, and FX runs it
   under a ceiling of $0, so a step on a priced route cannot slip in without one
   ([Cost, cache and takes](04-cost-and-cache.md)).

## What it honours

| `fx/image.generate@1` setting | On `imagegen@codex` |
|---|---|
| `prompt` | yes, wrapped in short instructions to Codex (below) |
| `background: auto` or `opaque` | yes |
| `background: transparent` | yes: a cut-out subject with an alpha channel |
| `size: auto`, or no `size` | yes: Codex picks the size |
| `size: <W>x<H>` | **no**: refused before Codex starts |
| `references` | **no**: refused before Codex starts |

`fx/image.edit@1` has no Codex route.

## Caveats

### Codex picks the size

Codex's built-in image tool takes a prompt and nothing else: its tool has no size or aspect-ratio
argument, and its images come back sized `auto`
([openai/codex#19175](https://github.com/openai/codex/issues/19175),
[openai/codex#28723](https://github.com/openai/codex/issues/28723)). What we and others have
seen is a fixed budget of about 1.57 megapixels (the area of 1024x1536), in whatever shape Codex
chooses. The prompt steers the shape; nothing steers the pixel count. In our tests:

| Asked for | Codex drew | Pixels |
|---|---|---|
| 1024x1024 | 1254x1254 | 1,572,516 |
| 1024x1536 | 1024x1536 | 1,572,864 |
| 1152x2496 | 1024x1536 | 1,572,864 |
| 2560x1440 | 1774x887 | 1,573,538 |
| no size, "a wide establishing shot" | 1672x941 | 1,573,352 |

So a picture from Codex is never larger than about 1.57 megapixels, and a 2K or 4K size is out of
reach on this route. This is what Codex does today, not a documented promise: a Codex update can
change it.

So FX refuses a step with an exact `size` on this route, at $0, rather than letting each image
fail the size check and be drawn again:

```text
failed    draw#1: Codex image generation takes no exact size: use size auto, and fx/image.resize@1 after it
```

Say the shape you want in the prompt ("on a square canvas", "portrait", "a wide shot"), leave
`size` out, and size the picture with a free local step after it. For an icon on an exact canvas,
scale it to fit and pad it, which keeps its proportions and its transparency:

```yaml
steps:
  draw:
    uses: fx/image.generate@1
    route: imagegen@codex
    with:
      prompt: "A game pickup icon: a golden key. Centred on a square canvas, no text."
      background: transparent
  fit:
    uses: fx/image.resize@1
    with: { image: "${{ steps.draw.outputs.image }}", longest_side: 1024 }
  icon:
    uses: fx/image.pad@1
    with: { image: "${{ steps.fit.outputs.image }}", width: 1024, height: 1024 }
```

`fx/image.resize@1` with both `width` and `height` stretches the picture to that size; use it
only when the shapes already match.

### Transparency is Codex's, and FX checks it

Codex makes transparent images itself: the subject comes back cut out, with soft edges and glows
kept as partial transparency. FX holds each answer to the setting: a picture asked for as
transparent must have an alpha channel, and one asked for as opaque must have no transparent
pixels. An answer that fails is drawn again, as on any route.

### Your plan's limits apply

- Each image counts against your Codex plan's usage limits, which FX can neither see nor
  enforce. OpenAI says image generation uses them
  [3–5 times faster](https://learn.chatgpt.com/docs/image-generation) on average than similar
  turns without images. A run's `spent $0.00` is FX's bill, not your plan's.
- At the limit, the step fails with the time the limit resets, and is not retried:

  ```text
  failed    draw#1: Codex image generation failed: the Codex usage limit is reached; it resets at unix time 1792311096
  ```

- FX never falls back to an API key: it hides your provider keys and `CODEX_API_KEY` from Codex,
  so Codex cannot bill an API account instead of your plan.

### Slower, and one at a time

An image takes about 20 seconds, and the route draws one at a time, so a workflow of 20 images
takes several minutes. The cache works as on any route: a step that already drew its image does
not call Codex again.

### Not the same picture as an API route

- **The prompt is wrapped.** FX asks Codex, in fixed instructions, to make exactly one image from
  your prompt as written; Codex's image tool may still revise it. The same prompt on an API route
  draws a different picture.
- **The model is Codex's.** Codex chooses it, and its quality; FX neither picks nor records
  them. Others have seen Codex report `gpt-image-2-codex` at quality `auto`
  ([openai/codex#28723](https://github.com/openai/codex/issues/28723)), where the API routes ask
  for the highest quality.
- **Switching a step to or from Codex draws it anew**: the route is part of the step's identity
  ([Cost, cache and takes](04-cost-and-cache.md)).

### Once Codex starts drawing, a failure is not repeated

If Codex exits, stalls past 10 minutes, or answers without an image after it has started the
turn, the step fails and FX does not draw it again on its own: an image may already have been
made and counted against your plan. Run again to try again. A failure before Codex starts
drawing (Codex not starting, or refusing the request) is retried as a connection failure is.

### Local only

The route uses the Codex signed in on the machine that runs FX. It is meant for your own runs,
not CI or a server, where nobody is signed in. With `GRIDA_FX_NETWORK=off`, FX does not start
Codex at all: `Codex was not started: GRIDA_FX_NETWORK is off`.

### What runs on your machine

- FX starts `codex app-server` for each image, in an empty scratch folder it removes afterwards,
  with a read-only sandbox and no approvals; anything Codex asks FX to approve is declined.
- FX never reads Codex's sign-in files.
- Your Codex settings still load. FX turns off the turn-end notification, memories and hooks for
  these conversations, but your MCP servers and plugins still start with each image, which makes
  each one slower.
- Codex keeps its own copy of each image under `~/.codex/generated_images/`. FX keeps the one in
  its store and never touches Codex's.
- Your prompts and images go to OpenAI under your ChatGPT account's terms, as when you use Codex
  yourself.
- FX talks to Codex through `codex app-server`, which Codex marks experimental. It was tested with
  Codex 0.160.1; a Codex update can change it, and a step that no longer understands Codex fails
  before Codex draws anything.

## When it fails

| The step says | What to do |
|---|---|
| `Codex is not installed: …` | Install Codex and run `codex login`, or set `GRIDA_FX_TOOL_CODEX`. |
| `Codex image generation takes no exact size: …` | Leave `size` out, and size the picture after it (above). |
| `Codex image generation takes no references` | Use an API route for that step. |
| `… the Codex usage limit is reached; it resets at …` | Wait for the reset, or use an API route. |
| `Codex image generation was not sent: Codex refused …` | Check `codex login status`; sign in again if needed. |
| `Codex image generation failed: Codex answered without an image` | Run again; rephrase a prompt Codex will not draw. |
| `Codex was not started: GRIDA_FX_NETWORK is off` | Unset `GRIDA_FX_NETWORK`. |

How the route is specified, sentence by sentence, is
[providers.md §9.6](../../spec/providers.md#96-codex).
