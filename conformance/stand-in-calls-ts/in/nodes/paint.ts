// Node types of the stand-in-calls-ts case, whose bodies make paid calls through the engine: a
// picture, an edit of it with a swatch the body makes, and a description of the edit.

import { node } from "@grida/fx";
import { png } from "../lib/png.ts";

const REPORT = {
  type: "object",
  properties: { subject: { type: "string" }, width: { type: "integer" }, height: { type: "integer" } },
  required: ["subject", "width", "height"],
  additionalProperties: false,
} as const;

export const paint = node(
  {
    name: "paint",
    params: { prompt: "string" },
    outputs: { image: "image/png" },
    calls: { "image.generate": 1 },
    version: 1,
  },
  async (ctx) => {
    const result = await ctx.imageGenerate({ prompt: ctx.params.prompt, size: "64x64", background: "opaque" });
    await ctx.fact("cached", result.cached);
    return { image: result.image };
  },
);

export const retouch = node(
  {
    name: "retouch",
    inputs: { image: "image/png" },
    outputs: { image: "image/png" },
    calls: { "image.edit": 1 },
    version: 1,
  },
  async (ctx) => {
    const swatch = ctx.out.png(png(8, 8, [200, 120, 40]));
    const result = await ctx.imageEdit({
      prompt: "Warm it up like the swatch.",
      size: "64x64",
      background: "opaque",
      image: ctx.inputs.image,
      references: [swatch],
    });
    return { image: result.image };
  },
);

export const describe = node(
  {
    name: "describe",
    inputs: { image: "image/png" },
    outputs: { report: "json" },
    calls: { "structured.generate": 1 },
    version: 1,
  },
  async (ctx) => {
    const result = await ctx.structuredGenerate({
      prompt: "Describe the picture.",
      schema: REPORT,
      context: [ctx.inputs.image],
    });
    const report = result.json as { subject: string; width: number; height: number };
    await ctx.fact("subject", report.subject);
    return { report: ctx.out.json(report) };
  },
);
