// The game's item table. The inventory, the shop and the save files read it, and so does FX's
// icon workflow (workflows/icons.ts): the line that adds an item to the game also gets it an icon.

export type Rarity = "common" | "rare" | "legendary";

export interface Item {
  /** Also the icon's name: the game loads `/icons/<id>.png`. */
  readonly id: string;
  readonly name: string;
  readonly rarity: Rarity;
  /** What the icon shows. */
  readonly looks: string;
  readonly price: number;
}

export const ITEMS: readonly Item[] = [
  {
    id: "copper_key",
    name: "Copper key",
    rarity: "common",
    looks: "a small copper key with a round, notched bow",
    price: 5,
  },
  {
    id: "storm_lantern",
    name: "Storm lantern",
    rarity: "rare",
    looks: "a brass lantern with a tiny blue thundercloud inside the glass",
    price: 40,
  },
  {
    id: "moon_compass",
    name: "Moon compass",
    rarity: "legendary",
    looks: "a silver compass whose needle is a thin crescent moon",
    price: 300,
  },
];

/** Where the game loads an item's icon from. */
export function iconUrl(item: Item): string {
  return `/icons/${item.id}.png`;
}
