# Example: rigged character (deliberately hard)

**The ask:** "From a written brief, give me one rigged character at game scale. Draw references
first, build the mesh from them, stand it upright, rig it, and add test clips. Have independent
reviewers check every stage on renders. Admit the export only when the numbers and the pictures
both pass. If the rig fails its audit, rebuild the body and try again, twice at most. Never spend
more than I allow, and if my laptop dies mid-run, don't pay again for what was done."

**Status:** the project plans and prices today, but does not run. Its reviews use
`vision.review`, which no provider adapter serves yet, and its Blender steps use tool scripts
(`ctx.tool(...).script`) and version constraints (`"blender>=5.2"`), which are planned. So its
node bodies in `nodes/` and its scripts in `blender/` are placeholders: they show the shape of
each node, not working code.

```
rigged-character/
  fx.yaml                             # the routes, and the $30 ceiling
  routes.yaml                         # an illustrative vision.review route, so the reviews plan
  workflows/rigged-character.yaml     # the whole flow, including the recovery loop
  nodes/agents.py                     # three agents (references, orientation, agent rigging)
  nodes/blender.py                    # normalize, turntable, audit_rig, export_game_glb (+ blender/*.py scripts)
  prompts/*.md
  inputs/wren.yaml, inputs/wren.md    # a brief, and the choices for this character
```

Every other capability uses a route of FX's built-in table: images on
`gpt-image-2.5-sunburst@openai`, agent turns on `openai/gpt-5.6-sol@openrouter`, and the mesh and
the rig on Tripo. From this folder:

```bash
grida-fx doctor rigged-character   # Blender 5.2 found; OPENAI_API_KEY, OPENROUTER_API_KEY and TRIPO_API_KEY set
grida-fx plan   rigged-character --brief inputs/wren.md --partition head_body
```

```
rigged-character  ·  1 phase
phase 1   116 steps   53–367 provider calls   $3.06 – $85.23
cached    0 of 3 known steps
estimate  $3.06 – $85.23   ceiling $30.00   ⚠ the worst case exceeds the ceiling; the run stops before crossing it
```

Where the worst case comes from, with each route's allowance (an agent turn $0.10, an image
$0.70, an edit $0.85, a review $0.03, a mesh $2.50, a rig $0.50). The reference agent asks for
`1024x1536` inside its tool, where the plan cannot see it, so the plan prices its pictures at the
image routes' whole range; a live run holds that size's tier, $0.24 an image and $0.38 an edit.

```
references   3 takes × (12 agent turns + image + 2 edits + review)                  ≤ $10.89
body         3 builds, each:                                                        ≤ $74.31
               part[head], part[body]  2 × 3 takes × (mesh + review)       $15.18
               assemble                3 takes × (30 agent turns + review)  $9.09
               rig_provider            1 job (mesh.rig)                     $0.50
               audit                   local                                $0.00
admit        1 review                                                                ≤ $0.03
worst case                                                                            $85.23
```

The low end, $3.06, is what surely runs: the first take of everything, at the lowest prices
(16 + 36 + 1 = 53 calls). Regeneration accounts for the rest, up to 367 calls. The ceiling stops
a run before it crosses $30, whatever the regeneration does.

```bash
grida-fx run rigged-character --brief inputs/wren.md --partition head_body --live --max-usd 30
# ... laptop dies during body.assemble take 2 ...
grida-fx run rigged-character --brief inputs/wren.md --partition head_body --live --max-usd 30
# references, both parts: cached. assemble take 2: the agent's paid turns are replayed from the
# record, and the agent continues from its last tool result. The rig job, if it was submitted, is
# collected, never resubmitted.
```

## What this example tests

- **Agents as ordinary nodes.** Tools are plain Python on the user's machine. Each model turn is a
  priced, budgeted, recorded and replayable call.
- **External tools** (Blender) declared on node types, checked by `doctor` and by every plan.
- **Nested regeneration:** part takes and assembly takes inside body rebuilds, priced worst case
  at plan time and enforced at run time by the ceiling.
- **A long provider job** (`mesh.rig`) that resumes without paying twice.
- **The recovery loop:** "if the rig audit fails, rebuild the body" is one `regenerate` on a group,
  not graph splicing in class inheritance.
- **No Python builder needed.** The partition table and the rigging switch are `lookup` and `if:`.

## What the user did not write, compared with a hand-built pipeline

- **Runner classes, a mode table and graph splicing.** That's the workflow file.
- **Stage recovery with hash-chained checkpoints, budget pools, tool journals, run locks.** That's
  the runner, the takes, the capability record and the ceiling.
- **A Blender probe node.** That's `tools=` plus `grida-fx doctor`.
- **Separate submit and collect nodes for rigging.** That's one `mesh.rig` job.
- **Admit and select nodes for each review round.** Those are judges with
  `regenerate`, and `select`.
