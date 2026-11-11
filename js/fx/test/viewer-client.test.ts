import { describe, expect, test } from "bun:test";
import { ViewerProtocol as Protocol } from "../src/viewer-protocol.js";
import {
  ViewerClient,
  ViewerClientError,
  type ViewerContext,
  type ViewerMessageEvent,
  type ViewerSourceWindow,
  type ViewerTargetWindow,
} from "../src/viewer/client.js";

const ORIGIN = "http://127.0.0.1:8787";
const context = (sampled_at = 10): ViewerContext => ({
  subject: { kind: "run", workflow: { id: "acme", source: "workflows/acme.yaml" },
    reference: { kind: "service_entry", project_id: "a".repeat(64), entry_kind: "run", entry_id: "b".repeat(64) } },
  scope_id: null,
  selection: { kind: "instance", instance_id: "prepare#1", path: "prepare", step: "prepare", take: [1] },
  sampled_at,
});

class Source implements ViewerSourceWindow {
  readonly listeners = new Set<(event: ViewerMessageEvent) => void>();
  addEventListener(_type: "message", listener: (event: ViewerMessageEvent) => void) { this.listeners.add(listener); }
  removeEventListener(_type: "message", listener: (event: ViewerMessageEvent) => void) { this.listeners.delete(listener); }
  emit(data: unknown, source: unknown, origin = ORIGIN) {
    for (const listener of [...this.listeners]) listener({ data, source, origin });
  }
}
class Page implements ViewerTargetWindow {
  readonly requests: Protocol.Request[] = [];
  readonly origins: string[] = [];
  handle: ((request: Protocol.Request) => void) | undefined;
  constructor(readonly source: Source) {}
  postMessage(value: unknown, origin: string) {
    if (!Protocol.isRequest(value)) throw new Error("Invalid outgoing request");
    this.requests.push(value);
    this.origins.push(origin);
    this.handle?.(value);
  }
  reply(request: Protocol.Request, result: ViewerContext | null = context()) {
    this.source.emit({ channel: Protocol.CHANNEL, version: 1, type: "response", request_id: request.request_id, ok: true, result }, this);
  }
  fail(request: Protocol.Request, code = "not_ready") {
    this.source.emit({ channel: Protocol.CHANNEL, version: 1, type: "response", request_id: request.request_id,
      ok: false, error: { code, message: "The requested content is unavailable." } }, this);
  }
  update(value: ViewerContext) {
    this.source.emit({ channel: Protocol.CHANNEL, version: 1, type: "context_changed", context: value }, this);
  }
  last(): Protocol.Request { return this.requests.at(-1)!; }
  count(method: string) { return this.requests.filter((request) => request.method === method).length; }
}
function setup(timeoutMs = 1_000) {
  const source = new Source();
  const page = new Page(source);
  const client = new ViewerClient({ targetWindow: page, origin: ORIGIN, sourceWindow: source, timeoutMs });
  return { source, page, client };
}
async function microtasks() { for (let i = 0; i < 8; i++) await Promise.resolve(); }

describe("viewer protocol validation", () => {
  test("unknown methods and versions remain recognizable for explicit refusal", () => {
    const request = { channel: "fx_viewer", version: 2, type: "request", request_id: "r1", method: "new_method", params: {} };
    expect(Protocol.isRequest(request)).toBe(true);
    expect(Protocol.isMethod(request.method)).toBe(false);
    expect(Protocol.isParams(request.method, request.params)).toBe(false);
    expect(Protocol.isRequest({ ...request, request_id: "" })).toBe(false);
    expect(Protocol.isRequest({ ...request, params: [] })).toBe(false);
  });

  test("context distinguishes recorded instance, scope, pending repeat and boundary", () => {
    for (const selection of [
      context().selection,
      { kind: "scope", scope_id: "nested#1" },
      { kind: "pending_repeat", path: "batch" },
      { kind: "scope_boundary", scope_id: "nested#1", side: "input" },
      null,
    ]) expect(Protocol.isContext({ ...context(), selection })).toBe(true);
    expect(Protocol.isContext({ ...context(), subject: { kind: "run", reference: { kind: "export", manifest_sha256: "c".repeat(64) } } })).toBe(true);
    expect(Protocol.isContext({ ...context(), subject: { kind: "catalog" }, selection: null })).toBe(true);
  });

  test("malformed identities and private source paths are refused before reaching consumers", () => {
    for (const source of ["/private/project/fx.yaml", "C:/private/fx.yaml", "file:secret", "https://example.test/fx.yaml", "workflows\\acme.yaml", "bad\nfile"]) {
      expect(Protocol.isContext({ ...context(), subject: { kind: "run", workflow: { id: "acme", source } } })).toBe(false);
    }
    expect(Protocol.isContext({ ...context(), selection: { kind: "instance", instance_id: "" } })).toBe(false);
    expect(Protocol.isContext({ ...context(), selection: { kind: "instance", instance_id: "prepare#1", take: [0] } })).toBe(false);
    expect(Protocol.isContext({ ...context(), selection: { kind: "instance", instance_id: "prepare\n#1" } })).toBe(false);
    expect(Protocol.isContext({ ...context(), subject: { kind: "plan", reference: context().subject.reference } })).toBe(false);
    expect(Protocol.isContext({ ...context(), sampled_at: NaN })).toBe(false);
    expect(Protocol.isContext({ ...context(), retained: false })).toBe(false);
    expect(Protocol.isContext({ ...context(), subject: { kind: "run", reference: { kind: "export", manifest_sha256: "not-a-digest" } } })).toBe(false);
  });

  test("known methods validate only their own action arguments", () => {
    expect(Protocol.isParams("present", { action: "open_scope", scope_id: null })).toBe(true);
    expect(Protocol.isParams("present", { action: "reveal_instance", instance_id: "prepare#1" })).toBe(true);
    expect(Protocol.isParams("get_context", {})).toBe(true);
    expect(Protocol.isParams("get_context", { action: "fit" })).toBe(false);
    expect(Protocol.isAction({ action: "fit", instance_id: "prepare#1" })).toBe(false);
    expect(Protocol.isAction({ action: "run", target: "acme" })).toBe(false);
    expect(Protocol.isAction({ action: "open_scope" })).toBe(false);
  });
});

describe("browser viewer client", () => {
  test("a request uses exact origin and accepts only its target, origin, channel and reply ID", async () => {
    const { client, page, source } = setup();
    const reply = client.getContext();
    const request = page.last();
    const message = { channel: "fx_viewer", version: 1, type: "response", request_id: request.request_id, ok: true, result: context(20) };
    source.emit(message, {}, ORIGIN);
    source.emit(message, page, "https://other.test");
    source.emit({ ...message, channel: "other" }, page);
    source.emit({ ...message, request_id: "other" }, page);
    let settled = false;
    void reply.then(() => { settled = true; });
    await microtasks();
    expect(settled).toBe(false);
    page.reply(request, context(30));
    expect(await reply).toEqual(context(30));
    expect(page.origins).toEqual([ORIGIN]);
    client.dispose();
  });

  test("two windows sharing a message source cannot answer each other's calls", async () => {
    const source = new Source();
    const a = new Page(source);
    const b = new Page(source);
    const first = new ViewerClient({ targetWindow: a, sourceWindow: source, origin: ORIGIN });
    const second = new ViewerClient({ targetWindow: b, sourceWindow: source, origin: ORIGIN });
    const one = first.getContext();
    const two = second.getContext();
    expect(a.last().request_id).not.toBe(b.last().request_id);
    b.reply(a.last(), context(99));
    a.reply(b.last(), context(98));
    a.reply(a.last(), context(1));
    b.reply(b.last(), context(2));
    expect(await one).toEqual(context(1));
    expect(await two).toEqual(context(2));
    first.dispose();
    second.dispose();
    expect(source.listeners.size).toBe(0);
  });

  test("matching malformed replies fail explicitly instead of hanging", async () => {
    const { client, page, source } = setup();
    const result = client.getContext();
    const rejected = result.catch((error: unknown) => error);
    source.emit({ channel: "fx_viewer", version: 1, type: "response", request_id: page.last().request_id,
      ok: true, result: { ...context(), scope_id: 12 } }, page);
    expect(await rejected).toMatchObject({ code: "invalid_response" });
    client.dispose();
  });

  test("incompatible protocol and remote refusal remain typed errors", async () => {
    const { client, page, source } = setup();
    const incompatible = client.getContext();
    const rejected = incompatible.catch((error: unknown) => error);
    source.emit({ channel: "fx_viewer", version: 2, type: "response", request_id: page.last().request_id, ok: true, result: context() }, page);
    expect(await rejected).toMatchObject({ name: "ViewerClientError", code: "unsupported_version" });
    const refused = client.present({ action: "fit" });
    page.fail(page.last(), "unavailable");
    try { await refused; throw new Error("Expected refusal"); } catch (error) {
      expect(error).toBeInstanceOf(ViewerClientError);
      expect(error).toMatchObject({ code: "unavailable", message: "The requested content is unavailable." });
    }
    client.dispose();
  });

  test("presentation timeout leaves the outcome unconfirmed and never retries", async () => {
    const { client, page } = setup(10);
    await expect(client.present({ action: "reveal_instance", instance_id: "prepare#1" })).rejects.toMatchObject({ code: "timeout" });
    expect(page.count("present")).toBe(1);
    page.reply(page.requests[0]!);
    client.dispose();
    expect(page.count("present")).toBe(1);
  });

  test("concurrent local listeners share one wire subscription and retain separate snapshots", async () => {
    const { client, page } = setup();
    const a: ViewerContext[] = [];
    const b: ViewerContext[] = [];
    const first = client.subscribe((value) => { a.push(value); value.selection = null; });
    const second = client.subscribe((value) => b.push(value));
    expect(page.count("subscribe")).toBe(1);
    const subscribe = page.last();
    page.update(context(20));
    page.reply(subscribe, context(10));
    const [removeA, removeB] = await Promise.all([first, second]);
    expect(a.map((value) => value.sampled_at)).toEqual([20]);
    expect(b).toEqual([context(20)]);
    page.update(context(30));
    expect(b).toEqual([context(20), context(30)]);
    removeA();
    removeA();
    expect(page.count("unsubscribe")).toBe(0);
    page.update(context(40));
    expect(a.length).toBe(2);
    expect(b.at(-1)).toEqual(context(40));
    removeB();
    expect(page.count("unsubscribe")).toBe(1);
    page.reply(page.last(), null);
    await microtasks();
    client.dispose();
    expect(page.count("unsubscribe")).toBe(1);
  });

  test("listener exceptions do not prevent other listeners or cleanup", async () => {
    const { client, page } = setup();
    page.handle = (request) => page.reply(request, request.method === "unsubscribe" ? null : context());
    const seen: ViewerContext[] = [];
    const removeA = await client.subscribe(() => { throw new Error("Listener failed"); });
    const removeB = await client.subscribe((value) => seen.push(value));
    page.update(context(50));
    expect(seen).toEqual([context(), context(50)]);
    expect(page.count("subscribe")).toBe(1);
    removeA();
    removeB();
    await microtasks();
    client.dispose();
  });

  test("concurrent subscription refusal is shared and is not retried", async () => {
    const { client, page } = setup();
    const one = client.subscribe(() => {});
    const two = client.subscribe(() => {});
    const rejected = Promise.all([one.catch((error: unknown) => error), two.catch((error: unknown) => error)]);
    page.fail(page.last());
    for (const error of await rejected) expect(error).toMatchObject({ code: "not_ready" });
    expect(page.count("subscribe")).toBe(1);
    client.dispose();
  });

  test("new subscription waits for pending wire unsubscription", async () => {
    const { client, page } = setup();
    const first = client.subscribe(() => {});
    page.reply(page.last());
    const remove = await first;
    remove();
    const unsubscribe = page.last();
    const next = client.subscribe(() => {});
    expect(page.count("subscribe")).toBe(1);
    page.reply(unsubscribe, null);
    await microtasks();
    expect(page.count("subscribe")).toBe(2);
    page.reply(page.last(), context(60));
    const removeNext = await next;
    removeNext();
    page.reply(page.last(), null);
    await microtasks();
    client.dispose();
  });

  test("disposal settles all pending calls and removes local resources", async () => {
    const { client, page, source } = setup();
    const pending = client.getContext();
    const subscription = client.subscribe(() => {});
    const rejected = Promise.all([pending.catch((error: unknown) => error), subscription.catch((error: unknown) => error)]);
    client.dispose();
    for (const error of await rejected) expect(error).toMatchObject({ code: "disposed" });
    expect(source.listeners.size).toBe(0);
    expect(page.requests.every((request) => request.method !== "present")).toBe(true);
    expect(page.count("unsubscribe")).toBeGreaterThanOrEqual(1);
    const count = page.requests.length;
    client.dispose();
    expect(page.requests.length).toBe(count);
    await expect(client.getContext()).rejects.toMatchObject({ code: "disposed" });
  });

  test("origins must be exact HTTP(S) origins", () => {
    const source = new Source();
    const page = new Page(source);
    for (const origin of ["*", "null", "file:///viewer", "https://example.test/viewer", "https://user:secret@example.test", "https://example.test/"]) {
      expect(() => new ViewerClient({ targetWindow: page, sourceWindow: source, origin })).toThrow(ViewerClientError);
    }
    expect(source.listeners.size).toBe(0);
  });

  test("the client builds for browsers without importing the Node SDK or engine", async () => {
    const result = await Bun.build({ entrypoints: [new URL("../src/viewer/client.ts", import.meta.url).pathname], target: "browser" });
    expect(result.success).toBe(true);
    const output = await result.outputs[0]!.text();
    expect(output).not.toContain("node:");
    expect(output).not.toContain("GRIDA_FX_BIN");
    expect(output).not.toContain("host-entry");
  });
});
