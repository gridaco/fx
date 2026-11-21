# Waiting-step dogfood

Copy this directory to a fresh project folder with the Grida Python package
installed. Both workflows draw three PNG swatches locally and run with a
zero-dollar ceiling.

## Supply a revision and review an image

`workflow.yaml` opens two independent requests. A build system can supply the
revision through the SDK, while a person chooses one of the generated PNGs.
Start a named run in one terminal:

```sh
grida fx start --background
grida fx run workflow.yaml --name review --wait-for-answers --max-usd 0 --open
```

From the same project in a second terminal, run this once the revision card appears:

```python
from grida import fx

run = "review-variants/review"
request = next(r for r in fx.list_requests(run) if r["step"] == "revision#1")
fx.respond(run, request["id"], "rev-42")
```

In the viewer, open **Review** on `choose`, select a swatch and submit. Either
request can be answered first. The run's `outputs/image.png` is the chosen
image. Its `outputs/manifest.json` records the supplied revision and that
image's digest. The revision declares only a string schema. The image request
adds a message and a map from keys to the frozen image files.

## Try the form showcase

```sh
grida fx start --background
grida fx run showcase.yaml --name dogfood --wait-for-answers --max-usd 0 --open
```

`showcase.yaml` has several independently answerable steps in one run.

| Card | What to try |
| --- | --- |
| Choose the cover | Pick one of three PNGs. The next review opens with the exact swatch you picked. |
| Approve public use | Choose **No** to record a decision and skip marking the sample public. |
| Annotate the cover | Enter a review note, add two requested changes, and set the assigned designer to **No value**. This review opens after the cover is chosen. |
| Set label length | Click the front `placement` card to expand the stack. Answer Print and Web independently. They use the same options and keep separate request IDs and drafts. |
| Set the print settings | Pick **satin**, enter **12** copies, a scale of **0.95**, and a top margin of **3.5** mm. Check **No value** under Color profile override to send an explicit null. Click **Include Production labels**, then **Add field**, and enter **batch** with the value **proof**. |
| Choose the thumbnail | Pick a swatch or let the deadline expire. The local fallback selects C and the thumbnail still completes. |
| Verify the print proof | Enter a code such as `FX-042`, try an invalid code, or leave it unanswered. On expiry, certification is skipped and this branch fails. Other open requests remain available. |

## Answer types in the viewer

| Workflow schema or setting | Editor |
| --- | --- |
| `enum: [matte, satin]` | A picker for literal values. The paper finish is a string in the submitted answer. |
| `choices` | A candidate picker with previews of recorded files. A key identifies the original value exposed as `outputs.selected`. Use `choice_field` to put the key in an object answer. |
| `type: boolean` | **Yes** and **No** radio buttons. |
| `type: string`, `integer`, or `number` | Text, whole-number, or decimal input. FX validates constraints such as `pattern`, `enum`, and numeric bounds. |
| `type: "null"` | **No value** checkbox. For an optional field, leaving it unset omits the field. |
| `type: [string, "null"]` | A type selector with **Text** and **No value**. |
| `type: array` | Add or remove items, with an editor for each item. |
| `type: object` | Named and nested fields, including custom field names where the schema permits them. |

Click **Review** on one card, edit a draft, close it, and open a different
card. Reopen the first card to check that its draft is still there. Expand
the repeated cards to review each item separately. Use **View answer** or
**View outcome** to reopen a completed or expired review.

Each review also has its own ID when inspected from a second terminal. Submit
to an exact request while the viewer is open to see the result appear on its
card.

```sh
grida fx inspect review-showcase/dogfood --requests --json
```

To see the timeout states sooner, pass an inputs file with
`continue_seconds: 10` and `fail_seconds: 30`. Keep the original command's
inputs file when resuming that run.

For a detached run, start with a fresh name and omit `--wait-for-answers`.
The invocation returns incomplete when only requests remain. The viewer or
CLI can still record an answer. Resume with the same input options when ready
to let the workflow consume those answers or run any timeout fallback.
