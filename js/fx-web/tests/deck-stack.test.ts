import { describe, expect, test } from "bun:test";
import { deckStack } from "../src/deck";

const shown = (stack: ReturnType<typeof deckStack>) => stack.shown.flatMap((on, at) => on ? [at] : []);

describe("deckStack", () => {
  test("shows two members before the front and one after", () => {
    const stack = deckStack(9, 5);
    expect(shown(stack)).toEqual([3, 4, 5, 6]);
    expect(stack.slots.slice(3, 7)).toEqual([0, 1, 2, 3]);
    expect(stack.order.at(-1)).toBe(5);
  });

  test("takes more from the other side at the ends", () => {
    expect(shown(deckStack(9, 0))).toEqual([0, 1, 2, 3]);
    expect(shown(deckStack(9, 1))).toEqual([0, 1, 2, 3]);
    expect(shown(deckStack(9, 8))).toEqual([5, 6, 7, 8]);
    expect(deckStack(9, 8).slots[8]).toBe(3);
  });

  test("hidden members rest under the nearer end, below it", () => {
    const stack = deckStack(9, 5);
    expect(stack.slots).toEqual([0, 0, 0, 0, 1, 2, 3, 3, 3]);
    expect(stack.order.indexOf(2)).toBeLessThan(stack.order.indexOf(3));
    expect(stack.order.indexOf(8)).toBeLessThan(stack.order.indexOf(6));
  });

  test("a small deck shows every member in place", () => {
    const stack = deckStack(3, 0);
    expect(stack.slots).toEqual([0, 1, 2]);
    expect(stack.order).toEqual([2, 1, 0]);
  });

  test("a kept window stays while it holds the front", () => {
    expect(deckStack(9, 3, 5).start).toBe(1);
    expect(deckStack(9, 6, 5).start).toBe(5);
  });
});
