import { expect, test } from "bun:test";
import { thumbnailCapacity, thumbnailFailureEvictions } from "../src/card-models";

test("thumbnail capacity preserves visible stills and resumes when an old still leaves view", () => {
  expect(thumbnailCapacity(["a"], new Set(["a"]), 2)).toEqual({ ready: true });
  expect(thumbnailCapacity(["a", "b"], new Set(["a", "b"]), 2)).toEqual({ ready: false });
  expect(thumbnailCapacity(["a", "b"], new Set(["b"]), 2)).toEqual({ ready: true, evict: "a" });
  // After the oldest offscreen entry is replaced, another visible entry cannot be evicted.
  expect(thumbnailCapacity(["b", "c"], new Set(["b", "c"]), 2)).toEqual({ ready: false });
  expect(thumbnailCapacity(["b", "c"], new Set(["c"]), 2)).toEqual({ ready: true, evict: "b" });
});

test("more than 64 current failed models do not lose their failure markers and repeat work", () => {
  const failures = Array.from({ length: 80 }, (_, index) => `current-${index}`);
  const current = new Set(failures);
  expect(thumbnailFailureEvictions(failures, current)).toEqual([]);
  const historical = Array.from({ length: 70 }, (_, index) => `old-${index}`);
  expect(thumbnailFailureEvictions([...historical, ...failures], current)).toEqual(historical.slice(0, 6));
  // Navigating away bounds old diagnostic metadata while a fresh graph keeps all its markers.
  expect(thumbnailFailureEvictions(failures, new Set())).toEqual(failures.slice(0, 16));
});
