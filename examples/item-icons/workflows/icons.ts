// The icons workflow, built from the game's own item table: one inventory icon per item, framed
// by its rarity. A builder is a function that returns a workflow; FX runs it while planning.
//
//   grida fx plan workflows/icons.ts:build

import { expr, workflow } from "@grida/fx";
import { ITEMS, type Rarity } from "../src/items.ts";

// How each rarity is framed, so the icons of one rarity read as a set. Typed by `Rarity`: a
// rarity the game adds without a frame here is an error `tsc` reports.
const FRAMES: { readonly [rarity in Rarity]: string } = {
  common: "on a plain round slate-grey token",
  rare: "on a round deep-blue token with a thin silver rim",
  legendary: "on a round violet token with a gold rim and a soft glow",
};

export function build() {
  return workflow({
    id: "icons",
    title: "Item icons",
    description: "One inventory icon for each item in the game's item table.",
    steps: {
      icon: {
        for_each: ITEMS.map((item) => ({
          id: item.id,
          prompt:
            `A game inventory icon of ${item.looks}, ${FRAMES[item.rarity]}. ` +
            "Centred, seen slightly from above, flat painted style, no text.",
        })),
        key: expr("item.id"),
        uses: "fx/image.generate@1",
        with: { prompt: expr("item.prompt"), size: "1024x1024", background: "transparent" },
      },
    },
    outputs: { icons: expr("steps.icon.*.outputs.image") },
  });
}
