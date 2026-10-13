// Bodies that hand the engine values JSON cannot hold: the TypeScript node host refuses each
// before anything is sent, where JSON.stringify would write something else.

import { node } from "@grida/fx";

export const nan = node(
  { name: "nan", outputs: { text: "text" } },
  async (ctx) => {
    await ctx.fact("score", 0 / 0);
    return { text: ctx.out.text("scored") };
  },
);

export const keyed = node(
  { name: "keyed", outputs: { report: "json" } },
  async (ctx) => ({ report: ctx.out.json({ sizes: new Map([["a", 1]]) } as never) }),
);

export const plain = node(
  { name: "plain", outputs: { entries: "text{}" } },
  async (ctx) => ({ entries: { b: ctx.out.text("b"), a: ctx.out.text("a") } as never }),
);
