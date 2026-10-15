// What the SDK's stand-in answerers share (src/stand-in.ts; spec/protocol.md §5.7): the call, the
// answer's encoding, and the Answerer driven without an engine. The cases of
// python/tests/test_stand_in.py that apply to TypeScript stand-ins.

import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { CallFailed, CallRefused, Cancelled } from "../src/engine-errors.js";
import { AdapterFile, InputFile } from "../src/files.js";
import { frame, Refusal } from "../src/protocol.js";
import {
  Answer,
  Answerer,
  DECLINE,
  encode,
  initializeResult,
  isDecline,
  type Reply,
  type StandIn,
  type StandInCall,
  sdkVersion,
} from "../src/stand-in.js";
import { protocolErrors } from "./json-schema.js";
import { packageRoot } from "./support.js";

const folder = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-stand-in-")));
afterAll(() => rmSync(folder, { recursive: true, force: true }));

const KEY = "a".repeat(64);
const DIGEST = "d".repeat(64);
const b64 = (data: string) => Buffer.from(data, "utf8").toString("base64");

function params(changes: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    capability: "image.generate",
    route: { id: "img-a@acme", fingerprint: "e".repeat(64) },
    request: { prompt: "a lighthouse" },
    take: [1],
    key: KEY,
    instance: { id: "draw#1", path: "draw", step: "draw" },
    files: {},
    ...changes,
  };
}

/** The call the answerer hands the stand-in for `given`, or the error it answers. */
async function callFor(given: unknown): Promise<StandInCall | Reply> {
  let seen: StandInCall | null = null;
  const answerer = new Answerer((call) => {
    seen = call;
    return DECLINE;
  });
  const replies = await answerAll(answerer, [[1, given]]);
  return seen ?? (replies[0]?.[1] as Reply);
}

/** Starts every `[id, params]` (cancelling `cancel` right after) and returns the replies in the
 * order they came, once there is one for each. */
async function answerAll(
  answerer: Answerer,
  asks: readonly (readonly [number | string, unknown])[],
  cancel?: number | string,
): Promise<[number | string, Reply][]> {
  const replies: [number | string, Reply][] = [];
  await new Promise<void>((done, fail) => {
    const timer = setTimeout(() => fail(new Error(`only ${replies.length} replies`)), 10_000);
    for (const [id, given] of asks) {
      answerer.start(id, given, (reply) => {
        replies.push([id, reply]);
        if (replies.length === asks.length) {
          clearTimeout(timer);
          done();
        }
      });
    }
    if (cancel !== undefined) {
      answerer.cancel(cancel);
    }
  });
  return replies;
}

// --- the call ---------------------------------------------------------------------------------

test.each([
  [{ capability: 3 }, "stand_in.answer's capability is a capability's name"],
  [{ route: { id: "img-a@acme" } }, "stand_in.answer's route is {id, fingerprint}"],
  [{ take: [] }, "stand_in.answer's take is a list of take numbers"],
  [{ take: [0] }, "stand_in.answer's take is a list of take numbers"],
  [{ take: [true] }, "stand_in.answer's take is a list of take numbers"],
  [{ take: [1.5] }, "stand_in.answer's take is a list of take numbers"],
  [{ key: "" }, "stand_in.answer's key is the call key"],
  [{ instance: { id: "draw#1" } }, "stand_in.answer's instance is {id, path, step}"],
  [{ files: { [DIGEST]: { digest: DIGEST } } }, "stand_in.answer's files are file refs by digest"],
  [{ request: [] }, "stand_in.answer's request is a JSON object"],
  [{ request: { image: { file: DIGEST } } }, `the request holds the file ${DIGEST}, which files does not hold`],
])("params that are not a call's: %p", async (changes, message) => {
  expect(await callFor(params(changes))).toEqual({ error: { code: -32602, message } });
});

test("only a digest makes a file value", async () => {
  const ref = { digest: DIGEST, kind: "image/png", size: 1, name: "a.png", path: "/x", facts: {} };
  const request = {
    a: { file: DIGEST },
    b: { file: DIGEST, also: 1 },
    c: { file: "a.png" },
    d: [[{ file: DIGEST }]],
  };
  const call = (await callFor(params({ request, files: { [DIGEST]: ref }, take: [1, 2] }))) as StandInCall;
  const file = call.files.get(DIGEST);
  expect(file).toBeInstanceOf(InputFile);
  expect(call.request.a).toBe(file as InputFile);
  expect(call.request.b).toEqual({ file: DIGEST, also: 1 });
  expect(call.request.c).toEqual({ file: "a.png" });
  expect((call.request.d as unknown[][])[0]?.[0]).toBe(file as InputFile);
  // The request is a copy: the params keep their file values.
  expect(call.params.request).toBe(request as never);
  expect(request.a).toEqual({ file: DIGEST });
  expect([call.capability, call.route, call.key, call.takes, call.take, call.instance]).toEqual([
    "image.generate",
    { id: "img-a@acme", fingerprint: "e".repeat(64) },
    KEY,
    [1, 2],
    2,
    { id: "draw#1", path: "draw", step: "draw" },
  ]);
  expect(call.signal.aborted).toBe(false);
});

// --- the answer -------------------------------------------------------------------------------

test("answers and what they send", async () => {
  const clip = join(folder, "clip.mp4");
  writeFileSync(clip, "clip");
  writeFileSync(join(folder, "a.mp3"), "mp3");
  const source = { digest: DIGEST, kind: "audio/mpeg", size: 3, name: "a.mp3", path: join(folder, "a.mp3"), facts: {} };
  const bytes = new Uint8Array(Buffer.from("xxsliced").buffer, 2, 6);
  const sent = await encode({
    files: {
      raw: new TextEncoder().encode("raw"),
      buffer: Buffer.from("buffer"),
      sliced: bytes.subarray(2),
      made: { bytes: new TextEncoder().encode("glTF"), kind: "model/gltf-binary" },
      plain: { bytes: new TextEncoder().encode("plain") },
      echo: new InputFile(source),
      path: pathToFileURL(clip),
    },
    data: { facts: null },
  });
  expect(sent).toEqual({
    files: {
      raw: { base64: b64("raw") },
      buffer: { base64: b64("buffer") },
      sliced: { base64: b64("iced") },
      made: { base64: b64("glTF"), kind: "model/gltf-binary" },
      plain: { base64: b64("plain") },
      echo: { base64: b64("mp3"), kind: "audio/mpeg" },
      path: { base64: b64("clip"), kind: "video/mp4" },
    },
    data: { facts: null },
  });
  expect(protocolErrors(sent, "stand_in_answer_result")).toEqual([]);
  expect(await encode(DECLINE)).toEqual({ decline: true });
  expect(await encode({})).toEqual({ files: {}, data: null });
  expect(protocolErrors(await encode({}), "stand_in_answer_result")).toEqual([]);
});

test("isDecline tells DECLINE from an answer", () => {
  expect(isDecline(DECLINE)).toBe(true);
  expect(isDecline(Symbol.for("grida.fx.decline"))).toBe(true);
  expect(isDecline(Symbol("decline"))).toBe(false);
  expect(isDecline({ data: null })).toBe(false);
  expect(isDecline(undefined)).toBe(false);
});

test("an answer's undefined members are left out; an undefined array item is refused", async () => {
  expect(await encode({ data: { a: 1, b: undefined } })).toEqual({ files: {}, data: { a: 1 } });
  expect(await encode({ files: undefined, data: undefined })).toEqual({ files: {}, data: null });
  expect(await encode({ files: { image: undefined, raw: new TextEncoder().encode("raw") } })).toEqual({
    files: { raw: { base64: b64("raw") } },
    data: null,
  });
  const sent = frame({ jsonrpc: "2.0", id: 1, result: await encode(Answer.submit({ x: 1, note: undefined })) });
  expect(sent.toString("utf8")).toEndWith(
    '{"files":{},"data":{"text":"","tool_calls":[{"id":"call_1","name":"submit","arguments":{"x":1}}]}}}',
  );
  await expect(encode({ data: [undefined] })).rejects.toThrow(
    new TypeError("an answer's data is not a JSON value FX can read: at /data/0: undefined is not a JSON value"),
  );
});

test("the answer helpers", () => {
  expect(Answer.json([1, "two"])).toEqual({ data: { json: [1, "two"] } });
  expect(Answer.turn().data).toEqual({ text: "", tool_calls: [] });
  expect(Answer.turn("done").data).toEqual({ text: "done", tool_calls: [] });
  const calls = [{ name: "a" }, { name: "b", id: "x", arguments: { n: 1 } }, { name: "c" }];
  expect((Answer.turn("", calls).data as { tool_calls: unknown }).tool_calls).toEqual([
    { id: "call_1", name: "a", arguments: {} },
    { id: "x", name: "b", arguments: { n: 1 } },
    { id: "call_3", name: "c", arguments: {} },
  ]);
  expect(Answer.submit().data).toEqual({ text: "", tool_calls: [{ id: "call_1", name: "submit", arguments: {} }] });
  expect(Answer.submit({ x: 1 }).data).toEqual({
    text: "",
    tool_calls: [{ id: "call_1", name: "submit", arguments: { x: 1 } }],
  });
});

test.each([
  [undefined, "a stand-in returns an answer or DECLINE, not undefined"],
  [new Uint8Array(1), "a stand-in returns an answer or DECLINE, not a Uint8Array"],
  ["png", "a stand-in returns an answer or DECLINE, not a string"],
  [Symbol("decline"), "a stand-in returns an answer or DECLINE, not a symbol"],
  [{ file: {} }, "an answer holds files and data, not 'file'"],
  [{ files: { image: "picture.png" } }, "an answer's file is bytes, an InputFile, {bytes, kind} or a file: URL; image is a string"],
  // Only the scheme: a URL may carry credentials or a signature, which must not reach a record.
  [{ files: { image: new URL("https://acme-user:acme-token@files.acme.test/a.png?sig=abc") } }, "image is an https: URL, not a file: URL"],
  [{ files: { image: new URL("data:image/png;base64,AA==") } }, "image is a data: URL, not a file: URL"],
  [{ files: { image: new ArrayBuffer(1) } }, "an answer's file is bytes, an InputFile, {bytes, kind} or a file: URL; image is an ArrayBuffer"],
  // An adapter's call hands out AdapterFile objects; a stand-in's answer takes its own call's files.
  [
    { files: { image: new AdapterFile({ digest: DIGEST, kind: "image/png", size: 1, path: "/acme/a.png" }) } },
    "an answer's file is bytes, an InputFile, {bytes, kind} or a file: URL; image is an AdapterFile",
  ],
  [{ files: { image: { bytes: new Uint8Array(1), kind: 3 } } }, "an answer's file kind is a file kind; image's is 3"],
  [{ files: { Image: new Uint8Array(1) } }, "an answer's files are named in lowercase ([a-z][a-z0-9_]*), not 'Image'"],
  [{ files: [new Uint8Array(1)] }, "an answer's files are an object of names to files, not an array"],
  [{ data: { score: Number.NaN } }, "an answer's data is not a JSON value FX can read: at /data/score: NaN is not a JSON number"],
  [{ data: new Map() }, "an answer's data is not a JSON value FX can read: at /data: a Map is not a JSON value"],
])("what a stand-in cannot answer: %p", async (returned, message) => {
  await expect(encode(returned)).rejects.toThrow(new TypeError(message));
});

test("initialize", () => {
  const result = initializeResult({ protocol: "fx-node-protocol-v1" });
  expect(result).toEqual({
    protocol: "fx-node-protocol-v1",
    host: { language: "typescript", version: process.versions.node as string, sdk_version: sdkVersion() },
  });
  expect(protocolErrors(result, "initialize_result")).toEqual([]);
  const manifest = JSON.parse(readFileSync(join(packageRoot, "package.json"), "utf8"));
  expect(sdkVersion()).toBe(manifest.version);
  let mismatch: unknown;
  try {
    initializeResult({ protocol: "fx-node-protocol-v9" });
  } catch (error) {
    mismatch = error;
  }
  expect(mismatch).toBeInstanceOf(Refusal);
  expect((mismatch as Refusal).body).toEqual({
    code: -32003,
    message: `the engine speaks fx-node-protocol-v9 and @grida/fx ${manifest.version} speaks fx-node-protocol-v1`,
    data: { engine_protocol: "fx-node-protocol-v9", host_protocol: "fx-node-protocol-v1" },
  });
  expect(() => initializeResult(null)).toThrow("initialize needs the engine's protocol");
});

// --- the answerer -----------------------------------------------------------------------------

test("an answerer takes a function", () => {
  expect(() => new Answerer(3 as never)).toThrow(new TypeError("a stand-in is a function of one StandInCall, not a number"));
});

test("an answerer refuses params it cannot read, at once", async () => {
  const answerer = new Answerer(() => DECLINE);
  expect(await answerAll(answerer, [[1, "not params"], [2, params()]])).toEqual([
    [1, { error: { code: -32602, message: "stand_in.answer's params are a JSON object" } }],
    [2, { result: { decline: true } }],
  ]);
  expect(answerer.fault).toBeNull();
});

test("one call at a time, in the order they came", async () => {
  const order: string[] = [];
  const answer: StandIn = async (call) => {
    const prompt = String(call.request.prompt);
    order.push(`start ${prompt}`);
    if (prompt === "slow") {
      await new Promise((done) => setTimeout(done, 50));
    }
    order.push(`end ${prompt}`);
    if (prompt === "refuse") {
      throw new CallRefused("no lighthouses here");
    }
    if (prompt === "fail") {
      throw new CallFailed("the lighthouse fell");
    }
    if (prompt === "empty") {
      throw new CallRefused("");
    }
    return prompt === "count" ? Answer.json({ asked: order.length }) : DECLINE;
  };
  const answerer = new Answerer(answer);
  const prompts = ["slow", "decline", "refuse", "fail", "empty", "count"];
  const replies = await answerAll(
    answerer,
    prompts.map((prompt, index) => [10 + index, params({ request: { prompt } })] as const),
  );
  expect(replies).toEqual([
    [10, { result: { decline: true } }],
    [11, { result: { decline: true } }],
    [12, { error: { code: -32015, message: "no lighthouses here" } }],
    [13, { error: { code: -32016, message: "the lighthouse fell" } }],
    [14, { error: { code: -32015, message: "CallRefused" } }],
    [15, { result: { files: {}, data: { json: { asked: 12 } } } }],
  ]);
  expect(order.slice(0, 4)).toEqual(["start slow", "end slow", "start decline", "end decline"]);
  expect(answerer.fault).toBeNull();
});

test("an answerer keeps the first fault", async () => {
  const told: unknown[] = [];
  const first = new RangeError("first");
  let asked = 0;
  const answerer = new Answerer(
    () => {
      asked += 1;
      throw first;
    },
    { onFault: (error) => told.push(error) },
  );
  expect(await answerAll(answerer, [[1, params()], [2, params()]])).toEqual([
    [1, { error: { code: -32099, message: "RangeError: first" } }],
    [2, { error: { code: -32099, message: "the stand-in failed earlier" } }],
  ]);
  expect(answerer.fault).toEqual({ error: first });
  expect(told).toEqual([first]);
  expect(asked).toBe(1);
});

test.each([
  [() => undefined, "TypeError: a stand-in returns an answer or DECLINE, not undefined"],
  [() => ({ files: { image: "a.png" } }), "TypeError: an answer's file is bytes, an InputFile, {bytes, kind} or a file: URL; image is a string"],
  [() => Promise.reject(42), "NonError: 42"],
  [() => {
    throw new Cancelled("cancelled by itself");
  }, "Cancelled: cancelled by itself"],
])("a return value or a throw that is a fault: %#", async (fn, message) => {
  const answerer = new Answerer(fn as StandIn);
  expect(await answerAll(answerer, [[1, params()]])).toEqual([[1, { error: { code: -32099, message } }]]);
  expect(answerer.fault).not.toBeNull();
});

test("a fault is named as the answerer is told to", async () => {
  const answerer = new Answerer(
    () => {
      throw new Error("x");
    },
    { errorText: (error) => `named ${(error as Error).message}` },
  );
  expect(await answerAll(answerer, [[1, params()]])).toEqual([[1, { error: { code: -32099, message: "named x" } }]]);
});

const CANCELLED = { code: -32002, message: "the engine cancelled stand_in.answer" };

test("a call cancelled before it starts is answered cancelled and never asked", async () => {
  const asked: string[] = [];
  const answerer = new Answerer((call) => {
    asked.push(call.key);
    return DECLINE;
  });
  expect(await answerAll(answerer, [["x", params()]], "x")).toEqual([["x", { error: CANCELLED }]]);
  await new Promise((done) => setTimeout(done, 10));
  expect(asked).toEqual([]);
});

test("a running call is answered cancelled at once; what it does later is dropped, never a fault", async () => {
  let release: () => void = () => undefined;
  const aborted: unknown[] = [];
  const answerer = new Answerer(async (call) => {
    if (call.key === KEY) {
      call.signal.addEventListener("abort", () => aborted.push((call.signal.reason as Error).message));
      await new Promise<void>((done) => {
        release = done;
      });
      throw new Error("late");
    }
    return DECLINE;
  });
  const replies: [number, Reply][] = [];
  answerer.start(1, params(), (reply) => replies.push([1, reply]));
  answerer.start(2, params({ key: "b".repeat(64) }), (reply) => replies.push([2, reply]));
  await new Promise((done) => setTimeout(done, 10));
  expect(answerer.cancel(1)).toBe(true);
  expect(replies).toEqual([[1, { error: CANCELLED }]]);
  expect(aborted).toEqual(["the engine cancelled stand_in.answer"]);
  // The next call waits for the cancelled one to settle: one at a time.
  await new Promise((done) => setTimeout(done, 10));
  expect(replies).toHaveLength(1);
  release();
  await new Promise((done) => setTimeout(done, 10));
  expect(replies).toEqual([
    [1, { error: CANCELLED }],
    [2, { result: { decline: true } }],
  ]);
  expect(answerer.fault).toBeNull();
  // Cancelling it again, or an id that names nothing, is ignored.
  expect(answerer.cancel(1)).toBe(false);
  expect(answerer.cancel("nothing")).toBe(false);
});

test("close ends every call and answers none", async () => {
  const signals: AbortSignal[] = [];
  const answerer = new Answerer(async (call) => {
    signals.push(call.signal);
    await new Promise((done) => setTimeout(done, 30));
    return DECLINE;
  });
  const replies: Reply[] = [];
  answerer.start(1, params(), (reply) => replies.push(reply));
  answerer.start(2, params(), (reply) => replies.push(reply));
  await new Promise((done) => setTimeout(done, 5));
  answerer.close();
  await new Promise((done) => setTimeout(done, 60));
  expect(replies).toEqual([]);
  expect(signals.map((signal) => signal.aborted)).toEqual([true]);
});

test("an error answer holding a lone surrogate can be sent", async () => {
  const name = "gull\udcff.png";
  const answerer = new Answerer((call) => {
    if (call.key === KEY) {
      throw new CallRefused(`no file ${name}`);
    }
    throw new Error(`cannot read ${name}`);
  });
  const replies = await answerAll(answerer, [[1, params()], [2, params({ key: "b".repeat(64) })]]);
  expect(replies).toEqual([
    [1, { error: { code: -32015, message: "no file gull\\udcff.png" } }],
    [2, { error: { code: -32099, message: "Error: cannot read gull\\udcff.png" } }],
  ]);
  for (const [id, reply] of replies) {
    frame({ jsonrpc: "2.0", id, ...reply }); // throws when it cannot be sent
  }
});
