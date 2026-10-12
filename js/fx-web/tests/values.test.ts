import { describe, expect, test } from "bun:test";
import { jsonShape, jsonSize, parseJson, plainJson } from "../src/values";

describe("plainJson", () => {
  test("unwraps value and list encodings into ordinary JSON", () => {
    expect(plainJson({ value: { a: 1 } })).toEqual({ a: 1 });
    expect(plainJson({ list: [{ value: 1 }, "b", { list: [true] }] })).toEqual([1, "b", [true]]);
    expect(plainJson({ id: "idle", frames: 6 })).toEqual({ id: "idle", frames: 6 });
    expect(plainJson(3)).toBe(3);
  });

  test("leaves files, collections and markers to the viewer", () => {
    for (const marked of [{ file: { digest: "a" } }, { collection: [] }, { none: true }, { missing: true }, { pending: ["a#1"] }, { failed: "a#1" }]) {
      expect(plainJson(marked)).toBeUndefined();
      expect(plainJson({ list: ["x", marked] })).toBeUndefined();
      // A member at any depth: an object of files is not plain JSON.
      expect(plainJson({ "pet.json": marked, manifest: { id: "a" } })).toBeUndefined();
    }
  });
});

test("parseJson reads a JSON file's text, or nothing", () => {
  expect(parseJson('{"id": "pebble"}')).toEqual({ id: "pebble" });
  expect(parseJson("{not json")).toBeUndefined();
});

describe("jsonShape", () => {
  test("reads a value by its shape", () => {
    expect(jsonShape("text")).toBe("scalar");
    expect(jsonShape(null)).toBe("scalar");
    expect(jsonShape([280, 110, 320])).toBe("line");
    expect(jsonShape({ id: "idle", frames: 6 })).toBe("fields");
    expect(jsonShape([{ id: "idle", frames: 6 }])).toBe("folded");
    expect(jsonShape({ durations: [1, 2] })).toBe("folded");
    expect(jsonShape(Array.from({ length: 17 }, (_, i) => i))).toBe("folded");
  });

  test("folded JSON names its size", () => {
    expect(jsonSize([1])).toBe("1 item");
    expect(jsonSize([{}, {}])).toBe("2 items");
    expect(jsonSize({ a: [1], b: {} })).toBe("2 fields");
  });
});
