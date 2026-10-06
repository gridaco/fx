# Annotations and judges

Two different things, often used together:

- **An annotation** is a mark a model, or your code, draws on an image: a point, a set of points,
  or a box, with an optional comment. Annotations are an **artifact**: a file of marks, kept,
  cached, shown and usable downstream like any other result. They say nothing about whether
  anything is good.
- **A verdict** is a decision, `accept` or `reject`. It is a **fact** reported by a judge.

What the marks *mean* is up to you and your prompt: problems to fix, things to keep, where
something is, which region is which.

**What runs today:** the annotations artifact, `annotations` ports, and the marks your own nodes
write with `ctx.annotate` ([below](#annotations-you-write)). The model-driven types,
`vision.annotate@1`, `vision.review@1` and `structured.review@1`, are **planned**: they plan and
price, and run once their bodies land. Until then, a model's judgement is a
`structured.generate` answer that a judge you write reads, as in [Nodes](03-nodes.md#judges-you-write).

## The annotations artifact

An `annotations` output is a list of marks, possibly empty:

```json
{
  "kind": "fx-annotations-v1",
  "annotations": [
    { "shape": "point",  "at": [0.47, 0.61], "label": "a sixth finger", "color": "#e4572e" },
    { "shape": "points", "points": [[0.38,0.05],[0.62,0.05],[0.64,0.24],[0.36,0.24]], "closed": true,
      "label": "this area is blurred" },
    { "shape": "box",    "box": [0.41, 0.70, 0.59, 0.81], "label": "text on the sign" },
    { "label": "the whole image is too dark" }
  ]
}
```

| Field | |
|---|---|
| `shape` | `point` (`at`), `points` (`points`; `closed: true` for a region, `false` for a line), or `box` (x0, y0, x1, y1). Omit it for a note about the whole image. |
| `label` | optional comment |
| `color` | optional. When absent, a viewer picks one, the same for the same `tag`. |
| `tag` | optional short name to group marks (`issue`, `keep`, `face` ...); entirely yours |
| anything else | optional extra fields you ask for (below) |

Coordinates are fractions (0–1) of the image at full size. When several images are annotated
together, each mark also has `image: <index>`.

## Annotating: `fx/vision.annotate@1` (planned)

```yaml
  marks:
    uses: fx/vision.annotate@1
    with:
      image: ${{ steps.draw.outputs.image }}
      prompt: "Mark every place that looks wrong, with a short comment for each."
```

- **The prompt decides.** Ask for colours, tags, one mark per problem, boxes only: whatever
  your use needs.
- **Optional settings:**
  - `shapes: [box]` limits the shapes allowed;
  - `fields: { severity: [minor, major] }` asks for extra fields on every mark, validated like any
    structured output.
- **How the model points is the node's job.** By default it overlays a labelled grid and has the
  model name cells, which works with any vision model. Routes that return coordinates natively can
  use `grounding: native`.

Downstream:
- `fx/annotations.mask@1` (planned) turns marks into a mask for `image.edit`: a PNG the size of
  the image whose marked areas are fully transparent (alpha 0 marks what may change), the
  convention every shipped `image.edit` route follows;
- `fx/image.crop@1` takes `region:` from a mark;
- your own nodes read them with `ctx.read.annotations(name)`.

```yaml
  face:
    uses: fx/vision.annotate@1
    with: { image: "${{ inputs.sprite }}", prompt: "Draw one box around the face.", shapes: [box] }
  face_crop:
    uses: fx/image.crop@1
    with: { image: "${{ inputs.sprite }}", region: "${{ steps.face.outputs.annotations }}", padding: 0.25 }
```

## Judges: annotate, then decide (planned)

`fx/vision.review@1` annotates the image and then decides:

```yaml
  review:
    uses: fx/vision.review@1
    judges: draw
    with:
      image: ${{ steps.draw.outputs.image }}
      criteria:                                   # optional: answered one by one
        - "It shows exactly ${{ item.name }}, as described."
        - "There is no text, logo, watermark or frame."
        - "Hands, faces and limbs are anatomically plausible."
```

**What a review produces:**
- **`outputs.annotations`:** what it marked while looking. When a criterion fails, its marks say
  where.
- **`outputs.mask`:** the marked areas as a mask for repair, in `image.edit`'s convention (marked
  areas fully transparent, so they are what may change).
- **`facts.verdict`,** plus `facts.criteria` (pass or fail per criterion) when you gave criteria.

**Why annotate before deciding:** a judge that has to show where a problem is raises fewer vague
false alarms. A judge that answers each criterion separately waves fewer problems through on one
overall impression. Both are just how the review is prompted; you can ask your own question
instead (`question:`, a template like any prompt).

**Feeding the result forward:**

```yaml
    on_reject:
      regenerate: { max: 3, feedback: true }      # the next take is told what was marked, and why
```

```yaml
  fix:
    if: ${{ steps.review.facts.verdict == 'reject' }}
    uses: fx/image.edit@1
    with:
      image:  ${{ steps.draw.outputs.image }}
      mask:   ${{ steps.review.outputs.mask }}
      prompt: "Fix only the marked areas."
```

**Plain verdicts:** `report: verdict` skips the annotations and gives the decision with a one-line
reason. It is the cheapest call, for a first-pass filter over many takes.

## Annotations you write

Any node can output annotations (an output declared `annotations`), with
`ctx.annotate(shape=..., label=..., color=..., tag=..., **fields)`. A judge you write may attach the
marks that justify its verdict. They work downstream exactly like a built-in judge's.
