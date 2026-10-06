# Example: looping parallax

**The ask:** "I have background layers. Make each one repeat horizontally. Mirror it by default.
For layers I mark `repaint`, have a model repaint the seam instead, and fall back to mirroring if
the repaint isn't seamless. Then give me a manifest and a preview frame."

```
looping-parallax/
  fx.yaml                 # routes image.edit to gpt-image-2.5-sunburst@openai: masks and alpha
  workflows/looping-parallax.yaml
  nodes/seams.py          # loops_already (judge), layer_repaint (paid), seam_check (judge)
  nodes/compose.py        # the manifest, and a preview frame at scroll 0
  prompts/seam.md
  inputs/harbor.yaml      # two layers; near_masts is marked repaint
  inputs/mirror-only.yaml # the same two layers, both mirrored: nothing is paid
  art/*.png               # the pictures: ../draw_placeholders.py
```

## Run it

From this folder. Offline, at $0, with no keys:

```bash
grida-fx run looping-parallax --inputs inputs/mirror-only.yaml
```

Both layers are mirrored: each comes out twice as wide, its right edge meeting its left. The run
folder's `outputs/` holds the layers, `manifest.json` and `preview.png`.

With a repaint, which needs `OPENAI_API_KEY`:

```bash
grida-fx plan looping-parallax --inputs inputs/harbor.yaml
grida-fx run  looping-parallax --inputs inputs/harbor.yaml --live --max-usd 2
```

```
looping-parallax  ·  1 phase
phase 1   11 steps   0–2 provider calls   $0.00 – $1.70
cached    0 of 6 known steps
estimate  $0.00 – $1.70   ceiling $2.00
```

That is the plan in a fresh folder. After the mirror-only run, `far_cliffs`' three steps and
`near_masts`' `loops_already` are cached already, and it says `cached    4 of 6 known steps`.

- **11 steps:** `far_cliffs` mirrors, so it has `loops_already`, `mirror` and `chosen`.
  `near_masts` is marked `repaint`: the same three, plus two takes each of `repaint` and
  `seam_ok`. Then `compose`.
- **0 – 2 calls:** every paid step is a *maybe*: `near_masts` is repainted only if it doesn't loop
  already, and a second take only if the first seam is rejected. At worst that is two
  `image.edit` calls, $1.70. The repaint names no size, so each is priced at the built-in
  route's whole range, up to $0.85. If every layer mirrors, nothing is paid at all.
- **Without `--live`** the run mirrors `far_cliffs` and refuses the repaint at $0
  (`image.edit on gpt-image-2.5-sunburst@openai is a paid call; run with --live`).

## How the repaint works

`layer_repaint` turns the layer by half its width, so its seam lies in the middle, and sends it
with a mask whose transparent band (an eighth of the width, centred on the seam) is the only part
the model may change. The model answers at a size of its own: the node scales the answer back,
keeps only the band from it, and turns the layer back. Everything outside the band stays the
layer's own pixels. `seam_check` then judges the three joins the repaint makes: the seam itself
(the right edge against the left, within 0.02), and the two places where the painted band meets
the layer's own pixels, which may be no more abrupt than the layer already was there. It also
confirms that no pixel outside the band changed.

## What this example tests

- **Plan-time facts and assertions.** Width and opacity are read from the files while planning,
  so a narrow layer is refused before anything runs.
- **Conditions on run-time verdicts.** `repaint` and `mirror` are *maybe* steps. The plan prices
  the repaint as worst case and decides both when the verdicts exist.
- **Fallback without magic.** A judge with `regenerate … then: continue`, a conditional `mirror`,
  and an explicit `select`.
- **Paid work inside a user's node.** `layer_repaint` calls `ctx.image_edit`. Editing the seam math
  re-runs the node, but an identical edit request is answered from the cache.

## Friction found while writing it

- **The `if:` on `mirror` is the hardest line in the file.** It restates the logic of the steps
  above. A `fallback:` shorthand on the judge (`on_reject: { regenerate: …, then: { use: mirror } }`)
  would read better, but it hides an edge. It is still an open choice.
- **`select` needs to know "accepted".** `first_of` skips outputs whose step was skipped or
  rejected-with-continue. That rule has to be stated precisely.
