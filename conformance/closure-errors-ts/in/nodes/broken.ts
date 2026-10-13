// A module that imports a file the project does not have.

import { node } from "@grida/fx";
import { frame } from "./missing.ts";

export const broken = node(
  { name: "broken", params: { text: "string" }, outputs: { text: "text" } },
  async (ctx) => ({ text: ctx.out.text(frame(ctx.params.text)) }),
);
