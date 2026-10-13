import { describe, expect, test } from "bun:test";
import { bookmarkLayout, deckTicks, type DeckMember } from "../src/deck";

const member = (state: string, key: string | null = null, take: number | null = null): DeckMember => ({ id: `${key}-${take}-${state}`, state, key, take });

describe("deckTicks", () => {
  test("one mark per member, toned by state", () => {
    expect(deckTicks([member("succeeded"), member("failed"), member("running"), member("skipped")]).map((tick) => [tick.first, tick.last, tick.tone]))
      .toEqual([[0, 0, "done"], [1, 1, "failed"], [2, 2, "active"], [3, 3, "idle"]]);
  });

  test("more members than marks share them in even slices, the worst state showing", () => {
    const members = Array.from({ length: 48 }, (_, at) => member(at === 7 ? "failed" : "succeeded"));
    const ticks = deckTicks(members);
    expect(ticks).toHaveLength(16);
    expect(ticks[0]).toEqual({ first: 0, last: 2, tone: "done" });
    expect(ticks[2]).toEqual({ first: 6, last: 8, tone: "failed" });
    expect(ticks.at(-1)).toEqual({ first: 45, last: 47, tone: "done" });
    // Every member is in exactly one slice.
    expect(ticks.reduce((sum, tick) => sum + tick.last - tick.first + 1, 0)).toBe(48);
  });
});

describe("bookmarkLayout", () => {
  test("rows tile the marks, the outer ones reaching further", () => {
    const shape = bookmarkLayout(4, 2, 8, 12);
    expect(shape.height).toBe(16);
    expect(shape.marks).toEqual([4, 12]);
    expect(shape.rows).toEqual([{ top: -12, height: 16 }, { top: 4, height: 4 }, { top: 8, height: 4 }, { top: 12, height: 16 }]);
  });
});
