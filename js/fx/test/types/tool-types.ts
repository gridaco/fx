// Type-level tests of agent tools, agents and stand-ins: `bun run typecheck` (tsc --noEmit)
// compiles this file and never runs it. Each `@ts-expect-error` marks code that must not compile;
// one that compiles anyway fails the check (TS2578). The rest must compile, with the types the
// `Equal` checks name.

import {
  Answer,
  type AnswerFile,
  type Ctx,
  DECLINE,
  type FromSchema,
  type InputFile,
  isDecline,
  node,
  type StandIn,
  type StandInCall,
  type Tool,
  ToolReply,
  tool,
} from "../../src/index.js";
import type { JsonValue } from "../../src/workflow.js";

type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
function expectType<T>(_value: T): void {}
declare const bytes: Uint8Array;
declare const file: InputFile;

// ------------------------------------------------------------------------------------------------
// A tool's arguments, from its parameters

export const look = tool(
  {
    name: "look",
    description: "Looks at the scene from a side.",
    parameters: {
      type: "object",
      properties: {
        side: { enum: ["front", "back", "left"] },
        zoom: { type: "number" },
        label: { type: "string" },
        steps: { type: "integer" },
        hidden: { type: "boolean" },
        tags: { type: "array", items: { type: "string" } },
        at: {
          type: "object",
          properties: { x: { type: "number" }, y: { type: "number" } },
          required: ["x", "y"],
        },
        note: { type: ["string", "null"] },
        mode: { const: "fast" },
        either: { anyOf: [{ type: "string" }, { type: "integer" }] },
        anything: {},
      },
      required: ["side", "zoom"],
    },
  },
  async (args, ctx) => {
    expectType<Equal<typeof args.side, "front" | "back" | "left">>(true);
    expectType<Equal<typeof args.zoom, number>>(true);
    expectType<Equal<typeof args.label, string | undefined>>(true);
    expectType<Equal<typeof args.steps, number | undefined>>(true);
    expectType<Equal<typeof args.hidden, boolean | undefined>>(true);
    expectType<Equal<typeof args.tags, readonly string[] | undefined>>(true);
    expectType<Equal<typeof args.at, { readonly x: number; readonly y: number } | undefined>>(true);
    expectType<Equal<typeof args.note, string | null | undefined>>(true);
    expectType<Equal<typeof args.mode, "fast" | undefined>>(true);
    expectType<Equal<typeof args.either, string | number | undefined>>(true);
    expectType<Equal<typeof args.anything, unknown>>(true);
    expectType<Ctx>(ctx);
    expectType<Record<string, unknown>>(ctx.state);
    await ctx.fact("looked", args.side);
    return new ToolReply(`looked from the ${args.side}`, [ctx.out.png(bytes), file, "views/front.png"]);
  },
);
expectType<Tool>(look);
expectType<Equal<typeof look.name, string>>(true);

// Parameters that say nothing: the arguments are an object of JSON values.
export const free = tool({ name: "free", description: "Anything.", parameters: {} }, (args) => {
  expectType<Equal<typeof args, { readonly [name: string]: JsonValue }>>(true);
  return null;
});

// Parameters that are not an object schema: the engine passes an object all the same.
tool({ name: "prim", description: "A string?", parameters: { type: "string" } }, (args) => {
  expectType<Equal<typeof args, { readonly [name: string]: JsonValue }>>(true);
  return null;
});
tool({ name: "list", description: "A list?", parameters: { type: "array", items: { type: "string" } } }, (args) => {
  expectType<Equal<typeof args, { readonly [name: string]: JsonValue }>>(true);
  return null;
});

// A tool may return what its arguments hold (readonly arrays and objects), and `as const` values.
tool(
  {
    name: "echo",
    description: "Echoes the tags.",
    parameters: { type: "object", properties: { tags: { type: "array", items: { type: "string" } } } },
  },
  (args) => ({ seen: args.tags, fixed: ["a", "b"] as const }),
);

// A tool may return a JSON value, a reply, or nothing.
tool({ name: "a", description: "d", parameters: { type: "object" } }, () => ({ seen: [1, "two", null] }));
tool({ name: "b", description: "d", parameters: { type: "object" } }, () => "text");
tool({ name: "c", description: "d", parameters: { type: "object" } }, () => undefined);
tool({ name: "d", description: "d", parameters: { type: "object" } }, async () => new ToolReply("x"));

// The value a schema describes.
expectType<Equal<FromSchema<{ readonly type: "string" }>, string>>(true);
expectType<Equal<FromSchema<{ readonly type: readonly ["integer", "null"] }>, number | null>>(true);
expectType<Equal<FromSchema<{ readonly enum: readonly [1, 2] }>, 1 | 2>>(true);
expectType<Equal<FromSchema<{ readonly oneOf: readonly [{ readonly const: "a" }, { readonly type: "boolean" }] }>, "a" | boolean>>(
  true,
);
expectType<
  Equal<
    FromSchema<{ readonly type: "object"; readonly additionalProperties: { readonly type: "number" } }>,
    { readonly [key: string]: number }
  >
>(true);
expectType<Equal<FromSchema<{ readonly type: "object"; readonly additionalProperties: false }>, {}>>(true);
expectType<Equal<FromSchema<true>, unknown>>(true);

// ------------------------------------------------------------------------------------------------
// Agents

node({ name: "places", outputs: { report: "json" } }, async (ctx) => {
  const agent = ctx.agent({ system: "You place props.", tools: [look, free], recentImages: 2, maxTokens: 400 });
  const text = await agent.run("Describe the scene.", { maxSteps: 3, images: [file, "data:image/png;base64,AA=="] });
  expectType<Equal<typeof text, string>>(true);
  const placed = await agent.run("Place the lamp.", {
    maxSteps: 8,
    submit: {
      type: "object",
      properties: { x: { type: "number" }, y: { type: "number" }, why: { type: "string" } },
      required: ["x", "y"],
    },
    check: (value) => {
      expectType<Equal<typeof value, { readonly x: number; readonly y: number; readonly why?: string }>>(true);
      return value.x < 0 ? "x is off the table" : undefined;
    },
  });
  expectType<Equal<typeof placed, { readonly x: number; readonly y: number; readonly why?: string }>>(true);
  // An async check, and a check that throws to refuse.
  await agent.run("Name it.", {
    maxSteps: 1,
    submit: { type: "string" },
    check: async (value) => {
      expectType<string>(value);
      if (!value) {
        throw ctx.fail("no name");
      }
    },
  });
  expectType<readonly JsonValue[]>(agent.transcript);
  // What the SDK hands out goes back in as it is: a submitted value with an array, the takes, an
  // `as const` value.
  const tagged = await agent.run("Tag it.", {
    maxSteps: 2,
    submit: { type: "object", properties: { tags: { type: "array", items: { type: "string" } } }, required: ["tags"] },
  });
  expectType<Equal<typeof tagged, { readonly tags: readonly string[] }>>(true);
  await ctx.fact("tagged", tagged);
  await ctx.fact("takes", ctx.instance.takes);
  const KEYS = ["x", "y"] as const;
  await ctx.fact("keys", { keys: KEYS, at: [placed.x, placed.y] as const, skipped: undefined });
  await ctx.annotate({ label: "tags", tags: tagged.tags, takes: ctx.instance.takes });
  await ctx.imageGenerate({ prompt: "a lamp", tags: tagged.tags, size: undefined, references: [file] });
  return { report: ctx.out.json({ x: placed.x, y: placed.y, text, tagged }) };
});

// ------------------------------------------------------------------------------------------------
// Stand-ins

export const answer: StandIn = async (call) => {
  expectType<StandInCall>(call);
  expectType<string>(call.capability);
  expectType<number>(call.take);
  expectType<ReadonlyMap<string, InputFile>>(call.files);
  expectType<AbortSignal>(call.signal);
  if (call.capability === "structured.generate") {
    return Answer.json({ name: "lamp" });
  }
  if (call.capability === "agent.turn") {
    return call.take > 1 ? Answer.submit({ x: 1, y: 2 }) : Answer.turn("looking", [{ name: "look", arguments: { side: "front" } }]);
  }
  if (call.capability === "image.edit") {
    return { files: { image: { bytes, kind: "image/png" } } };
  }
  const files: Record<string, AnswerFile> = { image: bytes, echo: file, path: new URL("file:///tmp/a.png") };
  return call.key ? { files, data: null } : DECLINE;
};

// A stand-in that wraps another: isDecline narrows, where `=== DECLINE` does not.
export const wraps: StandIn = async (call) => {
  const inner = await answer(call);
  if (isDecline(inner)) {
    return DECLINE;
  }
  expectType<Equal<typeof inner, Answer>>(true);
  return { files: inner.files ?? {}, data: { wrapped: true, was: inner.data ?? null } };
};

// Answers take readonly and `as const` values.
const KEYS = ["lamp", "rug"] as const;
Answer.json({ keys: KEYS });
Answer.submit({ keys: KEYS, note: undefined });
Answer.turn("", [{ name: "look", arguments: { sides: KEYS } }]);

// An async stand-in whose only answer is DECLINE.
export const declines: StandIn = async (call) => {
  if (call.capability === "agent.turn") {
    throw new Error("not this one");
  }
  return DECLINE;
};
expectType<symbol>(DECLINE);

// ------------------------------------------------------------------------------------------------
// What must not compile

tool(
  {
    name: "place",
    description: "Places a prop.",
    parameters: {
      type: "object",
      properties: { name: { type: "string" }, x: { type: "number" }, scale: { type: "number" } },
      required: ["name", "x"],
    },
  },
  (args) => {
    // @ts-expect-error an argument's type comes from its schema
    const x: string = args.x;
    // @ts-expect-error an argument the parameters do not declare
    args.y;
    // @ts-expect-error an argument that is not required may be absent
    args.scale.toFixed(2);
    // @ts-expect-error the arguments are the model's: read them, never change them
    args.name = "lamp";
    void x;
    return null;
  },
);

// @ts-expect-error a tool returns a JSON value or a reply, not a Map
tool({ name: "e", description: "d", parameters: { type: "object" } }, () => new Map([["a", 1]]));

// @ts-expect-error a tool's parameters are a JSON Schema object
tool({ name: "f", description: "d", parameters: "string" }, () => null);

node({ name: "checks" }, async (ctx) => {
  const agent = ctx.agent({ system: "s" });
  // @ts-expect-error a submitted value has its schema's type
  const count: string = await agent.run("Count.", { maxSteps: 1, submit: { type: "integer" } });
  // @ts-expect-error a check takes the submitted value's type
  await agent.run("Count.", { maxSteps: 1, submit: { type: "integer" }, check: (value: string) => value });
  // @ts-expect-error a check returns a refusal or nothing
  await agent.run("Count.", { maxSteps: 1, submit: { type: "integer" }, check: () => false });
  // @ts-expect-error a check needs a submit schema
  await agent.run("Count.", { maxSteps: 1, check: () => undefined });
  // @ts-expect-error an agent's tools are made by tool()
  ctx.agent({ system: "s", tools: [() => "plain function"] });
  void count;
});

// @ts-expect-error a stand-in returns an answer or DECLINE
export const wrong: StandIn = () => undefined;

// @ts-expect-error a stand-in declines with DECLINE, not any symbol
export const otherSymbol: StandIn = async () => Symbol("decline");

// @ts-expect-error an answer's file is bytes, an InputFile, {bytes, kind} or a file URL
export const wrongFile: StandIn = () => ({ files: { image: "picture.png" } });
