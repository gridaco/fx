# Movie sprite

A character image becomes a transparent body-idle loop. FX fits a green plate,
compiles the motion brief, makes one video call, then keys and closes the take
locally. You can instead supply footage and pay nothing for generation.

This is the source used by the [movie sprite showcase](https://grida.co/fx/workflows/movie-sprite).
The published result also uses the source-bound face pin in `finish.face-pinned.json`.
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

The fixture script draws a geometric figure and encodes a tiny transparent clip:

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
phase 1   4 steps   1 provider calls   $0.09 – $0.11
cached    0 of 2 known steps
estimate  $0.09 – $0.11
```

Running that path without `--live` refuses the video call at $0. Estimates depend
on the chosen duration and route; a live cap is a ceiling, not the quoted price.

## Reproduce the showcase's local finishing

From an FX checkout, the shared downloader fetches the published input image and
take into ignored `.showcase-inputs/`, checking their hashes before accepting them:

```sh
python ../fetch_showcase_inputs.py .
uv run python -m grida.fx plan workflow.yaml --inputs finish-showcase.yaml
uv run python -m grida.fx run workflow.yaml --inputs finish-showcase.yaml --no-view
```

This uses the existing paid take and the exact face polygon, source digest and
native coordinates used for the showcase. It makes no provider calls. The face
pin is valid only for that take; changing footage requires new spatial settings.
`showcase.yaml` preserves the original character, direction and motion constraints
for generating a **new** take, using `finish.initial.json` without that historical pin.

For your own character, copy `showcase.yaml`, change the character and direction,
and plan first. A deliberate paid run requires provider credentials, `--live`
and a budget chosen by you, for example:

```sh
uv run python -m grida.fx run workflow.yaml --inputs showcase.yaml --live --max-usd 2 --no-view
```

New video takes are nondeterministic. Finishing may refuse excessive motion; a
fresh take needs its own review and source-bound face settings. A valid encode or
technical report does not approve artistic quality.

## Source and media

The project contains source, prompts, settings and a synthesized fixture script.
Generated media and run data stay outside Git. `nodes/_movie_sprite/LICENSE`
retains the original helper source's BSD 3-Clause notice; `NOTICE.md` describes
its adaptation. That source license makes no claim about downloaded artwork.
