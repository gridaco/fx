# Example: looping parallax

**The ask:** "I have background layers. Make each one repeat horizontally. Mirror it by default.
For layers I mark `repaint`, have a model repaint the seam instead, and fall back to mirroring if
the repaint isn't seamless. Then give me a manifest and a preview frame."

```
looping-parallax/
  fx.yaml
  workflows/looping-parallax.yaml
  nodes/seams.py          # loops_already (judge), layer_repaint (paid), seam_check (judge);
                          #   the seam math is elided
  nodes/compose.py        # manifest + preview frame (the drawing is elided)
  prompts/seam.md
  inputs/harbor.yaml + art/*.png     # the pictures: ../draw_placeholders.py
```

From this folder (`fx.yaml` lists [`../routes.yaml`](../routes.yaml) under `route_tables`):

```bash
grida-fx plan looping-parallax --inputs inputs/harbor.yaml
grida-fx run  looping-parallax --inputs inputs/harbor.yaml --live --max-usd 2
```

The node bodies are pseudo-code: they call helpers the example does not define
(`wrap_seam_error`, `shift_seam_to_centre`, `draw_preview`, …). The workflow plans as shown;
fill those in before a run, or its local steps fail with a `NameError`.

```
looping-parallax  ·  1 phase
phase 1   11 steps   0–2 provider calls   $0.00 – $0.60
cached    0 of 6 known steps
estimate  $0.00 – $0.60   ceiling $2.00
```

- **11 steps:** `far_cliffs` mirrors, so it has `loops_already`, `mirror` and `chosen`.
  `near_masts` is marked `repaint`: the same three, plus two takes each of `repaint` and
  `seam_ok`. Then `compose`.
- **0 – 2 calls:** every paid step is a *maybe*: `near_masts` is repainted only if it doesn't loop
  already, and a second take only if the first seam is rejected. At worst that is two
  `image.edit` calls at $0.30 ([`../routes.yaml`](../routes.yaml)): $0.60. If every layer
  mirrors, nothing is paid at all.

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
