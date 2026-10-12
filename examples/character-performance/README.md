# Reference-driven character performance

Turn one character illustration and one recorded performance into a short, silent
character clip. The picture supplies appearance; the video supplies movement and
timing. Reuse the clip inside a larger reel, advertisement or storyboard, or compose
two reviewed performances with the optional cast and reel workflows below.

Each take receives a technical check, a conservative border scan of every native
frame, and an independent review of identity, illustration style, anatomy,
framing and correspondence with eight synchronized reference poses. The border
scan is a pixel proxy for plain studio backgrounds; it does not establish
semantic quality. This is a bounded asset stage: a sampled pass does not establish
perfect transitions or reliability on unseen inputs. Character redraws, hand errors
and timing drift remain possible.

## Setup and source version

Install Python 3.11 or newer, [uv](https://docs.astral.sh/uv/), and FFmpeg with
`ffprobe` on PATH. Motion transfer and the composed cast workflow are additions in
this source revision; an older published Grida package may not include them.
Build the FX checkout containing this example using the
[source build instructions](https://github.com/gridaco/fx/blob/4323dd28a67b3d6906444bb1691d78dafa8fcca4/CONTRIBUTING.md#build-from-source),
then install this example's dependencies:

```sh
uv sync
```

Follow [Use a source build with Grida](https://github.com/gridaco/fx/blob/4323dd28a67b3d6906444bb1691d78dafa8fcca4/CONTRIBUTING.md#use-a-source-build-with-grida-fx)
to select the built engine and matching Python SDK before running the recipes.
`grida fx nodes video.motion_transfer` verifies that the selected engine exposes
`fx/video.motion_transfer@1`. A matching installed
[Grida](https://github.com/gridaco/fx/blob/main/README.md#install) release also runs
these standalone example files.

## Run one performance

Make an input file beside your media. Relative file paths resolve beside this file:

```yaml
# my-inputs.yaml
character: ./my-character.png
motion: ./my-performance.mp4
name: My character
start: 0
seconds: 4
fps: 24
background: f4eee6
body_fill: 0.6
```

Use one visible, unobstructed adult character, including the feet, against a simple
background. Transparent character art works best with the local plate preparation.
The driving video must be a portrait MP4 with one real person, visible head and
body, a fixed camera, and a selected interval of 3–10 seconds. This example accepts
width/height ratios of 0.45–0.8, at most 4096 pixels per side and 60 fps. Choose art
and footage you are permitted to upload and transform.

```sh
# Free local preparation executes while planning; no provider call.
grida fx plan workflow.yaml --inputs my-inputs.yaml --check

# An offline run refuses the two paid calls and spends $0.
grida fx run workflow.yaml --inputs my-inputs.yaml

# Requires FAL_KEY and OPENROUTER_API_KEY through FX's credential loader.
grida fx run workflow.yaml --inputs my-inputs.yaml --live --max-usd 10
```

The selected routes live in [fx.yaml](fx.yaml). The default motion route is Wan
Motion on fal, with pose retargeting enabled and optional identity preprocessing
disabled. Preparation explicitly samples the chosen interval at 24 fps by
removing or duplicating frames; it performs no temporal interpolation. Wan's
$0.06-per-24-frames tariff therefore matches the validated source duration, and
the route reserves $0.075 per second. Its bounded profile accepts 3–10 seconds
at 24 fps. A mismatched prepared frame rate is refused before uploading the clip.

The independent sampled-frame review uses Astra on OpenRouter. One performance
has two paid calls: motion transfer and review. Planning shows their estimate,
and the run's ceiling includes engine retries. The built-in Kling motion routes
are alternatives selected in `fx.yaml`; the workflow supports explicit 24 or
30 fps preparation. Wan requires 24 fps. Provider profiles and supported controls
are documented in the
[provider contract](https://github.com/gridaco/fx/blob/4323dd28a67b3d6906444bb1691d78dafa8fcca4/spec/providers.md#93-fal).

A technically completed run can contain a rejected study. Require
`report.review_status == "sampled_review_pass"` before treating a take as accepted.
Admission requires every sampled review criterion to pass and every native frame
to have a clear studio border scan. `sampled_review_status` retains the sampled
verdict separately; `framing_guard_fail` rejects edge contact, unknown studio
backgrounds or incomplete scans even if the sampled review passed. The report
retains review digests and limited scope. A semantic rejection remains recorded; the workflow performs
no automatic creative reroll. A new creative attempt is a separate run, while FX
owns transport retries and their reservations. Publication requires a separate
content review.


## Selection evidence

These bounded trials held Riko's 720 × 1280 plate (`body_fill: 0.6`), the selected
four-second [Polina interval](motion-reference.json), preservation brief and
sampled review criteria fixed. Route and explicit source-frame preparation varied;
Wan requires 24 fps. Native output dimensions were retained. The
[Kampus comparison](motion-reference-alternative.json) changed the performance
and interval, so it is a separate input case.

| Reference and configuration | Observed sampled result |
|---|---|
| Polina, Riko, Kling V3 Pro and 2.6 Pro, 25 fps reference | Identity, style and framing passed; pose correspondence failed |
| Same Riko plate and Polina timeline, Kling V3 Pro, only reference sampling changed to 30 fps | Pose correspondence still failed; frame-rate normalization alone did not resolve the mismatch |
| Simpler Kampus interval, Kling V3 Pro, 30 fps | Riko and Nami passed all sampled criteria |
| Original Polina interval, Wan Motion retargeting, 24 fps | Riko passed the sampled comparison, including overhead and head-side gestures; its first production take passed the eight samples but was later rejected for hand/hair clipping between them |
| Riko production 01, all native frames inspected | Border contact at 1.29–1.54 s exposed the sampled review's blind spot; the workflow now requires a clear all-frame studio-border scan |
| Same Riko configuration with only `body_fill: 0.45` | All sampled criteria and the 96-frame border scan passed; native inspection at 1.46 s confirmed both hands and hair have lateral margins |
| Polina, Wan, Nami plate at `body_fill: 0.5` | Identity, motion and style passed; framing failed because a rabbit ear was clipped in the native frame at 1.11 s |
| Same Nami configuration with only `body_fill: 0.4` | All sampled criteria and the 96-frame border scan passed; the native frame at 1.11 s confirmed clear ear margins |

Wan was selected for its observed transfer of the original expressive performance,
with explicit 24 fps preparation. These cases compare working configurations;
they establish no universal model ranking or success rate. The Nami failure also
shows why each character needs its own framing check; Riko also demonstrated why
eight samples alone cannot establish continuous framing. Rejected takes remain
rejected, and changed margins are a separate recorded creative attempt.

## A second performance reference

The [six-second balance study](inputs/dance.yaml) uses the same Riko preparation
and Wan route with a different [selected recording](motion-reference-dance.json).
Acquire the original from its publisher; the source footage is not hosted by FX:

```sh
python fetch_inputs.py
python fetch_motion.py --reference motion-reference-dance.json \
  --output .showcase-inputs/dance-motion.mp4
grida fx run workflow.yaml --inputs inputs/dance.yaml --live --max-usd 10
```

The frozen result follows the one-leg balance and free-leg extension in the
inspected poses. Independent review covered 24 chronological pose pairs, five
full-size pairs and the poster, in addition to the workflow's eight-pair review
and clear 145-frame border scan. Fine hand poses differ: relaxed source hands
become splayed or V-like fingers. The plain background establishes no floor
contact plane, and the generated extension keeps a more bent leg. This is an
additional bounded transfer case, not a claim of exact contact, finger fidelity
or reliability on unseen choreography. Fresh generation is nondeterministic
and needs its own review.

## A more active dance reference

The [active dance recipe](inputs/hiphop.yaml) uses Riko and the 16–22 second
interval of [Antoni Shkraba's recording](motion-reference-hiphop.json). In a fresh
example copy, set only `routes.video.motion_transfer` in `fx.yaml` to
`fal-ai/kling-video/v3/pro/motion-control@fal`, retaining the independent review
route. Then acquire the pinned inputs and plan before spending:

```sh
uv run --no-sync python fetch_inputs.py
uv run --no-sync python fetch_motion.py --reference motion-reference-hiphop.json \
  --output .showcase-inputs/hiphop-motion.mp4
grida fx plan workflow.yaml --inputs inputs/hiphop.yaml --check
grida fx run workflow.yaml --inputs inputs/hiphop.yaml --live --max-usd 10
```

This case compares different routes on the same performance. Wan retained the
coarse choreography but produced conspicuously enlarged palms. Kling V3 Pro
improved the inspected hand proportions, but its first take clipped a shoe in
43 of 180 native frames. Changing only `body_fill` from 0.45 to 0.4 produced the
accepted take: eight sampled review criteria and all 180 native border checks
passed. Independent publication review inspected 24 chronological pose pairs,
five complete native pairs, the identity comparison and poster.

That take retains the provider's native 1072 × 1920, 30 fps output. Cross-steps,
reaches, turns and the knee lift follow the inspected reference poses; fingers
remain simplified and floor contact is unverified. The native frame contains
substantial empty space above the character. A later crop is an editorial choice
requiring its own review. These observations describe these exact attempts;
they do not rank the models generally or certify every transition. A new run
is nondeterministic and needs fresh review. Raw and transcoded stock footage
remains excluded from FX's public package.

## Inspect or reuse one stage

| Stage | Input | Result |
|---|---|---|
| Prepare locally | illustration, raw driving video, explicit interval and fps | 720 × 1280 opaque character plate, silent trimmed reference, preparation report |
| Transfer performance | prepared plate, prepared video, preservation brief | native generated MP4, with no forced output resizing |
| Finish locally | generated MP4, both references | silent MP4, poster, identity comparison, eight synchronized frame pairs, full-decode and all-frame studio-border report |
| Independently review | identity comparison and frame pairs | structured identity, pose correspondence, anatomy/framing and style verdict |
| Admit | review and technical report | accepted or rejected take with review scope retained |

Preparation fits existing character pixels, crops transparent margins and
normalizes source alpha. `body_fill` (0.4–0.8; default 0.6) bounds the subject's
width and height relative to the plate, leaving room for lateral and overhead
gestures. Match this scale to the driving performer. Preparation does not repaint
the character. Reference audio is removed before upload; final audio is removed
again. Finishing preserves native output dimensions, frame rate and timing.
Every video frame is decoded for technical validity. The border guard compares
all four outer edges with an estimated background from four 8 × 8 corner patches.
A maximum RGB-channel difference above 20 marks possible foreground contact.
Corner differences above 8 make the result unknown and block admission in this
bounded plain-studio workflow. The report lists affected frame indices, native
timestamps and sides; temporary decoded PNGs are removed. A clear result is a
conservative framing check, while artistic review remains explicitly sampled.

Run the free input stage independently when choosing an interval:

```sh
grida fx run prepare.yaml --inputs my-inputs.yaml \
  --deliver character=.showcase-inputs/character-plate.png \
  --deliver motion=.showcase-inputs/motion-prepared.mp4 \
  --deliver report=.showcase-inputs/preparation.json
```

To use these exact prepared inputs, make another input file with `prepared: true`,
`start: 0`, their exact duration, and their actual frame rate:

```yaml
character: ./.showcase-inputs/character-plate.png
motion: ./.showcase-inputs/motion-prepared.mp4
prepared: true
seconds: 4
fps: 24
name: My character
```

Prepared inputs are checked for portrait dimensions, one video track, declared
frame rate and duration, complete decoding, absent audio and a fully opaque
720 × 1280 character plate before the paid stage.

[finish.yaml](finish.yaml) can inspect already supplied footage at $0. Its input
file adds `video: ./my-generated-take.mp4` to the prepared character, motion and
duration. This free path produces evidence with review pending.

When adapting these Python nodes, increment a node's declared `version` whenever
its behavior changes. FX uses that declared version for cache identity; changing
the Python body alone does not invalidate a stored result. The finishing and
admission nodes use version 2 for the all-frame border guard, so older sampled-only
reports are recomputed through the guard before reuse.

## Compose two characters

[cast.yaml](cast.yaml) composes two instances of the same performance workflow
and then [reel.yaml](reel.yaml). Supply two prepared plates and one shared,
silent 24 fps driving interval. [inputs/cast.yaml](inputs/cast.yaml) names the
canonical Riko and Nami plates and four-second reference:

```sh
grida fx plan cast.yaml --inputs inputs/cast.yaml --check
grida fx run cast.yaml --inputs inputs/cast.yaml --live --max-usd 20
```

A fresh cast has four paid calls: one transfer and one review for each character.
They share the engine's spending ceiling, cache and retry owner. Exact stored
results can be reused; the reel refuses a rejected report or one whose video
digest differs from its supplied clip. The two characters perform separately in
sequence. By default, aspect fit preserves each complete frame.

To compose performances you already generated and accepted, place their videos
and matching reports at the paths in [inputs/reel.yaml](inputs/reel.yaml), or edit
that file to point to your own deliveries. These example paths are
`.showcase-inputs/riko-performance.mp4`, `riko-report.json`,
`nami-performance.mp4` and `nami-report.json`. The reports travel with their exact
video bytes. Run the free composition:

```sh
grida fx run reel.yaml --inputs inputs/reel.yaml \
  --deliver video=.delivery/cast-reel.mp4 \
  --deliver poster=.delivery/cast-reel.webp \
  --deliver report=.delivery/cast-reel.json
```

The reel adds original typography and concatenates the complete performances
into a silent 1080 × 1920, 30 fps video. For two four-second inputs, the result is
eight seconds. Converting a 24 fps performance to the reel's 30 fps duplicates
frames without interpolation; it does not add generated movement. Full decoding
and source review lineage are checked. The composition's report retains
`requires_separate_content_review`: inherited sampled passes are admission
evidence, while the final edit still needs its own visual review.

Optional `poster_at` selects a moment in finished-reel seconds. It defaults to
1 second and must fall inside the composition; the selected frame rounds down
to the 30 fps output grid. Changing this input changes only the poster and its
recorded selection, not the video pixels. The report binds the exact frame index,
timestamp and poster digest. The sequential recipe selects Riko's upright reach
at 3.2 seconds rather than the earlier crouched pose. Reel node version 5 records
this explicit selection.

Use `mode: shared_timeline` to switch identity once while continuing the same
recorded performance. The two accepted reports must bind the same `motion_sha256`,
and both source durations must match on the 30 fps output grid. `switch_at`
defaults to 2 seconds, rounds down to an output frame and must leave at least
half a second for each character. The first clip supplies the beginning; the
second supplies the remaining interval from that same time, rather than
restarting its dance. Two four-second sources produce a four-second result.
This is an editorial identity cut; matching pose continuity still needs review.
The report records both selected source intervals and the actual switch frame.

The [shared reel recipe](inputs/shared-reel.yaml) uses the exact canonical clips
with one common fixed region, keeping the fitted stage dimensions consistent:

```sh
grida fx run reel.yaml --inputs inputs/shared-reel.yaml
# The same composition controls also work with the full paid cast graph.
grida fx run cast.yaml --inputs inputs/shared-cast.yaml --live --max-usd 20
```

Optional `first_region` and `second_region` inputs in the reel or cast are static,
caller-authored framing rectangles: `[x, y, width, height]`, normalized to each
source frame. Their default is `[0, 0, 1, 1]`. The rectangle must be finite,
nonempty and entirely within the source; its integer pixel crop expands outward
to contain the requested bounds, then aspect fit preserves that selected region.
Choose a region that contains every gesture in the complete clip. These controls
do not detect poses or follow a moving subject, and source acceptance does not
approve a later crop. Review the final composition for clipped ears, raised hands
and feet before publishing it.

The canonical input files select fixed regions after inspecting all 96 native
frames of each accepted take. These regions request a 48-pixel margin around the
observed foreground bounds, limited by the source edges; Nami's right margin is
39 pixels. They belong to those exact clips; recheck framing
and revise the regions when generating new performances.

## Canonical references

The canonical character set is Riko, Nami and Sena. Each is a separate identity;
new roles and stories may be written around their visual concepts. A reference
sheet, turnaround or rig is a separate input when a future workflow needs one.
A single front illustration does not establish the unseen back of the character.
Riko and Nami are the distributable example inputs. Sena remains a local
development reference while upstream source-art redistribution is unresolved;
her raw references are excluded from the public package.

`fetch_inputs.py` acquires checksum-pinned FX-owned illustrations and published
performance deliveries into `.showcase-inputs/`. Their file names match the
canonical input recipes:

```sh
uv run --no-sync python fetch_inputs.py
```

The selected motion reference is a short full-body performance by Polina
Tankilevitch, acquired from [Pexels](https://www.pexels.com/video/woman-dancing-actively-5385812/).
[motion-reference.json](motion-reference.json) records the source, checksum, terms
and use boundary. The selected interval is 0–4 seconds, explicitly prepared at
24 fps. `fetch_motion.py` acquires just this selected clip, or validates a manually
downloaded copy:

```sh
uv run --no-sync python fetch_motion.py
# Alternatively:
uv run --no-sync python fetch_motion.py --source /path/to/downloaded-clip.mp4
```

To prepare the canonical shared motion and Riko plate, run `prepare.yaml` with
[inputs/riko.yaml](inputs/riko.yaml) and deliver its character to
`.showcase-inputs/riko-plate.png` and motion to
`.showcase-inputs/motion-prepared.mp4`. Prepare Nami's plate with
[inputs/nami.yaml](inputs/nami.yaml), delivering its character to
`.showcase-inputs/nami-plate.png`. The shared motion interval and frame rate are
the same in both input files. [showcase.yaml](showcase.yaml) runs Riko directly
from those prepared references; `inputs/cast.yaml` uses both plates.

The Pexels license permits modification and adaptation. Applying a selected clip
as an inference reference is an interpretation of that grant, rather than an
explicit AI permission in its terms. Raw and transcoded stock footage is excluded
from FX's public media distribution; acquire it from the source. The example
publishes its transformed character clip and composed review evidence. Neither
FX nor the character implies endorsement by the performer.

## Provider-free verification

```sh
uv run --no-sync python check.py
uv run --no-sync python check_reel.py
```

These checks draw original placeholders and synthesize test videos, then execute
the graphs with public SDK stand-ins. They check paid-call refusal, complete
decoding, sampled-evidence output, admission, exact cache replay, invalid intervals,
corrupt media, prepared alpha and video-track rejection, composition lineage and
full-frame preservation. The composed cast uses exactly four stand-in provider
calls at $0. Network, dotenv loading and provider keys are disabled. These are
executable contract checks, not a verdict about generated artwork.

Generated media, local inputs, run records and delivery bundles stay outside Git.
See [workflow.yaml](workflow.yaml), [nodes/performance.py](nodes/performance.py),
[review instructions](prompts/review.md) and [review schema](schemas/review.json)
for the complete source.
