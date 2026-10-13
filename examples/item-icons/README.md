# Example: item-icons

**The ask:** "Draw an inventory icon for every item in my web game, from the item table the game
already has, and put them where the game loads them."

The game's items live in its own TypeScript, `src/items.ts`, which the inventory and the shop
import. The workflow imports it too: a builder, `workflows/icons.ts`, reads the table and makes
one icon step per item, so the line that adds an item to the game also gets it an icon. No list
is copied into YAML, and the types keep the two in step.

```
item-icons/
  fx.yaml                  # every icon on Grida Gateway (GG_API_KEY)
  package.json             # the game's package: ES modules, @grida/fx
  tsconfig.json
  src/items.ts             # the game's item table
  workflows/icons.ts       # the builder: one icon per item, framed by rarity
  stand_in.ts              # answers the icons offline, for trying it without a key
```

## Try it without a key

From this folder, with Node 24 or later
([When FX needs Node](https://github.com/gridaco/fx/blob/main/docs/guide/01-getting-started.md#when-fx-needs-node)):

```bash
npm install
grida fx run workflows/icons.ts:build --stand-in stand_in.ts#answer \
  --deliver 'icons=public/icons/{key}.png'
```

A [stand-in](https://github.com/gridaco/fx/blob/main/docs/guide/05-running.md#stand-ins-testing-without-a-provider)
answers each icon with a flat token, offline and at $0, and `--deliver` copies the icons to
`public/icons/<id>.png`, where the game loads them (`iconUrl` in `src/items.ts`).

## Run it

```bash
grida fx plan workflows/icons.ts:build
```

```
icons  ·  1 phase
phase 1   3 steps   3 provider calls   $0.63 – $0.87
cached    0 of 3 known steps
estimate  $0.63 – $0.87
```

Then draw them, with your key in `GG_API_KEY` and a ceiling:

```bash
grida fx run workflows/icons.ts:build --live --max-usd 1 \
  --deliver 'icons=public/icons/{key}.png'
```

Add an item to `src/items.ts` and run again: only its icon is drawn, and the others come from the
cache. Changing an item's `looks` or rarity redraws that icon alone.

## What this example shows

- **A builder in TypeScript.** `build()` returns what `workflow(…)` makes; FX runs it while
  planning, so the workflow always matches the table
  ([Builders](https://github.com/gridaco/fx/blob/main/js/fx/README.md#builders)).
- **The game's own code as the source.** The builder imports `src/items.ts` as the game does.
  `FRAMES` has a frame for each `Rarity`, so a rarity the game adds without one is an error `tsc`
  reports.
- **A stand-in in TypeScript.** `stand_in.ts#answer` answers every `image.generate` call with a
  picture it makes; everything else runs for real.
- **Delivery.** `--deliver 'icons=public/icons/{key}.png'` copies each icon, by its item's id, into
  the game.
