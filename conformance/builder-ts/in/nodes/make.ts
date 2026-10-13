// A node type whose text names the take that made it, so takes differ.

import { node } from "@grida/fx";

export const make = node(
  { name: "make", params: { text: "string" }, outputs: { text: "text" }, version: 1 },
  async (ctx) => ({ text: ctx.out.text(`take ${ctx.instance.take}: ${ctx.params.text}\n`) }),
);
