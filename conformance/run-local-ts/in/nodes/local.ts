// Node types of the run-local-ts case: every output shape a local node returns, as run-local's
// nodes/local.py returns them, and a keyed output whose keys are not in sorted order.

import { node } from "@grida/fx";

export const say = node(
  { name: "say", params: { word: "string" }, outputs: { text: "text" }, version: 1 },
  async (ctx) => {
    await ctx.fact("letters", ctx.params.word.length);
    return { text: ctx.out.text(ctx.params.word + "\n") };
  },
);

export const describe = node(
  { name: "describe", inputs: { text: "text" }, outputs: { report: "json" }, version: 1 },
  async (ctx) => {
    const word = (await ctx.read.text("text")).trim();
    const report = {
      word,
      letters: word.length,
      half: word.length / 2,
      ratio: word.length / 8,
      vowels: [...word].filter((letter) => "aeiou".includes(letter)),
      upper: word.toUpperCase(),
      empty: null,
    };
    return { report: ctx.out.json(report) };
  },
);

export const letters = node(
  { name: "letters", params: { word: "string" }, outputs: { parts: "text[]" }, version: 1 },
  async (ctx) => ({ parts: [...ctx.params.word.slice(0, 2)].map((letter) => ctx.out.text(letter)) }),
);

export const index = node(
  { name: "index", params: { word: "string" }, outputs: { entries: "text{}" }, version: 1 },
  async (ctx) => {
    const word = ctx.params.word;
    const entries = new Map([
      ["first", ctx.out.text(word.charAt(0))],
      ["last", ctx.out.text(word.charAt(word.length - 1))],
    ]);
    return { entries };
  },
);

export const mark = node(
  { name: "mark", inputs: { subject: "text" }, outputs: { annotations: "annotations" }, version: 1 },
  async (ctx) => {
    await ctx.annotate({ shape: "point", at: [0.25, 0.5], label: "the dot of the i", tag: "spot" });
    await ctx.annotate({ shape: "box", box: [0.1, 0.2, 0.4, 0.6], label: "the first letter", color: "#e4572e" });
    await ctx.annotate({ shape: "points", points: [[0.1, 0.9], [0.9, 0.9]], closed: false, label: "a line" });
    await ctx.annotate({ label: "a note about the whole word" });
    return {};
  },
);

export const check = node(
  { name: "check", inputs: { subject: "text" }, outputs: {}, judge: true, version: 1 },
  async (ctx) => {
    const words = (await ctx.read.text("subject")).split(/\s+/).filter((word) => word !== "");
    await ctx.fact("words", words.length);
    await ctx.fact("verdict", words.length > 0 ? "accept" : "reject");
  },
);

// A keyed output keeps the order the body made it in: a Map, never a plain object, which would
// put the integer-like keys `2` and `10` first.
export const order = node(
  { name: "order", outputs: { entries: "text{}" }, version: 1 },
  async (ctx) => {
    const entries = new Map([
      ["sun", ctx.out.text("s")],
      ["10", ctx.out.text("t")],
      ["2", ctx.out.text("u")],
    ]);
    return { entries };
  },
);

// A keyed input arrives as a Map in the collection's order: the body writes its keys as it sees
// them.
export const listing = node(
  { name: "listing", inputs: { entries: "text{}" }, outputs: { keys: "text" }, version: 1 },
  async (ctx) => {
    const lines: string[] = [];
    for (const [key, file] of ctx.inputs.entries) {
      lines.push(`${key} ${await file.text()}\n`);
    }
    return { keys: ctx.out.text(lines.join("")) };
  },
);
