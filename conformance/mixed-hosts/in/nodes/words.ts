// The TypeScript half of the mixed-hosts case: node types that make what the Python half reads,
// and read what it makes.

import { node } from "@grida/fx";

export const order = node(
  { name: "order", outputs: { entries: "text{}" }, version: 1 },
  async (ctx) => ({
    entries: new Map([
      ["sun", ctx.out.text("s")],
      ["10", ctx.out.text("t")],
      ["2", ctx.out.text("u")],
    ]),
  }),
);

export const report = node(
  { name: "report", params: { phrase: "string" }, outputs: { report: "json" }, version: 1 },
  async (ctx) => {
    const words = ctx.params.phrase.split(" ");
    return { report: ctx.out.json({ words, letters: words.join("").length }) };
  },
);

export const frame = node(
  { name: "frame", inputs: { text: "text" }, outputs: { text: "text" }, version: 1 },
  async (ctx) => ({ text: ctx.out.text(`[\n${await ctx.read.text("text")}]\n`) }),
);

export const tally = node(
  { name: "tally", inputs: { sizes: "text[]" }, outputs: { text: "text" }, version: 1 },
  async (ctx) => {
    const lines = await Promise.all(ctx.inputs.sizes.map((file) => file.text()));
    await ctx.fact("lines", lines.length);
    return { text: ctx.out.text(lines.join("")) };
  },
);
