# Movie sprite

A character image becomes a transparent body-idle loop. FX fits a green plate,
compiles the motion brief, makes one video call, then keys and closes the take
locally. You can instead supply footage and pay nothing for generation.

This is the source used by the [movie sprite showcase](https://grida.co/fx/workflows/movie-sprite).
The Nami generation recipe is `showcase.yaml`; `finish-showcase.yaml` reproduces
its reviewed local finishing. The historical traveler recipes remain in
`showcase.traveler.yaml` and `finish-traveler.yaml`.
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

Wan's prompt expansion defaults to enabled. FX's current queue adapter does not
expose that setting, so provider-side rewriting may affect the motion brief.
Content moderation also remains enabled. A matching first and last frame guides
the generation; it does not promise a clean loop or consistent animation.
Local finishing and visual review are still necessary.

## Reproduce the showcase's local finishing

From an FX checkout, the shared downloader fetches the published input image and
take into ignored `.showcase-inputs/`, checking their hashes before accepting them:

```sh
python ../fetch_showcase_inputs.py .
uv run python -m grida.fx plan workflow.yaml --inputs finish-showcase.yaml
uv run python -m grida.fx run workflow.yaml --inputs finish-showcase.yaml --no-view
```

This uses the existing paid Nami take and makes no provider calls. Two local
steps reproduce its reviewed result: `trim.nami.json` selects frames **[12,228)**
from the 30 fps take, then `finish.nami-face-pinned.json` keys and closes that
216-frame interval. Its 7.2 seconds of source motion play over 8 seconds at 27
fps. The explicit trim removes noisy leading and trailing frames without
generation or interpolation. Use this optional trim for opaque or green-screen
takes; it does not preserve an existing alpha channel.

The held polygon includes the face, adjacent head and parts of the raised
fingers; it preserves those pixels from the trimmed first frame while the body
idles. Its digest and native coordinates are valid only for this exact trimmed
take. Changing footage requires new spatial settings and review; the finishing
motion limit remains unchanged.

The trim uses bitexact container flags, verified with FFmpeg 9.0.1. Identical
compressed bytes are not promised across FFmpeg versions or builds. The strict
source digest will refuse a different encoding; recreate and review its
source-bound settings explicitly rather than weakening that check.

`finish-traveler.yaml` replays the historical traveler with
`finish.face-pinned.json`. `showcase.traveler.yaml` preserves that character's
generation brief. `showcase.yaml` generates a **new** Nami take, using
`finish.initial.json` without any take-specific trim or held polygon. Nami's
source is the original adult illustration, with only its alpha channel
normalized; its RGB pixels and 2048×3072 dimensions are unchanged.

For your own character, copy `showcase.yaml`, change the character and direction,
and plan first. A deliberate paid run requires provider credentials, `--live`
and a budget chosen by you, for example:

```sh
uv run python -m grida.fx run workflow.yaml --inputs showcase.yaml --live --max-usd 3 --no-view
```

New video takes are nondeterministic. Finishing may refuse excessive motion; a
fresh take needs its own review and source-bound face settings. A valid encode or
technical report does not approve artistic quality.

## Source and media

The project contains source, prompts, settings and a synthesized fixture script.
Generated media and run data stay outside Git. `nodes/_movie_sprite/LICENSE`
retains the original helper source's BSD 3-Clause notice; `NOTICE.md` describes
its adaptation. That source license makes no claim about downloaded artwork.
