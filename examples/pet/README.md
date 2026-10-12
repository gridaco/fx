# Example: pet

**The ask:** "Make me a little animated pet that idles, runs, waves, jumps, sulks, waits, works
and looks things over, and that I can drop into the Codex app."

Every picture is drawn by [your own Codex](../../docs/guide/12-codex.md), signed in with your
ChatGPT account: no API key, and $0 to FX. Codex first draws one picture of the pet, from your
words or from a picture you give it. Each state is then one strip of poses drawn from that
picture, so every state is the same pet. FX counts the poses, cuts them into equal cells and lays
the rows out as one sprite sheet, with a preview of each state. With the default states, the sheet and its `pet.json` are a pet in the layout of the
Codex app's own pets: nine states of up to eight 192 x 208 frames.

```
pet/
  fx.yaml                  # every picture on imagegen@codex
  workflows/pet.yaml       # the whole flow, and Codex's nine states as a table
  workflows/reel.yaml      # a finished pet acting out its states, as a video that loops
  prompts/base.md          # the one picture every state is drawn from
  prompts/row.md           # one strip of poses
  nodes/sprite.py          # find and count poses, cut a row, lay out the sheet, play a preview
  nodes/reel.py            # the reel: the sheet played on a backdrop, encoded with FFmpeg
  inputs/pebble.yaml       # one pet
  inputs/coral.yaml        # the website's octopus, from a picture
  inputs/coral-reel.yaml   # its recorded sheet, for the reel
  showcase-inputs.json     # where to download the website's picture and sheet
  stand_in.py              # answers the drawing offline, for trying it without Codex
  pyproject.toml           # the Python SDK, which the nodes use (with Pillow)
```

## Before you start

1. Install Codex and sign in with ChatGPT, then check that FX finds it
   ([Images from your own Codex](../../docs/guide/12-codex.md#set-up)):

   ```bash
   grida fx doctor pet
   ```

   ```text
   uses      image.edit imagegen@codex servable (1 step)
   uses      image.generate imagegen@codex servable (2 steps)
   ```

   It exits 1 when one of them can't be served.

2. Install the Python SDK for the nodes, from this folder:

   ```bash
   uv sync
   ```

   (`pip install grida` into the Python FX uses works too.)

## Try it without Codex

A stand-in answers each drawing with plain shapes; everything else runs for real, so you get a
sheet, a `pet.json` and the previews in seconds, for nothing:

```bash
grida fx run pet --inputs inputs/pebble.yaml --stand-in stand_in.py#answer
```

## Run it

```bash
grida fx plan pet --inputs inputs/pebble.yaml
```

```
pet  ·  1 phase
phase 1   69 steps   9–25 provider calls   $0.00
cached    0 of 1 known steps
estimate  $0.00
```

- **1 picture, then 8 strips:** `running-left` is `running-right` with each frame flipped in its
  own cell. The strips are drawn from the first picture, so the plan knows them only once it is
  made: that is the one known step.
- **Up to 25 calls:** a strip whose poses can't be counted, or that cuts one off at its edge, is
  drawn again, three takes at most. The 69 steps count every possible take.
- **$0.00:** FX books Codex at $0, so the run needs no `--max-usd`. Each strip still counts
  against your ChatGPT plan's usage.

Then draw it, straight into Codex's pets folder:

```bash
grida fx run pet --inputs inputs/pebble.yaml --live \
  --deliver pet=$HOME/.codex/pets/pebble/pet.json \
  --deliver spritesheet=$HOME/.codex/pets/pebble/spritesheet.webp
```

Codex draws the pet's picture first, then three strips at a time, about 40 seconds each, so the
run takes three to four minutes.
The run folder's `outputs/` has the sheet, `pet.json`, `sprite.json` (the rows, their frame counts
and timings) and `previews/<state>.gif`; `outputs/files/` is the same as one folder.

Look at `outputs/base.png` and the previews. If a state is wrong, ask for another take of that
strip alone, then run again: the other strips come from the cache, so Codex draws one picture.

```bash
grida fx reroll runs/pet/<run> "state['waving'].draw"
grida fx run pet --inputs inputs/pebble.yaml --live \
  --deliver pet=$HOME/.codex/pets/pebble/pet.json \
  --deliver spritesheet=$HOME/.codex/pets/pebble/spritesheet.webp
```

The choice is kept in `workflows/pet.takes.yaml`. Changing one state's `action` redraws that
state alone too.

## Your own pet

Copy `inputs/pebble.yaml` and change it:

| Input | |
|---|---|
| `id` | the pet's folder name: `~/.codex/pets/<id>` |
| `name`, `description` | shown with the pet |
| `look` | the pet in words: shape, colours, face, markings |
| `reference` | optional: a picture of the pet to start from (a drawing, a photo of a toy, a character), PNG, JPEG or WebP |
| `style` | `sticker`, `pixel`, `plush`, `clay`, `vector`, `toy` or `painterly` |

With a `reference`, Codex redraws it in the style as the pet's first picture, and every state
follows that. Write `look` about the same pet: where the words and the picture disagree, Codex
follows the words. Give it on the command line:

```bash
grida fx run pet --inputs inputs/pebble.yaml --reference ./my-pet.png --live
```

A simple, distinctive design (one shape, a few colours, one marking) comes back the most alike
from strip to strip.

## Other states and frame counts

Leave `states` out for Codex's nine (`tables.codex_states` in the workflow). Give your own list
for any sprite: up to 16 states of up to 8 frames, each with an `action` to draw, optional
`durations` in milliseconds, or `mirror_of` another state. `cell_width` and `cell_height` set the
cell. The sheet has a column per frame of the longest state, and `sprite.json` says where
everything is. `pet.json` is written only for Codex's own layout, which is the one the app loads.

```yaml
states:
  - { id: idle, frames: 4, durations: [200, 120, 120, 200], action: "breathing calmly in place" }
  - { id: walk, frames: 6, action: "walking to the right, facing right" }
  - { id: walk-left, frames: 6, mirror_of: walk }
```

## How it works

1. **`base`:** the pet's one picture. Without a reference it is `sketch`, an
   `fx/image.generate@1` from `look`; with one it is `redraw`, an `fx/image.edit@1` of it. Each has
   an `if:` on `inputs.reference`, and `fx/select@1` takes whichever ran.
2. **`state.draw`:** one strip per state, `fx/image.generate@1` on `imagegen@codex` with the base
   picture as its one reference, a transparent background and no size: Codex picks the shape
   (strips come back about 3:1), and the prompt asks for the same pet as input image 1, posed in
   one row with clear space between the poses.
3. **`state.poses`**, a judge you write: rows of the strip with pet pixels make a band, gaps of
   empty columns part the poses, and specks (a stray pixel, a tear apart from the face) join the
   nearest one. The strip is accepted with exactly the asked number of poses, none cut by its
   edge, and each pose is marked on it for the viewer.
4. **`state.row`:** the poses go into equal cells at one scale for the whole row, so the pet
   neither grows nor shrinks between frames. They keep their height above a shared ground line, so
   a jump stays a jump, and each is centred on its feet rather than its outline, so a raised arm
   doesn't shift the body.
5. **`sheet`** stacks the rows in the order of the states, flips mirrored ones frame by frame, and
   writes a lossless WebP; **`preview`** plays each row at its own timing; **`pack`** lays the
   pet out as one folder.

## A reel of your pet

After a run, play the pet's states as a short video that loops, with a poster frame:

```bash
grida fx run reel --spritesheet runs/pet/<run>/outputs/spritesheet.webp \
  --layout runs/pet/<run>/outputs/layout.json
```

It needs FFmpeg on PATH, draws nothing and costs nothing. With Codex's nine states the pet
idles, waves, runs right, looks things over, runs left, waits and runs back to where it began.
The jump is left out: its row is drawn smaller, so the whole jump fits a cell. Other states each
play in place. `outputs/` has `video.mp4` (960 x 600, 30 fps) and `poster.webp`.

## The website's octopus

The [FX website](https://grida.co/fx/workflows/pet) shows Coral, drawn from
`inputs/coral.yaml`. Its reference picture, itself generated from a text prompt, and the recorded
sheet are public downloads:

```bash
python ../fetch_showcase_inputs.py .
grida fx run reel --inputs inputs/coral-reel.yaml           # the website's cover, at $0
grida fx run pet --inputs inputs/coral.yaml --live          # a new take of Coral
```

A new take draws new strips, so it differs from the recorded sheet.

## Next to Codex's own pet skill

Codex can make pets itself, with its `hatch-pet` skill. This example follows the same sheet and
the same idea, one picture first and every state drawn from it, but leaves two things out:

- **Looking around.** Version 2 of the sheet adds two rows of 16 look directions; this example
  makes version 1.
- **Reviewing by eye.** The skill has Codex review the drawings. Here the checks are counting and
  clipping; the rest is your look at the previews.
