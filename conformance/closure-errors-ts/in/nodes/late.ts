// A body that imports a project module only when it runs, by a name it computes: the module is
// not in the closure its type was named by, so the run fails the node.

import { node } from "@grida/fx";

export const late = node(
  { name: "late", params: { name: "string" }, outputs: { text: "text" } },
  async (ctx) => {
    const { shout } = (await import(`../lib/${ctx.params.name}.ts`)) as { shout(text: string): string };
    return { text: ctx.out.text(shout("late")) };
  },
);
