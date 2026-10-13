// A versioned node type in plain JavaScript, which no workflow uses: fx.lock pins it all the same.

import { node } from "@grida/fx";

export const shout = node(
  { name: "shout", params: { text: "string" }, outputs: { text: "text" }, version: 1 },
  async (ctx) => ({ text: ctx.out.text(ctx.params.text.toUpperCase()) }),
);
