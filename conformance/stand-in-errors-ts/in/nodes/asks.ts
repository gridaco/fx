// A node type of the stand-in-errors-ts case whose body makes a call its capability refuses.

import { node } from "@grida/fx";

export const asks_nothing = node(
  { name: "asks_nothing", outputs: { image: "image/png" }, calls: { "image.generate": 1 }, version: 1 },
  async (ctx) => {
    const result = await ctx.capability("image.generate", {});
    return { image: result.image };
  },
);
