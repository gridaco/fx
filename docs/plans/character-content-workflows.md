# Reusable video workflow examples

This note records the boundaries and evaluation criteria for two standalone
examples: [product promo](../../examples/product-promo/README.md) and
[character performance](../../examples/character-performance/README.md).
Their READMEs own the current commands, selected routes and limitations. The
[showcase guide](../../www/docs/showcases.md) owns portable recordings and media
delivery. Example source, generated media and review evidence are separate
artifacts.

## Chosen boundaries

| Example | Caller supplies | Workflow returns | Caller still decides |
|---|---|---|---|
| Product promo | A software recording, exact-copy brief, named source regions and timestamped focus hints | An editable shot plan, a silent rendered clip, poster, storyboard and structural report | Which product behavior to show, the message, accurate source regions and final editorial acceptance |
| Character performance | A character illustration, a short human driving video and framing constraints | A silent motion-conditioned clip, poster, identity and pose comparisons, technical report and sampled review | Identity rights, a suitable driving performance, creative context and final visual acceptance |

Both examples do one bounded job that can be composed into a larger production.
Neither requires a campaign, a complete application or a game. A broader demo
can combine these jobs, but it must preserve their individual input contracts
and disclose additional editorial decisions.

## Product promo

The canonical subject is **Forma**, a fictional minimal design tool operated
through chat. Its original source fixture shows a request, a generated canvas
and a revision. The fixture is explicitly synthetic; it does not claim to be a
recording of a shipping product. The workflow itself accepts supplied video and
does not depend on the fixture generator or privileged access to UI elements.

The utility turns a prepared recording and a concise brief into an editorial
clip. A language model proposes timing, camera emphasis and exact-copy text
placement in structured data. Local rendering preserves source pixels and
renders typography deterministically. Distinct source regions use deliberate
cuts; moves within a region can interpolate smoothly. Generative reconstruction
of the interface is unnecessary for this job.

The caller supplies:

- A software recording with the behavior to demonstrate.
- Exact copy and a bounded message, duration and output specification.
- Named normalized source regions that contain the relevant UI.
- Timestamped focus hints that identify useful moments in the recording.

A text-only idea or storyboard sketch belongs upstream unless a separate,
tested interpretation stage converts it into these inputs. Merely attaching a
sketch does not demonstrate storyboard understanding. Source preparation must
remove confidential screens and visible private text before any provider call
or public packaging.

The canonical result is 18.5 seconds at 1920 × 1080, 30 fps, intentionally
silent. A second original recording and brief produce a 19-second variation.
Both received independent review of twelve timestamped storyboard samples, the
full-size poster, the exact-copy brief and the structural report. This sample
review does not establish continuous motion quality or behavior on arbitrary
recordings. The renderer also strictly decodes every output frame; decoding is
technical evidence, not an artistic verdict.

The selected edit plan is the model's output. The source fixture, brief, source
regions, focus hints, prompt and renderer were authored and refined during
development. Those interventions are part of the workflow's actual boundary.
The saved plan can be rendered again locally without another paid generation.

Acceptance criteria:

1. The intended message and product action are understandable.
2. Supplied copy is exact and readable at the output size.
3. Framing preserves relevant UI, and displayed behavior follows the recording.
4. Timing and transitions support the brief without exposing unrelated screens.
5. The result decodes completely and can be used as a clip in another edit.

Device placement, narration, interactive branching and human-led storytelling
are possible separate utilities. They are outside this example's demonstrated
contract. A linear promotional video alone does not establish interactivity.

## Character performance

The utility uses a reference image for identity and a real driving video for
motion. A generic text request to dance is insufficient evidence of motion
transfer. Preparation, generation, finishing and semantic review remain separate
stages so users can replace a stage or inspect why a take was rejected.

The caller supplies:

- An approved illustration with one recognizable character and visible body.
- A rights-cleared, single-person driving performance with readable limbs,
  suitable camera framing and a useful short interval.
- Character name, duration, source start, background and body-fill constraints.
- Optional production direction that preserves the identity and intended motion.

Free preparation fits the illustration onto an opaque portrait plate without
repainting it and normalizes the selected driving interval. Matching the
character's occupied area to the performer, and leaving room for gestures,
matters more than filling the frame. A prepared-input path validates exact
image and video bytes without copying the driving video into the public export.
The selected Wan Motion route requires an explicitly prepared 24 fps source of
3–10 seconds. Preparation converts frame rate by frame duplication or dropping;
it does not synthesize intermediate poses. Other supported routes have their
own contracts, so changing a route requires checking the source preparation.

The paid motion route receives both references. Local finishing removes audio
and metadata, retains native video dimensions, strictly decodes all frames and
scans each frame's borders against its uniform studio background. Border contact
or an ambiguous background blocks admission; this conservative pixel proxy
cannot certify anatomy. Finishing also produces synchronized pose comparisons.
An independent structured reviewer
checks the authoritative character and eight paired driving/result frames.
The report records the exact input, take, finished-video and review digests.

A technically successful run can still have `review_status` set to
`sampled_review_fail` or `framing_guard_fail`. Consumers must inspect this field before using a take as
accepted content. Semantic rejection does not secretly regenerate the video.
The optional local reel utility also requires passing reviews, a clear border
scan covering every native frame, and matching video digests before encoding supplied clips.
The composed cast example invokes the same utility twice for Riko and Nami,
then fits both performances into a silent eight-second vertical reel. Its
local composition runs at 30 fps and carries source review digests; the new
layout still needs its own visual review.
The whole-frame default may retain unnecessary background. Callers can supply
fixed normalized framing regions; composition then preserves each complete
selected region. The canonical edit uses regions checked against every native
frame's visible bounds, with additional margin.

Acceptance criteria:

1. Hair, face, silhouette and costume retain the intended character identity.
2. Sampled key poses and gesture timing correspond to the driving performance.
3. The complete requested body remains visible, including extended hands and feet.
4. No severe limb fusion, extra people or distracting style change is visible.
5. The entire file decodes and its duration follows the prepared reference.

Eight frame pairs cannot certify every transition, fine finger anatomy or exact
continuous choreography. The comparison is a useful rejection gate, followed by
content review of the selected result. It does not establish a general success
rate. Silent output makes no speech, music or listening-quality claim.

Driving footage may come from ordinary stock licensing when its actual terms
permit the intended adaptation. Record the selected creator, source, exact
bytes, terms and the interpretation used for inference. A stock license is not
an express model-training grant. Do not rehost raw or trimmed stock footage as
an FX asset when standalone redistribution is excluded. The example can fetch
that source from its publisher while FX hosts its own character inputs and
reviewed adaptations.

## Canonical characters

Riko, Nami and Sena are selected character concepts. An example uses the subset
it needs; it does not inherit a source game's story, relationships, dialogue or
setting. Original scenarios can retain the recognizable visual concepts.

- [Riko](canonical-characters/riko.md) is an adult human character in contemporary
  streetwear. Selected source files and portable provenance were adopted across
  the product ownership boundary into FX-owned storage.
- Nami reuses the existing FX-owned canonical illustration and recorded lineage.
  The [shared-character rules](../../www/docs/showcases.md#shared-character-across-examples)
  distinguish an illustration from its registered frame, portrait and 3D form.
- [Sena](canonical-characters/sena.md) is selected for local development. Her
  upstream style-reference rights remain unresolved for public delivery; raw
  artwork and derivatives stay outside the public package until resolved.

A matching name does not make crops, masks or rig settings interchangeable.
Bind every derived representation to exact source digests and review it for its
intended use. Generate turnarounds, neutral views or rig-ready forms only when a
workflow consumes them. Multi-character scenes additionally need checks for
identity swaps, merged features and occlusion.

Examples and website inputs must resolve from FX-owned assets or their recorded
upstream media source. They must not require a sibling repository, cross-product
symlink or private local path. FX ownership does not require putting generated
artwork into Git.

## Evaluation and delivery

Plan offline first and exercise validation, paid-call refusal and local paths
with synthesized media and provider stand-ins. Compare production hypotheses
using the same input and rubric, changing a named variable at a time. A different
creative take is a new recorded run; the engine remains the only retry owner for
paid calls.

Record tested inputs, accepted and rejected takes, selection reasons, cost and
manual interventions. Count failed calls and independent review in the same
spending accounting. Distinguish a cache replay from fresh generation. Narrow a
workflow when its quality bar cannot be met instead of silently changing the
promise or treating a technically valid file as accepted art.

A canonical handoff includes:

- A standalone example with its inputs, prompts, dependencies and paid/free stages.
- An accepted result with exact lineage and accurately scoped independent review.
- Provider-free checks and a replayable local finishing path.
- Pinned public input downloads, portable workflow recordings and reviewed media.
- Website source, a reproducible static build and browser inspection of that build.

Keep raw runs, secrets and operational execution notes outside Git. Inspect
exported prompts, filenames, captions, metadata and visible text before delivery.
Publish only the reviewed immutable media closure. Opening a PR, updating media
pins and deploying a website are distinct operations; each receipt must describe
what actually happened.
