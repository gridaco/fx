import { expect, test } from "bun:test";
import { stateIcon, stateIconParts } from "../src/state-icon";

test("every state takes one glyph in the shared circle slot", () => {
  expect(["succeeded", "cached", "done", "failed", "blocked", "running", "cancelling", "interrupted", "skipped", "maybe", "unexpanded", "pending", "planned", "unknown"].map(stateIcon))
    .toEqual(["succeeded", "succeeded", "succeeded", "failed", "failed", "running", "cancelled", "cancelled", "skipped", "maybe", "maybe", "pending", "pending", "pending"]);
  expect(stateIconParts("running").map((part) => part.attributes.class)).toEqual(["fx-state-track", "fx-state-arc", "fx-state-core"]);
  expect(stateIconParts("pending").map((part) => part.attributes.class)).toEqual(["fx-state-ring"]);
  // Waiting for a route's slot is still: a clock, never the spinning glyph.
  expect(stateIcon("waiting")).toBe("waiting");
  expect(stateIconParts("waiting").map((part) => part.attributes.class)).toEqual(["fx-state-ring", "fx-state-hands"]);
});
