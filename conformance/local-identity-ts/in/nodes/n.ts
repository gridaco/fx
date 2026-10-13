// Two project node types: one named by its version, one by its source.

import { node } from "@grida/fx";
import { tint } from "acme-colors";
import { outline } from "@acme/shapes";
import { frame } from "./helper.ts";
import { pad } from "#lib/pad.ts";
import type { Framed } from "./types.ts";

export const echo = node(
  { name: "echo", params: { text: "string" }, outputs: { text: "text" }, resources: ["prompts/r.md"] },
  async (ctx) => {
    const framed: Framed = frame(await ctx.prompt("prompts/r.md"));
    return { text: ctx.out.text(tint(outline(pad(framed)))) };
  },
);

export const pinned = node(
  { name: "pinned", params: { text: "string" }, outputs: { text: "text" }, version: 2 },
  async (ctx) => ({ text: ctx.out.text(frame(ctx.params.text)) }),
);
