// The node protocol's transport (src/protocol.ts): framing, strict reading and the I-JSON check,
// with the vectors the Python host's transport is held to (python/tests/test_host.py).

import { describe, expect, test } from "bun:test";
import {
  checkValue,
  encodableText,
  FrameReader,
  frame,
  ProtocolError,
  parseMessage,
  pyRepr,
  typeName,
} from "../src/protocol.js";

const framed = (body: string | Buffer) => {
  const bytes = typeof body === "string" ? Buffer.from(body, "utf8") : body;
  return Buffer.concat([Buffer.from(`Content-Length: ${bytes.length}\r\n\r\n`), bytes]);
};

/** Every body `data` holds, then the end of the stream. */
function read(data: Buffer, chunk = data.length): Buffer[] {
  const reader = new FrameReader();
  const bodies: Buffer[] = [];
  for (let at = 0; at < data.length; at += chunk) {
    bodies.push(...reader.push(data.subarray(at, at + chunk)));
  }
  if (reader.broken !== null) {
    throw reader.broken;
  }
  reader.end();
  return bodies;
}

describe("framing", () => {
  test("frames are read whole, other header fields ignored, LF-only lines accepted", () => {
    const data = Buffer.concat([
      framed('{"a":1}'),
      Buffer.from("content-length:  2 \nX: y\r\n\r\n[]"),
      framed('"é"'),
    ]);
    const expected = ['{"a":1}', "[]", '"é"'];
    expect(read(data).map((body) => body.toString("utf8"))).toEqual(expected);
    // Byte by byte, as a pipe may deliver them.
    expect(read(data, 1).map((body) => body.toString("utf8"))).toEqual(expected);
    expect(read(Buffer.alloc(0))).toEqual([]);
  });

  test.each([
    ["Content-Length: 5\r\n\r\n{}", "the stream ended after 2 of a message's 5 bytes"],
    ["Content-Length: 2\r\n", "the stream ended inside a message header"],
    ["Content-Length", "the stream ended inside a message header"],
    ["X-Only: 1\r\n\r\n{}", "a message header has no Content-Length"],
    ["Content-Length: -2\r\n\r\n", "Content-Length is not a byte count: b' -2'"],
    ["Content-Length: 1_0\r\n\r\n", "Content-Length is not a byte count: b' 1_0'"],
    ["Content-Length: 2\r\nContent-Length: 3\r\n\r\n{}", "a message header gives two different lengths"],
    ["no colon here\r\n\r\n", "a message header is not a field: b'no colon here'"],
    [`Content-Length: ${"9".repeat(10000)}\r\n\r\n`, "a message header line is too long"],
  ])("a broken frame breaks the stream: %#", (data, message) => {
    expect(() => read(Buffer.from(data))).toThrow(new ProtocolError(message));
  });

  test("the frames before a broken one are read; nothing after it", () => {
    const reader = new FrameReader();
    const bodies = reader.push(Buffer.concat([framed("[1]"), Buffer.from("no colon\r\n\r\n"), framed("[2]")]));
    expect(bodies.map((body) => body.toString())).toEqual(["[1]"]);
    expect(reader.broken).toEqual(new ProtocolError("a message header is not a field: b'no colon'"));
    expect(reader.push(framed("[3]"))).toEqual([]);
    reader.end();
  });

  test("a message is one frame of compact UTF-8 JSON", () => {
    const body = '{"jsonrpc":"2.0","id":1,"result":{"text":"é","n":0.5}}';
    expect(frame({ jsonrpc: "2.0", id: 1, result: { text: "é", n: 0.5 } })).toEqual(
      Buffer.concat([Buffer.from(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n`), Buffer.from(body)]),
    );
  });

  test("nothing outside I-JSON is written", () => {
    for (const value of [Number.NaN, Number.POSITIVE_INFINITY, new Map([["a", 1]]), "\ud800", 2 ** 53 + 2, [undefined], 1n]) {
      expect(() => frame({ result: value })).toThrow(TypeError);
    }
  });

  test("an object member whose value is undefined is left out, as JSON.stringify leaves it out", () => {
    const body = '{"jsonrpc":"2.0","id":1,"result":{"kept":[1,{}]}}';
    expect(frame({ jsonrpc: "2.0", id: 1, result: { kept: [1, { gone: undefined }], gone: undefined } })).toEqual(
      Buffer.concat([Buffer.from(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n`), Buffer.from(body)]),
    );
  });

  test("a large frame in many chunks is read in time linear in its size", () => {
    const size = 16 * 1024 * 1024;
    const body = Buffer.alloc(size, 0x20);
    body[0] = 0x5b; // [
    body[size - 1] = 0x5d; // ]
    const data = Buffer.concat([framed(body), framed("[1]")]);
    const reader = new FrameReader();
    const bodies: Buffer[] = [];
    const started = performance.now();
    for (let at = 0; at < data.length; at += 1024) {
      bodies.push(...reader.push(data.subarray(at, at + 1024)));
    }
    const took = performance.now() - started;
    reader.end();
    expect(bodies.map((item) => item.length)).toEqual([size, 3]);
    expect(bodies[0]?.equals(body)).toBe(true);
    // Joining the pending bytes at every chunk copies the frame about 8000 times over (seconds);
    // joining them once takes milliseconds.
    expect(took).toBeLessThan(1500);
  });

  test("a lone surrogate in a message's text is written as its escape", () => {
    expect(encodableText("no file gull\udcff.png")).toBe("no file gull\\udcff.png");
    expect(encodableText("😀 stays")).toBe("😀 stays");
  });
});

describe("reading a message", () => {
  test.each([
    ["NaN", "the message holds NaN, which is not a JSON number"],
    ["[Infinity]", "the message holds Infinity, which is not a JSON number"],
    ['{"a": -Infinity}', "the message holds -Infinity, which is not a JSON number"],
    ["1e400", "the number 1e400 is too large"],
    ["9007199254740993", "the integer 9007199254740993 cannot be read without rounding"],
    ["1152921504606846976", "the integer 1152921504606846976 cannot be read without rounding"],
    [`1${"0".repeat(5000)}`, `the integer 1${"0".repeat(39)} cannot be read without rounding`],
    ['"\\ud800"', "the message holds a lone surrogate"],
    ['{"\\udc00": 1}', "the message holds a lone surrogate"],
    ['{"a": 1, "\\u0061": 2}', "the message repeats the key 'a' in one object"],
    ["[1,]", null],
    ["[".repeat(100000), "the message nests too deeply"],
    ['{"a" 1}', null],
    ["[1] 2", null],
    ['"tab\there"', null],
  ])("refused: %#", (body, message) => {
    const run = () => parseMessage(Buffer.from(body, "utf8"));
    expect(run).toThrow(ProtocolError);
    if (message !== null) {
      expect(run).toThrow(message);
    } else {
      expect(run).toThrow(/^the message is not JSON: /);
    }
  });

  test("bytes that are not UTF-8, or a byte-order mark, are refused", () => {
    expect(() => parseMessage(Buffer.from([0xff]))).toThrow(
      "the message is not UTF-8: byte 0xff at position 0 starts no character",
    );
    expect(() => parseMessage(Buffer.from([0x22, 0xed, 0xa0, 0x80, 0x22]))).toThrow(/^the message is not UTF-8: /);
    expect(() => parseMessage(Buffer.from("﻿{}", "utf8"))).toThrow(/^the message is not JSON: /);
  });

  test("the edges are read", () => {
    expect(parseMessage(Buffer.from("[9007199254740992, -9007199254740992, 10000000000000000, -0]"))).toEqual([
      2 ** 53,
      -(2 ** 53),
      10 ** 16,
      0,
    ]);
    expect(parseMessage(Buffer.from('"\\ud83d\\ude00"'))).toBe("😀");
    expect(parseMessage(Buffer.from('{"a": 1.5e3, "b": [true, false, null], "c": "\\"\\\\\\/\\b\\f\\n\\r\\t"}'))).toEqual({
      a: 1500,
      b: [true, false, null],
      c: '"\\/\b\f\n\r\t',
    });
    // A key named like a prototype stays a key.
    const read = parseMessage(Buffer.from('{"__proto__": {"x": 1}}')) as Record<string, unknown>;
    expect(Object.keys(read)).toEqual(["__proto__"]);
    expect(Object.getPrototypeOf(read)).toBe(Object.prototype);
  });
});

describe("the I-JSON check", () => {
  const nested = (levels: number): unknown[] => {
    let value: unknown[] = [];
    for (let level = 1; level < levels; level += 1) {
      value = [value];
    }
    return value;
  };

  test("it names where", () => {
    checkValue({ a: [1, "b", null, true, 0.5, { c: [1, 2] }] });
    expect(() => checkValue({ a: [0, { "b/c": Number.NaN }] })).toThrow(
      new TypeError("at /a/1/b~1c: NaN is not a JSON number"),
    );
    expect(() => checkValue({ "a~": Number.NEGATIVE_INFINITY })).toThrow("at /a~0: -Infinity is not a JSON number");
    const looped: Record<string, unknown> = {};
    looped.self = looped;
    expect(() => checkValue(looped)).toThrow(new TypeError("at /self: the value holds itself"));
    const shared = [1];
    checkValue({ a: shared, b: [shared, [shared]] });
  });

  test("undefined is refused as a value and as an array item; an object member holding it is left out", () => {
    expect(() => checkValue(undefined)).toThrow(new TypeError("undefined is not a JSON value"));
    expect(() => checkValue({ a: [1, undefined] })).toThrow(new TypeError("at /a/1: undefined is not a JSON value"));
    checkValue({ a: undefined, b: { c: undefined } });
  });

  test.each([
    [() => 1, "a function is not a JSON value"],
    [Symbol("s"), "a symbol is not a JSON value"],
    [1n, "a bigint is not a JSON value"],
    [new Map(), "a Map is not a JSON value"],
    [new Set(), "a Set is not a JSON value"],
    [new Date(0), "a Date is not a JSON value"],
    [new (class Item {})(), "an Item is not a JSON value"],
    [new Uint8Array(1), "a Uint8Array is not a JSON value"],
    [2 ** 53 + 2, "the integer 9007199254740994 cannot be read exactly"],
    [-(2 ** 60), "the integer -1152921504606846976 cannot be read exactly"],
    [1e300, `the integer of ${BigInt(1e300).toString(2).length} bits cannot be read exactly`],
    ["lone \udc00", "a string holds a lone surrogate"],
  ])("refused: %#", (value, message) => {
    expect(() => checkValue({ value })).toThrow(new TypeError(`at /value: ${message}`));
  });

  test("an array's hole and a key with a lone surrogate are refused; an object without a prototype is plain", () => {
    expect(() => checkValue([1, , 3])).toThrow("at /1: undefined is not a JSON value");
    expect(() => checkValue({ "\ud800": 1 })).toThrow("a key holds a lone surrogate");
    const bare = Object.create(null) as Record<string, unknown>;
    bare.a = 1;
    checkValue(bare);
    checkValue(2 ** 53);
    checkValue(-(2 ** 53));
  });

  test("nothing nests deeper than the engine reads", () => {
    checkValue(nested(513)); // the innermost array, empty, sits inside 512
    checkValue([nested(512)]);
    expect(() => checkValue(nested(514))).toThrow(
      /^at \/0\/0\/0\/0\/…: the value nests deeper than the 512 levels the engine reads$/,
    );
    expect(() => checkValue([[1]], "", 511)).toThrow("the value nests deeper than the 512 levels");
    checkValue([[]], "", 511);
    // A request's params sit one level inside the message.
    checkValue({ value: nested(511) }, "", 1);
    expect(() => checkValue({ value: nested(512) }, "", 1)).toThrow("nests deeper");
  });

  test("names in messages", () => {
    expect(typeName("x")).toBe("a string");
    expect(typeName([])).toBe("an array");
    expect(typeName({})).toBe("an object");
    expect(typeName(null)).toBe("null");
    expect(pyRepr("Bad")).toBe("'Bad'");
    expect(pyRepr("it's")).toBe('"it\'s"');
    expect(pyRepr(["b", "c"])).toBe("['b', 'c']");
    expect(pyRepr("a\nb\u0001")).toBe("'a\\nb\\x01'");
  });
});
