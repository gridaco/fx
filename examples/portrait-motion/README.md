# Portrait motion

A finished PNG becomes independently usable eyes and mouth states. The workflow
locates a face, decides which features can move, draws one state sheet, registers
it, outlines the features, reviews every combination and restores accepted patches
at the original size. The source alpha and pixels outside the patches stay intact.

This is the source used by the [portrait motion showcase](https://grida.co/fx/workflows/portrait-motion).
Its input is a 2048×2048 closeup of an original rabbit-hood character.
This example focuses on
facial patches; it does not need a body animation or the movie-sprite example.
The helpers in `nodes/_portrait_motion/` belong to this example, not the FX API.
It runs without any other checkout or application package.

## Setup

Install [uv](https://docs.astral.sh/uv/), then run in this folder:

```sh
uv sync
```

NumPy, SciPy and Pydantic are example dependencies, not FX dependencies.
Commands below use the Python installation; with the `grida` host installed,
`uv run grida fx` is equivalent to `uv run python -m grida.fx`.

## Try it without a provider

The fixture draws a geometric sprite and an opaque portrait:

```sh
uv run python inputs/make_inputs.py .fixtures
uv run python -m grida.fx plan workflow.yaml --inputs .fixtures/face.yaml
```

Fresh-cache output from the current engine:

```text
portrait-motion  ·  1 phase
phase 1   56 steps   1–25 provider calls   $0.05 – $36.43
cached    0 of 7 known steps
estimate  $0.05 – $36.43
```

The high end includes bounded judge regenerations, not just the happy path.
An ordinary run without `--live` refuses the first provider call at $0.
The fixture also provides deterministic stand-ins for all paid calls, so the
entire face and whole-portrait paths can run for free through the public SDK:

```sh
uv run python check.py
```

The stand-in returns the drawn guide unchanged and synthetic passing decisions.
It verifies execution, branching, registration and pixel-preservation contracts;
it does not demonstrate animation quality or replace semantic review.

## Use the showcase input

From an FX checkout, download the canonical face into ignored `.showcase-inputs/`
with digest verification, then plan the real workflow:

```sh
python ../fetch_showcase_inputs.py .
uv run python -m grida.fx plan workflow.yaml --inputs showcase.yaml
```

`showcase.yaml` and `inputs/face-closeup.json` preserve the actual showcased
input specification. The input image is downloaded; generated artwork and run
records are not committed here. Its 2048×2048 working sheet has four 1024×1024
donor cells. Those are processing dimensions: the delivered states remain at the
input's native 2048×2048, with patches restored at their original offsets.
For your own sprite, replace `portrait` and edit
the spec's state descriptions and timeline. With `face_crop: false`, the input
must instead be opaque and exactly match the spec's working canvas.

The supported starting state is open eyes and a closed resting mouth. Leave
clear skin around the full eyelid and lash contours; a visible iris alone is not
enough when hair crosses the replacement boundary. The workflow returns a partial
result or a refusal when those constraints cannot be met.

A paid run needs provider credentials, `--live` and an explicit ceiling chosen
by you, for example:

```sh
uv run python -m grida.fx run workflow.yaml --inputs showcase.yaml --live --max-usd 40 --no-view
```

Results include a status record (`complete`, `partial` or `refused`), accepted
state PNGs, native face patches, a lossless animated WebP and a manifest. A valid
refusal is a useful result. A passed still-image review does not claim temporal
quality; visual review and publication approval remain separate.

## Source and media

The project contains source, prompts, schemas, specifications and synthesized
fixture code. `nodes/_portrait_motion/LICENSE` retains the original helper
source's BSD 3-Clause notice; `NOTICE.md` describes its adaptation. That source
license makes no claim about downloaded artwork.

The source illustration is prepared separately by `source-workflow.yaml`, using
`source-showcase.yaml` and the prompt in `prompts/source-closeup.md`. This optional
paid image edit creates a resting mouth and unobstructed open eyes before the
facial workflow runs. Running the facial workflow never invokes it automatically.

To prepare a different source from the downloaded reference, plan it first:

```sh
uv run python -m grida.fx plan source-workflow.yaml --inputs source-showcase.yaml
# Optional paid edit, with a separate ceiling chosen by you:
uv run python -m grida.fx run source-workflow.yaml --inputs source-showcase.yaml --live --max-usd 3 --deliver portrait=portrait.png --no-view
```

Use `--portrait portrait.png` with the main workflow to override the recorded
showcase input. Keep its own live ceiling separate from source preparation.

Its reference is a native crop at integer bounds `(580, 80, 1476, 976)` from
Stage Gen's public, repository-owned Nami `reaching_pose.webp` (2048×1536,
SHA-256 `468768f45ec08b5a69be1a4cb496a8068cf0d6f65c3f2bf8dd231cabfee7a315`).
No private reference or unpublished character variant is used. The unmodified
reference only admitted one eye: swept hair obstructed the other eye, and its
wide-open mouth was outside this workflow's starting-state scope. The source edit
addresses those input constraints without bypassing admission.

`rest` always means the unchanged source expression. The generated mouth states
are a smile and an open A. The fixture keeps the smaller four-card specification
in `inputs/face-four-card.json` for fast offline checks.
