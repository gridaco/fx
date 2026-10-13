// A module that imports lib/words.ts spelled lib/Words.ts. Where file names ignore case (macOS),
// Node would load it; FX refuses it there as everywhere else.

import { node } from "@grida/fx";
import { trimmed } from "../lib/Words.ts";

export const wrongcase = node(
  { name: "wrongcase", params: { text: "string" }, outputs: { text: "text" } },
  async (ctx) => ({ text: ctx.out.text(trimmed(ctx.params.text)) }),
);
