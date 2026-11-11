import { node } from "@grida/fx";

export const native = node(
  { name: "native", inputs: { source: "file" }, params: { settings: { type: "object" } },
    outputs: { result: "json" }, calls: { "acme.native": 1 }, version: 1 },
  async (ctx) => {
    const answer = await ctx.capability("acme.native", { source: ctx.inputs.source, ...ctx.params.settings });
    return { result: ctx.out.json(answer.json) };
  },
);
