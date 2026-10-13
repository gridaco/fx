// JSX needs a compiler: Node's type stripping does not read it, so FX hosts no .tsx module.

import { node } from "@grida/fx";

export const card = node({ name: "card", outputs: { text: "text" } }, (ctx) => ({ text: ctx.out.text(<b>card</b>) }));
