import { expect, test } from "bun:test";
import { scopeState } from "../src/scope-state";

test("scope summaries keep static uncertainty and runtime failure truthful", () => {
  expect(scopeState(["planned", "absent"], true)).toBe("planned");
  expect(scopeState(["done", "maybe"], true)).toBe("maybe");
  expect(scopeState(["done", "unexpanded"], true)).toBe("unexpanded");
  expect(scopeState(["done", "absent"], true)).toBe("done");
  expect(scopeState(["failed", "planned"], true)).toBe("failed");
  expect(scopeState(["running", "failed"], false)).toBe("running");
  expect(scopeState(["succeeded", "failed"], false)).toBe("failed");
  expect(scopeState(["succeeded", "skipped"], false)).toBe("succeeded");
  expect(scopeState(["skipped", "absent"], false)).toBe("skipped");
  expect(scopeState(["succeeded", "unknown"], false)).toBe("unknown");
  expect(scopeState([], false)).toBe("empty");
});
