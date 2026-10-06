# Example: hello

**The ask:** "Draw a badge for each entry in my list, make sure each has a clear background,
scale them down, and give me one sheet with all of them and a folder I can ship."

Nothing in it is paid: it runs offline, at $0, with no keys. It needs only a Python with `grida`
and Pillow for its nodes ([From a clone](../README.md#from-a-clone)).

```
hello/
  fx.yaml
  workflows/hello.yaml       # the whole flow
  nodes/badges.py            # two nodes you write: draw one badge, lay badges out on a sheet
  inputs/badges.yaml         # three badges
```

## Run it

From this folder:

```bash
grida-fx plan hello --inputs inputs/badges.yaml
grida-fx run  hello --inputs inputs/badges.yaml
```

```
hello  ·  1 phase
phase 1   11 steps   0 provider calls   $0.00
cached    0 of 3 known steps
estimate  $0.00
run       runs/hello/<date>-1
result    ok   spent $0.00
```

- **11 steps:** for each of the three badges, `draw`, `clear` and `small`; then `sheet` and
  `pack`. Only the three `draw`s are known before the run: everything else reads what they make.
- **The result** is in the run folder's `outputs/`: `sheet.png`, `badges/<id>.png`, the folder
  `pack` laid out (`files/badges/<id>.png` and `files/sheet.png`) and its `manifest.json`.

Run it again and every step comes from the cache. Change one badge's colour in
`inputs/badges.yaml` and only that badge is drawn, checked and scaled again, then the sheet and
the folder.

## What this example shows

- **Your own nodes.** `nodes/badges.py` draws with Pillow. A node declares its params, inputs and
  outputs; FX hashes its source, so editing it re-runs exactly the steps that use it.
- **A keyed repeat.** `badge` runs once per list entry, keyed by `id`, and
  `steps.badge.*.small.outputs.image` collects the results by key.
- **A judge.** `fx/image.check_alpha@1` judges `draw`: a badge without a clear background fails,
  and `small` waits for the verdict ([Annotations and judges](../../docs/guide/06-annotations-and-judges.md)).
- **Built-in local steps.** `fx/image.resize@1` scales each badge; `fx/package@1` lays the
  results out by destination path, beside a manifest.
- **The cache.** Every step's result is kept by what went into it, so a second run is free and
  instant, and a change re-runs only what it touches
  ([Cost, cache and takes](../../docs/guide/04-cost-and-cache.md)).
