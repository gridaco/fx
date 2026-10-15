// The TypeScript node host serving a project adapter (src/host.ts `serveAdapters`;
// spec/protocol.md §5.8 and §6.8): `adapter.load` on a planning host and its failures, calls
// answered at once with their `fetch` round trips, faults and `$/cancel`. The cases follow
// python/tests/test_host.py's adapter tests.

import { afterEach, describe, expect, test } from "bun:test";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  expectValid,
  HOST_SKIP,
  HOST_TEST,
  type HostProcess,
  type Message,
  makeProject,
  start,
  stopAll,
} from "./host-support.js";

afterEach(stopAll);

const ADAPTER = `import {
  type AdapterCall,
  type AdapterFile,
  Answered,
  adapter,
  type CollectCall,
  type CollectOutcome,
  FetchError,
  type SendOutcome,
  type SubmitOutcome,
} from "@grida/fx";
import { PALETTE } from "./helpers.ts";

console.log("an adapter module prints too");

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

async function send(call: AdapterCall): Promise<SendOutcome> {
  const prompt = call.request.prompt;
  if (prompt === "fetch") {
    try {
      const response = await call.fetch(\`\${call.baseUrl}/draw\`, { method: "POST", json: { prompt }, credential: true });
      return Answered.json({ status: response.status, body: response.json() as string }, { costUsd: 0.01 });
    } catch (error) {
      if (error instanceof FetchError) {
        return error.phase === "refused"
          ? { outcome: "refused", reason: error.reason }
          : { outcome: "failed", reason: error.reason, retryable: true };
      }
      throw error;
    }
  }
  if (prompt === "wait") {
    await new Promise((done) => call.signal.addEventListener("abort", done));
    return { outcome: "refused", reason: "too late" };
  }
  if (prompt === "refuse") {
    return { outcome: "refused", reason: "no kites today" };
  }
  if (prompt === "busy") {
    return { outcome: "not_received", reason: "acme is busy", retryAfterS: 1 };
  }
  if (prompt === "boom") {
    throw new RangeError("acme broke");
  }
  if (prompt === "stray") {
    return { outcome: "refused", reason: "x", cost_usd: 1 } as unknown as SendOutcome;
  }
  const references = (call.request.references ?? []) as AdapterFile[];
  const picture = Buffer.concat(await Promise.all(references.map((file) => file.bytes())));
  return { outcome: "answered", files: { image: picture }, data: { palette: PALETTE, take: call.take } };
}

async function submit(call: AdapterCall): Promise<SubmitOutcome> {
  if (call.request.prompt === "fetch") {
    const response = await call.fetch(\`\${call.baseUrl}/jobs\`, { method: "POST", json: {} });
    return { outcome: "accepted", handle: { status: response.status } };
  }
  await sleep(200);
  if (call.request.prompt === "lost") {
    return { outcome: "uncertain", reason: "acme did not answer" };
  }
  return { outcome: "accepted", handle: { job: \`j-\${call.attempt}\` } };
}

function collect(call: CollectCall): CollectOutcome {
  if (call.handle.job === "j-2") {
    return { outcome: "ended", reason: "the job was cancelled", costUsd: 0 };
  }
  return Answered.json({ collected: call.handle.job ?? null });
}

export const acme = adapter({
  requests: ["structured.generate", "image.generate"],
  jobs: ["acme.render"],
  send,
  submit,
  collect,
  version: 2,
});

export const NOT_AN_ADAPTER = 3;
`;

const ACME = { path: "adapters/acme.ts", attribute: "acme" };

/** A project with an adapter module that imports a module beside it. */
function project(): string {
  return makeProject({
    "adapters/acme.ts": ADAPTER,
    "adapters/helpers.ts": 'export const PALETTE = ["teal"];\n',
    "adapters/broken.ts":
      'import { adapter } from "@grida/fx";\n\nexport const acme = adapter({ requests: ["image.generate"] } as never);\n',
    "nodes/echo.ts":
      'import { node } from "@grida/fx";\n\nexport const echo = node({ name: "echo", outputs: {} }, () => undefined);\n',
  });
}

function call(prompt: string, capability = "structured.generate", over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    call_id: "c1",
    adapter: ACME,
    capability,
    route: {
      id: "text-a@acme",
      model: "text-a",
      provider: "acme",
      contract: { adapter: "adapters/acme.ts#acme", adapter_behavior: 2 },
    },
    base_url: "https://api.acme.test/v1",
    request: { prompt },
    files: {},
    take: [1],
    key: "a".repeat(64),
    attempt: 1,
    ...over,
  };
}

const RESULTS: Record<string, string> = {
  "adapter.send": "adapter_send_result",
  "adapter.submit": "adapter_submit_result",
  "adapter.collect": "adapter_collect_result",
};

function outcome(method: string, answer: Message): unknown {
  if (answer.result === undefined) {
    throw new Error(`an error: ${JSON.stringify(answer)}`);
  }
  expectValid(answer.result, RESULTS[method] as string);
  return answer.result;
}

/** Sends a request with the id `id`. */
function ask(host: HostProcess, id: number, method: string, params: unknown): void {
  host.send({ jsonrpc: "2.0", id, method, params });
}

describe.skipIf(HOST_SKIP !== null)("the TypeScript adapter host", () => {
  test(
    "adapter.load on a planning host, which goes on describing",
    async () => {
      const root = project();
      const host = start(root);
      await host.initialize();
      const loaded = await host.request("adapter.load", ACME);
      expectValid(loaded.result, "adapter_load_result");
      expect(loaded.result).toEqual({
        adapter: {
          version: 2,
          capabilities: { "structured.generate": "request", "image.generate": "request", "acme.render": "job" },
        },
        closure: [
          { label: "adapters/acme.ts", path: join(root, "adapters", "acme.ts") },
          { label: "adapters/helpers.ts", path: join(root, "adapters", "helpers.ts") },
        ],
      });
      // Loaded once per session, and the planning host goes on describing.
      expect((await host.request("adapter.load", ACME)).result).toEqual(loaded.result);
      const described = await host.describe([{ path: "nodes/echo.ts", attribute: "echo" }]);
      expect(described.modules[0]?.error).toBeUndefined();
      const refused = async (path: unknown, attribute: unknown) =>
        (await host.request("adapter.load", { path, attribute })).error;
      expect(await refused("adapters/acme.ts", "NOT_AN_ADAPTER")).toEqual({
        code: -32004,
        message: "adapters/acme.ts: NOT_AN_ADAPTER is not an export declared with adapter()",
      });
      expect(await refused("adapters/acme.ts", "missing")).toEqual({
        code: -32004,
        message: "adapters/acme.ts: missing is not an export declared with adapter()",
      });
      expect(await refused("adapters/broken.ts", "acme")).toEqual({
        code: -32004,
        message: "adapters/broken.ts failed to import: TypeError: adapter(): requests need send",
      });
      expect(await refused("adapters/none.ts", "acme")).toEqual({
        code: -32004,
        message: "no module file adapters/none.ts",
      });
      expect((await refused("../adapters/acme.ts", "acme"))?.code).toBe(-32602);
      expect(await refused("adapters/acme.ts", "not a name")).toEqual({
        code: -32602,
        message: "adapter.load's attribute is a name",
      });
      expect(await host.shutdown()).toBe(0);
      expect(host.stderr).toContain("an adapter module prints too");
    },
    HOST_TEST,
  );

  test(
    "a stand-in host refuses adapter.load",
    async () => {
      const root = project();
      writeFileSync(join(root, "stand_in.ts"), "export function answer() { return null; }\n");
      const host = start(root);
      await host.initialize();
      expect((await host.request("stand_in.load", { path: join(root, "stand_in.ts"), function: "answer" })).result).toEqual(
        {},
      );
      expect((await host.request("adapter.load", ACME)).error).toEqual({
        code: -32600,
        message: "this host answers for a stand-in",
      });
    },
    HOST_TEST,
  );

  test(
    "an adapter host serves calls at once, each fetch carrying its call_id",
    async () => {
      const root = project();
      const host = start(root);
      await host.initialize();
      // A call for an adapter the session has not loaded.
      expect((await host.request("adapter.send", call("a kite"))).error).toEqual({
        code: -32602,
        message: "no adapter adapters/acme.ts#acme is loaded on this host",
      });
      expect((await host.request("adapter.load", ACME)).result).toBeDefined();

      // A call that fetches waits for the engine's answer, while another is answered.
      ask(host, 100, "adapter.send", call("fetch"));
      const fetch = await host.next();
      expect(fetch.method).toBe("fetch");
      expect(fetch.params).toEqual({
        call_id: "c1",
        method: "POST",
        url: "https://api.acme.test/v1/draw",
        body: { json: { prompt: "fetch" } },
        lane: "provider",
        credential: true,
      });
      const refused = await host.request("adapter.send", call("refuse", "structured.generate", { call_id: "c2" }));
      expect(outcome("adapter.send", refused)).toEqual({ outcome: "refused", reason: "no kites today" });
      const body = Buffer.from('"d-1"').toString("base64");
      host.send({ jsonrpc: "2.0", id: fetch.id, result: { status: 200, headers: [["x-acme", "1"]], base64: body } });
      const answered = await host.next();
      expect(answered.id).toBe(100);
      expect(outcome("adapter.send", answered)).toEqual({
        outcome: "answered",
        files: {},
        data: { json: { status: 200, body: "d-1" } },
        cost_usd: 0.01,
      });
      // An exchange the engine refused: the adapter maps its phase onto its outcome.
      ask(host, 101, "adapter.send", call("fetch"));
      const refusedFetch = await host.next();
      const failure = { phase: "refused", reason: "the network is off (GRIDA_FX_NETWORK=off)" };
      host.send({ jsonrpc: "2.0", id: refusedFetch.id, result: { error: failure } });
      expect(outcome("adapter.send", await host.next())).toEqual({
        outcome: "refused",
        reason: "the network is off (GRIDA_FX_NETWORK=off)",
      });

      // A request's files are read from their store copies.
      writeFileSync(join(root, "kite.png"), "\x89PNG kite", "latin1");
      const digest = "d".repeat(64);
      const file = { digest, kind: "image/png", size: 9, path: join(root, "kite.png") };
      const drawn = await host.request(
        "adapter.send",
        call("draw", "image.generate", {
          request: { prompt: "draw", references: [{ file: digest }] },
          files: { [digest]: file },
          take: [1, 2],
        }),
      );
      expect(outcome("adapter.send", drawn)).toEqual({
        outcome: "answered",
        files: { image: { base64: Buffer.from("\x89PNG kite", "latin1").toString("base64") } },
        data: { palette: ["teal"], take: 2 },
      });
      expect(outcome("adapter.send", await host.request("adapter.send", call("busy")))).toEqual({
        outcome: "not_received",
        reason: "acme is busy",
        retry_after_s: 1,
      });

      // Two submits at once: neither waits for the other.
      const started = Date.now();
      ask(host, 110, "adapter.submit", call("render", "acme.render", { attempt: 1 }));
      ask(host, 111, "adapter.submit", call("render", "acme.render", { attempt: 2 }));
      const submitted = new Map<unknown, Message>();
      for (const answer of [await host.next(), await host.next()]) {
        submitted.set(answer.id, answer);
      }
      expect(Date.now() - started).toBeLessThan(390);
      expect(outcome("adapter.submit", submitted.get(110) as Message)).toEqual({
        outcome: "accepted",
        handle: { job: "j-1" },
      });
      expect((outcome("adapter.submit", submitted.get(111) as Message) as { handle: unknown }).handle).toEqual({
        job: "j-2",
      });
      ask(host, 112, "adapter.submit", call("fetch", "acme.render", { call_id: "c9" }));
      const submitFetch = await host.next();
      expect([submitFetch.method, (submitFetch.params as { call_id: string }).call_id]).toEqual(["fetch", "c9"]);
      host.send({ jsonrpc: "2.0", id: submitFetch.id, result: { status: 202, headers: [], base64: "" } });
      expect(outcome("adapter.submit", await host.next())).toEqual({ outcome: "accepted", handle: { status: 202 } });
      expect(outcome("adapter.submit", await host.request("adapter.submit", call("lost", "acme.render")))).toEqual({
        outcome: "uncertain",
        reason: "acme did not answer",
      });
      const collected = await host.request("adapter.collect", call("render", "acme.render", { handle: { job: "j-1" } }));
      expect(outcome("adapter.collect", collected)).toEqual({
        outcome: "answered",
        files: {},
        data: { json: { collected: "j-1" } },
      });
      const ended = await host.request("adapter.collect", call("render", "acme.render", { handle: { job: "j-2" } }));
      expect(outcome("adapter.collect", ended)).toEqual({
        outcome: "ended",
        reason: "the job was cancelled",
        cost_usd: 0,
      });

      // A capability the adapter does not serve with the method, and params of another shape.
      expect((await host.request("adapter.send", call("x", "acme.render"))).error).toEqual({
        code: -32602,
        message: "adapters/acme.ts#acme does not serve acme.render with adapter.send",
      });
      expect((await host.request("adapter.collect", call("x", "structured.generate", { handle: {} }))).error).toEqual({
        code: -32602,
        message: "adapters/acme.ts#acme does not serve structured.generate with adapter.collect",
      });
      expect((await host.request("adapter.send", call("x", "structured.generate", { attempt: 0 }))).error).toEqual({
        code: -32602,
        message: "adapter.send's attempt is from 1 to 6",
      });
      expect((await host.request("adapter.send", call("x", "structured.generate", { handle: {} }))).error).toEqual({
        code: -32602,
        message: "adapter.send has no handle",
      });

      // A fault ends its own call, logged with its stack; the next call is served.
      expect((await host.request("adapter.send", call("boom"))).error).toEqual({
        code: -32099,
        message: "RangeError: acme broke",
      });
      expect((await host.request("adapter.send", call("stray"))).error).toEqual({
        code: -32099,
        message: "TypeError: Refused holds outcome and reason, not 'cost_usd'",
      });
      expect(outcome("adapter.send", await host.request("adapter.send", call("refuse")))).toEqual({
        outcome: "refused",
        reason: "no kites today",
      });
      expect((await host.request("shutdown")).result).toBeNull();
      expect((await host.request("adapter.send", call("refuse"))).error).toEqual({
        code: -32600,
        message: "adapter.send came after shutdown",
      });
      host.notify("exit");
      expect((await host.ended()).code).toBe(0);
      const log = host.stderr;
      expect(log).toContain("@grida/fx node host: the adapter adapters/acme.ts#acme failed:\nRangeError: acme broke\n");
      expect(log).toContain("adapters/acme.ts:");
      expect(log).not.toContain("adapter.js");
    },
    HOST_TEST,
  );

  test(
    "$/cancel answers a call cancelled at once, and aborts its signal",
    async () => {
      const host = start(project());
      await host.initialize();
      expect((await host.request("adapter.load", ACME)).result).toBeDefined();
      const cancelled = { code: -32002, message: "the engine cancelled adapter.send" };
      // A call waiting on its signal: answered cancelled at once, and what it returns dropped.
      ask(host, 200, "adapter.send", call("wait"));
      await new Promise((done) => setTimeout(done, 50));
      host.notify("$/cancel", { id: 200 });
      expect(await host.next()).toEqual({ jsonrpc: "2.0", id: 200, error: cancelled });
      // A call the engine drops while its fetch is pending: the engine answers the fetch
      // `cancelled`, and nothing more is answered for the call.
      ask(host, 201, "adapter.send", call("fetch"));
      const fetch = await host.next();
      expect(fetch.method).toBe("fetch");
      host.notify("$/cancel", { id: 201 });
      expect(await host.next()).toEqual({ jsonrpc: "2.0", id: 201, error: cancelled });
      host.send({ jsonrpc: "2.0", id: fetch.id, error: { code: -32002, message: "the adapter call was dropped" } });
      // The next answer is the next call's.
      expect(outcome("adapter.send", await host.request("adapter.send", call("refuse")))).toEqual({
        outcome: "refused",
        reason: "no kites today",
      });
      // A submit cancelled while it sleeps: answered at once; what it returns later is dropped.
      ask(host, 202, "adapter.submit", call("render", "acme.render"));
      host.notify("$/cancel", { id: 202 });
      host.notify("$/cancel", { id: 999 }); // names nothing: ignored
      expect(await host.next()).toEqual({
        jsonrpc: "2.0",
        id: 202,
        error: { code: -32002, message: "the engine cancelled adapter.submit" },
      });
      await new Promise((done) => setTimeout(done, 400));
      expect(host.pending()).toBe(0);
      expect(outcome("adapter.send", await host.request("adapter.send", call("busy")))).toMatchObject({
        outcome: "not_received",
      });
      expect(host.stderr).not.toContain("failed:");
      expect(await host.shutdown()).toBe(0);
    },
    HOST_TEST,
  );
});
