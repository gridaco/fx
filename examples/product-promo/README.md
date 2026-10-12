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

The caller owns source capture, approval of editorial cuts, narration, audio rights, factual
product claims, and final placement in a larger edit. By default the utility evenly retimes
the complete source into the chosen body duration. An explicit `source_segments` plan can
omit waits and retime retained intervals independently, preserving the supplied pixels.
The proposal may select these intervals when the brief requests wait removal; review the
saved plan against the full recording before publication. Audio is intentionally omitted,
so music, voiceover, and sound design can be assembled independently.

Three stages are individually replaceable:

1. **Inspect:** validate and fully decode the source recording and brief, then produce 12 timestamped
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
source/brief/font digests, edit decisions, playback rate or interval timeline, dimensions, frame count, and the
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

For a longer recording with idle generation time, a plan may additionally contain:

```json
"source_segments": [
  {"source_start": 4.8, "source_end": 7.3, "duration_seconds": 2.5},
  {"source_start": 22.5, "source_end": 25.5, "duration_seconds": 2.5},
  {"source_start": 28, "source_end": 31, "duration_seconds": 2.5},
  {"source_start": 37.5, "source_end": 44, "duration_seconds": 6.5}
]
```

These are source seconds, followed by each interval's output duration. Intervals must be
chronological, non-overlapping, finite, and confined to the actual recording. Use 1–8
intervals, each at least 0.1 source seconds and 0.5 output seconds. Output durations must
contain whole 30 fps frames and sum exactly to `footage_seconds`. Omitted ranges become
clean cuts. Intervals include their start and exclude their end; every supplied source-time
focus hint must lie inside a retained interval. Focus beat `at` values remain output body
seconds. The report records each interval's playback rate and the mapped body/video time
of every hint. An omitted `source_segments` field retains the continuous playback path.

The optional [dashboard fixture](fixtures/dashboard.html) tests this boundary with a
different operation: a table becomes a dashboard, then chat focuses returning customers.
Its original 44-second browser capture includes deliberate waits. The saved
[dashboard plan](inputs/dashboard-edit.json) selects four intervals for a 19-second promo,
revealing the first dashboard at 4.9 seconds and holding the final result. Capture it with
Playwright and an installed Chromium browser, then replay locally:

```bash
# Optional fixture authoring dependency; the workflow itself does not use a browser.
uv run --with playwright playwright install chromium
uv run --with playwright python make_dashboard.py
# Alternatively pass --browser /path/to/chromium to use an installed browser.
grida fx run workflow.yaml --inputs dashboard-replay.yaml \
  --deliver video=dist/dashboard.mp4 --deliver poster=dist/dashboard.webp
```

This captures original DOM/CSS at authored timeline positions, rather than recording a
shipping service or live generation. No outside product capture or generated UI is used.
For your own recording, use its actual source timings and review every retained cut.
The dashboard's model-selected intervals and timing are unchanged. The caller expanded
one canvas crop to preserve the complete filename and corrected the authored source's
row count and chart-label alignment before final review. The exact result passed all eight
independent sampled criteria plus a separate chronological and targeted-cut frame review.
To replay those frozen source pixels without a browser, fetch the pinned inputs and use
[dashboard-showcase-replay.yaml](dashboard-showcase-replay.yaml):

```bash
python fetch_inputs.py
grida fx run workflow.yaml --inputs dashboard-showcase-replay.yaml
```

The framework is deliberately a single composition, not a general video editor. Device
mockups, independent asset tracks, audio mixing, and portrait exports are further
composition work; the supplied recording and exact-copy contracts remain useful boundaries.

## Frozen showcase and review

The selected canonical is a 17.5-second Forma clip. Its first canvas appears around
5.1 seconds, followed by a readable refinement and a sustained warmer result. The caller
authored four source intervals and revised camera beat times in the original model's
accepted plan; [replay-segments.yaml](replay-segments.yaml) supplies that saved edit and
renders locally at $0. The source recording, exact copy, and crop regions are unchanged.
The original 18.5-second checkpoint and its unchanged model proposal remain available
through [replay.yaml](replay.yaml).

The same workflow also rendered a 19-second original poster-design recording with a
different brief and palette. Both Garden edits and the poster variation passed all eight
independent sampled-review criteria. The timing revision additionally received independent
chronological and dense-cut frame review. A few inputs are compatibility evidence, not a
general success rate. Source fixtures, exact copy, regions, focus hints, and the local
composition were authored and refined during development.

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
# The selected caller-refined edit replays locally without a provider call.
grida fx run workflow.yaml --inputs replay-segments.yaml
# The original checkpoint remains available.
grida fx run workflow.yaml --inputs replay.yaml
# Fresh optional planning may choose a different edit.
grida fx run workflow.yaml --inputs showcase.yaml --live --max-usd 2
```
The source generator above is the fully offline alternative. A maintained example and a frozen recording have separate lifetimes;
fresh planning may choose different timing.

## Verify offline

```bash
uv run python check.py
```

The check strips provider credentials, disables dotenv/network, creates deterministic
fixtures, plans the real graph, proves paid-call refusal, renders with a saved plan, fully
decodes the result, and proves exact cache replay. A 12 fps synthetic source verifies actual
decoded output colors and the precise cut frame after interval retiming. It also checks
source-hint mapping, omitted hints, overlap, source bounds, duration sums, and whole-frame
output durations. Damaged footage fails at $0. Invalid crop bounds, camera ordering,
non-finite coordinates, excessive zoom, and malformed copy are refused. These are structural checks,
not a substitute for a visual review of your own footage.

The bundled [Manrope font](fonts/Manrope.ttf) is licensed under the
[SIL Open Font License](fonts/OFL.txt). Its [upstream source](https://github.com/google/fonts/tree/main/ofl/manrope)
is included for attribution. No outside product footage, reference artwork, soundtrack,
or private business material is part of this example.
