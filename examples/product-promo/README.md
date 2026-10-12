# Product promo

Turn a prepared software recording into a short, editable launch clip: exact titles,
readable camera closeups, an outcome reveal, and a clean poster. The recording's pixels
stay intact. A model proposes timing and framing; local Python and FFmpeg render the result.

This is a bounded insert utility for a larger launch video. It accepts prepared footage,
not an idea for an entire product, a complete storyboard, or a shipping application.
Its current composition is a silent 1920×1080, 30 fps MP4 lasting 10–22 seconds.

## Run the original example

Install [FFmpeg](https://ffmpeg.org/download.html) on PATH and the example's dependencies:

```bash
uv sync
uv run python make_fixture.py
# Plan for free; no keys or network are used by planning.
grida fx plan workflow.yaml --inputs .fixture/garden.json --check --max-usd 2
# OPENROUTER_API_KEY admits the one paid proposal; rendering is local.
grida fx run workflow.yaml --inputs .fixture/garden.json --live --max-usd 2 \
  --deliver video=dist/promo.mp4 --deliver poster=dist/poster.webp \
  --deliver edit=dist/edit.json --deliver report=dist/report.json \
  --deliver storyboard=dist/storyboard.png
```

The current engine's plan, with the original fixture:

```text
product-promo  ·  1 phase
phase 1   3 steps   1 provider calls   $0.02 – $0.60
cached    0 of 1 known steps
estimate  $0.02 – $0.60   ceiling $2.00
```

`make_fixture.py` draws an original scripted recording of **Forma**, an invented
chat-native design tool. The user asks for a garden landing page, then refines its palette.
This is a clearly authored fixture, not evidence of a shipping product's behavior.
It needs no network or image/video model. `--variant poster` creates a different original
recording with an exhibition poster and a violet palette:

```bash
uv run python make_fixture.py .fixture --variant poster
grida fx run workflow.yaml --inputs .fixture/poster.json --live --max-usd 2
```

The fixture generator is separate from the utility. Replace the recording with your
own rights-cleared screen capture for a real product demonstration.

## Inputs and reusable boundary

[workflow.yaml](workflow.yaml) accepts three inputs:

| Input | What the caller supplies |
| --- | --- |
| `footage` | A prepared video with one video stream, 1–120 seconds; readable source UI and the desired behavior already present |
| `brief` | Exact product name, opening/closing lines, support copy, colors, intent, and optional timestamped focus hints; see [inputs/forma.json](inputs/forma.json) |
| `edit` | Optional saved camera/timing JSON; skips the paid proposal, see [inputs/forma-edit.json](inputs/forma-edit.json) |

The caller owns source capture, pause removal, narration, audio rights, factual product
claims, and final placement in a larger edit. The utility evenly retimes the complete
source into the chosen body duration; it does not infer cuts or invent product states.
Keep the source concise when timing and readability matter. Audio is intentionally omitted,
so music, voiceover, and sound design can be assembled independently.

Three stages are individually replaceable:

1. **Inspect:** validate source dimensions/duration and brief, then produce 12 timestamped
   chronological samples plus portable metadata and digests.
2. **Propose:** one image-aware `structured.generate` call produces a typed camera/timing
   plan. Focus hints bind important interactions to source seconds and image fractions.
3. **Render:** validate the plan, animate exact text from the brief, play the original
   recording pixels through source-bound camera framing, then verify and fully decode
   the MP4. Focus beats hold their framing, moving during the final 0.75 seconds before
   the next beat when the crop is unchanged. Distinct crop regions use clean cuts
   at the start of that interval, avoiding half-visible inactive UI panes. The first and last body beats show the whole recording.

Outputs are `video`, `poster`, `edit`, `report`, `storyboard`, and `source`. The poster is a
clean, complete final body composition without title-transition blending. `report` records
source/brief/font digests, edit decisions, playback rate, dimensions, frame count, and the
full-decode verdict. It does not claim artistic acceptance.

## Revise locally

The accepted edit can be saved, reviewed, and changed without proposing it again. Run the
original fixture with its saved plan at $0:

```bash
uv run python make_fixture.py
# replay.json is authored alongside the fixture, pointing to inputs/forma-edit.json.
grida fx run workflow.yaml --inputs .fixture/garden-replay.json \
  --deliver video=dist/replay.mp4 --deliver poster=dist/replay.webp
```

Change only exact titles or colors in a copy of the brief, or edit focus coordinates and
seconds in a copy of the plan. A saved plan must begin at body time zero and finish at
`footage_seconds`, with 3–8 sorted beats. Each beat has `at`, `x`, `y`, and `zoom`; coordinates
are normalized source fractions and zoom is 1–2.25. An optional `crop` is `[left, top, right, bottom]`
in normalized source coordinates. Named `regions` in the brief and a `region` on each focus
hint guide the proposal toward complete UI areas. Crops isolate the active content,
without painting over the recording. Use zoom 1 with a prepared crop to preserve its
complete content. The canonical example provides request, canvas, and refinement regions. The schema is in
[schemas/edit.json](schemas/edit.json); local validation also checks ordering, spacing,
complete opening/closing framing, and total duration. Invalid plans fail before encoding.
No paid call occurs when an `edit` input is supplied.

The framework is deliberately a single composition, not a general video editor. Device
mockups, arbitrary multishot editing, audio mixing, and portrait exports are further
composition work; the supplied recording and exact-copy contracts remain useful boundaries.

## Frozen showcase and review

The canonical recording is an 18.5-second Forma clip. The same workflow also rendered a
second 19-second original poster-design recording with a different brief and palette.
Both passed all eight independent sampled-review criteria. Two inputs are a small
compatibility sample, not a general success rate. The selected model plan was used
unchanged; the source fixture, exact copy, regions, focus hints, and composition were
authored and refined during development. The frozen production run reused the exact
cached proposal and rendered locally at $0.

The first independent review identified an outcome crop and an insufficient result hold.
The composition was revised to hold framing between shorter moves, keep the final hero
readable, and provide a clean full-interface poster. Review is separate from provider
transport retries. [workflows/review.yaml](workflows/review.yaml) optionally asks a different
model to evaluate timestamped samples and the full-size poster:

```bash
grida fx run workflows/review.yaml --inputs review-inputs.yaml --live --max-usd 2
```

[review-inputs.yaml](review-inputs.yaml) names the delivered `storyboard`, `poster`,
`brief`, and `report`. This review can evaluate
sampled layout, exact copy, and narrative coherence; it cannot establish continuous motion
or listening. The local renderer separately verifies dimensions, frame rate, and full-file
decoding. The silent composition makes no audio-quality claim.

The public frozen input is described in [showcase-inputs.json](showcase-inputs.json).
Download it with the example-local [pinned-input fetcher](fetch_inputs.py),
then use [showcase.yaml](showcase.yaml):

```bash
python fetch_inputs.py
grida fx run workflow.yaml --inputs showcase.yaml --live --max-usd 2
# Its saved accepted edit replays locally without a provider call.
grida fx run workflow.yaml --inputs replay.yaml
```
The source generator above is the fully offline alternative. A maintained example and a frozen recording have separate lifetimes;
fresh planning may choose different timing.

## Verify offline

```bash
uv run python check.py
```

The check strips provider credentials, disables dotenv/network, creates deterministic
fixtures, plans the real graph, proves paid-call refusal, renders with a saved plan, fully
decodes the result, and proves exact cache replay. Damaged footage fails at $0. It also rejects invalid crop bounds, camera ordering,
non-finite coordinates, excessive zoom, and malformed copy. These are structural checks,
not a substitute for a visual review of your own footage.

The bundled [Manrope font](fonts/Manrope.ttf) is licensed under the
[SIL Open Font License](fonts/OFL.txt). Its [upstream source](https://github.com/google/fonts/tree/main/ofl/manrope)
is included for attribution. No outside product footage, reference artwork, soundtrack,
or private business material is part of this example.
