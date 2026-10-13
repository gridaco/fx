// Node type declarations (src/node.ts) and the errors a body sees (src/engine-errors.ts): the
// type spec `describe` reports, and the sentences a declaration is refused with, which are the
// engine's and the Python SDK's (python/tests/test_spec.py), JavaScript's null and booleans aside.

import { describe, expect, test } from "bun:test";
import {
  CallFailed,
  CallRefused,
  Cancelled,
  CapabilityError,
  EngineError,
  engineError,
  isEngineError,
  isNodeFailure,
  NodeFailure,
  UnknownFile,
} from "../src/engine-errors.js";
import { NODE, type NodeDecl, node, nodeRecordOf, paramSchema, parsePort, SpecError } from "../src/node.js";
import { protocolErrors } from "./json-schema.js";

const body = () => ({});

/** Declares `decl` with a body that returns nothing; the declaration error, if any. */
function refusal(decl: Record<string, unknown>): string {
  try {
    node(decl as unknown as NodeDecl, body as never);
  } catch (error) {
    return (error as Error).message;
  }
  return "";
}

describe("a declaration", () => {
  test("a member whose value is undefined is left out, as JSON leaves it out", () => {
    const declared = node(
      {
        name: "k",
        description: undefined,
        inputs: { image: "image", mask: undefined },
        params: { x: { type: "integer", default: undefined }, y: undefined },
        calls: { "image.edit": 1, "image.generate": undefined },
      } as unknown as NodeDecl,
      body as never,
    );
    expect(declared.spec.params).toEqual({ x: { type: "integer" } });
    expect(Object.keys(declared.spec.params.x ?? {})).toEqual(["type"]);
    expect(Object.keys(declared.spec.inputs)).toEqual(["image"]);
    expect(Object.keys(declared.spec.calls)).toEqual(["image.edit"]);
    expect(Object.hasOwn(declared.spec, "description")).toBe(false);
  });

  test("a member whose value is undefined is left out at any depth of a param schema", () => {
    const declared = node(
      {
        name: "k",
        params: {
          p: {
            type: "object",
            properties: { a: { type: "string", const: undefined } },
            prefixItems: [{ type: "string", enum: undefined }],
          },
        },
      } as unknown as NodeDecl,
      body as never,
    );
    const p = declared.spec.params.p as Record<string, Record<string, Record<string, unknown>>>;
    expect(declared.spec.params).toEqual({
      p: { type: "object", properties: { a: { type: "string" } }, prefixItems: [{ type: "string" }] },
    });
    expect(Object.hasOwn(p.properties?.a ?? {}, "const")).toBe(false);
  });

  test("a pattern is read with the u flag, else without it; one that is neither is refused", () => {
    const declared = node(
      {
        name: "k",
        params: {
          code: { type: "string", pattern: "^\\d{3}\\-\\d{4}$" },
          styles: { type: "object", patternProperties: { "^[a-z\\_]+$": { type: "number" } } },
          letters: { type: "string", pattern: "^\\p{L}+$" },
        },
      } as unknown as NodeDecl,
      body as never,
    );
    expect(Object.keys(declared.spec.params)).toEqual(["code", "styles", "letters"]);
    expect(refusal({ name: "k", params: { code: { type: "string", pattern: "(" } } })).toBe(
      "k: param 'code': the pattern '(' is not a regular expression",
    );
    expect(
      refusal({
        name: "k",
        params: { box: { type: "object", properties: { tags: { type: "array", items: { patternProperties: { "[": {} } } } } } },
      }),
    ).toBe("k: param 'box': the pattern '[' is not a regular expression");
  });

  test("becomes the type spec describe reports, in the protocol's key order", () => {
    const repaint = node(
      {
        name: "image.repaint",
        description: "Repaints a seam.",
        inputs: { image: "image", refs: "image[]", lines: "audio{}", spec: "json?" },
        params: {
          opaque: "boolean",
          strength: "number",
          labels: "array",
          options: "object",
          count: "integer",
          name: "string",
          mode: ["fast", "slow"],
          steps: { type: "integer", minimum: 1, maximum: 8, default: 2 },
          prompt: { type: "string", template: true },
          note: { type: "string", optional: true },
          schema: { type: "object", "x-fx-optional": true },
        },
        outputs: { image: "image/png", report: "json", extra: "file/zip[]?" },
        calls: { "image.edit": 1, "agent.turn": "steps" },
        resources: ["prompts/seam.md", ".config/x.json", "..data/y"],
        tools: ["ffmpeg", "blender>=4.2", "magick ~= 7"],
        view: "views/repaint.html",
        version: 3,
      },
      () => ({ image: null as never, report: null as never }),
    );
    expect(Object.keys(repaint.spec)).toEqual([
      "name",
      "description",
      "inputs",
      "params",
      "outputs",
      "judge",
      "calls",
      "resources",
      "tools",
      "view",
      "version",
      "retry",
    ]);
    expect(protocolErrors(repaint.spec, "type_spec")).toEqual([]);
    expect(repaint.spec.params).toEqual({
      opaque: { type: "boolean" },
      strength: { type: "number" },
      labels: { type: "array" },
      options: { type: "object" },
      count: { type: "integer" },
      name: { type: "string" },
      mode: { enum: ["fast", "slow"] },
      steps: { type: "integer", minimum: 1, maximum: 8, default: 2 },
      prompt: { type: "string", "x-fx-template": true },
      note: { type: "string", "x-fx-optional": true },
      schema: { type: "object", "x-fx-optional": true },
    });
    expect(repaint.spec.inputs).toEqual({ image: "image", refs: "image[]", lines: "audio{}", spec: "json?" });
    expect(repaint.spec.retry).toBe("service");
    expect(repaint.name).toBe("image.repaint");
    expect(Object.isFrozen(repaint.spec)).toBe(true);
  });

  test("the defaults, and a blank description left out", () => {
    const echo = node({ name: "echo", description: "  \n" }, body);
    expect(echo.spec).toEqual({
      name: "echo",
      inputs: {},
      params: {},
      outputs: {},
      judge: false,
      calls: {},
      resources: [],
      tools: [],
      view: null,
      version: null,
      retry: "service",
    });
  });

  test("carries the brand the host reads, with the body", () => {
    const echo = node({ name: "echo" }, body);
    const record = nodeRecordOf(echo);
    expect(record?.body).toBe(body);
    expect(record?.spec).toBe(echo.spec);
    expect((echo as unknown as Record<symbol, unknown>)[Symbol.for("grida.fx.node")]).toBe(record);
    expect(NODE).toBe(Symbol.for("grida.fx.node") as typeof NODE);
    // Another copy's node type, by its brand alone.
    const foreign = { [Symbol.for("grida.fx.node")]: { spec: echo.spec, body } };
    expect(nodeRecordOf(foreign)?.body).toBe(body);
    expect(nodeRecordOf({})).toBeUndefined();
    expect(nodeRecordOf(null)).toBeUndefined();
    expect(nodeRecordOf(() => 1)).toBeUndefined();
  });

  test("param shorthand and the predecessor's spelling", () => {
    expect(paramSchema({ type: "string", optional: true, template: false })).toEqual({
      type: "string",
      "x-fx-optional": true,
      "x-fx-template": false,
    });
    expect(paramSchema({ optional: "maybe" })).toEqual({ optional: "maybe" });
    expect(paramSchema({ optional: true, "x-fx-optional": true })).toEqual({ "x-fx-optional": true });
    expect(() => paramSchema({ optional: true, "x-fx-optional": false })).toThrow(
      new SpecError("param schema sets both optional and x-fx-optional"),
    );
    const original = { type: "string", optional: true };
    paramSchema(original);
    expect(original).toEqual({ type: "string", optional: true });
    for (const declared of [null, undefined, 3, "str", [String], new Set(), () => 1]) {
      if (Array.isArray(declared)) {
        continue; // a list is choices
      }
      expect(() => paramSchema(declared)).toThrow("is a type name, a list of choices or a schema");
    }
  });

  test.each([
    ["image", { kind: "image", shape: "one", optional: false }],
    ["image/png", { kind: "image/png", shape: "one", optional: false }],
    ["image[]", { kind: "image", shape: "list", optional: false }],
    ["text{}", { kind: "text", shape: "keyed", optional: false }],
    ["json?", { kind: "json", shape: "one", optional: true }],
    ["audio[]?", { kind: "audio", shape: "list", optional: true }],
    ["model/gltf+json{}?", { kind: "model/gltf+json", shape: "keyed", optional: true }],
    ["annotations", { kind: "annotations", shape: "one", optional: false }],
    ["file/x-a.b-c", { kind: "file/x-a.b-c", shape: "one", optional: false }],
  ])("the port %s", (notation, port) => {
    expect(parsePort(notation)).toEqual(port as never);
  });

  test.each(["image?[]", "Image", "image/", "image[]{}", "", " image"])("the port %p is refused", (notation) => {
    expect(() => parsePort(notation)).toThrow(
      new SpecError(`port '${notation}' is not kind, kind[], kind{} with an optional ?`),
    );
  });

  test("a port of an unknown family is refused", () => {
    expect(() => parsePort("picture/png")).toThrow(
      "port kind 'picture/png' is not one of ['annotations', 'audio', 'file', 'image', 'json', 'model', 'text', 'video']",
    );
  });

  test.each([
    [{ name: "Kind" }, "node type name 'Kind' must be lower_snake words joined by ."],
    [{ name: "a..b" }, "node type name 'a..b' must be lower_snake words joined by ."],
    [{ name: "a." }, "node type name 'a.' must be lower_snake words joined by ."],
    [{ name: "1a" }, "node type name '1a' must be lower_snake words joined by ."],
    [{ name: 3 }, "node type name 3 must be lower_snake words joined by ."],
    [{ name: "k", inputs: { x: "image" }, params: { x: "string", a: "integer" } }, "k: ['x'] declared as both input and param"],
    [
      { name: "k", calls: { "agent.turn": "steps" }, params: { steps: "integer" } },
      "k: calls['agent.turn'] names 'steps', which is not an integer setting with a minimum of at least 1",
    ],
    [
      { name: "k", calls: { "agent.turn": "steps" }, params: { steps: { type: "integer", minimum: 0 } } },
      "k: calls['agent.turn'] names 'steps', which is not an integer setting with a minimum of at least 1",
    ],
    [
      { name: "k", calls: { "agent.turn": "steps" }, params: { steps: { type: "string", minimum: 1 } } },
      "k: calls['agent.turn'] names 'steps', which is not an integer setting with a minimum of at least 1",
    ],
    [
      { name: "k", calls: { "agent.turn": "missing" } },
      "k: calls['agent.turn'] names 'missing', which is not an integer setting with a minimum of at least 1",
    ],
    [
      { name: "k", calls: { "agent.turn": "steps" }, params: { steps: { type: "integer", minimum: 1 } } },
      "k: calls['agent.turn'] needs 'steps' to set a maximum",
    ],
    [{ name: "k", calls: { "image.edit": 0 } }, "k: calls['image.edit'] must be at least 1"],
    [{ name: "k", calls: { "image.edit": -2 } }, "k: calls['image.edit'] must be at least 1"],
    [
      { name: "k", calls: { "image.edit": true } },
      "k: calls['image.edit'] is a whole number of at least 1 or the name of an integer param",
    ],
    [
      { name: "k", calls: { "image.edit": 1.5 } },
      "k: calls['image.edit'] is a whole number of at least 1 or the name of an integer param",
    ],
    [{ name: "k", calls: { "": 1 } }, "k: calls are keyed by capability names"],
    [{ name: "k", calls: 3 }, "k: calls map capabilities to bounds"],
    [{ name: "k", params: ["a"] }, "k: params map names to declarations"],
    [{ name: "k", inputs: "image" }, "k: inputs map names to port notation"],
    [
      { name: "k", inputs: { Image: "image" } },
      "k: input name 'Image' must be one lower_snake word (a letter, then letters, digits or _)",
    ],
    [
      { name: "k", params: { "a-b": "string" } },
      "k: param name 'a-b' must be one lower_snake word (a letter, then letters, digits or _)",
    ],
    [
      { name: "k", outputs: { _x: "text" } },
      "k: output name '_x' must be one lower_snake word (a letter, then letters, digits or _)",
    ],
    [{ name: "k", inputs: { x: "imag" } }, "port kind 'imag' is not one of"],
    [{ name: "k", outputs: { x: "text??" } }, "port 'text??' is not kind"],
    [{ name: "k", outputs: { x: 5 } }, "port 5 is not kind"],
    [{ name: "k", params: { x: { "x-fx-optional": "yes" } } }, "k: param 'x': x-fx-optional must be true or false"],
    [
      { name: "k", params: { x: { default: Number.NaN } } },
      "k: param 'x' is not a JSON value: at /default: NaN is not a JSON number",
    ],
    [
      { name: "k", params: { x: { default: 2 ** 60 } } },
      "k: param 'x' is not a JSON value: at /default: the integer 1152921504606846976 cannot be read exactly",
    ],
    [
      { name: "k", params: { x: { enum: [1, undefined] } } },
      "k: param 'x' is not a JSON value: at /enum/1: undefined is not a JSON value",
    ],
    [{ name: "k", params: { x: 5 } }, "param declaration 5 is a type name, a list of choices or a schema"],
    [{ name: "k", params: { x: "str" } }, "param declaration 'str' is a type name, a list of choices or a schema"],
    [
      { name: "k", resources: ["/abs/path.md"] },
      "k: resource '/abs/path.md' must be a POSIX path relative to the project root",
    ],
    [{ name: "k", resources: ["../up.md"] }, "k: resource '../up.md' must be"],
    [{ name: "k", resources: ["./here.md"] }, "k: resource './here.md' must be"],
    [{ name: "k", resources: ["a//b.md"] }, "k: resource 'a//b.md' must be"],
    [{ name: "k", resources: ["a/b/"] }, "k: resource 'a/b/' must be"],
    [{ name: "k", resources: ["a\\b.md"] }, "k: resource 'a\\\\b.md' must be"],
    [{ name: "k", resources: ["a/../b.md"] }, "k: resource 'a/../b.md' must be"],
    [{ name: "k", resources: ["r.md", "r.md"] }, "k: resource 'r.md' is declared twice"],
    [{ name: "k", resources: "r.md" }, "k: resources is a list of strings, not one string"],
    [{ name: "k", resources: 3 }, "k: resources are a list of strings"],
    [
      { name: "k", tools: ["Blender"] },
      "k: tool 'Blender' must be a program name, optionally followed by a version bound",
    ],
    [{ name: "k", tools: ["blender 4.2"] }, "k: tool 'blender 4.2' must be"],
    [{ name: "k", tools: ["-x"] }, "k: tool '-x' must be"],
    [{ name: "k", tools: ["ffmpeg", "ffmpeg"] }, "k: tool 'ffmpeg' is declared twice"],
    [{ name: "k", tools: "ffmpeg" }, "k: tools is a list of strings, not one string"],
    [{ name: "k", version: -1 }, "k: version must be a whole number of at least 0, or null"],
    [{ name: "k", version: true }, "k: version must be a whole number of at least 0, or null"],
    [{ name: "k", version: 1.5 }, "k: version must be a whole number of at least 0, or null"],
    [{ name: "k", retry: "always" }, "k: retry is 'service' or 'engine', not 'always'"],
    [{ name: "k", view: 3 }, "k: view must be a string or null"],
    [{ name: "k", judge: 1 }, "k: judge must be true or false"],
    [{ name: "k", description: 3 }, "k: description must be a string or null"],
    [{ name: "k", input: {} }, "k: a node declaration has no field 'input'"],
  ])("refused: %#", (decl, message) => {
    expect(refusal(decl)).toStartWith(message);
  });

  test("a declaration that is not an object, or a body that is not a function, is refused", () => {
    const untyped = node as (decl: unknown, body: unknown) => unknown;
    expect(() => untyped(null, body)).toThrow(new TypeError("a node declaration is an object, not null"));
    expect(() => untyped({ name: "k" }, "body")).toThrow(
      new TypeError("a node body is a function, not a string"),
    );
    try {
      node({ name: "Bad" }, body);
    } catch (error) {
      expect((error as Error).name).toBe("SpecError");
    }
  });
});

describe("errors a body sees", () => {
  test("an engine error's class comes from its code", () => {
    const failed = engineError(-32016, "every attempt failed", { capability: "image.edit" });
    expect(failed).toBeInstanceOf(CallFailed);
    expect(failed).toBeInstanceOf(CapabilityError);
    expect(failed).toBeInstanceOf(EngineError);
    expect(failed).toBeInstanceOf(Error);
    expect(failed.name).toBe("CallFailed");
    expect(failed.code).toBe(-32016);
    expect(failed.message).toBe("every attempt failed");
    expect(failed.data).toEqual({ capability: "image.edit" });
    expect(engineError(-32002, "stopped")).toBeInstanceOf(Cancelled);
    expect(engineError(-32024, "no such file")).toBeInstanceOf(UnknownFile);
    const other = engineError(-32050, "something else");
    expect(other.constructor).toBe(EngineError);
    expect(other.code).toBe(-32050);
    expect(other.data).toBeNull();
    expect(new CallRefused("no").code).toBe(-32015);
    expect(new EngineError("plain").code).toBe(-32099);
    expect(new CallRefused("no")).not.toBeInstanceOf(CallFailed);
  });

  test("another copy's errors are recognised by their brands", () => {
    const foreignFailure = Object.defineProperty(new Error("on purpose"), Symbol.for("grida.fx.node_failure"), {
      value: true,
    });
    expect(isNodeFailure(foreignFailure)).toBe(true);
    expect(foreignFailure instanceof NodeFailure).toBe(true);
    const foreignEngine = Object.assign(new Error("every attempt failed"), { code: -32016, data: null });
    Object.defineProperty(foreignEngine, Symbol.for("grida.fx.engine_error"), { value: true });
    expect(isEngineError(foreignEngine)).toBe(true);
    expect(foreignEngine instanceof EngineError).toBe(true);
    expect(foreignEngine instanceof CapabilityError).toBe(true);
    expect(foreignEngine instanceof CallFailed).toBe(true);
    expect(foreignEngine instanceof CallRefused).toBe(false);
    expect(new Error("x") instanceof NodeFailure).toBe(false);
    expect(isNodeFailure(new NodeFailure("x"))).toBe(true);
    expect(new NodeFailure("x").name).toBe("NodeFailure");
  });
});
