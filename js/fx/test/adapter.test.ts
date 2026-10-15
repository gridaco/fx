// Project adapters (src/adapter.ts; spec/adapters.md §9, spec/protocol.md §5.8, §6.8 and §8.2): the
// declaration, the call, `fetch`, the outcomes on the wire, and the AdapterServer driven without an
// engine. host-adapter.test.ts drives them through a host process, and adapter-engine.test.ts
// through the engine. The cases follow python/tests/test_adapter.py.

import { describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import {
  type AdapterMethod,
  type AdapterReply,
  AdapterServer,
  adapterRecordOf,
  encodeOutcome,
  fetchParams,
  functionFor,
  NotACall,
  readResponse,
} from "../src/adapter.js";
import { Cancelled, EngineError } from "../src/engine-errors.js";
import {
  type AdapterCall,
  AdapterCall as Call,
  AdapterFile,
  Answered,
  adapter,
  FetchError,
  FetchResponse,
  InputFile,
  type SendOutcome,
} from "../src/index.js";
import { protocolErrors } from "./json-schema.js";

const KEY = "a".repeat(64);
const DIGEST = "d".repeat(64);
const SEND: AdapterMethod = "adapter.send";
const SUBMIT: AdapterMethod = "adapter.submit";
const COLLECT: AdapterMethod = "adapter.collect";

function expectValid(value: unknown, name: string): void {
  expect(protocolErrors(value, name)).toEqual([]);
}

function folder(): string {
  const path = mkdtempSync(join(tmpdir(), "grida-fx-ts-adapter-"));
  process.on("exit", () => rmSync(path, { recursive: true, force: true }));
  return path;
}

function params(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    call_id: "c1",
    adapter: { path: "adapters/acme.ts", attribute: "acme" },
    capability: "image.generate",
    route: {
      id: "img-a@acme",
      model: "img-a",
      provider: "acme",
      contract: { adapter: "adapters/acme.ts#acme", adapter_behavior: 1 },
    },
    base_url: "https://api.acme.test/v1",
    request: { prompt: "a kite" },
    files: {},
    take: [1],
    key: KEY,
    attempt: 1,
    ...over,
  };
}

const noop = async (): Promise<SendOutcome> => ({ outcome: "refused", reason: "nothing" });
const accept = () => ({ outcome: "accepted", handle: {} }) as const;
const collectNothing = () => ({ outcome: "unreachable", reason: "nothing" }) as const;
const b64 = (text: string) => Buffer.from(text, "latin1").toString("base64");

// ------------------------------------------------------------------------------------------------
// Declaring

describe("declaring an adapter", () => {
  test("its capabilities, in order, and the function of each method", () => {
    const declared = adapter({
      requests: ["structured.generate", "image.generate"],
      jobs: ["acme.render"],
      send: noop,
      submit: accept,
      collect: collectNothing,
      version: 3,
    });
    expect(declared.requests).toEqual(["structured.generate", "image.generate"]);
    expect(declared.jobs).toEqual(["acme.render"]);
    expect(Object.entries(declared.capabilities)).toEqual([
      ["structured.generate", "request"],
      ["image.generate", "request"],
      ["acme.render", "job"],
    ]);
    expect(declared.version).toBe(3);
    expect(declared.send).toBe(noop);
    expect(Object.isFrozen(declared)).toBe(true);
    const record = adapterRecordOf(declared);
    expect(record).toBeDefined();
    expect(declared[Symbol.for("grida.fx.adapter") as never]).toBe(record as never);
    if (record === undefined) {
      return;
    }
    expect(functionFor(record, SEND, "image.generate")).toBe(noop);
    expect(functionFor(record, SUBMIT, "acme.render")).toBe(accept);
    expect(functionFor(record, COLLECT, "acme.render")).toBe(collectNothing);
    expect(functionFor(record, SEND, "acme.render")).toBeUndefined();
    expect(functionFor(record, SUBMIT, "image.generate")).toBeUndefined();
    expect(functionFor(record, SEND, "video.generate")).toBeUndefined();
    expect(functionFor(record, SEND, "constructor")).toBeUndefined();
    // Without a version, and with undefined members left out.
    // Any iterable of names, as in Python.
    const jobs = new Set(["video.generate"]) as unknown as string[];
    const unversioned = adapter({ jobs, submit: accept, collect: collectNothing, send: undefined });
    expect(unversioned.version).toBeNull();
    expect(unversioned.capabilities).toEqual({ "video.generate": "job" });
    expect("send" in unversioned).toBe(false);
    // Anything else holds no adapter.
    for (const value of [undefined, null, 3, {}, noop, { [Symbol.for("grida.fx.adapter")]: { capabilities: [] } }]) {
      expect(adapterRecordOf(value)).toBeUndefined();
    }
  });

  test("declarations that are refused, with the Python SDK's sentences", () => {
    const refused = (declaration: unknown) => {
      try {
        adapter(declaration as never);
      } catch (error) {
        expect(error).toBeInstanceOf(TypeError);
        return (error as Error).message;
      }
      return "accepted";
    };
    expect(refused(null)).toBe("an adapter declaration is an object, not null");
    expect(refused({ requests: ["image.generate"], send: noop, versoin: 1 })).toBe(
      "an adapter declaration has no field 'versoin'",
    );
    expect(refused({})).toBe("adapter() declares no capability: give it requests or jobs");
    expect(refused({ requests: [], send: noop })).toBe("adapter() declares no capability: give it requests or jobs");
    expect(refused({ requests: "image.generate" })).toBe("adapter(): requests is a list of capability names");
    expect(refused({ jobs: [3] })).toBe("adapter(): jobs is a list of capability names");
    expect(refused({ requests: ["Image"] })).toBe('adapter(): "Image" is not a capability name');
    expect(refused({ requests: ["image."] })).toBe('adapter(): "image." is not a capability name');
    expect(
      refused({ requests: ["image.generate"], jobs: ["image.generate"], send: noop, submit: accept, collect: collectNothing }),
    ).toBe("adapter(): image.generate is named twice");
    expect(refused({ requests: ["image.generate"] })).toBe("adapter(): requests need send");
    expect(refused({ jobs: ["video.generate"], submit: accept })).toBe("adapter(): jobs need submit and collect");
    expect(refused({ requests: ["image.generate"], send: 3 })).toBe("adapter(): send is a function");
    for (const version of [-1, 1.5, true, "1", 2 ** 60]) {
      expect(refused({ requests: ["image.generate"], send: noop, version })).toBe(
        "adapter(): version is an integer of at least 0, or null",
      );
    }
  });
});

// ------------------------------------------------------------------------------------------------
// The call

describe("an adapter call", () => {
  test("reads its params; its request's file values are its files", async () => {
    const root = folder();
    writeFileSync(join(root, "kite.png"), "kite");
    const file = { digest: DIGEST, kind: "image/png", size: 4, path: join(root, "kite.png") };
    const request = { prompt: "a kite", references: [{ file: DIGEST }], other: { file: "x.png" } };
    const given = params({ request, files: { [DIGEST]: file }, take: [1, 3], attempt: 2 });
    expectValid(given, "adapter_send_params");
    const call = new Call(given);
    expect(call.capability).toBe("image.generate");
    expect(call.route).toEqual({
      id: "img-a@acme",
      model: "img-a",
      provider: "acme",
      contract: { adapter: "adapters/acme.ts#acme", adapter_behavior: 1 },
    });
    expect([call.baseUrl, call.key, call.attempt]).toEqual(["https://api.acme.test/v1", KEY, 2]);
    expect([call.takes, call.take, call.handle]).toEqual([[1, 3], 3, undefined]);
    const kite = call.files.get(DIGEST) as AdapterFile;
    expect(kite).toBeInstanceOf(AdapterFile);
    expect([kite.digest, kite.kind, kite.size, kite.path]).toEqual([DIGEST, "image/png", 4, join(root, "kite.png")]);
    expect(Buffer.from(await kite.bytes()).toString()).toBe("kite");
    // The request is a copy, its file values the call's files.
    expect((call.request.references as AdapterFile[])[0]).toBe(kite);
    expect(call.request.other).toEqual({ file: "x.png" });
    expect((given.request as typeof request).references).toEqual([{ file: DIGEST }]);
    expect(call.signal.aborted).toBe(false);
    expect(String(call)).toBe("AdapterCall(adapter.send image.generate on img-a@acme, attempt 2, take 3)");
    const collecting = params({ handle: { job: "j-1" } });
    expectValid(collecting, "adapter_collect_params");
    expect(new Call(collecting, { method: COLLECT }).handle).toEqual({ job: "j-1" });
  });

  test("params that are not a call's", () => {
    const cases: [AdapterMethod, Record<string, unknown>, string][] = [
      [SEND, { call_id: "" }, "adapter.send's call_id is a string"],
      [SEND, { adapter: { path: "a.ts" } }, "adapter.send's adapter is {path, attribute}"],
      [SEND, { capability: 3 }, "adapter.send's capability is a capability's name"],
      [
        SEND,
        { route: { id: "img-a@acme", model: "img-a", provider: "acme" } },
        "adapter.send's route is {id, model, provider, contract}",
      ],
      [SEND, { base_url: "" }, "adapter.send's base_url is a URL"],
      [SEND, { request: [] }, "adapter.send's request is a JSON object"],
      [
        SEND,
        { files: { [DIGEST]: { digest: DIGEST, kind: "image/png", size: -1, path: "/x" } } },
        "adapter.send's files are {digest, kind, size, path} by digest",
      ],
      [SEND, { take: [] }, "adapter.send's take is a list of take numbers"],
      [SEND, { take: [true] }, "adapter.send's take is a list of take numbers"],
      [SEND, { key: null }, "adapter.send's key is the call key"],
      [SEND, { attempt: 7 }, "adapter.send's attempt is from 1 to 6"],
      [SEND, { attempt: true }, "adapter.send's attempt is from 1 to 6"],
      [SEND, { handle: {} }, "adapter.send has no handle"],
      [SUBMIT, { handle: {} }, "adapter.submit has no handle"],
      [COLLECT, {}, "adapter.collect's handle is a JSON object"],
      [COLLECT, { handle: ["j-1"] }, "adapter.collect's handle is a JSON object"],
      [
        SEND,
        { request: { image: { file: DIGEST } } },
        `the request holds the file ${DIGEST}, which files does not hold`,
      ],
    ];
    for (const [method, over, message] of cases) {
      expect(() => new Call(params(over), { method })).toThrow(new NotACall(message));
    }
    expect(() => new Call([], { method: SEND })).toThrow("adapter.send's params are a JSON object");
    expect(() => new Call(params(), { method: "adapter.load" as never })).toThrow("adapter.load is not an adapter call");
  });
});

// ------------------------------------------------------------------------------------------------
// fetch

/** Answers each `fetch` with the next of `answers`, keeping what it was sent. */
function engine(...answers: unknown[]): { sent: Record<string, unknown>[]; fetch: (p: Record<string, unknown>) => Promise<unknown> } {
  const sent: Record<string, unknown>[] = [];
  return {
    sent,
    fetch: async (given) => {
      sent.push(given);
      const answer = answers.shift();
      if (answer instanceof Error) {
        throw answer;
      }
      return answer;
    },
  };
}

describe("fetch", () => {
  test("sends the call's call_id and reads the response", async () => {
    const answering = engine({
      status: 200,
      headers: [
        ["content-type", "application/json"],
        ["x-acme", "1"],
      ],
      base64: b64('{"ok": true}'),
    });
    const call = new Call(params({ call_id: "c7" }), { fetch: answering.fetch });
    const response = await call.fetch(new URL("https://api.acme.test/v1/models"));
    expect(answering.sent).toEqual([
      { call_id: "c7", method: "GET", url: "https://api.acme.test/v1/models", lane: "provider" },
    ]);
    expect(response).toBeInstanceOf(FetchResponse);
    expect(response.status).toBe(200);
    expect(response.headers).toEqual([
      ["content-type", "application/json"],
      ["x-acme", "1"],
    ]);
    expect(new Headers(response.headers as [string, string][]).get("X-Acme")).toBe("1");
    expect(Buffer.from(response.body).toString()).toBe('{"ok": true}');
    expect(response.json()).toEqual({ ok: true });
    expect(response.text()).toBe('{"ok": true}');
  });

  test("options on the wire", () => {
    const sent = [
      fetchParams("c1", "https://api.acme.test/v1/draw", {
        method: "post" as "POST",
        headers: { Accept: "application/json", "x-trace": "on" },
        json: { prompt: "a kite", seed: undefined },
        credential: true,
        timeoutS: 30,
        maxResponseBytes: 1024,
      }),
      fetchParams("c1", "https://api.acme.test/v1/files", {
        method: "POST",
        headers: new Map([["X-Kind", "png"]]),
        body: new Uint8Array([0x89, 0x50, 0x4e, 0x47]),
        contentType: "image/png",
        lane: "upload",
        timeoutS: 0.5,
      }),
      fetchParams("c1", "https://files.acme.test/a.png", { lane: "download", headers: [], credential: false }),
      fetchParams("c1", "https://api.acme.test/v1/null", { method: "POST", json: null }),
    ];
    expect(sent).toEqual([
      {
        call_id: "c1",
        method: "POST",
        url: "https://api.acme.test/v1/draw",
        headers: [
          ["accept", "application/json"],
          ["x-trace", "on"],
        ],
        body: { json: { prompt: "a kite", seed: undefined } },
        lane: "provider",
        credential: true,
        timeout_s: 30,
        max_response_bytes: 1024,
      },
      {
        call_id: "c1",
        method: "POST",
        url: "https://api.acme.test/v1/files",
        headers: [["x-kind", "png"]],
        body: { base64: Buffer.from([0x89, 0x50, 0x4e, 0x47]).toString("base64"), content_type: "image/png" },
        lane: "upload",
        timeout_s: 0.5,
      },
      { call_id: "c1", method: "GET", url: "https://files.acme.test/a.png", lane: "download" },
      { call_id: "c1", method: "POST", url: "https://api.acme.test/v1/null", body: { json: null }, lane: "provider" },
    ]);
    for (const given of sent) {
      expectValid(JSON.parse(JSON.stringify(given)), "fetch_params");
    }
  });

  test("refusals before anything is sent", async () => {
    const cases: [unknown, Record<string, unknown>, string][] = [
      [3, {}, "fetch's url is a string or a URL"],
      ["", {}, "fetch's url is an absolute URL"],
      ["https://a.test", { verb: "GET" }, "fetch has no option 'verb'"],
      ["https://a.test", { method: "PUT" }, "fetch takes GET or POST, not PUT"],
      ["https://a.test", { json: { a: 1 } }, "a GET has no body"],
      ["https://a.test", { body: new Uint8Array(1), contentType: "text/plain" }, "a GET has no body"],
      [
        "https://a.test",
        { method: "POST", json: 1, body: new Uint8Array(1), contentType: "text/plain" },
        "fetch takes json or body, not both",
      ],
      ["https://a.test", { method: "POST", body: new Uint8Array(1) }, "a body needs its contentType"],
      ["https://a.test", { method: "POST", body: "x", contentType: "a/b" }, "fetch's body is bytes, not a string"],
      ["https://a.test", { contentType: "text/plain" }, "contentType goes with a body"],
      ["https://a.test", { lane: "cdn" }, "fetch's lane is provider, upload or download, not 'cdn'"],
      ["https://a.test", { credential: "yes" }, "credential is true or false"],
      ["https://a.test", { headers: { "x y": "1" } }, 'the header name "x y" is not a token'],
      ["https://a.test", { headers: [["a"]] }, "fetch's headers are an object or [name, value] pairs"],
      ["https://a.test", { headers: "a: 1" }, "fetch's headers are an object or [name, value] pairs"],
      ["https://a.test", { headers: { a: 1 } }, "a header's name and value are strings"],
      ["https://a.test", { timeoutS: 0 }, "timeoutS is above 0 and at most 300"],
      ["https://a.test", { timeoutS: 301 }, "timeoutS is above 0 and at most 300"],
      ["https://a.test", { timeoutS: Number.NaN }, "timeoutS is above 0 and at most 300"],
      ["https://a.test", { timeoutS: "30" }, "timeoutS is above 0 and at most 300"],
      ["https://a.test", { maxResponseBytes: 0 }, "maxResponseBytes is from 1 to 67108864"],
      ["https://a.test", { maxResponseBytes: 64 * 1024 * 1024 + 1 }, "maxResponseBytes is from 1 to 67108864"],
      ["https://a.test", { maxResponseBytes: 1.5 }, "maxResponseBytes is from 1 to 67108864"],
    ];
    const answering = engine();
    const call = new Call(params(), { fetch: answering.fetch });
    for (const [url, options, message] of cases) {
      await expect(call.fetch(url as string, options as never)).rejects.toThrow(new TypeError(message));
    }
    await expect(call.fetch("https://a.test", 3 as never)).rejects.toThrow("fetch's options are an object, not a number");
    expect(answering.sent).toEqual([]);
  });

  test("a failed exchange, and what the SDK cannot read", async () => {
    for (const phase of ["refused", "not_sent", "after_send"] as const) {
      let failed: unknown;
      try {
        readResponse({ error: { phase, reason: "the network is off" } });
      } catch (error) {
        failed = error;
      }
      expect(failed).toBeInstanceOf(FetchError);
      expect(failed).toBeInstanceOf(Error);
      const { phase: given, reason, message, name } = failed as FetchError;
      expect([given, reason, message, name]).toEqual([phase, "the network is off", "the network is off", "FetchError"]);
    }
    // A FetchError of another copy of @grida/fx is one too.
    const other = Object.defineProperty(new Error("x"), Symbol.for("grida.fx.fetch_error"), { value: true });
    expect(other).toBeInstanceOf(FetchError);
    expect(new Error("x")).not.toBeInstanceOf(FetchError);
    for (const unreadable of [
      null,
      { error: { phase: "lost", reason: "x" } },
      { error: { phase: "refused", reason: "x" }, status: 200 },
      { status: 200, headers: [], base64: "not base64!" },
      { status: 200, headers: [], base64: "AAA" },
      { status: "200", headers: [], base64: "" },
      { status: 200, headers: [["a"]], base64: "" },
    ]) {
      let failed: unknown;
      try {
        readResponse(unreadable);
      } catch (error) {
        failed = error;
      }
      expect(failed).toBeInstanceOf(EngineError);
      expect((failed as EngineError).code).toBe(-32099);
      expect((failed as EngineError).message).toBe("the engine answered fetch with something the SDK cannot read");
    }
    // An error answer reaches the adapter as the engine's error.
    const call = new Call(params(), { fetch: engine(new Cancelled("the adapter call was dropped")).fetch });
    await expect(call.fetch("https://api.acme.test/v1")).rejects.toBeInstanceOf(Cancelled);
    await expect(new Call(params()).fetch("https://api.acme.test/v1")).rejects.toThrow(
      "this call has no engine to fetch through",
    );
  });
});

// ------------------------------------------------------------------------------------------------
// Outcomes

describe("outcomes", () => {
  test("on the wire", async () => {
    const root = folder();
    writeFileSync(join(root, "clip.mp4"), "clip");
    writeFileSync(join(root, "kite.png"), "kite");
    const kite = new AdapterFile({ digest: DIGEST, kind: "image/png", size: 4, path: join(root, "kite.png") });
    const cases: [AdapterMethod, unknown, Record<string, unknown>][] = [
      [
        SEND,
        {
          outcome: "answered",
          files: {
            raw: Buffer.from("raw"),
            made: { bytes: Buffer.from("glTF"), kind: "model/gltf-binary" },
            echo: kite,
            path: pathToFileURL(join(root, "clip.mp4")),
            skipped: undefined,
          },
          data: { seed: 7, note: undefined },
          costUsd: 0.04,
          usage: { inputTokens: 12, outputTokens: 3, cachedInputTokens: undefined },
        },
        {
          outcome: "answered",
          files: {
            raw: { base64: b64("raw") },
            made: { base64: b64("glTF"), kind: "model/gltf-binary" },
            echo: { base64: b64("kite"), kind: "image/png" },
            path: { base64: b64("clip"), kind: "video/mp4" },
          },
          data: { seed: 7, note: undefined },
          cost_usd: 0.04,
          usage: { input_tokens: 12, output_tokens: 3 },
        },
      ],
      [
        SEND,
        Answered.json({ name: "teal" }, { costUsd: 0 }),
        { outcome: "answered", files: {}, data: { json: { name: "teal" } }, cost_usd: 0 },
      ],
      [
        SEND,
        { outcome: "not_received", reason: "acme is busy", retryAfterS: 2.5 },
        { outcome: "not_received", reason: "acme is busy", retry_after_s: 2.5 },
      ],
      [SUBMIT, { outcome: "not_received", reason: "no route" }, { outcome: "not_received", reason: "no route" }],
      [SEND, { outcome: "refused", reason: "a blank prompt" }, { outcome: "refused", reason: "a blank prompt" }],
      [
        SEND,
        { outcome: "failed", reason: "acme returned HTTP 500", retryable: true, costUsd: undefined },
        { outcome: "failed", reason: "acme returned HTTP 500", retryable: true },
      ],
      [
        SUBMIT,
        { outcome: "failed", reason: "acme returned HTTP 400", retryable: false, costUsd: 0.01 },
        { outcome: "failed", reason: "acme returned HTTP 400", retryable: false, cost_usd: 0.01 },
      ],
      [
        SUBMIT,
        { outcome: "accepted", handle: { job: "j-1", poll: [1, 2] } },
        { outcome: "accepted", handle: { job: "j-1", poll: [1, 2] } },
      ],
      [SUBMIT, { outcome: "uncertain", reason: "timed out" }, { outcome: "uncertain", reason: "timed out" }],
      [COLLECT, { outcome: "answered" }, { outcome: "answered", files: {}, data: null }],
      [
        COLLECT,
        { outcome: "ended", reason: "the job was cancelled", costUsd: 0 },
        { outcome: "ended", reason: "the job was cancelled", cost_usd: 0 },
      ],
      [COLLECT, { outcome: "ended", reason: "expired" }, { outcome: "ended", reason: "expired" }],
      [COLLECT, { outcome: "unreachable", reason: "still running" }, { outcome: "unreachable", reason: "still running" }],
    ];
    const results = { [SEND]: "adapter_send_result", [SUBMIT]: "adapter_submit_result", [COLLECT]: "adapter_collect_result" };
    for (const [method, outcome, wire] of cases) {
      const sent = await encodeOutcome(method, outcome);
      expect(sent).toEqual(wire);
      expectValid(JSON.parse(JSON.stringify(sent)), results[method]);
    }
  });

  test("each outcome holds its members, and each method takes its outcomes", async () => {
    const cases: [AdapterMethod, unknown, string][] = [
      [SEND, { outcome: "refused", reason: "" }, "a reason is a sentence, not a blank string"],
      [SUBMIT, { outcome: "uncertain", reason: "  \n" }, "a reason is a sentence, not a blank string"],
      [COLLECT, { outcome: "unreachable" }, "a reason is a sentence, not undefined"],
      [SEND, { outcome: "failed", reason: "x", retryable: 1 }, "retryable is true or false"],
      [SEND, { outcome: "failed", reason: "x" }, "retryable is true or false"],
      [SEND, { outcome: "failed", reason: "x", retryable: true, costUsd: -1 }, "costUsd is a number of at least 0, not -1"],
      [COLLECT, { outcome: "ended", reason: "x", costUsd: Infinity }, "costUsd is a number of at least 0, not Infinity"],
      [SEND, { outcome: "answered", costUsd: "0.1" }, "costUsd is a number, not a string"],
      [SEND, { outcome: "not_received", reason: "x", retryAfterS: -1 }, "retryAfterS is a number of at least 0, not -1"],
      [SEND, { outcome: "answered", usage: { tokens: 1 } }, "usage counts inputTokens, cachedInputTokens, outputTokens, not 'tokens'"],
      [SEND, { outcome: "answered", usage: { inputTokens: -1 } }, "usage inputTokens is an integer of at least 0, not -1"],
      [SEND, { outcome: "answered", usage: [1] }, "usage is an object of token counts, not an array"],
      [SUBMIT, { outcome: "accepted", handle: ["j-1"] }, "a handle is an object, not an array"],
      [SUBMIT, { outcome: "accepted" }, "a handle is an object, not undefined"],
      [SEND, { outcome: "answered", cost_usd: 1 }, "Answered holds outcome, files, data, costUsd and usage, not 'cost_usd'"],
      [SEND, { outcome: "refused", reason: "x", costUsd: 0 }, "Refused holds outcome and reason, not 'costUsd'"],
      [SEND, { outcome: "accepted", handle: {} }, "send returns Answered, NotReceived, Refused or Failed, not Accepted"],
      [SEND, undefined, "send returns Answered, NotReceived, Refused or Failed, not undefined"],
      [SEND, { outcome: "done" }, "send returns Answered, NotReceived, Refused or Failed, not the outcome 'done'"],
      [SEND, new Map(), "send returns Answered, NotReceived, Refused or Failed, not a Map"],
      [
        SUBMIT,
        { outcome: "answered" },
        "submit returns Accepted, Uncertain, NotReceived, Refused or Failed, not Answered",
      ],
      [COLLECT, { outcome: "refused", reason: "x" }, "collect returns Answered, Ended or Unreachable, not Refused"],
      [
        SEND,
        { outcome: "answered", data: Number.NaN },
        "an answer's data is not a JSON value FX can read: at /data: NaN is not a JSON number",
      ],
      [
        SEND,
        { outcome: "answered", files: { Image: new Uint8Array(1) } },
        "an answer's files are named in lowercase ([a-z][a-z0-9_]*), not 'Image'",
      ],
      [
        SEND,
        { outcome: "answered", files: { image: "x.png" } },
        "an answer's file is bytes, an AdapterFile, {bytes, kind} or a file: URL; image is a string",
      ],
      [
        SEND,
        {
          outcome: "answered",
          files: {
            image: new InputFile({ digest: DIGEST, kind: "image/png", size: 1, name: "a.png", path: "/acme/a.png", facts: {} }),
          },
        },
        "an answer's file is bytes, an AdapterFile, {bytes, kind} or a file: URL; image is an InputFile",
      ],
      [
        SUBMIT,
        { outcome: "accepted", handle: { at: Infinity } },
        "a handle is not a JSON object FX can read: at /handle/at: Infinity is not a JSON number",
      ],
      [SUBMIT, { outcome: "accepted", handle: { at: new Date(0) } }, "a handle is not a JSON object FX can read: at /handle/at: a Date is not a JSON value"],
    ];
    for (const [method, outcome, message] of cases) {
      await expect(encodeOutcome(method, outcome)).rejects.toThrow(new TypeError(message));
    }
    await expect(
      encodeOutcome(SEND, { outcome: "answered", files: { image: pathToFileURL(join(folder(), "missing.png")) } }),
    ).rejects.toThrow("ENOENT");
  });
});

// ------------------------------------------------------------------------------------------------
// Serving

/** What a server answered, by request id, in order. */
class Replies {
  readonly answers: [number, AdapterReply][] = [];
  #waiting: (() => void) | null = null;

  to(id: number): (reply: AdapterReply) => void {
    return (reply) => {
      this.answers.push([id, reply]);
      this.#waiting?.();
    };
  }

  async waitFor(count: number): Promise<void> {
    while (this.answers.length < count) {
      await new Promise<void>((done, fail) => {
        const timer = setTimeout(() => fail(new Error(`${this.answers.length} answers, not ${count}`)), 10_000);
        this.#waiting = () => {
          clearTimeout(timer);
          done();
        };
      });
    }
  }
}

function serve(
  server: AdapterServer,
  replies: Replies,
  id: number,
  method: AdapterMethod,
  fn: (call: AdapterCall) => unknown,
  over: Record<string, unknown> = {},
): AbortController {
  const controller = new AbortController();
  const call = new Call(params({ ...(method === COLLECT ? { handle: {} } : {}), ...over }), {
    method,
    signal: controller.signal,
  });
  server.start(id, { method, fn: fn as never, call, controller, name: "adapters/acme.ts#acme" }, replies.to(id));
  return controller;
}

describe("the adapter server", () => {
  test("answers calls at once, and in any order", async () => {
    const server = new AdapterServer();
    const replies = new Replies();
    let release: () => void = () => {};
    const released = new Promise<void>((done) => {
      release = done;
    });
    serve(server, replies, 1, SEND, async () => {
      await released;
      return { outcome: "refused", reason: "first, answered last" };
    });
    serve(server, replies, 2, SUBMIT, (call) => ({ outcome: "accepted", handle: { job: call.key.slice(0, 4) } }));
    serve(server, replies, 3, COLLECT, async (call) => Answered.json({ handle: call.handle ?? null }), { handle: { job: "j" } });
    await replies.waitFor(2);
    expect(server.pending).toBe(1);
    release();
    await replies.waitFor(3);
    expect(replies.answers).toEqual([
      [2, { result: { outcome: "accepted", handle: { job: "aaaa" } } }],
      [3, { result: { outcome: "answered", files: {}, data: { json: { handle: { job: "j" } } } } }],
      [1, { result: { outcome: "refused", reason: "first, answered last" } }],
    ]);
    expect(server.pending).toBe(0);
  });

  test("cancels calls: answered at once, the signal aborted, what comes later discarded", async () => {
    const server = new AdapterServer();
    const replies = new Replies();
    let seen: unknown;
    serve(server, replies, 1, SEND, async (call) => {
      await new Promise((done) => call.signal.addEventListener("abort", done));
      seen = call.signal.reason;
      return { outcome: "refused", reason: "too late" };
    });
    let finish: () => void = () => {};
    serve(server, replies, 2, SUBMIT, async () => {
      await new Promise<void>((done) => {
        finish = done;
      });
      throw new Error("thrown after the call was cancelled");
    });
    await Promise.resolve();
    expect(server.cancel(1)).toBe(true);
    expect(server.cancel(1)).toBe(false);
    expect(server.cancel(99)).toBe(false);
    expect(server.cancel(2)).toBe(true);
    finish();
    await new Promise((done) => setTimeout(done, 20));
    expect(replies.answers).toEqual([
      [1, { error: { code: -32002, message: "the engine cancelled adapter.send" } }],
      [2, { error: { code: -32002, message: "the engine cancelled adapter.submit" } }],
    ]);
    expect(seen).toBeInstanceOf(Cancelled);
    expect((seen as Error).message).toBe("the engine cancelled adapter.send");
  });

  test("a fault ends only its own call", async () => {
    const faults: [string, string][] = [];
    const server = new AdapterServer({ onFault: (error, name) => faults.push([name, String((error as Error).message)]) });
    const replies = new Replies();
    serve(server, replies, 1, SEND, () => {
      throw new RangeError("acme broke");
    });
    serve(server, replies, 2, SEND, () => undefined);
    serve(server, replies, 3, SEND, () => ({ outcome: "answered", files: { image: "x.png" } }));
    serve(server, replies, 4, SEND, () => {
      throw "a string";
    });
    serve(server, replies, 5, SEND, () => ({ outcome: "refused", reason: "still serving" }));
    await replies.waitFor(5);
    const byId = new Map(replies.answers);
    expect(byId.get(1)).toEqual({ error: { code: -32099, message: "RangeError: acme broke" } });
    expect(byId.get(2)).toEqual({
      error: { code: -32099, message: "TypeError: send returns Answered, NotReceived, Refused or Failed, not undefined" },
    });
    expect(byId.get(3)).toEqual({
      error: {
        code: -32099,
        message:
          "TypeError: an answer's file is bytes, an AdapterFile, {bytes, kind} or a file: URL; image is a string",
      },
    });
    expect(byId.get(4)).toEqual({ error: { code: -32099, message: "NonError: a string" } });
    expect(byId.get(5)).toEqual({ result: { outcome: "refused", reason: "still serving" } });
    expect(faults.map(([name]) => name)).toEqual(Array(4).fill("adapters/acme.ts#acme"));
  });

  test("closing it answers nothing, and aborts every call", async () => {
    const server = new AdapterServer();
    const replies = new Replies();
    const signals: AbortSignal[] = [];
    for (const id of [1, 2]) {
      serve(server, replies, id, SEND, async (call) => {
        signals.push(call.signal);
        await new Promise((done) => call.signal.addEventListener("abort", done));
        return { outcome: "refused", reason: "closed" };
      });
    }
    await new Promise((done) => setTimeout(done, 10));
    server.close();
    await new Promise((done) => setTimeout(done, 10));
    expect(signals.map((signal) => signal.aborted)).toEqual([true, true]);
    expect(replies.answers).toEqual([]);
    expect(server.pending).toBe(0);
  });
});
