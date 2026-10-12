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
   route     image.edit imagegen@codex servable
   route     image.generate imagegen@codex servable
   ```

   `not installed (codex)` means FX found no Codex. Put `codex` on your `PATH`, or name the
   program with `GRIDA_FX_TOOL_CODEX=/path/to/codex`.

3. Pick the route for your project's images in `fx.yaml`:

   ```yaml
   routes:
     image.generate: imagegen@codex
     image.edit: imagegen@codex
   ```

   Or for one step only, with `route: imagegen@codex` on that step: a project can draw its
   drafts on Codex and its finals on an API route.

4. Run:

   ```sh
   grida fx run kite --live
   ```

   A run whose paid calls are all on Codex needs no `--max-usd`. FX runs it under a ceiling of $0
   and refuses a call on any other route, so a priced step cannot slip in without one
   ([Cost, cache and takes](04-cost-and-cache.md)). It still spends your Codex plan: an agent
   running FX for you should ask you first, as for any live run.

## What it honours

| `fx/image.generate@1` and `fx/image.edit@1` setting | On `imagegen@codex` |
|---|---|
| `prompt` | yes, wrapped in short instructions to Codex (below) |
| `background: auto` or `opaque` | yes |
| `background: transparent` | yes: a cut-out subject with an alpha channel |
| `size: auto`, or no `size` | yes: Codex picks the size |
| `size: <W>x<H>` | **no**: refused while planning |
| `references`, and an edit's `image` | yes: at most 5 pictures in all, PNG, JPEG or WebP |
| `mask` | **no**: refused while planning |

### Drawing from pictures

Give a step pictures and Codex draws from them: the same character in a new pose, a sketch
redrawn in a style, a picture edited. In the prompt, call them by their place in the list,
`input image 1`, `input image 2`, ...; an edit's `image` is input image 1, then its `references`.

```yaml
steps:
  hero:
    uses: fx/image.generate@1
    with: { prompt: "A small fox mascot, full body, facing the viewer.", background: transparent }
  waving:
    uses: fx/image.generate@1
    with:
      prompt: "The fox in input image 1, waving with one paw. Keep its colours and markings."
      background: transparent
      references: ["${{ steps.hero.outputs.image }}"]
```

Drawing every pose from one picture keeps a character the same from picture to picture, which a
description alone does not. FX copies the pictures into the image's own scratch folder and names
them to Codex's image tool, which reads them at full resolution.

Codex takes at most 5 pictures in one image. More are refused while planning, at $0, as are
references on a route that takes none:

```text
refused   waving.with.references: imagegen@codex takes at most 5 input pictures, not 6
```

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
| no size, "about 4:1" | 2172x724 | 1,572,528 |
| no size, "about 1:3" | 724x2172 | 1,572,528 |
| an edit of a 1000x400 picture | 1984x793 | 1,573,312 |

The shape follows the wording between about 1:3 and 3:1, never further. With pictures, it
follows the first picture's shape unless the prompt asks for another.

So a picture from Codex is never larger than about 1.57 megapixels, and a 2K or 4K size is out of
reach on this route. This is what Codex does today, not a documented promise: a Codex update can
change it.

So the plan refuses a step with an exact `size` on this route, at $0, rather than letting each
image fail the size check and be drawn again:

```text
refused   draw.with.size: imagegen@codex does not support exact_size
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
transparent must have transparent pixels (an alpha channel alone, under a painted checkerboard,
is not enough), and one asked for as opaque must have no transparent pixels. An answer that fails is drawn again, as on any route, and each new attempt is another
image on your plan.

### Your plan's limits apply

- Each image counts against your Codex plan's usage limits, which FX can neither see nor
  enforce. OpenAI says image generation uses them
  [3–5 times faster](https://learn.chatgpt.com/docs/image-generation) on average than similar
  turns without images. A run's `spent $0.00` is FX's bill, not your plan's. What Codex did count
  is kept in each call's record in the cache (`usage`: the Codex version, and the tokens of the
  turn), marked as billed to your plan.
- At the limit, the step fails with the time the limit resets, and is not retried:

  ```text
  failed    draw#1: Codex image generation failed: the Codex usage limit is reached; it resets at unix time 1792311096
  ```

  The run's other Codex steps then start nothing until that time, so a long run stops spending
  turns on a plan that cannot draw. They fail with
  `the Codex usage limit is reached; it resets at unix time …, and no Codex call starts before then`.

- FX draws only on a ChatGPT sign-in. When it starts Codex it asks how Codex is signed in, and
  refuses, before anything is drawn, a Codex signed in with an API key (`codex login
  --with-api-key`), which would bill an API account. It also hides your provider keys and
  `CODEX_API_KEY` from Codex.

### Slower, three at a time

An image takes 20 to 60 seconds, and the route draws three at a time, so a workflow of 20 images
takes a few minutes. Both Codex routes share those three. Steps that wait for one are shown as
`waiting`, not `running`. The first image of a run also starts Codex, which the run then keeps for
the others. To draw more or fewer at once, set the route's `concurrency` in `fx.yaml`:

```yaml
routes:
  image.generate: { route: imagegen@codex, concurrency: 2 }
```

The cache works as on any route: a step that already drew its image does not call Codex again.

### Not the same picture as an API route

- **The prompt is wrapped.** FX asks Codex, in fixed instructions, to call its image tool once
  with your prompt as written and your pictures; Codex's image tool may still revise the prompt. The same prompt on an API route
  draws a different picture.
- **The model is Codex's.** Codex chooses it, and its quality; FX neither picks nor records
  them. Others have seen Codex report `gpt-image-2-codex` at quality `auto`
  ([openai/codex#28723](https://github.com/openai/codex/issues/28723)), where the API routes ask
  for the highest quality.
- **Switching a step to or from Codex draws it anew**: the route is part of the step's identity
  ([Cost, cache and takes](04-cost-and-cache.md)).

### A failure is not repeated

If Codex exits, stalls, or answers without an image, the step fails and FX does not ask Codex
again on its own (the one exception is a Codex too busy to take the request, which never saw it): once Codex has been asked to draw, an image may already have been made and
counted against your plan, and a Codex that failed before that will most likely fail the same
way again. Run again to try again. (Within a turn, Codex retries its own requests to OpenAI; that
is Codex's.)

### Local only

The route uses the Codex signed in on the machine that runs FX. It is meant for your own runs,
not CI or a server, where nobody is signed in. With `GRIDA_FX_NETWORK=off`, FX does not start
Codex at all: `Codex was not started: GRIDA_FX_NETWORK is off`.

### What runs on your machine

- FX starts one `codex app-server` for a run, on its first Codex image, and ends it with the run.
  Each image gets a thread of its own, in a scratch folder holding only copies of its pictures,
  which FX removes afterwards, with a read-only sandbox and no approvals; anything Codex asks FX
  to approve is declined. The read-only sandbox stops writes, not reads: Codex is asked to use no
  tool but its image tool, and nothing else is offered to it (below).
- Your own Codex tools are off for these conversations: your MCP servers (FX lists them with
  `codex mcp list` and turns each off), apps, plugins, hooks, memories and the turn-end
  notification. So are Codex's other tools (its shell, web search, browser and computer use,
  sub-agents, image viewer and skills), which also halves what each image spends of the context. Your reasoning effort and service tier are not used either: an image turn runs at
  low effort and the standard tier, so it spends your plan no faster than it must.
- Codex runs in a process group of its own, with everything it starts. When the run ends, is
  cancelled, or FX exits, FX ends the whole group; a step that is cancelled while Codex draws asks
  Codex to stop that image.
- FX never reads Codex's sign-in files.
- Codex keeps its own copy of each image under `~/.codex/generated_images/`. FX keeps the one in
  its store and never touches Codex's.
- Your prompts and images go to OpenAI under your ChatGPT account's terms, as when you use Codex
  yourself.
- FX talks to Codex through `codex app-server`, which Codex marks experimental. It was tested with
  Codex 0.160.1; a Codex update can change it. A step that no longer understands Codex fails, and
  may fail after Codex has drawn (an image FX cannot recognise is lost to it).

## When it fails

| The step says | What to do |
|---|---|
| `Codex is not installed: …` | Install Codex and run `codex login`, or set `GRIDA_FX_TOOL_CODEX`. |
| `imagegen@codex does not support exact_size` | Leave `size` out, and size the picture after it (above). |
| `imagegen@codex takes at most 5 input pictures, …` | Give the step fewer pictures, or use an API route for it. |
| `… is image/gif; Codex takes PNG, JPEG or WebP` | Convert the picture first. |
| `… the Codex usage limit is reached; it resets at …` | Wait for the reset, or use an API route. |
| `Codex is not signed in: …`, or `Codex is signed in with an API key, …` | Run `codex login` and sign in with ChatGPT. |
| `Codex's MCP servers could not be listed, …` | Check that `codex mcp list` works; update Codex. |
| `Codex image generation was not started: …` | Check that `codex app-server` starts; update Codex. |
| `Codex image generation failed: Codex answered without an image` | Run again; rephrase a prompt Codex will not draw. |
| `Codex was not started: GRIDA_FX_NETWORK is off` | Unset `GRIDA_FX_NETWORK`. |

How the route is specified, sentence by sentence, is
[providers.md §9.6](../../spec/providers.md#96-codex).
