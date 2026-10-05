# The workflow file

A workflow file declares inputs, steps and outputs. The syntax will feel familiar if you have
written GitHub Actions. The meaning is different, and that matters:

- **A step is a pure function of what it declares.** No step sees a shared folder, the
  environment, or another step's files unless they are wired to it.
- **Results are kept by content.** FX runs a step only when something it declares changes. See
  [Cost, cache and takes](04-cost-and-cache.md).
- **Order comes from wiring.** Steps run as soon as their inputs exist, in parallel where possible.
  Scheduling is per step, even across the instances of a repeat.

## Shape

```yaml
fx: workflow/v1
id: concept-gallery              # stable id, used in runs/ and by `grida-fx run concept-gallery`
title: Concept gallery
description: A reviewed storyworld and one concept image per entity.

inputs:  { ... }                 # what a caller supplies
tables:  { ... }                 # constant lookup tables (optional)
let:     { ... }                 # named expressions (optional)
budget:  { max_usd: 12 }         # this workflow's default ceiling (optional)
assert:  [ ... ]                 # workflow-level checks (optional)
steps:   { ... }                 # the work
outputs: { ... }                 # what the workflow promises
```

FX reads a strict subset of YAML ([yaml.md](../../spec/yaml.md)), so a file has one meaning
everywhere. A plain value that some YAML reader takes for something other than a string is refused
with "quote it":
- `yes`, `no`, `on` and `off` in any letter case, and `True`, `FALSE` or `Null` (only the lower
  case words are booleans and null);
- numbers with a leading zero (`017`), `0x1F`, `0o17`, `0b101`, `1_000`, `16:9`, `.inf` and `.nan`;
- dates and timestamps (`2026-10-05`).

Write `"16:9"`. Duplicate keys, anchors, aliases and tags are refused too. `1024x1024`, `y` and `n`
are ordinary strings, and `1`, `1.0` and `1e0` are one number.

## Inputs

```yaml
inputs:
  synopsis:     { type: file, kind: text/markdown }
  poster:       { type: file, kind: image }
  max_entities: { type: integer, default: 24, minimum: 1, maximum: 48 }
  canvas:                                   # a nested object: just nest fields
    width:  { type: integer, minimum: 64 }
    height: { type: integer, minimum: 64 }
  layers:                                   # a list of objects: `items` is a field map
    type: list
    max_items: 32
    items:
      id:       { type: string, pattern: "^[a-z0-9_]+$" }
      file:     { type: file, kind: image }
      parallax: { type: number, minimum: 0, maximum: 1 }
  tags:     { type: list, items: { type: string }, unique_items: true }   # a list of plain values
  voices:   { type: map, values: { voice_id: { type: string }, style: { type: string } } }
  sprites:  { type: files, kind: image, glob: true }   # many files; keys are file stems
  face:     { $ref: ./portrait-motion.yaml#/inputs/face }   # reuse another workflow's input
```

**The shorthand:**
- **Types:** `string`, `integer`, `number`, `boolean`, `file` (with `kind`), `files`, `list`, `map`,
  and nested objects.
- **Keywords** are snake_case (`max_items`, `unique_items`, `min_length` ...).
- **It compiles to JSON Schema.** `grida-fx schema <workflow>` prints the result.

**One declaration feeds three things:**
- `--synopsis path.md` flags (the kebab-case form of each name);
- `--inputs inputs.yaml` (repeatable, merged in order; paths inside are relative to that file);
- validation before anything is planned.

**Defaults and content:**
- **An optional input with no default is `null`.**
- **Files are read by content.** The same bytes under two paths are the same input.
- **A few objects are reserved.** FX writes its own values in an identity as one-member objects:
  `file` with a 64-character digest, `missing: true`, `failed` with a string, `collection` with a
  list, or `pending`. A workflow, an inputs file, a table or a JSON file that holds one of these
  is refused ([identity.md §3](../../spec/identity.md#3-the-plain-projection)).
  `{"file": "a.png"}` is ordinary data.

## Steps

```yaml
steps:
  propose:
    uses: fx/structured.generate@1     # which node type
    with:                                  # its inputs and settings
      prompt: ./prompts/propose.md
      schema: ./schemas/world.json
      context: [ "${{ inputs.synopsis }}", "${{ steps.poster_small.outputs.image }}" ]
```

`uses:` names a node type:

| Form | Meaning |
|---|---|
| `fx/<name>@<major>` | built-in node type (see [Nodes](03-nodes.md)) |
| `./nodes/mirror.py#mirror_repeat` | a node type you wrote, in your project |
| `./workflows/icon.yaml` | another workflow, used as one step |

A path that starts with `./` or `../` is relative to the workflow's home: the folder of the nearest
`fx.yaml` above the workflow file, which is your project root. It means the same from whichever
workflow file it is written in, the way local actions are in GitHub Actions. That holds for
`uses:`, `prompt: ./prompts/...`, `schema: ./schemas/...` and a file given in `with:` alike. An
absolute path is refused.

Nothing is fetched from elsewhere while planning. A registry of workflows and nodes comes later,
and it copies: `grida-fx add <item>` will put a copy in your project, used through a `./` path like
your own files.

Every step field:

| Field | Meaning |
|---|---|
| `with:` | inputs and settings of the node type. The node type says which are files and which are settings. |
| `if:` | run this step only when the expression is true. See *Conditions*. |
| `needs:` | run after these steps without using their results (ordering only; rarely needed) |
| `for_each:`, `as:`, `key:`, `max:` | repeat this step, or a group, per item. See *Repeating*. |
| `matrix:` | repeat over every combination of several lists |
| `steps:` | a group: nested steps that repeat or regenerate together |
| `judges:`, `on_reject:` | this step judges another. See *Judges*. |
| `regenerate:` | redo a judged step, or a group, until accepted |
| `takes:`, `pick:` | draw several takes now, and choose which one downstream steps get. See [Cost](04-cost-and-cache.md#takes). |
| `assert:` | checks with your message. See *Assertions*. |
| `at: plan` | run this free local step while planning. See *Running a step while planning*. |
| `budget:` | a ceiling for this step, group or each instance, inside the run's ceiling; a step inside several is held to the innermost one only |
| `concurrency:` | at most this many instances of a repeated node step at once; on a repeated group or workflow step it does nothing yet |
| `route:` | which model serves this step |
| `requires:` | what the route must support (`image_input`, `mask`, `alpha` ...). The plan refuses others, offline. |
| `independent_of:` | refuse a plan where this step and those share an underlying model, whatever the provider |
| `timeout:` | wall-clock limit for this step, in seconds: past it the step fails (`ran past <n> seconds`) and is not run again |

## Expressions

`${{ ... }}` can appear in any value. Expressions are deliberately small: if you need more, write a
node.

| You can write | Example |
|---|---|
| references | `inputs.poster`, `steps.draw.outputs.image`, `steps.review.facts.verdict`, `item.id`, `let.ready` |
| one instance | `steps.entity['harbor_keeper'].draw` (a literal key, quoted) or `steps.cell[item.eye]` (an expression) |
| a collection | `steps.entity.*.draw.outputs.image` (see *Collections*) |
| a list element or a field | `inputs.face.eyes[0]`, `steps.propose.outputs.json.entities` |
| text | `"A ${{ item.kind }} named ${{ item.name }}"` |
| arithmetic and comparison | `facts(item.file).width * 2`, `item.parallax < 0.5`, `&&`, `\|\|`, `!` |
| a fallback for nothing | `steps.repaint.outputs.image ?? item.file` |
| file facts | `facts(inputs.poster).width`, `.height`, `.has_alpha`, `.opaque`, `.bytes`, `.kind`; a WAV's `.duration`; an MP4, WebM or Matroska video's `.width`, `.height`, `.fps`, `.duration`, `.frames`, `.has_alpha` ([facts.md](../../spec/facts.md)) |
| functions | `lookup(map, key)`, `min`, `max`, `len`, `contains`, `concat(a, b)`, `join(list, ", ")`, `stem(file)`, `digest(value)`, `accepted(collection)` |

**Rules:**
- **Typing:** a value that is exactly one expression keeps its type (a list stays a list). An
  expression inside other text makes a string.
- **No more than that:** there are no loops, user functions or string methods.
- **`facts()` on a step's output** works too. It is then a run-time value (see *Conditions*).
- **Two kinds of fact.** `facts(file)` gives *file facts*, which FX computes from the file's bytes
  ([identity.md §4](../../spec/identity.md#4-file-kinds-and-file-facts)). `steps.<name>.facts`
  gives *node facts*, the values a node reported about its own result (`ctx.fact`), such as a
  judge's `verdict`.

`let:` names an expression so you write it once:

```yaml
let:
  needs_repaint: ${{ item.repeat == 'repaint' && steps.loops_already.facts.verdict == 'reject' }}
```

## Repeating

```yaml
  entity:
    for_each: ${{ steps.propose.outputs.json.entities }}
    as: item
    key: ${{ item.id }}          # how instances are named: entity['harbor_keeper']
    max: 48                      # required when the list comes from a step: the cost ceiling
    steps:
      direct:
        uses: fx/structured.generate@1
        with: { prompt: ./prompts/direct-entity.md, schema: ./schemas/entity-direction.json, vars: { entity: "${{ item }}" } }
      draw:
        uses: fx/image.generate@1
        with: { prompt: "${{ steps.direct.outputs.json.prompt }}" }
```

- **Inside a group**, `steps.<name>` refers to the sibling in the same instance. A nested repeat
  sees the outer `as:` variable too.
- **A group without a repeat** (just nested `steps:`) is addressed `steps.references.draw`.
- **`concurrency: N`** on a repeated node step runs at most N of its instances at once. It does
  nothing yet on a repeated group, such as `entity` here, or a repeated workflow step. To pace a
  group's paid calls, limit their route in `fx.yaml`
  ([Cost](04-cost-and-cache.md#budgets-inside-a-run)).
- **`key:`** names instances, so takes and delivered files stay attached to the right item when
  the list changes.
  - **Without it,** the position is the name.
  - **Items with no natural id** can use a content key, like `key: ${{ digest(item.text) }}`, which
    survives inserting a line elsewhere.
- **`matrix:`** repeats over every combination:

  ```yaml
    combo:
      matrix: { eye: "${{ inputs.face.eyes }}", mouth: "${{ inputs.face.mouths }}" }
      uses: ./nodes/combine.py#combine
      with: { eye: "${{ matrix.eye }}", mouth: "${{ matrix.mouth }}" }
  ```

  Instances are `steps.combo['open']['smile']`. File templates can use `{key.eye}` and `{key.mouth}`.

### Collections

`steps.entity.*.draw.outputs.image` is a **collection**:

- **Order:** it is ordered like the `for_each` list (or the matrix), and every element knows its key.
- **Skipped instances are absent.** An instance rejected with `on_reject: continue` is present and
  carries its verdict. `accepted(...)` keeps only accepted elements.
- **In Python** a collection arrives as an ordered mapping, `{key: file}`, and each file has `.key`,
  so nothing depends on position.

### When the list comes from a step

When the list comes from a step, FX can't know its length while planning. Such a repeat starts a
new **phase**:
- The plan prices the run up to `max`.
- When the list exists, FX prices that phase exactly (a `phase_planned` event in the run's
  record) and continues. With `--yes-up-to`, it stops before a phase whose worst case would take
  the run past that amount; running again with a higher one continues. The ceiling holds either
  way: no paid call is made that could cross it.
- You don't declare phases; they follow from the wiring.

To avoid a phase, compute the list while planning (next section).

## Running a step while planning

```yaml
  parse:
    uses: ./nodes/script.py#parse_script
    at: plan
    with: { script: "${{ inputs.script }}", voices: "${{ inputs.voices }}" }
  line:
    for_each: ${{ steps.parse.outputs.json.lines }}      # known while planning: no phase, exact price
```

`at: plan` is allowed for a free, deterministic, local step: no paid calls, declared inputs only.
- **Its outputs are plan-time values,** so repeats over them are exact, `assert:` can check them,
  and the plan is priced exactly.
- **It is cached like any step,** so planning again doesn't redo it.

## Conditions

`if:` can depend on two kinds of value:

- **Things known while planning:** inputs, file facts, tables, `at: plan` outputs. The step is in
  or out of the plan.
- **Things known only while running:** a judge's verdict, a node fact a step reported. The step is
  *maybe*. The plan prices it as if it runs, and decides when the value exists.

To use whichever result exists, pick explicitly, or use `??`:

```yaml
  chosen:
    uses: fx/select@1
    with: { first_of: [ "${{ steps.repaint.outputs.image }}", "${{ steps.mirror.outputs.image }}" ] }
```

`select` skips results whose step was skipped or rejected. Its output keeps the candidates' name:
`steps.chosen.outputs.image` above.

## Judges

A judge is a step that looks at another step's result and reports a verdict, `accept` or `reject`.
It may produce outputs too; a vision review produces its marks
([Annotations and judges](06-annotations-and-judges.md)).

```yaml
  review:
    uses: fx/vision.review@1
    judges: draw
    with: { image: "${{ steps.draw.outputs.image }}", criteria: [ ... ] }
    independent_of: [direct]
    on_reject: continue
```

`vision.review` is planned: it plans and prices today, and runs once its body lands. Until then
a review is a `structured.generate` answer that a small judge of yours reads
([Nodes](03-nodes.md#judges-you-write)).

- **A judged step is finished only when its judges are.** Everything that reads `draw` waits for
  `review`, with no `needs:` required.
- **What a rejection means** is up to you:

| `on_reject:` | Meaning |
|---|---|
| `fail` (default) | the judged step fails, and everything that depends on it is skipped |
| `continue` | record the verdict; downstream still runs (show it, sort by it, filter it with `accepted()`) |
| `skip` | downstream of the judged step is skipped, but the run succeeds |
| `regenerate: { max: N, then: …, feedback: true }` | another take until accepted; **`max` counts all takes**, the first included. `then:` is `fail`, `continue`, `skip` or `keep_best: { by: <fact>, order: lowest\|highest }`. `feedback` hands the judges' marks on a rejected take to the next one; the first take is told nothing, so read it as `${{ feedback && feedback.<judge>.<fact> || '' }}`. |

**Regeneration redoes the judged step, or a whole group:**

```yaml
  build:
    steps:
      mesh:   { uses: fx/mesh.generate@1, with: { ... } }
      rig:    { uses: fx/mesh.rig@1,      with: { model: "${{ steps.mesh.outputs.model }}" } }
      audit:  { uses: ./nodes/audit.py#audit_rig, judges: rig }
    regenerate: { max: 3, until: "${{ steps.audit.facts.verdict == 'accept' }}" }   # 1 build + up to 2 rebuilds
```

From outside a group that regenerates, a reference means its last take: the accepted one, or the
one `then:` kept. The plan prices the worst case: every take of every regeneration.

Three kinds of failure are deliberately kept apart:

1. **A paid call that fails** is retried by the engine, its only retry owner: never by you, the
   node type or the provider adapter. A request is simply sent again only when it provably never
   reached the provider; every other repeat is a new attempt, and a call gets at most 6 attempts
   in all. Each attempt is reserved against the ceiling, recorded and settled, so an attempt that
   billed is counted. A check the engine makes on an answer (a corrupt file, the wrong size, a
   schema mismatch) fails that attempt, which stays billed. Errors that can't change on retry (an
   unknown voice, an invalid setting) fail at once.
2. **A broken result from a node body** (an output that is missing, undeclared or the wrong shape,
   or `ctx.fail(...)`) fails the step. It is not a take.
3. **A result that is valid but judged wrong** is a new take, and only when you asked for
   regeneration. Takes are priced, numbered and kept.

## Assertions

```yaml
assert:                                       # workflow level: checked while planning
  - check: ${{ len(inputs.layers) > 0 }}
    message: "give at least one layer"

steps:
  layer:
    for_each: ${{ inputs.layers }}
    assert:
      - check: ${{ facts(item.file).width >= 64 }}            # plan time: refuses the plan
        message: "layer ${{ item.id }} is narrower than 64 px"
  face:
    uses: fx/vision.annotate@1
    assert:
      - check: ${{ (steps.face.outputs.annotations.annotations[0].box[3] - steps.face.outputs.annotations.annotations[0].box[1]) * facts(inputs.sprite).height >= 64 }}
        message: "the face is smaller than 64 px"              # run time: stops this step
        on_fail: skip                                          # fail (default) or skip
```

(`vision.annotate` is planned, like `vision.review`.)

- **An assertion over plan-time values** refuses the plan, before anything runs or bills.
- **An assertion over run-time values** is checked as soon as they exist. It stops the step (and
  everything depending on it) with your message, which is shown by `grida-fx run` and
  `grida-fx inspect`.

## Budgets

```yaml
  scene:
    for_each: ${{ inputs.scenes }}
    key: ${{ item.id }}
    uses: ./workflows/voiced-scene.yaml
    budget: { max_usd: 3 }        # each scene, inside the run's --max-usd 30
```

Every paid call inside a scene reserves its worst case against the scene's budget and the run's
ceiling, and a call that does not fit is not made: its step fails and the steps that need it are
skipped. A step inside several budgets (a step with its own `budget:` inside a scene) is held to
the innermost one and the run's ceiling only. Budgets admit calls, not whole scenes: a scene starts
whatever is left, so a shared ceiling can end with a scene half done. See
[Cost](04-cost-and-cache.md#budgets-inside-a-run).

## Outputs

```yaml
outputs:
  world:    ${{ steps.propose.outputs.json }}
  images:   ${{ steps.entity.*.draw.outputs.image }}
  manifest: ${{ steps.close.outputs.manifest }}
```

Outputs are what `grida-fx run` reports and delivers, and what another workflow sees when it uses
this one as a step.

## Workflows as steps

```yaml
  icons:
    for_each: ${{ inputs.items }}
    key: ${{ item.id }}
    uses: ./workflows/icon.yaml
    with: { name: "${{ item.name }}" }
```

- **The used workflow's steps become part of this run,** addressed through the step:
  `steps.icons['lantern'].outputs.icon`, or `icons['lantern'].draw` in paths.
- **Its cache entries are shared** with standalone runs of it.
