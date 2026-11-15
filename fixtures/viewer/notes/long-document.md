# A longer workflow guide

Most notes should be readable directly on the canvas. This deliberately longer
document shows how a large source file keeps a bounded card without losing the
full text available in the details panel.

## Prepare a useful reference

Start with a simple silhouette. The reference in this fixture is drawn from
ordinary geometric shapes by a local Python node. It is original test media,
requires no account or provider key, and costs nothing to generate.

Keep the important shape inside the image bounds. Extra canvas space makes it
easier to see whether a later operation changes the content or merely changes
its framing. In a practical workflow, this is a good place to explain what a
reference image is expected to contain.

## Follow the two variations

The repeated group receives the same reference for each item. One item uses a
warm palette and the other uses a cool palette. Each note belongs to the group
that declares it; it does not introduce another executable step or dependency.

Select a card to inspect its recorded inputs and outputs. The viewer is showing
a saved run, so inspecting an image or a note does not execute the workflow.
The same record can be shared without access to the original source directory.

## Read an imported workflow

The imported workflow has its own short note. Open that workflow to inspect its
local steps and documentation, then return to the parent canvas. The parent
does not pretend that the imported note belongs to one of its own steps.

This ownership rule matters when a workflow is reused in several places. Each
recorded occurrence keeps the right scope while the documentation remains
ordinary authored text.

## Keep examples literal

Code fences in a note are documentation, including expressions such as:

```yaml
with:
  image: ${{ steps.seed.outputs.image }}
```

The note renderer never evaluates that expression. The example explains a
connection while the actual workflow declaration remains responsible for it.
Plain-text notes preserve punctuation and whitespace without interpreting
Markdown formatting.

## Share a recorded explanation

A new plan captures the current note text. Resuming a saved run keeps the
original captured documentation, even when someone has edited the source file.
The host can still offer an action that opens the current editable file.

Portable exports can include or withhold authored values. Notes follow that
choice, and the export removes local source provenance. A missing source file
produces a visible diagnostic instead of turning documentation into a failed
execution step.

## Finish the inspection

Long-document ending retained in full: the card is only a preview of this guide,
and the details panel retains this final paragraph.
