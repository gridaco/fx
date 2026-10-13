// Param values checked against their declared schemas (src/schema.ts): each keyword, worded as
// spec/protocol.md §6.2's table words it, §6.2's order of errors, exact decimal multipleOf, patterns
// read with and without the u flag, a null for an optional param, and values at param_files
// pointers, which are not checked and compare as their plain projection.

import { describe, expect, test } from "bun:test";
import { NodeFailure } from "../src/engine-errors.js";
import { canonicalJson, checkedParams, isPattern, type Schema, schemaErrors } from "../src/schema.js";

/** The first message `value` gets from `schema`, or null when it meets it. */
function first(value: unknown, schema: Schema): string | null {
  const [error] = schemaErrors(value, schema);
  return error === undefined ? null : `${error.where.join("/") || "the value"}: ${error.message}`;
}

describe("a value checked against a schema", () => {
  test.each([
    // type, a list of types, integer, null
    ["big", { type: "integer" }, 'the value: "big" is not of type "integer"'],
    [1.5, { type: "integer" }, 'the value: 1.5 is not of type "integer"'],
    [2, { type: "integer" }, null],
    [2, { type: "number" }, null],
    [null, { type: "string" }, 'the value: null is not of type "string"'],
    [null, { type: ["string", "null"] }, null],
    [true, { type: ["string", "integer", "null"] }, 'the value: true is not of type "integer" or "null" or "string"'],
    [[1], { type: "object" }, 'the value: [1] is not of type "object"'],
    [{ a: 1 }, { type: "array" }, 'the value: {"a":1} is not of type "array"'],
    // enum, const
    ["triangle", { enum: ["circle", "square"] }, 'the value: "triangle" is not one of ["circle","square"]'],
    [{ b: 1, a: 2 }, { enum: [{ a: 2, b: 1 }] }, null],
    [4, { const: 3 }, "the value: 4 is not 3"],
    // numbers
    [15, { minimum: 16 }, "the value: 15 is less than the minimum 16"],
    [257, { maximum: 256 }, "the value: 257 is greater than the maximum 256"],
    [0, { exclusiveMinimum: 0 }, "the value: 0 is not greater than 0"],
    [1, { exclusiveMaximum: 1 }, "the value: 1 is not less than 1"],
    [7, { multipleOf: 2 }, "the value: 7 is not a multiple of 2"],
    [0.75, { multipleOf: 0.25 }, null],
    // multipleOf exactly in decimal, where a binary quotient is not whole (0.7 / 0.1 = 6.999…)
    [0.7, { multipleOf: 0.1 }, null],
    [0.3, { multipleOf: 0.1 }, null],
    [0.6, { multipleOf: 0.1 }, null],
    [0.15, { multipleOf: 0.05 }, null],
    [1.15, { multipleOf: 0.01 }, null],
    [0.07, { multipleOf: 0.01 }, null],
    [3, { multipleOf: 0.1 }, null],
    [1e-7, { multipleOf: 1e-8 }, null],
    [1.5e21, { multipleOf: 0.5 }, null],
    [0.75, { multipleOf: 0.1 }, "the value: 0.75 is not a multiple of 0.1"],
    [0.30000000000000004, { multipleOf: 0.1 }, "the value: 0.30000000000000004 is not a multiple of 0.1"],
    [-0.7, { multipleOf: 0.1 }, null],
    [0.1, { multipleOf: 0.3 }, "the value: 0.1 is not a multiple of 0.3"],
    // strings, counted in code points
    ["ab", { minLength: 3 }, 'the value: "ab" is shorter than 3 characters'],
    ["abc", { maxLength: 1 }, 'the value: "abc" is longer than 1 character'],
    ["😀😀", { maxLength: 2 }, null],
    ["a1", { pattern: "^[a-z]+$" }, 'the value: "a1" does not match the pattern "^[a-z]+$"'],
    ["x-ray", { pattern: "ray" }, null],
    // Read without the u flag when it needs identity escapes, as the engine reads them.
    ["123-4567", { pattern: "^\\d{3}\\-\\d{4}$" }, null],
    ["not a code", { pattern: "^\\d{3}\\-\\d{4}$" }, 'the value: "not a code" does not match the pattern "^\\\\d{3}\\\\-\\\\d{4}$"'],
    ["é", { pattern: "^\\p{L}$" }, null],
    ["x", { pattern: "(" }, 'the value: the pattern "(" is not a regular expression'],
    // arrays
    [[1, "two"], { items: { type: "integer" } }, '1: "two" is not of type "integer"'],
    [["a", 2, 3], { prefixItems: [{ type: "string" }, { type: "string" }], items: { type: "integer" } }, '1: 2 is not of type "string"'],
    // items: false refuses each item after the prefix, at its place, as the engine does.
    [["a", "b"], { prefixItems: [{ type: "string" }], items: false }, '1: "b" is not allowed'],
    [[1], { items: false }, "0: 1 is not allowed"],
    [["a", 1, 2], { prefixItems: [{ type: "string" }], items: false }, "1: 1 is not allowed"],
    [["a", "b", true], { prefixItems: [{ type: "string" }, { type: "integer" }], items: false }, '1: "b" is not of type "integer"'],
    [["a"], { prefixItems: [{ type: "string" }], items: false }, null],
    [[], { minItems: 1 }, "the value: [] has fewer than 1 item"],
    [[1, 2, 3], { maxItems: 2 }, "the value: [1,2,3] has more than 2 items"],
    [[1, { a: 1 }, { a: 1 }], { uniqueItems: true }, 'the value: [1,{"a":1},{"a":1}] has repeated items'],
    [[1, 2], { uniqueItems: true }, null],
    // objects
    [{ width: "wide" }, { properties: { width: { type: "number" } } }, 'width: "wide" is not of type "number"'],
    [{}, { required: ["width", "label"] }, 'the value: "width" is a required property'],
    [{ a: 1, z: 2, b: 3 }, { properties: { a: {} }, additionalProperties: false }, 'the value: the properties "b", "z" are not allowed'],
    [{ a: 1, b: 3 }, { properties: { a: {} }, additionalProperties: false }, 'the value: the property "b" is not allowed'],
    [{ a: 1, x_1: 2 }, { properties: { a: {} }, patternProperties: { "^x_": { type: "integer" } }, additionalProperties: false }, null],
    [{ line_height: 1.5 }, { patternProperties: { "^[a-z\\_]+$": { type: "number" } }, additionalProperties: false }, null],
    [{ line_height: "x" }, { patternProperties: { "^[a-z\\_]+$": { type: "number" } } }, 'line_height: "x" is not of type "number"'],
    [{ a: 1, b: "x" }, { properties: { a: {} }, additionalProperties: { type: "integer" } }, 'b: "x" is not of type "integer"'],
    [{}, { minProperties: 1 }, "the value: {} has fewer than 1 property"],
    [{ a: 1, b: 2 }, { maxProperties: 1 }, 'the value: {"a":1,"b":2} has more than 1 property'],
    // boolean schemas, and keywords nobody checks
    [{ a: 1 }, { properties: { a: false } }, "a: 1 is not allowed"],
    ["x", true, null],
    ["x", { format: "email", "x-fx-template": true, description: "anything" }, null],
  ] as [unknown, Schema, string | null][])("%j against %j", (value, schema, message) => {
    expect(first(value, schema)).toBe(message);
  });

  test("errors in §6.2's order: by place, a place before the places it starts, then by keyword", () => {
    const schema: Schema = {
      type: "object",
      properties: {
        b: { type: "string", minLength: 5, enum: ["long enough"] },
        a: { type: "array", items: { type: "integer" } },
      },
      required: ["z"],
    };
    const errors = schemaErrors({ b: 3, a: [1, "x", 2, "y"], c: 1 }, schema).map((error) => [
      error.where.join("/"),
      error.keyword.join("/"),
    ]);
    expect(errors).toEqual([
      ["", "required"],
      ["a/1", "properties/a/items/type"],
      ["a/3", "properties/a/items/type"],
      ["b", "properties/b/enum"],
      ["b", "properties/b/type"],
    ]);
    // Array indexes by number: 10 after 9.
    const indexes = schemaErrors([...Array(11).keys()].map(() => "x"), { items: { type: "integer" } });
    expect(indexes.map((error) => error.where[0])).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
  });

  test("canonical JSON: members sorted, numbers as ECMAScript writes them", () => {
    expect(canonicalJson({ b: [1e21, 0.5, -0], a: "é\n", c: undefined })).toBe('{"a":"é\\n","b":[1e+21,0.5,0]}');
  });
});

describe("a run's params", () => {
  const declared = {
    shape: { enum: ["circle", "square"] },
    size: { type: "integer", minimum: 16 },
    note: { type: "string", "x-fx-optional": true },
    label: { type: "string" },
    box: { type: "object", properties: { width: { type: "integer" } } },
    image: { type: "string" },
  };
  const fails = (params: Record<string, unknown>, files: Record<string, string> = {}) => {
    try {
      checkedParams(params, declared, files);
    } catch (error) {
      expect(error).toBeInstanceOf(NodeFailure);
      return (error as Error).message;
    }
    return null;
  };

  test("a value that does not meet its schema fails the node, naming the param and the place", () => {
    expect(fails({ shape: "triangle", size: 16 })).toBe('param shape: "triangle" is not one of ["circle","square"]');
    expect(fails({ shape: "circle", size: "big" })).toBe('param size: "big" is not of type "integer"');
    expect(fails({ shape: "circle", size: 8 })).toBe("param size: 8 is less than the minimum 16");
    expect(fails({ label: null })).toBe('param label: null is not of type "string"');
    expect(fails({ box: { width: "wide" } })).toBe('param box/width: "wide" is not of type "integer"');
    // The params in the order they are declared.
    expect(fails({ label: 1, shape: "triangle" })).toBe('param shape: "triangle" is not one of ["circle","square"]');
  });

  test("a null for an optional param is absent; a param the run does not give stays absent", () => {
    const given = { shape: "circle", note: null, label: "x" };
    const checked = checkedParams(given, declared);
    expect(checked).toEqual({ shape: "circle", label: "x" });
    expect(Object.hasOwn(checked, "note")).toBe(false);
    expect(given.note).toBeNull();
    expect(checkedParams({ note: "kept" }, declared)).toEqual({ note: "kept" });
    expect(fails({ note: 3 })).toBe('param note: 3 is not of type "string"');
  });

  test("a value at a param_files pointer is a file, never checked", () => {
    expect(fails({ image: null }, { "/image": "a".repeat(64) })).toBeNull();
    expect(fails({ image: null })).toBe('param image: null is not of type "string"');
    expect(fails({ box: { width: null } }, { "/box/width": "a".repeat(64) })).toBeNull();
  });

  test("a file compares as its plain projection where the places that hold it are checked", () => {
    const [a, b] = ["a".repeat(64), "b".repeat(64)];
    const refs = { refs: { type: "array", uniqueItems: true }, pinned: { type: "array", const: [{ file: a }] } };
    const check = (params: Record<string, unknown>, files: Record<string, string>) => {
      try {
        return checkedParams(params, refs, files);
      } catch (error) {
        return (error as Error).message;
      }
    };
    // Two different files are two items; the params the body gets keep the wire's nulls.
    expect(check({ refs: [null, null] }, { "/refs/0": a, "/refs/1": b })).toEqual({ refs: [null, null] });
    expect(check({ refs: [null, null] }, { "/refs/0": a, "/refs/1": a })).toBe(
      `param refs: [{"file":"${a}"},{"file":"${a}"}] has repeated items`,
    );
    expect(check({ pinned: [null] }, { "/pinned/0": a })).toEqual({ pinned: [null] });
    expect(check({ pinned: [null] }, { "/pinned/0": b })).toBe(`param pinned: [{"file":"${b}"}] is not [{"file":"${a}"}]`);
  });

  test("a param's own default meets a decimal multipleOf", () => {
    const strength = { strength: { type: "number", minimum: 0, maximum: 1, multipleOf: 0.1, default: 0.7 } };
    for (const value of [0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1]) {
      expect(checkedParams({ strength: value }, strength)).toEqual({ strength: value });
    }
  });

  test("a pattern that is no regular expression, with the u flag or without it", () => {
    expect(isPattern("(")).toBe(false);
    expect(isPattern("^\\d{3}\\-\\d{4}$")).toBe(true);
    expect(isPattern("^[a-z]+$")).toBe(true);
  });
});
