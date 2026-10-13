// Two project node types: one named by its version, one by its source.

import { node } from "@grida/fx";
import { frame } from "./helper.ts";

export const echo = node(
  { name: "echo", params: { text: "string" }, outputs: { text: "text" }, resources: ["prompts/r.md"] },
  async (ctx) => ({ text: ctx.out.text(frame(await ctx.prompt("prompts/r.md"))) }),
);

export const pinned = node(
  { name: "pinned", params: { text: "string" }, outputs: { text: "text" }, version: 2 },
  async (ctx) => ({ text: ctx.out.text(frame(ctx.params.text)) }),
);
