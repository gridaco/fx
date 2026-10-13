// In-process stand-ins (src/api.ts `run(target, { standIn })`, src/socket.ts, src/stand-in.ts
// `serveStandIn`) against a fake engine: a Node script, set as GRIDA_FX_BIN, that checks its
// standard input is a socket, speaks the stand-in session on it as the engine does, and logs what
// the answerer sent. Skipped without Node or on Windows. Only one case runs against the real
// engine, and only where one is built: test/api.test.ts, a paid call answered in process and then
// from the stand-in store. Conformance covers stand-in files (`--stand-in stand_in.ts#answer`),
// which the TypeScript node host answers, not this answerer.

import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { Plan } from "../src/api.js";
import { engineVersion, run } from "../src/index.js";
import { CallFailed, CallRefused } from "../src/engine-errors.js";
import { makeRequest } from "../src/request.js";
import { socketPair, socketRoot } from "../src/socket.js";
import { Answer, DECLINE, type StandIn, type StandInCall } from "../src/stand-in.js";
import { protocolErrors } from "./json-schema.js";
import { SLOW, temporaryFolder } from "./support.js";

const NODE = spawnSync("node", ["--version"]).status === 0 && process.platform !== "win32";

const ENGINE = `#!/usr/bin/env node
const fs = require("node:fs");
const net = require("node:net");
const config = JSON.parse(fs.readFileSync(process.env.FAKE_STAND_IN_CONFIG, "utf8"));
const log = (value) => fs.appendFileSync(process.env.FAKE_STAND_IN_LOG, JSON.stringify(value) + "\\n");
const args = process.argv.slice(2);
log({ args, sdk_node: process.env.GRIDA_FX_SDK_NODE ?? null, sdk_host: process.env.GRIDA_FX_SDK_NODE_HOST ?? null });
const verb = args[0];
if (verb === "--version") {
  process.stdout.write("grida-fx 0.0.0-fake\\n");
  process.exit(0);
}
if (verb === "project") {
  process.stdout.write(JSON.stringify(config.projection));
  process.exit(0);
}
if (verb === "inspect") {
  process.stdout.write(JSON.stringify(config.inspection));
  process.exit(0);
}
if (verb !== "run" || !args.includes("--stand-in=-")) {
  process.exit(97);
}
let socket = false;
try {
  socket = fs.fstatSync(0).isSocket();
} catch {}
log({ socket });
if (!socket) {
  process.stderr.write("grida-fx: --stand-in - needs the stand-in's socket on standard input\\n");
  process.exit(2);
}
const sock = new net.Socket({ fd: 0, readable: true, writable: true });
const frameOf = (message) => {
  const body = Buffer.from(typeof message === "string" ? message : JSON.stringify(message), "utf8");
  return Buffer.concat([Buffer.from("Content-Length: " + body.length + "\\r\\n\\r\\n"), body]);
};
let buffer = Buffer.alloc(0);
const received = [];
const waiting = new Map();
sock.on("data", (chunk) => {
  buffer = Buffer.concat([buffer, chunk]);
  for (;;) {
    const end = buffer.indexOf("\\r\\n\\r\\n");
    if (end < 0) return;
    const length = Number(/Content-Length: *(\\d+)/i.exec(buffer.subarray(0, end).toString("latin1"))[1]);
    if (buffer.length < end + 4 + length) return;
    const message = JSON.parse(buffer.subarray(end + 4, end + 4 + length).toString("utf8"));
    buffer = buffer.subarray(end + 4 + length);
    log({ received: message });
    const wait = waiting.get(message.id);
    if (wait) {
      waiting.delete(message.id);
      wait(message);
    } else {
      received.push(message);
    }
  }
});
sock.on("error", () => {});
sock.on("close", () => {
  log({ closed: true });
  for (const wait of waiting.values()) wait(null);
  waiting.clear();
});
const reply = (id) =>
  new Promise((resolve) => {
    const found = received.findIndex((message) => message.id === id);
    if (found >= 0) resolve(received.splice(found, 1)[0]);
    else waiting.set(id, resolve);
  });
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
(async () => {
  for (const step of config.steps) {
    if (step.send !== undefined) sock.write(frameOf(step.send));
    if (step.wait !== undefined) await Promise.race([reply(step.wait), sleep(10000)]);
    if (step.sleep !== undefined) await sleep(step.sleep);
  }
  process.stdout.write(config.stdout ?? "");
  process.exit(config.status ?? 0);
})();
`;

const PROJECTION = {
  instances: {},
  run: { run_started: { stand_in: true }, run_finished: { ok: true, incomplete: false, outputs: {}, failed: [], charged_usd: 0 } },
};
const INSPECTION = { run: { workflow: "wf", folder: "runs/wf-1", state: "succeeded", charged_usd: 0, steps: [] } };

/** A fake engine in a folder of its own: its options for the SDK, its config and its log. */
function fake() {
  const folder = temporaryFolder("stand-in-run");
  const engine = join(folder.path, "fake-engine");
  const config = join(folder.path, "config.json");
  const logFile = join(folder.path, "log.jsonl");
  writeFileSync(engine, ENGINE);
  chmodSync(engine, 0o755);
  return {
    ...folder,
    options: { cwd: folder.path, env: { GRIDA_FX_BIN: engine, FAKE_STAND_IN_CONFIG: config, FAKE_STAND_IN_LOG: logFile } },
    configure(steps: unknown[], over: Record<string, unknown> = {}) {
      writeFileSync(
        config,
        JSON.stringify({ steps, stdout: "run       runs/wf-1\n", projection: PROJECTION, inspection: INSPECTION, ...over }),
      );
    },
    log(): Record<string, unknown>[] {
      return existsSync(logFile)
        ? readFileSync(logFile, "utf8")
            .trim()
            .split("\n")
            .map((line) => JSON.parse(line))
        : [];
    },
    /** What the answerer sent, by the id it answered. */
    replies(): Map<unknown, Record<string, unknown>> {
      const replies = new Map<unknown, Record<string, unknown>>();
      for (const entry of this.log()) {
        if (entry.received !== undefined) {
          const message = entry.received as Record<string, unknown>;
          replies.set(message.id, message);
        }
      }
      return replies;
    },
  };
}

const INITIALIZE = {
  protocol: "fx-node-protocol-v1",
  engine: { name: "grida-fx", version: "0.0.0-fake" },
  project_root: "/nonexistent",
  sources: [],
};

function ask(prompt: string, key = "a".repeat(64)): Record<string, unknown> {
  return {
    capability: "image.generate",
    route: { id: "img-a@acme", fingerprint: "e".repeat(64) },
    request: { prompt, size: "64x64" },
    take: [1],
    key,
    instance: { id: "draw#1", path: "draw", step: "draw" },
    files: {},
  };
}

const request = (id: number | string, method: string, params?: unknown) => ({
  send: { jsonrpc: "2.0", id, method, ...(params === undefined ? {} : { params }) },
});
const notify = (method: string, params?: unknown) => ({
  send: { jsonrpc: "2.0", method, ...(params === undefined ? {} : { params }) },
});

/** The session's end: shutdown, then exit. */
const END = [request("end", "shutdown"), { wait: "end" }, notify("exit"), { sleep: 50 }];

const answer: StandIn = async (call: StandInCall) => {
  const prompt = String(call.request.prompt);
  if (prompt === "decline") {
    return DECLINE;
  }
  if (prompt === "refuse") {
    throw new CallRefused("no lighthouses here");
  }
  if (prompt === "fail") {
    throw new CallFailed("the lighthouse fell");
  }
  if (prompt === "json") {
    return Answer.json({ take: call.take });
  }
  if (prompt === "wait") {
    await new Promise((done) => call.signal.addEventListener("abort", done));
    return DECLINE;
  }
  return { files: { image: new TextEncoder().encode("picture") } };
};

describe.skipIf(!NODE)("run with a stand-in", () => {
  test(
    "the engine's standard input is the stand-in's socket, and its calls are answered",
    async () => {
      const engine = fake();
      try {
        engine.configure([
          request(1, "initialize", INITIALIZE),
          { wait: 1 },
          request(2, "stand_in.answer", ask("a lighthouse")),
          request(3, "stand_in.answer", ask("json")),
          request(4, "stand_in.answer", ask("decline")),
          request(5, "stand_in.answer", ask("refuse")),
          request(6, "stand_in.answer", ask("fail")),
          request(7, "stand_in.answer", ask("wait")),
          { wait: 6 },
          notify("$/cancel", { id: 7 }),
          { wait: 7 },
          ...END,
        ]);
        const result = await run("wf.yaml", { ...engine.options, standIn: answer });
        expect(result.standIn).toBe(true);
        expect(result.ok).toBe(true);
        const log = engine.log();
        // A stand-in run is not planned first.
        expect(log.filter((entry) => entry.args !== undefined).map((entry) => (entry.args as string[])[0])).toEqual([
          "run",
          "project",
          "inspect",
        ]);
        expect(log[0]?.args).toEqual(["run", "wf.yaml", "--stand-in=-", "--no-view"]);
        expect(log.find((entry) => entry.socket !== undefined)?.socket).toBe(true);
        const replies = engine.replies();
        const initialized = replies.get(1)?.result;
        expect(protocolErrors(initialized, "initialize_result")).toEqual([]);
        expect((initialized as { host: { language: string } }).host.language).toBe("typescript");
        const results = [2, 3, 4].map((id) => replies.get(id)?.result);
        for (const value of results) {
          expect(protocolErrors(value, "stand_in_answer_result")).toEqual([]);
        }
        expect(results).toEqual([
          { files: { image: { base64: Buffer.from("picture").toString("base64") } }, data: null },
          { files: {}, data: { json: { take: 1 } } },
          { decline: true },
        ]);
        expect([5, 6, 7].map((id) => replies.get(id)?.error)).toEqual([
          { code: -32015, message: "no lighthouses here" },
          { code: -32016, message: "the lighthouse fell" },
          { code: -32002, message: "the engine cancelled stand_in.answer" },
        ]);
        expect(replies.get("end")).toEqual({ jsonrpc: "2.0", id: "end", result: null });
      } finally {
        engine.cleanup();
      }
    },
    SLOW,
  );

  test(
    "the session's own refusals",
    async () => {
      const engine = fake();
      try {
        engine.configure([
          { send: "{not json" },
          request(1, "stand_in.answer", ask("x")),
          { wait: 1 },
          { send: { jsonrpc: "2.0", id: true, method: "initialize", params: INITIALIZE } },
          request(2, "initialize", { ...INITIALIZE, protocol: "fx-node-protocol-v9" }),
          { wait: 2 },
          request(3, "initialize", INITIALIZE),
          { wait: 3 },
          request(4, "initialize", INITIALIZE),
          { send: { jsonrpc: "2.0", id: 99, result: {} } },
          notify("unknown"),
          request(5, "describe", { targets: [], builtins: false }),
          request(6, "stand_in.answer", { ...ask("x"), take: [] }),
          { wait: 6 },
          ...END,
        ]);
        await run("wf.yaml", { ...engine.options, standIn: answer });
        const received = engine
          .log()
          .filter((entry) => entry.received !== undefined)
          .map((entry) => entry.received);
        expect(received).toEqual([
          { jsonrpc: "2.0", id: null, error: { code: -32700, message: expect.stringContaining("the message is not JSON") } },
          { jsonrpc: "2.0", id: 1, error: { code: -32600, message: "stand_in.answer came before initialize" } },
          { jsonrpc: "2.0", id: null, error: { code: -32600, message: "a request id is an integer or a string" } },
          {
            jsonrpc: "2.0",
            id: 2,
            error: {
              code: -32003,
              message: expect.stringMatching(/^the engine speaks fx-node-protocol-v9 and @grida\/fx \S+ speaks fx-node-protocol-v1$/),
              data: { engine_protocol: "fx-node-protocol-v9", host_protocol: "fx-node-protocol-v1" },
            },
          },
          { jsonrpc: "2.0", id: 3, result: expect.objectContaining({ protocol: "fx-node-protocol-v1" }) },
          { jsonrpc: "2.0", id: 4, error: { code: -32600, message: "initialize was already answered" } },
          { jsonrpc: "2.0", id: 5, error: { code: -32601, message: "@grida/fx's stand-in has no method describe" } },
          { jsonrpc: "2.0", id: 6, error: { code: -32602, message: "stand_in.answer's take is a list of take numbers" } },
          { jsonrpc: "2.0", id: "end", result: null },
        ]);
      } finally {
        engine.cleanup();
      }
    },
    SLOW,
  );

  test(
    "a stand-in that fails stops the run, and run rejects with what it threw",
    async () => {
      const engine = fake();
      try {
        const steps = [
          request(1, "initialize", INITIALIZE),
          { wait: 1 },
          request(2, "stand_in.answer", ask("x")),
          { wait: 2 },
          request(3, "stand_in.answer", ask("y")),
          { wait: 3 },
          ...END,
        ];
        engine.configure(steps, { status: 1 });
        const thrown = new RangeError("the stand-in broke");
        let asked = 0;
        const breaks: StandIn = () => {
          asked += 1;
          throw thrown;
        };
        await expect(run("wf.yaml", { ...engine.options, standIn: breaks })).rejects.toBe(thrown);
        expect(asked).toBe(1);
        const replies = engine.replies();
        expect(replies.get(2)?.error).toEqual({ code: -32099, message: "RangeError: the stand-in broke" });
        expect(replies.get(3)?.error).toEqual({ code: -32099, message: "the stand-in failed earlier" });
        // A return value that is no answer is the stand-in's fault too, and so is its TypeError.
        engine.configure(steps, { status: 1, stdout: "" });
        const rejected = run("wf.yaml", { ...engine.options, standIn: (() => undefined) as never });
        await expect(rejected).rejects.toBeInstanceOf(TypeError);
        await expect(rejected).rejects.toThrow("a stand-in returns an answer or DECLINE, not undefined");
      } finally {
        engine.cleanup();
      }
    },
    SLOW,
  );

  test(
    "what a stand-in run cannot be is refused before anything starts",
    async () => {
      const engine = fake();
      try {
        engine.configure([]);
        for (const [options, message] of [
          [{ live: true }, "a stand-in run is never live: pass live: false, or no standIn"],
          [{ yesUpTo: 1 }, "a stand-in run runs every phase: yesUpTo does not apply"],
        ] as const) {
          await expect(run("wf.yaml", { ...engine.options, ...options, standIn: answer })).rejects.toThrow(new TypeError(message));
        }
        await expect(run("wf.yaml", { ...engine.options, standIn: "answer" as never })).rejects.toThrow(
          new TypeError("standIn is a function of one StandInCall, not a string"),
        );
        expect(engine.log()).toEqual([]);
      } finally {
        engine.cleanup();
      }
    },
    SLOW,
  );

  test(
    "a plan runs with a stand-in",
    async () => {
      const engine = fake();
      try {
        engine.configure([request(1, "initialize", INITIALIZE), { wait: 1 }, ...END]);
        const planned = new Plan({ problems: [] } as never, {} as never, makeRequest("wf.yaml", engine.options));
        const result = await run(planned, { standIn: answer });
        expect(result.standIn).toBe(true);
        expect(engine.log()[0]?.args).toEqual(["run", "wf.yaml", "--stand-in=-", "--no-view"]);
      } finally {
        engine.cleanup();
      }
    },
    SLOW,
  );

  test(
    "the engine learns the Node running the SDK only when it is Node; an explicit env wins",
    async () => {
      const engine = fake();
      try {
        engine.configure([]);
        await engineVersion(engine.options);
        const [first] = engine.log();
        // Under Bun the SDK's process is not a Node the engine may host with; from src/ there is
        // no compiled host program of its own.
        const inherited = (name: string) => process.env[name] ?? null;
        expect(first?.sdk_node).toBe(process.versions.bun === undefined ? process.execPath : inherited("GRIDA_FX_SDK_NODE"));
        expect(first?.sdk_host).toBe(inherited("GRIDA_FX_SDK_NODE_HOST"));
        await engineVersion({
          ...engine.options,
          env: { ...engine.options.env, GRIDA_FX_SDK_NODE: "/chosen/node", GRIDA_FX_SDK_NODE_HOST: undefined },
        });
        const second = engine.log()[1];
        expect([second?.sdk_node, second?.sdk_host]).toEqual(["/chosen/node", null]);
      } finally {
        engine.cleanup();
      }
    },
    SLOW,
  );
});

describe.skipIf(process.platform === "win32")("the socket pair", () => {
  /** Sends `text` from `from` and reads it on `to`. */
  const passes = (from: import("node:net").Socket, to: import("node:net").Socket, text: string) =>
    new Promise<string>((done) => {
      to.resume();
      to.once("data", (chunk: Buffer) => done(chunk.toString("utf8")));
      from.write(text);
    });

  test.each([
    ["the temporary folder", 10],
    ["/tmp, when a socket path there would be too long", 200],
  ])("is made in %s, and leaves nothing behind", async (_where, length) => {
    const folder = { path: mkdtempSync("/tmp/gfx-"), cleanup: () => rmSync(folder.path, { recursive: true, force: true }) };
    const before = process.env.TMPDIR;
    try {
      const root = join(folder.path, "x".repeat(length));
      mkdirSync(root);
      process.env.TMPDIR = root;
      expect(socketRoot()).toBe(length < 100 ? root : "/tmp");
      const { ours, theirs } = await socketPair();
      try {
        expect(await passes(ours, theirs, "to the engine")).toBe("to the engine");
        expect(await passes(theirs, ours, "to the stand-in")).toBe("to the stand-in");
      } finally {
        ours.destroy();
        theirs.destroy();
      }
      expect(readdirSync(root)).toEqual([]);
    } finally {
      if (before === undefined) {
        delete process.env.TMPDIR;
      } else {
        process.env.TMPDIR = before;
      }
      folder.cleanup();
    }
  });
});
