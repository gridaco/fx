# Nodes

A node type is one kind of work. You use built-in ones by name. When none fits, write your own in
[Python](#writing-a-node-type-in-python) or [TypeScript](#writing-a-node-type-in-typescript), or in
[any language](#other-languages) that speaks the node protocol.

## Built-in node types (`fx/...`)

Every built-in type is declared, so a workflow using it plans and prices. A type marked
**planned** has no body (or, for a paid type, no route) yet: it plans, and runs once that lands.

**Generation** (paid; each needs a route):

| Type | Inputs and settings → output |
|---|---|
| `image.generate@1` | `prompt`, `size`, `background` → `image`. It also takes `references`, input pictures to draw from: only `imagegen@codex` accepts them (up to 5); on the other routes, use `image.edit@1`. |
| `image.edit@1` | `image`, `mask` (a PNG the size of `image`: fully transparent pixels mark where it may change; a strong hint, not a guarantee), `references`, `prompt`, `size`, `background` → `image` |
| `structured.generate@1` | `prompt`, `system`, `context` (text, json and images; images are reduced and flattened onto `matte:`, white by default), `schema`, `max_tokens` → `json` |
| `video.generate@1` | `prompt`, `first_frame`, `last_frame`, `duration`, `resolution`, `aspect_ratio` → `video` |
| `speech.generate@1` | `text` (delivery as audio tags, `[whispering] ...`), `voice`, `stability`, `language_code`, `max_chars` → `audio` |
| `sound.generate@1` | `prompt`, `duration`, `prompt_influence`, `loop` → `audio` |
| `music.generate@1` | `prompt`, `duration` → `audio` |
| `mesh.generate@1` | `views` (pictures by side: `front` and at least one of `back`, `left`, `right`), `face_limit`, `quad`, `texture`, `pbr` → `model` |
| `mesh.rig@1` | `model`, `rig_type`, `skeleton`, `allow_negative_check` → rigged `model` (binary glTF) |
| `background.remove@1` | `image` → `image` with alpha (**planned**: FX can call fal for it, but ships no route yet; a route in your own table makes it run) |

- **A prompt may be a template file** (`prompt: ./prompts/draw.md`), rendered with the step's
  `vars:` and the workflow's `inputs`.
- **Some calls are long provider jobs** (video, rigging). FX submits once, polls, and after a
  crash collects instead of submitting again.
- **Length-priced calls** (speech by characters, music by seconds) are priced from `max_chars` or
  `duration` when the text isn't known yet.

**Judges and annotators** (see [Annotations and judges](06-annotations-and-judges.md); the free
ones are local and run on Python):

| Type | What it does |
|---|---|
| `image.check_alpha@1` | free: is the picture transparent where it should be (`expect: transparent`), or fully opaque (`expect: opaque`) |
| `image.check_size@1` | free: is the picture `width` × `height` |
| `vision.annotate@1` | **planned:** `image` or `images`, a `prompt`, optional `shapes`, `fields` and `grounding` → `annotations` |
| `vision.review@1` | **planned:** `image` or `images`, a `question` (a template) or `criteria`, optional `report`, `review_size` and `feedback` (marks from an earlier take) → `annotations`, a `mask`, and a verdict |
| `structured.review@1` | **planned:** `subject` (json), a `rubric` file, a `question` or `criteria`, and optional `context` files → a verdict, with per-criterion reasons |
| `audio.check_duration@1` | **planned:** is the clip between `min_s` and `max_s` |

**Media** (local, free; they run on Python, see
[Getting started](01-getting-started.md#when-fx-needs-python)):
- image: `resize` (`longest_side`, or `width` and `height`), `crop` (`box`, `padding`, or a
  `region` from annotations), `pad` (`width`, `height`), `mirror_repeat` (`axis`: `x`, `y` or
  `xy`);
- **planned:** image `compose`, `key` (chroma), `sheet`, `contact_sheet`; video `probe`, `frames`,
  `encode`; audio `normalize`, `trim`, `concat`, `mix`; annotations `mask`, `filter`.

**Plumbing** (local, free; all but `select` run on Python):
- `select@1`: the first of `first_of` that exists and wasn't rejected (it runs in the engine);
- `json.merge@1`: `documents` → one `json`;
- `package@1`: lay results out by destination path (`files`) beside a `manifest` you write in YAML;
  it returns both;
- `files.copy@1`: `file` → `file`.

`grida fx nodes` lists every type. `grida fx nodes speech.generate` shows one type's settings,
outputs and the routes that can serve it.

## Writing a node type in Python

```python
# nodes/mirror.py
from grida.fx import node, Ctx
from PIL import Image, ImageOps


@node(
    "mirror_repeat",
    inputs={"image": "image"},
    params={"axes": ("x", "y", "xy")},
    outputs={"image": "image/png"},
)
def mirror_repeat(ctx: Ctx) -> dict:
    src = ctx.read.image("image")  # a PIL image of the staged input
    out = Image.new(src.mode, (src.width * 2, src.height))
    out.paste(src, (0, 0))
    out.paste(ImageOps.mirror(src), (src.width, 0))
    return {"image": ctx.out.png(out)}  # FX validates, stores and records it
```

Use it from a workflow with `uses: ./nodes/mirror.py#mirror_repeat`. Helper modules import from
the project root: `from nodes.seams_math import wrap_seam_error`.

What you do and don't do in a node:

- **Do:** read what you declared, do the work, return the declared outputs.
- **Don't:** write files outside `ctx`, read files you didn't declare, call providers directly (use
  capabilities, below), retry, cache, or record provenance. FX does all of that.
- **Outputs must match the declaration.** Returning an undeclared output, or omitting a declared
  one, fails the step. An optional output (`"image?"`) may be omitted, or returned as `None`.

`def` or `async def` both work.

### Declaring a node

```python
@node(
    "layer_repaint",                       # the type name, unique in your project
    inputs={"image": "image", "refs": "image[]", "lines": "audio{}", "spec": "json?"},
    params={"opaque": bool, "strength": float, "labels": list, "options": dict},
    outputs={"image": "image/png", "report": "json"},
    calls={"image.edit": 1},               # paid calls it makes, at most, per run (priced by plans)
    resources=["prompts/seam.md"],         # project files this node reads
    tools=["ffmpeg"],                      # external programs; checked by doctor and by plans
    version=None,                          # see Versions
)
```

**Input shapes:**

| Notation | Meaning |
|---|---|
| `"image"` | one file |
| `"image[]"` | a list |
| `"image{}"` | a keyed collection: an ordered `{key: file}` |
| a trailing `?` | optional, `None` when absent |

**Params** are any JSON value: `int`, `float`, `bool`, `str`, `list`, `dict`, a tuple of allowed
values, or a JSON Schema dict.

### What `ctx` gives you

| | |
|---|---|
| `ctx.read.image(name)`, `.json(name)`, `.text(name)`, `.annotations(name)`, `.bytes(name)` | read a declared input (`.image` returns PIL) |
| `ctx.inputs[name]` | the file itself: `.path`, `.kind`, `.digest`, `.key`, `.facts` (its file facts, computed by FX: `bytes`, `kind`, and for images `width`, `height`, `has_alpha`, `opaque`; for WAV audio `duration`; for MP4, WebM and Matroska video `width`, `height`, `fps`, `duration`, `frames`, `has_alpha`) |
| `ctx.params` | your settings, typed |
| `ctx.out.png(img)`, `.json(obj)`, `.text(s)`, `.bytes(data, kind)` | produce an output value to return. FX writes a JSON value itself, in one canonical layout, so every SDK writes the same bytes. |
| `ctx.out.path(name)` then `ctx.out.file(path)` | for tools that write a file themselves: get a path, then return it |
| `ctx.work_path(name)`, `ctx.state` | scratch space and state for this node's run (tools, agents) |
| `ctx.fact(name, value)` | report a node fact, a small value about this result: a score, a verdict, a measurement. Workflows read it as `steps.<name>.facts.<fact>` in `if:` and judges, and `grida fx inspect` shows it. |
| `ctx.annotate(shape=…, label=…, color=…, tag=…, **fields)` | add a mark to this node's `annotations` output |
| `ctx.capability(...)`, or typed: `ctx.image_generate(...)`, `ctx.image_edit(...)`, `ctx.structured_generate(...)` | a paid call through FX: routed, priced, budgeted, retried by the engine, recorded, call-cached. Results give files (`result.image`) and conveniences (`result.pil()`). |
| `ctx.agent(...)` | a tool-using model loop (below) |
| `ctx.tool("ffmpeg").run([...argv])` | an external program you declared, run in a session of its own and stopped with everything it started at a timeout |
| `ctx.prompt(path, **vars)` | render one of your declared `resources` with `${{ }}` |
| `ctx.progress(text, fraction)` | shown live by `grida fx run` |
| `ctx.cancelled` | check it in long loops; FX also stops you at the step timeout |
| `raise ctx.fail(message)` | a broken result: the step fails with your message (not a take) |

### Paid calls inside your node

```python
@node(
    "layer_repaint",
    inputs={"image": "image"},
    outputs={"image": "image/png"},
    calls={"image.edit": 1},
    resources=["prompts/seam.md"],
)
async def layer_repaint(ctx: Ctx) -> dict:
    seam = make_seam_mask(ctx.read.image("image"))
    result = await ctx.image_edit(
        image=ctx.inputs["image"],
        mask=ctx.out.png(seam),
        prompt=ctx.prompt("prompts/seam.md"),
    )
    return {"image": result.image}
```

- **Declare what you call.** `calls={"image.edit": 1}` says the most calls of each capability one
  run of the node makes. The plan prices the node from it, and a call you did not declare fails
  the step instead of spending.
- **Every capability call is kept by its request.** If your node runs again with an identical
  request (same prompt, same input bytes, same route, same take), the answer comes from the cache
  and nothing is billed. That's why editing your node's code is safe; see
  [Cost](04-cost-and-cache.md).
- **Don't retry a capability call yourself.** The engine is its only retry owner: at most 6
  attempts, each one reserved, recorded and settled
  ([The workflow file](02-workflow-file.md#judges) has the three kinds of failure).
- **Calling a provider your own way** (an HTTP API FX doesn't know) is possible. Declare
  `retry="engine"` and FX runs your node again after it raises an exception, at most 6 runs in
  all; calls an earlier run completed come from the cache. A node that failed with
  `ctx.fail(...)`, through a paid call that failed or was refused, or by running past its
  `timeout:` is never run again. FX cannot price or hold a call it does not route, so
  the plan and the ceiling don't see it; declaring its price is planned. Prefer a capability when
  one exists.
- **Errors that won't change on retry** (an unknown voice, invalid settings) fail at once. They are
  not retried.

### Judges you write

Declare `judge=True`: the node must then report a `verdict` fact, and the plan can check, offline,
that every `judges:` step really is a judge. A judge may have outputs too.

```python
@node(
    "loops_already", inputs={"image": "image"}, outputs={"annotations": "annotations?"}, judge=True
)
def loops_already(ctx: Ctx) -> dict:
    err = seam_error(ctx.read.image("image"))
    ctx.fact("seam_error", err)
    if err >= 0.02:
        ctx.annotate(
            shape="box", box=[0.97, 0.0, 1.0, 1.0], label=f"visible step at the seam ({err:.3f})"
        )
    ctx.fact("verdict", "accept" if err < 0.02 else "reject")
    return {}
```

### Agents

```python
from grida.fx import Ctx, ToolReply, node, tool


def blender(ctx: Ctx, script: str, *args: str) -> None:
    ctx.tool("blender").run(["--background", "--python", script, "--", *args])


@tool
def rotate(ctx: Ctx, degrees: float) -> str:
    """Rotate the mesh around its vertical axis."""
    blender(ctx, "blender/rotate.py", str(ctx.state["mesh"]), str(degrees))
    return "rotated"


@tool
def render_views(ctx: Ctx) -> ToolReply:
    """Render front, side and top views."""
    folder = ctx.work_path("views")
    folder.mkdir(parents=True, exist_ok=True)
    blender(ctx, "blender/render_views.py", str(ctx.state["mesh"]), str(folder))
    return ToolReply("front, side and top", images=sorted(folder.glob("*.png")))


@node(
    "orient",
    inputs={"mesh": "model"},
    outputs={"mesh": "model"},
    calls={"agent.turn": 30},
    tools=["blender"],
    resources=["prompts/orient.md", "blender/rotate.py", "blender/render_views.py"],
)
async def orient(ctx: Ctx) -> dict:
    ctx.state["mesh"] = ctx.inputs["mesh"].copy_to(ctx.work_path("mesh.glb"))
    agent = ctx.agent(system=ctx.prompt("prompts/orient.md"), tools=[rotate, render_views])
    await agent.run("Stand the character upright, facing +Y, feet on the ground.", max_steps=30)
    return {"mesh": ctx.out.file(ctx.state["mesh"])}
```

- **Declare the turns:** `calls={"agent.turn": 30}` bounds the agent, and the plan prices it from
  that. `routes:` in `fx.yaml` (or the step's `route:`) picks the model for `agent.turn`.
- **Your tools run on your machine.** A tool returns text, or a `ToolReply` when the model should
  see pictures too. Each model turn is a paid capability call: priced, budgeted, recorded and kept
  by request, so a resumed run replays the turns it already paid for.
- **A structured answer:** `agent.run(..., submit=schema, check=fn)` gives the agent a `submit`
  tool; the run ends with the answer your `check` accepts.

### External programs

```python
tools = ["blender", "ffmpeg"]
```

What that gets you:
- `grida fx doctor` checks that the tools exist, and a run refuses to start, before any spend, when
  a step that may run declares one that is missing.
- `GRIDA_FX_TOOL_<NAME>` points at a particular build
  (`GRIDA_FX_TOOL_BLENDER=/opt/blender/blender`); otherwise the tool is found on `PATH`.
- Recording each tool's exact version with its results, and version constraints
  (`"blender>=5.2"`), are planned.

Scripts you pass to a tool (`blender/rotate.py`) are declared `resources`, so they count as part
of the node.

### Versions

Every node type has an identity, and results are kept per identity. For a node type in your
project you choose how that identity is set:

- **Omit `version`** (the default for your own nodes): the identity follows your source. "Source"
  means:
  - the node's module;
  - the project modules it imports;
  - its declared `resources`.

  Edit any of them, and its results are recomputed on the next run. Paid calls inside it are still
  kept by request, so only calls whose request actually changed bill again.
- **Set `version=3`:** the identity changes only when you bump the number. Use this once a node is
  stable and its local work is expensive.
  - `grida fx lock` records the source digest in `fx.lock`. A source change without a bump makes
    `grida fx plan` stop and ask: bump, or confirm "no change in behaviour" with
    `grida fx lock --same nodes/mirror.py#mirror_repeat`.
  - A versioned node with no `fx.lock` entry yet plans normally; `grida fx lock --check` reports
    it as unlocked.

Built-in node types always carry versions; their major version is in `uses:`.

## Writing a node type in TypeScript

```ts
// nodes/pack.ts
import { node } from "@grida/fx";
import { layOut } from "#lib/atlas.ts";

export const packAtlas = node(
  {
    name: "pack_atlas",
    inputs: { icons: "image{}" },
    params: { columns: "integer", cell: { type: "integer", default: 64, minimum: 8 } },
    outputs: { atlas: "image/png", index: "json" },
  },
  async (ctx) => {
    const icons = [...ctx.inputs.icons]; // [key, InputFile] pairs, in the collection's order
    // Your own code: draws the atlas with the image library you choose, and returns its PNG
    // bytes and where each icon went, as [{ key, x, y }, …].
    const atlas = await layOut(icons, ctx.params.columns, ctx.params.cell);
    return { atlas: ctx.out.png(atlas.png), index: ctx.out.json(atlas.cells) };
  },
);
```

Use it from a workflow with `uses: ./nodes/pack.ts#packAtlas`, the name of the export (letters,
digits and `_`). `node(declaration, body)` declares a node type; every export that holds one is a
type FX finds in the module, and other exports are left alone. The body may be `async`. What it
should and shouldn't do is what [a Python node](#writing-a-node-type-in-python) should and
shouldn't.

Everything a body prints (`console.log`, `process.stdout`, a program it starts) goes to FX's
standard error, never into its conversation with the engine, and standard input is empty.
`process.exit()` throws instead of ending the host. A project may use Python and TypeScript nodes
together, even in one workflow; one language per project is the usual choice.

### A TypeScript project

Node runs a project's `.ts` files as they are, stripping their types: there is no build step, and
FX never type checks. `tsc` does, when you ask it to. Set the project up once, with Node 24 or later
([Getting started](01-getting-started.md#when-fx-needs-node)), as the
[hello-ts](../../examples/hello-ts/) example is:

```sh
npm install @grida/fx
npm install --save-dev typescript @types/node
```

`package.json`, beside the dependencies npm records:

```json
{
  "type": "module",
  "imports": { "#lib/*": "./lib/*" }
}
```

`tsconfig.json` (`erasableSyntaxOnly` needs TypeScript 5.8 or later):

```json
{
  "compilerOptions": {
    "module": "nodenext",
    "strict": true,
    "noEmit": true,
    "allowImportingTsExtensions": true,
    "erasableSyntaxOnly": true,
    "verbatimModuleSyntax": true
  }
}
```

Write TypeScript that Node can strip:
- **Erasable syntax only.** Types, interfaces, generics, `as` and `satisfies` are fine; `enum`,
  `namespace` and parameter properties are not, since Node would have to compile them.
  `erasableSyntaxOnly` makes `tsc` say so.
- **Imports name the `.ts` file:** `import { seamError } from "./seams.ts"`, not `./seams.js` or
  `./seams`. Import types with `import type`, which `verbatimModuleSyntax` asks for.
- **`#` imports instead of tsconfig `paths`.** package.json's `imports` (`"#lib/*": "./lib/*"`) is
  Node's own way to name a folder, so Node and `tsc` agree on it: `import { layOut } from
  "#lib/atlas.ts"`.
- **ES modules only.** `"type": "module"`, and node modules are `.ts`, `.mts`, `.js` or `.mjs`.
  A `.tsx`, `.cts` or `.cjs` module is refused (`no node host for .tsx modules`), and so is a
  project module that Node would load as CommonJS.
- **Packages ship JavaScript.** Node strips no types under `node_modules`, so a package of nodes
  you publish ships JavaScript and `.d.ts`, as `@grida/fx` does. A workspace package linked into
  `node_modules` runs from its own folder, `.ts` files and all.

### Declaring a node in TypeScript

```ts
export const layerRepaint = node(
  {
    name: "layer_repaint", // the type name, unique in your project
    inputs: { image: "image", refs: "image[]", lines: "audio{}", spec: "json?" },
    params: {
      opaque: "boolean",
      strength: "number",
      side: ["left", "right"], // one of these values
      labels: { type: "array", items: { type: "string" } }, // any JSON Schema
      note: { type: "string", optional: true }, // a step may leave it out
    },
    outputs: { image: "image/png", report: "json" },
    calls: { "image.edit": 1 }, // paid calls it makes, at most, per run
    resources: ["prompts/seam.md"], // project files this node reads
    tools: ["ffmpeg"], // external programs
    version: null, // see Versions
  },
  async (ctx) => {
    // ctx.inputs.refs is a readonly InputFile[], ctx.inputs.lines a ReadonlyMap<string,
    // InputFile>, ctx.inputs.spec an InputFile or undefined; ctx.params.side "left" | "right".
    const report = { side: ctx.params.side, lines: [...ctx.inputs.lines.keys()] };
    return { image: ctx.inputs.image, report: ctx.out.json(report) };
  },
);
```

The fields are `@node`'s, with the type's name as `name`. The body's types follow from them:

| Declared | In the body |
|---|---|
| input `"image"` | `ctx.inputs.image`, an `InputFile` |
| input `"image[]"` | a `readonly InputFile[]` |
| input `"audio{}"` | a `ReadonlyMap<string, InputFile>`, in the collection's order |
| input with a trailing `?` | an optional property, `undefined` when absent |
| param `"string"`, `"integer"`, `"number"`, `"boolean"` | `string`, `number`, `number`, `boolean` |
| param `"array"`, `"object"` | `readonly JsonValue[]`, `{ readonly [key: string]: JsonValue }`: JSON values you can pass on to a call's request, a fact or `ctx.out.json` |
| param `["left", "right"]` | `"left" \| "right"` |
| param as a JSON Schema object | its type, read from `type`, `enum`, `const`, `prefixItems`, `items` and `properties`, and a `JsonValue` where they say nothing narrower; `optional: true` makes it an optional property, with a `default` or without |
| output `"image/png"` | an `Output` made by `ctx.out`, or an `InputFile` |
| output `"image[]"` or `"image{}"` | a readonly array of them, or a `ReadonlyMap` of them |
| output `"json?"` | may be left out, or `null` |

- **`tsc` checks the body against its declaration:** a param used as another type than its
  choices, a param or input you didn't declare, an output missing or undeclared, a value that is
  not a file where an output wants one, a port that isn't one (`"imgae"`), and an optional input
  or param used as if it were always there.
- **FX checks the values when it runs the node.** A step's `with:` can bind a param to an
  expression, so before the body starts, FX checks each param's value against its declaration and
  fails the step with the first that doesn't fit:
  `param side: "up" is not one of ["left","right"]`, or
  `param strength: "big" is not of type "number"`. An optional param whose value isn't there
  (bound to an input nobody gave) is left out, as if the step hadn't set it. When the body
  returns, FX checks its outputs: one missing or undeclared, or a value that isn't a file, fails
  the step.
- **Keyed collections are `Map`s, both ways.** A plain object puts integer-like keys (`"10"`,
  `"2"`) before the others; a `Map` keeps the collection's order. Return a `Map` for a keyed
  output, and keep keyed data you write as JSON in an array, as `layOut` does above.

### What `ctx` gives you in TypeScript

| | |
|---|---|
| `ctx.inputs.name` | the file, an `InputFile`: `.path`, `.kind`, `.digest`, `.size`, `.name`, `.key`, `.facts` (its [file facts](#what-ctx-gives-you)), and `await .bytes()`, `.text()`, `.json()` |
| `await ctx.read.bytes(name)`, `.text(name)`, `.json(name)` | read an input of one file that isn't optional, by its name, or any `InputFile`. `text` is strict UTF-8. There is no image decoding: read `.path` with the image library you choose |
| `ctx.params` | your settings, typed from the declaration |
| `ctx.out.png(bytes)`, `.json(value)`, `.text(text)`, `.bytes(data, kind)` | produce an output value to return; `png` takes encoded PNG bytes |
| `ctx.out.path(name)` then `ctx.out.file(path)` | for tools that write a file themselves: get a path, then return it |
| `ctx.workPath(name)`, `ctx.workDir`, `ctx.state` | scratch space and state for this node's run (tools, agents) |
| `await ctx.fact(name, value)` | report a node fact |
| `await ctx.annotate({ shape, label, color, tag, … })` | add a mark to this node's `annotations` output |
| `await ctx.capability(capability, request)`, or typed: `ctx.imageGenerate(…)`, `ctx.imageEdit(…)`, `ctx.structuredGenerate(…)` | a paid call through FX. The result has `.image`, `.audio`, `.video` (its first file of that kind), `.files`, `.json`, `.data`, `.cached` and `.costUsd` |
| `ctx.agent({ system, tools })` | a tool-using model loop (below) |
| `await ctx.tool("ffmpeg").run([...argv])` | an external program you declared, as in Python; it resolves to `{ status, stdout, stderr }` |
| `await ctx.prompt(path, variables)` | render one of your declared `resources` with `${{ }}` |
| `ctx.progress(text, fraction)` | shown live by `grida fx run` |
| `ctx.signal` | an `AbortSignal`, aborted when FX stops the node (its step's `timeout:`, a stopped run): pass it on to what you await, or check `ctx.signal.aborted` in long loops |
| `throw ctx.fail(message)` | a broken result: the step fails with your message (not a take) |
| `ctx.instance` | `.id`, `.path`, `.step`, `.key` and `.take` of the instance running |

The JSON values you give FX (facts, marks, `ctx.out.json`, a call's request) are checked before
they leave: `NaN`, the infinities, integers beyond 2^53, `undefined` in an array, and objects that
aren't plain (a `Map`, a `Date`, a class instance) are refused with a sentence that says where they
are, rather than changed the way `JSON.stringify` would change them. An object member that is
`undefined` is left out, as `JSON.stringify` leaves it out: `{ prompt, size: ctx.params.size }`
sends no `size` when the step left that optional param out.

### Paid calls, judges and agents in TypeScript

A paid call, as [in Python](#paid-calls-inside-your-node): declare it, and never retry it
yourself. `retry: "engine"` runs the node again after it throws, as `retry="engine"` does.

```ts
export const layerRepaint = node(
  {
    name: "layer_repaint",
    inputs: { image: "image" },
    outputs: { image: "image/png" },
    calls: { "image.edit": 1 },
    resources: ["prompts/seam.md"],
  },
  async (ctx) => {
    const seam = await seamMask(ctx.inputs.image); // PNG bytes, from your own code
    const result = await ctx.imageEdit({
      image: ctx.inputs.image,
      mask: ctx.out.png(seam),
      prompt: await ctx.prompt("prompts/seam.md"),
    });
    return { image: result.image };
  },
);
```

A [judge](#judges-you-write) declares `judge: true` and reports a `verdict` fact:

```ts
export const loopsAlready = node(
  {
    name: "loops_already",
    inputs: { image: "image" },
    outputs: { annotations: "annotations?" },
    judge: true,
  },
  async (ctx) => {
    const err = await seamError(ctx.inputs.image);
    await ctx.fact("seam_error", err);
    if (err >= 0.02) {
      await ctx.annotate({
        shape: "box",
        box: [0.97, 0, 1, 1],
        label: `visible step at the seam (${err.toFixed(3)})`,
      });
    }
    await ctx.fact("verdict", err < 0.02 ? "accept" : "reject");
    return {};
  },
);
```

An [agent](#agents) gets tools declared with `tool(…)`. A tool's `parameters` is a JSON Schema
object, and its handler's arguments are typed from it: write it in place, or `as const` when you
keep it in a variable of its own.

```ts
// nodes/orient.ts
import { copyFile, mkdir, readdir } from "node:fs/promises";
import { join } from "node:path";
import { type Ctx, node, tool, ToolReply } from "@grida/fx";

async function blender(ctx: Ctx, script: string, ...args: string[]): Promise<void> {
  await ctx.tool("blender").run(["--background", "--python", script, "--", ...args]);
}

const rotate = tool(
  {
    name: "rotate",
    description: "Rotate the mesh around its vertical axis.",
    parameters: {
      type: "object",
      properties: { degrees: { type: "number" } },
      required: ["degrees"],
    },
  },
  async ({ degrees }, ctx) => {
    await blender(ctx, "blender/rotate.py", String(ctx.state["mesh"]), String(degrees));
    return "rotated";
  },
);

const renderViews = tool(
  {
    name: "render_views",
    description: "Render front, side and top views.",
    parameters: { type: "object", properties: {} },
  },
  async (_arguments, ctx) => {
    const folder = ctx.workPath("views");
    await mkdir(folder, { recursive: true });
    await blender(ctx, "blender/render_views.py", String(ctx.state["mesh"]), folder);
    const views = (await readdir(folder)).filter((name) => name.endsWith(".png")).sort();
    return new ToolReply("front, side and top", views.map((name) => join(folder, name)));
  },
);

export const orient = node(
  {
    name: "orient",
    inputs: { mesh: "model" },
    outputs: { mesh: "model" },
    calls: { "agent.turn": 30 },
    tools: ["blender"],
    resources: ["prompts/orient.md", "blender/rotate.py", "blender/render_views.py"],
  },
  async (ctx) => {
    const mesh = ctx.workPath("mesh.glb");
    await copyFile(ctx.inputs.mesh.path, mesh);
    ctx.state["mesh"] = mesh;
    const agent = ctx.agent({
      system: await ctx.prompt("prompts/orient.md"),
      tools: [rotate, renderViews],
    });
    await agent.run("Stand the character upright, facing +Y, feet on the ground.", {
      maxSteps: 30,
    });
    return { mesh: ctx.out.file(mesh) };
  },
);
```

- **A tool returns** text, another JSON value, or a `ToolReply` when the model should see
  pictures too: files, `ctx.out` outputs, paths or `data:` URLs. A tool that throws tells the
  model what went wrong, and the agent goes on; one that throws `ctx.fail(…)` fails the node.
- **A structured answer:** `agent.run(instructions, { maxSteps, submit: schema, check })` gives the
  agent a `submit` tool and resolves to the answer, typed from `submit`. `check` returns a string
  to refuse an answer with that reason, and nothing to accept it.
- **External programs** are declared and found as [in Python](#external-programs):
  `tools: ["blender", "ffmpeg"]`. `ctx.tool(name).run(argv, { cwd, timeoutS, env, check })`
  starts one in a session of its own, in the node's work folder unless `cwd` says otherwise. A
  non-zero exit fails the node unless `check: false`. `timeoutS` (seconds, or `null` for none)
  stops it with everything it started and fails the node. It is stopped the same way when FX
  stops the node (`ctx.signal`) and when the node's run ends: no program outlives its node.

### Versions in TypeScript

[Versions](#versions) work as in Python: `version: 3` in the declaration, and
`grida fx lock --same nodes/pack.ts#packAtlas` to confirm a change. Without a version, the
identity follows the source:
- the node's module, and every project file it imports, on through their own imports: relative
  paths, `#` imports and `.json` modules alike. A workspace package linked into `node_modules`
  counts by its real folder, so one inside the project is project files;
- the files of a package you list under `sources:` in `fx.yaml`, wherever it is installed;
- its declared `resources`.

An import written with a literal path counts wherever it is, whether or not it runs, as in
Python: `import … from "./x.ts"`, `export … from`, `await import("./x.ts")` and
`require("./x.cjs")`, inside a function or a branch that never runs too. An `import type` adds
nothing, since Node erases it, and neither does any file under `node_modules`. An import whose
path is computed as the code runs (`` import(`./plugins/${name}.ts`) ``) counts once Node loads
it, for the module whose top-level code loaded it, wherever the code doing the loading lives. A
project file that only a package loads (a config loader's `import(url)`) isn't counted, and
neither is one loaded in a worker thread or a child process: list such files under `resources`
so their edits count. The list doesn't depend on which modules FX happened to load first, except
in rare cases the [protocol](../../spec/protocol.md#51-describe) lists, such as a registry that
caches what it loaded by a computed path: write its paths as literals.

Two rules keep that list true:
- **Spell paths as they are on disk.** macOS would load `./lib/Atlas.ts` for `lib/atlas.ts`, so FX
  refuses the module instead: `nodes/pack.ts failed to import: lib/Atlas.ts is spelled
  lib/atlas.ts on disk`. A body that loads such a path when it runs fails its step with the
  same advice: `the body loaded lib/Atlas.ts at run time, outside its source closure:
  lib/Atlas.ts is spelled lib/atlas.ts on disk`.
- **Import statically.** A body that loads a project file outside its module's closure, by a
  path it computes (in its own `import()` or a helper's), fails its step: `the body loaded
  lib/atlas.ts at run time, outside its source closure: import it statically`, whether or not
  another node's module had loaded that file already. A literal path counts before it runs, so
  `await import("../lib/atlas.ts")` in the body is as good as an import at the top. Each run is
  checked on its own, and never fails for what another run loaded, even from work it left
  running.

## Other languages

Any other language can implement the node protocol (JSON-RPC 2.0 over stdio, specified in
[`spec/`](../../spec/): the engine sends the request, the node calls back to the engine, and
answers with its outputs) and be used the same way. FX picks a module's host by its suffix: `.py`
for Python; `.ts`, `.mts`, `.js` and `.mjs` for TypeScript.
