# Movie sprite

A character image becomes a transparent body-idle loop. FX fits a green plate,
compiles the motion brief, makes one video call, then keys and closes the take
locally. You can instead supply footage and pay nothing for generation.

This is the source behind the [movie sprite showcase](https://grida.co/fx/workflows/movie-sprite).
The default recipes reproduce the reviewed Nami motion loop. The earlier quiet
Nami and traveler presets remain available separately.

| Preset | New generation | Free finishing |
| --- | --- | --- |
| Nami motion loop | `showcase.yaml` | `finish-showcase.yaml` |
| Historical quiet Nami | `showcase.quiet-nami.yaml` | `finish.quiet-nami.yaml` |
| Historical traveler | `showcase.traveler.yaml` | `finish-traveler.yaml` |

The helpers in `nodes/_movie_sprite/` belong to this example; they are not FX APIs.
No other checkout or application package is needed.

## Setup

Install [uv](https://docs.astral.sh/uv/) and FFmpeg (`ffmpeg` and `ffprobe` on `PATH`),
then run in this folder:

```sh
uv sync
```

The example declares NumPy, SciPy, Pydantic and OpenCV locally. FX itself does not
need those dependencies. Commands below use the Python installation; with the
`grida` host installed, `uv run grida fx` is equivalent to `uv run python -m grida.fx`.

## Try it without a provider

The fixture draws a geometric figure on green. The workflow trims its full frame
interval, then keys it to a transparent loop:

```sh
uv run python fixtures/make_inputs.py .fixtures
uv run python -m grida.fx plan workflow.yaml --inputs .fixtures/clip.yaml
uv run python -m grida.fx run workflow.yaml --inputs .fixtures/clip.yaml --no-view
```

The run produces a transparent FFV1 Matroska loop, its exact first-frame PNG,
a silent MP4 preview, a contact sheet, manifests and a PNG-frame ZIP. To inspect it in the viewer, start the project service first, then run with `--open`:

```sh
uv run python -m grida.fx start --background
uv run python -m grida.fx run workflow.yaml --inputs .fixtures/clip.yaml --open
```

Planning the generated-take path also stays offline:

```sh
uv run python -m grida.fx plan workflow.yaml --inputs .fixtures/take.yaml
```

Fresh-cache output from the current engine:

```text
movie-sprite  ·  1 phase
phase 1   4 steps   1 provider calls   $0.30 – $0.38
cached    0 of 2 known steps
estimate  $0.30 – $0.38
```

Running that path without `--live` refuses the video call at $0. Estimates depend
on the chosen duration and route; a live cap is a ceiling, not the quoted price.

## Video route

This project's `routes.yaml` selects
[`fal-ai/wan/v2.7/image-to-video@fal`](https://fal.ai/models/fal-ai/wan/v2.7/image-to-video/api)
through FX's existing queue adapter. It accepts the green plate as both
`image_url` and `end_image_url`, with an integer duration. The example uses 3–10
seconds at 720p or 1080p. The source plate fixes the portrait or landscape framing.

[Public fal rates](https://fal.ai/models/fal-ai/wan/v2.7/image-to-video), checked
2026-10-12, are $0.10/second at 720p and $0.15/second at 1080p. The route's highs
add a 25% planning allowance: an 8-second 720p take plans at $0.80–$1.00.
This project route does not change FX's built-in video default.

Wan's prompt expansion defaults to enabled, and would rewrite the motion brief.
The route turns it off through fal's own parameter,
`provider_options: {fal: {enable_prompt_expansion: false}}` in its contract,
which FX passes to fal as written.
Content moderation also remains enabled. A matching first and last frame guides
the generation; it does not promise a clean loop or consistent animation.
Local finishing and visual review are still necessary.

## Reproduce the Nami showcase

`showcase.yaml` uses the original adult Nami illustration for a six-second,
1080p take in a wide 16:9 frame. `plate_padding: 64` leaves room around the hair
and hood during the sway; the workflow's default stays 32 for the older presets.
Padding is measured on the 1280×720 source plate, before the provider scales it.
This is framing headroom, not a guarantee that a new generation stays in bounds.

The selected take visibly moves the upper body, hood and hair. Natural blinks
and small facial changes remain in the footage: this preset does not pin the
face or raised hands. The brief asks for a consistent expression and identity;
the selected take passed independent visual review. New takes need their own review.

The free recipe binds the exact raw take with `trim.nami-motion.json`, selecting
frames **[12,132)** at 30 fps. `finish.nami-motion.json` keys and closes that
120-frame interval into a **four-second, 30 fps** loop, with no held regions,
moving guards or local repairs. It does not generate or interpolate source
frames; optical flow is used only for the declared endpoint closure.

The raw input is `.showcase-inputs/nami-motion-take.mp4`, SHA-256
`34b779452fa55caee632f3f1428033682180032ea40826933d988a368c44d3ae`.
The shared downloader verifies the published inputs and places them in ignored
`.showcase-inputs/`. These commands replay the existing take without provider calls:

```sh
python ../fetch_showcase_inputs.py .
uv run python -m grida.fx plan workflow.yaml --inputs finish-showcase.yaml --max-usd 0
uv run python -m grida.fx run workflow.yaml --inputs finish-showcase.yaml --max-usd 0 --no-view
```

The generation recipe uses the unbound six-second `finish.motion.json`, rather
than applying a measured interval to unseen footage. A new take can differ from
the selected take and needs its own trim, finishing settings and review.

## Historical presets

The same input manifest retains the original Nami illustration, quiet Nami take
and traveler inputs. Their pins and source-bound finishing settings are unchanged.

```sh
uv run python -m grida.fx run workflow.yaml --inputs finish.quiet-nami.yaml --max-usd 0 --no-view
# Or: --inputs finish-traveler.yaml
```

Quiet Nami uses the original eight-second portrait take. `trim.nami.json`
selects **[12,228)**, then `finish.nami-face-pinned.json` keys and closes those
216 frames. Their 7.2 seconds of source motion play over eight seconds at 27 fps.

The held polygon includes the face, adjacent head and parts of the raised
fingers; it preserves those pixels from the trimmed first frame while the body
idles. Its digest and native coordinates are valid only for this exact trimmed
take. This differs deliberately from the motion loop's moving face.

Use the optional trim for opaque or green-screen takes; it does not preserve
an existing alpha channel. Its bitexact container flags were verified with
FFmpeg 9.0.1. Identical compressed bytes are not promised across versions or
builds. Quiet Nami's source-bound held polygon refuses a different encoding;
recreate and review its settings explicitly rather than weakening that check.

`finish-traveler.yaml` replays the historical traveler with
`finish.face-pinned.json`. `showcase.traveler.yaml` preserves that character's
generation brief. `showcase.quiet-nami.yaml` generates a new quiet Nami take with
`finish.initial.json`, without the published take's measured trim or polygon.
Both Nami generation presets use the same original adult illustration, with
only its alpha channel normalized; its RGB pixels and 2048×3072 dimensions are
unchanged.

## Generate your own take

For your own character, copy `showcase.yaml`, change the character and direction,
and plan first. A deliberate paid run requires provider credentials, `--live`
and a budget chosen by you, for example:

```sh
uv run python -m grida.fx run workflow.yaml --inputs showcase.yaml --live --max-usd 3 --no-view
```

New video takes are nondeterministic. Finishing may refuse excessive motion;
held-region geometry, when used, must be bound to the exact footage. A valid
encode or technical report does not approve artistic quality.

## Source and media

The project contains source, prompts, settings and a synthesized fixture script.
Generated media and run data stay outside Git.
