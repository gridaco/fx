import { expect, test } from "bun:test";
import { version } from "./index.ts";

test("the package names its version", () => {
  expect(version).toMatch(/^\d+\.\d+\.\d+/);
});
