// Type-level tests of node declarations: `bun run typecheck` (tsc --noEmit) compiles this file and
// never runs it. Each `@ts-expect-error` marks code that must not compile; one that compiles anyway
// fails the check (TS2578). The rest must compile, with the types the `Equal` checks name.

import { type Ctx, type InputFile, type JsonValue, node, type NodeType, type Output } from "../../src/index.js";

type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
function expectType<T>(_value: T): void {}
declare const bytes: Uint8Array;

// ------------------------------------------------------------------------------------------------
// What a body receives

export const badge = node(
  {
    name: "badge",
    description: "Paints one badge.",
    inputs: { mask: "image?", refs: "image[]", sheet: "image{}", base: "image/png" },
    params: {
      shape: ["circle", "square", "diamond"],
      label: "string",
      count: "integer",
      scale: "number",
      transparent: "boolean",
      tags: "array",
      extra: "object",
      size: { type: "integer", default: 64, minimum: 16, maximum: 256 },
      mode: { enum: ["fast", "slow"] },
      fixed: { const: 3 },
      note: { type: "string", optional: true },
      hint: { type: "string", "x-fx-optional": true },
      kept: { type: "string", optional: true, default: "x" },
      prompt: { type: "string", template: true },
      maybe: { type: ["string", "null"] },
      sizes: { type: "array", items: { type: "integer" } },
      box: {
        type: "object",
        properties: { width: { type: "number" }, label: { type: "string" } },
        required: ["width"],
      },
      anything: {},
    },
    outputs: { image: "image/png", report: "json?", frames: "image[]", byKey: "image{}?" },
  },
  async (ctx) => {
    expectType<Equal<typeof ctx.inputs.base, InputFile>>(true);
    expectType<Equal<typeof ctx.inputs.refs, readonly InputFile[]>>(true);
    expectType<Equal<typeof ctx.inputs.sheet, ReadonlyMap<string, InputFile>>>(true);
    expectType<Equal<typeof ctx.inputs.mask, InputFile | undefined>>(true);
    expectType<Equal<typeof ctx.params.shape, "circle" | "square" | "diamond">>(true);
    expectType<Equal<typeof ctx.params.label, string>>(true);
    expectType<Equal<typeof ctx.params.count, number>>(true);
    expectType<Equal<typeof ctx.params.scale, number>>(true);
    expectType<Equal<typeof ctx.params.transparent, boolean>>(true);
    expectType<Equal<typeof ctx.params.tags, readonly JsonValue[]>>(true);
    expectType<Equal<typeof ctx.params.extra, { readonly [key: string]: JsonValue }>>(true);
    expectType<Equal<typeof ctx.params.size, number>>(true);
    expectType<Equal<typeof ctx.params.mode, "fast" | "slow">>(true);
    expectType<Equal<typeof ctx.params.fixed, 3>>(true);
    expectType<Equal<typeof ctx.params.note, string | undefined>>(true);
    expectType<Equal<typeof ctx.params.hint, string | undefined>>(true);
    // A default fills in what a step leaves out, but a step may bind it to null, which leaves it out.
    expectType<Equal<typeof ctx.params.kept, string | undefined>>(true);
    expectType<Equal<typeof ctx.params.prompt, string>>(true);
    expectType<Equal<typeof ctx.params.maybe, string | null>>(true);
    expectType<Equal<typeof ctx.params.sizes, readonly number[]>>(true);
    expectType<Equal<(typeof ctx.params.box)["width"], number>>(true);
    expectType<Equal<(typeof ctx.params.box)["label"], string | undefined>>(true);
    expectType<Equal<typeof ctx.params.anything, JsonValue>>(true);
    expectType<AbortSignal>(ctx.signal);
    expectType<number>(ctx.instance.take);
    expectType<Uint8Array>(await ctx.read.bytes("base"));
    expectType<string>(await ctx.read.text(ctx.inputs.refs[0] as InputFile));
    await ctx.inputs.mask?.bytes();
    for (const [key, file] of ctx.inputs.sheet) {
      expectType<string>(key);
      expectType<InputFile>(file);
    }
    await ctx.fact("score", 0.5);
    await ctx.annotate({ shape: "box", box: [0, 0, 1, 1], label: "all" });
    ctx.progress("painting", 0.5);
    const result = await ctx.imageGenerate({ prompt: ctx.params.label, references: ctx.inputs.refs });
    expectType<InputFile>(result.image);
    return {
      image: ctx.out.png(bytes),
      frames: [ctx.out.png(bytes), result.image, ...ctx.inputs.refs],
      byKey: new Map([["a", ctx.out.png(bytes)]]),
    };
  },
);

expectType<Equal<typeof badge.name, "badge">>(true);
expectType<NodeType>(badge);

// Optional outputs may be null or left out; a body with no required outputs may return nothing.
node({ name: "a", outputs: { report: "json?" } }, () => ({ report: null }));
node({ name: "b", outputs: { report: "json?" } }, (ctx) => ({ report: ctx.out.json({ ok: true }) }));
node({ name: "c", outputs: { report: "json?" } }, () => ({}));
node({ name: "d", judge: true }, async (ctx) => {
  await ctx.fact("verdict", "accept");
});
node({ name: "e" }, () => undefined);
node({ name: "f", inputs: { image: "image" }, outputs: { image: "image" } }, (ctx) => ({ image: ctx.inputs.image }));
node({ name: "g", outputs: { files: "file{}" } }, () => ({ files: new Map<string, Output>() }));
// The engine writes a one-file `annotations` port from the body's marks when the body leaves it out.
node({ name: "notes", outputs: { annotations: "annotations" } }, async (ctx) => {
  await ctx.annotate({ label: "a note" });
  return {};
});
node({ name: "notes_given", outputs: { annotations: "annotations" } }, (ctx) => ({ annotations: ctx.out.json({ marks: [] }) }));

/** A tool or helper takes any node's context: any input, any param. */
export function helper(ctx: Ctx): string {
  expectType<InputFile | readonly InputFile[] | ReadonlyMap<string, InputFile> | undefined>(ctx.inputs.anything);
  expectType<unknown>(ctx.params.anything);
  return ctx.instance.path;
}
helper(null as unknown as Ctx<{ readonly name: "z"; readonly inputs: { readonly image: "image" } }>);

// Loose array and object params hold JSON values: they are passed on to a request, a fact or a JSON
// output as they are.
node(
  {
    name: "forwards",
    params: { list: "array", schema: { type: "object" }, rows: { type: "array" }, loose: { type: "object", optional: true } },
    outputs: { report: "json" },
    calls: { "structured.generate": 1 },
  },
  async (ctx) => {
    await ctx.structuredGenerate({ prompt: "x", schema: ctx.params.schema, rows: ctx.params.rows });
    await ctx.fact("list", ctx.params.list);
    await ctx.fact("loose", ctx.params.loose ?? null);
    return { report: ctx.out.json({ list: ctx.params.list, rows: ctx.params.rows }) };
  },
);

// prefixItems types a tuple prefix (each item may be absent), followed by what `items` says.
node(
  {
    name: "tuples",
    params: {
      pair: { type: "array", prefixItems: [{ type: "string" }, { type: "integer" }], items: { type: "boolean" } },
      closed: { type: "array", prefixItems: [{ type: "string" }], items: false },
      open: { type: "array", prefixItems: [{ type: "string" }] },
      none: { type: "array", items: false },
    },
  },
  (ctx) => {
    expectType<Equal<typeof ctx.params.pair, readonly [string?, number?, ...boolean[]]>>(true);
    expectType<Equal<typeof ctx.params.closed, readonly [string?]>>(true);
    expectType<Equal<typeof ctx.params.open, readonly [string?, ...JsonValue[]]>>(true);
    expectType<Equal<typeof ctx.params.none, readonly []>>(true);
    expectType<Equal<(typeof ctx.params.pair)[0], string | undefined>>(true);
    // @ts-expect-error a prefix item is typed by its own schema, not by items
    const head: number | undefined = ctx.params.pair[0];
    void head;
    return {};
  },
);

// An optional param with a default may still be absent: a step that binds it to null leaves it out.
node({ name: "optional_default", params: { note: { type: "string", optional: true, default: "none" } } }, (ctx) => {
  // @ts-expect-error an optional param may be absent, with a default or not
  const note: string = ctx.params.note;
  void note;
  return {};
});

// ------------------------------------------------------------------------------------------------
// What must not compile

node({ name: "h", params: { shape: ["circle", "square"] } }, (ctx) => {
  // @ts-expect-error a param's type comes from its choices
  const triangle: "triangle" = ctx.params.shape;
  // @ts-expect-error a param's type comes from its choices
  const count: number = ctx.params.shape;
  // @ts-expect-error an unknown param
  ctx.params.size;
  void [triangle, count];
  return {};
});

node({ name: "i", params: { mode: { enum: ["fast", "slow"] }, size: "integer" } }, (ctx) => {
  // @ts-expect-error a schema's enum is the param's type
  const mode: "medium" = ctx.params.mode;
  // @ts-expect-error an integer param is a number
  const size: string = ctx.params.size;
  // @ts-expect-error an unknown input
  ctx.inputs.image;
  void [mode, size];
  return {};
});

// @ts-expect-error a declared output is missing
node({ name: "j", outputs: { image: "image" } }, () => ({}));

// @ts-expect-error a required output may not be left out by returning nothing
node({ name: "k", outputs: { image: "image" } }, () => undefined);

// @ts-expect-error an undeclared output
node({ name: "l", outputs: { image: "image" } }, (ctx) => ({ image: ctx.out.png(bytes), extra: ctx.out.png(bytes) }));

// @ts-expect-error only a one-file `annotations` port is left to the engine
node({ name: "notes_list", outputs: { annotations: "annotations[]" } }, () => ({}));

// @ts-expect-error a type with no outputs returns none
node({ name: "m" }, (ctx) => ({ extra: ctx.out.png(bytes) }));

// @ts-expect-error an output is a file, not a string
node({ name: "n", outputs: { image: "image" } }, () => ({ image: "image.png" }));

// @ts-expect-error an output is a file, not an object that looks like one
node({ name: "o", outputs: { image: "image" } }, () => ({ image: { kind: "image/png" } }));

// @ts-expect-error a list port takes a list
node({ name: "p", outputs: { frames: "image[]" } }, (ctx) => ({ frames: ctx.out.png(bytes) }));

// @ts-expect-error a keyed port takes a Map, never a plain object
node({ name: "q", outputs: { byKey: "image{}" } }, (ctx) => ({ byKey: { a: ctx.out.png(bytes) } }));

// @ts-expect-error an async body is held to the same outputs
node({ name: "r", outputs: { image: "image" } }, async () => ({ image: 1 }));

// @ts-expect-error a port string that is not a port
node({ name: "s", inputs: { image: "picture" } }, () => ({}));

// @ts-expect-error a port string that is not a port
node({ name: "t", outputs: { image: "images" } }, () => ({}));

// @ts-expect-error a port string that is not a port
node({ name: "u", inputs: { image: "image[]x" } }, () => ({}));

// @ts-expect-error a param declaration that is neither a type name, choices nor a schema
node({ name: "v", params: { size: "int" } }, () => ({}));

node(
  { name: "w", inputs: { mask: "image?", refs: "image[]?" }, params: { note: { type: "string", optional: true } } },
  async (ctx) => {
    // @ts-expect-error an optional input may be absent
    await ctx.inputs.mask.bytes();
    // @ts-expect-error an optional input may be absent
    ctx.inputs.refs.length;
    // @ts-expect-error an optional param may be absent
    ctx.params.note.length;
    // @ts-expect-error a read names a declared input
    await ctx.read.bytes("nope");
    return {};
  },
);

node({ name: "x" }, (ctx) => {
  // @ts-expect-error a fact is a JSON value
  void ctx.fact("when", new Date());
  // @ts-expect-error a mark's shape is one of point, points, box
  void ctx.annotate({ shape: "circle" });
  // @ts-expect-error a capability request holds JSON values and files
  void ctx.imageGenerate({ prompt: "p", when: new Date() });
  return {};
});

node({ name: "reads", inputs: { base: "image", mask: "image?", refs: "image[]", sheet: "image{}" } }, async (ctx) => {
  await ctx.read.bytes("base");
  // @ts-expect-error an optional input may be absent: read ctx.inputs.mask once it is there
  await ctx.read.text("mask");
  // @ts-expect-error a list input is not one file
  await ctx.read.bytes("refs");
  // @ts-expect-error a keyed input is not one file
  await ctx.read.json("sheet");
  if (ctx.inputs.mask !== undefined) {
    await ctx.read.text(ctx.inputs.mask);
  }
  return {};
});

// A body that returns early: the members of every object it returns are held to the outputs.
node({ name: "early", outputs: { report: "json?" } }, (ctx) => {
  if (ctx.instance.take > 1) {
    return;
  }
  return { report: ctx.out.json(1) };
});
node({ name: "early_null", outputs: { report: "json?" } }, async (ctx) => {
  if (ctx.instance.take > 1) {
    return null;
  }
  return { report: ctx.out.json(1) };
});

// @ts-expect-error an undeclared output, after an early return
node({ name: "early_extra", outputs: { report: "json?" } }, (ctx) => {
  if (ctx.instance.take > 1) {
    return;
  }
  return { report: ctx.out.json(1), extra: ctx.out.json(2) };
});

// @ts-expect-error a misspelled output, after an early return of null
node({ name: "early_misspelled", outputs: { report: "json?" } }, (ctx) => {
  if (ctx.instance.take > 1) {
    return null;
  }
  return { reprot: ctx.out.json(1) };
});

// @ts-expect-error a misspelled output, after an early return of an async body
node({ name: "early_async", outputs: { report: "json?" } }, async (ctx) => {
  if (ctx.instance.take > 1) {
    return;
  }
  return { reprot: ctx.out.json(1) };
});

// @ts-expect-error a judge returns no outputs, early return or not
node({ name: "early_judge", judge: true }, async (ctx) => {
  if (ctx.instance.take > 1) {
    return;
  }
  return { bonus: ctx.out.json(1) };
});

// Not caught: TypeScript reduces `a | b` to `a` when `b` is a subtype of it, so the member only
// `b` has is gone before node() sees the type; the engine refuses the output when it is returned.
node({ name: "reduced", outputs: { image: "image" } }, (ctx) => {
  const a = { image: ctx.out.png(bytes) };
  const b = { image: ctx.out.png(bytes), extra: ctx.out.png(bytes) };
  return ctx.instance.take > 1 ? a : b;
});

// @ts-expect-error a declaration field node() does not know
node({ name: "typo", descripton: "Paints." }, () => ({}));

// @ts-expect-error a declaration field node() does not know
node({ name: "typos", resource: ["prompts/a.md"], judges: true }, () => ({}));
